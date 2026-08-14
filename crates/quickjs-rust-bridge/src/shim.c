/* QuickJS C ABI shim used by the Rust bridge. */

#include "quickjs.h"

#include <assert.h>
#include <limits.h>
#include <stdint.h>
#include <stdlib.h>

_Static_assert(JS_EVAL_TYPE_GLOBAL == 0,
               "Rust JS_EVAL_TYPE_GLOBAL must match quickjs.h");
_Static_assert(JS_EVAL_TYPE_MODULE == 1,
               "Rust JS_EVAL_TYPE_MODULE must match quickjs.h");
_Static_assert(JS_EVAL_FLAG_STRICT == (1 << 3),
               "Rust JS_EVAL_FLAG_STRICT must match quickjs.h");
_Static_assert(JS_EVAL_FLAG_BACKTRACE_BARRIER == (1 << 6),
               "Rust JS_EVAL_FLAG_BACKTRACE_BARRIER must match quickjs.h");
_Static_assert(JS_EVAL_FLAG_ASYNC == (1 << 7),
               "Rust JS_EVAL_FLAG_ASYNC must match quickjs.h");

typedef struct QjsValue {
    JSValue value;
} QjsValue;

typedef struct QjsUnhandledRejection {
    JSValue promise;
    JSValue reason;
    struct QjsUnhandledRejection *next;
} QjsUnhandledRejection;

typedef int QjsInterruptCallback(void *opaque);


enum QjsHostArgKind {
    QJS_ARG_UNDEFINED = 0,
    QJS_ARG_NULL = 1,
    QJS_ARG_BOOLEAN = 2,
    QJS_ARG_NUMBER = 3,
    QJS_ARG_STRING = 4,
    QJS_ARG_UNSUPPORTED = 5,
};


typedef struct QjsHostArg {
    int32_t kind;
    double number;
    const uint8_t *text;
    size_t text_len;
} QjsHostArg;


typedef struct QjsHostResult {
    int32_t kind;
    double number;
    const uint16_t *text;
    size_t text_len;
} QjsHostResult;


typedef int QjsHostDispatch(void *opaque, void *handler, size_t argument_count,
                            const QjsHostArg *arguments, QjsHostResult *result);


typedef void QjsHostRelease(void *opaque, void *handler);


typedef struct QjsRuntime {
    JSRuntime *raw;
    JSContext *context;
    QjsUnhandledRejection *rejection_head;
    QjsUnhandledRejection *rejection_tail;
    int rejection_tracker_oom;
    QjsInterruptCallback *interrupt_callback;
    void *interrupt_opaque;
    QjsHostDispatch *host_dispatch;
    QjsHostRelease *host_release;
    void *host_opaque;
    JSClassID host_owner_class_id;
} QjsRuntime;


typedef struct QjsHostOwner {
    QjsRuntime *runtime;
    void *handler;
} QjsHostOwner;


static void qjs_host_owner_finalizer(JSRuntime *raw, JSValue value) {
    QjsHostOwner *owner = JS_GetOpaque(value, JS_GetClassID(value));

    (void)raw;
    if (owner == NULL) {
        return;
    }
    if (owner->handler != NULL && owner->runtime->host_release != NULL) {
        owner->runtime->host_release(owner->runtime->host_opaque,
                                     owner->handler);
    }
    free(owner);
}

static const JSClassDef qjs_host_owner_class = {
    "QjsHostFunctionOwner",
    .finalizer = qjs_host_owner_finalizer,
};


enum QjsValueKind {
    QJS_KIND_UNDEFINED = 0,
    QJS_KIND_NULL = 1,
    QJS_KIND_BOOLEAN = 2,
    QJS_KIND_NUMBER = 3,
    QJS_KIND_BIG_INT = 4,
    QJS_KIND_STRING = 5,
    QJS_KIND_SYMBOL = 6,
    QJS_KIND_FUNCTION = 7,
    QJS_KIND_OBJECT = 8,
    QJS_KIND_OTHER = 9,
};

enum QjsEvalFailureStage {
    QJS_EVAL_FAILURE_NONE = 0,
    QJS_EVAL_FAILURE_COMPILE = 1,
    QJS_EVAL_FAILURE_EXECUTE = 2,
};

_Static_assert(QJS_EVAL_FAILURE_COMPILE == 1,
               "Rust QJS_EVAL_FAILURE_COMPILE must match shim.c");

static QjsValue *qjs_box(JSContext *ctx, JSValue value) {
    QjsValue *boxed;

    if (JS_IsException(value)) {
        return NULL;
    }
    boxed = malloc(sizeof(*boxed));
    if (boxed == NULL) {
        JS_FreeValue(ctx, value);
        JS_ThrowOutOfMemory(ctx);
        return NULL;
    }
    boxed->value = value;
    return boxed;
}

static void qjs_promise_rejection_tracker(JSContext *context,
                                          JSValueConst promise,
                                          JSValueConst reason,
                                          JS_BOOL is_handled, void *opaque) {
    QjsRuntime *runtime = opaque;
    QjsUnhandledRejection *current;
    QjsUnhandledRejection *previous = NULL;

    if (is_handled) {
        current = runtime->rejection_head;
        while (current != NULL) {
            if (JS_StrictEq(context, current->promise, promise)) {
                if (previous == NULL) {
                    runtime->rejection_head = current->next;
                } else {
                    previous->next = current->next;
                }
                if (runtime->rejection_tail == current) {
                    runtime->rejection_tail = previous;
                }
                JS_FreeValue(context, current->promise);
                JS_FreeValue(context, current->reason);
                free(current);
                return;
            }
            previous = current;
            current = current->next;
        }
        return;
    }

    current = malloc(sizeof(*current));
    if (current == NULL) {
        runtime->rejection_tracker_oom = 1;
        return;
    }
    current->promise = JS_DupValue(context, promise);
    current->reason = JS_DupValue(context, reason);
    current->next = NULL;
    if (runtime->rejection_tail == NULL) {
        runtime->rejection_head = current;
    } else {
        runtime->rejection_tail->next = current;
    }
    runtime->rejection_tail = current;
}

static int qjs_interrupt_trampoline(JSRuntime *raw, void *opaque) {
    QjsRuntime *runtime = opaque;

    (void)raw;
    if (runtime->interrupt_callback == NULL) {
        return 0;
    }
    return runtime->interrupt_callback(runtime->interrupt_opaque);
}

QjsRuntime *qjs_runtime_new(void) {
    QjsRuntime *runtime = calloc(1, sizeof(*runtime));
    if (runtime == NULL) {
        return NULL;
    }
    runtime->raw = JS_NewRuntime();
    if (runtime->raw != NULL) {
        JS_SetCanBlock(runtime->raw, 0);
        JS_SetHostPromiseRejectionTracker(runtime->raw,
                                          qjs_promise_rejection_tracker,
                                          runtime);
        JS_NewClassID(&runtime->host_owner_class_id);
        JS_NewClass(runtime->raw, runtime->host_owner_class_id,
                    &qjs_host_owner_class);
    } else {
        free(runtime);
        return NULL;
    }
    return runtime;
}

void qjs_runtime_free(QjsRuntime *runtime) {
    QjsUnhandledRejection *current = runtime->rejection_head;

    JS_SetInterruptHandler(runtime->raw, NULL, NULL);
    runtime->interrupt_callback = NULL;
    runtime->interrupt_opaque = NULL;
    JS_SetHostPromiseRejectionTracker(runtime->raw, NULL, NULL);
    while (current != NULL) {
        QjsUnhandledRejection *next = current->next;
        JS_FreeValueRT(runtime->raw, current->promise);
        JS_FreeValueRT(runtime->raw, current->reason);
        free(current);
        current = next;
    }
    JS_FreeRuntime(runtime->raw);
    free(runtime);
}

JSContext *qjs_context_new(QjsRuntime *runtime) {
    assert(runtime->context == NULL);
    runtime->context = JS_NewContext(runtime->raw);
    if (runtime->context != NULL) {

        JS_SetContextOpaque(runtime->context, runtime);
    }
    return runtime->context;
}

void qjs_context_free(JSContext *context) {
    JS_FreeContext(context);
}

void qjs_runtime_run_gc(QjsRuntime *runtime) {
    JS_RunGC(runtime->raw);
}

void qjs_runtime_set_memory_limit(QjsRuntime *runtime, size_t limit) {
    JS_SetMemoryLimit(runtime->raw, limit);
}

void qjs_runtime_set_max_stack_size(QjsRuntime *runtime, size_t size) {
    JS_SetMaxStackSize(runtime->raw, size);
}

void qjs_runtime_set_interrupt_handler(QjsRuntime *runtime,
                                       QjsInterruptCallback *callback,
                                       void *opaque) {
    runtime->interrupt_callback = callback;
    runtime->interrupt_opaque = opaque;
    JS_SetInterruptHandler(runtime->raw,
                           callback == NULL ? NULL : qjs_interrupt_trampoline,
                           callback == NULL ? NULL : runtime);
}

QjsValue *qjs_new_undefined(JSContext *context) {
    return qjs_box(context, JS_UNDEFINED);
}

QjsValue *qjs_new_null(JSContext *context) {
    return qjs_box(context, JS_NULL);
}

QjsValue *qjs_new_boolean(JSContext *context, int value) {
    return qjs_box(context, JS_NewBool(context, value != 0));
}

QjsValue *qjs_new_number(JSContext *context, double value) {
    return qjs_box(context, JS_NewFloat64(context, value));
}

QjsValue *qjs_new_big_int64(JSContext *context, int64_t value) {
    return qjs_box(context, JS_NewBigInt64(context, value));
}

QjsValue *qjs_new_big_uint64(JSContext *context, uint64_t value) {
    return qjs_box(context, JS_NewBigUint64(context, value));
}


static JSValue qjs_string_from_utf16(JSContext *context, const uint16_t *units,
                                     size_t length) {
    static const char hex[] = "0123456789abcdef";
    char *json;
    JSValue parsed;
    size_t index;
    size_t offset = 0;

    if (length > (SIZE_MAX - 3) / 6) {
        return JS_ThrowOutOfMemory(context);
    }
    json = malloc(length * 6 + 3);
    if (json == NULL) {
        return JS_ThrowOutOfMemory(context);
    }
    json[offset++] = '"';
    for (index = 0; index < length; ++index) {
        uint16_t unit = units[index];
        json[offset++] = '\\';
        json[offset++] = 'u';
        json[offset++] = hex[(unit >> 12) & 0x0f];
        json[offset++] = hex[(unit >> 8) & 0x0f];
        json[offset++] = hex[(unit >> 4) & 0x0f];
        json[offset++] = hex[unit & 0x0f];
    }
    json[offset++] = '"';
    json[offset] = '\0';
    parsed = JS_ParseJSON(context, json, offset, "<host string>");
    free(json);
    return parsed;
}

QjsValue *qjs_new_string_utf16(JSContext *context, const uint16_t *units,
                               size_t length) {
    return qjs_box(context, qjs_string_from_utf16(context, units, length));
}

void qjs_value_free(JSContext *context, QjsValue *value) {
    if (value != NULL) {
        JS_FreeValue(context, value->value);
        free(value);
    }
}

int qjs_value_kind(JSContext *context, const QjsValue *value) {
    JSValueConst raw = value->value;

    if (JS_IsUndefined(raw)) return QJS_KIND_UNDEFINED;
    if (JS_IsNull(raw)) return QJS_KIND_NULL;
    if (JS_IsBool(raw)) return QJS_KIND_BOOLEAN;
    if (JS_IsNumber(raw)) return QJS_KIND_NUMBER;
    if (JS_IsBigInt(context, raw)) return QJS_KIND_BIG_INT;
    if (JS_IsString(raw)) return QJS_KIND_STRING;
    if (JS_IsSymbol(raw)) return QJS_KIND_SYMBOL;
    if (JS_IsFunction(context, raw)) return QJS_KIND_FUNCTION;
    if (JS_IsObject(raw)) return QJS_KIND_OBJECT;
    return QJS_KIND_OTHER;
}

int qjs_value_get_boolean(JSContext *context, const QjsValue *value, int *result) {
    int converted = JS_ToBool(context, value->value);
    if (converted < 0) return -1;
    *result = converted;
    return 0;
}

int qjs_value_get_number(JSContext *context, const QjsValue *value, double *result) {
    return JS_ToFloat64(context, result, value->value);
}

int qjs_value_to_cesu8(JSContext *context, const QjsValue *value,
                       const uint8_t **bytes, size_t *length) {
    const char *converted = JS_ToCStringLen2(context, length, value->value, 1);
    if (converted == NULL) return -1;
    *bytes = (const uint8_t *)converted;
    return 0;
}

void qjs_cesu8_free(JSContext *context, const uint8_t *bytes) {
    JS_FreeCString(context, (const char *)bytes);
}

QjsValue *qjs_eval(JSContext *context, const uint8_t *source, size_t source_length,
                   const char *source_name, int flags, int *failure_stage) {
    JSValue compiled;
    JSValue result;

    *failure_stage = QJS_EVAL_FAILURE_NONE;
    compiled = JS_Eval(context, (const char *)source, source_length,
                       source_name, flags | JS_EVAL_FLAG_COMPILE_ONLY);
    if (JS_IsException(compiled)) {
        *failure_stage = QJS_EVAL_FAILURE_COMPILE;
        return NULL;
    }
    result = JS_EvalFunction(context, compiled);
    if (JS_IsException(result)) {
        *failure_stage = QJS_EVAL_FAILURE_EXECUTE;
    }
    return qjs_box(context, result);
}

QjsValue *qjs_call(JSContext *context, const QjsValue *callable,
                   const QjsValue *this_value, size_t argument_count,
                   const QjsValue *const *arguments) {
    JSValue *raw_arguments = NULL;
    JSValue result;
    size_t index;

    if (argument_count > INT_MAX ||
        argument_count > SIZE_MAX / sizeof(*raw_arguments)) {
        JS_ThrowRangeError(context, "too many call arguments");
        return NULL;
    }
    if (argument_count > 0) {
        raw_arguments = malloc(argument_count * sizeof(*raw_arguments));
        if (raw_arguments == NULL) {
            JS_ThrowOutOfMemory(context);
            return NULL;
        }
        for (index = 0; index < argument_count; ++index) {
            raw_arguments[index] = arguments[index]->value;
        }
    }

    result = JS_Call(context, callable->value,
                     this_value == NULL ? JS_UNDEFINED : this_value->value,
                     (int)argument_count, raw_arguments);
    free(raw_arguments);
    return qjs_box(context, result);
}

int qjs_execute_pending_job(QjsRuntime *runtime, JSContext **context) {
    return JS_ExecutePendingJob(runtime->raw, context);
}

int qjs_has_pending_job(QjsRuntime *runtime) {
    return JS_IsJobPending(runtime->raw);
}

int qjs_has_unhandled_rejection(QjsRuntime *runtime) {
    return runtime->rejection_head != NULL || runtime->rejection_tracker_oom;
}

QjsValue *qjs_take_unhandled_rejection(QjsRuntime *runtime) {
    QjsUnhandledRejection *rejection = runtime->rejection_head;
    JSValue reason;

    if (rejection == NULL) {
        if (!runtime->rejection_tracker_oom || runtime->context == NULL) {
            return NULL;
        }
        runtime->rejection_tracker_oom = 0;
        JS_ThrowOutOfMemory(runtime->context);
        return qjs_box(runtime->context, JS_GetException(runtime->context));
    }
    runtime->rejection_head = rejection->next;
    if (runtime->rejection_head == NULL) {
        runtime->rejection_tail = NULL;
    }
    JS_FreeValue(runtime->context, rejection->promise);
    reason = rejection->reason;
    free(rejection);
    return qjs_box(runtime->context, reason);
}

QjsValue *qjs_take_exception(JSContext *context) {
    JSValue exception = JS_GetException(context);
    QjsValue *boxed = malloc(sizeof(*boxed));

    if (boxed == NULL) {
        JS_FreeValue(context, exception);
        return NULL;
    }
    boxed->value = exception;
    return boxed;
}

void qjs_discard_exception(JSContext *context) {
    JSValue exception = JS_GetException(context);
    JS_FreeValue(context, exception);
}

QjsValue *qjs_get_property(JSContext *context, const QjsValue *value,
                           const char *name) {
    return qjs_box(context, JS_GetPropertyStr(context, value->value, name));
}

int qjs_set_property(JSContext *context, const QjsValue *target,
                     const char *name, const QjsValue *value) {

    return JS_SetPropertyStr(context, target->value, name,
                             JS_DupValue(context, value->value));
}

QjsValue *qjs_global_object(JSContext *context) {
    return qjs_box(context, JS_GetGlobalObject(context));
}

void qjs_throw_error(JSContext *context, const char *message) {
    JS_ThrowInternalError(context, "%s", message);
}

void qjs_runtime_set_host_dispatch(QjsRuntime *runtime,
                                   QjsHostDispatch *dispatch,
                                   QjsHostRelease *release, void *opaque) {
    runtime->host_dispatch = dispatch;
    runtime->host_release = release;
    runtime->host_opaque = opaque;
}

#define QJS_HOST_INLINE_ARGS 8

static void qjs_host_describe(JSContext *context, JSValueConst value,
                              QjsHostArg *slot) {
    slot->text = NULL;
    slot->text_len = 0;
    slot->number = 0.0;

    if (JS_IsUndefined(value)) {
        slot->kind = QJS_ARG_UNDEFINED;
    } else if (JS_IsNull(value)) {
        slot->kind = QJS_ARG_NULL;
    } else if (JS_IsBool(value)) {
        slot->kind = QJS_ARG_BOOLEAN;
        slot->number = JS_ToBool(context, value) ? 1.0 : 0.0;
    } else if (JS_IsNumber(value)) {
        double number;
        if (JS_ToFloat64(context, &number, value) < 0) {
            slot->kind = QJS_ARG_UNSUPPORTED;
            return;
        }
        slot->kind = QJS_ARG_NUMBER;
        slot->number = number;
    } else if (JS_IsString(value)) {
        size_t length;
        const char *text = JS_ToCStringLen2(context, &length, value, 1);
        if (text == NULL) {
            slot->kind = QJS_ARG_UNSUPPORTED;
            return;
        }
        slot->kind = QJS_ARG_STRING;
        slot->text = (const uint8_t *)text;
        slot->text_len = length;
    } else {
        slot->kind = QJS_ARG_UNSUPPORTED;
    }
}

static JSValue qjs_host_build(JSContext *context, const QjsHostResult *result) {
    switch (result->kind) {
    case QJS_ARG_UNDEFINED:
        return JS_UNDEFINED;
    case QJS_ARG_NULL:
        return JS_NULL;
    case QJS_ARG_BOOLEAN:
        return JS_NewBool(context, result->number != 0.0);
    case QJS_ARG_NUMBER:
        return JS_NewFloat64(context, result->number);
    case QJS_ARG_STRING:
        return qjs_string_from_utf16(context, result->text, result->text_len);
    default:
        return JS_ThrowInternalError(context, "invalid host return value");
    }
}

static JSValue qjs_host_trampoline(JSContext *context, JSValueConst this_value,
                                   int argc, JSValueConst *argv, int magic,
                                   JSValue *func_data) {
    QjsRuntime *runtime = JS_GetContextOpaque(context);
    QjsHostArg inline_arguments[QJS_HOST_INLINE_ARGS];
    QjsHostArg *arguments = inline_arguments;
    QjsHostOwner *owner;
    QjsHostResult result;
    JSValue returned;
    void *handler;
    int status;
    int count;

    (void)this_value;
    (void)magic;
    if (runtime == NULL || runtime->host_dispatch == NULL) {
        return JS_ThrowInternalError(context, "no host dispatch is installed");
    }
    owner = JS_GetOpaque(func_data[0], runtime->host_owner_class_id);
    if (owner == NULL || owner->handler == NULL) {
        return JS_ThrowInternalError(context, "this host function was released");
    }
    handler = owner->handler;

    if (argc > QJS_HOST_INLINE_ARGS) {
        if ((size_t)argc > SIZE_MAX / sizeof(*arguments)) {
            return JS_ThrowRangeError(context, "too many call arguments");
        }
        arguments = malloc((size_t)argc * sizeof(*arguments));
        if (arguments == NULL) {
            JS_ThrowOutOfMemory(context);
            return JS_EXCEPTION;
        }
    }
    for (count = 0; count < argc; ++count) {
        qjs_host_describe(context, argv[count], &arguments[count]);
    }

    result.kind = QJS_ARG_UNDEFINED;
    result.number = 0.0;
    result.text = NULL;
    result.text_len = 0;
    status = runtime->host_dispatch(runtime->host_opaque, handler,
                                    (size_t)argc, arguments, &result);

    for (count = 0; count < argc; ++count) {
        if (arguments[count].kind == QJS_ARG_STRING) {
            JS_FreeCString(context, (const char *)arguments[count].text);
        }
    }
    if (arguments != inline_arguments) {
        free(arguments);
    }

    if (status != 0) {

        return JS_EXCEPTION;
    }
    returned = qjs_host_build(context, &result);
    return returned;
}


QjsValue *qjs_new_host_function(JSContext *context, const char *name,
                                int length, void *handler) {
    QjsRuntime *runtime = JS_GetContextOpaque(context);
    QjsHostOwner *owner;
    JSValue data;
    JSValue function;
    JSValue function_name;

    if (runtime == NULL) {
        JS_ThrowInternalError(context, "no host dispatch is installed");
        return NULL;
    }


    owner = malloc(sizeof(*owner));
    if (owner == NULL) {
        JS_ThrowOutOfMemory(context);
        return NULL;
    }
    owner->runtime = runtime;
    owner->handler = handler;

    data = JS_NewObjectClass(context, (int)runtime->host_owner_class_id);
    if (JS_IsException(data)) {
        free(owner);
        return NULL;
    }
    JS_SetOpaque(data, owner);

    function = JS_NewCFunctionData(context, qjs_host_trampoline, length,
                                   0, 1, &data);
    if (JS_IsException(function)) {
        owner->handler = NULL;
        JS_FreeValue(context, data);
        return NULL;
    }
    JS_FreeValue(context, data);


    function_name = JS_NewString(context, name);
    if (JS_IsException(function_name)) {
        owner->handler = NULL;
        JS_FreeValue(context, function);
        return NULL;
    }
    if (JS_DefinePropertyValueStr(context, function, "name", function_name,
                                  JS_PROP_CONFIGURABLE) < 0) {
        owner->handler = NULL;
        JS_FreeValue(context, function);
        return NULL;
    }
    return qjs_box(context, function);
}
