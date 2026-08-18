//! Rust standard-library hooks used by the pinned `QuickJS` C build.

use std::cmp::Ordering;
use std::ffi::{CStr, c_char, c_double, c_int, c_long, c_void};
use std::{ptr, slice};

fn c_byte(byte: c_int) -> u8 {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the C string functions convert their int argument to unsigned char"
    )]
    let byte = byte as u8;
    byte
}

/// Terminates the process through Rust's standard library.
#[unsafe(no_mangle)]
pub extern "C" fn qjs_rust_abort() -> ! {
    std::process::abort()
}

/// Computes the inverse hyperbolic tangent through Rust's standard library.
#[unsafe(no_mangle)]
pub extern "C" fn qjs_rust_atanh(value: c_double) -> c_double {
    value.atanh()
}

/// Rounds to the nearest integer, with halfway cases rounded to even.
///
/// The active `QuickJS` caller supplies only finite values in `[0, 255]`, so
/// Rust's saturating float-to-integer behavior outside the C `lrint` domain is
/// deliberately irrelevant to this private ABI.
#[unsafe(no_mangle)]
#[allow(
    clippy::cast_possible_truncation,
    reason = "QuickJS constrains this private lrint call to the c_long range"
)]
pub extern "C" fn qjs_rust_lrint(value: c_double) -> c_long {
    value.round_ties_even() as c_long
}

/// Finds a byte in a bounded C memory region.
///
/// # Safety
///
/// When `length` is non-zero, `pointer` must address `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qjs_rust_memchr(
    pointer: *const c_void,
    byte: c_int,
    length: usize,
) -> *mut c_void {
    if length == 0 {
        return ptr::null_mut();
    }

    // SAFETY: the private C ABI requires a readable region of `length` bytes.
    let bytes = unsafe { slice::from_raw_parts(pointer.cast::<u8>(), length) };
    let Some(offset) = bytes
        .iter()
        .position(|candidate| *candidate == c_byte(byte))
    else {
        return ptr::null_mut();
    };

    // SAFETY: `offset` was found inside the `length`-byte input region.
    unsafe { pointer.cast::<u8>().add(offset).cast_mut().cast() }
}

/// Finds the first matching byte in a NUL-terminated C string.
///
/// # Safety
///
/// `string` must point to a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qjs_rust_strchr(string: *const c_char, byte: c_int) -> *mut c_char {
    // SAFETY: the private C ABI requires a valid NUL-terminated string.
    let bytes = unsafe { CStr::from_ptr(string) }.to_bytes_with_nul();
    let Some(offset) = bytes
        .iter()
        .position(|candidate| *candidate == c_byte(byte))
    else {
        return ptr::null_mut();
    };

    // SAFETY: `offset` selects a byte in the source string, including its NUL.
    unsafe { string.add(offset).cast_mut() }
}

/// Compares two NUL-terminated C strings as sequences of unsigned bytes.
///
/// # Safety
///
/// `left` and `right` must each point to a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qjs_rust_strcmp(left: *const c_char, right: *const c_char) -> c_int {
    // SAFETY: the private C ABI requires two valid NUL-terminated strings.
    let left = unsafe { CStr::from_ptr(left) }.to_bytes();
    // SAFETY: the private C ABI requires two valid NUL-terminated strings.
    let right = unsafe { CStr::from_ptr(right) }.to_bytes();

    match left.cmp(right) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// Finds the last matching byte in a NUL-terminated C string.
///
/// # Safety
///
/// `string` must point to a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qjs_rust_strrchr(string: *const c_char, byte: c_int) -> *mut c_char {
    // SAFETY: the private C ABI requires a valid NUL-terminated string.
    let bytes = unsafe { CStr::from_ptr(string) }.to_bytes_with_nul();
    let Some(offset) = bytes
        .iter()
        .rposition(|candidate| *candidate == c_byte(byte))
    else {
        return ptr::null_mut();
    };

    // SAFETY: `offset` selects a byte in the source string, including its NUL.
    unsafe { string.add(offset).cast_mut() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abort_hook_has_the_expected_noreturn_abi() {
        let hook: extern "C" fn() -> ! = qjs_rust_abort;
        let _ = hook;
    }

    #[test]
    fn atanh_matches_rusts_float_operation() {
        for value in [-0.75, -0.0, 0.0, 0.25, 0.75] {
            assert_eq!(qjs_rust_atanh(value).to_bits(), value.atanh().to_bits());
        }
    }

    #[test]
    fn lrint_rounds_halfway_values_to_even() {
        for (value, expected) in [(0.5, 0), (1.5, 2), (2.5, 2), (3.5, 4), (254.5, 254)] {
            assert_eq!(qjs_rust_lrint(value), expected);
        }
    }

    #[test]
    fn memchr_honors_the_bound_and_returns_the_source_pointer() {
        let bytes = b"bcad";

        // SAFETY: the byte array is readable for the supplied bounds.
        let found = unsafe { qjs_rust_memchr(bytes.as_ptr().cast(), i32::from(b'a'), bytes.len()) };
        assert_eq!(found.cast_const(), bytes.as_ptr().wrapping_add(2).cast());
        // SAFETY: only the first two bytes are made visible to the hook.
        let not_found = unsafe { qjs_rust_memchr(bytes.as_ptr().cast(), i32::from(b'a'), 2) };
        assert!(not_found.is_null());
        // SAFETY: C permits an arbitrary pointer when the requested length is zero.
        let null_empty = unsafe { qjs_rust_memchr(ptr::null(), 0, 0) };
        assert!(null_empty.is_null());
    }

    #[test]
    fn strchr_includes_the_terminator_and_converts_the_needle_to_a_byte() {
        let string = c"abca";
        let start = string.as_ptr();

        // SAFETY: `string` is a live NUL-terminated C string.
        assert_eq!(
            unsafe { qjs_rust_strchr(start, i32::from(b'a')) },
            start.cast_mut()
        );
        // SAFETY: `string` is a live NUL-terminated C string.
        assert_eq!(
            unsafe { qjs_rust_strchr(start, i32::from(b'a') + 256) },
            start.cast_mut()
        );
        // SAFETY: `string` is a live NUL-terminated C string.
        assert_eq!(
            unsafe { qjs_rust_strchr(start, 0) },
            start.wrapping_add(4).cast_mut()
        );
        // SAFETY: `string` is a live NUL-terminated C string.
        assert!(unsafe { qjs_rust_strchr(start, i32::from(b'z')) }.is_null());
    }

    #[test]
    fn strcmp_orders_strings_as_unsigned_bytes() {
        // SAFETY: all operands are live NUL-terminated C strings.
        unsafe {
            assert_eq!(qjs_rust_strcmp(c"same".as_ptr(), c"same".as_ptr()), 0);
            assert!(qjs_rust_strcmp(c"short".as_ptr(), c"shorter".as_ptr()) < 0);
            assert!(qjs_rust_strcmp(c"\xFF".as_ptr(), c"\x7F".as_ptr()) > 0);
        }
    }

    #[test]
    fn strrchr_returns_the_last_match_and_includes_the_terminator() {
        let string = c"abca";
        let start = string.as_ptr();

        // SAFETY: `string` is a live NUL-terminated C string.
        assert_eq!(
            unsafe { qjs_rust_strrchr(start, i32::from(b'a')) },
            start.wrapping_add(3).cast_mut()
        );
        // SAFETY: `string` is a live NUL-terminated C string.
        assert_eq!(
            unsafe { qjs_rust_strrchr(start, 0) },
            start.wrapping_add(4).cast_mut()
        );
        // SAFETY: `string` is a live NUL-terminated C string.
        assert!(unsafe { qjs_rust_strrchr(start, i32::from(b'z')) }.is_null());
    }
}
