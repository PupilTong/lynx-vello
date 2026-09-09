//! The module names both kinds of realm agree on.
//!
//! A source is registered per `QuickJS` runtime, so a group registers each of
//! these twice: once on the runtime its views' realms share, once on the one
//! its workers share. The names are here, beside [`crate::clock`], because
//! neither runtime owns them.

/// The native module every realm's Rust-backed members are exported from.
pub(crate) const HOST_MODULE_SPECIFIER: &str = "bobcat-internal:host";

/// The timer runtime: the realm's half of `setTimeout` and its three
/// companions, imported for its effect.
pub(crate) const TIMER_MODULE_SPECIFIER: &str = "bobcat:timers";
pub(crate) const TIMER_MODULE_SOURCE: &str =
    include_str!("../../../packages/bobcat-element/src/timers.mjs");

/// The `EventTarget` a view's `lynx.getEngine()` and a worker's global scope
/// are both built on.
pub(crate) const EVENT_TARGET_MODULE_SPECIFIER: &str = "bobcat:event-target";
pub(crate) const EVENT_TARGET_SOURCE: &str =
    include_str!("../../../packages/bobcat-element/src/event-target.mjs");

/// Lynx's typed asynchronous Context channel, shared by MTS and BTS.
pub(crate) const CONTEXT_MODULE_SPECIFIER: &str = "bobcat:cross-thread-context";
pub(crate) const CONTEXT_MODULE_SOURCE: &str =
    include_str!("../../../packages/bobcat-element/src/cross-thread-context.mjs");

/// The built-in BTS entry, loaded like any other Worker script. It installs
/// Lynx bindings in JavaScript; the worker protocol carries no realm kind.
pub(crate) const BTS_MODULE_SPECIFIER: &str = "bobcat:bts";
pub(crate) const BTS_MODULE_SOURCE: &str =
    include_str!("../../../packages/bobcat-element/src/background-thread-runtime.mjs");
