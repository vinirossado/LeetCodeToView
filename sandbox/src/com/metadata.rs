// IMetaDataImport — nomes de métodos/tipos vêm daqui (metadata da própria
// assembly), diferente de nomes de variável local, que só existem no PDB
// (ver pdb.rs). Interface grande (~60 métodos), ordem de cor.h
// (dotnet/runtime). Cada slot não usado abaixo está nomeado (não só como
// placeholder anônimo) pra dar pra auditar/contar se algo crashar.
// GetMethodProps é o slot 30 (0-indexed).

use std::os::raw::c_void;

use super::{Guid, HResult, S_OK};

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
