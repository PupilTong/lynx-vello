//! HTTP screenshot service implemented as a native Bobcat embedder.
//!
//! The server owns URL loading, source-container decoding, request limits,
//! headless GPU attachment, frame pacing, image encoding, and HTTP lifecycle.
//! [`bobcat_core::LynxView`] continues to own the document, script realm,
//! layout, painting, and its engine threads.

mod bmp;
mod capture;
mod http;

pub use http::{ServerError, serve};
