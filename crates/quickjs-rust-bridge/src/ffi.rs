//! Private ABI boundary implemented by `shim.c`.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_double, c_int, c_void};

pub(crate) type InterruptCallback = unsafe extern "C" fn(opaque: *mut c_void) -> c_int;

pub(crate) const HOST_ARG_UNDEFINED: i32 = 0;
pub(crate) const HOST_ARG_NULL: i32 = 1;
pub(crate) const HOST_ARG_BOOLEAN: i32 = 2;
pub(crate) const HOST_ARG_NUMBER: i32 = 3;
pub(crate) const HOST_ARG_STRING: i32 = 4;
pub(crate) const HOST_ARG_STRUCTURED: i32 = 6;
pub(crate) const HOST_ARG_BYTES: i32 = 8;
pub(crate) const HOST_ARG_BYTES_OWNED: i32 = 9;
pub(crate) const HOST_ARG_TRANSFERRED: i32 = 10;
pub(crate) const HOST_ARG_REVIVE: i32 = 11;

pub(crate) const REJECTION_NONE: i32 = 0;
pub(crate) const REJECTION_TAKEN: i32 = 1;

#[repr(C)]
pub(crate) struct QjsHostArg {
    pub(crate) kind: i32,
    pub(crate) number: c_double,
    pub(crate) text: *const u8,
    pub(crate) text_len: usize,
}

pub(crate) type QjsHostResult = QjsHostArg;

#[repr(C)]
pub(crate) struct QjsRevival {
    pub(crate) bytes: *const u8,
    pub(crate) length: usize,
    pub(crate) slots: usize,
    pub(crate) descriptors: *const QjsHostArg,
}

pub(crate) type HostDispatch = unsafe extern "C" fn(
    opaque: *mut c_void,
    context: *mut QjsContext,
    handler: *mut c_void,
    argument_count: usize,
    arguments: *const QjsHostArg,
    result: *mut QjsHostResult,
) -> c_int;

pub(crate) type HostRelease = unsafe extern "C" fn(opaque: *mut c_void, handler: *mut c_void);

pub(crate) type ModuleNormalize = unsafe extern "C" fn(
    opaque: *mut c_void,
    base: *const c_char,
    name: *const c_char,
    error: *mut c_int,
) -> *const c_char;

pub(crate) const REQUIRED_COMMONJS: i32 = 0;
pub(crate) const REQUIRED_JSON: i32 = 1;
pub(crate) const REQUIRED_MODULE: i32 = 2;
pub(crate) const REQUIRED_DETECT: i32 = 3;

#[repr(C)]
pub(crate) struct QjsRequiredSource {
    pub(crate) url: *const c_char,
    pub(crate) text: *const u8,
    pub(crate) text_length: usize,
    pub(crate) kind: i32,
    pub(crate) error: *const c_char,
    pub(crate) cached: *const u8,
    pub(crate) cached_length: usize,
    pub(crate) cached_kind: i32,
    pub(crate) capture: i32,
}

pub(crate) type RequireLoad = unsafe extern "C" fn(
    opaque: *mut c_void,
    url: *const c_char,
    parameters: *const c_char,
    source: *mut QjsRequiredSource,
) -> c_int;

pub(crate) const COMPILED_MODULE: i32 = 0;
pub(crate) const COMPILED_COMMONJS: i32 = 1;

#[repr(C)]
pub(crate) struct QjsCompiled {
    pub(crate) kind: i32,
    pub(crate) name: *const c_char,
    pub(crate) parameters: *const c_char,
    pub(crate) bytes: *const u8,
    pub(crate) length: usize,
    _next: *mut QjsCompiled,
}

pub(crate) type ShadowRealmInit = unsafe extern "C" fn(
    opaque: *mut c_void,
    child: *mut QjsContext,
    message: *mut *const c_char,
) -> c_int;

#[repr(C)]
pub(crate) struct QjsRuntime {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct QjsContext {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct QjsValue {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryUsage {
    pub malloc_size: i64,
    pub malloc_limit: i64,
    pub memory_used_size: i64,
    pub malloc_count: i64,
    pub memory_used_count: i64,
    pub atom_count: i64,
    pub atom_size: i64,
    pub str_count: i64,
    pub str_size: i64,
    pub obj_count: i64,
    pub obj_size: i64,
    pub prop_count: i64,
    pub prop_size: i64,
    pub shape_count: i64,
    pub shape_size: i64,
    pub js_func_count: i64,
    pub js_func_size: i64,
    pub js_func_code_size: i64,
    pub js_func_pc2line_count: i64,
    pub js_func_pc2line_size: i64,
    pub c_func_count: i64,
    pub array_count: i64,
    pub fast_array_count: i64,
    pub fast_array_elements: i64,
    pub binary_object_count: i64,
    pub binary_object_size: i64,
}

unsafe extern "C" {
    pub(crate) fn qjs_host_owner_class_id_new() -> u32;
    pub(crate) fn qjs_runtime_new(
        host_owner_class_id: u32,
        shadow_realm_class_id: u32,
        realm_link_class_id: u32,
    ) -> *mut QjsRuntime;
    pub(crate) fn qjs_runtime_free(runtime: *mut QjsRuntime);
    pub(crate) fn qjs_context_new(runtime: *mut QjsRuntime) -> *mut QjsContext;
    pub(crate) fn qjs_context_free(context: *mut QjsContext);
    pub(crate) fn qjs_context_enable_module_loading(
        context: *mut QjsContext,
        normalize: ModuleNormalize,
        opaque: *mut c_void,
    );
    pub(crate) fn qjs_context_install_shadow_realm(
        context: *mut QjsContext,
        init: ShadowRealmInit,
        opaque: *mut c_void,
    ) -> c_int;
    pub(crate) fn qjs_context_shadow_holders(context: *mut QjsContext) -> usize;
    pub(crate) fn qjs_context_take_module_request(context: *mut QjsContext) -> *const c_char;
    pub(crate) fn qjs_context_complete_module(
        context: *mut QjsContext,
        name: *const c_char,
        url: *const c_char,
        text: *const u8,
        length: usize,
        error: *const c_char,
    ) -> c_int;
    pub(crate) fn qjs_context_complete_module_code(
        context: *mut QjsContext,
        name: *const c_char,
        url: *const c_char,
        text: *const u8,
        length: usize,
        bytecode: *const u8,
        bytecode_length: usize,
        capture: c_int,
        strict: c_int,
    ) -> c_int;
    pub(crate) fn qjs_context_take_compiled(context: *mut QjsContext) -> *mut QjsCompiled;
    pub(crate) fn qjs_compiled_free(context: *mut QjsContext, compiled: *mut QjsCompiled);
    pub(crate) fn qjs_bytecode_format_version() -> c_int;
    pub(crate) fn qjs_context_resume_module_loads(context: *mut QjsContext);
    pub(crate) fn qjs_context_load_module(
        context: *mut QjsContext,
        name: *const c_char,
    ) -> *mut QjsValue;
    pub(crate) fn qjs_runtime_run_gc(runtime: *mut QjsRuntime);
    pub(crate) fn qjs_runtime_compute_memory_usage(
        runtime: *mut QjsRuntime,
        usage: *mut MemoryUsage,
    );
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
    pub(crate) fn qjs_runtime_add_module_bytecode(
        runtime: *mut QjsRuntime,
        name: *const c_char,
        bytecode: *const u8,
        length: usize,
    ) -> c_int;
    pub(crate) fn qjs_runtime_set_strip_info(runtime: *mut QjsRuntime, flags: c_int);
    pub(crate) fn qjs_context_compile_module_bytecode(
        context: *mut QjsContext,
        source: *const u8,
        source_length: usize,
        name: *const c_char,
        out_length: *mut usize,
    ) -> *mut u8;
    pub(crate) fn qjs_bytecode_free(context: *mut QjsContext, bytes: *mut u8);
    pub(crate) fn qjs_runtime_reserve_module_prefix(
        runtime: *mut QjsRuntime,
        prefix: *const c_char,
    ) -> c_int;
    pub(crate) fn qjs_context_add_host_module_export(
        context: *mut QjsContext,
        name: *const c_char,
        export_name: *const c_char,
        value: *const QjsValue,
    ) -> c_int;
    pub(crate) fn qjs_context_add_host_module_shared_object(
        context: *mut QjsContext,
        name: *const c_char,
        export_name: *const c_char,
    ) -> c_int;
    pub(crate) fn qjs_context_add_host_module_transfer_member(
        context: *mut QjsContext,
        name: *const c_char,
        export_name: *const c_char,
        member: c_int,
    ) -> c_int;
    pub(crate) fn qjs_context_register_synchronous_loader(
        context: *mut QjsContext,
        name: *const c_char,
        export_name: *const c_char,
        load: RequireLoad,
        opaque: *mut c_void,
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
        realm: *mut QjsContext,
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
        transfer_at: c_int,
    ) -> *mut QjsValue;
}

#[cfg(feature = "devtools")]
pub(crate) use devtools::*;

#[cfg(feature = "devtools")]
mod devtools {
    use std::ffi::{c_char, c_int, c_void};

    use super::{QjsContext, QjsValue};

    pub(crate) const HOST_ARG_VALUE: i32 = 12;
    pub(crate) const HOST_VALUES: c_int = -2;

    pub(crate) const REFLECT_READ: c_int = 0;
    pub(crate) const REFLECT_PROXY: c_int = 1;
    pub(crate) const REFLECT_ABSENT: c_int = 2;

    pub(crate) const CLASS_PRIMITIVE: c_int = 0;
    pub(crate) const CLASS_PROXY: c_int = 1;
    pub(crate) const CLASS_ARRAY: c_int = 2;
    pub(crate) const CLASS_ERROR: c_int = 3;
    pub(crate) const CLASS_PROMISE: c_int = 4;
    pub(crate) const CLASS_FUNCTION: c_int = 5;
    pub(crate) const CLASS_OBJECT: c_int = 6;
    pub(crate) const CLASS_TYPED_ARRAY: c_int = 7;
    pub(crate) const CLASS_MAP: c_int = 8;
    pub(crate) const CLASS_SET: c_int = 9;
    pub(crate) const CLASS_DATE: c_int = 10;
    pub(crate) const CLASS_REGEXP: c_int = 11;

    pub(crate) const PROPERTY_CONFIGURABLE: i32 = 1;
    pub(crate) const PROPERTY_WRITABLE: i32 = 2;
    pub(crate) const PROPERTY_ENUMERABLE: i32 = 4;
    pub(crate) const PROPERTY_GETSET: i32 = 16;

    #[repr(C)]
    pub(crate) struct QjsOwnProperty {
        pub(crate) name: *mut QjsValue,
        pub(crate) symbol: *mut QjsValue,
        pub(crate) value: *mut QjsValue,
        pub(crate) getter: *mut QjsValue,
        pub(crate) setter: *mut QjsValue,
        pub(crate) flags: i32,
    }

    pub(crate) type DescribeWrite =
        unsafe extern "C" fn(opaque: *mut c_void, bytes: *const c_char, length: usize);

    unsafe extern "C" {
        pub(crate) fn qjs_devtools_classify(
            context: *mut QjsContext,
            value: *const QjsValue,
            length: *mut usize,
        ) -> c_int;
        pub(crate) fn qjs_devtools_own_properties(
            context: *mut QjsContext,
            object: *const QjsValue,
            non_indexed: c_int,
            properties: *mut *mut QjsOwnProperty,
            count: *mut usize,
        ) -> c_int;
        pub(crate) fn qjs_devtools_own_properties_free(properties: *mut QjsOwnProperty);
        pub(crate) fn qjs_devtools_own_property(
            context: *mut QjsContext,
            object: *const QjsValue,
            name: *const c_char,
            length: usize,
            property: *mut QjsOwnProperty,
        ) -> c_int;
        pub(crate) fn qjs_devtools_prototype(
            context: *mut QjsContext,
            object: *const QjsValue,
            prototype: *mut *mut QjsValue,
        ) -> c_int;
        pub(crate) fn qjs_devtools_describe(
            context: *mut QjsContext,
            value: *const QjsValue,
            depth: u32,
            string_length: u32,
            item_count: u32,
            write: DescribeWrite,
            opaque: *mut c_void,
        );
        pub(crate) fn qjs_devtools_json_stringify(
            context: *mut QjsContext,
            value: *const QjsValue,
        ) -> *mut QjsValue;
        pub(crate) fn qjs_devtools_json_parse(
            context: *mut QjsContext,
            text: *const c_char,
            length: usize,
        ) -> *mut QjsValue;
    }
}
