/* QuickJS C ABI shim used by the Rust bridge. */

#include "quickjs.h"

#include <limits.h>
#include <math.h>
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

typedef struct QjsContext QjsContext;

typedef struct QjsValue {
    JSValue value;
} QjsValue;

typedef struct QjsUnhandledRejection {
    QjsContext *context;
    JSValue promise;
    JSValue reason;
    /* The runtime's `rejection_serial` when this was recorded, so a caller
       can tell what an evaluation it ran left behind. */
    uint64_t serial;
    struct QjsUnhandledRejection *next;
} QjsUnhandledRejection;

typedef int QjsInterruptCallback(void *opaque);

/* Module source text is context-independent, so one runtime holds one copy
   and every context on it compiles that copy into its own module. */
typedef struct QjsModuleSource {
    char *name;
    uint8_t *source;
    size_t source_length;
    char *url;
    char *error;
    int requested;
    struct QjsModuleSource *next;
} QjsModuleSource;

/* `JSModuleDef` lives on `JSContext.loaded_modules`, so what a source
   compiles to is per-context state, not runtime state. */
typedef struct QjsModuleInstance {
    const QjsModuleSource *source;
    JSModuleDef *definition;
    /* Rising per realm, in creation order, so the list is ordered by it: a
       comparison against `QjsContext.link_mark` is what says whether the
       compile now in progress is what made this instance, and so whether it
       is one no body has started running in yet. */
    uint64_t serial;
    /* Set once this realm has seen this module's own evaluation promise,
       which only a synchronous `require` of it produces. Both states that
       leaves it in — evaluated, or suspended on its own top-level await —
       are answered from rather than re-entered, and a module leaves neither
       of them, so this is the one thing about its status that stays true. */
    int evaluated;
    struct QjsModuleInstance *next;
} QjsModuleInstance;

typedef struct QjsHostModuleExport {
    char *name;
    JSValue value;
    struct QjsHostModuleExport *next;
} QjsHostModuleExport;

/* One runtime keeps one loader namespace, so a specifier a realm claims for
   a native module is reserved here for as long as any realm claims it. What
   it buys is that a source module registered later cannot silently shadow a
   realm's native module. */
typedef struct QjsHostModuleName {
    char *name;
    size_t users;
    struct QjsHostModuleName *next;
} QjsHostModuleName;

/* A module-name prefix reserved on a runtime: a name with it resolves only
   to a source the runtime registered or a native module the importing realm
   declared, and anything else fails in the realm that asked without becoming
   a request to the host. */
typedef struct QjsReservedPrefix {
    char *prefix;
    size_t length;
    struct QjsReservedPrefix *next;
} QjsReservedPrefix;

/* A native module's exports are `JSValue`s built in one context, so both the
   module and its exports belong to that context. */
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
    /* A structured clone: `JS_WriteObject`'s own bytes, in `text`. Opaque to
       the host, and read back by whichever realm of this build receives it. */
    QJS_ARG_STRUCTURED = 6,
    /* The serializer refused this value and has thrown. Never crosses into
       Rust: the trampoline returns the pending exception instead of
       dispatching. */
    QJS_ARG_FAILED = 7,
};


typedef struct QjsHostArg {
    int32_t kind;
    double number;
    const uint8_t *text;
    size_t text_len;
} QjsHostArg;


/* The result of a host call travels in the same struct an argument does:
   both are the boundary's own vocabulary — primitives plus opaque structured
   clones — and both carry their bytes in `text`. */
typedef QjsHostArg QjsHostResult;


typedef int QjsHostDispatch(void *opaque, QjsContext *context, void *handler,
                            size_t argument_count, const QjsHostArg *arguments,
                            QjsHostResult *result);


typedef void QjsHostRelease(void *opaque, void *handler);


typedef struct QjsRuntime {
    JSRuntime *raw;
    QjsUnhandledRejection *rejection_head;
    QjsUnhandledRejection *rejection_tail;
    /* Counts every rejection ever recorded; see `QjsUnhandledRejection`. */
    uint64_t rejection_serial;
    int rejection_tracker_oom;
    QjsInterruptCallback *interrupt_callback;
    void *interrupt_opaque;
    QjsHostDispatch *host_dispatch;
    QjsHostRelease *host_release;
    void *host_opaque;
    JSClassID host_owner_class_id;
    /* A `ShadowRealm` instance, and the object a wrapped function or a
       pending `importValue` keeps its realms in. Registered per runtime, as
       the host owner class is, from IDs the host allocated once. */
    JSClassID shadow_realm_class_id;
    JSClassID realm_link_class_id;
    QjsModuleSource *module_sources;
    QjsHostModuleName *host_module_names;
    QjsReservedPrefix *reserved_prefixes;
    /* How many module-graph evaluations are on the C stack: an entry that
       can run a module body raises it for as long as it might be doing so.
       At zero no body of any realm on this runtime is part-way through, and
       a module this runtime has is therefore safe to link or evaluate. */
    int evaluation_depth;
    /* The one object every realm given it shares by reference, and how many
       live realms have been given it; `JS_UNDEFINED` while none has. See
       `qjs_context_add_host_module_shared_object`. */
    JSValue shared_object;
    size_t shared_object_holders;
} QjsRuntime;


/* One realm. Reached from a bare `JSContext *` — a module load, a host call,
   a promise job — through the context opaque, which is cleared before the
   context is released so a late callback finds nothing instead of a
   dangling wrapper. */
typedef const char *QjsModuleNormalize(void *opaque, const char *base,
                                        const char *name, int *error);

typedef struct QjsDeferredImport {
    char *base;
    char *name;
    JSValue resolve[2];
    JSValue attributes;
    struct QjsDeferredImport *next;
} QjsDeferredImport;

/* How a synchronously loaded source is read. The host answers one of these
   beside the text; `QJS_REQUIRED_DETECT` is it declining to, which only C
   can settle, since only C has the text and QuickJS's own detector. */
enum QjsRequiredKind {
    QJS_REQUIRED_COMMONJS = 0,
    QJS_REQUIRED_JSON = 1,
    QJS_REQUIRED_MODULE = 2,
    QJS_REQUIRED_DETECT = 3,
};

/* One synchronous source load. The host fills either the three source fields
   or `error`, and keeps owning both: the buffers are borrowed until the
   callback is entered again, which a load started from a file's own text
   does, so C copies the URL and compiles or parses the text before it runs
   anything of that file's. */
typedef struct QjsRequiredSource {
    const char *url;
    const uint8_t *text;
    size_t text_length;
    int32_t kind;
    const char *error;
} QjsRequiredSource;

typedef int QjsRequireLoad(void *opaque, const char *url,
                           QjsRequiredSource *source);

/* HostInitializeShadowRealm: called inside `new ShadowRealm()` with the
   fresh child realm, which the host owns from the moment it is called —
   whatever it answers, C never frees the child itself. Nonzero refuses the
   construction; `*message`, which the host keeps alive until its next call,
   is then what the `TypeError` says. */
typedef int QjsShadowRealmInit(void *opaque, QjsContext *child,
                               const char **message);

typedef struct QjsRealmToken QjsRealmToken;

struct QjsContext {
    JSContext *raw;
    QjsRuntime *runtime;
    QjsModuleInstance *module_instances;
    QjsHostModule *host_modules;
    QjsModuleSource *module_sources;
    QjsDeferredImport *deferred_imports;
    QjsModuleNormalize *normalize;
    void *normalize_opaque;
    QjsRequireLoad *require_load;
    void *require_opaque;
    /* Inline linking: while it is raised, an import this realm meets is
       resolved through the synchronous loader instead of being deferred to
       the host's own fetch. A `require` inside a required module's body
       raises it again, so it counts rather than flags. */
    int synchronous_link;
    /* The compile half of that: raised only while source text is being
       compiled, which is where the instances that no body has run in yet
       are made. `JS_EvalFunction` is outside it. */
    int synchronous_compile;
    /* The serial the next module instance of this realm takes, and the
       highest one that could already have been evaluated. QuickJS keeps a
       module's evaluation status private, so these two, with the runtime's
       evaluation depth, are how a realm answers the only question it needs
       of that status: may this module be linked or evaluated now, without
       re-entering a body that is already running? */
    uint64_t instance_serial;
    uint64_t link_mark;
    /* The host's half of `ShadowRealm`, set by installing it. */
    QjsShadowRealmInit *shadow_init;
    void *shadow_opaque;
    /* This realm's own intrinsic `%eval%` when it is a ShadowRealm's child,
       taken before any code of it ran: `evaluate` is an indirect eval, and
       the global `eval` binding is the child's code's to replace. */
    JSValue shadow_eval;
    /* How objects of other realms name this one; made the first time one
       needs to. */
    QjsRealmToken *token;
    /* Whether this realm was given the runtime's shared object, and so is
       one of the holders its release counts down. */
    int holds_shared_object;
};


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


/* One realm, as an object of another realm names it across a ShadowRealm
   boundary.

   What crosses is a JavaScript object of one realm that has to reach
   another — a `ShadowRealm` instance in its parent, a wrapped function in
   the realm it was handed to, a pending `importValue` — and the realm it
   reaches is the host's, not the object's. QuickJS has no public way for a
   class of ours to trace a `JSContext`, so a `JS_DupContext` held by one of
   those objects would be a reference the collector cannot see, and every
   cycle through it would be uncollectable. They hold this token instead: C
   counts it, a finalizer may release it, and its `context` goes NULL as the
   first thing `qjs_context_free` does, so every crossing after that throws
   instead of reaching freed memory.

   Two counts, because they answer two questions. `references` is memory:
   the realm's own reference plus one per object that names the token, and
   the token is freed when it reaches zero, whichever of the realm and the
   objects goes last. `holders` is liveness, the host's question — how many
   of those objects need this realm alive to do their job: a `ShadowRealm`
   whose child it is, a wrapped function whose target lives in it, an
   `importValue` whose module loads in it. A pending `importValue` also names
   the realm that asked, only to settle its promise if it is still there, and
   that watch is a reference and not a holder: it needs nothing kept. */
struct QjsRealmToken {
    /* NULL once `qjs_context_free` has begun. */
    QjsContext *context;
    size_t references;
    size_t holders;
};

/* The realm's token, made on first use; the realm's own reference is the
   first one. NULL only when it could not be allocated. */
static QjsRealmToken *qjs_context_token(QjsContext *context) {
    QjsRealmToken *token = context->token;

    if (token == NULL) {
        token = calloc(1, sizeof(*token));
        if (token == NULL) {
            return NULL;
        }
        token->context = context;
        token->references = 1;
        context->token = token;
    }
    return token;
}

static void qjs_token_hold(QjsRealmToken *token) {
    token->references += 1;
    token->holders += 1;
}

static void qjs_token_watch(QjsRealmToken *token) {
    token->references += 1;
}

/* Pure C, so a GC finalizer may call it. */
static void qjs_token_release(QjsRealmToken *token, int held) {
    if (held) {
        token->holders -= 1;
    }
    token->references -= 1;
    if (token->references == 0) {
        free(token);
    }
}

/* What a wrapped function or a pending `importValue` keeps in a function-data
   slot, which `js_c_function_data_mark` traces: the realm it works in, held,
   and the realm a pending import settles a promise of, watched. Never exposed
   to JavaScript. */
typedef struct QjsShadowLink {
    QjsRealmToken *target;
    QjsRealmToken *caller;
} QjsShadowLink;

static void qjs_realm_link_finalizer(JSRuntime *raw, JSValue value) {
    QjsShadowLink *link = JS_GetOpaque(value, JS_GetClassID(value));

    (void)raw;
    if (link == NULL) {
        return;
    }
    qjs_token_release(link->target, 1);
    if (link->caller != NULL) {
        qjs_token_release(link->caller, 0);
    }
    free(link);
}

static const JSClassDef qjs_realm_link_class = {
    "QjsRealmLink",
    .finalizer = qjs_realm_link_finalizer,
};

/* A `ShadowRealm` instance holds its child realm's token as its opaque. */
static void qjs_shadow_realm_finalizer(JSRuntime *raw, JSValue value) {
    QjsRealmToken *token = JS_GetOpaque(value, JS_GetClassID(value));

    (void)raw;
    if (token != NULL) {
        qjs_token_release(token, 1);
    }
}

static const JSClassDef qjs_shadow_realm_class = {
    "ShadowRealm",
    .finalizer = qjs_shadow_realm_finalizer,
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

_Static_assert(QJS_REQUIRED_COMMONJS == 0,
               "Rust QJS_REQUIRED_COMMONJS must match shim.c");
_Static_assert(QJS_REQUIRED_JSON == 1,
               "Rust QJS_REQUIRED_JSON must match shim.c");
_Static_assert(QJS_REQUIRED_MODULE == 2,
               "Rust QJS_REQUIRED_MODULE must match shim.c");
_Static_assert(QJS_REQUIRED_DETECT == 3,
               "Rust QJS_REQUIRED_DETECT must match shim.c");

enum QjsRejectionStatus {
    QJS_REJECTION_NONE = 0,
    QJS_REJECTION_TAKEN = 1,
    QJS_REJECTION_TRACKER_OOM = -1,
};

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

static void qjs_promise_rejection_tracker(JSContext *raw_context,
                                          JSValueConst promise,
                                          JSValueConst reason,
                                          JS_BOOL is_handled, void *opaque) {
    QjsRuntime *runtime = opaque;
    QjsContext *context = JS_GetContextOpaque(raw_context);
    QjsUnhandledRejection *current;
    QjsUnhandledRejection *previous = NULL;

    /* A context whose wrapper is gone has no host left to report to. Its
       jobs can still run — QuickJS keeps the realm alive while they do. */
    if (context == NULL) {
        return;
    }
    if (is_handled) {
        current = runtime->rejection_head;
        while (current != NULL) {
            if (current->context == context &&
                JS_StrictEq(raw_context, current->promise, promise)) {
                if (previous == NULL) {
                    runtime->rejection_head = current->next;
                } else {
                    previous->next = current->next;
                }
                if (runtime->rejection_tail == current) {
                    runtime->rejection_tail = previous;
                }
                JS_FreeValue(raw_context, current->promise);
                JS_FreeValue(raw_context, current->reason);
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
    current->context = context;
    current->promise = JS_DupValue(raw_context, promise);
    current->reason = JS_DupValue(raw_context, reason);
    current->serial = ++runtime->rejection_serial;
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

static QjsModuleInstance *qjs_find_module_instance(QjsContext *context,
                                                   const QjsModuleSource *source) {
    QjsModuleInstance *instance = context->module_instances;

    while (instance != NULL && instance->source != source) {
        instance = instance->next;
    }
    return instance;
}

static QjsHostModuleName *qjs_find_host_module_name(QjsRuntime *runtime,
                                                    const char *name) {
    QjsHostModuleName *reserved = runtime->host_module_names;

    while (reserved != NULL && strcmp(reserved->name, name) != 0) {
        reserved = reserved->next;
    }
    return reserved;
}

static int qjs_reserve_host_module_name(QjsRuntime *runtime,
                                        const char *name) {
    QjsHostModuleName *reserved = qjs_find_host_module_name(runtime, name);
    size_t name_length;

    if (reserved != NULL) {
        reserved->users += 1;
        return 0;
    }
    name_length = strlen(name);
    reserved = calloc(1, sizeof(*reserved));
    if (reserved == NULL) {
        return -1;
    }
    reserved->name = malloc(name_length + 1);
    if (reserved->name == NULL) {
        free(reserved);
        return -1;
    }
    memcpy(reserved->name, name, name_length + 1);
    reserved->users = 1;
    reserved->next = runtime->host_module_names;
    runtime->host_module_names = reserved;
    return 0;
}

static void qjs_release_host_module_name(QjsRuntime *runtime,
                                         const char *name) {
    QjsHostModuleName *reserved = runtime->host_module_names;
    QjsHostModuleName *previous = NULL;

    while (reserved != NULL && strcmp(reserved->name, name) != 0) {
        previous = reserved;
        reserved = reserved->next;
    }
    if (reserved == NULL) {
        return;
    }
    reserved->users -= 1;
    if (reserved->users > 0) {
        return;
    }
    if (previous == NULL) {
        runtime->host_module_names = reserved->next;
    } else {
        previous->next = reserved->next;
    }
    free(reserved->name);
    free(reserved);
}

static void qjs_host_module_names_free(QjsHostModuleName *reserved) {
    while (reserved != NULL) {
        QjsHostModuleName *next = reserved->next;
        free(reserved->name);
        free(reserved);
        reserved = next;
    }
}

static int qjs_module_name_is_reserved(const QjsRuntime *runtime,
                                       const char *name) {
    const QjsReservedPrefix *reserved = runtime->reserved_prefixes;
    size_t length = strlen(name);

    while (reserved != NULL &&
           (length < reserved->length ||
            memcmp(name, reserved->prefix, reserved->length) != 0)) {
        reserved = reserved->next;
    }
    return reserved != NULL;
}

static void qjs_reserved_prefixes_free(QjsReservedPrefix *reserved) {
    while (reserved != NULL) {
        QjsReservedPrefix *next = reserved->next;
        free(reserved->prefix);
        free(reserved);
        reserved = next;
    }
}

static QjsHostModule *qjs_find_host_module(QjsContext *context,
                                           const char *name) {
    QjsHostModule *module = context->host_modules;

    while (module != NULL && strcmp(module->name, name) != 0) {
        module = module->next;
    }
    return module;
}

static int qjs_host_module_init(JSContext *raw_context,
                                JSModuleDef *definition) {
    QjsContext *context = JS_GetContextOpaque(raw_context);
    QjsHostModule *module;
    QjsHostModuleExport *exported;

    if (context == NULL) {
        return -1;
    }
    module = context->host_modules;
    while (module != NULL && module->definition != definition) {
        module = module->next;
    }
    if (module == NULL) {
        JS_ThrowInternalError(raw_context,
                              "native host module is not registered");
        return -1;
    }
    for (exported = module->exports; exported != NULL;
         exported = exported->next) {
        if (JS_SetModuleExport(raw_context, definition, exported->name,
                               JS_DupValue(raw_context, exported->value)) < 0) {
            return -1;
        }
    }
    return 0;
}

static char *qjs_strdup(const char *text) {
    size_t length = strlen(text) + 1;
    char *copy = malloc(length);
    if (copy != NULL)
        memcpy(copy, text, length);
    return copy;
}

static QjsModuleSource *qjs_find_local_source(QjsContext *context,
                                                const char *name) {
    QjsModuleSource *source = context->module_sources;
    while (source != NULL && strcmp(source->name, name) != 0)
        source = source->next;
    return source;
}

static char *qjs_module_normalize(JSContext *raw, const char *base,
                                   const char *name, void *opaque) {
    QjsContext *context = JS_GetContextOpaque(raw);
    QjsModuleSource *source;
    const char *normalized;
    int error = 0;
    (void)opaque;
    if (context == NULL || context->normalize == NULL)
        return JS_DefaultModuleNormalizeName(raw, base, name);
    source = qjs_find_local_source(context, base);
    if (source != NULL && source->url != NULL)
        base = source->url;
    normalized = context->normalize(context->normalize_opaque, base, name, &error);
    if (error) {
        JS_ThrowTypeError(raw, "%s", normalized);
        return NULL;
    }
    return js_strdup(raw, normalized);
}

static int qjs_defer_module(JSContext *raw, const char *base, const char *name,
                            JSValueConst *resolve, JSValueConst attributes,
                            void *opaque) {
    QjsContext *context = JS_GetContextOpaque(raw);
    QjsDeferredImport *pending;
    (void)opaque;
    if (context == NULL) {
        JS_ThrowInternalError(raw, "this realm is being released");
        return -1;
    }
    pending = calloc(1, sizeof(*pending));
    if (pending == NULL) {
        JS_ThrowOutOfMemory(raw);
        return -1;
    }
    pending->base = qjs_strdup(base);
    pending->name = qjs_strdup(name);
    if (pending->base == NULL || pending->name == NULL) {
        free(pending->base);
        free(pending->name);
        free(pending);
        JS_ThrowOutOfMemory(raw);
        return -1;
    }
    pending->resolve[0] = JS_DupValue(raw, resolve[0]);
    pending->resolve[1] = JS_DupValue(raw, resolve[1]);
    pending->attributes = JS_DupValue(raw, attributes);
    pending->next = context->deferred_imports;
    context->deferred_imports = pending;
    return 0;
}

static void qjs_deferred_import_free(JSContext *raw, QjsDeferredImport *pending) {
    free(pending->base);
    free(pending->name);
    JS_FreeValue(raw, pending->resolve[0]);
    JS_FreeValue(raw, pending->resolve[1]);
    JS_FreeValue(raw, pending->attributes);
    free(pending);
}

/* Gives one compiled module its `import.meta.url`. Called between compiling
   and linking, which is where the module definition is the host's to reach. */
static int qjs_set_import_meta_url(JSContext *raw, JSModuleDef *definition,
                                   const char *url) {
    JSValue meta = JS_GetImportMeta(raw, definition);
    JSValue value;
    int status;

    if (JS_IsException(meta))
        return -1;
    value = JS_NewString(raw, url);
    if (JS_IsException(value)) {
        JS_FreeValue(raw, meta);
        return -1;
    }
    status = JS_DefinePropertyValueStr(raw, meta, "url", value, JS_PROP_C_W_E);
    JS_FreeValue(raw, meta);
    return status;
}

/* Compiles one source this realm has into a module of it, and remembers the
   instance.

   `linked` resolves every import during the compile, through this same
   loader, which is what an inline link — a `require` — needs and what an
   asynchronous import must not do: there, an import is answered with NULL
   and deferred until the host has fetched its source.

   The module is named by the name the source is known by rather than by the
   URL it answered from, because that name is what an import of it normalizes
   to: QuickJS registers the module under it before parsing the body, so an
   import that comes back around to this very module while it is still being
   compiled resolves from `JSContext.loaded_modules` instead of reaching this
   loader again. `import.meta.url` is the response URL either way. */
static JSModuleDef *qjs_compile_module_source(QjsContext *context,
                                              const QjsModuleSource *source,
                                              int linked) {
    JSContext *raw_context = context->raw;
    QjsModuleInstance *instance = calloc(1, sizeof(*instance));
    JSValue compiled;
    JSModuleDef *definition;

    if (instance == NULL) {
        JS_ThrowOutOfMemory(raw_context);
        return NULL;
    }
    compiled = JS_Eval(raw_context, (const char *)source->source,
                       source->source_length, source->name,
                       JS_EVAL_TYPE_MODULE | JS_EVAL_FLAG_COMPILE_ONLY |
                       (linked ? 0 : JS_EVAL_FLAG_COMPILE_UNLINKED));
    if (JS_IsException(compiled)) {
        free(instance);
        return NULL;
    }
    definition = JS_VALUE_GET_PTR(compiled);
    JS_FreeValue(raw_context, compiled);
    /* A realm-local module knows the URL it answered from; a source the
       runtime carries is known by the name it was registered under. */
    if (qjs_set_import_meta_url(
            raw_context, definition,
            source->url != NULL ? source->url : source->name) < 0) {
        free(instance);
        return NULL;
    }
    instance->source = source;
    instance->definition = definition;
    instance->serial = ++context->instance_serial;
    /* Only a compile a `require` drives makes an instance nothing can have
       evaluated yet. One made anywhere else is linked into a graph this
       realm is about to evaluate on its own, so it goes on the far side of
       the mark: what is known about it from here on is only what was seen. */
    if (context->synchronous_compile == 0) {
        context->link_mark = context->instance_serial;
    }
    instance->next = context->module_instances;
    context->module_instances = instance;
    return definition;
}

/* May this module be linked into another, or evaluated, without re-entering
   a body that is already running?

   QuickJS keeps `JSModuleDef`'s status private, and re-entering a module
   that is part-way through its body corrupts the evaluation that is running
   it, so a realm answers from the three things it can know for itself:

   - a module this realm has already put through `JS_EvalFunction` is evaluated, suspended on its
     own top-level await, or back to unlinked because linking is what failed, and QuickJS answers
     from each of the three rather than re-entering a body;
   - a module the compile now in progress made has had no body start in it at all;
   - with no module-graph evaluation anywhere on the stack, no body of any module is part-way
     through.

   Anything else is refused. That is narrower than Node, which reads the
   status and refuses only a true cycle: a module this realm reached by an
   `import` and is asked for by a `require` from inside another module's body
   is refused here even when its body has already finished. */
static int qjs_module_is_quiet(const QjsContext *context,
                               const QjsModuleInstance *instance) {
    return instance->evaluated || instance->serial > context->link_mark ||
           context->runtime->evaluation_depth == 0;
}

/* Throws what a `require` cannot do, as the `Error` the caller reads. */
static JSValue qjs_throw_require_error(JSContext *raw, const char *url,
                                       const char *reason);

/* The realm-local source table, which an inline link fills as the host's own
   asynchronous completion does. Declared here because the loader reaches it
   and the ABI entry it belongs to is further down. */
int qjs_context_complete_module(QjsContext *context, const char *name,
                                const char *url, const uint8_t *text,
                                size_t length, const char *error);

/* Loads one import's source through the host's synchronous loader and copies
   it into this realm.

   The copy is the point: the host lends its buffers only until the next
   load, and compiling this source is what starts the next one. */
static int qjs_require_module_source(QjsContext *context, const char *name) {
    QjsRequiredSource loaded;
    QjsModuleSource *source;
    int status;

    loaded.url = NULL;
    loaded.text = NULL;
    loaded.text_length = 0;
    loaded.kind = QJS_REQUIRED_COMMONJS;
    loaded.error = NULL;
    if (context->require_load(context->require_opaque, name, &loaded) != 0) {
        /* An import whose source cannot be had is unresolvable, which is
           what a `ReferenceError` says here and in QuickJS's own loader. */
        JS_ThrowReferenceError(context->raw, "%s", loaded.error);
        return -1;
    }
    /* -5 is this name having been answered already, which is the host's own
       fetch of it having landed first: its source is this source. */
    status = qjs_context_complete_module(context, name, loaded.url,
                                         loaded.text, loaded.text_length,
                                         NULL);
    if (status != 0 && status != -5) {
        if (!JS_HasException(context->raw)) {
            JS_ThrowInternalError(context->raw,
                                  "module '%s' could not be completed", name);
        }
        return -1;
    }
    source = qjs_find_local_source(context, name);
    if (source == NULL) {
        JS_ThrowInternalError(context->raw, "module '%s' was not stored", name);
        return -1;
    }
    /* The host has answered it: nothing is left for its own fetch to take. */
    source->requested = 1;
    return 0;
}

static JSModuleDef *qjs_module_loader(JSContext *raw_context,
                                      const char *module_name, void *opaque,
                                      JSValueConst attributes) {
    QjsRuntime *runtime = opaque;
    QjsContext *context = JS_GetContextOpaque(raw_context);
    QjsModuleSource *source;
    QjsModuleInstance *instance;
    QjsHostModule *host_module;
    QjsHostModuleExport *exported;
    JSModuleDef *definition;
    (void)attributes;

    if (context == NULL) {
        JS_ThrowInternalError(raw_context, "this realm is being released");
        return NULL;
    }
    source = qjs_find_local_source(context, module_name);
    if (source == NULL)
        source = qjs_find_module_source(runtime, module_name);
    if (source != NULL && source->error != NULL) {
        JS_ThrowTypeError(raw_context, "%s", source->error);
        return NULL;
    }
    if (source != NULL && source->source == NULL) {
        /* A source the host has yet to answer. An import being linked inline
           cannot wait for that fetch, so it asks the same synchronous loader
           the `require` underneath it came through; the answer completes the
           request the host already took, which its own answer then finds
           already made. */
        if (context->synchronous_link == 0) {
            return NULL;
        }
        if (qjs_require_module_source(context, module_name) < 0) {
            return NULL;
        }
        source = qjs_find_local_source(context, module_name);
    }
    if (source != NULL) {
        instance = qjs_find_module_instance(context, source);
        if (instance != NULL) {
            if (context->synchronous_link > 0 &&
                !qjs_module_is_quiet(context, instance)) {
                qjs_throw_require_error(
                    raw_context,
                    source->url != NULL ? source->url : source->name,
                    "it is part of a module graph that is still evaluating");
                return NULL;
            }
            return instance->definition;
        }
        return qjs_compile_module_source(context, source,
                                         context->synchronous_link > 0);
    }

    host_module = qjs_find_host_module(context, module_name);
    /* The realm's own sources, answered or pending, the runtime's sources
       and this realm's native modules have all been looked in. A reserved
       name none of them has is not the host's to fetch, so it is neither
       loaded synchronously nor recorded as a request. */
    if (host_module == NULL &&
        qjs_module_name_is_reserved(runtime, module_name)) {
        JS_ThrowReferenceError(raw_context, "module '%s' is not preloaded",
                               module_name);
        return NULL;
    }
    if (host_module == NULL && context->synchronous_link > 0 &&
        context->require_load != NULL) {
        if (qjs_require_module_source(context, module_name) < 0) {
            return NULL;
        }
        source = qjs_find_local_source(context, module_name);
        if (source == NULL) {
            JS_ThrowInternalError(raw_context, "module '%s' was not stored",
                                  module_name);
            return NULL;
        }
        return qjs_compile_module_source(context, source, 1);
    }
    if (host_module == NULL && context->normalize != NULL) {
        source = calloc(1, sizeof(*source));
        if (source == NULL) {
            JS_ThrowOutOfMemory(raw_context);
            return NULL;
        }
        source->name = qjs_strdup(module_name);
        if (source->name == NULL) {
            free(source);
            JS_ThrowOutOfMemory(raw_context);
            return NULL;
        }
        source->next = context->module_sources;
        context->module_sources = source;
        return NULL;
    }
    if (host_module == NULL) {
        JS_ThrowReferenceError(raw_context, "module '%s' is not preloaded",
                               module_name);
        return NULL;
    }
    if (host_module->definition != NULL) {
        return host_module->definition;
    }
    definition = JS_NewCModule(raw_context, module_name, qjs_host_module_init);
    if (definition == NULL) {
        return NULL;
    }
    host_module->definition = definition;
    for (exported = host_module->exports; exported != NULL;
         exported = exported->next) {
        if (JS_AddModuleExport(raw_context, definition, exported->name) < 0) {
            return NULL;
        }
    }
    return definition;
}

static int qjs_check_module_attributes(JSContext *raw, void *opaque,
                                       JSValueConst attributes) {
    JSPropertyEnum *properties;
    uint32_t count;
    (void)opaque;
    if (JS_IsUndefined(attributes))
        return 0;
    if (JS_GetOwnPropertyNames(raw, &properties, &count, attributes,
                              JS_GPN_STRING_MASK | JS_GPN_ENUM_ONLY) < 0)
        return -1;
    JS_FreePropertyEnum(raw, properties, count);
    if (count != 0) {
        JS_ThrowTypeError(raw, "import attributes are not supported");
        return -1;
    }
    return 0;
}

static void qjs_module_sources_free(QjsModuleSource *module) {
    while (module != NULL) {
        QjsModuleSource *next = module->next;
        free(module->name);
        free(module->source);
        free(module->url);
        free(module->error);
        free(module);
        module = next;
    }
}

static void qjs_module_instances_free(QjsModuleInstance *instance) {
    while (instance != NULL) {
        QjsModuleInstance *next = instance->next;
        free(instance);
        instance = next;
    }
}

static void qjs_host_modules_free(QjsContext *context,
                                  QjsHostModule *module) {
    while (module != NULL) {
        QjsHostModule *next = module->next;
        QjsHostModuleExport *exported = module->exports;
        while (exported != NULL) {
            QjsHostModuleExport *next_export = exported->next;
            JS_FreeValue(context->raw, exported->value);
            free(exported->name);
            free(exported);
            exported = next_export;
        }
        qjs_release_host_module_name(context->runtime, module->name);
        free(module->name);
        free(module);
        module = next;
    }
}

static void qjs_rejections_drop_context(QjsRuntime *runtime,
                                        QjsContext *context) {
    QjsUnhandledRejection *current = runtime->rejection_head;
    QjsUnhandledRejection *previous = NULL;

    while (current != NULL) {
        QjsUnhandledRejection *next = current->next;

        if (current->context == context) {
            if (previous == NULL) {
                runtime->rejection_head = next;
            } else {
                previous->next = next;
            }
            if (runtime->rejection_tail == current) {
                runtime->rejection_tail = previous;
            }
            JS_FreeValue(context->raw, current->promise);
            JS_FreeValue(context->raw, current->reason);
            free(current);
        } else {
            previous = current;
        }
        current = next;
    }
}

JSClassID qjs_host_owner_class_id_new(void) {
    JSClassID class_id = 0;

    return JS_NewClassID(&class_id);
}

/* The three class IDs are allocated once per process by the host and
   registered here per runtime. */
QjsRuntime *qjs_runtime_new(JSClassID host_owner_class_id,
                            JSClassID shadow_realm_class_id,
                            JSClassID realm_link_class_id) {
    QjsRuntime *runtime = calloc(1, sizeof(*runtime));
    if (runtime == NULL) {
        return NULL;
    }
    runtime->raw = JS_NewRuntime();
    if (runtime->raw == NULL) {
        free(runtime);
        return NULL;
    }
    JS_SetCanBlock(runtime->raw, 0);
    JS_SetHostPromiseRejectionTracker(runtime->raw,
                                      qjs_promise_rejection_tracker, runtime);
    JS_SetModuleLoaderFunc2(runtime->raw, qjs_module_normalize, qjs_module_loader,
                            qjs_check_module_attributes, runtime);
    JS_SetModuleLoadDeferrer(runtime->raw, qjs_defer_module, runtime);
    runtime->shared_object = JS_UNDEFINED;
    runtime->host_owner_class_id = host_owner_class_id;
    runtime->shadow_realm_class_id = shadow_realm_class_id;
    runtime->realm_link_class_id = realm_link_class_id;
    if (JS_NewClass(runtime->raw, runtime->host_owner_class_id,
                    &qjs_host_owner_class) < 0 ||
        JS_NewClass(runtime->raw, runtime->shadow_realm_class_id,
                    &qjs_shadow_realm_class) < 0 ||
        JS_NewClass(runtime->raw, runtime->realm_link_class_id,
                    &qjs_realm_link_class) < 0) {
        JS_FreeRuntime(runtime->raw);
        free(runtime);
        return NULL;
    }
    /* A wrapped function is a C-function-data callback, which is handed the
       caller's context and nothing else; this is how it finds the class IDs
       it checks its own data against. */
    JS_SetRuntimeOpaque(runtime->raw, runtime);
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
    JS_FreeValueRT(runtime->raw, runtime->shared_object);
    JS_FreeRuntime(runtime->raw);
    qjs_module_sources_free(runtime->module_sources);
    qjs_host_module_names_free(runtime->host_module_names);
    qjs_reserved_prefixes_free(runtime->reserved_prefixes);
    free(runtime);
}

int qjs_runtime_reserve_module_prefix(QjsRuntime *runtime, const char *prefix) {
    QjsReservedPrefix *reserved = calloc(1, sizeof(*reserved));

    if (reserved == NULL) {
        return -1;
    }
    reserved->prefix = qjs_strdup(prefix);
    if (reserved->prefix == NULL) {
        free(reserved);
        return -1;
    }
    reserved->length = strlen(prefix);
    reserved->next = runtime->reserved_prefixes;
    runtime->reserved_prefixes = reserved;
    return 0;
}

int qjs_runtime_add_module(QjsRuntime *runtime, const char *name,
                           const uint8_t *source, size_t source_length) {
    QjsModuleSource *module;
    size_t name_length;

    if (qjs_find_module_source(runtime, name) != NULL ||
        qjs_find_host_module_name(runtime, name) != NULL) {
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

QjsContext *qjs_context_new(QjsRuntime *runtime) {
    QjsContext *context = calloc(1, sizeof(*context));

    if (context == NULL) {
        return NULL;
    }
    context->runtime = runtime;
    context->shadow_eval = JS_UNDEFINED;
    context->raw = JS_NewContext(runtime->raw);
    if (context->raw == NULL) {
        free(context);
        return NULL;
    }
    JS_SetContextOpaque(context->raw, context);
    return context;
}

void qjs_context_free(QjsContext *context) {
    /* First, before anything below can run a finalizer or a GC: every object
       of another realm that names this one finds it gone from here on, and
       throws instead of reaching what is about to be freed. */
    if (context->token != NULL) {
        context->token->context = NULL;
        qjs_token_release(context->token, 0);
        context->token = NULL;
    }
    qjs_rejections_drop_context(context->runtime, context);
    qjs_host_modules_free(context, context->host_modules);
    /* The export above was this realm's reference; the runtime's own goes
       with its last holder, before the collection below, so what the object
       kept alive is collected with this realm. */
    if (context->holds_shared_object &&
        --context->runtime->shared_object_holders == 0) {
        JS_FreeValueRT(context->runtime->raw, context->runtime->shared_object);
        context->runtime->shared_object = JS_UNDEFINED;
    }
    qjs_module_instances_free(context->module_instances);
    qjs_module_sources_free(context->module_sources);
    while (context->deferred_imports != NULL) {
        QjsDeferredImport *pending = context->deferred_imports;
        context->deferred_imports = pending->next;
        qjs_deferred_import_free(context->raw, pending);
    }
    JS_FreeValue(context->raw, context->shadow_eval);
    context->shadow_eval = JS_UNDEFINED;
    /* Pending jobs and settling promises keep the realm alive past this
       point. They must not find a wrapper the host no longer holds. */
    JS_SetContextOpaque(context->raw, NULL);
    JS_FreeContext(context->raw);
    /* A realm's global object is cyclic, so releasing the context only drops
       a reference — the collector is what actually reaches the finalizers,
       and those are the host's only signal that its closures are free. */
    JS_RunGC(context->runtime->raw);
    free(context);
}

/* Adds one already-built export value to a native module of this realm. The
   module's export list holds a reference of its own, which `qjs_context_free`
   releases before the context. */
static int qjs_add_host_module_export(QjsContext *context, const char *name,
                                      const char *export_name,
                                      JSValueConst value) {
    QjsHostModule *module;
    QjsHostModuleExport *current;
    QjsHostModuleExport *exported;
    size_t export_name_length;
    int new_module = 0;

    if (qjs_find_module_source(context->runtime, name) != NULL ||
        qjs_find_local_source(context, name) != NULL) {
        return -2;
    }
    module = qjs_find_host_module(context, name);
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
        if (qjs_reserve_host_module_name(context->runtime, name) < 0) {
            free(module->name);
            free(module);
            return -1;
        }
        new_module = 1;
    }

    export_name_length = strlen(export_name);
    exported = calloc(1, sizeof(*exported));
    if (exported == NULL) {
        if (new_module) {
            qjs_release_host_module_name(context->runtime, module->name);
            free(module->name);
            free(module);
        }
        return -1;
    }
    exported->name = malloc(export_name_length + 1);
    if (exported->name == NULL) {
        free(exported);
        if (new_module) {
            qjs_release_host_module_name(context->runtime, module->name);
            free(module->name);
            free(module);
        }
        return -1;
    }
    memcpy(exported->name, export_name, export_name_length + 1);
    exported->value = JS_DupValue(context->raw, value);
    exported->next = module->exports;
    module->exports = exported;
    if (new_module) {
        module->next = context->host_modules;
        context->host_modules = module;
    }
    return 0;
}

int qjs_context_add_host_module_export(QjsContext *context, const char *name,
                                       const char *export_name,
                                       const QjsValue *value) {
    return qjs_add_host_module_export(context, name, export_name, value->value);
}

/* Adds the runtime's shared object as one export of a native module of this
   realm: one object with a null prototype, so it names no realm's
   intrinsics, and the same reference in every realm of the runtime that is
   given it. It is the only value one realm can reach in another without a
   host function between them. The runtime keeps it while any realm it was
   given to is alive; the next realm given it after the last of those is
   freed gets a new, empty one. */
int qjs_context_add_host_module_shared_object(QjsContext *context,
                                              const char *name,
                                              const char *export_name) {
    QjsRuntime *runtime = context->runtime;
    int status;

    if (JS_IsUndefined(runtime->shared_object)) {
        JSValue object = JS_NewObjectProto(context->raw, JS_NULL);
        if (JS_IsException(object)) {
            JS_FreeValue(context->raw, JS_GetException(context->raw));
            return -1;
        }
        runtime->shared_object = object;
    }
    status = qjs_add_host_module_export(context, name, export_name,
                                        runtime->shared_object);
    if (status == 0 && !context->holds_shared_object) {
        context->holds_shared_object = 1;
        runtime->shared_object_holders += 1;
    }
    if (runtime->shared_object_holders == 0) {
        JS_FreeValueRT(runtime->raw, runtime->shared_object);
        runtime->shared_object = JS_UNDEFINED;
    }
    return status;
}

QjsValue *qjs_module_namespace(QjsContext *context, const char *name) {
    const QjsModuleSource *source = qjs_find_local_source(context, name);
    if (source == NULL)
        source = qjs_find_module_source(context->runtime, name);
    QjsModuleInstance *instance;
    QjsHostModule *host;
    JSModuleDef *definition = NULL;

    if (source != NULL) {
        instance = qjs_find_module_instance(context, source);
        if (instance != NULL) {
            definition = instance->definition;
        }
    } else {
        host = qjs_find_host_module(context, name);
        if (host != NULL) {
            definition = host->definition;
        }
    }
    if (definition == NULL) {
        JS_ThrowReferenceError(context->raw, "module '%s' has not been loaded",
                               name);
        return NULL;
    }
    return qjs_box(context->raw,
                   JS_GetModuleNamespace(context->raw, definition));
}

void qjs_context_enable_module_loading(QjsContext *context,
                                        QjsModuleNormalize *normalize,
                                        void *opaque) {
    context->normalize = normalize;
    context->normalize_opaque = opaque;
}

/* Node's module wrapper, built around the parameter list the caller names.
   Neither the prologue nor the infix carries a newline, so a line in the body
   is the line it sits on in the file. */
static const char QJS_WRAPPER_PROLOGUE[] = "(function (";
static const char QJS_WRAPPER_INFIX[] = ") {";
static const char QJS_WRAPPER_EPILOGUE[] = "\n})";

/* The three shapes a loaded source is read as, as the answer names them. */
static const char QJS_KIND_COMMONJS[] = "commonjs";
static const char QJS_KIND_JSON[] = "json";
static const char QJS_KIND_MODULE[] = "module";

/* Throws what a failed load reports, as the `Error` the host worded. */
static JSValue qjs_require_throw_load(JSContext *raw, const char *reason) {
    JSValue error = JS_NewError(raw);
    JSValue text = JS_NewString(raw, reason);

    if (JS_IsException(error) || JS_IsException(text)) {
        JS_FreeValue(raw, error);
        JS_FreeValue(raw, text);
        return JS_EXCEPTION;
    }
    if (JS_DefinePropertyValueStr(raw, error, "message", text,
                                  JS_PROP_WRITABLE | JS_PROP_CONFIGURABLE) <
        0) {
        JS_FreeValue(raw, error);
        return JS_EXCEPTION;
    }
    return JS_Throw(raw, error);
}

/* Throws `Error("cannot require '<url>': <reason>")`, the shape a failed
   load already has, for the two things a synchronous `require` of an ES
   module cannot do at all. A URL is of no bounded length, so the message is
   built rather than formatted into a buffer. */
static JSValue qjs_throw_require_error(JSContext *raw, const char *url,
                                       const char *reason) {
    static const char PROLOGUE[] = "cannot require '";
    static const char INFIX[] = "': ";
    size_t url_length = strlen(url);
    size_t reason_length = strlen(reason);
    size_t fixed = sizeof(PROLOGUE) - 1 + sizeof(INFIX) - 1;
    size_t offset;
    char *message;
    JSValue thrown;

    if (url_length > SIZE_MAX - fixed - reason_length - 1) {
        return JS_ThrowOutOfMemory(raw);
    }
    message = malloc(fixed + url_length + reason_length + 1);
    if (message == NULL) {
        return JS_ThrowOutOfMemory(raw);
    }
    memcpy(message, PROLOGUE, sizeof(PROLOGUE) - 1);
    offset = sizeof(PROLOGUE) - 1;
    memcpy(message + offset, url, url_length);
    offset += url_length;
    memcpy(message + offset, INFIX, sizeof(INFIX) - 1);
    offset += sizeof(INFIX) - 1;
    memcpy(message + offset, reason, reason_length + 1);
    thrown = qjs_require_throw_load(raw, message);
    free(message);
    return thrown;
}

/* Parses the wrapped body under the response URL, so a `SyntaxError` and
   every frame beneath it name the file. Source text may contain interior
   NULs, so the buffer is built by length.

   Compilation only: the *script* this parses is author text, which can leave
   statements of its own outside the wrapper, and running those is
   `JS_EvalFunction`'s — which the caller reaches only once it has finished
   with everything the host lent it. */
static JSValue qjs_require_compile(JSContext *raw, const char *url,
                                   const char *parameters,
                                   const uint8_t *text, size_t text_length) {
    size_t parameters_length = strlen(parameters);
    size_t fixed = sizeof(QJS_WRAPPER_PROLOGUE) - 1 +
                   sizeof(QJS_WRAPPER_INFIX) - 1 +
                   sizeof(QJS_WRAPPER_EPILOGUE) - 1;
    size_t length;
    size_t offset;
    char *wrapped;
    JSValue compiled;

    if (text_length > SIZE_MAX - fixed - parameters_length - 1) {
        return JS_ThrowOutOfMemory(raw);
    }
    length = fixed + parameters_length + text_length;
    wrapped = malloc(length + 1);
    if (wrapped == NULL) {
        return JS_ThrowOutOfMemory(raw);
    }
    offset = 0;
    memcpy(wrapped, QJS_WRAPPER_PROLOGUE, sizeof(QJS_WRAPPER_PROLOGUE) - 1);
    offset += sizeof(QJS_WRAPPER_PROLOGUE) - 1;
    memcpy(wrapped + offset, parameters, parameters_length);
    offset += parameters_length;
    memcpy(wrapped + offset, QJS_WRAPPER_INFIX, sizeof(QJS_WRAPPER_INFIX) - 1);
    offset += sizeof(QJS_WRAPPER_INFIX) - 1;
    memcpy(wrapped + offset, text, text_length);
    offset += text_length;
    memcpy(wrapped + offset, QJS_WRAPPER_EPILOGUE,
           sizeof(QJS_WRAPPER_EPILOGUE) - 1);
    wrapped[length] = '\0';
    compiled = JS_Eval(raw, wrapped, length, url,
                       JS_EVAL_TYPE_GLOBAL | JS_EVAL_FLAG_COMPILE_ONLY);
    free(wrapped);
    return compiled;
}

static JSValue qjs_require_parse_json(JSContext *raw, const char *url,
                                      const uint8_t *text,
                                      size_t text_length) {
    char *terminated;
    JSValue parsed;

    if (text_length == SIZE_MAX) {
        return JS_ThrowOutOfMemory(raw);
    }
    terminated = malloc(text_length + 1);
    if (terminated == NULL) {
        return JS_ThrowOutOfMemory(raw);
    }
    memcpy(terminated, text, text_length);
    terminated[text_length] = '\0';
    parsed = JS_ParseJSON(raw, terminated, text_length, url);
    free(terminated);
    return parsed;
}

/* Builds one load's answer: `{ url, kind, value }`, on an object of this
   call's own. It consumes `response` and `value` whether or not it succeeds,
   as each define does.

   `response` is a string rather than the host's own buffer because a
   CommonJS file's answer is built after its body has run, and a load from
   that body is what replaces the buffer: the URL is copied into this realm
   before anything of the file runs. */
static JSValue qjs_require_answer(JSContext *raw, JSValue response,
                                  const char *kind_name, JSValue value) {
    JSValue kind = JS_UNDEFINED;
    JSValue result = JS_UNDEFINED;

    if (JS_IsException(response)) {
        JS_FreeValue(raw, value);
        return JS_EXCEPTION;
    }
    kind = JS_NewString(raw, kind_name);
    if (JS_IsException(kind)) {
        JS_FreeValue(raw, response);
        JS_FreeValue(raw, value);
        return JS_EXCEPTION;
    }
    result = JS_NewObject(raw);
    if (JS_IsException(result)) {
        JS_FreeValue(raw, response);
        JS_FreeValue(raw, kind);
        JS_FreeValue(raw, value);
        return JS_EXCEPTION;
    }
    if (JS_DefinePropertyValueStr(raw, result, "url", response,
                                  JS_PROP_C_W_E) < 0) {
        JS_FreeValue(raw, kind);
        JS_FreeValue(raw, value);
        JS_FreeValue(raw, result);
        return JS_EXCEPTION;
    }
    if (JS_DefinePropertyValueStr(raw, result, "kind", kind,
                                  JS_PROP_C_W_E) < 0) {
        JS_FreeValue(raw, value);
        JS_FreeValue(raw, result);
        return JS_EXCEPTION;
    }
    if (JS_DefinePropertyValueStr(raw, result, "value", value,
                                  JS_PROP_C_W_E) < 0) {
        JS_FreeValue(raw, result);
        return JS_EXCEPTION;
    }
    return result;
}

/* Forgets every rejection `context` recorded after `since` whose reason is
   `reason`: what one failed module evaluation left behind. QuickJS runs each
   module body as an async function, so a body that throws rejects that
   function's own promise as well as the evaluation promise, and every module
   of the graph on the failing path adds one more — all with the one thrown
   value, and none of them reachable from script. */
static void qjs_rejections_forget_since(QjsRuntime *runtime,
                                        QjsContext *context, uint64_t since,
                                        JSValueConst reason) {
    QjsUnhandledRejection *current = runtime->rejection_head;
    QjsUnhandledRejection *previous = NULL;

    while (current != NULL) {
        QjsUnhandledRejection *next = current->next;

        if (current->context == context && current->serial > since &&
            JS_StrictEq(context->raw, current->reason, reason)) {
            if (previous == NULL) {
                runtime->rejection_head = next;
            } else {
                previous->next = next;
            }
            if (runtime->rejection_tail == current) {
                runtime->rejection_tail = previous;
            }
            JS_FreeValue(context->raw, current->promise);
            JS_FreeValue(context->raw, current->reason);
            free(current);
        } else {
            previous = current;
        }
        current = next;
    }
}

/* Evaluates one module for a `require` and answers its namespace.
   `JS_EvalFunction` links the graph and runs it, and hands back the
   evaluation promise, which is already settled unless something in the graph
   awaited at its top level. The three states are the three answers:

   - fulfilled: the namespace object, which is what `require` returns;
   - rejected: whatever the body threw, thrown again here, so a `require` fails the way the module
     did. Every rejection the evaluation left with that value is forgotten first — the evaluation
     promise and the promise of each module body on the failing path: the realm has been told about
     it by the throw, and promises nothing else holds would otherwise be reported a second time as
     unhandled rejections;
   - pending: refused. Settling it means running promise jobs, and a `require` is a call inside
     whatever job is already running: it cannot run the queue it is itself part of. Node refuses
     the same thing as `ERR_REQUIRE_ASYNC_MODULE`.

   A module that got as far as being evaluated is remembered as evaluated
   whichever of the three it was: QuickJS answers a second evaluation of it
   from the first one's outcome rather than running the body again. */
static JSValue qjs_require_evaluate(QjsContext *context,
                                    QjsModuleInstance *instance,
                                    const char *url) {
    JSContext *raw = context->raw;
    JSValue promise;
    JSValue reason;
    uint64_t since = context->runtime->rejection_serial;
    int state;

    context->runtime->evaluation_depth += 1;
    promise = JS_EvalFunction(
        raw, JS_DupValue(raw, JS_MKPTR(JS_TAG_MODULE, instance->definition)));
    context->runtime->evaluation_depth -= 1;
    /* Whatever came of it, this module is past re-entry: evaluated, awaiting
       its own top-level await, or — if linking is what failed — back to
       unlinked. None of the three is a body part-way through. */
    instance->evaluated = 1;
    if (JS_IsException(promise)) {
        return JS_EXCEPTION;
    }
    state = JS_PromiseState(raw, promise);
    if (state == JS_PROMISE_REJECTED) {
        reason = JS_PromiseResult(raw, promise);
        qjs_rejections_forget_since(context->runtime, context, since, reason);
        JS_FreeValue(raw, promise);
        return JS_Throw(raw, reason);
    }
    if (state != JS_PROMISE_FULFILLED) {
        JS_FreeValue(raw, promise);
        if (state == JS_PROMISE_PENDING) {
            return qjs_throw_require_error(
                raw, url,
                "it uses top-level await, which a synchronous require cannot "
                "wait for");
        }
        return JS_ThrowInternalError(raw,
                                     "module evaluation did not answer with a "
                                     "promise");
    }
    JS_FreeValue(raw, promise);
    return JS_GetModuleNamespace(raw, instance->definition);
}

/* Answers a `require` from a module this realm already has, whichever of an
   import and an earlier `require` brought it in. It is evaluated at most
   once: QuickJS answers a second evaluation from the first one's outcome. */
static JSValue qjs_require_instance(QjsContext *context,
                                    QjsModuleInstance *instance) {
    const QjsModuleSource *source = instance->source;
    const char *url = source->url != NULL ? source->url : source->name;
    JSValue namespace;

    if (!qjs_module_is_quiet(context, instance)) {
        /* Node calls the case it can tell apart `ERR_REQUIRE_CYCLE_MODULE`;
           this realm cannot read a module's status, so it refuses every
           module of a graph that is still evaluating, not only the cycle. */
        return qjs_throw_require_error(
            context->raw, url,
            "it is part of a module graph that is still evaluating");
    }
    namespace = qjs_require_evaluate(context, instance, url);
    if (JS_IsException(namespace)) {
        return JS_EXCEPTION;
    }
    return qjs_require_answer(context->raw, JS_NewString(context->raw, url),
                              QJS_KIND_MODULE, namespace);
}

/* Compiles one loaded source as an ES module, links it inline and evaluates
   it, for a `require` that has just had its text.

   Everything the host lent is copied or compiled before any body runs: the
   text and the response URL go into this realm's own source table first, and
   the compile — linked, so every import in the graph is resolved through the
   same synchronous loader, recursively — is finished before `JS_EvalFunction`
   starts the first body. */
static JSValue qjs_require_module(QjsContext *context, const char *requested,
                                  const QjsRequiredSource *loaded) {
    JSContext *raw = context->raw;
    QjsModuleSource *source;
    QjsModuleInstance *instance;
    QjsModuleInstance *walk;
    uint64_t made_after = context->instance_serial;
    JSValue result;
    int status;

    /* -5 is this URL having been answered already, by the host's own fetch
       of an import of it: that source is this source, and it stands. */
    status = qjs_context_complete_module(context, requested, loaded->url,
                                         loaded->text, loaded->text_length,
                                         NULL);
    if (status != 0 && status != -5) {
        if (!JS_HasException(raw)) {
            JS_ThrowInternalError(raw, "module '%s' could not be completed",
                                  requested);
        }
        return JS_EXCEPTION;
    }
    source = qjs_find_local_source(context, requested);
    if (source == NULL) {
        return JS_ThrowInternalError(raw, "module '%s' was not stored",
                                     requested);
    }
    if (source->source == NULL) {
        /* This URL is already recorded as a load that failed — an import of
           it the host refused — and a source table entry is not replaced.
           The `require` fails the way that import did. */
        return qjs_require_throw_load(raw, source->error != NULL
                                               ? source->error
                                               : "the module has no source");
    }
    source->requested = 1;

    context->synchronous_link += 1;
    context->synchronous_compile += 1;
    context->link_mark = context->instance_serial;
    if (qjs_compile_module_source(context, source, 1) == NULL) {
        result = JS_EXCEPTION;
    } else {
        instance = qjs_find_module_instance(context, source);
        /* The compile is over: what it made is about to be evaluated, so
           nothing of it is a module nothing has run in any more. */
        context->synchronous_compile -= 1;
        context->link_mark = context->instance_serial;
        result = instance == NULL
                     ? JS_ThrowInternalError(raw, "module '%s' was not compiled",
                                             requested)
                     : qjs_require_evaluate(context, instance, source->url);
        context->synchronous_compile += 1;
        if (!JS_IsException(result)) {
            /* The whole graph ran, so every module this link compiled is
               evaluated, not only the one that was asked for. */
            for (walk = context->module_instances;
                 walk != NULL && walk->serial > made_after; walk = walk->next) {
                walk->evaluated = 1;
            }
            result = qjs_require_answer(raw, JS_NewString(raw, source->url),
                                        QJS_KIND_MODULE, result);
        }
    }
    context->synchronous_compile -= 1;
    context->synchronous_link -= 1;
    /* The mark never goes back: every instance this link made has been
       through an evaluation, so none of them is one nothing has run in. */
    context->link_mark = context->instance_serial;
    return result;
}

/* `loadModuleSync(url, parameters)`: one synchronous load, compiled.
   Answers `{ url, kind, value }` — the URL the host answered from, which of
   the three shapes it was read as, and the wrapper function of a CommonJS
   file, the parsed value of a JSON one, or the namespace object of an ES
   module. No resolution, no cache, no module object: `bobcat:module` is
   where Node's algorithm lives, and this is the one step of it that needs
   the engine.

   The order of what it does is the whole remaining lifetime invariant. The
   host keeps owning the buffers it filled in, and lends them only until the
   next load: so the response URL is copied into a string and the text is
   compiled, parsed or copied into this realm's source table *before*
   anything of the file runs — for a CommonJS file that is `JS_EvalFunction`,
   since text closing the wrapper early leaves statements in the enclosing
   script, and for a module it is the same call, after a compile that has
   already loaded every import in the graph. Nothing borrowed is read past
   that point. */
static JSValue qjs_load_module_sync(JSContext *raw, JSValueConst this_value,
                                    int argc, JSValueConst *argv) {
    QjsContext *context = JS_GetContextOpaque(raw);
    QjsRequiredSource loaded;
    const QjsModuleSource *source;
    QjsModuleInstance *instance;
    const char *url;
    const char *parameters;
    char *terminated = NULL;
    JSValue response = JS_UNDEFINED;
    JSValue value = JS_UNDEFINED;
    JSValue result = JS_UNDEFINED;
    int kind;
    int failed;

    (void)this_value;
    if (context == NULL) {
        return JS_ThrowInternalError(raw, "this realm is being released");
    }
    /* Both are required to be strings rather than converted: a conversion
       would run author code, and this entry's caller is a built-in. */
    if (argc < 2 || !JS_IsString(argv[0]) || !JS_IsString(argv[1])) {
        return JS_ThrowTypeError(
            raw, "loadModuleSync expects a URL and a parameter list");
    }
    url = JS_ToCString(raw, argv[0]);
    if (url == NULL) {
        return JS_EXCEPTION;
    }
    /* A module this realm already has is answered from its instance rather
       than loaded again: one URL is one module, whichever of an import and a
       `require` reached it first, and it runs once. */
    source = qjs_find_local_source(context, url);
    if (source == NULL) {
        source = qjs_find_module_source(context->runtime, url);
    }
    instance = source == NULL ? NULL
                              : qjs_find_module_instance(context, source);
    if (instance != NULL) {
        JS_FreeCString(raw, url);
        return qjs_require_instance(context, instance);
    }
    /* The loader's rule for a reserved name, for the one load that does not
       go through the loader: a name this realm has no instance of is not
       the host's to answer. */
    if (qjs_module_name_is_reserved(context->runtime, url)) {
        JS_ThrowReferenceError(raw, "module '%s' is not preloaded", url);
        JS_FreeCString(raw, url);
        return JS_EXCEPTION;
    }
    parameters = JS_ToCString(raw, argv[1]);
    if (parameters == NULL) {
        JS_FreeCString(raw, url);
        return JS_EXCEPTION;
    }
    loaded.url = NULL;
    loaded.text = NULL;
    loaded.text_length = 0;
    loaded.kind = QJS_REQUIRED_COMMONJS;
    loaded.error = NULL;
    failed = context->require_load(context->require_opaque, url, &loaded) != 0;
    if (failed) {
        JS_FreeCString(raw, url);
        JS_FreeCString(raw, parameters);
        return qjs_require_throw_load(raw, loaded.error);
    }
    kind = loaded.kind;
    if (kind == QJS_REQUIRED_DETECT) {
        /* The host has no text, so it cannot decide; QuickJS's detector
           wants a terminated buffer, and the text the host lends is not one.
           The copy it needs is the copy the load needs anyway. */
        if (loaded.text_length == SIZE_MAX) {
            JS_FreeCString(raw, url);
            JS_FreeCString(raw, parameters);
            return JS_ThrowOutOfMemory(raw);
        }
        terminated = malloc(loaded.text_length + 1);
        if (terminated == NULL) {
            JS_FreeCString(raw, url);
            JS_FreeCString(raw, parameters);
            return JS_ThrowOutOfMemory(raw);
        }
        memcpy(terminated, loaded.text, loaded.text_length);
        terminated[loaded.text_length] = '\0';
        kind = JS_DetectModule(terminated, loaded.text_length)
                   ? QJS_REQUIRED_MODULE
                   : QJS_REQUIRED_COMMONJS;
        loaded.text = (const uint8_t *)terminated;
    }
    if (kind == QJS_REQUIRED_MODULE) {
        /* `parameters` is a CommonJS wrapper's, and a module has none. The
           URL asked for is what the module is stored under, because that is
           the name an import of it resolves to. */
        JS_FreeCString(raw, parameters);
        result = qjs_require_module(context, url, &loaded);
        JS_FreeCString(raw, url);
        free(terminated);
        return result;
    }
    JS_FreeCString(raw, url);
    response = JS_NewString(raw, loaded.url);
    if (JS_IsException(response)) {
        JS_FreeCString(raw, parameters);
        free(terminated);
        return JS_EXCEPTION;
    }
    value = kind == QJS_REQUIRED_JSON
                ? qjs_require_parse_json(raw, loaded.url, loaded.text,
                                         loaded.text_length)
                : qjs_require_compile(raw, loaded.url, parameters, loaded.text,
                                      loaded.text_length);
    JS_FreeCString(raw, parameters);
    free(terminated);
    if (JS_IsException(value)) {
        JS_FreeValue(raw, response);
        return JS_EXCEPTION;
    }
    if (kind != QJS_REQUIRED_JSON) {
        /* Nothing the host lent is read past here. */
        value = JS_EvalFunction(raw, value);
        if (JS_IsException(value)) {
            JS_FreeValue(raw, response);
            return JS_EXCEPTION;
        }
    }
    return qjs_require_answer(raw, response,
                              kind == QJS_REQUIRED_JSON ? QJS_KIND_JSON
                                                        : QJS_KIND_COMMONJS,
                              value);
}

/* A realm takes one loader, and the export is what carries it: a second
   registration under any name would leave the first export answering from the
   second host, so it is refused (-6) before anything is built. */
int qjs_context_register_synchronous_loader(QjsContext *context,
                                           const char *name,
                                           const char *export_name,
                                           QjsRequireLoad *load,
                                           void *opaque) {
    JSValue function;
    int status;

    if (context->require_load != NULL) {
        return -6;
    }
    function =
        JS_NewCFunction(context->raw, qjs_load_module_sync, export_name, 2);
    if (JS_IsException(function)) {
        return -1;
    }
    status = qjs_add_host_module_export(context, name, export_name, function);
    JS_FreeValue(context->raw, function);
    if (status == 0) {
        context->require_load = load;
        context->require_opaque = opaque;
    }
    return status;
}

/* Borrowed until context destruction; taking a request marks it dispatched. */
const char *qjs_context_take_module_request(QjsContext *context) {
    QjsModuleSource *source;
    for (source = context->module_sources; source != NULL; source = source->next) {
        if (source->source == NULL && source->error == NULL && !source->requested) {
            source->requested = 1;
            return source->name;
        }
    }
    return NULL;
}

int qjs_context_complete_module(QjsContext *context, const char *name,
                                const char *url, const uint8_t *text,
                                size_t length, const char *error) {
    QjsModuleSource *source = qjs_find_local_source(context, name);
    if (qjs_find_module_source(context->runtime, name) != NULL ||
        qjs_find_host_module_name(context->runtime, name) != NULL)
        return -2;
    /* Already answered. A `require` linking this name inline answers it from
       the same host, out of band of the request the host took, so the fetch
       that was already running finds the source it was going to supply
       standing: nothing is replaced, and nothing is wrong. */
    if (source != NULL && (source->source != NULL || source->error != NULL))
        return -5;
    if (source == NULL) {
        source = calloc(1, sizeof(*source));
        if (source == NULL)
            goto oom;
        source->name = qjs_strdup(name);
        if (source->name == NULL) {
            free(source);
            goto oom;
        }
        source->next = context->module_sources;
        context->module_sources = source;
    }
    if (error != NULL) {
        source->error = qjs_strdup(error);
        if (source->error == NULL)
            goto oom;
    } else {
        if (length == SIZE_MAX)
            goto oom;
        source->source = malloc(length + 1);
        source->url = qjs_strdup(url);
        if (source->source == NULL || source->url == NULL) {
            free(source->source);
            free(source->url);
            source->source = NULL;
            source->url = NULL;
            goto oom;
        }
        memcpy(source->source, text, length);
        source->source[length] = 0;
        source->source_length = length;
    }
    return 0;
 oom:
    JS_ThrowOutOfMemory(context->raw);
    return -1;
}

void qjs_context_resume_module_loads(QjsContext *context) {
    QjsDeferredImport *pending = context->deferred_imports;
    context->deferred_imports = NULL;
    /* A resumed import links and evaluates its graph, so module bodies run
       under this call: what they can reach is what `qjs_module_is_quiet`
       refuses to evaluate a second time. */
    context->runtime->evaluation_depth += 1;
    while (pending != NULL) {
        QjsDeferredImport *next = pending->next;
        JS_ResumeModuleLoad(context->raw, pending->base, pending->name,
                            (JSValueConst *)pending->resolve, pending->attributes);
        qjs_deferred_import_free(context->raw, pending);
        pending = next;
    }
    context->runtime->evaluation_depth -= 1;
}

/* Loads one module as the root of a graph, the way an `import()` of it
   would, and answers the promise QuickJS settles with its namespace.

   The name is its own base, so it normalizes from itself. A graph whose
   every source this realm already has is linked and evaluated under this
   call, which is why the call raises the evaluation depth as a resumed
   import does. A source it lacks, the root's own included, is treated as
   any import's is: in a realm with module loading enabled it becomes a
   request, and resuming the loads once it has been completed is what goes
   on to link and evaluate the graph; in any other realm the loader refuses
   it as not preloaded, which rejects the load. */
QjsValue *qjs_context_load_module(QjsContext *context, const char *name) {
    JSValue promise;

    context->runtime->evaluation_depth += 1;
    promise = JS_LoadModule(context->raw, name, name);
    context->runtime->evaluation_depth -= 1;
    return qjs_box(context->raw, promise);
}

void qjs_runtime_run_gc(QjsRuntime *runtime) {
    JS_RunGC(runtime->raw);
}

void qjs_runtime_compute_memory_usage(QjsRuntime *runtime, JSMemoryUsage *usage) {
    JS_ComputeMemoryUsage(runtime->raw, usage);
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

QjsValue *qjs_new_undefined(QjsContext *context) {
    return qjs_box(context->raw, JS_UNDEFINED);
}

QjsValue *qjs_new_null(QjsContext *context) {
    return qjs_box(context->raw, JS_NULL);
}

QjsValue *qjs_new_boolean(QjsContext *context, int value) {
    return qjs_box(context->raw, JS_NewBool(context->raw, value != 0));
}

QjsValue *qjs_new_number(QjsContext *context, double value) {
    return qjs_box(context->raw, JS_NewFloat64(context->raw, value));
}

QjsValue *qjs_new_big_int64(QjsContext *context, int64_t value) {
    return qjs_box(context->raw, JS_NewBigInt64(context->raw, value));
}

QjsValue *qjs_new_big_uint64(QjsContext *context, uint64_t value) {
    return qjs_box(context->raw, JS_NewBigUint64(context->raw, value));
}


/* The escape hatch for ill-formed UTF-16: text containing an unpaired
   surrogate has no UTF-8 spelling, and `JS_NewStringLen` would replace the
   surrogate with U+FFFD. `JS_ParseJSON` is the one entry point that accepts a
   lone `\uD800` escape and preserves it, at the cost of four passes and three
   allocations. Well-formed text never reaches here: it goes straight through
   `qjs_new_string_utf8`. */
static JSValue qjs_string_from_utf16(JSContext *raw_context,
                                     const uint16_t *units, size_t length) {
    static const char hex[] = "0123456789abcdef";
    char *json;
    JSValue parsed;
    size_t index;
    size_t offset = 0;

    if (length > (SIZE_MAX - 3) / 6) {
        return JS_ThrowOutOfMemory(raw_context);
    }
    json = malloc(length * 6 + 3);
    if (json == NULL) {
        return JS_ThrowOutOfMemory(raw_context);
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
    parsed = JS_ParseJSON(raw_context, json, offset, "<host string>");
    free(json);
    return parsed;
}

QjsValue *qjs_new_string_utf16(QjsContext *context, const uint16_t *units,
                               size_t length) {
    return qjs_box(context->raw,
                   qjs_string_from_utf16(context->raw, units, length));
}

/* Well-formed UTF-8 straight into QuickJS's own decoder, which has an ASCII
   fast path that memcpy's into a Latin-1 string. Every Rust `str` qualifies,
   so this is the only construction path host text normally takes. */
QjsValue *qjs_new_string_utf8(QjsContext *context, const uint8_t *bytes,
                              size_t length) {
    return qjs_box(context->raw,
                   JS_NewStringLen(context->raw, (const char *)bytes, length));
}

/* Atoms live on the runtime, not the context: `JS_NewAtomLen` interns into
   `ctx->rt`, so a name interned through one realm resolves a property in
   every realm on that runtime, and only the runtime is needed to release it. */
uint32_t qjs_atom_new(QjsContext *context, const uint8_t *bytes,
                      size_t length) {
    return (uint32_t)JS_NewAtomLen(context->raw, (const char *)bytes, length);
}

void qjs_atom_free(QjsRuntime *runtime, uint32_t atom) {
    JS_FreeAtomRT(runtime->raw, (JSAtom)atom);
}

void qjs_value_free(QjsContext *context, QjsValue *value) {
    if (value != NULL) {
        JS_FreeValue(context->raw, value->value);
        free(value);
    }
}

int qjs_value_kind(QjsContext *context, const QjsValue *value) {
    JSValueConst raw = value->value;

    if (JS_IsUndefined(raw)) return QJS_KIND_UNDEFINED;
    if (JS_IsNull(raw)) return QJS_KIND_NULL;
    if (JS_IsBool(raw)) return QJS_KIND_BOOLEAN;
    if (JS_IsNumber(raw)) return QJS_KIND_NUMBER;
    if (JS_IsBigInt(context->raw, raw)) return QJS_KIND_BIG_INT;
    if (JS_IsString(raw)) return QJS_KIND_STRING;
    if (JS_IsSymbol(raw)) return QJS_KIND_SYMBOL;
    if (JS_IsFunction(context->raw, raw)) return QJS_KIND_FUNCTION;
    if (JS_IsObject(raw)) return QJS_KIND_OBJECT;
    return QJS_KIND_OTHER;
}

int qjs_value_get_boolean(QjsContext *context, const QjsValue *value,
                          int *result) {
    int converted = JS_ToBool(context->raw, value->value);
    if (converted < 0) return -1;
    *result = converted;
    return 0;
}

int qjs_value_get_number(QjsContext *context, const QjsValue *value,
                         double *result) {
    return JS_ToFloat64(context->raw, result, value->value);
}

int qjs_value_promise_state(QjsContext *context, const QjsValue *value) {
    return JS_PromiseState(context->raw, value->value);
}

QjsValue *qjs_value_promise_result(QjsContext *context,
                                   const QjsValue *value) {
    return qjs_box(context->raw, JS_PromiseResult(context->raw, value->value));
}

int qjs_value_to_cesu8(QjsContext *context, const QjsValue *value,
                       const uint8_t **bytes, size_t *length) {
    const char *converted =
        JS_ToCStringLen2(context->raw, length, value->value, 1);
    if (converted == NULL) return -1;
    *bytes = (const uint8_t *)converted;
    return 0;
}

void qjs_cesu8_free(QjsContext *context, const uint8_t *bytes) {
    JS_FreeCString(context->raw, (const char *)bytes);
}

QjsValue *qjs_eval(QjsContext *context, const uint8_t *source,
                   size_t source_length, const char *source_name, int flags,
                   int *failure_stage) {
    JSValue compiled;
    JSValue result;
    int is_module = (flags & JS_EVAL_TYPE_MASK) == JS_EVAL_TYPE_MODULE;

    *failure_stage = QJS_EVAL_FAILURE_NONE;
    compiled = JS_Eval(context->raw, (const char *)source, source_length,
                       source_name, flags | JS_EVAL_FLAG_COMPILE_ONLY);
    if (JS_IsException(compiled)) {
        *failure_stage = QJS_EVAL_FAILURE_COMPILE;
        return NULL;
    }
    /* A module evaluated directly is known by the source name it was given,
       which is the only name this realm has for it. */
    if (is_module &&
        qjs_set_import_meta_url(context->raw, JS_VALUE_GET_PTR(compiled),
                                source_name) < 0) {
        JS_FreeValue(context->raw, compiled);
        return NULL;
    }
    /* Evaluating a module runs its graph's bodies; a plain script's own
       statements cannot start one, since an `import()` in it defers. */
    context->runtime->evaluation_depth += is_module;
    result = JS_EvalFunction(context->raw, compiled);
    context->runtime->evaluation_depth -= is_module;
    if (JS_IsException(result)) {
        *failure_stage = QJS_EVAL_FAILURE_EXECUTE;
    }
    return qjs_box(context->raw, result);
}

QjsValue *qjs_call(QjsContext *context, const QjsValue *callable,
                   const QjsValue *this_value, size_t argument_count,
                   const QjsValue *const *arguments) {
    JSValue *raw_arguments = NULL;
    JSValue result;
    size_t index;

    if (argument_count > INT_MAX ||
        argument_count > SIZE_MAX / sizeof(*raw_arguments)) {
        JS_ThrowRangeError(context->raw, "too many call arguments");
        return NULL;
    }
    if (argument_count > 0) {
        raw_arguments = malloc(argument_count * sizeof(*raw_arguments));
        if (raw_arguments == NULL) {
            JS_ThrowOutOfMemory(context->raw);
            return NULL;
        }
        for (index = 0; index < argument_count; ++index) {
            raw_arguments[index] = arguments[index]->value;
        }
    }

    result = JS_Call(context->raw, callable->value,
                     this_value == NULL ? JS_UNDEFINED : this_value->value,
                     (int)argument_count, raw_arguments);
    free(raw_arguments);
    return qjs_box(context->raw, result);
}

/* Runs one job from the runtime's queue, whichever realm queued it, and
   reports the realm it ran in. That realm is NULL when the job was the last
   thing keeping a released context alive. */
int qjs_execute_pending_job(QjsRuntime *runtime, QjsContext **context) {
    JSContext *raw_context = NULL;
    int status;

    /* A job may be what a module's top-level await was suspended on, which
       runs the rest of that module's body and of the graph waiting on it. */
    runtime->evaluation_depth += 1;
    status = JS_ExecutePendingJob(runtime->raw, &raw_context);
    runtime->evaluation_depth -= 1;
    *context = raw_context == NULL ? NULL : JS_GetContextOpaque(raw_context);
    return status;
}

int qjs_has_pending_job(QjsRuntime *runtime) {
    return JS_IsJobPending(runtime->raw);
}

/* Takes the oldest unhandled rejection `realm` left behind, leaving every
   other realm's queued rejections in place for that realm's own next
   checkpoint. A runtime's realms share one job queue but not one another's
   failures. The tracker's own out-of-memory flag is the runtime's, not a
   realm's, so whichever realm asks first reports it. */
int qjs_take_unhandled_rejection(QjsRuntime *runtime, QjsContext *realm,
                                 QjsContext **context, QjsValue **value) {
    QjsUnhandledRejection *rejection = runtime->rejection_head;
    QjsUnhandledRejection *previous = NULL;

    *context = NULL;
    *value = NULL;
    while (rejection != NULL && rejection->context != realm) {
        previous = rejection;
        rejection = rejection->next;
    }
    if (rejection == NULL) {
        if (!runtime->rejection_tracker_oom) {
            return QJS_REJECTION_NONE;
        }
        runtime->rejection_tracker_oom = 0;
        return QJS_REJECTION_TRACKER_OOM;
    }
    if (previous == NULL) {
        runtime->rejection_head = rejection->next;
    } else {
        previous->next = rejection->next;
    }
    if (runtime->rejection_tail == rejection) {
        runtime->rejection_tail = previous;
    }
    JS_FreeValue(rejection->context->raw, rejection->promise);
    *context = rejection->context;
    *value = qjs_box(rejection->context->raw, rejection->reason);
    free(rejection);
    return QJS_REJECTION_TAKEN;
}

QjsValue *qjs_take_exception(QjsContext *context) {
    JSValue exception = JS_GetException(context->raw);
    QjsValue *boxed = malloc(sizeof(*boxed));

    if (boxed == NULL) {
        JS_FreeValue(context->raw, exception);
        return NULL;
    }
    boxed->value = exception;
    return boxed;
}

void qjs_discard_exception(QjsContext *context) {
    JSValue exception = JS_GetException(context->raw);
    JS_FreeValue(context->raw, exception);
}

QjsValue *qjs_get_property(QjsContext *context, const QjsValue *value,
                           const char *name) {
    return qjs_box(context->raw,
                   JS_GetPropertyStr(context->raw, value->value, name));
}

int qjs_set_property(QjsContext *context, const QjsValue *target,
                     const char *name, const QjsValue *value) {

    return JS_SetPropertyStr(context->raw, target->value, name,
                             JS_DupValue(context->raw, value->value));
}

QjsValue *qjs_global_object(QjsContext *context) {
    return qjs_box(context->raw, JS_GetGlobalObject(context->raw));
}

void qjs_throw_error(QjsContext *context, const char *message) {
    JS_ThrowInternalError(context->raw, "%s", message);
}

void qjs_runtime_set_host_dispatch(QjsRuntime *runtime,
                                   QjsHostDispatch *dispatch,
                                   QjsHostRelease *release, void *opaque) {
    runtime->host_dispatch = dispatch;
    runtime->host_release = release;
    runtime->host_opaque = opaque;
}

#define QJS_HOST_INLINE_ARGS 8
/* The JSValues `qjs_call_member` builds on the stack before it spills to the
   heap: the Rust side's `CALL_INLINE_ARGS`, sized for the engine's event
   dispatch, which takes two arguments per path step. */
#define QJS_CALL_INLINE_ARGS 32

static void qjs_host_describe(JSContext *raw_context, JSValueConst value,
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
        slot->number = JS_ToBool(raw_context, value) ? 1.0 : 0.0;
    } else if (JS_IsNumber(value)) {
        double number;
        if (JS_ToFloat64(raw_context, &number, value) < 0) {
            slot->kind = QJS_ARG_UNSUPPORTED;
            return;
        }
        slot->kind = QJS_ARG_NUMBER;
        slot->number = number;
    } else if (JS_IsString(value)) {
        size_t length;
        /* UTF-8, not CESU-8: a surrogate pair is one four-byte sequence, so
           the host borrows these bytes as they are and only a lone
           surrogate's bytes are not UTF-8. */
        const char *text = JS_ToCStringLen2(raw_context, &length, value, 0);
        if (text == NULL) {
            slot->kind = QJS_ARG_UNSUPPORTED;
            return;
        }
        slot->kind = QJS_ARG_STRING;
        slot->text = (const uint8_t *)text;
        slot->text_len = length;
    } else {
        /* Everything that is not a primitive — an object, a BigInt, a Symbol
           — crosses as QuickJS's own serialization rather than not at all.
           Never `JS_WRITE_OBJ_BYTECODE`, so the stream writes every atom as a
           string and is self-contained across the runtimes of a group; never
           SAB, because this bridge exposes no shared memory. A refusal is the
           serializer's own exception, left pending for the trampoline. */
        size_t length = 0;
        uint8_t *bytes =
            JS_WriteObject(raw_context, &length, value, JS_WRITE_OBJ_REFERENCE);
        if (bytes == NULL) {
            slot->kind = QJS_ARG_FAILED;
            return;
        }
        slot->kind = QJS_ARG_STRUCTURED;
        slot->text = bytes;
        slot->text_len = length;
    }
}

/* Releases whatever one described argument holds. Both kinds that carry bytes
   own them: a string is `JS_ToCStringLen2`'s, a structured clone is
   `JS_WriteObject`'s. */
static void qjs_host_release_arg(JSContext *raw_context, QjsHostArg *slot) {
    if (slot->kind == QJS_ARG_STRING) {
        JS_FreeCString(raw_context, (const char *)slot->text);
    } else if (slot->kind == QJS_ARG_STRUCTURED) {
        js_free(raw_context, (void *)slot->text);
    }
    slot->text = NULL;
    slot->text_len = 0;
}

static JSValue qjs_host_build(JSContext *raw_context,
                              const QjsHostResult *result) {
    switch (result->kind) {
    case QJS_ARG_UNDEFINED:
        return JS_UNDEFINED;
    case QJS_ARG_NULL:
        return JS_NULL;
    case QJS_ARG_BOOLEAN:
        return JS_NewBool(raw_context, result->number != 0.0);
    case QJS_ARG_NUMBER:
        return JS_NewFloat64(raw_context, result->number);
    case QJS_ARG_STRING:
        return JS_NewStringLen(raw_context, (const char *)result->text,
                               result->text_len);
    case QJS_ARG_STRUCTURED:
        /* Rebuilds the value in this realm. May throw — a truncated or
           foreign stream — which the callers already treat as any other
           exception from building an argument or a return value. */
        return JS_ReadObject(raw_context, result->text, result->text_len,
                             JS_READ_OBJ_REFERENCE);
    default:
        return JS_ThrowInternalError(raw_context, "invalid host return value");
    }
}

/* One crossing for a whole host->realm call: the arguments arrive as the
   boundary's own primitive descriptors and become JSValues in a stack array,
   so a call costs no per-argument heap box on either side of the ABI.

   Returns 0 when the member ran (`*result` owns the returned value), 1 when
   the target has no callable under that atom, and -1 with a pending exception
   otherwise. */
int qjs_call_member(QjsContext *context, const QjsValue *target, uint32_t atom,
                    size_t argument_count, const QjsHostArg *arguments,
                    QjsValue **result) {
    JSContext *raw_context = context->raw;
    JSValue inline_argv[QJS_CALL_INLINE_ARGS];
    JSValue *argv = inline_argv;
    JSValue member;
    JSValue returned;
    size_t built;
    int status = 0;

    *result = NULL;
    member = JS_GetProperty(raw_context, target->value, (JSAtom)atom);
    if (JS_IsException(member)) {
        return -1;
    }
    if (!JS_IsFunction(raw_context, member)) {
        JS_FreeValue(raw_context, member);
        return 1;
    }
    if (argument_count > INT_MAX ||
        argument_count > SIZE_MAX / sizeof(*argv)) {
        JS_FreeValue(raw_context, member);
        JS_ThrowRangeError(raw_context, "too many call arguments");
        return -1;
    }
    if (argument_count > QJS_CALL_INLINE_ARGS) {
        argv = malloc(argument_count * sizeof(*argv));
        if (argv == NULL) {
            JS_FreeValue(raw_context, member);
            JS_ThrowOutOfMemory(raw_context);
            return -1;
        }
    }
    for (built = 0; built < argument_count; ++built) {
        argv[built] = qjs_host_build(raw_context, &arguments[built]);
        if (JS_IsException(argv[built])) {
            status = -1;
            break;
        }
    }
    if (status == 0) {
        returned = JS_Call(raw_context, member, JS_UNDEFINED,
                           (int)argument_count, argv);
        /* `qjs_box` turns an exception into NULL and leaves it pending, and
           throws OOM itself if the box cannot be allocated. */
        *result = qjs_box(raw_context, returned);
        if (*result == NULL) {
            status = -1;
        }
    }
    while (built > 0) {
        JS_FreeValue(raw_context, argv[--built]);
    }
    if (argv != inline_argv) {
        free(argv);
    }
    JS_FreeValue(raw_context, member);
    return status;
}


static JSValue qjs_host_trampoline(JSContext *raw_context,
                                   JSValueConst this_value, int argc,
                                   JSValueConst *argv, int magic,
                                   JSValue *func_data) {
    QjsContext *context = JS_GetContextOpaque(raw_context);
    QjsRuntime *runtime;
    QjsHostArg inline_arguments[QJS_HOST_INLINE_ARGS];
    QjsHostArg *arguments = inline_arguments;
    QjsHostOwner *owner;
    QjsHostResult result;
    JSValue returned;
    void *handler;
    int status;
    int count;
    int described;
    int refused = 0;

    (void)this_value;
    (void)magic;
    if (context == NULL) {
        return JS_ThrowInternalError(raw_context, "this realm is being released");
    }
    runtime = context->runtime;
    if (runtime->host_dispatch == NULL) {
        return JS_ThrowInternalError(raw_context, "no host dispatch is installed");
    }
    owner = JS_GetOpaque(func_data[0], runtime->host_owner_class_id);
    if (owner == NULL || owner->handler == NULL) {
        return JS_ThrowInternalError(raw_context, "this host function was released");
    }
    handler = owner->handler;

    if (argc > QJS_HOST_INLINE_ARGS) {
        if ((size_t)argc > SIZE_MAX / sizeof(*arguments)) {
            return JS_ThrowRangeError(raw_context, "too many call arguments");
        }
        arguments = malloc((size_t)argc * sizeof(*arguments));
        if (arguments == NULL) {
            JS_ThrowOutOfMemory(raw_context);
            return JS_EXCEPTION;
        }
    }
    /* An argument the serializer refuses ends the call before the host sees
       any of it: the exception it already threw is what reaches the JS caller,
       so `postMessage(function(){})` is a `TypeError` at the call rather than
       a host-side refusal wearing a different message. Describing stops at
       that argument, so the exception the caller sees is the *first* refusal
       rather than whichever argument threw last. `described` is then how many
       slots hold anything to release. */
    for (described = 0; described < argc; ++described) {
        qjs_host_describe(raw_context, argv[described], &arguments[described]);
        if (arguments[described].kind == QJS_ARG_FAILED) {
            refused = 1;
            ++described;
            break;
        }
    }
    if (refused) {
        for (count = 0; count < described; ++count) {
            qjs_host_release_arg(raw_context, &arguments[count]);
        }
        if (arguments != inline_arguments) {
            free(arguments);
        }
        return JS_EXCEPTION;
    }

    result.kind = QJS_ARG_UNDEFINED;
    result.number = 0.0;
    result.text = NULL;
    result.text_len = 0;
    status = runtime->host_dispatch(runtime->host_opaque, context, handler,
                                    (size_t)argc, arguments, &result);

    for (count = 0; count < argc; ++count) {
        qjs_host_release_arg(raw_context, &arguments[count]);
    }
    if (arguments != inline_arguments) {
        free(arguments);
    }

    if (status != 0) {

        return JS_EXCEPTION;
    }
    returned = qjs_host_build(raw_context, &result);
    return returned;
}


QjsValue *qjs_new_host_function(QjsContext *context, const char *name,
                                int length, void *handler) {
    JSContext *raw_context = context->raw;
    QjsHostOwner *owner;
    JSValue data;
    JSValue function;
    JSValue function_name;

    owner = malloc(sizeof(*owner));
    if (owner == NULL) {
        JS_ThrowOutOfMemory(raw_context);
        return NULL;
    }
    owner->runtime = context->runtime;
    owner->handler = handler;

    data = JS_NewObjectClass(raw_context,
                             (int)context->runtime->host_owner_class_id);
    if (JS_IsException(data)) {
        free(owner);
        return NULL;
    }
    JS_SetOpaque(data, owner);

    function = JS_NewCFunctionData(raw_context, qjs_host_trampoline, length,
                                   0, 1, &data);
    if (JS_IsException(function)) {
        owner->handler = NULL;
        JS_FreeValue(raw_context, data);
        return NULL;
    }
    JS_FreeValue(raw_context, data);


    function_name = JS_NewString(raw_context, name);
    if (JS_IsException(function_name)) {
        owner->handler = NULL;
        JS_FreeValue(raw_context, function);
        return NULL;
    }
    if (JS_DefinePropertyValueStr(raw_context, function, "name", function_name,
                                  JS_PROP_CONFIGURABLE) < 0) {
        owner->handler = NULL;
        JS_FreeValue(raw_context, function);
        return NULL;
    }
    return qjs_box(raw_context, function);
}


/* ShadowRealm (TC39, stage 2.7): a child realm on the same runtime, whose
   values reach its parent only as primitives and as wrapped functions — a
   function of one realm that is a callable of the other, whose arguments,
   receiver and result are wrapped the same way on every call.

   The host owns every child: `new ShadowRealm()` asks it to adopt one through
   the realm's `QjsShadowRealmInit`, and the child lives for as long as the
   host keeps it. JavaScript holds only tokens (`QjsRealmToken`), so a realm
   the host has let go of is a `TypeError` at every boundary instead of
   memory something still points at.

   Two engine facts shape all of it. A C-function-data callback runs in its
   *caller's* `JSContext` — only `JS_CLASS_C_FUNCTION` switches to the realm
   it was made in — so a wrapped function carries its target realm in its
   data and every call into a realm uses that realm's own `JSContext`. And
   every boundary error is made in the realm whose code reached the boundary,
   which is the callback's own `ctx`; the `importValue` continuations, which
   run in the child, are the exception and make theirs in the realm that
   asked. */

static QjsRuntime *qjs_runtime_of(JSContext *raw) {
    return JS_GetRuntimeOpaque(JS_GetRuntime(raw));
}

/* Asks the host's own interrupt state whether the running entry has been
   interrupted. QuickJS has no public getter for an exception being
   uncatchable, and `JS_Throw` clears the flag, so a boundary that replaces an
   exception asks this instead; the host's answer stays set for as long as
   its entry runs. */
static int qjs_runtime_is_interrupted(QjsRuntime *runtime) {
    return runtime->interrupt_callback != NULL &&
           runtime->interrupt_callback(runtime->interrupt_opaque) != 0;
}

/* What QuickJS's own interrupt throws, so an interrupt that crossed a
   boundary stays one no `catch` on the far side sees. */
static JSValue qjs_throw_interrupted(JSContext *realm) {
    JS_ThrowInternalError(realm, "interrupted");
    JS_SetUncatchableException(realm, 1);
    return JS_EXCEPTION;
}

static const char QJS_BOUNDARY_MESSAGE[] =
    "a value thrown across a ShadowRealm boundary";

/* Builds, without throwing, a `TypeError` (or `SyntaxError`) of `realm`
   whose message is the string `message`, which it consumes. The message is
   defined rather than formatted: `JS_Throw*Error` truncates at 256 bytes,
   and a copied message is the original string as it stands. */
static JSValue qjs_error_with_message(JSContext *realm, int syntax,
                                      JSValue message) {
    JSValue error;

    if (JS_IsException(message)) {
        return JS_EXCEPTION;
    }
    if (syntax) {
        JS_ThrowSyntaxError(realm, "%s", "");
    } else {
        JS_ThrowTypeError(realm, "%s", "");
    }
    error = JS_GetException(realm);
    if (JS_DefinePropertyValueStr(realm, error, "message", message,
                                  JS_PROP_WRITABLE | JS_PROP_CONFIGURABLE) <
        0) {
        JS_FreeValue(realm, error);
        return JS_EXCEPTION;
    }
    return error;
}

static JSValue qjs_throw_with_message(JSContext *realm, int syntax,
                                      JSValue message) {
    JSValue error = qjs_error_with_message(realm, syntax, message);

    if (JS_IsException(error)) {
        return JS_EXCEPTION;
    }
    return JS_Throw(realm, error);
}

static JSValue qjs_throw_released(JSContext *realm) {
    return JS_ThrowTypeError(realm,
                             "this ShadowRealm's realm has been released");
}

/* The error a value thrown in one realm becomes in `realm`: a fresh
   `TypeError` (a `SyntaxError` for source that did not parse), never the
   thrown value itself. Its message is the original's when the original is an
   `Error` whose own `message` is a data property holding a string, which is
   read without invoking anything; otherwise a fixed one, so a thrown plain
   object or a getter exposes nothing. */
static JSValue qjs_boundary_error(JSContext *realm, JSValueConst original,
                                  int syntax) {
    JSValue message = JS_UNDEFINED;

    if (JS_IsError(realm, original)) {
        JSPropertyDescriptor descriptor;
        JSAtom name = JS_NewAtom(realm, "message");
        int found;

        if (name == JS_ATOM_NULL) {
            return JS_EXCEPTION;
        }
        /* An `Error` is an ordinary object, so this runs no trap. */
        found = JS_GetOwnProperty(realm, &descriptor, original, name);
        JS_FreeAtom(realm, name);
        if (found < 0) {
            return JS_EXCEPTION;
        }
        if (found > 0) {
            if (!(descriptor.flags & JS_PROP_GETSET) &&
                JS_IsString(descriptor.value)) {
                message = JS_DupValue(realm, descriptor.value);
            }
            JS_FreeValue(realm, descriptor.value);
            JS_FreeValue(realm, descriptor.getter);
            JS_FreeValue(realm, descriptor.setter);
        }
    }
    if (JS_IsUndefined(message)) {
        message = JS_NewString(realm, QJS_BOUNDARY_MESSAGE);
    }
    return qjs_error_with_message(realm, syntax, message);
}

/* Replaces the pending exception, which code of another realm threw, with
   its boundary error in `realm`. The original never reaches `realm`'s code.
   An interrupt is thrown again as an interrupt. */
static JSValue qjs_boundary_rethrow(QjsRuntime *runtime, JSContext *realm,
                                    int syntax) {
    JSValue original = JS_GetException(realm);
    JSValue copy;

    if (qjs_runtime_is_interrupted(runtime)) {
        JS_FreeValue(realm, original);
        return qjs_throw_interrupted(realm);
    }
    copy = qjs_boundary_error(realm, original, syntax);
    JS_FreeValue(realm, original);
    if (JS_IsException(copy)) {
        return JS_EXCEPTION;
    }
    return JS_Throw(realm, copy);
}

/* A link object of `realm`'s, holding `target` and watching `caller`. The
   opaque is set before anything can free the object, so its finalizer
   always has both references to release. */
static JSValue qjs_realm_link_new(QjsRuntime *runtime, JSContext *realm,
                                  QjsRealmToken *target,
                                  QjsRealmToken *caller) {
    QjsShadowLink *link = malloc(sizeof(*link));
    JSValue object;

    if (link == NULL) {
        return JS_ThrowOutOfMemory(realm);
    }
    object = JS_NewObjectClass(realm, (int)runtime->realm_link_class_id);
    if (JS_IsException(object)) {
        free(link);
        return JS_EXCEPTION;
    }
    qjs_token_hold(target);
    link->target = target;
    link->caller = caller;
    if (caller != NULL) {
        qjs_token_watch(caller);
    }
    JS_SetOpaque(object, link);
    return object;
}

static JSValue qjs_wrapped_call(JSContext *ctx, JSValueConst this_value,
                                int argc, JSValueConst *argv, int magic,
                                JSValue *data);

/* CopyNameAndLength(wrapped, target), reading `target` in `source`, its own
   realm, and defining on `wrapped` in `into`. -1 leaves whatever a read
   threw pending. */
static int qjs_copy_name_and_length(JSContext *source, JSContext *into,
                                    JSValueConst wrapped,
                                    JSValueConst target) {
    JSAtom length_name = JS_NewAtom(source, "length");
    double length = 0.0;
    JSValue value;
    int found;

    if (length_name == JS_ATOM_NULL) {
        return -1;
    }
    found = JS_GetOwnProperty(source, NULL, target, length_name);
    if (found > 0) {
        value = JS_GetProperty(source, target, length_name);
        if (JS_IsException(value)) {
            found = -1;
        } else {
            if (JS_IsNumber(value)) {
                double number = 0.0;

                JS_ToFloat64(source, &number, value);
                if (isnan(number)) {
                    length = 0.0;
                } else if (!isfinite(number)) {
                    /* +Infinity stays; -Infinity is 0. */
                    length = number > 0 ? number : 0.0;
                } else {
                    number = trunc(number);
                    length = number > 0 ? number : 0.0;
                }
            }
            JS_FreeValue(source, value);
        }
    }
    JS_FreeAtom(source, length_name);
    if (found < 0) {
        return -1;
    }
    if (JS_DefinePropertyValueStr(into, wrapped, "length",
                                  JS_NewFloat64(into, length),
                                  JS_PROP_CONFIGURABLE) < 0) {
        return -1;
    }
    value = JS_GetPropertyStr(source, target, "name");
    if (JS_IsException(value)) {
        return -1;
    }
    if (!JS_IsString(value)) {
        JS_FreeValue(source, value);
        value = JS_NewString(into, "");
        if (JS_IsException(value)) {
            return -1;
        }
    }
    return JS_DefinePropertyValueStr(into, wrapped, "name", value,
                                     JS_PROP_CONFIGURABLE) < 0
               ? -1
               : 0;
}

/* WrappedFunctionCreate: a callable of `into` whose calls run `target` in
   the realm `from` names. Its length is 0 because a function-data record's
   length is what QuickJS pads the argument list to; the observable `length`
   is defined over it. */
static JSValue qjs_wrapped_function_create(QjsRuntime *runtime,
                                           JSContext *into,
                                           JSContext *error_realm,
                                           QjsRealmToken *from,
                                           JSValueConst target) {
    JSValue data[2];
    JSValue wrapped;
    JSContext *source;
    int status;

    if (from->context == NULL) {
        return qjs_throw_released(error_realm);
    }
    data[0] = target;
    data[1] = qjs_realm_link_new(runtime, into, from, NULL);
    if (JS_IsException(data[1])) {
        return JS_EXCEPTION;
    }
    wrapped = JS_NewCFunctionData(into, qjs_wrapped_call, 0, 0, 2, data);
    JS_FreeValue(into, data[1]);
    if (JS_IsException(wrapped)) {
        return JS_EXCEPTION;
    }
    /* A getter on `name` or `length` is code of `from`'s, which may reach a
       host function that lets the realm go. */
    source = JS_DupContext(from->context->raw);
    status = qjs_copy_name_and_length(source, into, wrapped, target);
    JS_FreeContext(source);
    if (status < 0) {
        JS_FreeValue(into, wrapped);
        return qjs_boundary_rethrow(runtime, error_realm, 0);
    }
    return wrapped;
}

/* GetWrappedValue: a primitive crosses as itself — strings, symbols and
   BigInts are the runtime's, not a realm's — and a callable crosses wrapped.
   Any other object is a `TypeError` in `error_realm`. */
static JSValue qjs_get_wrapped_value(QjsRuntime *runtime, JSContext *into,
                                     JSContext *error_realm,
                                     QjsRealmToken *from,
                                     JSValueConst value) {
    if (!JS_IsObject(value)) {
        return JS_DupValue(into, value);
    }
    if (!JS_IsFunction(into, value)) {
        return JS_ThrowTypeError(error_realm,
                                 "an object that is not callable cannot "
                                 "cross a ShadowRealm boundary");
    }
    return qjs_wrapped_function_create(runtime, into, error_realm, from,
                                       value);
}

/* A wrapped function's [[Call]]: `data[0]` is the target, `data[1]` the
   link holding its realm. The receiver and the arguments are wrapped into
   the target's realm, the call runs there, and its result — or what it
   threw — comes back wrapped. Nothing reads the link's realm after the call,
   which may have let it go. A wrapped function is not a constructor. */
static JSValue qjs_wrapped_call(JSContext *ctx, JSValueConst this_value,
                                int argc, JSValueConst *argv, int magic,
                                JSValue *data) {
    QjsRuntime *runtime = qjs_runtime_of(ctx);
    QjsShadowLink *link = JS_GetOpaque(data[1], runtime->realm_link_class_id);
    QjsContext *caller = JS_GetContextOpaque(ctx);
    QjsRealmToken *caller_token;
    JSValue inline_arguments[QJS_HOST_INLINE_ARGS];
    JSValue *arguments = inline_arguments;
    JSValue receiver;
    JSValue returned;
    JSValue result = JS_EXCEPTION;
    JSContext *target;
    int built = 0;

    (void)magic;
    if (link == NULL || link->target->context == NULL || caller == NULL) {
        return qjs_throw_released(ctx);
    }
    caller_token = qjs_context_token(caller);
    if (caller_token == NULL) {
        return JS_ThrowOutOfMemory(ctx);
    }
    if (argc > QJS_HOST_INLINE_ARGS) {
        if ((size_t)argc > SIZE_MAX / sizeof(*arguments)) {
            return JS_ThrowRangeError(ctx, "too many call arguments");
        }
        arguments = malloc((size_t)argc * sizeof(*arguments));
        if (arguments == NULL) {
            return JS_ThrowOutOfMemory(ctx);
        }
    }
    target = JS_DupContext(link->target->context->raw);
    receiver =
        qjs_get_wrapped_value(runtime, target, ctx, caller_token, this_value);
    if (!JS_IsException(receiver)) {
        for (built = 0; built < argc; ++built) {
            arguments[built] = qjs_get_wrapped_value(runtime, target, ctx,
                                                     caller_token, argv[built]);
            if (JS_IsException(arguments[built])) {
                break;
            }
        }
        if (built == argc && link->target->context == NULL) {
            /* A `name` or `length` getter of the caller's, read while
               wrapping, let the target's realm go. */
            result = qjs_throw_released(ctx);
        } else if (built == argc) {
            returned = JS_Call(target, data[0], receiver, argc, arguments);
            if (JS_IsException(returned)) {
                result = qjs_boundary_rethrow(runtime, ctx, 0);
            } else {
                result = qjs_get_wrapped_value(runtime, ctx, ctx, link->target,
                                               returned);
                JS_FreeValue(target, returned);
            }
        }
        while (built > 0) {
            JS_FreeValue(target, arguments[--built]);
        }
        JS_FreeValue(target, receiver);
    }
    if (arguments != inline_arguments) {
        free(arguments);
    }
    JS_FreeContext(target);
    return result;
}

/* `new ShadowRealm()`, in the parent realm `ctx`. The child is a fresh
   context of this runtime with nothing installed, which the host's
   initializer adopts; from that call on the child may already be freed, so
   only its token is consulted. */
static JSValue qjs_shadow_realm_construct(JSContext *ctx,
                                          JSValueConst new_target, int argc,
                                          JSValueConst *argv) {
    QjsRuntime *runtime = qjs_runtime_of(ctx);
    QjsContext *parent;
    QjsContext *child;
    QjsRealmToken *token = NULL;
    QjsShadowRealmInit *init;
    void *opaque;
    JSValue prototype;
    JSValue realm;
    JSValue global;
    const char *message = NULL;
    int status;

    (void)argc;
    (void)argv;
    /* OrdinaryCreateFromConstructor. The fallback is this realm's prototype
       rather than the new target's realm's: QuickJS keeps
       `JS_GetFunctionRealm` private. */
    prototype = JS_GetPropertyStr(ctx, new_target, "prototype");
    if (JS_IsException(prototype)) {
        return JS_EXCEPTION;
    }
    if (!JS_IsObject(prototype)) {
        JS_FreeValue(ctx, prototype);
        prototype = JS_GetClassProto(ctx, runtime->shadow_realm_class_id);
    }
    realm = JS_NewObjectProtoClass(ctx, prototype,
                                   runtime->shadow_realm_class_id);
    JS_FreeValue(ctx, prototype);
    if (JS_IsException(realm)) {
        return JS_EXCEPTION;
    }
    /* Read after the `prototype` lookup, which can run code. */
    parent = JS_GetContextOpaque(ctx);
    if (parent == NULL || parent->shadow_init == NULL) {
        JS_FreeValue(ctx, realm);
        return JS_ThrowInternalError(ctx, "this realm is being released");
    }
    child = qjs_context_new(runtime);
    if (child == NULL) {
        JS_FreeValue(ctx, realm);
        return JS_ThrowOutOfMemory(ctx);
    }
    /* Before any code of the child's can replace it. */
    global = JS_GetGlobalObject(child->raw);
    child->shadow_eval = JS_GetPropertyStr(child->raw, global, "eval");
    JS_FreeValue(child->raw, global);
    if (JS_IsException(child->shadow_eval)) {
        child->shadow_eval = JS_UNDEFINED;
        JS_FreeValue(child->raw, JS_GetException(child->raw));
    } else {
        token = qjs_context_token(child);
    }
    if (token == NULL) {
        qjs_context_free(child);
        JS_FreeValue(ctx, realm);
        return JS_ThrowOutOfMemory(ctx);
    }
    qjs_token_hold(token);
    JS_SetOpaque(realm, token);
    init = parent->shadow_init;
    opaque = parent->shadow_opaque;
    status = init(opaque, child, &message);
    /* The child is the host's now, and may already be gone. */
    if (status != 0) {
        JS_FreeValue(ctx, realm);
        if (qjs_runtime_is_interrupted(runtime)) {
            JS_FreeValue(ctx, JS_GetException(ctx));
            return qjs_throw_interrupted(ctx);
        }
        return qjs_throw_with_message(
            ctx, 0,
            JS_NewString(ctx, message != NULL
                                  ? message
                                  : "the host refused to initialize the "
                                    "ShadowRealm"));
    }
    if (token->context == NULL) {
        JS_FreeValue(ctx, realm);
        return JS_ThrowTypeError(
            ctx, "the host released the ShadowRealm during its initialization");
    }
    return realm;
}

/* `ShadowRealm.prototype.evaluate(sourceText)`: PerformShadowRealmEval.

   The child's own `%eval%`, called indirectly, is the one QuickJS entry that
   evaluates a script against the global variable environment with a fresh
   declarative environment of its own, so a `let` of one call is not seen by
   the next while a `var` is. It cannot tell a parse error from a
   `SyntaxError` the code threw, which the two kinds of boundary error need,
   so the text is compiled once, unrun, first. No evaluation depth is raised,
   for the reason `qjs_eval` gives: a script cannot start a module body. */
static JSValue qjs_shadow_realm_evaluate(JSContext *ctx,
                                         JSValueConst this_value, int argc,
                                         JSValueConst *argv) {
    QjsRuntime *runtime = qjs_runtime_of(ctx);
    QjsRealmToken *token =
        JS_GetOpaque(this_value, runtime->shadow_realm_class_id);
    JSContext *child;
    JSValue eval;
    JSValue compiled;
    JSValue value;
    JSValue result;
    const char *text;
    size_t length;

    (void)argc;
    if (token == NULL) {
        return JS_ThrowTypeError(ctx, "ShadowRealm.prototype.evaluate called "
                                      "on an object that is not a ShadowRealm");
    }
    if (!JS_IsString(argv[0])) {
        return JS_ThrowTypeError(
            ctx, "ShadowRealm.prototype.evaluate expects source text that is "
                 "a string");
    }
    if (token->context == NULL) {
        return qjs_throw_released(ctx);
    }
    child = JS_DupContext(token->context->raw);
    eval = JS_DupValue(child, token->context->shadow_eval);
    text = JS_ToCStringLen(ctx, &length, argv[0]);
    if (text == NULL) {
        result = JS_EXCEPTION;
        goto done;
    }
    compiled = JS_Eval(child, text, length, "<input>",
                       JS_EVAL_TYPE_GLOBAL | JS_EVAL_FLAG_COMPILE_ONLY);
    JS_FreeCString(ctx, text);
    if (JS_IsException(compiled)) {
        result = qjs_boundary_rethrow(runtime, ctx, 1);
        goto done;
    }
    JS_FreeValue(child, compiled);
    value = JS_Call(child, eval, JS_UNDEFINED, 1, argv);
    if (JS_IsException(value)) {
        result = qjs_boundary_rethrow(runtime, ctx, 0);
        goto done;
    }
    result = qjs_get_wrapped_value(runtime, ctx, ctx, token, value);
    JS_FreeValue(child, value);
done:
    JS_FreeValue(child, eval);
    JS_FreeContext(child);
    return result;
}

/* `TypeError("module '<specifier>' has no export named '<name>'")` of
   `realm`, built rather than formatted because neither name is bounded. */
static JSValue qjs_missing_export_error(JSContext *realm,
                                        JSValueConst specifier,
                                        JSValueConst export_name) {
    static const char PROLOGUE[] = "module '";
    static const char INFIX[] = "' has no export named '";
    static const char EPILOGUE[] = "'";
    size_t fixed = sizeof(PROLOGUE) - 1 + sizeof(INFIX) - 1 +
                   sizeof(EPILOGUE) - 1;
    size_t specifier_length = 0;
    size_t name_length = 0;
    const char *specifier_text;
    const char *name_text;
    char *message;
    size_t offset;
    JSValue text;

    specifier_text = JS_ToCStringLen(realm, &specifier_length, specifier);
    if (specifier_text == NULL) {
        return JS_EXCEPTION;
    }
    name_text = JS_ToCStringLen(realm, &name_length, export_name);
    if (name_text == NULL) {
        JS_FreeCString(realm, specifier_text);
        return JS_EXCEPTION;
    }
    if (specifier_length > SIZE_MAX - fixed - name_length) {
        message = NULL;
    } else {
        message = malloc(fixed + specifier_length + name_length);
    }
    if (message == NULL) {
        JS_FreeCString(realm, specifier_text);
        JS_FreeCString(realm, name_text);
        return JS_ThrowOutOfMemory(realm);
    }
    offset = 0;
    memcpy(message, PROLOGUE, sizeof(PROLOGUE) - 1);
    offset += sizeof(PROLOGUE) - 1;
    memcpy(message + offset, specifier_text, specifier_length);
    offset += specifier_length;
    memcpy(message + offset, INFIX, sizeof(INFIX) - 1);
    offset += sizeof(INFIX) - 1;
    memcpy(message + offset, name_text, name_length);
    offset += name_length;
    memcpy(message + offset, EPILOGUE, sizeof(EPILOGUE) - 1);
    offset += sizeof(EPILOGUE) - 1;
    JS_FreeCString(realm, specifier_text);
    JS_FreeCString(realm, name_text);
    text = JS_NewStringLen(realm, message, offset);
    free(message);
    return qjs_error_with_message(realm, 0, text);
}

/* The boundary error for what the child left pending, made in `caller`;
   an exception of its own if even that fails. */
static JSValue qjs_take_boundary_error(JSContext *child, JSContext *caller) {
    JSValue pending = JS_GetException(child);
    JSValue reason = qjs_boundary_error(caller, pending, 0);

    JS_FreeValue(child, pending);
    if (JS_IsException(reason)) {
        reason = JS_GetException(caller);
    }
    return reason;
}

/* Settles the promise `importValue` answered, with `outcome` (consumed),
   calling its resolving function in the realm that asked so that an
   unhandled rejection is that realm's to report. */
static void qjs_shadow_import_settle(JSContext *caller, JSValueConst settle,
                                     JSValue outcome) {
    JSValue returned = JS_Call(caller, settle, JS_UNDEFINED, 1,
                               (JSValueConst *)&outcome);

    JS_FreeValue(caller, returned);
    JS_FreeValue(caller, outcome);
}

/* The two continuations of an `importValue`'s module load, which run in the
   child (`ctx`): `data` is `{ resolve, reject, exportName, specifier, link }`,
   the first two the caller's. A caller the host has let go of has no promise
   left to settle; an interrupt settles nothing. Nothing of the child's —
   not its error, not a promise of its own — reaches the caller. */
static JSValue qjs_shadow_import_fulfilled(JSContext *ctx,
                                           JSValueConst this_value, int argc,
                                           JSValueConst *argv, int magic,
                                           JSValue *data) {
    QjsRuntime *runtime = qjs_runtime_of(ctx);
    QjsShadowLink *link = JS_GetOpaque(data[4], runtime->realm_link_class_id);
    JSContext *caller;
    JSValue outcome;
    JSValue value;
    JSAtom name;
    int found;
    int fulfilled = 0;

    (void)this_value;
    (void)argc;
    (void)magic;
    if (link == NULL || link->caller == NULL ||
        link->caller->context == NULL) {
        return JS_UNDEFINED;
    }
    if (qjs_runtime_is_interrupted(runtime)) {
        return qjs_throw_interrupted(ctx);
    }
    caller = JS_DupContext(link->caller->context->raw);
    name = JS_ValueToAtom(ctx, data[2]);
    if (name == JS_ATOM_NULL) {
        outcome = qjs_take_boundary_error(ctx, caller);
    } else {
        /* A binding still in its temporal dead zone throws here; what it
           threw stays the child's, as every boundary error does. */
        found = JS_GetOwnProperty(ctx, NULL, argv[0], name);
        if (found < 0) {
            outcome = qjs_take_boundary_error(ctx, caller);
        } else if (found == 0) {
            outcome = qjs_missing_export_error(caller, data[3], data[2]);
            if (JS_IsException(outcome)) {
                outcome = JS_GetException(caller);
            }
        } else {
            value = JS_GetProperty(ctx, argv[0], name);
            if (JS_IsException(value)) {
                outcome = qjs_take_boundary_error(ctx, caller);
            } else {
                outcome = qjs_get_wrapped_value(runtime, caller, caller,
                                                link->target, value);
                JS_FreeValue(ctx, value);
                if (JS_IsException(outcome)) {
                    /* Already the caller's own `TypeError`. */
                    outcome = JS_GetException(caller);
                } else {
                    fulfilled = 1;
                }
            }
        }
        JS_FreeAtom(ctx, name);
    }
    if (qjs_runtime_is_interrupted(runtime)) {
        JS_FreeValue(caller, outcome);
        JS_FreeContext(caller);
        return qjs_throw_interrupted(ctx);
    }
    qjs_shadow_import_settle(caller, data[fulfilled ? 0 : 1], outcome);
    JS_FreeContext(caller);
    return JS_UNDEFINED;
}

/* Forgets the child's unhandled rejections whose reason is `reason`.

   A module body is an async function in QuickJS, so one that throws rejects
   a promise of its own that nothing handles, beside the evaluation promise
   the load handles; `import()` in any realm leaves that one behind. Here the
   failure is the caller's to hear about — as its own rejection — and not
   the child's, which asked for nothing, so the child's copy of it goes. */
static void qjs_rejections_forget_reason(QjsRuntime *runtime,
                                         QjsContext *context,
                                         JSValueConst reason) {
    QjsUnhandledRejection *current = runtime->rejection_head;
    QjsUnhandledRejection *previous = NULL;

    while (current != NULL) {
        QjsUnhandledRejection *next = current->next;

        if (current->context == context &&
            JS_StrictEq(context->raw, current->reason, reason)) {
            if (previous == NULL) {
                runtime->rejection_head = next;
            } else {
                previous->next = next;
            }
            if (runtime->rejection_tail == current) {
                runtime->rejection_tail = previous;
            }
            JS_FreeValue(context->raw, current->promise);
            JS_FreeValue(context->raw, current->reason);
            free(current);
        } else {
            previous = current;
        }
        current = next;
    }
}

static JSValue qjs_shadow_import_rejected(JSContext *ctx,
                                          JSValueConst this_value, int argc,
                                          JSValueConst *argv, int magic,
                                          JSValue *data) {
    QjsRuntime *runtime = qjs_runtime_of(ctx);
    QjsShadowLink *link = JS_GetOpaque(data[4], runtime->realm_link_class_id);
    QjsContext *child = JS_GetContextOpaque(ctx);
    JSContext *caller;
    JSValue reason;

    (void)this_value;
    (void)argc;
    (void)magic;
    if (child != NULL) {
        qjs_rejections_forget_reason(runtime, child, argv[0]);
    }
    if (link == NULL || link->caller == NULL ||
        link->caller->context == NULL) {
        return JS_UNDEFINED;
    }
    if (qjs_runtime_is_interrupted(runtime)) {
        return qjs_throw_interrupted(ctx);
    }
    caller = JS_DupContext(link->caller->context->raw);
    reason = qjs_boundary_error(caller, argv[0], 0);
    if (JS_IsException(reason)) {
        reason = JS_GetException(caller);
    }
    qjs_shadow_import_settle(caller, data[1], reason);
    JS_FreeContext(caller);
    return JS_UNDEFINED;
}

/* `ShadowRealm.prototype.importValue(specifier, exportName)`.

   The load is the child's own `JS_LoadModuleInternal`, reached through
   `JS_ResumeModuleLoad` with resolving functions of this shim's rather than
   a promise's, so no promise of the child's is created or observed. A source
   the child lacks is deferred the way any of its imports is — by the
   runtime's deferrer, into the child's own pending loads — and completing it
   on the child and resuming the child's loads is what goes on to settle
   this. The specifier resolves against the calling script's name, as
   `qjs_context_load_module`'s root resolves against its own. */
static JSValue qjs_shadow_realm_import_value(JSContext *ctx,
                                             JSValueConst this_value,
                                             int argc, JSValueConst *argv) {
    QjsRuntime *runtime = qjs_runtime_of(ctx);
    QjsRealmToken *token =
        JS_GetOpaque(this_value, runtime->shadow_realm_class_id);
    QjsContext *caller;
    QjsRealmToken *caller_token;
    JSContext *child = NULL;
    JSValue specifier;
    JSValue promise = JS_UNDEFINED;
    JSValue resolving[2] = {JS_UNDEFINED, JS_UNDEFINED};
    JSValue settle[2] = {JS_UNDEFINED, JS_UNDEFINED};
    JSValue data[5];
    JSValue link;
    JSValue result = JS_EXCEPTION;
    const char *specifier_text = NULL;
    const char *base_text = NULL;
    JSAtom base;

    (void)argc;
    if (token == NULL) {
        return JS_ThrowTypeError(ctx, "ShadowRealm.prototype.importValue "
                                      "called on an object that is not a "
                                      "ShadowRealm");
    }
    if (token->context == NULL) {
        return qjs_throw_released(ctx);
    }
    specifier = JS_ToString(ctx, argv[0]);
    if (JS_IsException(specifier)) {
        return JS_EXCEPTION;
    }
    if (!JS_IsString(argv[1])) {
        JS_FreeValue(ctx, specifier);
        return JS_ThrowTypeError(ctx, "ShadowRealm.prototype.importValue "
                                      "expects an export name that is a "
                                      "string");
    }
    /* Again: the conversion above can run code that lets the child go. */
    if (token->context == NULL) {
        JS_FreeValue(ctx, specifier);
        return qjs_throw_released(ctx);
    }
    caller = JS_GetContextOpaque(ctx);
    if (caller == NULL) {
        JS_FreeValue(ctx, specifier);
        return JS_ThrowInternalError(ctx, "this realm is being released");
    }
    caller_token = qjs_context_token(caller);
    if (caller_token == NULL) {
        JS_FreeValue(ctx, specifier);
        return JS_ThrowOutOfMemory(ctx);
    }
    promise = JS_NewPromiseCapability(ctx, resolving);
    if (JS_IsException(promise)) {
        JS_FreeValue(ctx, specifier);
        return JS_EXCEPTION;
    }
    child = JS_DupContext(token->context->raw);
    link = qjs_realm_link_new(runtime, child, token, caller_token);
    if (JS_IsException(link)) {
        goto done;
    }
    data[0] = resolving[0];
    data[1] = resolving[1];
    data[2] = argv[1];
    data[3] = specifier;
    data[4] = link;
    settle[0] =
        JS_NewCFunctionData(child, qjs_shadow_import_fulfilled, 1, 0, 5, data);
    settle[1] =
        JS_NewCFunctionData(child, qjs_shadow_import_rejected, 1, 0, 5, data);
    JS_FreeValue(child, link);
    if (JS_IsException(settle[0]) || JS_IsException(settle[1])) {
        goto done;
    }
    specifier_text = JS_ToCString(ctx, specifier);
    if (specifier_text == NULL) {
        goto done;
    }
    base = JS_GetScriptOrModuleName(ctx, 1);
    if (base != JS_ATOM_NULL) {
        base_text = JS_AtomToCString(ctx, base);
        JS_FreeAtom(ctx, base);
        if (base_text == NULL) {
            goto done;
        }
    }
    /* The load may link and evaluate a graph under this call, as a resumed
       import does. */
    runtime->evaluation_depth += 1;
    JS_ResumeModuleLoad(child,
                        base_text != NULL ? base_text : specifier_text,
                        specifier_text, (JSValueConst *)settle, JS_UNDEFINED);
    runtime->evaluation_depth -= 1;
    if (qjs_runtime_is_interrupted(runtime)) {
        JS_FreeValue(ctx, JS_GetException(ctx));
        result = qjs_throw_interrupted(ctx);
    } else if (!JS_HasException(ctx)) {
        result = JS_DupValue(ctx, promise);
    }
done:
    JS_FreeCString(ctx, specifier_text);
    JS_FreeCString(ctx, base_text);
    JS_FreeValue(child, settle[0]);
    JS_FreeValue(child, settle[1]);
    JS_FreeContext(child);
    JS_FreeValue(ctx, resolving[0]);
    JS_FreeValue(ctx, resolving[1]);
    JS_FreeValue(ctx, promise);
    JS_FreeValue(ctx, specifier);
    return result;
}

static const JSCFunctionListEntry qjs_shadow_realm_prototype[] = {
    JS_CFUNC_DEF("evaluate", 1, qjs_shadow_realm_evaluate),
    JS_CFUNC_DEF("importValue", 2, qjs_shadow_realm_import_value),
    JS_PROP_STRING_DEF("[Symbol.toStringTag]", "ShadowRealm",
                       JS_PROP_CONFIGURABLE),
};

/* Installs the global `ShadowRealm` constructor in this realm, backed by
   `init`. A realm takes one (-6 for a second); -1 leaves the exception
   pending. */
int qjs_context_install_shadow_realm(QjsContext *context,
                                     QjsShadowRealmInit *init, void *opaque) {
    JSContext *raw = context->raw;
    JSValue prototype;
    JSValue constructor;
    JSValue global;
    int status;

    if (context->shadow_init != NULL) {
        return -6;
    }
    prototype = JS_NewObject(raw);
    if (JS_IsException(prototype)) {
        return -1;
    }
    if (JS_SetPropertyFunctionList(
            raw, prototype, qjs_shadow_realm_prototype,
            (int)(sizeof(qjs_shadow_realm_prototype) /
                  sizeof(qjs_shadow_realm_prototype[0]))) < 0) {
        JS_FreeValue(raw, prototype);
        return -1;
    }
    constructor = JS_NewCFunction2(raw, qjs_shadow_realm_construct,
                                   "ShadowRealm", 0, JS_CFUNC_constructor, 0);
    if (JS_IsException(constructor)) {
        JS_FreeValue(raw, prototype);
        return -1;
    }
    /* `prototype` non-writable, `constructor` writable and configurable. */
    if (JS_SetConstructor(raw, constructor, prototype) < 0) {
        JS_FreeValue(raw, constructor);
        JS_FreeValue(raw, prototype);
        return -1;
    }
    global = JS_GetGlobalObject(raw);
    status = JS_DefinePropertyValueStr(raw, global, "ShadowRealm", constructor,
                                       JS_PROP_WRITABLE | JS_PROP_CONFIGURABLE);
    JS_FreeValue(raw, global);
    if (status < 0) {
        JS_FreeValue(raw, prototype);
        return -1;
    }
    /* The fallback prototype of a construction whose new target has none. */
    JS_SetClassProto(raw, context->runtime->shadow_realm_class_id, prototype);
    context->shadow_init = init;
    context->shadow_opaque = opaque;
    return 0;
}

/* How many objects need this realm alive to do their job. */
size_t qjs_context_shadow_holders(QjsContext *context) {
    return context->token != NULL ? context->token->holders : 0;
}
