// Chamado quando o binário já está DENTRO do nsjail (detectado via
// --csharp-worker no dispatcher, csharp/mod.rs). Porta a lógica validada em
// src/icordebug_spike.rs (attach via dbgshim, ICorDebug, breakpoint no
// método de entrada, stepping) pro caminho de produção: em vez de parar
// depois de um número fixo de passos e só imprimir em stderr, cada
// StepComplete de verdade (ver com/callback/stepping.rs::cb_step_complete)
// emite um `events::Event::Step` via stdout — o modelo de produto é
// trace-and-replay (grava a execução inteira, sem amostragem aqui). O cap
// de 5.000 eventos (`events::STEP_EVENT_CAP`) já está implementado em
// com/callback/stepping.rs, mesma decisão de escopo do lado Java.

use std::os::raw::{c_int, c_void};
use std::path::Path;
use std::ptr;
use std::time::{Duration, Instant};

use libloading::Library;

use crate::com;
use crate::events::{self, Event};
use crate::pdb::PortablePdb;

/// Caminho fixo: única cópia de libdbgshim.so na imagem, empacotada junto
/// com o netcoredbg (confirmado via `find / -iname libdbgshim*` na imagem
/// sandbox-spike) — o SDK do .NET puro não inclui essa lib.
const LIBDBGSHIM_PATH: &str = "/usr/share/netcoredbg/netcoredbg/libdbgshim.so";

type HResult = i32;
type Handle = *mut c_void;
type DWord = u32;
type WChar = u16;

type CreateProcessForLaunchFn = unsafe extern "C" fn(
    *mut WChar,
    c_int,
    *mut c_void,
    *const WChar,
    *mut DWord,
    *mut Handle,
) -> HResult;
type ResumeProcessFn = unsafe extern "C" fn(Handle) -> HResult;
type StartupCallback = extern "C" fn(*mut c_void, *mut c_void, HResult);
type RegisterForRuntimeStartupFn =
    unsafe extern "C" fn(DWord, StartupCallback, *mut c_void, *mut *mut c_void) -> HResult;

static mut CALLBACK_FIRED: bool = false;
static mut CALLBACK_HR: HResult = 0;
static mut P_CORDB: *mut c_void = ptr::null_mut();
// Loaded once up front in run_worker (single dll/execution per process —
// same lifetime assumption as com.rs's other statics), then read from
// com::LOCAL_NAME_RESOLVER's plain-fn callback (no captured state allowed,
// see the comment on LOCAL_NAME_RESOLVER in com/callback/mod.rs), hence a
// static instead of a local variable threaded through.
static mut PDB: Option<PortablePdb> = None;
// Set once at the top of run_worker, read from the STEP_SINK closure below
// to fill Event::Step's time_ns — same reason as PDB above: STEP_SINK is a
// plain `fn` pointer (com::STEP_SINK's declared type), so it can't capture
// a local `t0` the way jdi/Debugger.java's t0 is a plain local variable.
// Mirrors Java's "elapsed since the driver started" semantics.
static mut RUN_START: Option<Instant> = None;

extern "C" fn startup_callback(p_cordb: *mut c_void, _parameter: *mut c_void, hr: HResult) {
    unsafe {
        CALLBACK_FIRED = true;
        CALLBACK_HR = hr;
        P_CORDB = p_cordb;
    }
}

fn to_utf16(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn emit_error(message: impl Into<String>) -> i32 {
    let message = message.into();
    eprintln!("[sandbox-runner/csharp/worker] erro: {message}");
    events::emit(&Event::Error { message });
    1
}

pub fn run_worker(dll_file: &Path) -> i32 {
    let cwd = dll_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // Sinks que os callbacks de com.rs usam pra emitir eventos de verdade
    // (indireção necessária porque com.rs é compartilhado com o binário
    // legado icordebug-spike, que não tem o módulo `events` — ver comentário
    // em com/callback/mod.rs junto de STEP_SINK/ERROR_SINK).
    unsafe {
        RUN_START = Some(Instant::now());
        com::STEP_SINK = Some(|line, locals, stack| {
            let time_ns = RUN_START.map(|t0| t0.elapsed().as_nanos() as u64);
            events::emit(&Event::Step {
                line,
                locals,
                stack,
                time_ns,
                memory_bytes: None,
            });
        });
        com::ERROR_SINK = Some(|message| {
            eprintln!("[sandbox-runner/csharp/worker] erro (callback): {message}");
            events::emit(&Event::Error { message });
        });
        com::LIMIT_SINK = Some(|| {
            events::emit(&Event::StepLimitExceeded);
        });
        com::PROCESS_EXITED = false;
        com::FATAL_ERROR = false;
        com::STEP_EVENTS_EMITTED = 0;
        com::STEP_CAPPED = false;
        com::STEP_EVENTS_TOTAL = 0;
        // Parity port of jdi/Debugger.java's `spike.sample` (see
        // com::SAMPLE_N/STEP_EVENTS_TOTAL and cb_step_complete's doc
        // comment). Same SPIKE_SAMPLE env var name Java already uses, for
        // cross-language consistency (see RunOptions::sample_n's doc
        // comment) — deliberately NOT renamed to something more
        // production-sounding here; see tasks.md for that scope decision.
        // Read directly from the environment (not threaded through as a fn
        // arg) because run_worker is invoked from main.rs's dispatcher with
        // just `--dll`, no RunOptions — same pattern CSHARP_TIME_LIMIT_SECS
        // already uses a few lines below for the inner deadline. run_outer
        // forwards it explicitly via `--env SPIKE_SAMPLE=...` (see that
        // function), so it's guaranteed present here inside the jail.
        com::SAMPLE_N = std::env::var("SPIKE_SAMPLE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1)
            .max(1);

        // PDB is optional: `dotnet build -c Debug` produces one right next
        // to the dll (see ProcessSandboxRunner.compileCsharp on the API
        // side), but if it's missing/unparseable for any reason,
        // LOCAL_NAME_RESOLVER/LINE_RESOLVER stay None and extract_locals/
        // cb_step_complete fall back to the positional "local_N" naming and
        // raw IL offset that always worked before either resolver existed.
        PDB = PortablePdb::load(dll_file);
        com::LOCAL_NAME_RESOLVER = if (*std::ptr::addr_of!(PDB)).is_some() {
            Some(|token, offset| {
                (*std::ptr::addr_of!(PDB)).as_ref().map(|p| p.locals_for(token, offset)).unwrap_or_default()
            })
        } else {
            None
        };
        // Same PDB, same static-fn-pointer indirection as LOCAL_NAME_RESOLVER
        // right above (com::LINE_RESOLVER's own doc comment explains why a
        // plain `fn` is required here) — resolves (method token, IL offset)
        // to a real C# source line via the PDB's SequencePoints blob (see
        // pdb.rs::PortablePdb::line_for). Registered only when a PDB
        // actually loaded, same condition as the locals resolver, since
        // both read from the same `PDB` static.
        com::LINE_RESOLVER = if (*std::ptr::addr_of!(PDB)).is_some() {
            Some(|token, offset| (*std::ptr::addr_of!(PDB)).as_ref().and_then(|p| p.line_for(token, offset)))
        } else {
            None
        };
        // Same PDB/indirection again, this time for line-GRANULAR stepping
        // (see com::STEP_RANGE_RESOLVER's doc comment): resolves (method
        // token, current IL offset) -> the IL range of the covering sequence
        // point, via pdb.rs::PortablePdb::step_range_for. This is what lets
        // com.rs's arm_step call ICorDebugStepper::StepRange instead of
        // plain Step, so StepComplete fires once per SOURCE LINE instead of
        // once per IL instruction — the actual fix for the "same line
        // highlighted N times in a row" disclaimer (see tasks.md). Same
        // None-when-no-PDB condition as the two resolvers above, since it
        // reads from the same PDB static.
        com::STEP_RANGE_RESOLVER = if (*std::ptr::addr_of!(PDB)).is_some() {
            Some(|token, offset| (*std::ptr::addr_of!(PDB)).as_ref().and_then(|p| p.step_range_for(token, offset)))
        } else {
            None
        };
    }

    let lib = match unsafe { Library::new(LIBDBGSHIM_PATH) } {
        Ok(l) => l,
        Err(e) => return emit_error(format!("falha ao carregar libdbgshim.so ({LIBDBGSHIM_PATH}): {e}")),
    };

    let create_process_for_launch: libloading::Symbol<CreateProcessForLaunchFn> =
        match unsafe { lib.get(b"CreateProcessForLaunch\0") } {
            Ok(s) => s,
            Err(e) => return emit_error(format!("símbolo CreateProcessForLaunch não encontrado: {e}")),
        };
    let resume_process: libloading::Symbol<ResumeProcessFn> = match unsafe { lib.get(b"ResumeProcess\0") } {
        Ok(s) => s,
        Err(e) => return emit_error(format!("símbolo ResumeProcess não encontrado: {e}")),
    };
    let register_for_runtime_startup: libloading::Symbol<RegisterForRuntimeStartupFn> =
        match unsafe { lib.get(b"RegisterForRuntimeStartup\0") } {
            Ok(s) => s,
            Err(e) => return emit_error(format!("símbolo RegisterForRuntimeStartup não encontrado: {e}")),
        };

    let cmdline = format!("/usr/share/dotnet/dotnet {}", dll_file.to_str().unwrap_or_default());
    let mut cmdline_w = to_utf16(&cmdline);
    let cwd_w = to_utf16(cwd.to_str().unwrap_or("."));

    let mut pid: DWord = 0;
    let mut resume_handle: Handle = ptr::null_mut();
    let hr = unsafe {
        create_process_for_launch(
            cmdline_w.as_mut_ptr(),
            1, // bSuspendProcess = TRUE
            ptr::null_mut(),
            cwd_w.as_ptr(),
            &mut pid,
            &mut resume_handle,
        )
    };
    if hr != com::S_OK {
        return emit_error(format!("CreateProcessForLaunch falhou: hr=0x{:08x}", hr as u32));
    }

    let mut startup_token: *mut c_void = ptr::null_mut();
    let hr = unsafe {
        register_for_runtime_startup(pid, startup_callback, ptr::null_mut(), &mut startup_token)
    };
    if hr != com::S_OK {
        return emit_error(format!("RegisterForRuntimeStartup falhou: hr=0x{:08x}", hr as u32));
    }

    let hr = unsafe { resume_process(resume_handle) };
    if hr != com::S_OK {
        return emit_error(format!("ResumeProcess falhou: hr=0x{:08x}", hr as u32));
    }

    let attach_deadline = Instant::now() + Duration::from_secs(10);
    while !unsafe { CALLBACK_FIRED } {
        if Instant::now() > attach_deadline {
            return emit_error("timeout esperando o callback de RegisterForRuntimeStartup (10s)");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let startup_hr = unsafe { CALLBACK_HR };
    if startup_hr != com::S_OK {
        return emit_error(format!(
            "callback de RegisterForRuntimeStartup retornou hr=0x{:08x}",
            startup_hr as u32
        ));
    }

    let p_cordb = unsafe { P_CORDB };
    let icordebug_ptr = match unsafe { com::query_interface(p_cordb, &com::IID_ICORDEBUG) } {
        Ok(p) => p,
        Err(hr) => return emit_error(format!("QueryInterface(IID_ICORDEBUG) falhou: hr=0x{:08x}", hr as u32)),
    };

    let cordebug = com::CorDebug(icordebug_ptr);
    let hr = unsafe { cordebug.initialize() };
    if hr != com::S_OK {
        return emit_error(format!("ICorDebug::Initialize falhou: hr=0x{:08x}", hr as u32));
    }

    // ManagedCallbackObj precisa viver até o fim da sessão de debug — Box
    // "vazado" de propósito (processo inteiro morre no fim do worker de
    // qualquer forma, dentro do nsjail).
    let callback_obj = Box::new(com::ManagedCallbackObj {
        vtbl: &com::MANAGED_CALLBACK_VTBL,
        // ICorDebugManagedCallback2 — mandatory, see
        // IID_ICORDEBUG_MANAGED_CALLBACK2's doc comment in com/mod.rs for
        // why (Cordb::SetManagedHandler itself fails with E_NOINTERFACE
        // without it, for any CoreCLR >= 2.0 debuggee).
        vtbl2: &com::MANAGED_CALLBACK2_VTBL,
        ref_count: 0,
    });
    let callback_ptr = Box::into_raw(callback_obj) as *mut c_void;

    let hr = unsafe { cordebug.set_managed_handler(callback_ptr) };
    if hr != com::S_OK {
        return emit_error(format!("ICorDebug::SetManagedHandler falhou: hr=0x{:08x}", hr as u32));
    }

    if let Err(hr) = unsafe { cordebug.debug_active_process(pid) } {
        return emit_error(format!("ICorDebug::DebugActiveProcess falhou: hr=0x{:08x}", hr as u32));
    }

    // A partir daqui, toda a inspeção/emite-evento acontece dentro dos
    // callbacks (com.rs) — só esperamos o processo terminar (ExitProcess).
    //
    // This used to also race its own userspace deadline against nsjail's
    // `--time_limit` and emit Event::Timeout itself if it won — but that's
    // exactly backwards: nsjail's own --time_limit SIGKILLs THIS process
    // (run_worker is the direct jailed child), so whichever fires first
    // wins unpredictably, and if nsjail wins we're dead before finishing
    // the emit anyway. Timeout detection now lives one level up, in
    // run_outer() (via events::run_nsjail), which is NOT jailed and so
    // reliably survives to observe nsjail's own kill and emit a clean event
    // — see run_outer's doc comment. This inner deadline stays only as a
    // safety net against a hang that isn't actually a --time_limit kill
    // (e.g. stuck waiting on ExitProcess for some other reason); it just
    // returns non-zero without emitting, since run_outer can't tell that
    // apart from a normal crash either way.
    //
    // Renamed from SPIKE_TIME_LIMIT to CSHARP_TIME_LIMIT_SECS, same as
    // RunOptions::default above — but note this read is UNAFFECTED by that
    // rename in practice: this line runs INSIDE the jail, and nsjail does
    // NOT forward the parent's environment by default (confirmed
    // empirically, see tasks.md "Cap de 5.000 eventos" / "Throttling"
    // finding on SPIKE_TIME_LIMIT's pre-existing non-forwarding — this var
    // was never added to run_outer's `--env` allowlist, unlike SPIKE_SAMPLE
    // which was fixed). So this always falls back to the hardcoded default
    // below regardless of what's set on the host — a known, documented gap,
    // not fixed by this task (would require adding `--env` forwarding here
    // too, changing scope from "pick a value" to "fix env propagation").
    // The fallback default is kept in sync with RunOptions::default's 15s
    // (see that comment for the real measurement behind the number) so that,
    // even though this specific read can't be tuned via the env var today,
    // its hardcoded value still matches the deliberately-chosen production
    // default rather than silently drifting from it.
    let run_timeout_secs: u64 = std::env::var("CSHARP_TIME_LIMIT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);
    let run_deadline = Instant::now() + Duration::from_secs(run_timeout_secs);
    loop {
        if unsafe { com::PROCESS_EXITED } {
            break;
        }
        // Checked here (not just after the loop, as FATAL_ERROR used to be)
        // so a callback-detected fatal condition — e.g. the multi-thread
        // block below — ends the run immediately instead of idling up to
        // the full deadline waiting for a PROCESS_EXITED that may never
        // come (the debuggee is still running when we detect this).
        if unsafe { com::FATAL_ERROR } {
            return 1;
        }
        if Instant::now() > run_deadline {
            return 1;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Perf line, same shape/purpose as jdi/Debugger.java's final "[perf] ..."
    // line — lets sampling be validated the same way on both languages
    // (eventosTotais = every StepComplete callback, emitidos = only the
    // ones that actually paid extraction+emission cost per SAMPLE_N).
    unsafe {
        // Copy out of the `static mut`s into locals before formatting (not
        // `eprintln!("...", com::SAMPLE_N, ...)` directly) to avoid taking a
        // live shared reference into a mutable static, which 2024-edition
        // rustc warns on (`static_mut_refs`) — same non-issue in practice
        // (single-threaded at this point, nothing else writes these once
        // the debug session has ended) but no reason to introduce a new
        // warning the rest of this file doesn't already have.
        let elapsed_ms = RUN_START.map(|t0| t0.elapsed().as_millis() as u64).unwrap_or(0).max(1);
        let sample_n = com::SAMPLE_N;
        let total = com::STEP_EVENTS_TOTAL;
        let emitted = com::STEP_EVENTS_EMITTED;
        eprintln!(
            "[perf] sampleN={} eventosTotais={} emitidos={} tempo={}ms taxaTotal={:.1} ev/s taxaEmitida={:.1} ev/s",
            sample_n,
            total,
            emitted,
            elapsed_ms,
            total as f64 * 1000.0 / elapsed_ms as f64,
            emitted as f64 * 1000.0 / elapsed_ms as f64,
        );
    }

    // KNOWN GAP (documented in tasks.md, not fixed): `pid` here is the
    // DEBUGGEE (launched via dbgshim's CreateProcessForLaunch), a separate
    // OS process from run_worker itself. A cgroup OOM kill lands on
    // whichever process actually holds the memory — plausibly the
    // debuggee, not run_worker — in which case nsjail's own exit-code/
    // signal-based detection one level up (events::run_nsjail in
    // run_outer(), which only watches nsjail's DIRECT child, i.e. THIS
    // process) would see nothing wrong.
    //
    // The equivalent Java fix (Debugger.java checking
    // `vm.process().exitValue()` after VMDeathEvent) doesn't translate
    // here: tried reaping the debuggee ourselves via a raw `waitpid(pid,
    // ..., WNOHANG)` (pid is a real OS child of this process) both right
    // after ExitProcess and after falling through run_deadline. Tested
    // empirically by SIGKILLing the debuggee directly (same delivery
    // mechanism a cgroup OOM kill uses, from a separate process, mid-run):
    // ICorDebug's ExitProcess callback never fired at all (waited the full
    // 15s deadline, confirmed via logging — not just a reap race), and the
    // follow-up waitpid() also found nothing to reap. Whatever dbgshim uses
    // internally to track the debuggee doesn't surface this to our minimal
    // ICorDebugManagedCallback implementation, and there's no other
    // ICorDebug vtable slot implemented so far (or found) that exposes the
    // debuggee's raw exit status. So: memory_limit_exceeded for C# only
    // fires today when the OOM kill happens to land on run_worker itself
    // (covered by run_outer's events::run_nsjail, same as Timeout) — an
    // OOM kill that specifically targets the debuggee process is currently
    // indistinguishable from any other unexplained hang and just falls
    // through as a generic non-zero exit.
    if unsafe { com::FATAL_ERROR } {
        return 1;
    }
    0
}
