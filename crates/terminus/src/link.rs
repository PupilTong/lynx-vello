//! The link: the only paths an embedder names.
//!
//! Every other module of this crate is private and the crate root re-exports
//! nothing, so an item is named from outside through here or not at all.
//! Each item is re-exported by name, for one of two reasons: a crate built on
//! `terminus` names it (as do this crate's own integration tests, benchmarks
//! and examples, which build against the link like any other crate), or an
//! item here hands it out, so a type an embedder can hold is a type it can
//! write down. The submodules keep the names of the crate's own modules;
//! everything else is at the top.
//!
//! The crate denies `unreachable_pub`, so a `pub` item that nothing here
//! reaches fails the build: it is named here or it is `pub(crate)`.
//! `docs/terminus-link-api.md` records why each item is on the link.

pub use euclid::default::{Point2D, Rect, Size2D, Vector2D};
pub use seldon::text::{FontBlob, TextContext};
pub use stylo;
pub use stylo_dom::ElementState;
pub use vello_gpu::{kurbo, peniko};
pub use wgpu;

pub use crate::layout::TextLineReport;
pub use crate::paint::compose::FilterGroup;
pub use crate::paint::display_list::{
    DisplayList, DrawGlyphs, Embolden, Glyph, ImagePixels, ImageRef, NormalizedCoord, Texture,
};
#[cfg(feature = "devtools")]
pub use crate::paint::highlight::NodeHighlight;
pub use crate::render::image::{
    DocumentKind, DocumentRequest, FrameImage, FrameImages, ImageDescriptor, ImageEvent,
    ImageInbox, ImageReports, ImageResidency, ImageSizeHint, ImageStatus, MAX_RENDERABLE_DIMENSION,
    NoImages, is_renderable,
};
pub use crate::render::picture::{
    FrameTarget, FrameWriter, Loops, MediaCommand, MediaError, MediaErrorKind, MediaEvent,
    MediaSettings, PictureFrame, PictureId, PictureKey, PictureSlot, PlayPolicy, PlaybackControl,
    PlaybackReport, PlaybackRequest, TakenFrame,
};
pub use crate::style::animation::AnimationTick;
pub use crate::style::device::Device;
#[doc(hidden)]
pub use crate::style::device::standards_device;
pub use crate::style::engine::{
    CssDeclaration, CssKeyframe, CssRule, FontFaceRequest, FontFaceSource, ShadowSheet,
    StylesheetOrigin, UserAgentStylesheet,
};
#[cfg(feature = "devtools")]
pub use crate::style::matched::{MatchedDeclaration, MatchedRule, MatchedSelector, MatchedStyles};
pub use crate::style::pool::{MAX_STYLE_THREADS, StylePool, StylePoolError, StyleWorker};
pub use crate::style::query::InvalidSelector;
pub use crate::svg::image::VectorDocument;
pub use crate::tree::custom::{CustomElement, MethodCall, MethodError, MethodOutcome};
pub use crate::tree::document::{Document, NodeId};
pub use crate::tree::footprint::MemoryFootprint;
#[doc(hidden)]
pub use crate::tree::node::ChildrenIter;
pub use crate::tree::node::Node;
pub use crate::tree::popover::{PopoverError, PopoverState};
pub use crate::tree::shadow::{PLACEHOLDER_PSEUDO_ID, ShadowRootMode};
pub use crate::tree::top_layer::TopLayerEntry;
pub use crate::visual::container_timing::ContainerPaint;
pub use crate::visual::frame::{ENCODE_WINDOW_SCROLLPORTS, RELEVANCE_REACH_SCROLLPORTS};
pub use crate::visual::intersection::{
    ElementHandler, IntersectionEventHandler, IntersectionGeometry, IntersectionMargin,
    IntersectionObserver, IntersectionObserverEntry, IntersectionObserverId,
    IntersectionObserverInit, IntersectionObserverRoot, MarginLength, RootGeometry,
};
pub use crate::visual::resize::{
    ObservedBox, ResizeEventHandler, ResizeObserver, ResizeObserverEntry, ResizeObserverId,
};
pub use crate::visual::timing::CommitTiming;
pub use crate::visual::{
    AnimationSlot, CommittedFrame, ContentVisibilityChange, HitTarget, SceneRef, ScrollSlot,
    SnapSlot, SnapSlotAxis,
};

pub mod clock {
    pub use crate::clock::{
        Alignment, AnswerWhenReady, ClockInstant, SpawnedClock, SpawnerClock, alignment_channel,
    };
}

pub mod editing {
    pub use crate::editing::{
        Command, Direction, DocumentSelection, EditEvent, EditEventKind, EditOutcome, EnterKeyHint,
        FocusEvent, FocusEventKind, Granularity, InputMode, InputType, PreparedEdit,
        TextInputState,
    };
}

pub mod event {
    pub use crate::event::{
        ElementEvent, EventDetail, EventPhase, EventStep, EventSteps, EventWalk, ListenerKey,
        ListenerKinds, QueuedEventSink, RegistrationChange, StepOutcome, TOUCH_ACTIVE,
        TOUCH_CHANGED, TOUCH_TARGET, TouchPoint, TouchPoints,
    };
}

pub mod input {
    pub use crate::input::{
        InputEvent, InputKind, KeyPhase, Modifiers, PointerId, PointerKind, PointerPhase,
    };
}

pub mod layout {
    pub use crate::layout::{Layout, NaturalSize, ParagraphKind, Size};
}

pub mod render {
    pub mod gpu {
        pub use crate::render::gpu::{GpuError, Headless};
    }

    pub mod blur {
        pub use crate::render::blur::MAX_FILTER_AREA;
    }

    #[cfg(target_arch = "wasm32")]
    pub mod canvas {
        pub use crate::render::canvas::{drain, set_waker};
    }

    #[doc(hidden)]
    pub mod strips {
        pub use crate::render::sparse::StripScene;
    }
}

pub mod scroll {
    pub use crate::scroll::{
        AutoScrollRequest, ScrollAxes, ScrollBehavior, ScrollBox, ScrollIntoViewContainer,
        ScrollIntoViewOptions, ScrollLogicalPosition, ScrollRequest, SnapAxis, SnapAxisPositions,
        SnapPoint, SnapPositions, SnapStrictness,
    };
}

pub mod html {
    pub mod dialog {
        pub use crate::html::dialog::{
            DIALOG_TAG, InvalidState, UA_RULES, close, request_close, show, show_modal,
        };
    }

    pub mod popover {
        pub use crate::html::popover::UA_RULES;
    }

    pub mod canvas {
        pub use crate::html::canvas::{
            CANVAS_TAG, CanvasError, CanvasFrames, Committed, CompositeOp, Context2d, ContextMode,
            Direction, FillRule, GenericFamily, Gradient, Image, ImageDataRead, InvalidState,
            LineCap, LineJoin, OffscreenCanvas, Path, Pattern, Placeholder, Repetition,
            ResolvedFamily, ResolvedFont, ResolvedFontStyle, SmoothingQuality, Style, TextAlign,
            TextBaseline, TextMetrics, context_mode, transfer_control_to_offscreen,
            update_placeholders,
        };
        pub use crate::render::canvas::ReplayUser;
    }
}

pub mod painter {
    #[cfg(feature = "painter-test-utils")]
    #[doc(hidden)]
    pub use crate::painter::{
        AUTO_SCROLL_MIN_INTERVAL_SECONDS, ScrollIntents, ScrollIntentsTestUtils, WHEEL_REST_SECONDS,
    };
    pub use crate::painter::{
        DrawTarget, EmitEvent, FrameClock, FrameSize, GesturePolicy, GestureRouter, InputDecision,
        InputDecisions, Painter, PainterError, PresentedFrame, RouterHost, Screenshot, ScrollEntry,
        Viewport, WindowFrame, WindowTarget,
    };
}
