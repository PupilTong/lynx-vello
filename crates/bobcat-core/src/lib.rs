//! Native Lynx runtime core for lynx-vello.
//!
//! The protocol modules let hosts inject a [`script::ScriptEngine`] and
//! [`resource::ResourceFetcher`] through static generic contracts.
//! [`lynx_element`] is re-exported whole as the strict layer chain's door
//! downward.
//!
//! The default `quickjs` feature adds Bobcat's internal `QuickJS` adapter and
//! main-thread runtime. Disable default features to use only the external
//! engine contracts without compiling `QuickJS`.

/// The layer below, re-exported whole: the product reaches `ElementTree`,
/// `dom`, and the render stack exclusively through this door.
pub use lynx_element;

#[cfg(feature = "quickjs")]
pub mod quickjs;
pub mod resource;
pub mod script;
pub mod view;
