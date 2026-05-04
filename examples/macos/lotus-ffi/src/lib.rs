use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use lotus_core::Sheet;

#[no_mangle]
pub extern "C" fn lotus_sheet_new() -> *mut Sheet {
    Box::into_raw(Box::new(Sheet::new()))
}

/// # Safety
/// `sheet` must be a pointer returned by `lotus_sheet_new` and not yet freed.
#[no_mangle]
pub unsafe extern "C" fn lotus_sheet_free(sheet: *mut Sheet) {
    if !sheet.is_null() {
        drop(Box::from_raw(sheet));
    }
}

/// Set a cell's raw input and recalculate the whole sheet.
///
/// Returns `NULL` on success, or an owned C string describing the error
/// (e.g. `"#CIRCULAR!"`). The caller must free any returned string with
/// `lotus_string_free`.
///
/// # Safety
/// `sheet` must be valid; `cell` and `value` must be NUL-terminated UTF-8.
#[no_mangle]
pub unsafe extern "C" fn lotus_sheet_set(
    sheet: *mut Sheet,
    cell: *const c_char,
    value: *const c_char,
) -> *mut c_char {
    let Some(sheet) = sheet.as_mut() else {
        return into_owned("null sheet");
    };
    let cell = match cstr(cell) {
        Ok(s) => s.to_string(),
        Err(e) => return into_owned(e),
    };
    let value = match cstr(value) {
        Ok(s) => s.to_string(),
        Err(e) => return into_owned(e),
    };
    match sheet.set_cells(&[(cell, value)]) {
        Ok(()) => ptr::null_mut(),
        Err(e) => into_owned(&e),
    }
}

/// Get a cell's computed, display-formatted value. Always returns an owned
/// C string (empty string for empty cells). Free with `lotus_string_free`.
///
/// # Safety
/// `sheet` must be valid; `cell` must be NUL-terminated UTF-8.
#[no_mangle]
pub unsafe extern "C" fn lotus_sheet_get(
    sheet: *const Sheet,
    cell: *const c_char,
) -> *mut c_char {
    let Some(sheet) = sheet.as_ref() else {
        return into_owned("");
    };
    let Ok(cell) = cstr(cell) else {
        return into_owned("");
    };
    into_owned(&format!("{}", sheet.get(cell)))
}

/// Get a cell's raw input (the formula or literal text the user entered).
/// Returns empty string if unset. Free with `lotus_string_free`.
///
/// # Safety
/// `sheet` must be valid; `cell` must be NUL-terminated UTF-8.
#[no_mangle]
pub unsafe extern "C" fn lotus_sheet_get_raw(
    sheet: *const Sheet,
    cell: *const c_char,
) -> *mut c_char {
    let Some(sheet) = sheet.as_ref() else {
        return into_owned("");
    };
    let Ok(cell) = cstr(cell) else {
        return into_owned("");
    };
    into_owned(sheet.get_raw(cell).unwrap_or(""))
}

/// Free a string previously returned by this library.
///
/// # Safety
/// `s` must be a pointer returned by one of the `lotus_*` functions above,
/// or NULL. Do not pass strings allocated elsewhere.
#[no_mangle]
pub unsafe extern "C" fn lotus_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

unsafe fn cstr<'a>(p: *const c_char) -> Result<&'a str, &'static str> {
    if p.is_null() {
        return Err("null string");
    }
    CStr::from_ptr(p).to_str().map_err(|_| "invalid utf8")
}

fn into_owned(s: &str) -> *mut c_char {
    CString::new(s)
        .unwrap_or_else(|_| CString::new("error").unwrap())
        .into_raw()
}
