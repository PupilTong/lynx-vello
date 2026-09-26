//! The built-in modules both runtimes register, and the module names both
//! kinds of realm agree on.
//!
//! Both engine threads register the whole of [`BUILTIN_MODULES`] on their
//! runtime, so a group holds each source twice: once on the runtime its views'
//! realms share, once on the one its workers share. What a realm can use of
//! them is decided by the host modules it declares: an import of a host module
//! the realm does not declare fails with a `ReferenceError`, and an import of a
//! member its host module lacks fails at link with a `SyntaxError`. A name
//! under [`ENGINE_MODULE_PREFIXES`] that is neither registered nor declared
//! fails in the realm that asked with a `ReferenceError`, and is never
//! fetched. The names are here, beside [`crate::clock`], because neither
//! runtime owns them.

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

/// The native module a realm reaches the embedder's native modules through,
/// which every realm kind declares: `invokeNativeModule`, and nothing else.
/// The view's module table is not here: it is the MTS realm's startup member
/// `nativeModuleTable`, which the MTS realm posts to its BTS in `initialize`.
/// [`crate::native_module::install`] is what installs it.
pub(crate) const NATIVE_MODULES_HOST_SPECIFIER: &str = "bobcat-internal:native-modules";

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

/// A realm's `console` and `reportError`: the one value formatting and the one
/// `lynx.reportError` level rule every realm kind reports with, over the two
/// members [`crate::realm`] installs in every realm's core.
pub(crate) const DIAGNOSTICS_MODULE_SPECIFIER: &str = "bobcat:diagnostics";

/// The `EventTarget` a view's `lynx.getEngine()` and a worker's global scope
/// are both built on.
pub(crate) const EVENT_TARGET_MODULE_SPECIFIER: &str = "bobcat:event-target";

/// A realm's animation-frame callbacks and its one frame demand, for every
/// realm kind: the module both [`RUNTIME_MODULE_SPECIFIER`] and
/// [`BTS_RUNTIME_MODULE_SPECIFIER`] take `lynx.requestAnimationFrame` from,
/// and the one whose `__BobcatBeginFrame` both engine threads call to deliver
/// a vsync to a realm that asked for one.
pub(crate) const ANIMATION_FRAME_MODULE_SPECIFIER: &str = "bobcat:animation-frame";

/// The one builder of a realm's `SystemInfo`, where its runtime constants are
/// written.
pub(crate) const SYSTEM_INFO_MODULE_SPECIFIER: &str = "bobcat:system-info";

/// The one reader of the `<utf16Length>:<text>` records the host writes: the
/// Element PAPI's attribute and style answers, and the native module table
/// the BTS is posted in `initialize`.
pub(crate) const RECORD_MODULE_SPECIFIER: &str = "bobcat:record";

/// The native module transport, for every realm kind: `callNativeModule`,
/// which hands one call to [`NATIVE_MODULES_HOST_SPECIFIER`]'s
/// `invokeNativeModule`, and [`NATIVE_MODULE_CALLBACK_EXPORT`], which both
/// engine threads call with a module's answer. `bobcat:bts-runtime` builds
/// the BTS's `NativeModules` over it.
pub(crate) const NATIVE_MODULES_MODULE_SPECIFIER: &str = "bobcat:native-modules";

/// Called on [`NATIVE_MODULES_MODULE_SPECIFIER`] with one native module
/// callback's answer.
pub(crate) const NATIVE_MODULE_CALLBACK_EXPORT: &str = "__BobcatNativeModuleCallback";

pub(crate) const GLOBAL_EVENT_MODULE_SPECIFIER: &str = "bobcat:global-event-emitter";

/// Lynx's typed asynchronous Context channel, shared by MTS and BTS.
pub(crate) const CONTEXT_MODULE_SPECIFIER: &str = "bobcat:cross-thread-context";

/// The URL of a view's background thread, and the BTS bootstrap: the BTS is
/// the worker at this URL, and its realm's root module imports it as every
/// worker's root imports its URL. A registered module, which hands
/// `bobcat:bts-runtime` the loader of the view's BTS entry.
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

/// The worker realm's global-scope module.
pub(crate) const WORKER_MODULE_SPECIFIER: &str = "bobcat:worker";

/// The name a worker realm's root module is evaluated under: once per worker,
/// as the realm opens, and never registered, the way `bobcat:boot` is an MTS
/// realm's. A worker's `import` of its script is resolved against it, and the
/// script's URL is absolute, so it resolves to itself.
///
/// Inside the worker's realm the name is the root module itself, which
/// `QuickJS` finds among the realm's loaded modules before the loader is
/// asked, so no `ReferenceError` refuses it. A worker constructed over this
/// URL imports its own root while that root is still evaluating, so it never
/// finishes its boot: it reports nothing and holds what is posted to it until
/// it is terminated. Nothing guards against this: app code has no reason to
/// name an engine module.
pub(crate) const WORKER_BOOT_SPECIFIER: &str = "bobcat:worker-boot";

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

/// The name prefixes of the engine's own modules. A runtime reserves both: a
/// name under one is answered from the runtime's registered sources and the
/// importing realm's host modules, and from nothing else.
///
/// [`WORKER_CLASS_MODULE_SPECIFIER`] has no colon, so neither covers it; both
/// runtimes register it, so an import of it never reaches a fetcher either.
pub(crate) const ENGINE_MODULE_PREFIXES: [&str; 2] = ["bobcat:", "bobcat-internal:"];

/// Every built-in module. Both runtimes register all of them, and a realm's
/// host modules decide which of them it can link.
///
/// In the order of the `paths` of `packages/bobcat-element/src/tsconfig.json`,
/// which maps each specifier to the same file; a test holds the two equal.
pub(crate) const BUILTIN_MODULES: &[BuiltinModule] = &[
    builtin_module!(LYNX_MODULES_SPECIFIER, "lynx-modules"),
    builtin_module!(GLOBAL_EVENT_MODULE_SPECIFIER, "global-event-emitter"),
    builtin_module!(SELECTOR_QUERY_SPECIFIER, "selector-query"),
    builtin_module!(ELEMENT_MODULE_SPECIFIER, "element-papi"),
    builtin_module!(RUNTIME_MODULE_SPECIFIER, "main-thread-runtime"),
    builtin_module!(TIMER_MODULE_SPECIFIER, "timers"),
    builtin_module!(FUTURE_MODULE_SPECIFIER, "future"),
    builtin_module!(REQUIRE_MODULE_SPECIFIER, "module"),
    builtin_module!(SECTION_URL_MODULE_SPECIFIER, "section-url"),
    builtin_module!(BUNDLE_FETCH_MODULE_SPECIFIER, "bundle-fetch"),
    builtin_module!(DIAGNOSTICS_MODULE_SPECIFIER, "diagnostics"),
    builtin_module!(EVENT_TARGET_MODULE_SPECIFIER, "event-target"),
    builtin_module!(ANIMATION_FRAME_MODULE_SPECIFIER, "animation-frame"),
    builtin_module!(SYSTEM_INFO_MODULE_SPECIFIER, "system-info"),
    builtin_module!(RECORD_MODULE_SPECIFIER, "record"),
    builtin_module!(NATIVE_MODULES_MODULE_SPECIFIER, "native-modules"),
    builtin_module!(CONTEXT_MODULE_SPECIFIER, "cross-thread-context"),
    builtin_module!(WORKER_CLASS_MODULE_SPECIFIER, "worker"),
    builtin_module!(WORKER_MODULE_SPECIFIER, "worker-runtime"),
    builtin_module!(BTS_RUNTIME_MODULE_SPECIFIER, "background-thread-runtime"),
    builtin_module!(BTS_MODULE_SPECIFIER, "bts"),
];

/// Builds one of a group's two `QuickJS` runtimes: every built-in module
/// registered on it for every realm that will be opened there, and the
/// [`ENGINE_MODULE_PREFIXES`] reserved to those and to each realm's host
/// modules.
///
/// Both engine threads build theirs with this, and both keep an `Err` rather
/// than failing the group: it is the failure of every view or worker that
/// asks that runtime for a realm, each of which reports it as its own.
///
/// Sources are registered once per runtime rather than once per realm: a
/// runtime holds one source per name and compiles it into a module per realm,
/// so a second registration of a name would refuse the second realm.
pub(crate) fn build_runtime() -> Result<ScriptRuntime, ScriptError> {
    let mut runtime = ScriptRuntime::new()?;
    for module in BUILTIN_MODULES {
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
    for prefix in ENGINE_MODULE_PREFIXES {
        runtime.reserve_module_prefix(prefix).map_err(|mut error| {
            error.message = Arc::from(format!("reserving {prefix}: {}", error.message));
            error
        })?;
    }
    Ok(runtime)
}

#[cfg(test)]
mod tests {
    use super::BUILTIN_MODULES;

    /// What type-checks the built-ins: its `paths` resolve each specifier to
    /// the file the realm registers under it, one entry per line.
    const TSCONFIG: &str = include_str!("../../../packages/bobcat-element/src/tsconfig.json");

    /// The table a runtime registers and the `paths` the TypeScript is
    /// checked against name the same files under the same specifiers, in the
    /// same order, so neither can gain or rename a module alone.
    #[test]
    fn the_built_in_table_is_the_tsconfig_paths() {
        let paths: Vec<(&str, &str)> = TSCONFIG
            .lines()
            .map(str::trim)
            .skip_while(|line| *line != r#""paths": {"#)
            .skip(1)
            .take_while(|line| *line != "}")
            .map(|line| {
                let (specifier, file) = line
                    .split_once(": ")
                    .unwrap_or_else(|| panic!("one path per line: {line}"));
                let file = file
                    .trim_end_matches(',')
                    .strip_prefix(r#"["./"#)
                    .and_then(|file| file.strip_suffix(r#""]"#))
                    .unwrap_or_else(|| panic!("one file per path: {line}"));
                (specifier.trim_matches('"'), file)
            })
            .collect();
        let table: Vec<(&str, &str)> = BUILTIN_MODULES
            .iter()
            .map(|module| (module.specifier, module.file))
            .collect();
        assert_eq!(table, paths);
    }
}
