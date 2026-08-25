// Plumbing COM mínimo (ABI Itanium C++, como o CoreCLR expõe no Linux) pra
// falar com ICorDebug/ICorDebugManagedCallback sem nenhuma lib de binding.
// Cada interface COM é, na prática, um ponteiro pra um ponteiro de vtable
// (array de function pointers), na ordem exata declarada no cordebug.idl.
// Layout errado = crash silencioso ou corrupção — testado empiricamente.
//
// Module layout: this file (core plumbing — Guid/IIDs/HResult/IUnknown/
// report_error) plus four submodules —
//   `icordebug`: the client interfaces this driver calls INTO the runtime
//                (ICorDebug, Module, Function, Thread, Stepper, ILFrame).
//   `values`:    value dereferencing (GenericValue/ReferenceValue/
//                StringValue/ArrayValue).
//   `metadata`:  IMetaDataImport (method-name/entry-point lookup).
//   `callback`:  this driver's OWN ICorDebugManagedCallback/
//                ManagedCallback2 implementation, and every `static mut`
//                piece of session state those callbacks share (except
//                FATAL_ERROR/ERROR_SINK, see their doc comments below for
//                why those two stay here instead).
// Every submodule's public items are re-exported below so external callers
// (csharp.rs, icordebug_spike.rs) keep using `com::Whatever` unchanged.

#![allow(dead_code)]
// `unused_imports` on the four `pub use *` re-exports below: expected, not
// a real signal. This file is compiled twice — once as part of the lib
// crate (`sandbox_runner_lib`, where these re-exports genuinely are public
// API surface, so the lint doesn't fire there) and once as a private `mod
// com;` inside the standalone icordebug-spike binary (`icordebug_spike.rs`),
// which only ever names a handful of specific items — so from THAT crate's
// point of view, whichever of these four globs happens to contain none of
// the names icordebug_spike.rs actually uses looks "unused". Same
// architecture, same benign warning, as every `pub static mut` in this tree
// already had before the split (see icordebug_spike.rs's own `use com::{...
// specific names ...}`).

use std::os::raw::c_void;

mod callback;
mod icordebug;
mod metadata;
mod values;

#[allow(unused_imports)]
pub use callback::*;
#[allow(unused_imports)]
pub use icordebug::*;
#[allow(unused_imports)]
pub use metadata::*;
#[allow(unused_imports)]
pub use values::*;

// FATAL_ERROR/ERROR_SINK stay here (not in `callback`, unlike every other
// piece of shared callback session state) because they're genuinely
// cross-cutting: written by `report_error` right below, read by
// `icordebug::continue_` (the single choke point every callback goes
// through — see that function's own doc comment for why FATAL_ERROR being
// set there specifically matters), AND read/written externally by
// csharp::run_worker. Keeping them next to `report_error`, their sole
// internal writer, avoids a needless `callback::FATAL_ERROR` reference from
// `icordebug.rs` for the read side while still being just as reachable.
pub static mut ERROR_SINK: Option<fn(String)> = None;
pub static mut FATAL_ERROR: bool = false;

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
#[derive(Clone, Copy, PartialEq, Eq)]
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
// CORRIGIDO (ver tasks.md/git log — investigação do travamento em exceção
// C# não tratada): este valor estava ERRADO até então (0x3D6F5F62 é o IID
// real de `ICorDebugController`, confirmado byte a byte contra o
// cordebug.idl real do dotnet/runtime — `uuid(3d6f5f62-...)` logo acima de
// `interface ICorDebugController : IUnknown`). Inofensivo enquanto
// cb_query_interface não checava riid nenhum, mas agora que checa (ver
// abaixo) precisa ser o valor certo: o IID real de `ICorDebugManagedCallback`
// é `uuid(3d6f5f60-...)`, confirmado do mesmo jeito, um interface acima na
// mesma família 3d6f5f6X sequencial do arquivo.
pub const IID_ICORDEBUG_MANAGED_CALLBACK: Guid =
    guid!(0x3D6F5F60, 0x7538, 0x11D3, 0x8D, 0x5B, 0x00, 0x10, 0x4B, 0x35, 0xE7, 0xEF);
// IUnknown's own well-known IID (00000000-0000-0000-C000-000000000046) —
// every real COM QueryInterface must accept this in addition to whichever
// concrete interface(s) the object implements.
pub const IID_IUNKNOWN: Guid =
    guid!(0x00000000, 0x0000, 0x0000, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46);
// ICorDebugManagedCallback2's real IID (confirmed against cordebug.idl,
// same discipline as the others). NOT optional in practice: the real
// `Cordb::SetManagedHandler` (dotnet/runtime's src/coreclr/debug/di/
// rsmain.cpp) does `pCallback->QueryInterface(IID_ICorDebugManagedCallback2,
// ...)` and, for any CoreCLR >= 2.0 debuggee (i.e. always, here), returns
// E_NOINTERFACE outright if that fails — there's no default/fallback
// implementation for V2 the way there is for V3/V4
// (DefaultManagedCallback3/4). So a real ICorDebugManagedCallback2 vtable
// (below) is mandatory just to attach at all, not an optional nicety.
pub const IID_ICORDEBUG_MANAGED_CALLBACK2: Guid = guid!(
    0x250E5EEA, 0xDB5C, 0x4C76, 0xB6, 0xF3, 0x8C, 0x46, 0xF1, 0x2E, 0x32, 0x03
);
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
// IIDs de ICorDebugModule2/ICorDebugStepper2, confirmadas direto do
// cordebug.idl fonte (dotnet/runtime, buscado via `curl` antes de escrever
// qualquer código — mesma disciplina das outras IIDs acima), usadas pra
// habilitar Just My Code (JMC): sem isso, o stepper de instrução única
// (`step_into`) entra em TODO o subgrafo de chamadas de código de framework
// (ex: a inicialização preguiçosa de `Console.WriteLine` na primeira
// chamada), inundando o trace com dezenas/centenas de eventos internos do
// BCL antes de voltar pro código do usuário — ver doc comment em
// `create_stepper`/`set_module_jmc_status` (icordebug.rs) pro problema real
// encontrado rodando isso de verdade.
pub const IID_ICORDEBUG_MODULE2: Guid =
    guid!(0x7FCC5FB5, 0x49C0, 0x41DE, 0x99, 0x38, 0x3B, 0x88, 0xB5, 0xB9, 0xAD, 0xD7);
pub const IID_ICORDEBUG_STEPPER2: Guid =
    guid!(0xC5B6E9C3, 0xE7D1, 0x4A8E, 0x87, 0x3B, 0x7F, 0x04, 0x7F, 0x07, 0x06, 0xF7);

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
