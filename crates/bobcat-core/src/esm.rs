//! The built-in modules both runtimes register, and the module names both
//! kinds of realm agree on.
//!
//! Both engine threads register the whole of [`BUILTIN_MODULES`] on their
//! runtime, as the bytecode `build.rs` compiled each module to: a realm that
//! imports one reads the module back rather than compiling its text, and the
//! bytes themselves are this binary's, borrowed by every runtime. What a realm
//! can use of them is decided by the host modules it declares: an import of a host module
//! the realm does not declare fails with a `ReferenceError`, and an import of a
//! member its host module lacks fails at link with a `SyntaxError`. A name
//! under [`ENGINE_MODULE_PREFIXES`] that is neither registered nor declared
//! fails in the realm that asked with a `ReferenceError`, and is never
//! fetched. A `lynx:` name ([`NATIVE_MODULE_ESM_PREFIX`]) is never fetched
//! either: it is an embedder's native module's ES module, which a worker
//! realm reads from the view's bytecode and the main runtime reserves. The
//! names are here, beside [`crate::clock`], because neither runtime owns
//! them.

use std::sync::Arc;

use crate::main::quickjs::ScriptRuntime;
use crate::script::ScriptError;

macro_rules! runtime_bytecode {
    ($name:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/runtime/", $name, ".qbc"))
    };
}

pub(crate) struct BuiltinModule {
    pub(crate) specifier: &'static str,
    pub(crate) file: &'static str,
    pub(crate) bytecode: &'static [u8],
}

macro_rules! builtin_module {
    ($specifier:expr, $name:literal) => {
        BuiltinModule {
            specifier: $specifier,
            file: concat!($name, ".ts"),
            bytecode: runtime_bytecode!($name),
        }
    };
}

pub(crate) const HOST_MODULE_SPECIFIER: &str = "bobcat-internal:host";

pub(crate) const NATIVE_MODULES_HOST_SPECIFIER: &str = "bobcat-internal:native-modules";

pub(crate) const TIMER_MODULE_SPECIFIER: &str = "bobcat:timers";

pub(crate) const FUTURE_MODULE_SPECIFIER: &str = "bobcat:future";

pub(crate) const REQUIRE_MODULE_SPECIFIER: &str = "bobcat:module";

pub(crate) const DIAGNOSTICS_MODULE_SPECIFIER: &str = "bobcat:diagnostics";

pub(crate) const EVENT_TARGET_MODULE_SPECIFIER: &str = "bobcat:event-target";

pub(crate) const ANIMATION_FRAME_MODULE_SPECIFIER: &str = "bobcat:animation-frame";

pub(crate) const SYSTEM_INFO_MODULE_SPECIFIER: &str = "bobcat:system-info";

pub(crate) const RECORD_MODULE_SPECIFIER: &str = "bobcat:record";

pub(crate) const NATIVE_MODULES_MODULE_SPECIFIER: &str = "bobcat:native-modules";

pub(crate) const GLOBAL_EVENT_MODULE_SPECIFIER: &str = "bobcat:global-event-emitter";

pub(crate) const CONTEXT_MODULE_SPECIFIER: &str = "bobcat:cross-thread-context";

pub(crate) const BTS_MODULE_SPECIFIER: &str = "bobcat:bts";

pub(crate) const ELEMENT_MODULE_SPECIFIER: &str = "bobcat:element";

pub(crate) const RUNTIME_MODULE_SPECIFIER: &str = "bobcat:runtime";

pub(crate) const FRAME_MODULE_SPECIFIER: &str = "bobcat:frame";

pub(crate) const FRAME_BOOT_MODULE_SPECIFIER: &str = "bobcat:frame-boot";

pub(crate) const WORKER_CLASS_MODULE_SPECIFIER: &str = "bobcat-internal";

pub(crate) const WORKER_MODULE_SPECIFIER: &str = "bobcat:worker";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Export {
    RunTimer,
    SettleFuture,
    NativeModuleCallback,
    BeginFrame,
    DeliverWorkerMessage,
    DispatchWorkerEvent,
    PerformanceReady,
    InitEntry,
    Reload,
    UpdateData,
    UpdateGlobalProps,
    SendGlobalEvent,
    DispatchEvent,
    LoadMetrics,
}

impl Export {
    pub(crate) const COUNT: usize = Self::LoadMetrics as usize + 1;

    pub(crate) const fn specifier(self) -> &'static str {
        match self {
            Self::RunTimer => TIMER_MODULE_SPECIFIER,
            Self::SettleFuture => FUTURE_MODULE_SPECIFIER,
            Self::NativeModuleCallback => NATIVE_MODULES_MODULE_SPECIFIER,
            Self::BeginFrame => ANIMATION_FRAME_MODULE_SPECIFIER,
            Self::DeliverWorkerMessage => WORKER_MODULE_SPECIFIER,
            Self::DispatchWorkerEvent => WORKER_CLASS_MODULE_SPECIFIER,
            Self::PerformanceReady => BTS_RUNTIME_MODULE_SPECIFIER,
            Self::InitEntry
            | Self::Reload
            | Self::UpdateData
            | Self::UpdateGlobalProps
            | Self::SendGlobalEvent => RUNTIME_MODULE_SPECIFIER,
            Self::DispatchEvent => ELEMENT_MODULE_SPECIFIER,
            Self::LoadMetrics => FRAME_BOOT_MODULE_SPECIFIER,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::RunTimer => "__BobcatRunTimer",
            Self::SettleFuture => "__BobcatSettleFuture",
            Self::NativeModuleCallback => "__BobcatNativeModuleCallback",
            Self::BeginFrame => "__BobcatBeginFrame",
            Self::DeliverWorkerMessage => "__BobcatDeliverWorkerMessage",
            Self::DispatchWorkerEvent => "__BobcatDispatchWorkerEvent",
            Self::PerformanceReady => "__BobcatPerformanceReady",
            Self::InitEntry => "__BobcatInitEntry",
            Self::Reload => "__BobcatReload",
            Self::UpdateData => "__BobcatUpdateData",
            Self::UpdateGlobalProps => "__BobcatUpdateGlobalProps",
            Self::SendGlobalEvent => "__BobcatSendGlobalEvent",
            Self::DispatchEvent => "__BobcatDispatchEvent",
            Self::LoadMetrics => "__BobcatLoadMetrics",
        }
    }
}

pub(crate) const LYNX_MODULES_SPECIFIER: &str = "bobcat:lynx-modules";

pub const MTS_CHUNK_PREAMBLE: &str = concat!(
    "import { __Card__, lynx, console, SystemInfo, __globalProps, NativeModules, ",
    "_AddEventListener, _ReportError, _SetSourceMapRelease, __OnLifecycleEvent, ",
    "__LoadLepusChunk, __LoadStyleSheet, __AdoptStyleSheet } from \"bobcat:runtime\"; ",
    "import { __CreatePage, __CreateElement, __CreateWrapperElement, __CreateText, ",
    "__CreateImage, __CreateView, __CreateScrollView, __CreateRawText, __CreateList, ",
    "__CreateFrame, ",
    "__AppendElement, __InsertElementBefore, __RemoveElement, __ReplaceElement, ",
    "__ReplaceElements, __SwapElement, __SetClasses, __SetID, __GetID, __GetTag, ",
    "__GetChildren, __GetAttributeByName, __GetAttributeNames, __GetElementUniqueID, ",
    "__SetDataset, __GetDataset, __AddDataset, __SetInlineStyles, __AddInlineStyle, ",
    "__SetCSSId, __SetAttribute, __UpdateListCallbacks, __AddEvent, __GetEvent, ",
    "__GetEvents, __SetEvents, __AddEventListener, __RemoveEventListener, ",
    "__StopPropagation, __StopImmediatePropagation, __CreateEvent, __DispatchEvent, ",
    "__GetPageElement, __QuerySelector, ",
    "__QuerySelectorAll, __InvokeUIMethod, __GetComputedStyleByKey, __FlushElementTree ",
    "} from \"bobcat:element\"; ",
);

pub(crate) const BTS_RUNTIME_MODULE_SPECIFIER: &str = "bobcat:bts-runtime";

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

pub(crate) const SECTION_URL_MODULE_SPECIFIER: &str = "bobcat:section-url";

pub(crate) const BUNDLE_FETCH_MODULE_SPECIFIER: &str = "bobcat:bundle-fetch";

pub(crate) const PERFORMANCE_MODULE_SPECIFIER: &str = "bobcat:performance";

pub(crate) const SELECTOR_QUERY_SPECIFIER: &str = "bobcat:selector-query";

pub(crate) const ENGINE_MODULE_PREFIXES: [&str; 2] = ["bobcat:", "bobcat-internal:"];

pub(crate) const NATIVE_MODULE_ESM_PREFIX: &str = "lynx:";

pub(crate) const BUILTIN_MODULES: &[BuiltinModule] = &[
    builtin_module!(LYNX_MODULES_SPECIFIER, "lynx-modules"),
    builtin_module!(GLOBAL_EVENT_MODULE_SPECIFIER, "global-event-emitter"),
    builtin_module!(SELECTOR_QUERY_SPECIFIER, "selector-query"),
    builtin_module!(ELEMENT_MODULE_SPECIFIER, "element-papi"),
    builtin_module!(RUNTIME_MODULE_SPECIFIER, "main-thread-runtime"),
    builtin_module!(FRAME_MODULE_SPECIFIER, "frame"),
    builtin_module!(FRAME_BOOT_MODULE_SPECIFIER, "frame-boot"),
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
    builtin_module!(PERFORMANCE_MODULE_SPECIFIER, "performance"),
    builtin_module!(NATIVE_MODULES_MODULE_SPECIFIER, "native-modules"),
    builtin_module!(CONTEXT_MODULE_SPECIFIER, "cross-thread-context"),
    builtin_module!(WORKER_CLASS_MODULE_SPECIFIER, "worker"),
    builtin_module!(WORKER_MODULE_SPECIFIER, "worker-runtime"),
    builtin_module!(BTS_RUNTIME_MODULE_SPECIFIER, "background-thread-runtime"),
    builtin_module!(BTS_MODULE_SPECIFIER, "bts"),
];

pub(crate) fn build_runtime() -> Result<ScriptRuntime, ScriptError> {
    let mut runtime = ScriptRuntime::new()?;
    for module in BUILTIN_MODULES {
        runtime
            .register_module_bytecode(module.specifier, module.bytecode)
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

pub(crate) fn build_main_runtime() -> Result<ScriptRuntime, ScriptError> {
    let mut runtime = build_runtime()?;
    runtime
        .reserve_module_prefix(NATIVE_MODULE_ESM_PREFIX)
        .map_err(|mut error| {
            error.message = Arc::from(format!(
                "reserving {NATIVE_MODULE_ESM_PREFIX}: {}",
                error.message
            ));
            error
        })?;
    Ok(runtime)
}

#[cfg(test)]
mod tests {
    use super::BUILTIN_MODULES;

    const TSCONFIG: &str = include_str!("../../../packages/bobcat-element/src/tsconfig.json");

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
