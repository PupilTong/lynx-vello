//! The Lynx-owned payload carried by every widget ([`WidgetState`]), plus the
//! event-registration types.

use std::sync::{PoisonError, RwLock, RwLockReadGuard};

use smallvec::SmallVec;

use crate::kind::WidgetKind;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventBindingKind {
    Bind,
    Catch,
    CaptureBind,
    CaptureCatch,
    GlobalBind,
}

/// A single event binding on a widget.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EventBinding {
    pub name: Box<str>,
    pub kind: EventBindingKind,
    pub handler: Box<str>,
}

/// The Lynx-specific per-widget state, stored in `w3c-dom`'s NodeId-indexed
/// payload arena and exposed as the opaque payload of [`Widget`](crate::Widget)
/// (= `w3c_dom::Node<WidgetState>`).
#[derive(Debug)]
pub struct WidgetState {
    pub kind: WidgetKind,
    pub unique_id: i32,
    events: RwLock<SmallVec<[EventBinding; 2]>>,
}

impl WidgetState {
    #[must_use]
    pub fn new(kind: WidgetKind, unique_id: i32) -> Self {
        Self {
            kind,
            unique_id,
            events: RwLock::new(SmallVec::new()),
        }
    }
    pub fn events(&self) -> RwLockReadGuard<'_, SmallVec<[EventBinding; 2]>> {
        self.events.read().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn push_event(&self, event: EventBinding) {
        self.events
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}
