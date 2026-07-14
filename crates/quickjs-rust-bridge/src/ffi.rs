//! Private ABI boundary implemented by `shim.c`.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_double, c_int, c_void};

pub(crate) type InterruptCallback = unsafe extern "C" fn(opaque: *mut c_void) -> c_int;

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
}
