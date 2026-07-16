//! [`WidgetHandle`] — the canonical, retention-bearing element handle.
//!
//! The scripting engine owns widgets: each JS element wrapper holds one
//! `WidgetHandle` clone, and **a live handle is a liveness guarantee** — the
//! tree never frees a node while any handle into its subtree exists. That is
//! what raw ids cannot express, and why the PAPI surface traffics
//! exclusively in handles:
//!
//! - **Tree identity.** A handle records the [`Document`](w3c_dom::Document) token of the tree that
//!   minted it; using it against another [`WidgetTree`](crate::WidgetTree) is a typed
//!   [`WidgetError::ForeignWidget`](crate::WidgetError::ForeignWidget), never silent cross-tree
//!   aliasing.
//! - **Canonicality.** For each node there is at most one live `HandleInner`; every lookup for that
//!   node clones the same `Arc`, so `Arc` strong counts are exactly the number of outstanding
//!   external references — the retention signal reclamation is built on.
//! - **Drop-driven reclamation.** Dropping the last clone for a node pushes its id onto the tree's
//!   crate-private `Reaper` queue. At the next operation boundary the tree sweeps: a **detached**
//!   subtree in which *no* node has a live handle is freed atomically (slab entries + `unique_id`
//!   index). Attached nodes are never collected — the tree itself keeps document content alive,
//!   exactly like the browser, where the DOM tree retains its nodes and GC only ever collects
//!   *detached* ones nobody references.
//!
//! There is deliberately **no public disposal API**: freeing is a consequence
//! of ownership (drop your handles), not an opcode.

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use w3c_dom::NodeId;

/// A shared, cloneable handle to one widget in one [`WidgetTree`].
///
/// Cheap to clone (`Arc`); equality and hashing are **identity** — two
/// handles are equal exactly when they designate the same node of the same
/// tree. While any clone lives, the node (and, transitively, the detached
/// subtree containing it) is retained.
///
/// [`WidgetTree`]: crate::WidgetTree
#[derive(Clone)]
pub struct WidgetHandle {
    pub(crate) inner: Arc<HandleInner>,
}

impl WidgetHandle {
    /// The identity token of the tree this handle belongs to.
    #[must_use]
    pub fn tree_token(&self) -> NonZeroU64 {
        self.inner.tree_token
    }

    pub(crate) fn id(&self) -> NodeId {
        self.inner.id
    }
}

impl PartialEq for WidgetHandle {
    fn eq(&self, other: &Self) -> bool {
        // NodeId embeds the document token, so id equality is identity.
        self.inner.id == other.inner.id
    }
}

impl Eq for WidgetHandle {}

impl std::hash::Hash for WidgetHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.inner.id.hash(state);
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
    pub(crate) tree_token: NonZeroU64,
    pub(crate) id: NodeId,
    /// Where to report this node when the last clone drops. `Weak`: handles
    /// must not keep their tree's plumbing alive.
    reaper: Weak<Reaper>,
}

impl HandleInner {
    pub(crate) fn new(tree_token: NonZeroU64, id: NodeId, reaper: &Arc<Reaper>) -> Self {
        Self {
            tree_token,
            id,
            reaper: Arc::downgrade(reaper),
        }
    }
}

impl Drop for HandleInner {
    fn drop(&mut self) {
        // Last clone gone: the node is no longer externally retained. Queue
        // it; the owning tree decides at its next sweep whether that makes a
        // detached subtree collectible.
        if let Some(reaper) = self.reaper.upgrade() {
            reaper.note_dropped(self.id);
        }
    }
}

/// The drop-notification channel between handles and their tree.
///
/// Handles are dropped wherever the embedder pleases (including from wrapper
/// finalizers on other threads — `Arc`/`Mutex`, not `Rc`/`RefCell`, so the
/// tree stays `Send`); the tree drains the queue at operation boundaries.
pub(crate) struct Reaper {
    dropped: Mutex<Vec<NodeId>>,
    /// Cheap "anything queued?" flag so the per-operation sweep is a single
    /// relaxed load in the steady state.
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

    /// Take the queued drop notices, if any (clears the dirty flag first, so
    /// a drop racing the drain is picked up by the next sweep).
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
