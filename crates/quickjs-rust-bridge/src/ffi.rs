//! Private ABI boundary implemented by `shim.c`.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_double, c_int, c_void};

pub(crate) type InterruptCallback = unsafe extern "C" fn(opaque: *mut c_void) -> c_int;

pub(crate) const HOST_ARG_UNDEFINED: i32 = 0;
pub(crate) const HOST_ARG_NULL: i32 = 1;
pub(crate) const HOST_ARG_BOOLEAN: i32 = 2;
pub(crate) const HOST_ARG_NUMBER: i32 = 3;
pub(crate) const HOST_ARG_STRING: i32 = 4;

pub(crate) const REJECTION_NONE: i32 = 0;
pub(crate) const REJECTION_TAKEN: i32 = 1;

#[repr(C)]
pub(crate) struct QjsHostArg {
    pub(crate) kind: i32,
    pub(crate) number: c_double,
    pub(crate) text: *const u8,
    pub(crate) text_len: usize,
}

/// The result of a host call has the same shape an argument does: both are
/// the boundary's primitives-only vocabulary, and both spell text as UTF-8.
pub(crate) type QjsHostResult = QjsHostArg;

pub(crate) type HostDispatch = unsafe extern "C" fn(
    opaque: *mut c_void,
    context: *mut QjsContext,
    handler: *mut c_void,
    argument_count: usize,
    arguments: *const QjsHostArg,
    result: *mut QjsHostResult,
) -> c_int;

pub(crate) type HostRelease = unsafe extern "C" fn(opaque: *mut c_void, handler: *mut c_void);

#[repr(C)]
pub(crate) struct QjsRuntime {
    _private: [u8; 0],
}

/// One realm: a `JSContext` plus the module state that belongs to it.
#[repr(C)]
pub(crate) struct QjsContext {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct QjsValue {
    _private: [u8; 0],
}

unsafe extern "C" {
    pub(crate) fn qjs_host_owner_class_id_new() -> u32;
    pub(crate) fn qjs_runtime_new(host_owner_class_id: u32) -> *mut QjsRuntime;
    pub(crate) fn qjs_runtime_free(runtime: *mut QjsRuntime);
    pub(crate) fn qjs_context_new(runtime: *mut QjsRuntime) -> *mut QjsContext;
    pub(crate) fn qjs_context_free(context: *mut QjsContext);
    pub(crate) fn qjs_runtime_run_gc(runtime: *mut QjsRuntime);
    pub(crate) fn qjs_runtime_set_memory_limit(runtime: *mut QjsRuntime, limit: usize);
    pub(crate) fn qjs_runtime_set_max_stack_size(runtime: *mut QjsRuntime, size: usize);
    pub(crate) fn qjs_runtime_set_interrupt_handler(
        runtime: *mut QjsRuntime,
        callback: Option<InterruptCallback>,
        opaque: *mut c_void,
    );
    pub(crate) fn qjs_runtime_add_module(
        runtime: *mut QjsRuntime,
        name: *const c_char,
        source: *const u8,
        source_length: usize,
    ) -> c_int;
    pub(crate) fn qjs_context_add_host_module_export(
        context: *mut QjsContext,
        name: *const c_char,
        export_name: *const c_char,
        value: *const QjsValue,
    ) -> c_int;
    pub(crate) fn qjs_module_namespace(
        context: *mut QjsContext,
        name: *const c_char,
    ) -> *mut QjsValue;

    pub(crate) fn qjs_new_undefined(context: *mut QjsContext) -> *mut QjsValue;
    pub(crate) fn qjs_new_null(context: *mut QjsContext) -> *mut QjsValue;
    pub(crate) fn qjs_new_boolean(context: *mut QjsContext, value: c_int) -> *mut QjsValue;
    pub(crate) fn qjs_new_number(context: *mut QjsContext, value: c_double) -> *mut QjsValue;
    pub(crate) fn qjs_new_big_int64(context: *mut QjsContext, value: i64) -> *mut QjsValue;
    pub(crate) fn qjs_new_big_uint64(context: *mut QjsContext, value: u64) -> *mut QjsValue;
    pub(crate) fn qjs_new_string_utf16(
        context: *mut QjsContext,
        units: *const u16,
        length: usize,
    ) -> *mut QjsValue;
    pub(crate) fn qjs_new_string_utf8(
        context: *mut QjsContext,
        bytes: *const u8,
        length: usize,
    ) -> *mut QjsValue;
    pub(crate) fn qjs_atom_new(context: *mut QjsContext, bytes: *const u8, length: usize) -> u32;
    pub(crate) fn qjs_atom_free(runtime: *mut QjsRuntime, atom: u32);
    pub(crate) fn qjs_value_free(context: *mut QjsContext, value: *mut QjsValue);
    pub(crate) fn qjs_value_kind(context: *mut QjsContext, value: *const QjsValue) -> c_int;
    pub(crate) fn qjs_value_get_boolean(
        context: *mut QjsContext,
        value: *const QjsValue,
        result: *mut c_int,
    ) -> c_int;
    pub(crate) fn qjs_value_get_number(
        context: *mut QjsContext,
        value: *const QjsValue,
        result: *mut c_double,
    ) -> c_int;
    pub(crate) fn qjs_value_promise_state(
        context: *mut QjsContext,
        value: *const QjsValue,
    ) -> c_int;
    pub(crate) fn qjs_value_promise_result(
        context: *mut QjsContext,
        value: *const QjsValue,
    ) -> *mut QjsValue;
    pub(crate) fn qjs_value_to_cesu8(
        context: *mut QjsContext,
        value: *const QjsValue,
        bytes: *mut *const u8,
        length: *mut usize,
    ) -> c_int;
    pub(crate) fn qjs_cesu8_free(context: *mut QjsContext, bytes: *const u8);

    pub(crate) fn qjs_eval(
        context: *mut QjsContext,
        source: *const u8,
        source_length: usize,
        source_name: *const c_char,
        flags: c_int,
        failure_stage: *mut c_int,
    ) -> *mut QjsValue;
    pub(crate) fn qjs_call(
        context: *mut QjsContext,
        callable: *const QjsValue,
        this_value: *const QjsValue,
        argument_count: usize,
        arguments: *const *const QjsValue,
    ) -> *mut QjsValue;
    pub(crate) fn qjs_call_member(
        context: *mut QjsContext,
        target: *const QjsValue,
        atom: u32,
        argument_count: usize,
        arguments: *const QjsHostArg,
        result: *mut *mut QjsValue,
    ) -> c_int;
    pub(crate) fn qjs_execute_pending_job(
        runtime: *mut QjsRuntime,
        context: *mut *mut QjsContext,
    ) -> c_int;
    pub(crate) fn qjs_has_pending_job(runtime: *mut QjsRuntime) -> c_int;
    pub(crate) fn qjs_take_unhandled_rejection(
        runtime: *mut QjsRuntime,
        context: *mut *mut QjsContext,
        value: *mut *mut QjsValue,
    ) -> c_int;
    pub(crate) fn qjs_take_exception(context: *mut QjsContext) -> *mut QjsValue;
    pub(crate) fn qjs_discard_exception(context: *mut QjsContext);
    pub(crate) fn qjs_get_property(
        context: *mut QjsContext,
        value: *const QjsValue,
        name: *const c_char,
    ) -> *mut QjsValue;
    pub(crate) fn qjs_set_property(
        context: *mut QjsContext,
        target: *const QjsValue,
        name: *const c_char,
        value: *const QjsValue,
    ) -> c_int;
    pub(crate) fn qjs_global_object(context: *mut QjsContext) -> *mut QjsValue;
    pub(crate) fn qjs_throw_error(context: *mut QjsContext, message: *const c_char);
    pub(crate) fn qjs_runtime_set_host_dispatch(
        runtime: *mut QjsRuntime,
        dispatch: Option<HostDispatch>,
        release: Option<HostRelease>,
        opaque: *mut c_void,
    );
    pub(crate) fn qjs_new_host_function(
        context: *mut QjsContext,
        name: *const c_char,
        length: c_int,
        handler: *mut c_void,
    ) -> *mut QjsValue;
}
