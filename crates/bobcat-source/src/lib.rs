//! Parse Lynx XML, web bundles, and source-based native external bundles.
//!
//! XML always borrows its input. Web decoding uses the rkyv 0.7 wire model;
//! native decoding and native-to-web conversion use that same model. All
//! parsers and Bobcat source registration are always available to every
//! embedder. This crate neither owns IO nor launches a view.
#![forbid(unsafe_code)]

mod lower_style;
pub mod native;
mod page;
pub mod web;
pub mod xml;
pub use page::*;
