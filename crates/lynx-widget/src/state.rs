//! The Lynx external state carried by every widget ([`WidgetState`]), plus the
//! event-registration types.
//!
//! `w3c-dom`'s [`Node`](w3c_dom::Node) covers only the W3C-DOM subset;
//! everything Lynx-specific about a widget — its [`WidgetKind`], the
//! `unique_id`, the `css_id` style scope, the `data-*` dataset, and event
//! bindings — lives here, in the node's `ext` payload. Selector-visible
//! counterparts such as `l-css-id` and `data-*` are real node attributes,
//! written by [`WidgetTree`](crate::WidgetTree) alongside this state.

use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use w3c_dom::ExternalState;

use crate::kind::WidgetKind;

/// Which bind/catch channel an event handler was authored on.
///
/// These mirror Lynx's five mutually-exclusive event attribute namespaces
/// (`bindEvent`, `catchEvent`, `capture-bind`, `capture-catch`,
/// `global-bindEvent`). The phase/propagation semantics they imply are the
/// runtime's concern; here they are stored verbatim.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventKind {
    /// `bind*` — bubble-phase listener.
    Bind,
    /// `catch*` — bubble-phase listener that also halts propagation.
    Catch,
    /// `capture-bind*` — capture-phase listener.
    CaptureBind,
    /// `capture-catch*` — capture-phase listener that also halts propagation.
    CaptureCatch,
    /// `global-bind*` — page/component-wide listener, no capture/bubble phase.
    GlobalBind,
}

/// A single event binding on a widget.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EventReg {
    /// The event name (e.g. `"tap"`).
    pub name: Box<str>,
    /// The channel this handler was authored on.
    pub kind: EventKind,
    /// Opaque handler identifier. A later runtime crate resolves this to an
    /// actual callback; for now it is stored as an uninterpreted string.
    pub handler: Box<str>,
}

/// The Lynx-specific per-widget state, carried as the `ext` payload of
/// [`Widget`](crate::Widget) (= `w3c_dom::Node<WidgetState>`).
#[derive(Debug)]
pub struct WidgetState {
    /// The widget kind (the Lynx tag classification).
    pub kind: WidgetKind,
    /// The Lynx `unique_id`, assigned by the
    /// [`WidgetTree`](crate::WidgetTree) on creation (1-based, monotonically
    /// increasing).
    pub unique_id: i32,
    /// The `css_id` scoping this widget's styles: `0` means unset / global.
    /// Stamped directly by Lynx's `__SetCSSId` (there is no `<component>`
    /// element in this engine to inherit it from). [`WidgetTree`](crate::WidgetTree)
    /// mirrors it into the real `l-css-id` attribute.
    pub css_id: i32,
    /// `data-*` dataset entries (keys stored without the `data-` prefix),
    /// mirrored into real `data-*` attributes by [`WidgetTree`](crate::WidgetTree).
    pub dataset: FxHashMap<Box<str>, String>,
    /// Event bindings on this widget.
    pub events: SmallVec<[EventReg; 2]>,
}

impl WidgetState {
    /// Create the state for a freshly created widget of `kind` with the given
    /// Lynx `unique_id`.
    #[must_use]
    pub fn new(kind: WidgetKind, unique_id: i32) -> Self {
        Self {
            kind,
            unique_id,
            css_id: 0,
            dataset: FxHashMap::default(),
            events: SmallVec::new(),
        }
    }
}

impl ExternalState for WidgetState {}
