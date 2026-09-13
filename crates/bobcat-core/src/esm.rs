//! The module names both kinds of realm agree on.
//!
//! A source is registered per `QuickJS` runtime, so a group registers each of
//! these twice: once on the runtime its views' realms share, once on the one
//! its workers share. The names are here, beside [`crate::clock`], because
//! neither runtime owns them.

/// The `packages/bobcat-element/src` module `$name` as the JavaScript a realm
/// evaluates, compiled by `build.rs` into this Cargo build's `OUT_DIR`.
macro_rules! runtime_source {
    ($name:literal) => {
        include_str!(concat!(env!("OUT_DIR"), "/runtime/", $name, ".js"))
    };
}
pub(crate) use runtime_source;

/// The native module every realm's Rust-backed members are exported from.
pub(crate) const HOST_MODULE_SPECIFIER: &str = "bobcat-internal:host";

/// The timer runtime: the realm's half of `setTimeout` and its three
/// companions, imported for its effect.
pub(crate) const TIMER_MODULE_SPECIFIER: &str = "bobcat:timers";
pub(crate) const TIMER_MODULE_SOURCE: &str = runtime_source!("timers");

/// The `EventTarget` a view's `lynx.getEngine()` and a worker's global scope
/// are both built on.
pub(crate) const EVENT_TARGET_MODULE_SPECIFIER: &str = "bobcat:event-target";
pub(crate) const EVENT_TARGET_SOURCE: &str = runtime_source!("event-target");

/// Lynx's typed asynchronous Context channel, shared by MTS and BTS.
pub(crate) const CONTEXT_MODULE_SPECIFIER: &str = "bobcat:cross-thread-context";
pub(crate) const CONTEXT_MODULE_SOURCE: &str = runtime_source!("cross-thread-context");

/// The built-in BTS bootstrap, loaded like any other Worker script.
pub(crate) const BTS_MODULE_SPECIFIER: &str = "bobcat:bts";

/// BTS bindings live separately from the bootstrap that awaits the app entry.
pub(crate) const BTS_RUNTIME_MODULE_SPECIFIER: &str = "bobcat:bts-runtime";
pub(crate) const BTS_RUNTIME_MODULE_SOURCE: &str = runtime_source!("background-thread-runtime");

/// Named imports prepended to a BTS application entry, as for MTS. The
/// bootstrap uses the same import to initialize the Context before the app.
pub(crate) const BTS_ENTRY_PREAMBLE: &str = "import { lynx } from \"bobcat:bts-runtime\";\n";
