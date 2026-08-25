// ICorDebugManagedCallback: nossa implementação, exposta como objeto COM —
// 26 métodos + os 3 de IUnknown = 29 slots, na ordem exata do cordebug.idl,
// mais ICorDebugManagedCallback2 (11 métodos, ver ManagedCallback2Vtbl's doc
// comment). Assinaturas com *mut c_void genérico pra ponteiros de interface
// (não precisamos interpretar o tipo, só repassar/logar por enquanto).
//
// This is also where nearly every piece of `static mut` session state these
// callbacks share lives (FATAL_ERROR/ERROR_SINK are the two exceptions —
// see their doc comment in com/mod.rs for why) — set here, read here, and
// (for the sink/config ones) written externally by csharp::run_worker
// before the debug session starts (see com/mod.rs's own indirection note on
// why `fn` pointers rather than closures: this file is shared, via `mod
// com;`, with the legacy icordebug-spike binary, which has no `events`
// module to call directly).
//
// This module has its own submodule, `stepping`, holding the large
// (~500-line) stepping/inspection cluster (Breakpoint/StepComplete and
// everything they need to arm the next step and extract locals) — split
// out purely for size, since every private helper there is used only
// within that one cluster. Everything else — the vtable struct
// definitions, the plumbing (QueryInterface/AddRef/Release for both V1 and
// V2), and the ~20 "boring" callbacks that just call Continue() — stays
// here.

use std::os::raw::c_void;

mod stepping;

// See com/mod.rs's own `#[allow(unused_imports)]` note: this file (like
// com/mod.rs) is compiled twice — once as part of the lib crate, where this
// re-export genuinely is public API surface, and once inside the standalone
// icordebug-spike binary, which never names `extract_locals` directly, so
// the lint fires there only.
#[allow(unused_imports)]
pub use stepping::extract_locals;

use super::icordebug::{
    continue_, create_function_breakpoint, get_function_from_token, get_metadata_import, get_module_name,
    set_module_jmc_status,
};
use super::metadata::find_entry_point_token;
use super::{
    report_error, Guid, HResult, E_NOINTERFACE, IID_ICORDEBUG_MANAGED_CALLBACK, IID_ICORDEBUG_MANAGED_CALLBACK2,
    IID_IUNKNOWN, S_OK,
};

// Indireção de emissão de evento: os callbacks COM abaixo são `extern "C" fn`
// sem contexto arbitrário pra passar, e este arquivo é compartilhado (via
// `mod com;`) pelo binário legado icordebug-spike, que não tem o módulo
// `events` do crate da lib — então não dá pra chamar `crate::events::emit`
// direto daqui. Em vez disso, csharp::run_worker seta esses ponteiros de
// função (sem estado capturado, então cabem em `fn` puro) antes de iniciar a
// sessão de debug; o icordebug-spike nunca seta os sinks, então os callbacks
// simplesmente não emitem nada nele (comportamento antigo preservado).
pub static mut STEP_SINK: Option<fn(i64, std::collections::BTreeMap<String, serde_json::Value>, Vec<String>)> = None;
// Fires once, the moment the step cap below is reached, so run_worker can
// emit Event::StepLimitExceeded (same product-scope decision as the Java
// side — see jdi/Debugger.java and events::STEP_EVENT_CAP).
pub static mut LIMIT_SINK: Option<fn()> = None;
pub static mut PROCESS_EXITED: bool = false;
// Counts real step events emitted (not every StepComplete callback — only
// ones where inspection actually produced an event), same definition the
// Java side uses. Once it hits events::STEP_EVENT_CAP, cb_step_complete
// stops arming a new stepper and just lets the program run to completion
// uninstrumented, exactly like the JDI side.
pub static mut STEP_EVENTS_EMITTED: u32 = 0;
pub static mut STEP_CAPPED: bool = false;
// Sampling knob (parity port of jdi/Debugger.java's `spike.sample` /
// eventCount): counts EVERY StepComplete callback (not just the emitted
// ones — that's STEP_EVENTS_EMITTED above, a different, related counter),
// and only the callbacks where `STEP_EVENTS_TOTAL % SAMPLE_N == 0` pay the
// expensive locals/call-stack extraction + STEP_SINK emission cost. The
// stepper is still re-armed (create_stepper/step_into) on every single
// callback regardless of sampling — same semantics as Java, where sampling
// only skips emitStepEvent(), never the underlying StepRequest/resume
// protocol. Set once per run from csharp::run_worker (parsed from the
// SPIKE_SAMPLE env var, same name Java already uses — see csharp.rs).
pub static mut STEP_EVENTS_TOTAL: u32 = 0;
pub static mut SAMPLE_N: u32 = 1;
// Same indirection reason as STEP_SINK above (this file is shared with the
// legacy icordebug-spike binary, which has no `pdb` module to call
// directly) — csharp::run_worker sets this to a plain `fn` (no captured
// state, loads the PDB once up front) that maps (method token, IL offset)
// -> {slot index -> real variable name}. `None` (the icordebug-spike
// default, and csharp.rs's own fallback when no .pdb was found) means
// extract_locals keeps using positional `local_N` keys.
pub static mut LOCAL_NAME_RESOLVER: Option<fn(u32, u32) -> std::collections::BTreeMap<u32, String>> = None;
// Same indirection/lifecycle as LOCAL_NAME_RESOLVER immediately above (this
// file is shared with the legacy icordebug-spike binary, no `pdb` module
// available there either) — csharp::run_worker sets this from the SAME
// loaded PortablePdb, resolving (method token, IL offset) -> real C# source
// line via the SequencePoints blob (see pdb.rs::PortablePdb::line_for).
// `None` (the icordebug-spike default) OR a `Some` resolver returning `None`
// for a given (token, offset) — no .pdb, method not in it, offset falls in a
// "hidden" compiler-generated region — both mean cb_step_complete keeps
// using the raw IL offset as `line`, exactly like before this resolver
// existed; this is the same fallback philosophy LOCAL_NAME_RESOLVER already
// established for local_N names, just applied to the other half of the
// event schema.
pub static mut LINE_RESOLVER: Option<fn(u32, u32) -> Option<u32>> = None;
// Same indirection/lifecycle as LINE_RESOLVER immediately above, one level
// more specific: maps (method token, current IL offset) -> the half-open IL
// range `[start, end)` of the SEQUENCE POINT covering that offset — i.e.
// the exact source line's IL extent, per pdb.rs::PortablePdb::step_range_for
// (built on the same SequencePoints data LINE_RESOLVER already reads, just
// returning the covering point's range instead of only its line number).
// `None` (no PDB, method has no sequence point data, offset is inside a
// "hidden" compiler-generated region, or the resolver itself isn't set —
// e.g. icordebug-spike, which never sets it) means "can't compute a
// meaningful line range here" — `stepping::arm_step` then falls back to
// plain per-IL-instruction `Step`, same fallback philosophy as
// LINE_RESOLVER's own None case. Set once from csharp::run_worker (see that
// file), from the same loaded PortablePdb LINE_RESOLVER/LOCAL_NAME_RESOLVER
// already use.
//
// This is the actual fix for the "same line highlighted N times in a row"
// UX problem (see tasks.md): `ICorDebugStepper::StepRange` — unlike plain
// `Step`, which always completes at the very next IL instruction regardless
// of source line — does not fire StepComplete until execution leaves the
// given IL range, so arming it with the CURRENT line's full IL range
// produces exactly one StepComplete per SOURCE LINE, mirroring what JDI's
// `StepRequest.STEP_LINE` already gives Java for free (see cordebug.idl's
// own doc comment on StepRange, verified against the real dotnet/runtime
// source before writing this, same "don't guess the ABI" rule pdb.rs's
// SequencePoints parsing already followed).
pub static mut STEP_RANGE_RESOLVER: Option<fn(u32, u32) -> Option<(u32, u32)>> = None;

// Multi-thread event model decision (spec.md "Multi-thread", pending since
// Fase 1): blocked in the MVP, same choice made for Java (jdi/Debugger.java)
// — detected at runtime instead of a static source-code guess. Tracks every
// distinct ICorDebugThread pointer seen via CreateThread, in first-seen
// order (the first is the debuggee's own main thread, confirmed empirically
// against the real breakpoint's thread pointer).
//
// Unlike Java's JDI (which lets a ThreadGroup-name check cleanly tell real
// user threads apart from JVM housekeeping threads), a real, genuinely
// single-threaded C# program was empirically confirmed (see tasks.md) to
// still fire CreateThread TWICE — main, plus one CoreCLR-internal managed
// thread that appears right before ExitProcess, with no distinguishing
// property found on the ICorDebugThread handle itself to filter it out by
// kind. So the tolerance here is a plain count: up to 2 distinct threads
// (main + that one baseline extra) is normal; a 3rd distinct thread is
// treated as genuine user-created multi-thread code. Best-effort, not
// proven for every possible single-threaded program — same "simple over
// perfect" MVP tolerance already accepted for other heuristics in this
// codebase (e.g. the LikelyOom exit-code inference). Also independently
// justified by a second empirical finding: the existing worker does not
// correctly handle real concurrent user threads at all today (a manual run
// against a genuinely multi-threaded program hung to the full --time_limit
// timeout instead of stepping through/completing) — blocking early is
// strictly better than that silent hang, not just a matter of taste.
//
// KNOWN GAP, found empirically (repeated real runs against
// test-snippets-csharp/MultiThreadCs through the full API/Docker stack,
// not just a one-off): this detection is NOT fully reliable, unlike the
// Java side. In roughly 2 of 5 repeated runs, the stepper itself got stuck
// re-triggering StepComplete on the exact same IL offset inside
// `Thread.Start()`'s own CoreCLR implementation (`StartCore`) — thousands
// of identical step events, no forward progress — and this stall happens
// BEFORE a 3rd distinct thread's CreateThread callback ever fires, so
// SEEN_THREADS never crosses MAX_TOLERATED_THREADS and this block never
// triggers. This is a separate, deeper problem with single-stepping
// through OS-level thread-creation internals, not a bug in the counting
// logic here (confirmed: when CreateThread for the 3rd thread DOES fire
// before the stepper gets stuck, the block works correctly and quickly,
// ~0.2s). Net effect: this mitigation catches the common case and is
// strictly no worse than doing nothing (nsjail's own --time_limit still
// eventually kills the stuck case exactly as before this existed,
// producing a `timeout` event instead of a `stack_overflow`-would-be-
// clean-message one) — but does not guarantee a clean, fast block for
// every real multi-threaded C# program the way jdi/Debugger.java's
// equivalent does for Java. Investigating/fixing the stepper stall itself
// is out of scope here (see tasks.md).
static mut SEEN_THREADS: Vec<*mut c_void> = Vec::new();
const MAX_TOLERATED_THREADS: usize = 2;

// Real bug found and fixed while validating SequencePoints line resolution
// end-to-end (see tasks.md): `mdMethodDef` tokens (the `rid` half of
// `method_token`, everything LOCAL_NAME_RESOLVER/LINE_RESOLVER key their
// lookups by) are only unique WITHIN a single module — cb_step_complete
// fires for every single-step, including ones landed inside CoreCLR/BCL
// internals (System.Console, System.IO, ...), not just the user's own
// module. Confirmed empirically, with a real reproduction (not just a
// theoretical concern): stepping the `branching_loop` test program (see
// pdb.rs's fixture of the same name — it has a `Helper.TripleIt` method at
// rid 4) through a real `docker run`, a step landed inside a framework
// method called `CheckIo` (rid 4 too, in ITS OWN module — a real System.IO
// internal, confirmed via its position in the call stack) — before this
// fix, `LINE_RESOLVER`/`line_for` would have confidently returned
// `Some(24)` and `Some(27)` for that step: `Helper.TripleIt`'s REAL lines
// in Program.cs, entirely unrelated to what `CheckIo` was actually doing.
// A plausible-but-wrong line number is worse than the old raw-IL-offset
// fallback (which was at least honestly not-a-line-number) — so both
// resolvers are now only consulted when the current frame's function is
// confirmed to be in the SAME module the PDB was loaded for. Set once, the
// first time `cb_load_module` identifies the user's own module (see that
// function below) — by the time any step can possibly fire, that module
// has necessarily already loaded, so this is
// always populated before `cb_step_complete` (com/callback/stepping.rs)
// needs it.
pub(super) static mut USER_MODULE: *mut c_void = std::ptr::null_mut();

#[repr(C)]
pub struct ManagedCallbackVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub breakpoint: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub step_complete: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void, i32) -> HResult,
    pub break_: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub exception: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, i32) -> HResult,
    pub eval_complete: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub eval_exception: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub create_process: unsafe extern "C" fn(*mut c_void, *mut c_void) -> HResult,
    pub exit_process: unsafe extern "C" fn(*mut c_void, *mut c_void) -> HResult,
    pub create_thread: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub exit_thread: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub load_module: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub unload_module: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub load_class: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub unload_class: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub debugger_error: unsafe extern "C" fn(*mut c_void, *mut c_void, i32, u32) -> HResult,
    pub log_message:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, i32, *mut c_void, *mut c_void) -> HResult,
    pub log_switch: unsafe extern "C" fn(
        *mut c_void,
        *mut c_void,
        *mut c_void,
        i32,
        u32,
        *mut c_void,
        *mut c_void,
    ) -> HResult,
    pub create_app_domain: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub exit_app_domain: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub load_assembly: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub unload_assembly: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub control_c_trap: unsafe extern "C" fn(*mut c_void, *mut c_void) -> HResult,
    pub name_change: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub update_module_symbols:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub edit_and_continue_remap:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void, i32) -> HResult,
    pub breakpoint_set_error:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void, u32) -> HResult,
}

// ICorDebugManagedCallback2's own vtable (method order/signatures straight
// from cordebug.idl, same "don't guess the ABI" discipline as everywhere
// else in this file) — see IID_ICORDEBUG_MANAGED_CALLBACK2's doc comment
// (com/mod.rs) for why this is mandatory, not optional. Every method here
// just calls Continue() on the relevant controller (pAppDomain, or pProcess
// for the Connection notifications), same "the driver doesn't act on this
// specific event, just resume" behavior every uninteresting V1 callback
// already has (see e.g. cb_name_change) — this project doesn't do
// Edit-and-Continue, MDAs, or multi-process/connection scenarios, so
// there's nothing more meaningful to do with any of these; the one V2
// callback that DOES matter (Exception, the JMC "user first chance"
// notification) gets the exact same Continue()-only behavior real V1
// cb_exception already has, which is all this codebase currently acts on
// exceptions.
#[repr(C)]
pub struct ManagedCallback2Vtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub function_remap_opportunity:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void, *mut c_void, u32) -> HResult,
    pub create_connection: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *const u16) -> HResult,
    pub change_connection: unsafe extern "C" fn(*mut c_void, *mut c_void, u32) -> HResult,
    pub destroy_connection: unsafe extern "C" fn(*mut c_void, *mut c_void, u32) -> HResult,
    pub exception:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void, u32, i32, u32) -> HResult,
    pub exception_unwind: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, i32, u32) -> HResult,
    pub function_remap_complete: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> HResult,
    pub mda_notification: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> HResult,
}

#[repr(C)]
pub struct ManagedCallbackObj {
    pub vtbl: *const ManagedCallbackVtbl,
    // Second interface (ICorDebugManagedCallback2) via the standard COM
    // "vtable multiple inheritance" trick: this field's own ADDRESS (not
    // `vtbl` above) is what cb_query_interface hands back when asked for
    // IID_ICORDEBUG_MANAGED_CALLBACK2 (see that function) — the caller then
    // treats that address as a `ICorDebugManagedCallback2*` and dereferences
    // ITS first 8 bytes to find this vtable, exactly the way it dereferences
    // `vtbl` above to find the V1 one. MUST stay the second field (straight
    // after `vtbl`) for cb_query_interface's `offset_of!` math below to be
    // correct — moving it would silently break the interface identity this
    // whole mechanism depends on.
    pub vtbl2: *const ManagedCallback2Vtbl,
    pub ref_count: u32,
}

// FIX (ver tasks.md/git log — travamento em exceção C# não tratada,
// investigação com símbolos de debug reais do CoreCLR resolvidos via
// dotnet-symbol): esta função costumava aceitar QUALQUER riid sem checar,
// sempre devolvendo `this` — "spike: aceita qualquer IID (não checa),
// suficiente pro runtime aceitar nosso objeto como ICorDebugManagedCallback".
// Isso é uma violação real do contrato COM (QueryInterface DEVE recusar
// interfaces não implementadas) com uma consequência concreta e confirmada
// via gdb + símbolos reais: mscordbi.so, ao inicializar, faz
// `pCallback->QueryInterface(IID_ICorDebugManagedCallback2, ...)` (código
// real em dotnet/runtime's src/coreclr/debug/di/process.cpp) pra descobrir
// se o cliente also implementa a interface V2 (FunctionRemapOpportunity/
// CreateConnection/ChangeConnection/DestroyConnection/Exception(6 args)/
// ExceptionUnwind/FunctionRemapComplete/MDANotification — cordebug.idl).
// Como essa função sempre respondia S_OK com o MESMO ponteiro de vtable
// (moldado só pro layout de ICorDebugManagedCallback V1), mscordbi passava
// a acreditar que tínhamos ManagedCallback2 e, pro evento "JMC user first
// chance exception" (Debugger::SendExceptionEventsWorker, ramo
// `pDebugMethodInfo->IsJMCFunction()` — exatamente o nosso caso, já que
// setamos JMC no módulo do usuário), invocava o slot 7 do vtable V2
// (Exception, 6 args) — que no NOSSO vtable (moldado só pra V1) é
// `eval_complete` (4 args, sempre retorna S_OK sem nunca chamar
// Continue()). Resultado: `Debugger::SendExceptionHelperAndBlock` já tinha
// chamado `TrapAllRuntimeThreads()` esperando um Continue() que nunca
// chegava — deadlock genuíno do CoreCLR em
// `Thread::RareDisablePreemptiveGC` -> `Thread::WaitSuspendEvents`,
// confirmado via backtrace resolvido com símbolos de debug reais do
// libcoreclr.so (dotnet-symbol, ver tasks.md). Fix: checar riid de
// verdade, só aceitar IUnknown e o IID real de ICorDebugManagedCallback
// (não os V2/V3 que este código não implementa) — mscordbi então sabe que
// não pode contar com V2/V3 e não tenta mais essa chamada corrompida.
unsafe extern "C" fn cb_query_interface(
    this: *mut c_void,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> HResult {
    let requested = *riid;
    if requested == IID_IUNKNOWN || requested == IID_ICORDEBUG_MANAGED_CALLBACK {
        *ppv = this;
        S_OK
    } else if requested == IID_ICORDEBUG_MANAGED_CALLBACK2 {
        // Return the ADDRESS of the vtbl2 field, not `this` — see
        // ManagedCallbackObj::vtbl2's doc comment for why.
        let offset = std::mem::offset_of!(ManagedCallbackObj, vtbl2);
        *ppv = (this as *mut u8).add(offset) as *mut c_void;
        S_OK
    } else {
        *ppv = std::ptr::null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "C" fn cb_add_ref(this: *mut c_void) -> u32 {
    let obj = this as *mut ManagedCallbackObj;
    (*obj).ref_count += 1;
    (*obj).ref_count
}

unsafe extern "C" fn cb_release(this: *mut c_void) -> u32 {
    let obj = this as *mut ManagedCallbackObj;
    if (*obj).ref_count > 0 {
        (*obj).ref_count -= 1;
    }
    (*obj).ref_count
}

// Recovers the real ManagedCallbackObj base address from a `this` pointer
// that arrived through the V2 interface (i.e. pointing at the `vtbl2`
// field, per cb_query_interface's IID_ICORDEBUG_MANAGED_CALLBACK2 branch
// above) — the inverse of that same offset_of! math.
unsafe fn managed_callback_base_from_v2(v2_this: *mut c_void) -> *mut c_void {
    let offset = std::mem::offset_of!(ManagedCallbackObj, vtbl2);
    (v2_this as *mut u8).sub(offset) as *mut c_void
}

unsafe extern "C" fn cb2_query_interface(
    this: *mut c_void,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> HResult {
    cb_query_interface(managed_callback_base_from_v2(this), riid, ppv)
}

unsafe extern "C" fn cb2_add_ref(this: *mut c_void) -> u32 {
    cb_add_ref(managed_callback_base_from_v2(this))
}

unsafe extern "C" fn cb2_release(this: *mut c_void) -> u32 {
    cb_release(managed_callback_base_from_v2(this))
}

// See ManagedCallback2Vtbl's doc comment: every method here just resumes
// the debuggee via Continue() on the relevant controller — the same
// "driver doesn't act on this notification" behavior every uninteresting
// V1 callback already has. `exception` in particular is the one that
// matters: it's what fixes the uncaught-exception hang (see
// IID_ICORDEBUG_MANAGED_CALLBACK2's doc comment in com/mod.rs) by giving
// CoreCLR's real "JMC user first chance" notification
// (Debugger::SendExceptionEventsWorker -> SendExceptionHelperAndBlock,
// confirmed via a symbol-resolved gdb backtrace — see tasks.md) an actual
// Continue() to unblock on, instead of silently landing on an unrelated V1
// callback slot the way it did before this V2 vtable existed.
unsafe extern "C" fn cb2_function_remap_opportunity(
    _this: *mut c_void,
    app_domain: *mut c_void,
    _thread: *mut c_void,
    _old_function: *mut c_void,
    _new_function: *mut c_void,
    _old_il_offset: u32,
) -> HResult {
    continue_(app_domain)
}

unsafe extern "C" fn cb2_create_connection(
    _this: *mut c_void,
    process: *mut c_void,
    _connection_id: u32,
    _connection_name: *const u16,
) -> HResult {
    continue_(process)
}

unsafe extern "C" fn cb2_change_connection(
    _this: *mut c_void,
    process: *mut c_void,
    _connection_id: u32,
) -> HResult {
    continue_(process)
}

unsafe extern "C" fn cb2_destroy_connection(
    _this: *mut c_void,
    process: *mut c_void,
    _connection_id: u32,
) -> HResult {
    continue_(process)
}

unsafe extern "C" fn cb2_exception(
    _this: *mut c_void,
    app_domain: *mut c_void,
    _thread: *mut c_void,
    _frame: *mut c_void,
    _offset: u32,
    _event_type: i32,
    _flags: u32,
) -> HResult {
    continue_(app_domain)
}

unsafe extern "C" fn cb2_exception_unwind(
    _this: *mut c_void,
    app_domain: *mut c_void,
    _thread: *mut c_void,
    _event_type: i32,
    _flags: u32,
) -> HResult {
    continue_(app_domain)
}

unsafe extern "C" fn cb2_function_remap_complete(
    _this: *mut c_void,
    app_domain: *mut c_void,
    _thread: *mut c_void,
    _function: *mut c_void,
) -> HResult {
    continue_(app_domain)
}

unsafe extern "C" fn cb2_mda_notification(
    _this: *mut c_void,
    controller: *mut c_void,
    _thread: *mut c_void,
    _mda: *mut c_void,
) -> HResult {
    continue_(controller)
}

pub static MANAGED_CALLBACK2_VTBL: ManagedCallback2Vtbl = ManagedCallback2Vtbl {
    query_interface: cb2_query_interface,
    add_ref: cb2_add_ref,
    release: cb2_release,
    function_remap_opportunity: cb2_function_remap_opportunity,
    create_connection: cb2_create_connection,
    change_connection: cb2_change_connection,
    destroy_connection: cb2_destroy_connection,
    exception: cb2_exception,
    exception_unwind: cb2_exception_unwind,
    function_remap_complete: cb2_function_remap_complete,
    mda_notification: cb2_mda_notification,
};

unsafe extern "C" fn cb_break(_this: *mut c_void, _ad: *mut c_void, _thread: *mut c_void) -> HResult {
    S_OK
}

/// Real (not just theoretical) bug fixed here: this used to just `S_OK`
/// without ever calling `Continue()` — every OTHER callback in this file
/// resumes the app domain/process before returning, but this one silently
/// didn't. Confirmed via a real uncaught IndexOutOfRangeException that this
/// callback DOES fire in practice (`unhandled=0`, the first-chance
/// notification) — so the no-op was a live bug, not dead code: any program
/// that raises ANY exception (caught or not — first-chance fires for both)
/// would have left the debuggee suspended here forever. Fixed by calling
/// `Continue()` like every other callback does.
///
/// Investigated (see tasks.md) but did NOT resolve the broader open issue:
/// the follow-up `unhandled=1` notification ICorDebug's docs describe for a
/// truly-unhandled exception never arrives — the debuggee just runs to the
/// outer nsjail --time_limit instead, with the stepper apparently stuck
/// somewhere in the CLR's internal exception-unwind code (same *symptom* —
/// stepper progress stalling on a CLR-internal transition — as the
/// already-documented `Thread.Start()`/`StartCore` stall, possibly a
/// related root cause, not confirmed). Tried explicitly deactivating the
/// most-recently-armed `ICorDebugStepper` here before continuing, on the
/// hypothesis that an active stepper was itself blocking the unwind from
/// completing — `Deactivate()` returned success but did not change the
/// outcome, ruling that hypothesis out empirically rather than by
/// assumption. Left as an open item, not force-fixed.
unsafe extern "C" fn cb_exception(
    _this: *mut c_void,
    ad: *mut c_void,
    _thread: *mut c_void,
    _unhandled: i32,
) -> HResult {
    continue_(ad)
}

unsafe extern "C" fn cb_eval_complete(
    _this: *mut c_void,
    _ad: *mut c_void,
    _thread: *mut c_void,
    _eval: *mut c_void,
) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_eval_exception(
    _this: *mut c_void,
    _ad: *mut c_void,
    _thread: *mut c_void,
    _eval: *mut c_void,
) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_create_process(_this: *mut c_void, process: *mut c_void) -> HResult {
    eprintln!("[callback] CreateProcess! process={:?}", process);
    let hr = continue_(process);
    eprintln!("[callback]   Continue() no process -> hr=0x{:08x}", hr as u32);
    S_OK
}

unsafe extern "C" fn cb_exit_process(_this: *mut c_void, process: *mut c_void) -> HResult {
    eprintln!("[callback] ExitProcess! process={:?}", process);
    PROCESS_EXITED = true;
    S_OK
}

// static_mut_refs: same single-threaded-reentrant-callback safety model as
// every other `static mut` in this file (see module doc comment at the
// top) — a Vec just needs a real reference to call .contains()/.push()/
// .len() on, unlike the plain bool/u32/Option<fn> statics elsewhere here
// that the compiler accepts via bare value copies.
#[allow(static_mut_refs)]
unsafe extern "C" fn cb_create_thread(
    _this: *mut c_void,
    ad: *mut c_void,
    thread: *mut c_void,
) -> HResult {
    eprintln!("[callback] CreateThread! thread={:?}", thread);

    if !SEEN_THREADS.contains(&thread) {
        SEEN_THREADS.push(thread);
    }
    if SEEN_THREADS.len() > MAX_TOLERATED_THREADS {
        eprintln!("[callback]   multi-thread detectado ({} threads distintas) — bloqueando (MVP scope)", SEEN_THREADS.len());
        report_error("multi-thread execution is not supported yet (MVP scope)".to_string());
        // Do not Continue() the app domain — the debuggee stays suspended
        // (this callback fired with SUSPEND_ALL-equivalent semantics, same
        // as every other callback here) rather than being allowed to run
        // the new thread's code. run_worker's poll loop picks up
        // FATAL_ERROR and returns promptly instead of waiting on
        // PROCESS_EXITED, which would never come on its own from here.
        return S_OK;
    }

    let hr = continue_(ad);
    eprintln!("[callback]   Continue() no app_domain -> hr=0x{:08x}", hr as u32);
    S_OK
}

unsafe extern "C" fn cb_exit_thread(_this: *mut c_void, _ad: *mut c_void, _thread: *mut c_void) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_load_module(
    _this: *mut c_void,
    ad: *mut c_void,
    module: *mut c_void,
) -> HResult {
    match get_module_name(module) {
        Ok(name) => {
            eprintln!("[callback] LoadModule! module={:?} nome={}", module, name);
            // heurística: não é uma dll do próprio .NET (framework), então é do usuário
            let is_user_module = !name.starts_with("/usr/share/dotnet/");
            // JMC (Just My Code): marca TODAS as funções deste módulo como
            // user-code (TRUE) ou não (FALSE) numa única chamada — ver doc
            // comment de `set_module_jmc_status` (com/icordebug.rs). Feito
            // pra TODO módulo que carrega (não só o do usuário), explicitamente
            // pros dois casos, pra não depender do valor padrão do runtime.
            // Precisa acontecer aqui, em LoadModule, ANTES de qualquer
            // stepper poder existir (o primeiro stepper só é criado no
            // breakpoint do método de entrada, que só é armado depois que
            // o módulo do usuário carrega) — timing confirmado pela ordem
            // real dos callbacks (framework carrega antes do usuário).
            set_module_jmc_status(module, is_user_module);
            if is_user_module {
                // See USER_MODULE's doc comment above: this is what
                // cb_step_complete (com/callback/stepping.rs) compares
                // against before trusting LOCAL_NAME_RESOLVER/LINE_RESOLVER's
                // rid-keyed lookups — both resolve against the user's own
                // PDB, which only describes methods in THIS module.
                USER_MODULE = module;
                eprintln!("[callback]   é o módulo do usuário! procurando o método de entrada de verdade (EnumTypeDefs/EnumMethods, sem token fixo)...");
                match get_metadata_import(module) {
                    Ok(metadata) => match find_entry_point_token(metadata) {
                        Some(token) => {
                            eprintln!("[callback]   SUCESSO: achou o método de entrada, token=0x{:08x}", token);
                            match get_function_from_token(module, token) {
                                Ok(func) => {
                                    eprintln!("[callback]   GetFunctionFromToken OK: {:?}", func);
                                    match create_function_breakpoint(func) {
                                        Ok(bp) => eprintln!("[callback]   SUCESSO: breakpoint criado! {:?}", bp),
                                        Err(hr) => report_error(format!(
                                            "falha ao criar breakpoint no método de entrada: hr=0x{:08x}",
                                            hr as u32
                                        )),
                                    }
                                }
                                Err(hr) => report_error(format!("GetFunctionFromToken falhou: hr=0x{:08x}", hr as u32)),
                            }
                        }
                        None => report_error(
                            "não achou método de entrada ('Main' ou '<Main>$') na assembly do usuário".to_string(),
                        ),
                    },
                    Err(hr) => report_error(format!("GetMetaDataInterface falhou: hr=0x{:08x}", hr as u32)),
                }
            }
        }
        Err(hr) => {
            eprintln!("[callback] LoadModule! module={:?} (GetName falhou: 0x{:08x})", module, hr as u32);
            // Nome desconhecido: nunca pode ser o módulo do usuário (esse
            // já teria que ter um nome resolvível), então trata como
            // framework/não-JMC por segurança — mesma lógica seria aplicada
            // se GetName tivesse funcionado e retornado um path de
            // framework.
            set_module_jmc_status(module, false);
        }
    }
    let hr = continue_(ad);
    eprintln!("[callback]   Continue() no app_domain -> hr=0x{:08x}", hr as u32);
    S_OK
}

unsafe extern "C" fn cb_unload_module(_this: *mut c_void, _ad: *mut c_void, _module: *mut c_void) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_load_class(_this: *mut c_void, ad: *mut c_void, _c: *mut c_void) -> HResult {
    let hr = continue_(ad);
    eprintln!("[callback] LoadClass -> Continue() hr=0x{:08x}", hr as u32);
    S_OK
}

unsafe extern "C" fn cb_unload_class(_this: *mut c_void, _ad: *mut c_void, _c: *mut c_void) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_debugger_error(
    _this: *mut c_void,
    process: *mut c_void,
    error_hr: i32,
    error_code: u32,
) -> HResult {
    eprintln!(
        "[callback] DebuggerError! process={:?} hr=0x{:08x} code={}",
        process, error_hr as u32, error_code
    );
    S_OK
}

unsafe extern "C" fn cb_log_message(
    _this: *mut c_void,
    _ad: *mut c_void,
    _thread: *mut c_void,
    _level: i32,
    _switch_name: *mut c_void,
    _message: *mut c_void,
) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_log_switch(
    _this: *mut c_void,
    _ad: *mut c_void,
    _thread: *mut c_void,
    _level: i32,
    _reason: u32,
    _switch_name: *mut c_void,
    _parent_name: *mut c_void,
) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_create_app_domain(
    _this: *mut c_void,
    process: *mut c_void,
    app_domain: *mut c_void,
) -> HResult {
    eprintln!("[callback] CreateAppDomain! app_domain={:?}", app_domain);
    let hr = continue_(process);
    eprintln!("[callback]   Continue() no process -> hr=0x{:08x}", hr as u32);
    S_OK
}

unsafe extern "C" fn cb_exit_app_domain(
    _this: *mut c_void,
    _process: *mut c_void,
    _app_domain: *mut c_void,
) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_load_assembly(
    _this: *mut c_void,
    ad: *mut c_void,
    assembly: *mut c_void,
) -> HResult {
    eprintln!("[callback] LoadAssembly! assembly={:?}", assembly);
    let hr = continue_(ad);
    eprintln!("[callback]   Continue() no app_domain -> hr=0x{:08x}", hr as u32);
    S_OK
}

unsafe extern "C" fn cb_unload_assembly(_this: *mut c_void, _ad: *mut c_void, _assembly: *mut c_void) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_control_c_trap(_this: *mut c_void, _process: *mut c_void) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_name_change(_this: *mut c_void, ad: *mut c_void, _thread: *mut c_void) -> HResult {
    let hr = continue_(ad);
    eprintln!("[callback] NameChange -> Continue() hr=0x{:08x}", hr as u32);
    S_OK
}

unsafe extern "C" fn cb_update_module_symbols(
    _this: *mut c_void,
    _ad: *mut c_void,
    _module: *mut c_void,
    _stream: *mut c_void,
) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_edit_and_continue_remap(
    _this: *mut c_void,
    _ad: *mut c_void,
    _thread: *mut c_void,
    _function: *mut c_void,
    _accurate: i32,
) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_breakpoint_set_error(
    _this: *mut c_void,
    _ad: *mut c_void,
    _thread: *mut c_void,
    _breakpoint: *mut c_void,
    error: u32,
) -> HResult {
    eprintln!("[callback] BreakpointSetError! error={}", error);
    S_OK
}

pub static MANAGED_CALLBACK_VTBL: ManagedCallbackVtbl = ManagedCallbackVtbl {
    query_interface: cb_query_interface,
    add_ref: cb_add_ref,
    release: cb_release,
    breakpoint: stepping::cb_breakpoint,
    step_complete: stepping::cb_step_complete,
    break_: cb_break,
    exception: cb_exception,
    eval_complete: cb_eval_complete,
    eval_exception: cb_eval_exception,
    create_process: cb_create_process,
    exit_process: cb_exit_process,
    create_thread: cb_create_thread,
    exit_thread: cb_exit_thread,
    load_module: cb_load_module,
    unload_module: cb_unload_module,
    load_class: cb_load_class,
    unload_class: cb_unload_class,
    debugger_error: cb_debugger_error,
    log_message: cb_log_message,
    log_switch: cb_log_switch,
    create_app_domain: cb_create_app_domain,
    exit_app_domain: cb_exit_app_domain,
    load_assembly: cb_load_assembly,
    unload_assembly: cb_unload_assembly,
    control_c_trap: cb_control_c_trap,
    name_change: cb_name_change,
    update_module_symbols: cb_update_module_symbols,
    edit_and_continue_remap: cb_edit_and_continue_remap,
    breakpoint_set_error: cb_breakpoint_set_error,
};
