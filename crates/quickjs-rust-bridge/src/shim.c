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
_Static_assert(JS_PROMISE_PENDING == 0,
               "Rust QJS_PROMISE_PENDING must match quickjs.h");
_Static_assert(JS_PROMISE_FULFILLED == 1,
               "Rust QJS_PROMISE_FULFILLED must match quickjs.h");
_Static_assert(JS_PROMISE_REJECTED == 2,
               "Rust QJS_PROMISE_REJECTED must match quickjs.h");

typedef struct QjsValue {
    JSValue value;
} QjsValue;

typedef struct QjsUnhandledRejection {
    JSValue promise;
    JSValue reason;
    struct QjsUnhandledRejection *next;
} QjsUnhandledRejection;

typedef int QjsInterruptCallback(void *opaque);

typedef struct QjsModuleSource {
    char *name;
    uint8_t *source;
    size_t source_length;
    JSModuleDef *definition;
    struct QjsModuleSource *next;
} QjsModuleSource;

typedef struct QjsHostModuleExport {
    char *name;
    JSValue value;
    struct QjsHostModuleExport *next;
} QjsHostModuleExport;

typedef struct QjsHostModule {
    char *name;
    JSModuleDef *definition;
    QjsHostModuleExport *exports;
    struct QjsHostModule *next;
} QjsHostModule;

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


/* The result of a host call travels in the same struct an argument does:
   both are the primitives-only boundary vocabulary, and both carry text as
   UTF-8 bytes. */
typedef QjsHostArg QjsHostResult;


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
    QjsModuleSource *module_sources;
    QjsHostModule *host_modules;
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

static QjsModuleSource *qjs_find_module_source(QjsRuntime *runtime,
                                               const char *name) {
    QjsModuleSource *module = runtime->module_sources;

    while (module != NULL && strcmp(module->name, name) != 0) {
        module = module->next;
    }
    return module;
}

static QjsHostModule *qjs_find_host_module(QjsRuntime *runtime,
                                           const char *name) {
    QjsHostModule *module = runtime->host_modules;

    while (module != NULL && strcmp(module->name, name) != 0) {
        module = module->next;
    }
    return module;
}

static int qjs_host_module_init(JSContext *context, JSModuleDef *definition) {
    QjsRuntime *runtime = JS_GetContextOpaque(context);
    QjsHostModule *module;
    QjsHostModuleExport *exported;

    if (runtime == NULL) {
        return -1;
    }
    module = runtime->host_modules;
    while (module != NULL && module->definition != definition) {
        module = module->next;
    }
    if (module == NULL) {
        JS_ThrowInternalError(context,
                              "native host module is not registered");
        return -1;
    }
    for (exported = module->exports; exported != NULL;
         exported = exported->next) {
        if (JS_SetModuleExport(context, definition, exported->name,
                               JS_DupValue(context, exported->value)) < 0) {
            return -1;
        }
    }
    return 0;
}

static JSModuleDef *qjs_module_loader(JSContext *context,
                                      const char *module_name, void *opaque) {
    QjsRuntime *runtime = opaque;
    QjsModuleSource *module = qjs_find_module_source(runtime, module_name);
    QjsHostModule *host_module;
    QjsHostModuleExport *exported;
    JSValue compiled;
    JSModuleDef *definition;

    if (module != NULL) {
        if (module->definition != NULL) {
            return module->definition;
        }
        compiled = JS_Eval(context, (const char *)module->source,
                           module->source_length, module->name,
                           JS_EVAL_TYPE_MODULE | JS_EVAL_FLAG_COMPILE_ONLY);
        if (JS_IsException(compiled)) {
            return NULL;
        }
        definition = JS_VALUE_GET_PTR(compiled);
        module->definition = definition;
        JS_FreeValue(context, compiled);
        return definition;
    }

    host_module = qjs_find_host_module(runtime, module_name);
    if (host_module == NULL) {
        JS_ThrowReferenceError(context, "module '%s' is not preloaded",
                               module_name);
        return NULL;
    }
    if (host_module->definition != NULL) {
        return host_module->definition;
    }
    definition = JS_NewCModule(context, module_name, qjs_host_module_init);
    if (definition == NULL) {
        return NULL;
    }
    host_module->definition = definition;
    for (exported = host_module->exports; exported != NULL;
         exported = exported->next) {
        if (JS_AddModuleExport(context, definition, exported->name) < 0) {
            return NULL;
        }
    }
    return definition;
}

static void qjs_module_sources_free(QjsModuleSource *module) {
    while (module != NULL) {
        QjsModuleSource *next = module->next;
        free(module->name);
        free(module->source);
        free(module);
        module = next;
    }
}

static void qjs_host_modules_free(JSRuntime *runtime,
                                  QjsHostModule *module) {
    while (module != NULL) {
        QjsHostModule *next = module->next;
        QjsHostModuleExport *exported = module->exports;
        while (exported != NULL) {
            QjsHostModuleExport *next_export = exported->next;
            JS_FreeValueRT(runtime, exported->value);
            free(exported->name);
            free(exported);
            exported = next_export;
        }
        free(module->name);
        free(module);
        module = next;
    }
}

JSClassID qjs_host_owner_class_id_new(void) {
    JSClassID class_id = 0;

    return JS_NewClassID(&class_id);
}

QjsRuntime *qjs_runtime_new(JSClassID host_owner_class_id) {
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
        JS_SetModuleLoaderFunc(runtime->raw, NULL, qjs_module_loader, runtime);
        runtime->host_owner_class_id = host_owner_class_id;
        if (JS_NewClass(runtime->raw, runtime->host_owner_class_id,
                        &qjs_host_owner_class) < 0) {
            JS_FreeRuntime(runtime->raw);
            free(runtime);
            return NULL;
        }
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
    qjs_host_modules_free(runtime->raw, runtime->host_modules);
    JS_FreeRuntime(runtime->raw);
    qjs_module_sources_free(runtime->module_sources);
    free(runtime);
}

int qjs_runtime_add_module(QjsRuntime *runtime, const char *name,
                           const uint8_t *source, size_t source_length) {
    QjsModuleSource *module;
    size_t name_length;

    if (qjs_find_module_source(runtime, name) != NULL ||
        qjs_find_host_module(runtime, name) != NULL) {
        return -2;
    }
    if (source_length == SIZE_MAX) {
        return -1;
    }
    name_length = strlen(name);
    if (name_length == SIZE_MAX) {
        return -1;
    }

    module = calloc(1, sizeof(*module));
    if (module == NULL) {
        return -1;
    }
    module->name = malloc(name_length + 1);
    module->source = malloc(source_length + 1);
    if (module->name == NULL || module->source == NULL) {
        free(module->name);
        free(module->source);
        free(module);
        return -1;
    }
    memcpy(module->name, name, name_length + 1);
    memcpy(module->source, source, source_length);
    module->source[source_length] = '\0';
    module->source_length = source_length;
    module->next = runtime->module_sources;
    runtime->module_sources = module;
    return 0;
}

int qjs_runtime_add_host_module_export(QjsRuntime *runtime, const char *name,
                                       const char *export_name,
                                       const QjsValue *value) {
    QjsHostModule *module;
    QjsHostModuleExport *current;
    QjsHostModuleExport *exported;
    size_t export_name_length;
    int new_module = 0;

    if (qjs_find_module_source(runtime, name) != NULL) {
        return -2;
    }
    module = qjs_find_host_module(runtime, name);
    if (module != NULL && module->definition != NULL) {
        return -4;
    }
    if (module != NULL) {
        for (current = module->exports; current != NULL;
             current = current->next) {
            if (strcmp(current->name, export_name) == 0) {
                return -3;
            }
        }
    } else {
        size_t name_length = strlen(name);
        module = calloc(1, sizeof(*module));
        if (module == NULL) {
            return -1;
        }
        module->name = malloc(name_length + 1);
        if (module->name == NULL) {
            free(module);
            return -1;
        }
        memcpy(module->name, name, name_length + 1);
        new_module = 1;
    }

    export_name_length = strlen(export_name);
    exported = calloc(1, sizeof(*exported));
    if (exported == NULL) {
        if (new_module) {
            free(module->name);
            free(module);
        }
        return -1;
    }
    exported->name = malloc(export_name_length + 1);
    if (exported->name == NULL) {
        free(exported);
        if (new_module) {
            free(module->name);
            free(module);
        }
        return -1;
    }
    memcpy(exported->name, export_name, export_name_length + 1);
    exported->value = JS_DupValue(runtime->context, value->value);
    exported->next = module->exports;
    module->exports = exported;
    if (new_module) {
        module->next = runtime->host_modules;
        runtime->host_modules = module;
    }
    return 0;
}

QjsValue *qjs_module_namespace(JSContext *context, const char *name) {
    QjsRuntime *runtime = JS_GetContextOpaque(context);
    QjsModuleSource *source;
    QjsHostModule *host;
    JSModuleDef *definition = NULL;

    if (runtime != NULL) {
        source = qjs_find_module_source(runtime, name);
        host = qjs_find_host_module(runtime, name);
        if (source != NULL) {
            definition = source->definition;
        } else if (host != NULL) {
            definition = host->definition;
        }
    }
    if (definition == NULL) {
        JS_ThrowReferenceError(context, "module '%s' has not been loaded",
                               name);
        return NULL;
    }
    return qjs_box(context, JS_GetModuleNamespace(context, definition));
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


/* The escape hatch for ill-formed UTF-16: text containing an unpaired
   surrogate has no UTF-8 spelling, and `JS_NewStringLen` would replace the
   surrogate with U+FFFD. `JS_ParseJSON` is the one entry point that accepts a
   lone `\uD800` escape and preserves it, at the cost of four passes and three
   allocations. Well-formed text never reaches here: it goes straight through
   `qjs_new_string_utf8`. */
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

/* Well-formed UTF-8 straight into QuickJS's own decoder, which has an ASCII
   fast path that memcpy's into a Latin-1 string. Every Rust `str` qualifies,
   so this is the only construction path host text normally takes. */
QjsValue *qjs_new_string_utf8(JSContext *context, const uint8_t *bytes,
                              size_t length) {
    return qjs_box(context,
                   JS_NewStringLen(context, (const char *)bytes, length));
}

uint32_t qjs_atom_new(JSContext *context, const uint8_t *bytes, size_t length) {
    return (uint32_t)JS_NewAtomLen(context, (const char *)bytes, length);
}

void qjs_atom_free(JSContext *context, uint32_t atom) {
    JS_FreeAtom(context, (JSAtom)atom);
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

int qjs_value_promise_state(JSContext *context, const QjsValue *value) {
    return JS_PromiseState(context, value->value);
}

QjsValue *qjs_value_promise_result(JSContext *context,
                                   const QjsValue *value) {
    return qjs_box(context, JS_PromiseResult(context, value->value));
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
        return JS_NewStringLen(context, (const char *)result->text,
                               result->text_len);
    default:
        return JS_ThrowInternalError(context, "invalid host return value");
    }
}

/* One crossing for a whole host->realm call: the arguments arrive as the
   boundary's own primitive descriptors and become JSValues in a stack array,
   so a call costs no per-argument heap box on either side of the ABI.

   Returns 0 when the member ran (`*result` owns the returned value), 1 when
   the target has no callable under that atom, and -1 with a pending exception
   otherwise. */
int qjs_call_member(JSContext *context, const QjsValue *target, uint32_t atom,
                    size_t argument_count, const QjsHostArg *arguments,
                    QjsValue **result) {
    JSValue inline_argv[QJS_HOST_INLINE_ARGS];
    JSValue *argv = inline_argv;
    JSValue member;
    JSValue returned;
    size_t built;
    int status = 0;

    *result = NULL;
    member = JS_GetProperty(context, target->value, (JSAtom)atom);
    if (JS_IsException(member)) {
        return -1;
    }
    if (!JS_IsFunction(context, member)) {
        JS_FreeValue(context, member);
        return 1;
    }
    if (argument_count > INT_MAX ||
        argument_count > SIZE_MAX / sizeof(*argv)) {
        JS_FreeValue(context, member);
        JS_ThrowRangeError(context, "too many call arguments");
        return -1;
    }
    if (argument_count > QJS_HOST_INLINE_ARGS) {
        argv = malloc(argument_count * sizeof(*argv));
        if (argv == NULL) {
            JS_FreeValue(context, member);
            JS_ThrowOutOfMemory(context);
            return -1;
        }
    }
    for (built = 0; built < argument_count; ++built) {
        argv[built] = qjs_host_build(context, &arguments[built]);
        if (JS_IsException(argv[built])) {
            status = -1;
            break;
        }
    }
    if (status == 0) {
        returned =
            JS_Call(context, member, JS_UNDEFINED, (int)argument_count, argv);
        /* `qjs_box` turns an exception into NULL and leaves it pending, and
           throws OOM itself if the box cannot be allocated. */
        *result = qjs_box(context, returned);
        if (*result == NULL) {
            status = -1;
        }
    }
    while (built > 0) {
        JS_FreeValue(context, argv[--built]);
    }
    if (argv != inline_argv) {
        free(argv);
    }
    JS_FreeValue(context, member);
    return status;
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
