// ICorDebugValue / ICorDebugGenericValue (valores primitivos) e as
// interfaces de dereferenciar tipos por referência (string, array).

use std::os::raw::c_void;

use super::{Guid, HResult, S_OK};

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
