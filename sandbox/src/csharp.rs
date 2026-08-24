// Runtime C# (ICorDebug via interop direto — ver com.rs). Diferente do Java
// (onde nsjail lança um processo Java separado que fala JDWP com o driver),
// aqui a lógica de debug roda DENTRO do nosso próprio binário Rust — porque
// dbgshim precisa manipular o processo debuggee diretamente via FFI.
//
// Por isso o padrão é "self re-exec": o processo externo (run_outer) faz
// fork+exec do nsjail apontando pra ELE MESMO de novo, mas com uma flag
// interna (--csharp-worker) que faz o binário, já dentro do jail, pular
// direto pra run_worker() em vez de tentar reabrir o nsjail recursivamente.
//
// Flags de nsjail já validadas empiricamente no spike (não inventar de novo):
//   --rlimit_fsize inf       (CoreCLR faz memfd_create("doublemapper") +
//                              ftruncate pra 2TB — reserva virtual, não disco
//                              real; sem isso o processo morre com SIGXFSZ)
//   --tmpfsmount /tmp        (o handshake de debug do CoreCLR cria socket/pipes
//                              em /tmp — se for read-only, EROFS e trava)
//   DOTNET_GCHeapHardLimit   (sem isso o CoreCLR tenta dimensionar o heap pela
//                              RAM total do host e estoura o rlimit_as)

use std::os::raw::{c_int, c_void};
use std::path::Path;
use std::process::Command;
use std::ptr;
use std::time::{Duration, Instant};

use libloading::Library;

use crate::com;
use crate::events::{self, Event, RunOutcome};

pub const CSHARP_WORKER_FLAG: &str = "--csharp-worker";

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

pub struct RunOptions {
    pub time_limit_secs: String,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            time_limit_secs: std::env::var("SPIKE_TIME_LIMIT").unwrap_or_else(|_| "15".into()),
        }
    }
}

/// Chamado pelo dispatcher normal (fora do jail): compila o projeto e faz
/// fork+exec do nsjail apontando pro nosso próprio binário com
/// `--csharp-worker`, que vai rodar run_worker() já isolado.
///
/// Fase 2 hardening: run via `events::run_nsjail` instead of a bare
/// `.status()` with everything inherited, so a `--time_limit` kill or a
/// cgroup OOM kill of the worker (which run_worker() itself has no chance
/// to react to — it just dies) still produces a clean `timeout`/
/// `memory_limit_exceeded` event on our stdout. CoreCLR's own stack
/// overflow behavior was tested empirically (see tasks.md "Fase 2") and,
/// unlike Java's clean `StackOverflowError`, crashes the process with a
/// raw SIGSEGV/SIGABRT and no reliably parseable stderr marker — so C#
/// stack overflows currently fall through as a generic non-zero exit
/// rather than a `stack_overflow` event (documented as an open item).
pub fn run_outer(dll_file: &Path, opts: &RunOptions) -> std::process::ExitStatus {
    let cwd = dll_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let self_exe = std::env::current_exe().expect("não achou o próprio binário");

    eprintln!("[sandbox-runner/csharp] rodando {dll_file:?} isolado via nsjail (self re-exec)...");

    let mut cmd = Command::new("nsjail");
    cmd.args([
        "--mode", "o",
        "--time_limit", &opts.time_limit_secs,
        "--keep_caps", // TODO(Fase 2): trocar por capabilities mínimas quando isso for resolvido
        "--rlimit_fsize", "inf",
        "--tmpfsmount", "/tmp",
        "--rlimit_as", "3072",
        "--rlimit_cpu", &opts.time_limit_secs,
        "--rlimit_nproc", "256",
        "--rlimit_nofile", "256",
        "--use_cgroupv2",
        "--cgroup_mem_max", "268435456",
        "--cgroup_pids_max", "256",
        "--chroot", "/",
        "--cwd", cwd.to_str().unwrap(),
        "--env", "DOTNET_ROOT=/usr/share/dotnet",
        "--env", "PATH=/usr/share/dotnet:/usr/bin:/bin",
        "--env", "DOTNET_GCHeapHardLimit=0x8000000",
        // NOT --quiet — see the identical comment in java.rs::run(): we
        // need nsjail's own INFO-level "run time >= time limit" log line to
        // tell a timeout kill apart from a cgroup OOM kill (both produce
        // the exact same exit code otherwise).
        "--",
    ])
    .arg(self_exe)
    .args([CSHARP_WORKER_FLAG, "--dll", dll_file.to_str().unwrap()]);

    let result = events::run_nsjail(cmd);
    match result.outcome {
        RunOutcome::TimedOut => events::emit(&Event::Timeout),
        RunOutcome::LikelyOom => events::emit(&Event::MemoryLimitExceeded),
        RunOutcome::OutputTruncated => events::emit(&Event::OutputTruncated),
        RunOutcome::Normal => {}
    }
    result.status
}

/// Chamado quando o binário já está DENTRO do nsjail (detectado via
/// --csharp-worker no dispatcher). Porta a lógica validada em
/// src/icordebug_spike.rs (attach via dbgshim, ICorDebug, breakpoint no
/// método de entrada, stepping) pro caminho de produção: em vez de parar
/// depois de um número fixo de passos e só imprimir em stderr, cada
/// StepComplete de verdade (ver com.rs::cb_step_complete) emite um
/// `events::Event::Step` via stdout — o modelo de produto é trace-and-replay
/// (grava a execução inteira, sem amostragem aqui). O cap de 5.000 eventos
/// (`events::STEP_EVENT_CAP`) já está implementado em com.rs, mesma decisão
/// de escopo do lado Java.
pub fn run_worker(dll_file: &Path) -> i32 {
    let cwd = dll_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // Sinks que os callbacks de com.rs usam pra emitir eventos de verdade
    // (indireção necessária porque com.rs é compartilhado com o binário
    // legado icordebug-spike, que não tem o módulo `events` — ver comentário
    // em com.rs junto de STEP_SINK/ERROR_SINK).
    unsafe {
        com::STEP_SINK = Some(|line, locals, stack| {
            events::emit(&Event::Step {
                line,
                locals,
                stack,
                time_ns: None,
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
    let run_timeout_secs: u64 = std::env::var("SPIKE_TIME_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);
    let run_deadline = Instant::now() + Duration::from_secs(run_timeout_secs);
    loop {
        if unsafe { com::PROCESS_EXITED } {
            break;
        }
        if Instant::now() > run_deadline {
            return 1;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Same architectural gap found and fixed on the Java side (see
    // Debugger.java): `pid` here is the DEBUGGEE (launched via dbgshim's
    // CreateProcessForLaunch as a real OS child of this process), separate
    // from run_worker's own process. A cgroup OOM kill lands on whichever
    // process actually holds the memory — the debuggee, not run_worker —
    // so nsjail's own exit-code/signal-based detection one level up
    // (events::run_nsjail in run_outer(), which only watches nsjail's
    // DIRECT child, i.e. THIS process) would never see it: ICorDebug's
    // ExitProcess callback (cb_exit_process) only sets PROCESS_EXITED,
    // with no exit code, so we'd otherwise return 0 with no trace of what
    // happened. Fix: reap the debuggee ourselves via a raw waitpid (no new
    // dependency — same "no binding crate, hand-rolled FFI" style already
    // used throughout com.rs) and check whether it died by SIGKILL.
    unsafe {
        let mut status: c_int = 0;
        let rc = waitpid(pid as c_int, &mut status, WNOHANG);
        eprintln!(
            "[DEBUG csharp] waitpid(pid={pid}) rc={rc} status={status} last_os_error={:?}",
            std::io::Error::last_os_error()
        );
        if rc > 0 && wifsignaled(status) && wtermsig(status) == SIGKILL {
            events::emit(&Event::MemoryLimitExceeded);
        }
    }

    if unsafe { com::FATAL_ERROR } {
        return 1;
    }
    0
}

// Raw waitpid FFI — deliberately not pulling in the `libc` crate for one
// function, matching this file's/com.rs's existing style of hand-rolled FFI
// declarations instead of binding crates.
extern "C" {
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
}
const WNOHANG: c_int = 1;
const SIGKILL: c_int = 9;

fn wifsignaled(status: c_int) -> bool {
    // Linux/glibc macro: WIFSIGNALED(status) = ((signed char)(((status) & 0x7f) + 1) >> 1) > 0
    (((status & 0x7f) + 1) as i8 >> 1) > 0
}

fn wtermsig(status: c_int) -> c_int {
    status & 0x7f
}
