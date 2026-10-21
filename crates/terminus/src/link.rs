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
pub use vello;

pub use crate::layout::TextLineReport;
pub use crate::paint::compose::FilterGroup;
pub use crate::render::image::{
    DocumentKind, DocumentRequest, FrameImages, ImageEvent, ImageInbox, ImageOutcome, ImageReports,
    ImageRole, ImageSizeHint, MAX_RENDERABLE_DIMENSION, NoImages, is_renderable,
};
pub use crate::render::svg::VectorDocument;
pub use crate::style::animation::AnimationTick;
pub use crate::style::device::Device;
#[doc(hidden)]
pub use crate::style::device::standards_device;
pub use crate::style::engine::{
    CssDeclaration, CssKeyframe, CssRule, FontFaceRequest, FontFaceSource, StylesheetOrigin,
};
pub use crate::style::pool::{MAX_STYLE_THREADS, StylePool, StylePoolError, StyleWorker};
pub use crate::style::query::InvalidSelector;
pub use crate::tree::custom::{CustomElement, MethodCall, MethodError, MethodOutcome};
pub use crate::tree::document::{Document, NodeId};
#[doc(hidden)]
pub use crate::tree::node::ChildrenIter;
pub use crate::tree::node::Node;
pub use crate::tree::popover::{PopoverError, PopoverState};
pub use crate::tree::shadow::{PLACEHOLDER_PSEUDO_ID, ShadowRootMode};
pub use crate::tree::top_layer::TopLayerEntry;
pub use crate::visual::frame::ENCODE_WINDOW_SCROLLPORTS;
pub use crate::visual::intersection::{
    ElementHandler, IntersectionEventHandler, IntersectionGeometry, IntersectionMargin,
    IntersectionObserver, IntersectionObserverEntry, IntersectionObserverId,
    IntersectionObserverInit, IntersectionObserverRoot, MarginLength, RootGeometry,
};
pub use crate::visual::resize::{
    ObservedBox, ResizeEventHandler, ResizeObserver, ResizeObserverEntry, ResizeObserverId,
};
pub use crate::visual::{
    AnimationSlot, CommittedFrame, ContentVisibilityChange, HitTarget, SceneRef, ScrollSlot,
    SnapSlot, SnapSlotAxis,
};

/// Focus, the editing commands a host sends a focused editing host, and the
/// text-input state it reports.
pub mod editing {
    pub use crate::editing::{
        Command, Direction, DocumentSelection, EditEvent, EditEventKind, EditOutcome, EnterKeyHint,
        FocusEvent, FocusEventKind, Granularity, InputMode, InputType, PreparedEdit,
        TextInputState,
    };
}

/// Event paths, the events the engine dispatches to elements, and the
/// listener registrations a document reports.
pub mod event {
    pub use crate::event::{
        ElementEvent, EventDetail, EventPhase, EventStep, EventSteps, EventWalk, ListenerKey,
        ListenerKinds, QueuedEventSink, RegistrationChange, StepOutcome, TOUCH_ACTIVE,
        TOUCH_CHANGED, TOUCH_TARGET, TouchPoint, TouchPoints,
    };
}

/// The host's normalized input: pointer, wheel and key events.
pub mod input {
    pub use crate::input::{
        InputEvent, InputKind, KeyPhase, Modifiers, PointerId, PointerKind, PointerPhase,
    };
}

/// Layout results and the natural sizes a host reports.
pub mod layout {
    pub use crate::layout::{Layout, NaturalSize, ParagraphKind, Size};
}

/// Headless GPU rendering, and the area limit of a `filter: blur()` bake.
pub mod render {
    /// Headless rendering and readback.
    pub mod gpu {
        pub use crate::render::gpu::{GpuError, Headless};
    }

    /// The `filter: blur()` bake.
    pub mod blur {
        pub use crate::render::blur::MAX_FILTER_AREA;
    }
}

/// CSSOM-View scrolling and CSS scroll snap.
pub mod scroll {
    pub use crate::scroll::{
        AutoScrollRequest, CaptureAxes, ChainLink, ScrollAxes, ScrollAxis, ScrollBehavior,
        ScrollBox, ScrollCapture, ScrollIntoViewContainer, ScrollIntoViewOptions,
        ScrollLogicalPosition, ScrollRequest, SnapAxis, SnapAxisPositions, SnapPoint,
        SnapPositions, SnapStrictness,
    };
}

/// HTML elements this crate implements over its own tree.
///
/// The order an embedder writes [`dialog::UA_RULES`] and
/// [`popover::UA_RULES`] in, among its own UA rules, is set out in the
/// crate's `html` module (`src/html/mod.rs`, "The UA rules").
pub mod html {
    /// HTML's `<dialog>` element.
    pub mod dialog {
        pub use crate::html::dialog::{
            DIALOG_TAG, InvalidState, UA_RULES, close, request_close, show, show_modal,
        };
    }

    /// The UA rules HTML gives the `popover` attribute.
    pub mod popover {
        pub use crate::html::popover::UA_RULES;
    }
}

/// The presenting painter: input routing, scrolling, composition and one
/// draw target, over the frames a document commits.
pub mod painter {
    #[doc(hidden)]
    pub use crate::painter::ScrollIntents;
    #[cfg(feature = "painter-test-utils")]
    #[doc(hidden)]
    pub use crate::painter::{AUTO_SCROLL_MIN_INTERVAL_SECONDS, WHEEL_REST_SECONDS};
    pub use crate::painter::{
        DrawTarget, EmitEvent, FrameClock, FrameSize, GesturePolicy, GestureRouter, InputDecision,
        InputDecisions, Painter, PainterError, RouterHost, Screenshot, ScrollEntry, Viewport,
        WindowFrame, WindowTarget,
    };
}
