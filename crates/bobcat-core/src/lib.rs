//! Native Lynx runtime core for lynx-vello.
//!
//! The always-compiled modules are implementation-neutral: hosts inject a
//! [`script::ScriptEngine`], [`resource::ResourceFetcher`], and an
//! [`element::ElementPapi`] implementation through static generic contracts.
//! [`document`] specializes the generic DOM with Pulsar injected at document
//! construction, so a runtime document owns its scene builder and image store.
//!
//! The default `quickjs` feature adds Bobcat's internal `QuickJS` adapter and
//! main-thread runtime. Disable default features to use only the external
//! engine contracts without compiling `QuickJS`.

pub mod document;
pub mod element;
#[cfg(feature = "quickjs")]
pub mod quickjs;
pub mod resource;
pub mod script;
pub mod view;

pub use dom;
pub use pulsar;
