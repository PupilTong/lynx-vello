//! Native Lynx runtime core for lynx-vello.
//!
//! Hosts inject resources, normalized OS input, and draw-target capabilities
//! into an opaque [`LynxView`]. The script engine is not one of them: the core
//! owns the `QuickJS` runtime, the realms opened on it, the preloaded ESM
//! graph and the main-thread runtime outright, and the engine, document, and Element-PAPI tree
//! hand-off remain private runtime implementation.
//!
//! A view is two threads: the embedder's own — whichever one created the
//! [`LynxGroup`] it belongs to, which is where every draw happens — and the
//! Lynx main thread that group owns. Views in one group share that thread,
//! and with it one `QuickJS` runtime and one Stylo pool; views in different
//! groups share nothing.
//!
//! A view boots once, at construction: [`ViewSources`] carries everything it
//! runs on and [`LynxGroup::create_lynx_view`] does the rest. Decoded images are one of
//! those sources — the core neither fetches, decodes, caches nor retains a
//! single pixel. It asks the embedder's [`ResourceFetcher`](resource::ResourceFetcher),
//! which is the one resource system a view has, for them by source string:
//! named through `request_image`, answered through [`ImageReports`], and read
//! back synchronously while the frame composes.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

/// The private script boundary, exposed only so this crate's own benchmarks
/// can drive it. Not a contract: hidden, unstable, and free to change.
#[doc(hidden)]
pub mod bench_support;
#[path = "main/lib.rs"]
mod main;
#[path = "paint/lib.rs"]
mod paint;
pub mod resource;
pub mod script;
pub mod style;
#[path = "view/lib.rs"]
mod view;

/// Normalized host input vocabulary. No document or hit-test result crosses
/// this boundary.
pub mod input {
    pub use dom::Point2D;
    pub use dom::input::{InputEvent, InputKind, PointerId, PointerKind, PointerPhase};
}

pub use dom::{
    FontBlob, FrameImages, ImageEvent, ImageInbox, ImageReports, ImageSizeHint,
    MAX_RENDERABLE_DIMENSION, MAX_STYLE_THREADS, NoImages, is_renderable, vello,
};
pub use main::tree::PageConfig;
pub use style::{PreparsedDeclaration, PreparsedKeyframe, PreparsedRule, PreparsedStyleSheet};
#[cfg(target_arch = "wasm32")]
pub use view::configure_wasm_workers;
pub use view::{
    DrawTarget, EngineError, EngineEvent, EventRequester, FrameSize, LynxGroup, LynxView,
    LynxViewError, NoWakeup, Screenshot, StyleThreads, ViewSources, WindowTarget,
};
