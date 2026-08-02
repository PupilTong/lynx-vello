//! `pulsar` — Vello resources and GPU submission for the DOM paint pipeline.
//!
//! This crate deliberately knows nothing about DOM nodes, computed styles,
//! layout, paint order, or frame scheduling. It owns the decoded [`ImageStore`]
//! and [`gpu`] device/readback implementation, and re-exports its
//! version-matched [`vello`] types. The `dom` crate depends on these primitives
//! and keeps its document-aware painter private.
//!
//! This is an internal engine boundary, not the product embedder API. Product
//! callers use `bobcat_core::renderer` and never configure Vello/wgpu or submit
//! a scene themselves.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod gpu;
mod images;

pub use images::ImageStore;
/// Internal rendering layers share wgpu/peniko/kurbo exclusively through this
/// version-matched re-export.
pub use vello;
