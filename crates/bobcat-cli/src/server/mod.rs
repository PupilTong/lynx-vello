//! HTTP screenshot service implemented as a native Bobcat embedder.
//!
//! The server owns URL loading, source-container decoding, request limits,
//! headless GPU attachment, frame pacing, image encoding, and HTTP lifecycle.
//! [`bobcat_core::LynxView`] continues to own the document, script realm,
//! layout, painting, and its engine threads.

mod bmp;
mod capture;
mod form;
mod http;
mod remote;
mod tos;

pub use http::{ServerError, serve, serve_with_native_modules, serve_with_view_factory};

/// Per-view native modules together with the builder for their resource fetcher.
/// Both are constructed on the capture thread and may share thread-local state.
pub struct CaptureView<B> {
    pub native_modules: Vec<Box<dyn bobcat_core::NativeModule>>,
    pub resources: B,
    /// Optional host work to finish after settling and before the final draw.
    /// The server continues pumping the view while awaiting the returned future.
    pub before_capture: Option<BeforeCapture>,
}
/// A per-view, one-shot capture barrier on the capture owner thread.
pub type BeforeCapture =
    Box<dyn FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>>;
pub use tos::TosConfigError;

impl<B> std::fmt::Debug for CaptureView<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureView")
            .field("module_count", &self.native_modules.len())
            .finish_non_exhaustive()
    }
}
