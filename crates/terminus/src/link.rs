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
    CssDeclaration, CssKeyframe, CssRule, FontFaceRequest, FontFaceSource, ShadowSheet,
    StylesheetOrigin, UserAgentStylesheet,
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

/// The engine's monotonic clock, and the alignment that keeps it one clock
/// across threads.
///
/// Every instant this crate takes is a
/// [`ClockInstant`](crate::link::clock::ClockInstant). Natively that is
/// `std::time::Instant`, one clock for the whole process. On wasm32 each
/// Worker's clock starts from its own `performance.timeOrigin`, so a thread
/// that reads instants another thread subtracts is aligned once, at its
/// start, to the thread that spawned it: the spawner makes an
/// [`alignment_channel`](crate::link::clock::alignment_channel), the new
/// thread's first statement is
/// [`SpawnedClock::align_to_spawner`](crate::link::clock::SpawnedClock::align_to_spawner),
/// and the spawner answers through its
/// [`SpawnerClock`](crate::link::clock::SpawnerClock).
pub mod clock {
    pub use crate::clock::{
        Alignment, AnswerWhenReady, ClockInstant, SpawnedClock, SpawnerClock, alignment_channel,
    };
}

/// Focus, the editing commands a host sends a focused editing host, and the
/// text-input state it reports.
///
/// Plain-text editing in the browser's model: an element whose
/// `contenteditable` is present and not `"false"` is an editing host, and
/// its value is its direct text children. The document dispatches nothing,
/// so every event a change owes is reported for the embedder to dispatch —
/// focus changes through `Document::take_focus_events`, edit events in the
/// [`EditOutcome`](crate::link::editing::EditOutcome) of the
/// [`Command`](crate::link::editing::Command)
/// that caused them.
pub mod editing {
    pub use crate::editing::{
        Command, Direction, DocumentSelection, EditEvent, EditEventKind, EditOutcome, EnterKeyHint,
        FocusEvent, FocusEventKind, Granularity, InputMode, InputType, PreparedEdit,
        TextInputState,
    };
}

/// Event paths, the events the engine dispatches to elements, and the
/// listener registrations a document reports.
///
/// The document calls no script. It computes which nodes an event visits
/// and in which order (`Document::event_steps`), records who listens where
/// without the callbacks (`Document::add_listener`), and walks one path
/// ([`EventWalk`](crate::link::event::EventWalk)) while the embedder invokes each listener;
/// registrations that change whether a type has any listener at all are reported as
/// [`RegistrationChange`](crate::link::event::RegistrationChange)s.
pub mod event {
    pub use crate::event::{
        ElementEvent, EventDetail, EventPhase, EventStep, EventSteps, EventWalk, ListenerKey,
        ListenerKinds, QueuedEventSink, RegistrationChange, StepOutcome, TOUCH_ACTIVE,
        TOUCH_CHANGED, TOUCH_TARGET, TouchPoint, TouchPoints,
    };
}

/// The host's normalized input: pointer, wheel and key events.
///
/// An [`InputEvent`](crate::link::input::InputEvent) is plain data — where (viewport CSS px), what
/// kind of device, and what it just did — so a host adapter is a `match` and a
/// constructor call. The document only routes one (`Document::route_input`,
/// hit testing in paint order); naming, gestures and scrolling are the
/// presenting painter's ([`link::painter`](crate::link::painter)).
pub mod input {
    pub use crate::input::{
        InputEvent, InputKind, KeyPhase, Modifiers, PointerId, PointerKind, PointerPhase,
    };
}

/// Layout results and the natural sizes a host reports.
///
/// Box layout runs inside the document's commit, over `seldon`. What an
/// embedder sees of it is a node's rounded [`Layout`](crate::link::layout::Layout), and what it
/// supplies is the [`NaturalSize`](crate::link::layout::NaturalSize) of replaced content (an
/// image's pixels, a component's intrinsic size).
pub mod layout {
    pub use crate::layout::{Layout, NaturalSize, ParagraphKind, Size};
}

/// Headless GPU rendering, and the area limit of a `filter: blur()` bake.
///
/// The render floor knows no nodes, styles or layout: the document builds a
/// [`vello::Scene`] per commit and this floor
/// turns scenes into pixels, on the one workspace `vello` version this link
/// re-exports.
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
///
/// A scroll container is a box whose `overflow` is `scroll` or `hidden`
/// (`hidden` moves only when asked programmatically); `clip` is none. The
/// document keeps each container's committed offset, admitted against live
/// geometry on every read, and records programmatic requests
/// ([`ScrollRequest`](crate::link::scroll::ScrollRequest),
/// [`AutoScrollRequest`](crate::link::scroll::AutoScrollRequest)) in the frame it commits; the
/// presenting painter carries them out and owns the offsets a user's drag,
/// fling or wheel writes, which it posts back for the document to adopt.
pub mod scroll {
    pub use crate::scroll::{
        AutoScrollRequest, ScrollAxes, ScrollBehavior, ScrollBox, ScrollIntoViewContainer,
        ScrollIntoViewOptions, ScrollLogicalPosition, ScrollRequest, SnapAxis, SnapAxisPositions,
        SnapPoint, SnapPositions, SnapStrictness,
    };
}

/// HTML elements this crate implements over its own tree: HTML's `<dialog>`
/// element and the `popover` attribute's UA rules.
///
/// Neither is part of a document by itself. `Document::new` defines no
/// element, so an embedder that wants them calls
/// `Document::define_html_elements`, and adds the UA rules to its own UA
/// sheet.
///
/// # The UA rules
///
/// Every rule is HTML's selector inside `:where()`, so it has no
/// specificity: in one UA sheet with the embedder's rules, every embedder
/// rule that names a tag outranks it, as an author rule outranks a
/// browser's UA sheet. Among themselves the rules rank by source order
/// alone, and HTML's own precedence is the order an embedder writes them
/// in:
///
/// 1. `:where(dialog) { display: … }`, the display the embedder's block-like containers get, for
///    HTML's `block`, which no box here lowers to;
/// 2. [`dialog::UA_RULES`](crate::link::html::dialog::UA_RULES), in HTML's order;
/// 3. [`popover::UA_RULES`](crate::link::html::popover::UA_RULES), whose `[popover]` box beats
///    `dialog`'s as its higher specificity does in HTML;
/// 4. `:where(dialog:popover-open) { display: … }`, the same display, after the closed `dialog`'s
///    `none`, as in HTML.
///
/// The one pair whose order is then the reverse of HTML's, `dialog:modal`
/// before `[popover]`, writes the same values for every property both
/// declare, so no element can tell. HTML's one `!important`, the popover
/// `::backdrop`'s `pointer-events`, beats every author declaration, as a
/// UA `!important` does in a browser. A cascade layer would say the same,
/// but an `@layer` in a UA sheet trips Stylo's rule tree.
pub mod html {
    /// HTML's `<dialog>` element: its UA rules, a component that keeps
    /// `:open`, `:modal` and top-layer membership in step with the `open`
    /// attribute, and its four methods.
    ///
    /// The `open` attribute is the open state, and the value `"false"` reads
    /// as closed. [`show`](crate::link::html::dialog::show),
    /// [`show_modal`](crate::link::html::dialog::show_modal),
    /// [`close`](crate::link::html::dialog::close) and
    /// [`request_close`](crate::link::html::dialog::request_close) are HTML's `show()`,
    /// `showModal()`, `close()` and `requestClose()`; the component answers the same names
    /// through `Document::invoke_element_method`, where an
    /// [`InvalidState`](crate::link::html::dialog::InvalidState) is
    /// `MethodError::InvalidState`. `close` and `cancel` are queued to the
    /// document's `QueuedEventSink` for the embedder to fire.
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
///
/// A [`Painter`](crate::link::painter::Painter) is built on the thread that will draw and is
/// `!Send`, so that thread is its painting thread for its whole life; everything it owns
/// — a surface, a scene buffer, the gesture in progress — is touched from
/// there alone. It knows nothing of how frames reach it: its owner adopts
/// each committed frame with the pixels it draws (`Painter::adopt`), hands it
/// the host's input, and delivers the events it decides
/// ([`EmitEvent`](crate::link::painter::EmitEvent)) and
/// the scroll offsets it posts (`Painter::drain_changed`) to whatever runs
/// the document. The gesture names and thresholds are the owner's
/// ([`GesturePolicy`](crate::link::painter::GesturePolicy)). The frame is baked unscrolled, so a
/// scroll recomposes and re-hit-tests without a commit.
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
