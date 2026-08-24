// Plumbing COM mínimo (ABI Itanium C++, como o CoreCLR expõe no Linux) pra
// falar com ICorDebug/ICorDebugManagedCallback sem nenhuma lib de binding.
// Cada interface COM é, na prática, um ponteiro pra um ponteiro de vtable
// (array de function pointers), na ordem exata declarada no cordebug.idl.
// Layout errado = crash silencioso ou corrupção — testado empiricamente.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::os::raw::c_void;

// Indireção de emissão de evento: os callbacks COM abaixo são `extern "C" fn`
// sem contexto arbitrário pra passar, e este arquivo é compartilhado (via
// `mod com;`) pelo binário legado icordebug-spike, que não tem o módulo
// `events` do crate da lib — então não dá pra chamar `crate::events::emit`
// direto daqui. Em vez disso, csharp::run_worker seta esses ponteiros de
// função (sem estado capturado, então cabem em `fn` puro) antes de iniciar a
// sessão de debug; o icordebug-spike nunca seta os sinks, então os callbacks
// simplesmente não emitem nada nele (comportamento antigo preservado).
pub static mut STEP_SINK: Option<fn(i64, BTreeMap<String, serde_json::Value>, Vec<String>)> = None;
pub static mut ERROR_SINK: Option<fn(String)> = None;
// Fires once, the moment the step cap below is reached, so run_worker can
// emit Event::StepLimitExceeded (same product-scope decision as the Java
// side — see jdi/Debugger.java and events::STEP_EVENT_CAP).
pub static mut LIMIT_SINK: Option<fn()> = None;
pub static mut PROCESS_EXITED: bool = false;
pub static mut FATAL_ERROR: bool = false;
// Counts real step events emitted (not every StepComplete callback — only
// ones where inspection actually produced an event), same definition the
// Java side uses. Once it hits events::STEP_EVENT_CAP, cb_step_complete
// stops arming a new stepper and just lets the program run to completion
// uninstrumented, exactly like the JDI side.
pub static mut STEP_EVENTS_EMITTED: u32 = 0;
pub static mut STEP_CAPPED: bool = false;
// Same indirection reason as STEP_SINK/ERROR_SINK above (this file is
// shared with the legacy icordebug-spike binary, which has no `pdb` module
// to call directly) — csharp::run_worker sets this to a plain `fn` (no
// captured state, loads the PDB once up front) that maps
// (method token, IL offset) -> {slot index -> real variable name}. `None`
// (the icordebug-spike default, and csharp.rs's own fallback when no .pdb
// was found) means extract_locals keeps using positional `local_N` keys.
pub static mut LOCAL_NAME_RESOLVER: Option<fn(u32, u32) -> BTreeMap<u32, String>> = None;

unsafe fn report_error(message: String) {
    FATAL_ERROR = true;
    if let Some(sink) = ERROR_SINK {
        sink(message);
    }
}

pub type HResult = i32;
pub const S_OK: HResult = 0;
pub const E_NOTIMPL: HResult = 0x80004001u32 as i32;
pub const E_NOINTERFACE: HResult = 0x80004002u32 as i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

macro_rules! guid {
    ($d1:expr, $d2:expr, $d3:expr, $($b:expr),+) => {
        Guid { data1: $d1, data2: $d2, data3: $d3, data4: [$($b),+] }
    };
}

// IIDs de cordebug.idl (dotnet/runtime) — estáveis há anos.
pub const IID_ICORDEBUG: Guid = guid!(0x3D6F5F61, 0x7538, 0x11D3, 0x8D, 0x5B, 0x00, 0x10, 0x4B, 0x35, 0xE7, 0xEF);
pub const IID_ICORDEBUG_MANAGED_CALLBACK: Guid =
    guid!(0x3D6F5F62, 0x7538, 0x11D3, 0x8D, 0x5B, 0x00, 0x10, 0x4B, 0x35, 0xE7, 0xEF);
pub const IID_ICORDEBUG_IL_FRAME: Guid =
    guid!(0x03E26311, 0x4F76, 0x11D3, 0x88, 0xC6, 0x00, 0x60, 0x97, 0x94, 0x54, 0x18);
pub const IID_ICORDEBUG_GENERIC_VALUE: Guid =
    guid!(0x3D6F5F63, 0x7538, 0x11D3, 0x8D, 0x5B, 0x00, 0x10, 0x4B, 0x35, 0xE7, 0xEF);
pub const IID_IMETADATA_IMPORT: Guid =
    guid!(0x7DAC8207, 0xD3AE, 0x4C75, 0x9B, 0x67, 0x92, 0x80, 0x1A, 0x49, 0x7D, 0x44);
// IIDs de ICorDebugValue* — confirmados direto do cordebug.idl fonte
// (dotnet/runtime), não mais de memória — primeiro palpite (CC7BCAEx)
// estava errado, família certa é CC7BCAFx.
pub const IID_ICORDEBUG_REFERENCE_VALUE: Guid =
    guid!(0xCC7BCAF9, 0x8A68, 0x11D2, 0x98, 0x3C, 0x00, 0x00, 0xF8, 0x08, 0x34, 0x2D);
pub const IID_ICORDEBUG_STRING_VALUE: Guid =
    guid!(0xCC7BCAFD, 0x8A68, 0x11D2, 0x98, 0x3C, 0x00, 0x00, 0xF8, 0x08, 0x34, 0x2D);
pub const IID_ICORDEBUG_ARRAY_VALUE: Guid =
    guid!(0x0405B0DF, 0xA660, 0x11D2, 0xBD, 0x02, 0x00, 0x00, 0xF8, 0x08, 0x49, 0xBD);

// --- IUnknown ---

#[repr(C)]
pub struct IUnknownVtbl {
    pub query_interface:
        unsafe extern "C" fn(this: *mut c_void, riid: *const Guid, ppv: *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(this: *mut c_void) -> u32,
    pub release: unsafe extern "C" fn(this: *mut c_void) -> u32,
}

pub unsafe fn query_interface(obj: *mut c_void, iid: &Guid) -> Result<*mut c_void, HResult> {
    let vtbl = *(obj as *const *const IUnknownVtbl);
    let mut out: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).query_interface)(obj, iid, &mut out);
    if hr == S_OK {
        Ok(out)
    } else {
        Err(hr)
    }
}

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
pub unsafe fn continue_(controller: *mut c_void) -> HResult {
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

pub unsafe fn create_stepper(thread: *mut c_void) -> Result<*mut c_void, HResult> {
    let vtbl = *(thread as *const *const ThreadVtbl);
    let mut stepper: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).create_stepper)(thread, &mut stepper);
    if hr == S_OK {
        Ok(stepper)
    } else {
        Err(hr)
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

#[repr(C)]
pub struct StepperVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub is_active: *const c_void,
    pub deactivate: *const c_void,
    pub set_intercept_mask: *const c_void,
    pub set_unmapped_stop_mask: *const c_void,
    pub step: unsafe extern "C" fn(*mut c_void, i32) -> HResult,
}

/// Step(bStepIn=TRUE) — passo mínimo (granularidade de instrução IL, já que
/// ainda não temos sequence points do PDB pra step por linha de verdade).
pub unsafe fn step_into(stepper: *mut c_void) -> HResult {
    let vtbl = *(stepper as *const *const StepperVtbl);
    ((*vtbl).step)(stepper, 1)
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

/// Offset IL atual do frame (ICorDebugILFrame::GetIP) — usado como
/// substituto de número de linha enquanto não há leitura de PDB (ver
/// find_entry_point_token e spec.md: sem PDB, não existe mapeamento IL→linha
/// C# original disponível via API COM nenhuma).
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

// --- ICorDebugValue / ICorDebugGenericValue (valores primitivos) ---

#[repr(C)]
pub struct GenericValueVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub get_type: unsafe extern "C" fn(*mut c_void, *mut i32) -> HResult,
    pub get_size: *const c_void,
    pub get_address: *const c_void,
    pub create_breakpoint: *const c_void,
    pub get_value: unsafe extern "C" fn(*mut c_void, *mut c_void) -> HResult,
}

pub unsafe fn get_value_type(value: *mut c_void) -> Result<i32, HResult> {
    let vtbl = *(value as *const *const GenericValueVtbl);
    let mut t: i32 = 0;
    let hr = ((*vtbl).get_type)(value, &mut t);
    if hr == S_OK {
        Ok(t)
    } else {
        Err(hr)
    }
}

/// Lê o valor como i32 — só faz sentido pra tipos primitivos de 4 bytes
/// (ELEMENT_TYPE_I4 = int, ELEMENT_TYPE_BOOLEAN, etc.); spike, sem
/// diferenciar tipo ainda.
pub unsafe fn get_value_i32(value: *mut c_void) -> Result<i32, HResult> {
    let vtbl = *(value as *const *const GenericValueVtbl);
    let mut out: i32 = 0;
    let hr = ((*vtbl).get_value)(value, &mut out as *mut i32 as *mut c_void);
    if hr == S_OK {
        Ok(out)
    } else {
        Err(hr)
    }
}

// --- IMetaDataImport ---
// Interface grande (~60 métodos), ordem de cor.h (dotnet/runtime). Cada slot
// não usado abaixo está nomeado (não só como placeholder anônimo) pra dar
// pra auditar/contar se algo crashar. GetMethodProps é o slot 30 (0-indexed).

#[repr(C)]
pub struct MetaDataImportVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub close_enum: unsafe extern "C" fn(*mut c_void, *mut c_void), // 3, retorna void de verdade
    pub count_enum: *const c_void,                // 4
    pub reset_enum: *const c_void,                // 5
    pub enum_type_defs:                            // 6
        unsafe extern "C" fn(*mut c_void, *mut *mut c_void, *mut u32, u32, *mut u32) -> HResult,
    pub enum_interface_impls: *const c_void,       // 7
    pub enum_type_refs: *const c_void,             // 8
    pub find_type_def_by_name: *const c_void,      // 9
    pub get_scope_props: *const c_void,            // 10
    pub get_module_from_scope: *const c_void,      // 11
    pub get_type_def_props: *const c_void,         // 12
    pub get_interface_impl_props: *const c_void,   // 13
    pub get_type_ref_props: *const c_void,         // 14
    pub resolve_type_ref: *const c_void,           // 15
    pub enum_members: *const c_void,               // 16
    pub enum_members_with_name: *const c_void,     // 17
    pub enum_methods:                              // 18
        unsafe extern "C" fn(*mut c_void, *mut *mut c_void, u32, *mut u32, u32, *mut u32) -> HResult,
    pub enum_methods_with_name: *const c_void,     // 19
    pub enum_fields: *const c_void,                // 20
    pub enum_fields_with_name: *const c_void,      // 21
    pub enum_params: *const c_void,                // 22
    pub enum_member_refs: *const c_void,           // 23
    pub enum_method_impls: *const c_void,          // 24
    pub enum_permission_sets: *const c_void,       // 25
    pub find_member: *const c_void,                // 26
    pub find_method: *const c_void,                // 27
    pub find_field: *const c_void,                 // 28
    pub find_member_ref: *const c_void,            // 29
    pub get_method_props: unsafe extern "C" fn(   // 30
        *mut c_void, // this
        u32,         // mb (mdMethodDef)
        *mut u32,    // pClass
        *mut u16,    // szMethod
        u32,         // cchMethod
        *mut u32,    // pchMethod
        *mut u32,    // pdwAttr
        *mut *const u8, // ppvSigBlob
        *mut u32,    // pcbSigBlob
        *mut u32,    // pulCodeRVA
        *mut u32,    // pdwImplFlags
    ) -> HResult,
}

/// Nome do método a partir do token (mdMethodDef). Se a vtable acima estiver
/// desalinhada, é aqui que vai crashar ou vir HRESULT sem sentido — sinal
/// de que a contagem dos 27 slots antes de GetMethodProps está errada.
pub unsafe fn get_method_name(metadata: *mut c_void, token: u32) -> Result<String, HResult> {
    let vtbl = *(metadata as *const *const MetaDataImportVtbl);
    let mut buf = vec![0u16; 512];
    let mut written: u32 = 0;
    let hr = ((*vtbl).get_method_props)(
        metadata,
        token,
        std::ptr::null_mut(),
        buf.as_mut_ptr(),
        buf.len() as u32,
        &mut written,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
    );
    if hr != S_OK {
        return Err(hr);
    }
    let len = written.saturating_sub(1) as usize;
    Ok(String::from_utf16_lossy(&buf[..len.min(buf.len())]))
}

// --- ICorDebugReferenceValue / ICorDebugStringValue (dereferenciar tipos por referência) ---

#[repr(C)]
pub struct ReferenceValueVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub get_type: *const c_void,
    pub get_size: *const c_void,
    pub get_address: *const c_void,
    pub create_breakpoint: *const c_void,
    pub is_null: unsafe extern "C" fn(*mut c_void, *mut i32) -> HResult,
    pub get_value: *const c_void,
    pub set_value: *const c_void,
    pub dereference: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> HResult,
    pub dereference_strong: *const c_void,
}

pub unsafe fn is_null(reference_value: *mut c_void) -> Result<bool, HResult> {
    let vtbl = *(reference_value as *const *const ReferenceValueVtbl);
    let mut b: i32 = 0;
    let hr = ((*vtbl).is_null)(reference_value, &mut b);
    if hr == S_OK {
        Ok(b != 0)
    } else {
        Err(hr)
    }
}

pub unsafe fn dereference(reference_value: *mut c_void) -> Result<*mut c_void, HResult> {
    let vtbl = *(reference_value as *const *const ReferenceValueVtbl);
    let mut out: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).dereference)(reference_value, &mut out);
    if hr == S_OK {
        Ok(out)
    } else {
        Err(hr)
    }
}

#[repr(C)]
pub struct StringValueVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub get_type: *const c_void,
    pub get_size: *const c_void,
    pub get_address: *const c_void,
    pub create_breakpoint: *const c_void,
    pub is_valid: *const c_void,             // ICorDebugHeapValue
    pub create_reloc_breakpoint: *const c_void, // ICorDebugHeapValue
    pub get_length: unsafe extern "C" fn(*mut c_void, *mut u32) -> HResult,
    pub get_string: unsafe extern "C" fn(*mut c_void, u32, *mut u32, *mut u16) -> HResult,
}

pub unsafe fn get_string_value(string_value: *mut c_void) -> Result<String, HResult> {
    let vtbl = *(string_value as *const *const StringValueVtbl);
    let mut buf = vec![0u16; 1024];
    let mut written: u32 = 0;
    let hr = ((*vtbl).get_string)(string_value, buf.len() as u32, &mut written, buf.as_mut_ptr());
    if hr != S_OK {
        return Err(hr);
    }
    let len = (written as usize).min(buf.len());
    Ok(String::from_utf16_lossy(&buf[..len]))
}

// --- ICorDebugArrayValue (ordem confirmada no cordebug.idl, mesmo padrão da string) ---

#[repr(C)]
pub struct ArrayValueVtbl {
    pub query_interface: unsafe extern "C" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub get_type: *const c_void,             // 3 (ICorDebugValue)
    pub get_size: *const c_void,             // 4
    pub get_address: *const c_void,          // 5
    pub create_breakpoint: *const c_void,    // 6
    pub is_valid: *const c_void,             // 7 (ICorDebugHeapValue)
    pub create_reloc_breakpoint: *const c_void, // 8
    pub get_element_type: unsafe extern "C" fn(*mut c_void, *mut i32) -> HResult, // 9
    pub get_rank: *const c_void,             // 10
    pub get_count: unsafe extern "C" fn(*mut c_void, *mut u32) -> HResult, // 11
    pub get_dimensions: *const c_void,       // 12
    pub has_base_indicies: *const c_void,    // 13
    pub get_base_indicies: *const c_void,    // 14
    pub get_element: *const c_void,          // 15
    pub get_element_at_position: unsafe extern "C" fn(*mut c_void, u32, *mut *mut c_void) -> HResult, // 16
}

pub unsafe fn get_array_count(array_value: *mut c_void) -> Result<u32, HResult> {
    let vtbl = *(array_value as *const *const ArrayValueVtbl);
    let mut count: u32 = 0;
    let hr = ((*vtbl).get_count)(array_value, &mut count);
    if hr == S_OK {
        Ok(count)
    } else {
        Err(hr)
    }
}

pub unsafe fn get_array_element_type(array_value: *mut c_void) -> Result<i32, HResult> {
    let vtbl = *(array_value as *const *const ArrayValueVtbl);
    let mut t: i32 = 0;
    let hr = ((*vtbl).get_element_type)(array_value, &mut t);
    if hr == S_OK {
        Ok(t)
    } else {
        Err(hr)
    }
}

pub unsafe fn get_array_element_at(array_value: *mut c_void, position: u32) -> Result<*mut c_void, HResult> {
    let vtbl = *(array_value as *const *const ArrayValueVtbl);
    let mut value: *mut c_void = std::ptr::null_mut();
    let hr = ((*vtbl).get_element_at_position)(array_value, position, &mut value);
    if hr == S_OK {
        Ok(value)
    } else {
        Err(hr)
    }
}

/// Enumera todos os tokens de tipo (mdTypeDef) da assembly via EnumTypeDefs,
/// paginando até esgotar (padrão HCORENUM: hEnum começa null, cada chamada
/// preenche até cMax tokens, para quando pcTokens vier 0).
pub unsafe fn enum_type_defs(metadata: *mut c_void) -> Vec<u32> {
    let vtbl = *(metadata as *const *const MetaDataImportVtbl);
    let mut h_enum: *mut c_void = std::ptr::null_mut();
    let mut result = Vec::new();
    loop {
        let mut buf = [0u32; 32];
        let mut count: u32 = 0;
        let hr = ((*vtbl).enum_type_defs)(metadata, &mut h_enum, buf.as_mut_ptr(), buf.len() as u32, &mut count);
        if hr != S_OK || count == 0 {
            break;
        }
        result.extend_from_slice(&buf[..count as usize]);
    }
    if !h_enum.is_null() {
        ((*vtbl).close_enum)(metadata, h_enum);
    }
    result
}

/// Enumera os tokens de método (mdMethodDef) de um tipo, mesmo padrão de paginação.
pub unsafe fn enum_methods(metadata: *mut c_void, type_def: u32) -> Vec<u32> {
    let vtbl = *(metadata as *const *const MetaDataImportVtbl);
    let mut h_enum: *mut c_void = std::ptr::null_mut();
    let mut result = Vec::new();
    loop {
        let mut buf = [0u32; 32];
        let mut count: u32 = 0;
        let hr = ((*vtbl).enum_methods)(
            metadata,
            &mut h_enum,
            type_def,
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut count,
        );
        if hr != S_OK || count == 0 {
            break;
        }
        result.extend_from_slice(&buf[..count as usize]);
    }
    if !h_enum.is_null() {
        ((*vtbl).close_enum)(metadata, h_enum);
    }
    result
}

/// Acha o método de entrada de verdade: percorre todos os tipos e métodos
/// da assembly do usuário procurando por "Main" ou "<Main>$" (top-level
/// statements), em vez de assumir o token 0x06000001.
pub unsafe fn find_entry_point_token(metadata: *mut c_void) -> Option<u32> {
    for type_def in enum_type_defs(metadata) {
        for method_token in enum_methods(metadata, type_def) {
            if let Ok(name) = get_method_name(metadata, method_token) {
                if name == "Main" || name == "<Main>$" {
                    return Some(method_token);
                }
            }
        }
    }
    None
}

// --- ICorDebugManagedCallback: nossa implementação, exposta como objeto COM ---
// 26 métodos + os 3 de IUnknown = 29 slots, na ordem exata do cordebug.idl.
// Assinaturas com *mut c_void genérico pra ponteiros de interface (não
// precisamos interpretar o tipo, só repassar/logar por enquanto).

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

#[repr(C)]
pub struct ManagedCallbackObj {
    pub vtbl: *const ManagedCallbackVtbl,
    pub ref_count: u32,
}

unsafe extern "C" fn cb_query_interface(
    this: *mut c_void,
    _riid: *const Guid,
    ppv: *mut *mut c_void,
) -> HResult {
    // spike: aceita qualquer IID (não checa), suficiente pro runtime aceitar
    // nosso objeto como ICorDebugManagedCallback.
    *ppv = this;
    S_OK
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

unsafe extern "C" fn cb_breakpoint(
    _this: *mut c_void,
    app_domain: *mut c_void,
    thread: *mut c_void,
    breakpoint: *mut c_void,
) -> HResult {
    eprintln!(
        "[callback] SUCESSO: Breakpoint disparou! thread={:?} breakpoint={:?}",
        thread, breakpoint
    );

    match create_stepper(thread) {
        Ok(stepper) => {
            eprintln!("[callback]   Stepper criado: {:?}", stepper);
            let hr_step = step_into(stepper);
            eprintln!("[callback]   Step(bStepIn=TRUE) -> hr=0x{:08x}", hr_step as u32);
            let hr_cont = continue_(app_domain);
            eprintln!("[callback]   Continue() pra deixar o step acontecer -> hr=0x{:08x}", hr_cont as u32);
        }
        Err(hr) => {
            eprintln!("[callback]   FALHA ao criar stepper: hr=0x{:08x}", hr as u32);
        }
    }
    S_OK
}

// Must match events::STEP_EVENT_CAP in the lib crate — duplicated here
// (rather than `use crate::events::STEP_EVENT_CAP`) for the same reason
// STEP_SINK/LIMIT_SINK are indirections: this file is shared with the
// legacy icordebug-spike binary, which has no `events` module.
const STEP_EVENT_CAP: u32 = 5000;

/// Cada StepComplete inspeciona o frame atual e emite UM evento de step.
/// Ao atingir o cap de 5.000 eventos emitidos (`STEP_EVENT_CAP`, mesma
/// decisão de escopo do lado Java — ver jdi/Debugger.java), para de armar
/// um novo stepper e deixa o programa terminar sozinho, sem overhead de
/// instrumentação — emite `step_limit_exceeded` uma única vez nesse
/// momento. Se a inspeção falhar (ex: sem frame gerenciado ativo, perto do
/// fim da execução), não emite nada nesse passo mas continua avançando de
/// qualquer forma.
unsafe extern "C" fn cb_step_complete(
    _this: *mut c_void,
    app_domain: *mut c_void,
    thread: *mut c_void,
    _stepper: *mut c_void,
    _reason: i32,
) -> HResult {
    if !STEP_CAPPED {
        if let Ok(frame) = get_active_frame(thread) {
            if let Ok(il_frame) = query_interface(frame, &IID_ICORDEBUG_IL_FRAME) {
                let offset = get_il_offset(il_frame).unwrap_or(0);
                let line = offset as i64;
                let stack = get_call_stack_names(il_frame);
                let method_token = get_function(il_frame)
                    .and_then(|func| get_function_token(func))
                    .unwrap_or(0);
                let locals = extract_locals(il_frame, method_token, offset);
                if let Some(sink) = STEP_SINK {
                    sink(line, locals, stack);
                }
                STEP_EVENTS_EMITTED += 1;
                if STEP_EVENTS_EMITTED >= STEP_EVENT_CAP {
                    STEP_CAPPED = true;
                    if let Some(sink) = LIMIT_SINK {
                        sink();
                    }
                }
            }
        }
    }

    if !STEP_CAPPED {
        if let Ok(stepper) = create_stepper(thread) {
            step_into(stepper);
        }
    }
    continue_(app_domain);
    S_OK
}

/// Imprime uma variável: primitivo (i32 bruto) ou referência (string
/// dereferenciada via ICorDebugReferenceValue → ICorDebugStringValue).
const ELEMENT_TYPE_STRING: i32 = 0xE;
const ELEMENT_TYPE_ARRAY: i32 = 0x14;
const ELEMENT_TYPE_SZARRAY: i32 = 0x1D;

/// Cap on array elements serialized per local, same value as Java's
/// MAX_ARRAY_ELEMENTS (jdi/Debugger.java::serializeValue) — kept in sync
/// deliberately, so a large array doesn't produce a wildly different
/// payload size just because of which language ran it. C# doesn't need
/// Java's MAX_DEPTH/MAX_FIELDS/cycle-detection machinery yet: locals here
/// are only primitives/strings/flat arrays (no generic-object field
/// serialization — see tasks.md "Fase 2", explicitly out of scope), so
/// there's no recursion depth to cap, only the element count.
const MAX_ARRAY_ELEMENTS: u32 = 20;

/// Dereferencia um valor por referência (string, array, objeto): IsNull +
/// Dereference, retornando o ICorDebugValue de verdade por trás do ponteiro.
unsafe fn dereference_value(value: *mut c_void) -> Result<Option<*mut c_void>, HResult> {
    let ref_value = query_interface(value, &IID_ICORDEBUG_REFERENCE_VALUE)?;
    if is_null(ref_value)? {
        return Ok(None);
    }
    Ok(Some(dereference(ref_value)?))
}

/// Enumera as variáveis locais do frame (índice 0, 1, 2, ... até
/// GetLocalVariable falhar — sinal de que passou do fim da lista de locals
/// da assinatura do método). Chave é o nome real da variável, resolvido via
/// LOCAL_NAME_RESOLVER (leitura do Portable PDB — ver pdb.rs) quando
/// disponível; cai de volta pra "local_N" (índice posicional puro) quando
/// não há resolver setado (icordebug-spike, o binário legado) ou quando o
/// resolver não achou nome pro slot (sem .pdb encontrado, ou índice fora de
/// qualquer LocalScope conhecido).
pub unsafe fn extract_locals(il_frame: *mut c_void, method_token: u32, il_offset: u32) -> BTreeMap<String, serde_json::Value> {
    let names = LOCAL_NAME_RESOLVER.map(|resolve| resolve(method_token, il_offset)).unwrap_or_default();
    let mut locals = BTreeMap::new();
    for i in 0..64u32 {
        let value = match get_local_variable(il_frame, i) {
            Ok(v) => v,
            Err(_) => break,
        };
        let key = names.get(&i).cloned().unwrap_or_else(|| format!("local_{i}"));
        locals.insert(key, local_value_to_json(value));
    }
    locals
}

/// Converte um valor local pra JSON: primitivo (i32 bruto), string
/// dereferenciada, array dereferenciado (elementos lidos como i32), ou null
/// (referência nula ou qualquer falha na extração).
unsafe fn local_value_to_json(value: *mut c_void) -> serde_json::Value {
    let t = get_value_type(value).unwrap_or(-1);

    if t == ELEMENT_TYPE_STRING {
        return match dereference_value(value) {
            Ok(None) => serde_json::Value::Null,
            Ok(Some(dereferenced)) => match query_interface(dereferenced, &IID_ICORDEBUG_STRING_VALUE) {
                Ok(string_value) => match get_string_value(string_value) {
                    Ok(s) => serde_json::json!(s),
                    Err(_) => serde_json::Value::Null,
                },
                Err(_) => serde_json::Value::Null,
            },
            Err(_) => serde_json::Value::Null,
        };
    }

    if t == ELEMENT_TYPE_ARRAY || t == ELEMENT_TYPE_SZARRAY {
        return match dereference_value(value) {
            Ok(None) => serde_json::Value::Null,
            Ok(Some(dereferenced)) => match query_interface(dereferenced, &IID_ICORDEBUG_ARRAY_VALUE) {
                Ok(array_value) => match get_array_count(array_value) {
                    Ok(count) => {
                        // Cap at MAX_ARRAY_ELEMENTS (see doc comment above),
                        // same idea as Java's serializeValue: read only the
                        // capped elements, then append a truncation marker
                        // string (matching Java's "...(+N elementos)"
                        // pattern) instead of silently dropping the rest.
                        let cap = count.min(MAX_ARRAY_ELEMENTS);
                        let mut items = Vec::new();
                        for i in 0..cap {
                            let v = match get_array_element_at(array_value, i) {
                                Ok(elem) => get_value_i32(elem).unwrap_or(0),
                                Err(_) => 0,
                            };
                            items.push(serde_json::json!(v));
                        }
                        if count > cap {
                            items.push(serde_json::json!(format!(
                                "...(+{} elementos)",
                                count - cap
                            )));
                        }
                        serde_json::Value::Array(items)
                    }
                    Err(_) => serde_json::Value::Null,
                },
                Err(_) => serde_json::Value::Null,
            },
            Err(_) => serde_json::Value::Null,
        };
    }

    match get_value_i32(value) {
        Ok(v) => serde_json::json!(v),
        Err(_) => serde_json::Value::Null,
    }
}

unsafe extern "C" fn cb_break(_this: *mut c_void, _ad: *mut c_void, _thread: *mut c_void) -> HResult {
    S_OK
}

unsafe extern "C" fn cb_exception(
    _this: *mut c_void,
    _ad: *mut c_void,
    _thread: *mut c_void,
    _unhandled: i32,
) -> HResult {
    S_OK
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

unsafe extern "C" fn cb_create_thread(
    _this: *mut c_void,
    ad: *mut c_void,
    thread: *mut c_void,
) -> HResult {
    eprintln!("[callback] CreateThread! thread={:?}", thread);
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
            if !name.starts_with("/usr/share/dotnet/") {
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
        Err(hr) => eprintln!("[callback] LoadModule! module={:?} (GetName falhou: 0x{:08x})", module, hr as u32),
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
    breakpoint: cb_breakpoint,
    step_complete: cb_step_complete,
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
