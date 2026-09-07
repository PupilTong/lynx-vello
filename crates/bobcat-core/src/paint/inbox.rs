//! The group's one main → painter FIFO, read only on the host thread.
//!
//! A turn may belong to any view. Route the received batch by id and retain
//! other views' messages until their turns, without creating more channels.

use std::cell::RefCell;
use std::time::Duration;

use rustc_hash::FxHashMap;

use crate::view::{GroupNotification, ToPainter, ViewId};

pub(crate) struct GroupInbox {
    receiver: flume::Receiver<GroupNotification>,
    pending: RefCell<FxHashMap<ViewId, Vec<ToPainter>>>,
}

impl GroupInbox {
    pub(crate) fn new(receiver: flume::Receiver<GroupNotification>) -> Self {
        Self {
            receiver,
            pending: RefCell::new(FxHashMap::default()),
        }
    }

    pub(crate) fn register(&self, view: ViewId) {
        self.pending.borrow_mut().insert(view, Vec::new());
    }

    pub(crate) fn remove(&self, view: ViewId) {
        self.pending.borrow_mut().remove(&view);
    }

    fn route(pending: &mut FxHashMap<ViewId, Vec<ToPainter>>, message: GroupNotification) {
        // A dropped view has no entry. Discard late notifications without
        // recreating its buffer or retaining its resources.
        if let Some(messages) = pending.get_mut(&message.view) {
            messages.push(message.notification);
        }
    }

    pub(crate) fn drain(&self, view: ViewId, mut apply: impl FnMut(ToPainter)) {
        let mut pending = self.pending.borrow_mut();
        for notification in pending
            .get_mut(&view)
            .expect("a live link registers its view")
            .drain(..)
        {
            apply(notification);
        }
        for message in self.receiver.drain() {
            if message.view == view {
                // The current painter consumes directly; only siblings need
                // temporary storage, whose capacity survives their turns.
                apply(message.notification);
            } else {
                Self::route(&mut pending, message);
            }
        }
    }

    /// Offscreen ticks wait on the same FIFO while preserving other views'
    /// notifications. No borrow is held while the main thread is awaited.
    pub(crate) fn wait(&self, timeout: Duration) -> bool {
        let Ok(message) = self.receiver.recv_timeout(timeout) else {
            return false;
        };
        Self::route(&mut self.pending.borrow_mut(), message);
        true
    }

    pub(crate) fn is_disconnected(&self) -> bool {
        self.receiver.is_disconnected()
    }
}
