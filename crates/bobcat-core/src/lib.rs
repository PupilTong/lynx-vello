//! Native Lynx runtime core for lynx-vello.
//!
//! The protocol modules let hosts inject a [`script::ScriptEngine`] and
//! [`resource::ResourceFetcher`] through static generic contracts. [`dom`] is
//! re-exported whole as the strict layer chain's door downward, and [`tree`]
//! is the native element tree the JavaScript Element PAPI runtime
//! (`packages/bobcat-element`) drives through the `bobcat` realm global.
//!
//! [`image`] is the replaced-content decode contract and loader — the engine
//! owns that pipeline, while the codec itself is a third injected contract
//! ([`image::Decoder`], implemented by the embedder — `bobcat-cli` carries
//! the reference implementations).
//!
//! The default `quickjs` feature adds Bobcat's internal `QuickJS` adapter and
//! main-thread runtime. Disable default features to use only the external
//! engine contracts without compiling `QuickJS`.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub use dom;

pub mod engine;
pub mod image;
#[cfg(feature = "quickjs")]
pub mod quickjs;
pub mod resource;
pub mod script;
pub mod tree;
pub mod view;
