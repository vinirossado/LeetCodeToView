// As interfaces "cliente" que este driver chama PRA DENTRO do runtime —
// ICorDebug em si, Module/Function/Thread/Stepper/ILFrame — na ordem exata
// declarada no cordebug.idl (ver módulo doc comment em com/mod.rs).

use std::os::raw::c_void;

use super::metadata::get_method_name;
use super::{query_interface, Guid, HResult, S_OK, FATAL_ERROR, IID_ICORDEBUG_MODULE2, IID_ICORDEBUG_STEPPER2, IID_IMETADATA_IMPORT};

// --- ICorDebug (só os slots que usamos; o resto entra como placeholder de
// mesmo tamanho, já que o que importa pro ABI é a posição/contagem, não o
// tipo exato de slots que nunca chamamos) ---

#[repr(C)]
pub struct ICorDebugVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub initialize: unsafe extern "C" fn(this: *mut c_void) -> HResult,
    pub terminate: unsafe extern "C" fn(this: *mut c_void) -> HResult,
    pub set_managed_handler: unsafe extern "C" fn(this: *mut c_void, callback: *mut c_void) -> HResult,
    pub set_unmanaged_handler: unsafe extern "C" fn(this: *mut c_void, callback: *mut c_void) -> HResult,
    pub create_process: *const c_void, // não usamos (lançamos via dbgshim), só ocupa o slot
    pub debug_active_process:
        unsafe extern "C" fn(this: *mut c_void, id: u32, win32_attach: i32, pp_process: *mut *mut c_void) -> HResult,
    pub enumerate_processes: *const c_void,
    pub get_process: *const c_void,
    pub can_launch_or_attach: *const c_void,
}

pub struct CorDebug(pub *mut c_void);

impl CorDebug {
    pub unsafe fn vtbl(&self) -> *const ICorDebugVtbl {
        *(self.0 as *const *const ICorDebugVtbl)
    }

    pub unsafe fn initialize(&self) -> HResult {
        ((*self.vtbl()).initialize)(self.0)
    }

    pub unsafe fn set_managed_handler(&self, callback: *mut c_void) -> HResult {
        ((*self.vtbl()).set_managed_handler)(self.0, callback)
    }

    pub unsafe fn debug_active_process(&self, pid: u32) -> Result<*mut c_void, HResult> {
        let mut pp_process: *mut c_void = std::ptr::null_mut();
        let hr = ((*self.vtbl()).debug_active_process)(self.0, pid, 0, &mut pp_process);
        if hr == S_OK {
            Ok(pp_process)
        } else {
            Err(hr)
        }
    }
}

// --- ICorDebugController (base de ICorDebugProcess/ICorDebugAppDomain) ---
// Só declaramos o prefixo que usamos (Stop, Continue) — o resto da interface
// (EnumerateThreads, Detach, etc.) fica de fora por enquanto, não precisamos.

#[repr(C)]
pub struct ControllerVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub stop: unsafe extern "C" fn(*mut c_void, u32) -> HResult,
    pub continue_: unsafe extern "C" fn(*mut c_void, i32) -> HResult,
}

/// Chama Continue() num ICorDebugProcess ou ICorDebugAppDomain (ambos
/// herdam de ICorDebugController, Continue está na mesma posição de vtable).
///
/// Real bug found by actually running the new multi-thread block against
/// the real 8-thread test snippet through the full API/Docker stack (not
/// just a manual, lower-concurrency exploratory run): `ICorDebug::Continue`
/// resumes the WHOLE debuggee process, not just the one callback/thread
/// that received it. During a busy startup burst — several LoadModule/
/// LoadAssembly callbacks firing close together with the CreateThread
/// callbacks that detect and block — cb_create_thread correctly skipped
/// ITS OWN Continue() call once the threshold was crossed, but every OTHER
/// callback in that same burst kept calling this function unconditionally
/// and resumed the process anyway, undoing the block. Intermittent,
/// confirmed by repeated real runs: sometimes the block's CreateThread
/// callback happened to be the last one in a burst (worked), sometimes it
/// wasn't (didn't — the debuggee kept running until nsjail's own
/// `--time_limit` eventually killed it). Fixed at the single choke point
/// every callback already goes through, instead of adding a check to each
/// of the ~15 callback functions individually.
pub unsafe fn continue_(controller: *mut c_void) -> HResult {
    if FATAL_ERROR {
        return S_OK;
    }
    let vtbl = *(controller as *const *const ControllerVtbl);
    ((*vtbl).continue_)(controller, 0) // fIsOutOfBand = FALSE
}

// --- ICorDebugModule ---

#[repr(C)]
pub struct ModuleVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub get_process: *const c_void,
    pub get_base_address: *const c_void,
    pub get_assembly: *const c_void,
    pub get_name: unsafe extern "C" fn(*mut c_void, u32, *mut u32, *mut u16) -> HResult,
    pub enable_jit_debugging: *const c_void,
    pub enable_class_load_callbacks: *const c_void,
    pub get_function_from_token: unsafe extern "C" fn(*mut c_void, u32, *mut *mut c_void) -> HResult,
    pub get_function_from_rva: *const c_void,
    pub get_class_from_token: *const c_void,
    pub create_module_breakpoint: *const c_void,
    pub get_edit_and_continue_snapshot: *const c_void,
    pub get_metadata_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
}

/// IMetaDataImport do módulo (metadata da própria assembly — nomes de
/// métodos/tipos vêm daqui, diferente de nomes de variável local, que só
/// existem no PDB).
pub unsafe fn get_metadata_import(module: *mut c_void) -> Result<*mut c_void, HResult> {
    let vtbl = *(module as *const *const ModuleVtbl);
    let mut out: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).get_metadata_interface)(module, &IID_IMETADATA_IMPORT, &mut out);
    if hr == S_OK {
        Ok(out)
    } else {
        Err(hr)
    }
}

/// Nome do módulo (ex: caminho completo do .dll). Padrão "pergunta o tamanho,
/// aloca, pergunta de novo" comum em APIs COM que devolvem string.
pub unsafe fn get_module_name(module: *mut c_void) -> Result<String, HResult> {
    let vtbl = *(module as *const *const ModuleVtbl);
    let mut buf = vec![0u16; 512];
    let mut written: u32 = 0;
    let hr = ((*vtbl).get_name)(module, buf.len() as u32, &mut written, buf.as_mut_ptr());
    if hr != S_OK {
        return Err(hr);
    }
    let len = written.saturating_sub(1) as usize; // written inclui o \0 final
    Ok(String::from_utf16_lossy(&buf[..len.min(buf.len())]))
}

// --- ICorDebugModule2 (extensão de ICorDebugModule, só o slot de JMC) ---

#[repr(C)]
pub struct Module2Vtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub set_jmc_status:
        unsafe extern "C" fn(*mut c_void, i32, u32, *const u32) -> HResult, // bIsJustMyCode, cTokens, pTokens
}

/// Marca TODAS as funções do módulo como Just My Code (`bIsJustMyCode=TRUE`)
/// ou não (`FALSE`) numa única chamada (`ICorDebugModule2::SetJMCStatus`,
/// `cTokens=0`/`pTokens=NULL` — "erase all previous JMC settings in this
/// module" pro módulo inteiro, sem exceções por token). Chamado em
/// `cb_load_module` (com/callback.rs) pra TODO módulo que carrega, não só o
/// do usuário: setar explicitamente FALSE nos módulos de framework (em vez
/// de confiar no valor padrão do runtime pra módulos sem PDB correspondente)
/// deixa o comportamento determinístico e documentado, não dependente de uma
/// suposição não verificada sobre o CoreCLR.
///
/// Não fatal se falhar (`ICorDebugModule2` pode não existir em runtimes
/// muito antigos, ou `SetJMCStatus(TRUE, ...)` pode devolver
/// `CORDBG_E_FUNCTION_NOT_DEBUGGABLE` se alguma função do módulo do usuário
/// não tiver info de debug) — só loga, não aborta a sessão: JMC é uma
/// otimização de quais frames o stepper para, não uma correção de
/// resolução de linha/nome (essas continuam gated por `USER_MODULE`,
/// independente de JMC ter funcionado ou não).
pub unsafe fn set_module_jmc_status(module: *mut c_void, is_just_my_code: bool) {
    match query_interface(module, &IID_ICORDEBUG_MODULE2) {
        Ok(module2) => {
            let vtbl = *(module2 as *const *const Module2Vtbl);
            let hr = ((*vtbl).set_jmc_status)(module2, is_just_my_code as i32, 0, std::ptr::null());
            if hr != S_OK {
                eprintln!(
                    "[jmc] ICorDebugModule2::SetJMCStatus({}) módulo={:?} -> hr=0x{:08x}",
                    is_just_my_code, module, hr as u32
                );
            }
        }
        Err(hr) => {
            eprintln!(
                "[jmc] QueryInterface(ICorDebugModule2) falhou pra módulo={:?} -> hr=0x{:08x}",
                module, hr as u32
            );
        }
    }
}

pub unsafe fn get_function_from_token(module: *mut c_void, token: u32) -> Result<*mut c_void, HResult> {
    let vtbl = *(module as *const *const ModuleVtbl);
    let mut func: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).get_function_from_token)(module, token, &mut func);
    if hr == S_OK {
        Ok(func)
    } else {
        Err(hr)
    }
}

// --- ICorDebugFunction ---

#[repr(C)]
pub struct FunctionVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub get_module: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> HResult,
    pub get_class: *const c_void,
    pub get_token: unsafe extern "C" fn(*mut c_void, *mut u32) -> HResult,
    pub get_il_code: *const c_void,
    pub get_native_code: *const c_void,
    pub create_breakpoint: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> HResult,
}

pub unsafe fn get_function_module(function: *mut c_void) -> Result<*mut c_void, HResult> {
    let vtbl = *(function as *const *const FunctionVtbl);
    let mut module: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).get_module)(function, &mut module);
    if hr == S_OK {
        Ok(module)
    } else {
        Err(hr)
    }
}

pub unsafe fn get_function_token(function: *mut c_void) -> Result<u32, HResult> {
    let vtbl = *(function as *const *const FunctionVtbl);
    let mut token: u32 = 0;
    let hr = ((*vtbl).get_token)(function, &mut token);
    if hr == S_OK {
        Ok(token)
    } else {
        Err(hr)
    }
}

/// Cria um breakpoint bem no início da função (offset IL 0) — ICorDebugFunction::CreateBreakpoint
/// evita ter que passar por ICorDebugCode pra isso.
pub unsafe fn create_function_breakpoint(function: *mut c_void) -> Result<*mut c_void, HResult> {
    let vtbl = *(function as *const *const FunctionVtbl);
    let mut bp: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).create_breakpoint)(function, &mut bp);
    if hr == S_OK {
        Ok(bp)
    } else {
        Err(hr)
    }
}

// --- ICorDebugThread ---

#[repr(C)]
pub struct ThreadVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub get_process: *const c_void,
    pub get_id: *const c_void,
    pub get_handle: *const c_void,
    pub get_app_domain: *const c_void,
    pub set_debug_state: *const c_void,
    pub get_debug_state: *const c_void,
    pub get_user_state: *const c_void,
    pub get_current_exception: *const c_void,
    pub clear_current_exception: *const c_void,
    pub create_stepper: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> HResult,
    pub enumerate_chains: *const c_void,
    pub get_active_chain: *const c_void,
    pub get_active_frame: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> HResult,
}

/// Cria um stepper novo E já habilita JMC nele (`ICorDebugStepper2::SetJMC`,
/// ver doc comment das IIDs em com/mod.rs pro problema real que isso
/// resolve). Centralizado aqui (em vez de cada call site chamar `SetJMC`
/// depois) pra garantir que NENHUM caminho de criação de stepper esqueça de
/// habilitar JMC — os dois call sites existentes (`cb_breakpoint`, primeiro
/// stepper da sessão, e `cb_step_complete`, que re-arma um novo stepper a
/// cada `StepComplete`, ambos em com/callback.rs) chamam só
/// `create_stepper`, sem saber de JMC.
///
/// Não fatal se `QueryInterface(ICorDebugStepper2)`/`SetJMC` falhar: nesse
/// caso o stepper simplesmente se comporta como antes desta mudança
/// (instrução-a-instrução em TUDO, inclusive framework) — pior desempenho,
/// não um crash ou comportamento incorreto.
pub unsafe fn create_stepper(thread: *mut c_void) -> Result<*mut c_void, HResult> {
    let vtbl = *(thread as *const *const ThreadVtbl);
    let mut stepper: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).create_stepper)(thread, &mut stepper);
    if hr == S_OK {
        set_stepper_jmc(stepper);
        Ok(stepper)
    } else {
        Err(hr)
    }
}

#[repr(C)]
pub struct Stepper2Vtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub set_jmc: unsafe extern "C" fn(*mut c_void, i32) -> HResult, // fIsJMCStepper
}

/// Real bug found and fixed empirically, not guessed: an earlier version of
/// this function called `ICorDebugStepper2::SetJMC(TRUE)` directly on a
/// freshly created stepper and it failed with `E_INVALIDARG` on EVERY
/// single call (confirmed via a real run through the full Docker
/// pipeline — `docker exec` into the built image, running
/// `sandbox-runner --language csharp --file <dll>` directly (bypassing the
/// API's stderr-swallow-on-success behavior in `ProcessSandboxRunner.java`,
/// which normally discards this diagnostic output) to capture raw stderr:
/// 1777 identical `[jmc] ICorDebugStepper2::SetJMC(TRUE) -> hr=0x80070057`
/// lines, one per stepper, for a 5-iteration loop). Root-caused by reading
/// the REAL CoreCLR source (`gh api search/code` for
/// `CordbStepper::SetJMC`, found in
/// `src/coreclr/debug/di/breakpoint.cpp` despite the misleading filename):
/// `CordbStepper::SetJMC` unconditionally returns `E_INVALIDARG` when
/// `m_rgfMappingStop & STOP_ALL != 0`, and the constructor's member-init
/// list sets `m_rgfMappingStop(STOP_OTHER_UNMAPPED)` — non-zero — by
/// default on every new stepper. So `SetJMC` can never succeed without
/// first calling `SetUnmappedStopMask(STOP_NONE)` to clear that mask. Fixed
/// below; re-verified against the same real pipeline afterwards (see
/// tasks.md for the before/after event counts).
unsafe fn set_stepper_jmc(stepper: *mut c_void) {
    let hr_mask = set_unmapped_stop_mask(stepper, STOP_NONE);
    if hr_mask != S_OK {
        eprintln!("[jmc] ICorDebugStepper::SetUnmappedStopMask(STOP_NONE) -> hr=0x{:08x}", hr_mask as u32);
    }
    match query_interface(stepper, &IID_ICORDEBUG_STEPPER2) {
        Ok(stepper2) => {
            let vtbl = *(stepper2 as *const *const Stepper2Vtbl);
            let hr = ((*vtbl).set_jmc)(stepper2, 1 /* TRUE */);
            if hr != S_OK {
                eprintln!("[jmc] ICorDebugStepper2::SetJMC(TRUE) -> hr=0x{:08x}", hr as u32);
            }
        }
        Err(hr) => {
            eprintln!("[jmc] QueryInterface(ICorDebugStepper2) falhou -> hr=0x{:08x}", hr as u32);
        }
    }
}

pub unsafe fn get_active_frame(thread: *mut c_void) -> Result<*mut c_void, HResult> {
    let vtbl = *(thread as *const *const ThreadVtbl);
    let mut frame: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).get_active_frame)(thread, &mut frame);
    if hr == S_OK && !frame.is_null() {
        Ok(frame)
    } else {
        Err(hr)
    }
}

// --- ICorDebugStepper ---

/// `COR_DEBUG_STEP_RANGE` from cordebug.idl — a half-open `[startOffset,
/// endOffset)` IL-offset range, relative to the stepper's frame's method
/// (IL-relative by default; `SetRangeIL` would change that, never called
/// here since IL-relative is exactly what pdb.rs's SequencePoints offsets
/// already are). `#[repr(C)]`, two `ULONG32`s, no padding — verified against
/// the real struct definition in dotnet/runtime's cordebug.idl before use
/// (see StepRange's doc comment below), not guessed.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CorDebugStepRange {
    pub start_offset: u32,
    pub end_offset: u32,
}

#[repr(C)]
pub struct StepperVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub is_active: *const c_void,
    pub deactivate: *const c_void,
    pub set_intercept_mask: *const c_void,
    pub set_unmapped_stop_mask: unsafe extern "C" fn(*mut c_void, i32) -> HResult, // CorDebugUnmappedStop mask
    pub step: unsafe extern "C" fn(*mut c_void, i32) -> HResult,
    pub step_range:
        unsafe extern "C" fn(*mut c_void, i32, *const CorDebugStepRange, u32) -> HResult,
}

/// `CorDebugUnmappedStop::STOP_NONE` (cordebug.idl) — "stop nowhere in
/// unmapped code, transparently continue through it". Needed as a
/// precondition for JMC, see `set_stepper_jmc`'s doc comment above: a
/// freshly created `ICorDebugStepper` defaults to `STOP_OTHER_UNMAPPED`
/// (confirmed reading the real CoreCLR source,
/// `CordbStepper::CordbStepper`'s member-init list in
/// `src/coreclr/debug/di/breakpoint.cpp`), and `ICorDebugStepper2::SetJMC`
/// unconditionally rejects (`E_INVALIDARG`) any stepper whose
/// `m_rgfMappingStop & STOP_ALL` is non-zero (same file,
/// `CordbStepper::SetJMC`) — so JMC can never be enabled without first
/// clearing this mask.
const STOP_NONE: i32 = 0x0;

/// `ICorDebugStepper::SetUnmappedStopMask` — see `STOP_NONE`'s doc comment
/// above for why `set_stepper_jmc` needs this.
unsafe fn set_unmapped_stop_mask(stepper: *mut c_void, mask: i32) -> HResult {
    let vtbl = *(stepper as *const *const StepperVtbl);
    ((*vtbl).set_unmapped_stop_mask)(stepper, mask)
}

/// Step(bStepIn=TRUE) — passo mínimo (granularidade de instrução IL), usado
/// como FALLBACK quando não dá pra calcular um range de linha real (ver
/// `step_range` abaixo e com/callback.rs's `arm_step`): sem PDB, método sem
/// dados de sequence point, ou offset atual caindo numa região "hidden"
/// (código gerado pelo compilador, sem linha de origem real) — nesses casos
/// um único hop de instrução crua é o melhor que dá pra fazer honestamente
/// até aterrissar de volta em código com sequence point real.
pub unsafe fn step_into(stepper: *mut c_void) -> HResult {
    let vtbl = *(stepper as *const *const StepperVtbl);
    ((*vtbl).step)(stepper, 1)
}

/// `ICorDebugStepper::StepRange(bStepIn=TRUE, ranges, 1)` — o mecanismo REAL
/// que debuggers baseados em CLR (Visual Studio incluído) usam pra "step
/// over/into uma linha de código fonte": ao contrário de `Step`, que sempre
/// completa na PRÓXIMA instrução IL não importa a linha, `StepRange` só
/// dispara StepComplete quando a execução sai do range IL dado — então
/// armar com o range IL completo do sequence point ATUAL (ver
/// pdb.rs::PortablePdb::step_range_for) produz exatamente 1 StepComplete por
/// LINHA de origem, não por instrução IL. Isso é o análogo, do lado
/// ICorDebug, do que `StepRequest.STEP_LINE` já dá de graça pro JDI/Java.
/// Signature/ordem no vtable (índice logo após `Step`) verificadas contra o
/// `cordebug.idl` real (dotnet/runtime, `interface ICorDebugStepper`), não
/// adivinhadas.
pub unsafe fn step_range(stepper: *mut c_void, step_in: bool, start_offset: u32, end_offset: u32) -> HResult {
    let vtbl = *(stepper as *const *const StepperVtbl);
    let ranges = [CorDebugStepRange { start_offset, end_offset }];
    ((*vtbl).step_range)(stepper, step_in as i32, ranges.as_ptr(), 1)
}

// --- ICorDebugILFrame (só até o slot que usamos, GetLocalVariable) ---

#[repr(C)]
pub struct ILFrameVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub get_chain: *const c_void,
    pub get_code: *const c_void,
    pub get_function: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> HResult,
    pub get_function_token: *const c_void,
    pub get_stack_range: *const c_void,
    pub get_caller: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> HResult,
    pub get_callee: *const c_void,
    pub create_stepper: *const c_void,
    pub get_ip: unsafe extern "C" fn(*mut c_void, *mut u32, *mut i32) -> HResult,
    pub set_ip: *const c_void,
    pub enumerate_local_variables: *const c_void,
    pub get_local_variable: unsafe extern "C" fn(*mut c_void, u32, *mut *mut c_void) -> HResult,
}

/// Sobe um frame na call stack (ICorDebugFrame::GetCaller — funciona igual
/// pra ICorDebugILFrame, que herda de ICorDebugFrame). Retorna Ok(None)
/// quando chega no topo da pilha (sem mais caller).
pub unsafe fn get_caller(frame: *mut c_void) -> Result<Option<*mut c_void>, HResult> {
    let vtbl = *(frame as *const *const ILFrameVtbl);
    let mut caller: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).get_caller)(frame, &mut caller);
    if hr == S_OK {
        Ok(if caller.is_null() { None } else { Some(caller) })
    } else {
        Err(hr)
    }
}

/// Offset IL atual do frame (ICorDebugILFrame::GetIP) — não há API COM
/// nenhuma (ICorDebug) que devolva número de linha C# diretamente, então
/// esse offset é sempre o ponto de partida. `cb_step_complete`
/// (com/callback.rs) passa esse valor pro LINE_RESOLVER
/// (pdb.rs::PortablePdb::line_for, via o SequencePoints blob do PDB) pra
/// resolver a linha real; só cai de volta pro offset IL cru quando não há
/// PDB, o método não tem dados de sequence point, ou o offset cai numa
/// região "hidden" (código gerado pelo compilador, sem linha de origem
/// real).
pub unsafe fn get_il_offset(il_frame: *mut c_void) -> Result<u32, HResult> {
    let vtbl = *(il_frame as *const *const ILFrameVtbl);
    let mut offset: u32 = 0;
    let mut mapping_result: i32 = 0;
    let hr = ((*vtbl).get_ip)(il_frame, &mut offset, &mut mapping_result);
    if hr == S_OK {
        Ok(offset)
    } else {
        Err(hr)
    }
}

pub unsafe fn get_function(frame: *mut c_void) -> Result<*mut c_void, HResult> {
    let vtbl = *(frame as *const *const ILFrameVtbl);
    let mut func: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).get_function)(frame, &mut func);
    if hr == S_OK {
        Ok(func)
    } else {
        Err(hr)
    }
}

/// Sobe a call stack a partir de um frame, devolvendo o nome de método
/// (via metadata da assembly) de cada nível — sem número de linha, isso
/// depende de PDB (item futuro, ver spec.md).
pub unsafe fn get_call_stack_names(start_frame: *mut c_void) -> Vec<String> {
    let mut frame = start_frame;
    let mut depth = 0;
    let mut names = Vec::new();
    loop {
        let name_str = match get_function(frame) {
            Ok(func) => {
                let token = get_function_token(func);
                let name = (|| {
                    let module = get_function_module(func).ok()?;
                    let metadata = get_metadata_import(module).ok()?;
                    let t = *token.as_ref().ok()?;
                    get_method_name(metadata, t).ok()
                })();
                match (name, token) {
                    (Some(n), _) => n,
                    (None, Ok(t)) => format!("token=0x{:08x}", t),
                    (None, Err(hr)) => format!("<GetToken falhou: 0x{:08x}>", hr as u32),
                }
            }
            Err(hr) => format!("<GetFunction falhou: 0x{:08x}>", hr as u32),
        };
        names.push(name_str);

        match get_caller(frame) {
            Ok(Some(caller)) => {
                frame = caller;
                depth += 1;
                if depth > 50 {
                    break; // proteção contra pilha corrompida/recursão absurda
                }
            }
            _ => break,
        }
    }
    names
}

pub unsafe fn get_local_variable(il_frame: *mut c_void, index: u32) -> Result<*mut c_void, HResult> {
    let vtbl = *(il_frame as *const *const ILFrameVtbl);
    let mut value: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).get_local_variable)(il_frame, index, &mut value);
    if hr == S_OK {
        Ok(value)
    } else {
        Err(hr)
    }
}
