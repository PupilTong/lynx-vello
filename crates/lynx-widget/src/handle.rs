//! Owner-thread node handles exposed across the Widget/runtime boundary.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::{Rc, Weak};

use stylo_dom::ElementId;

/// Per-tree identity shared by every handle minted by one [`WidgetTree`].
///
/// The allocation identity, rather than an integer, prevents a handle from
/// one view being accepted by another view whose document happens to use the
/// same internal slot index.
#[derive(Debug)]
pub(crate) struct TreeIdentity;

/// An opaque handle to one Widget node.
///
/// VM and application code hold this type only behind [`Rc`]. The internal
/// arena id is deliberately private: callers cannot manufacture, copy, or
/// replay a bare slot id. Clone the surrounding [`WidgetHandle`] for a strong
/// delayed reference, or use [`WeakNodeHandle`] for an explicitly fallible
/// delayed reference.
///
/// ```compile_fail
/// let mut tree = lynx_widget::WidgetTree::new();
/// let handle = tree.create_view();
/// let raw_arena_id = handle.id; // private by design
/// ```
pub struct NodeHandle {
    pub(crate) tree: Rc<TreeIdentity>,
    pub(crate) id: ElementId,
    unique_id: i32,
}

impl NodeHandle {
    pub(crate) fn new(tree: Rc<TreeIdentity>, id: ElementId, unique_id: i32) -> Self {
        Self {
            tree,
            id,
            unique_id,
        }
    }

    /// The Lynx `unique_id` associated with this node.
    ///
    /// This is application-visible identity, not the private arena slot id.
    #[must_use]
    pub const fn unique_id(&self) -> i32 {
        self.unique_id
    }
}

impl fmt::Debug for NodeHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NodeHandle")
            .field("unique_id", &self.unique_id)
            .finish_non_exhaustive()
    }
}

impl PartialEq for NodeHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.tree, &other.tree) && self.id == other.id
    }
}

impl Eq for NodeHandle {}

impl Hash for NodeHandle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Rc::as_ptr(&self.tree).hash(state);
        self.id.hash(state);
    }
}

/// The only strong node identity exposed by `lynx-widget`.
pub type WidgetHandle = Rc<NodeHandle>;

/// An explicitly fallible, non-owning node identity for deferred work.
pub type WeakNodeHandle = Weak<NodeHandle>;
