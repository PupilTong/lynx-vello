//! Native Lynx runtime core for lynx-vello.
//!
//! Hosts inject resources into an opaque [`LynxView`], and normalized OS
//! input and a draw target into a [`Painter`]. The script engine is not one
//! of them: the core owns the `QuickJS` runtime, the realms opened on it, the
//! preloaded ESM graph and the main-thread runtime outright, and the engine,
//! document, and Element-PAPI tree hand-off remain private runtime
//! implementation.
//!
//! A view is two threads: the embedder's own — whichever one created the
//! [`LynxGroup`] it belongs to — and the Lynx main thread that group owns.
//! Views in one group share that thread, and with it one `QuickJS` runtime
//! and one Stylo pool; views in different groups share nothing.
//!
//! A view boots once, at construction: [`ViewSources`] carries everything it
//! runs on and [`LynxGroup::create_lynx_view`] does the rest. [`LynxView::reload`]
//! recreates the framework's component state inside those same realms; it does
//! not evaluate the entry again. Decoded images are one of
//! those sources — the core neither fetches, decodes, caches nor retains a
//! single pixel. It asks the embedder's [`ResourceFetcher`](resource::ResourceFetcher),
//! which is the one resource system a view has, for them by source string:
//! named through `request_image`, answered through [`ImageReports`], and read
//! back synchronously while the frame composes.
//!
//! Where the pixels go is a separate object. A [`Painter`] is built over one
//! [`DrawTarget`], on the thread that will draw into it, and observes a view
//! for as long as it is attached to one — at most one painter per view. A
//! view can run with none, and a painter goes on showing its last frame after
//! the view it was watching is gone.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// Proving a wgpu handle `Send`/`Sync` walks wgpu-core's whole context type
// (`Texture` → `CoreTexture` → `ContextWgpuCore` → ...), deeper than the
// default limit of 128; nightly reports that overflow as the future-error
// `recursion_depth_exceeding_limit` (rust-lang/rust#159228).
#![recursion_limit = "256"]

#[cfg(target_arch = "wasm32")]
mod alarm;
#[path = "background/lib.rs"]
mod background;
#[doc(hidden)]
pub mod bench_support;
mod canvas;
mod clock;
mod code_cache;
#[cfg(feature = "devtools")]
mod devtools;
mod esm;
mod fetch;
mod future;
mod jobs;
mod lifetime;
mod link;
#[path = "main/lib.rs"]
mod main;
pub mod native_module;
#[path = "paint/lib.rs"]
mod paint;
pub mod performance;
mod realm;
mod require;
pub mod resource;
pub mod script;
mod script_frames;
pub mod style;
#[cfg(test)]
mod test_support;
mod threads;
mod timers;
#[path = "view/lib.rs"]
mod view;

pub mod input {
    pub use terminus::link::Point2D;
    pub use terminus::link::input::{
        InputEvent, InputKind, KeyPhase, Modifiers, PointerId, PointerKind, PointerPhase,
    };
}

pub mod editing {
    pub use terminus::link::Rect;
    pub use terminus::link::editing::{
        Command, Direction, EnterKeyHint, Granularity, InputMode, InputType, TextInputState,
    };

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum TextInputSource {
        #[default]
        Keys,
        Commands,
    }
}

pub use code_cache::{MIN_CACHED_SOURCE_BYTES, script_engine_id};
#[cfg(feature = "devtools")]
pub use devtools::{DevtoolsSession, DevtoolsSink};
pub use esm::{BTS_CHUNK_PREAMBLE, MTS_CHUNK_PREAMBLE};
pub use main::tree::{PageConfig, document_tag};
pub use native_module::{
    ModuleCall, ModuleCallback, NativeModule, NativeModuleEsm, compile_native_module_esm,
};
pub use paint::{Painter, PostedFrame};
pub use style::{PreparsedDeclaration, PreparsedKeyframe, PreparsedRule, PreparsedStyleSheet};
pub use terminus::link::clock::ClockInstant;
pub use terminus::link::{
    DocumentKind, FontBlob, FrameImage, FrameImages, FrameTarget, FrameWriter, ImageDescriptor,
    ImageEvent, ImageInbox, ImageReports, ImageResidency, ImageSizeHint, Loops,
    MAX_RENDERABLE_DIMENSION, MAX_STYLE_THREADS, MediaCommand, MediaError, MediaErrorKind,
    MediaEvent, MediaSettings, NoImages, PictureFrame, PictureId, PictureKey, PictureSlot,
    PlayPolicy, PlaybackControl, PlaybackReport, PlaybackRequest, TakenFrame, VectorDocument,
    is_renderable, kurbo, peniko, wgpu,
};
#[cfg(target_arch = "wasm32")]
pub use view::configure_wasm_workers;
pub use view::{
    DrawTarget, EngineError, EngineEvent, EventRequester, FrameSize, LynxGroup, LynxView,
    LynxViewError, NoWakeup, PainterError, ScreenMetrics, Screenshot, ScriptSource, StyleThreads,
    ViewSources, WindowTarget, WorkerId, preload_system_fonts,
};
