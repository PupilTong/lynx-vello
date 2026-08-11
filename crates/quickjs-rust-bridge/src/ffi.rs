//! Private ABI boundary implemented by `shim.c`.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_double, c_int, c_void};

pub(crate) type InterruptCallback = unsafe extern "C" fn(opaque: *mut c_void) -> c_int;

pub(crate) const HOST_ARG_UNDEFINED: i32 = 0;
pub(crate) const HOST_ARG_NULL: i32 = 1;
pub(crate) const HOST_ARG_BOOLEAN: i32 = 2;
pub(crate) const HOST_ARG_NUMBER: i32 = 3;
pub(crate) const HOST_ARG_STRING: i32 = 4;
pub(crate) const HOST_ARG_OBJECT: i32 = 5;

#[repr(C)]
pub(crate) struct QjsHostArg {
    pub(crate) kind: i32,
    pub(crate) number: c_double,
    pub(crate) text: *const u8,
    pub(crate) text_len: usize,
    pub(crate) payload: u32,
}

#[repr(C)]
pub(crate) struct QjsHostResult {
    pub(crate) kind: i32,
    pub(crate) number: c_double,
    pub(crate) text: *const u16,
    pub(crate) text_len: usize,
    pub(crate) payload: u32,
}

pub(crate) type HostDispatch = unsafe extern "C" fn(
    opaque: *mut c_void,
    handler: *mut c_void,
    argument_count: usize,
    arguments: *const QjsHostArg,
    result: *mut QjsHostResult,
) -> c_int;

pub(crate) type HostRelease = unsafe extern "C" fn(opaque: *mut c_void, handler: *mut c_void);

/// Called from the garbage collector for a bridge-owned host object.
///
/// `payload` is returned if JavaScript-object construction fails or when
/// `QuickJS` finalizes the completed object. The callback must not enter
/// `QuickJS` or invoke arbitrary user code.
pub(crate) type HostObjectRelease = unsafe extern "C" fn(opaque: *mut c_void, payload: u32);

#[repr(C)]
pub(crate) struct QjsRuntime {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct JSContext {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct QjsValue {
    _private: [u8; 0],
}

unsafe extern "C" {
    pub(crate) fn qjs_runtime_new() -> *mut QjsRuntime;
    pub(crate) fn qjs_runtime_free(runtime: *mut QjsRuntime);
    pub(crate) fn qjs_context_new(runtime: *mut QjsRuntime) -> *mut JSContext;
    pub(crate) fn qjs_context_free(context: *mut JSContext);
    pub(crate) fn qjs_runtime_run_gc(runtime: *mut QjsRuntime);
    pub(crate) fn qjs_runtime_set_memory_limit(runtime: *mut QjsRuntime, limit: usize);
    pub(crate) fn qjs_runtime_set_max_stack_size(runtime: *mut QjsRuntime, size: usize);
    pub(crate) fn qjs_runtime_set_interrupt_handler(
        runtime: *mut QjsRuntime,
        callback: Option<InterruptCallback>,
        opaque: *mut c_void,
    );

    pub(crate) fn qjs_new_undefined(context: *mut JSContext) -> *mut QjsValue;
    pub(crate) fn qjs_new_null(context: *mut JSContext) -> *mut QjsValue;
    pub(crate) fn qjs_new_boolean(context: *mut JSContext, value: c_int) -> *mut QjsValue;
    pub(crate) fn qjs_new_number(context: *mut JSContext, value: c_double) -> *mut QjsValue;
    pub(crate) fn qjs_new_big_int64(context: *mut JSContext, value: i64) -> *mut QjsValue;
    pub(crate) fn qjs_new_big_uint64(context: *mut JSContext, value: u64) -> *mut QjsValue;
    pub(crate) fn qjs_new_string_utf16(
        context: *mut JSContext,
        units: *const u16,
        length: usize,
    ) -> *mut QjsValue;
    pub(crate) fn qjs_value_free(context: *mut JSContext, value: *mut QjsValue);
    pub(crate) fn qjs_value_kind(context: *mut JSContext, value: *const QjsValue) -> c_int;
    pub(crate) fn qjs_value_get_boolean(
        context: *mut JSContext,
        value: *const QjsValue,
        result: *mut c_int,
    ) -> c_int;
    pub(crate) fn qjs_value_get_number(
        context: *mut JSContext,
        value: *const QjsValue,
        result: *mut c_double,
    ) -> c_int;
    pub(crate) fn qjs_value_to_cesu8(
        context: *mut JSContext,
        value: *const QjsValue,
        bytes: *mut *const u8,
        length: *mut usize,
    ) -> c_int;
    pub(crate) fn qjs_cesu8_free(context: *mut JSContext, bytes: *const u8);

    pub(crate) fn qjs_eval(
        context: *mut JSContext,
        source: *const u8,
        source_length: usize,
        source_name: *const c_char,
        flags: c_int,
        failure_stage: *mut c_int,
    ) -> *mut QjsValue;
    pub(crate) fn qjs_call(
        context: *mut JSContext,
        callable: *const QjsValue,
        this_value: *const QjsValue,
        argument_count: usize,
        arguments: *const *const QjsValue,
    ) -> *mut QjsValue;
    pub(crate) fn qjs_execute_pending_job(
        runtime: *mut QjsRuntime,
        context: *mut *mut JSContext,
    ) -> c_int;
    pub(crate) fn qjs_has_pending_job(runtime: *mut QjsRuntime) -> c_int;
    pub(crate) fn qjs_has_unhandled_rejection(runtime: *mut QjsRuntime) -> c_int;
    pub(crate) fn qjs_take_unhandled_rejection(runtime: *mut QjsRuntime) -> *mut QjsValue;
    pub(crate) fn qjs_take_exception(context: *mut JSContext) -> *mut QjsValue;
    pub(crate) fn qjs_discard_exception(context: *mut JSContext);
    pub(crate) fn qjs_get_property(
        context: *mut JSContext,
        value: *const QjsValue,
        name: *const c_char,
    ) -> *mut QjsValue;
    pub(crate) fn qjs_set_property(
        context: *mut JSContext,
        target: *const QjsValue,
        name: *const c_char,
        value: *const QjsValue,
    ) -> c_int;
    pub(crate) fn qjs_global_object(context: *mut JSContext) -> *mut QjsValue;
    pub(crate) fn qjs_throw_error(context: *mut JSContext, message: *const c_char);
    pub(crate) fn qjs_runtime_set_host_dispatch(
        runtime: *mut QjsRuntime,
        dispatch: Option<HostDispatch>,
        release: Option<HostRelease>,
        object_release: Option<HostObjectRelease>,
        opaque: *mut c_void,
    );
    pub(crate) fn qjs_new_host_function(
        context: *mut JSContext,
        name: *const c_char,
        length: c_int,
        handler: *mut c_void,
    ) -> *mut QjsValue;
}
