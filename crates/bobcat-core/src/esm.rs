//! The module names both kinds of realm agree on, and the built-in modules
//! each engine thread registers on its runtime.
//!
//! A source is registered per `QuickJS` runtime, so a group registers each
//! module both lists name twice: once on the runtime its views' realms share,
//! once on the one its workers share. The names are here, beside
//! [`crate::clock`], because neither runtime owns them.

use std::sync::Arc;

use crate::main::quickjs::ScriptRuntime;
use crate::script::ScriptError;

/// The `packages/bobcat-element/src` module `$name` as the JavaScript a realm
/// evaluates, compiled by `build.rs` into this Cargo build's `OUT_DIR`.
macro_rules! runtime_source {
    ($name:literal) => {
        include_str!(concat!(env!("OUT_DIR"), "/runtime/", $name, ".js"))
    };
}

/// One built-in module, as a runtime registers it.
pub(crate) struct BuiltinModule {
    /// What a realm imports it by.
    pub(crate) specifier: &'static str,
    /// The `packages/bobcat-element/src` file it is compiled from.
    pub(crate) file: &'static str,
    /// The JavaScript `build.rs` compiled that file to.
    pub(crate) source: &'static str,
}

/// The [`BuiltinModule`] compiled from `packages/bobcat-element/src/$name.ts`,
/// imported as `$specifier`. The file name and the source are both written
/// from `$name`, so the two cannot name different files.
macro_rules! builtin_module {
    ($specifier:expr, $name:literal) => {
        BuiltinModule {
            specifier: $specifier,
            file: concat!($name, ".ts"),
            source: runtime_source!($name),
        }
    };
}

/// The native module every realm's Rust-backed members are exported from.
pub(crate) const HOST_MODULE_SPECIFIER: &str = "bobcat-internal:host";

/// The timer runtime: the realm's half of `setTimeout` and its three
/// companions, imported for its effect.
pub(crate) const TIMER_MODULE_SPECIFIER: &str = "bobcat:timers";

/// The `Future` class: one host-backed operation, usable synchronously
/// through `wait` and as a `PromiseLike` through `then`. The class is
/// JavaScript like every other built-in; the table behind it and the three
/// members it speaks to are [`crate::future`].
pub(crate) const FUTURE_MODULE_SPECIFIER: &str = "bobcat:future";

/// Node's `createRequire`, the one synchronous way into a source a realm has
/// not imported. The algorithm is JavaScript like every other built-in; what
/// it is written over is the two host members [`crate::require`] installs.
pub(crate) const REQUIRE_MODULE_SPECIFIER: &str = "bobcat:module";

/// The `EventTarget` a view's `lynx.getEngine()` and a worker's global scope
/// are both built on.
pub(crate) const EVENT_TARGET_MODULE_SPECIFIER: &str = "bobcat:event-target";

pub(crate) const GLOBAL_EVENT_MODULE_SPECIFIER: &str = "bobcat:global-event-emitter";

/// Lynx's typed asynchronous Context channel, shared by MTS and BTS.
pub(crate) const CONTEXT_MODULE_SPECIFIER: &str = "bobcat:cross-thread-context";

/// The built-in BTS bootstrap, loaded like any other Worker script.
pub(crate) const BTS_MODULE_SPECIFIER: &str = "bobcat:bts";

/// The Element PAPI: the named exports an MTS entry builds and edits its
/// view's document through.
pub(crate) const ELEMENT_MODULE_SPECIFIER: &str = "bobcat:element";

/// The MTS compatibility module: the bindings an MTS entry is given, and the
/// exports the view calls into its realm through.
pub(crate) const RUNTIME_MODULE_SPECIFIER: &str = "bobcat:runtime";

/// The MTS realm's `Worker` class. The one built-in name without a colon,
/// which the module normalizer passes through by name.
pub(crate) const WORKER_CLASS_MODULE_SPECIFIER: &str = "bobcat-internal";

/// The worker realm's global-scope module, on the *worker* runtime.
pub(crate) const WORKER_MODULE_SPECIFIER: &str = "bobcat:worker";

/// The compiler factory ABI: what the BTS runtime's `lynx.requireModule` and
/// its companions load a bundle body through.
pub(crate) const LYNX_MODULES_SPECIFIER: &str = "bobcat:lynx-modules";

/// The literal [`MTS_CHUNK_PREAMBLE`] is, as a macro, so that
/// `main::runtime`'s `ENTRY_PREAMBLE` can `concat!` onto it: `concat!` takes
/// literals and a `const` is not one.
macro_rules! mts_chunk_preamble {
    () => {
        concat!(
            "import { __Card__, lynx, console, SystemInfo, __globalProps, NativeModules, ",
            "_AddEventListener, _ReportError, _SetSourceMapRelease, __OnLifecycleEvent, ",
            "__LoadLepusChunk, __LoadStyleSheet, __AdoptStyleSheet } from \"bobcat:runtime\"; ",
            "import { __CreatePage, __CreateElement, __CreateWrapperElement, __CreateText, ",
            "__CreateImage, __CreateView, __CreateScrollView, __CreateRawText, __CreateList, ",
            "__AppendElement, __InsertElementBefore, __RemoveElement, __ReplaceElement, ",
            "__ReplaceElements, __SwapElement, __SetClasses, __SetID, __GetID, __GetTag, ",
            "__GetChildren, __GetAttributeByName, __GetAttributeNames, __GetElementUniqueID, ",
            "__SetDataset, __GetDataset, __AddDataset, __SetInlineStyles, __AddInlineStyle, ",
            "__SetCSSId, __SetAttribute, __UpdateListCallbacks, __AddEvent, __GetEvent, ",
            "__GetEvents, __SetEvents, __AddEventListener, __RemoveEventListener, ",
            "__StopPropagation, __StopImmediatePropagation, __GetPageElement, __QuerySelector, ",
            "__QuerySelectorAll, __InvokeUIMethod, __GetComputedStyleByKey, __FlushElementTree ",
            "} from \"bobcat:element\"; ",
        )
    };
}
pub(crate) use mts_chunk_preamble;

/// Named imports prepended to one *MTS chunk body* before it is registered as
/// a module: every binding `main::runtime`'s `ENTRY_PREAMBLE` gives a card's
/// entry, which is what a lazy container's `main-thread` section expects to
/// find in scope.
///
/// One physical line, deliberately: the body follows it on the same line, so
/// every line of the body keeps the number it had in the container. The entry
/// preamble is built from the same literal, so the two lists cannot drift.
///
/// `bobcat-source`'s lazy-container installer is what prepends it; it lives
/// here because the names are this realm's.
pub const MTS_CHUNK_PREAMBLE: &str = mts_chunk_preamble!();

/// BTS bindings live separately from the bootstrap that awaits the app entry.
pub(crate) const BTS_RUNTIME_MODULE_SPECIFIER: &str = "bobcat:bts-runtime";

/// Named imports prepended to a BTS application entry, as for MTS. The
/// bootstrap uses the same import to initialize the Context before the app.
pub(crate) const BTS_ENTRY_PREAMBLE: &str = "import { lynx } from \"bobcat:bts-runtime\";\n";

/// Named imports prepended to one *bundle body* before it is registered as a
/// module: every name web-core's `createBundleInitReturnObj` puts in a chunk
/// wrapper's parameter list (`createChunkLoading.ts`), the ones this realm has
/// a value for as imports of [`BTS_RUNTIME_MODULE_SPECIFIER`] and the rest as
/// `undefined`, exactly as web-core supplies them. `module` and `exports` are
/// not here: a `CommonJS` body needs its own pair, and the adapter that writes
/// one declares them beside this.
///
/// One physical line, deliberately: the body follows it on the same line, so
/// every line of the body keeps the number it had in the container.
///
/// `bobcat-source`'s `PageSource` is what prepends it, as a page's bodies are
/// its to register; it lives here because the names are this realm's.
pub const BTS_CHUNK_PREAMBLE: &str = concat!(
    "import { lynx, lynxCoreInject, NativeModules, console, SystemInfo, Card, Component, ",
    "nativeAppId, Behavior, LynxJSBI, setTimeout, setInterval, clearTimeout, clearInterval, ",
    "requestAnimationFrame, cancelAnimationFrame } from \"bobcat:bts-runtime\"; ",
    "const postMessage = undefined, ReactLynx = undefined, window = undefined, ",
    "document = undefined, frames = undefined, location = undefined, navigator = undefined, ",
    "localStorage = undefined, history = undefined, Caches = undefined, screen = undefined, ",
    "alert = undefined, confirm = undefined, prompt = undefined, webkit = undefined, ",
    "Reporter = undefined, print = undefined, global = undefined; ",
);

/// The URL rule both realms write a container's section and stylesheet URLs
/// with — one module so the two cannot drift, and so `bobcat-source` has one
/// rule to register under.
pub(crate) const SECTION_URL_MODULE_SPECIFIER: &str = "bobcat:section-url";

/// `lynx.fetchBundle`'s handle: the `{wait, then}` object and the callback
/// list, over the [`Future`] a `fetchResource` ([`crate::fetch`]) answers
/// with. Both realm kinds import it, because either thread's card may ask
/// for a lazy container.
///
/// [`Future`]: crate::future
pub(crate) const BUNDLE_FETCH_MODULE_SPECIFIER: &str = "bobcat:bundle-fetch";

/// BTS query builders carry selection tokens across Worker messages.
pub(crate) const SELECTOR_QUERY_SPECIFIER: &str = "bobcat:selector-query";

/// What `bobcat-main` registers on the runtime its views' realms share.
pub(crate) const MAIN_THREAD_MODULES: &[BuiltinModule] = &[
    builtin_module!(WORKER_CLASS_MODULE_SPECIFIER, "worker"),
    builtin_module!(EVENT_TARGET_MODULE_SPECIFIER, "event-target"),
    builtin_module!(CONTEXT_MODULE_SPECIFIER, "cross-thread-context"),
    builtin_module!(RUNTIME_MODULE_SPECIFIER, "main-thread-runtime"),
    builtin_module!(ELEMENT_MODULE_SPECIFIER, "element-papi"),
    builtin_module!(TIMER_MODULE_SPECIFIER, "timers"),
    builtin_module!(FUTURE_MODULE_SPECIFIER, "future"),
    builtin_module!(SECTION_URL_MODULE_SPECIFIER, "section-url"),
    builtin_module!(BUNDLE_FETCH_MODULE_SPECIFIER, "bundle-fetch"),
    builtin_module!(REQUIRE_MODULE_SPECIFIER, "module"),
];

/// What `bobcat-workers` registers on the runtime its worker realms share:
/// the global scope, timers, `Future`, `require`, the shared event machinery
/// and the BTS runtime module. Each script chooses its own imports.
///
/// `bobcat:element` and `bobcat:runtime` are absent because a worker has no
/// document to reach and no page to be the main thread of, and registering
/// them would make an import that must fail merely fail late.
pub(crate) const WORKER_MODULES: &[BuiltinModule] = &[
    builtin_module!(SELECTOR_QUERY_SPECIFIER, "selector-query"),
    builtin_module!(LYNX_MODULES_SPECIFIER, "lynx-modules"),
    builtin_module!(GLOBAL_EVENT_MODULE_SPECIFIER, "global-event-emitter"),
    builtin_module!(EVENT_TARGET_MODULE_SPECIFIER, "event-target"),
    builtin_module!(WORKER_MODULE_SPECIFIER, "worker-runtime"),
    builtin_module!(CONTEXT_MODULE_SPECIFIER, "cross-thread-context"),
    builtin_module!(BTS_RUNTIME_MODULE_SPECIFIER, "background-thread-runtime"),
    builtin_module!(TIMER_MODULE_SPECIFIER, "timers"),
    builtin_module!(FUTURE_MODULE_SPECIFIER, "future"),
    builtin_module!(SECTION_URL_MODULE_SPECIFIER, "section-url"),
    builtin_module!(BUNDLE_FETCH_MODULE_SPECIFIER, "bundle-fetch"),
    builtin_module!(REQUIRE_MODULE_SPECIFIER, "module"),
];

/// Builds one of a group's two `QuickJS` runtimes, with `modules` registered
/// on it for every realm that will be opened there.
///
/// Both engine threads build theirs with this, and both keep an `Err` rather
/// than failing the group: it is the failure of every view or worker that
/// asks that runtime for a realm, each of which reports it as its own.
///
/// Sources are registered once per runtime rather than once per realm: a
/// runtime holds one source per name and compiles it into a module per realm,
/// so a second registration of a name would refuse the second realm.
pub(crate) fn build_runtime(modules: &[BuiltinModule]) -> Result<ScriptRuntime, ScriptError> {
    let mut runtime = ScriptRuntime::new()?;
    for module in modules {
        runtime
            .register_module_source(module.specifier, module.source)
            .map_err(|mut error| {
                error.message = Arc::from(format!(
                    "registering {} ({}): {}",
                    module.specifier, module.file, error.message
                ));
                error
            })?;
    }
    Ok(runtime)
}
