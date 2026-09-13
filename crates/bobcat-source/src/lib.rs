//! Parse Lynx XML, web/native source bundles and ZIP resource archives.
//!
//! XML always borrows its input. Web decoding uses the rkyv 0.7 wire model;
//! native decoding and native-to-web conversion use that same model. All
//! parsers and Bobcat source registration are always available to every
//! embedder. This crate neither owns IO nor launches a view.
#![forbid(unsafe_code)]

mod archive;
mod custom_style;
mod lower_style;
pub mod native;
mod page;
pub mod web;
pub mod xml;
pub use archive::*;
pub use page::*;
