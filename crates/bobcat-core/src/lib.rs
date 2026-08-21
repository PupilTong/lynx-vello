//! Native Lynx runtime core for lynx-vello.
//!
//! Hosts inject resources, a JavaScript VM factory, normalized OS input, and
//! draw-target capabilities into an opaque [`LynxView`]. The engine, document,
//! and Element-PAPI tree hand-off remain private runtime implementation.
//!
//! [`image`] is the replaced-content decode contract and private loader — the
//! engine owns that pipeline, while the codec itself is a host-implemented
//! contract ([`image::Decoder`]; `bobcat-cli` carries reference codecs).
//! Wiring the codec into the future Lynx `<image>` element remains pending.
//!
//! The default `quickjs` feature adds Bobcat's internal `QuickJS` adapter and
//! main-thread runtime. Disable default features to use only the external
//! engine contracts without compiling `QuickJS`.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(target_arch = "wasm32", feature(stdarch_wasm_atomic_wait))]

pub mod clock;
mod engine;
pub mod image;
#[cfg(feature = "quickjs")]
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
    pub use dom::input::{DeltaMode, InputEvent, InputKind, PointerId, PointerKind, PointerPhase};
}

pub use clock::{AnimationClock, ManualClock, SystemClock};
#[cfg(target_arch = "wasm32")]
pub use engine::configure_wasm_workers;
pub use engine::{
    EngineError, EngineEvent, EventRequester, FrameRequester, FrameSize, NoWindow, Screenshot,
    ScriptRunError, Window, WindowTarget,
};
#[cfg(feature = "quickjs")]
pub use quickjs::engine_factory as quickjs_engine_factory;
pub use style::{PreparsedDeclaration, PreparsedKeyframe, PreparsedRule, PreparsedStyleSheet};
pub use tree::PageConfig;
pub use view::{LynxView, LynxViewError, OffscreenLynxView};
