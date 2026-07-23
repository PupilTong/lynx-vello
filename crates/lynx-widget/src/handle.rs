//! [`WidgetHandle`] — the canonical, retention-bearing element handle.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use w3c_dom::NodeId;

/// A shared, cloneable handle to one widget in one [`WidgetTree`](crate::WidgetTree).
#[derive(Clone)]
pub struct WidgetHandle {
    pub(crate) inner: Arc<HandleInner>,
}

impl WidgetHandle {
    pub(crate) fn id(&self) -> NodeId {
        self.inner.id
    }

    pub(crate) fn belongs_to(&self, reaper: &Arc<Reaper>) -> bool {
        std::ptr::eq(self.inner.reaper.as_ptr(), Arc::as_ptr(reaper))
    }
}

impl PartialEq for WidgetHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for WidgetHandle {}

impl std::hash::Hash for WidgetHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.inner), state);
    }
}

impl std::fmt::Debug for WidgetHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WidgetHandle")
            .field("id", &self.inner.id)
            .finish_non_exhaustive()
    }
}

/// The canonical per-node allocation behind every [`WidgetHandle`] clone.
pub(crate) struct HandleInner {
    pub(crate) id: NodeId,
    reaper: Weak<Reaper>,
}

impl HandleInner {
    pub(crate) fn new(id: NodeId, reaper: &Arc<Reaper>) -> Self {
        Self {
            id,
            reaper: Arc::downgrade(reaper),
        }
    }
}

impl Drop for HandleInner {
    fn drop(&mut self) {
        if let Some(reaper) = self.reaper.upgrade() {
            reaper.note_dropped(self.id);
        }
    }
}

/// The drop-notification channel between handles and their tree.
pub(crate) struct Reaper {
    dropped: Mutex<Vec<NodeId>>,
    dirty: AtomicBool,
}

impl Reaper {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            dropped: Mutex::new(Vec::new()),
            dirty: AtomicBool::new(false),
        })
    }

    fn note_dropped(&self, id: NodeId) {
        self.dropped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(id);
        self.dirty.store(true, Ordering::Release);
    }

    pub(crate) fn take_dropped(&self) -> Option<Vec<NodeId>> {
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return None;
        }
        let mut queue = self
            .dropped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if queue.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut *queue))
    }
}

impl std::fmt::Debug for Reaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reaper")
            .field("dirty", &self.dirty.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}
