//! The Lynx-owned payload carried by every widget ([`WidgetState`]), plus the
//! event-registration types.
//!
//! `w3c-dom`'s [`Node`](w3c_dom::Node) covers only the W3C-DOM
//! subset; Lynx-only identity and event registrations live in the node's
//! opaque payload. CSS scope (`l-css-id`) and dataset values are real DOM
//! attributes, so selector matching and invalidation never consult this
//! payload.

use std::sync::{PoisonError, RwLock, RwLockReadGuard};

use smallvec::SmallVec;

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

/// The Lynx-specific per-widget state, carried as the opaque payload of
/// [`Widget`](crate::Widget) (= `w3c_dom::Node<WidgetState>`).
#[derive(Debug)]
pub struct WidgetState {
    /// The widget kind (the Lynx tag classification).
    pub kind: WidgetKind,
    /// The Lynx `unique_id`, assigned by the
    /// [`WidgetTree`](crate::WidgetTree) on creation (1-based, monotonically
    /// increasing).
    pub unique_id: i32,
    /// Event bindings on this widget. Interior mutability lets the widget
    /// layer update its own payload through w3c-dom's read-only payload view;
    /// it does not mutate DOM or style-engine tree state.
    events: RwLock<SmallVec<[EventReg; 2]>>,
}

impl WidgetState {
    /// Create the state for a freshly created widget of `kind` with the given
    /// Lynx `unique_id`.
    #[must_use]
    pub fn new(kind: WidgetKind, unique_id: i32) -> Self {
        Self {
            kind,
            unique_id,
            events: RwLock::new(SmallVec::new()),
        }
    }
    /// Borrow this widget's event registrations.
    pub fn events(&self) -> RwLockReadGuard<'_, SmallVec<[EventReg; 2]>> {
        self.events.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Register an event without exposing mutable payload access through
    /// w3c-dom.
    pub(crate) fn push_event(&self, event: EventReg) {
        self.events
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}
