//! Native Lynx runtime core for lynx-vello.
//!
//! Hosts inject resources, normalized OS input, and draw-target capabilities
//! into an opaque [`LynxView`]. The script engine is not one of them: the core
//! owns its `QuickJS` realm, its preloaded ESM graph, and its main-thread
//! runtime outright, and the engine, document, and Element-PAPI tree hand-off
//! remain private runtime implementation.
//!
//! Decoded images are one of those injected resources. The core neither
//! fetches, decodes, caches nor retains a single pixel: an embedder installs a
//! [`ImageStore`] with [`LynxView::set_image_store`], and the view asks it for
//! images by source string. Wiring the store into the future Lynx `<image>`
//! element remains pending.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

/// The private script boundary, exposed only so this crate's own benchmarks
/// can drive it. Not a contract: hidden, unstable, and free to change.
#[doc(hidden)]
pub mod bench_support;
mod clock;
mod engine;
mod gesture;
mod quickjs;
pub mod resource;
mod runtime;
pub mod script;
pub mod style;
mod tree;
mod view;

/// Normalized host input vocabulary. No document or hit-test result crosses
/// this boundary.
pub mod input {
    pub use dom::Point2D;
    pub use dom::input::{InputEvent, InputKind, PointerId, PointerKind, PointerPhase};
}

pub use dom::{ImageFuture, ImageStore, ImageStoreError};
#[cfg(target_arch = "wasm32")]
pub use engine::configure_wasm_workers;
pub use engine::{
    EngineError, EngineEvent, EventRequester, FrameRequester, FrameSize, NoWindow, Screenshot,
    Window, WindowTarget,
};
pub use style::{PreparsedDeclaration, PreparsedKeyframe, PreparsedRule, PreparsedStyleSheet};
pub use tree::PageConfig;
pub use view::{LynxView, LynxViewError, OffscreenLynxView};
