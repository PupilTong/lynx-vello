//! Parse Lynx XML, web bundles, and source-based native external bundles.
//!
//! XML always borrows its input. `web-bundle` enables the rkyv 0.7 wire model;
//! `native-bundle` adds native decoding and native-to-web conversion using that
//! same model. `runtime` adds Bobcat source registration, without owning IO or
//! launching a view. Disable default features for a dependency-free XML parser.
#![forbid(unsafe_code)]

#[cfg(all(feature = "runtime", feature = "web-bundle"))]
mod lower_style;
#[cfg(feature = "native-bundle")]
pub mod native;
#[cfg(feature = "runtime")]
mod page;
#[cfg(feature = "web-bundle")]
pub mod web;
pub mod xml;
#[cfg(feature = "runtime")]
pub use page::*;
