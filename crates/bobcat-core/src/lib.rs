//! Native Lynx runtime core for lynx-vello.
//!
//! The protocol modules let hosts inject a [`script::ScriptEngine`],
//! [`resource::ResourceFetcher`], and native [`platform`] services through
//! static contracts.
//! [`lynx_element`] is re-exported whole as the strict layer chain's door
//! downward.
//!
//! [`image`] is the replaced-content decode contract and loader — the engine
//! owns that pipeline, while the codec itself is a third injected contract
//! ([`image::Decoder`], implemented by the embedder — `bobcat-cli` carries
//! the reference implementations).
//!
//! The default `quickjs` feature adds Bobcat's internal `QuickJS` adapter and
//! main-thread runtime. Disable default features to use only the external
//! engine contracts without compiling `QuickJS`.

// The coverage run compiles with `--cfg coverage_nightly` and the test modules
// opt out via `#[coverage(off)]`, which needs this experimental feature (same
// pattern as every other workspace crate).
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

/// The layer below, re-exported whole: the product reaches `ElementTree`,
/// `dom`, and the render stack exclusively through this door.
pub use lynx_element;

pub mod engine;
pub mod image;
pub mod platform;
#[cfg(feature = "quickjs")]
pub mod quickjs;
pub mod resource;
pub mod script;
pub mod view;
