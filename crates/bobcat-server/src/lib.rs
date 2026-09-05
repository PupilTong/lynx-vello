//! HTTP screenshot service implemented as a native Bobcat embedder.
//!
//! The server owns URL loading, source-container decoding, request limits,
//! headless GPU attachment, frame pacing, image encoding, and HTTP lifecycle.
//! [`bobcat_core::LynxView`] continues to own the document, script realm,
//! layout, painting, and its engine threads.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

mod bmp;
mod capture;
mod server;

pub use server::{ServerError, serve};
