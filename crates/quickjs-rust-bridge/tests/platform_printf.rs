#![cfg(not(target_arch = "wasm32"))]
#![allow(
    unsafe_code,
    reason = "these tests exercise the private C variadic formatter ABI directly"
)]

use std::ffi::{c_char, c_int, c_longlong, c_uint};
use std::ptr;

use quickjs_rust_bridge::{EvalOptions, EvalSource, Realm};

unsafe extern "C" {
    fn qjs_platform_snprintf(
        destination: *mut c_char,
        capacity: usize,
        format: *const c_char,
        ...
    ) -> c_int;
}

fn buffer_text(buffer: &[u8]) -> String {
    let end = buffer
        .iter()
        .position(|&byte| byte == 0)
        .expect("formatted output must be NUL terminated");
    String::from_utf8(buffer[..end].to_vec()).expect("the test formats contain ASCII only")
}

fn evaluate_string(realm: &mut Realm, source: &str) -> String {
    let value = realm
        .evaluate(EvalSource::new(source), EvalOptions::default())
        .expect("string expression should evaluate");
    String::from_utf16(&value.to_utf16().expect("expression should return a string"))
        .expect("the test result should be well-formed UTF-16")
}

#[test]
fn snprintf_obeys_c99_truncation_and_length_semantics() {
    let mut truncated = [b'!'; 5];
    // SAFETY: every pointer is live for the call and the variadic argument
    // matches the `%s` conversion's promoted C type.
    let required = unsafe {
        qjs_platform_snprintf(
            truncated.as_mut_ptr().cast(),
            truncated.len(),
            c"%s".as_ptr(),
            c"abcdef".as_ptr(),
        )
    };
    assert_eq!(required, 6);
    assert_eq!(buffer_text(&truncated), "abcd");

    let mut exact = [b'!'; 7];
    // SAFETY: the seven-byte destination fits six payload bytes and the
    // required terminator exactly.
    let required = unsafe {
        qjs_platform_snprintf(
            exact.as_mut_ptr().cast(),
            exact.len(),
            c"%s".as_ptr(),
            c"abcdef".as_ptr(),
        )
    };
    assert_eq!(required, 6);
    assert_eq!(&exact, b"abcdef\0");

    let mut one_byte = [b'!'; 1];
    // SAFETY: same valid format and argument as above; the destination has the
    // advertised one-byte capacity.
    let required = unsafe {
        qjs_platform_snprintf(
            one_byte.as_mut_ptr().cast(),
            one_byte.len(),
            c"%s".as_ptr(),
            c"abcdef".as_ptr(),
        )
    };
    assert_eq!(required, 6);
    assert_eq!(one_byte, [0]);

    let mut untouched = [b'!'; 1];
    // SAFETY: C99 permits a zero-sized destination; no byte may be accessed.
    let required = unsafe {
        qjs_platform_snprintf(
            untouched.as_mut_ptr().cast(),
            0,
            c"%s".as_ptr(),
            c"abcdef".as_ptr(),
        )
    };
    assert_eq!(required, 6);
    assert_eq!(untouched, [b'!']);

    // SAFETY: a null destination is valid when capacity is zero, and the
    // formatter still computes the complete required length.
    let required =
        unsafe { qjs_platform_snprintf(ptr::null_mut(), 0, c"%s".as_ptr(), c"abcdef".as_ptr()) };
    assert_eq!(required, 6);
}

#[test]
fn snprintf_supports_the_quickjs_integer_and_dynamic_format_set() {
    let mut buffer = [0_u8; 160];
    // SAFETY: the arguments exactly match their promoted C conversion types.
    let required = unsafe {
        qjs_platform_snprintf(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            c"%+07d|%0*d|%.*s|%02x|%c|%lld|%zu|%#x|%b|%d".as_ptr(),
            42 as c_int,
            6 as c_int,
            -42 as c_int,
            3 as c_int,
            c"abcdef".as_ptr(),
            10 as c_uint,
            c_int::from(b'Q'),
            c_longlong::MIN,
            usize::MAX,
            10 as c_uint,
            5 as c_uint,
            9 as c_int,
        )
    };
    let expected = format!(
        "+000042|-00042|abc|0a|Q|{}|{}|0xa|101|9",
        c_longlong::MIN,
        usize::MAX
    );
    assert_eq!(
        usize::try_from(required).expect("the formatter returned a negative length"),
        expected.len()
    );
    assert_eq!(buffer_text(&buffer), expected);
}

#[test]
fn snprintf_float_conversions_consume_their_arguments() {
    let mut buffer = [0_u8; 160];
    // Keep an integer after every floating-point family. If any family is
    // compiled out, nanoprintf emits the conversion literally without
    // consuming its double and the following integer exposes the va_list
    // desynchronization.
    // SAFETY: the arguments exactly match their promoted C conversion types.
    let required = unsafe {
        qjs_platform_snprintf(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            c"%.2f|%d|%.1e|%d|%.3g|%d|%a|%d".as_ptr(),
            1.25_f64,
            2 as c_int,
            12.0_f64,
            3 as c_int,
            125.0_f64,
            4 as c_int,
            1.5_f64,
            5 as c_int,
        )
    };
    let output = buffer_text(&buffer);

    assert_eq!(output, "1.25|2|12.0|3|125.000|4|0x1.8000000000000p+0|5");
    assert_eq!(
        usize::try_from(required).expect("the formatter returned a negative length"),
        output.len()
    );
}

#[test]
fn snprintf_writeback_consumes_its_pointer_and_preserves_later_arguments() {
    let mut buffer = [0_u8; 32];
    let mut written = -1 as c_int;
    // SAFETY: `written` is live and writable for the `%n` conversion, and the
    // following integer has the promoted type required by `%d`.
    let required = unsafe {
        qjs_platform_snprintf(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            c"abc%n|%d".as_ptr(),
            &mut written,
            7 as c_int,
        )
    };

    assert_eq!(written, 3);
    assert_eq!(required, 5);
    assert_eq!(buffer_text(&buffer), "abc|7");
}

#[test]
fn quickjs_date_formatting_uses_the_private_formatter() {
    let mut realm = Realm::new().expect("realm should initialize");
    let formatted = evaluate_string(
        &mut realm,
        "(() => [
            new Date(0).toISOString(),
            new Date(Date.UTC(-1, 5, 7, 8, 9, 10, 11)).toISOString(),
            new Date(Date.UTC(12345, 5, 7, 8, 9, 10, 11)).toISOString(),
            new Date(0).toUTCString(),
        ].join('|'))()",
    );

    assert_eq!(
        formatted,
        "1970-01-01T00:00:00.000Z|-000001-06-07T08:09:10.011Z|\
         +012345-06-07T08:09:10.011Z|Thu, 01 Jan 1970 00:00:00 GMT"
    );
}

#[test]
fn quickjs_errors_preserve_formatted_arguments() {
    let mut realm = Realm::new().expect("realm should initialize");
    let error = realm
        .evaluate(
            EvalSource::new("null.formattedProperty"),
            EvalOptions::default(),
        )
        .expect_err("property access should throw");

    assert_eq!(
        error.message,
        "cannot read property 'formattedProperty' of null"
    );
}

#[test]
fn quickjs_regexp_errors_preserve_formatted_characters() {
    let mut realm = Realm::new().expect("realm should initialize");
    let error = realm
        .evaluate(
            EvalSource::new("new RegExp('(?i:a')"),
            EvalOptions::default(),
        )
        .expect_err("the unterminated modifier group should be rejected");

    assert_eq!(error.message, "expecting ')'");
}

#[test]
fn quickjs_backtrace_expands_past_dbufs_stack_buffer() {
    let mut realm = Realm::new().expect("realm should initialize");
    let source_name = format!("bundle://{}.js", "long-name-".repeat(40));
    assert!(source_name.len() > 128);
    let error = realm
        .evaluate(
            EvalSource {
                text: "(() => { throw new Error('formatted stack'); })()",
                name: Some(&source_name),
                line_offset: 0,
            },
            EvalOptions::default(),
        )
        .expect_err("the script should throw");
    let stack = error.stack.expect("QuickJS should attach a stack");

    assert!(
        stack.contains(&source_name),
        "the >128-byte source name was truncated from the stack: {stack:?}"
    );
}
