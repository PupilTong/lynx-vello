//! Context-owned Lynx element-handle storage.

use dom::NodeId;

use crate::ElementId;

/// A Lynx element handle's direct arena index.
fn arena_index(id: ElementId) -> Option<usize> {
    let index = usize::try_from(id).ok()?;
    (index != 0).then_some(index)
}

/// The Lynx context's permanent-index element arena.
///
/// Every creation appends one DOM [`NodeId`]. Retirement takes the value and
/// permanently leaves `None`; neither that slot nor its unique id is ever
/// reused. Slot zero is the permanent "no element" sentinel, so every live
/// element's unique id is also its direct vector index.
#[derive(Debug)]
pub(crate) struct ElementArena {
    entries: Vec<Option<NodeId>>,
}

impl ElementArena {
    pub(crate) fn new() -> Self {
        Self {
            entries: vec![None],
        }
    }

    /// Computes the identity of the next append without consuming it.
    pub(crate) fn reserve(&self) -> ElementId {
        u32::try_from(self.entries.len())
            .expect("a Lynx element arena exhausted its u32 unique ids")
    }

    pub(crate) fn insert(&mut self, unique_id: ElementId, node_id: NodeId) -> ElementId {
        let arena_index = arena_index(unique_id).expect("a reserved element unique id is positive");
        assert_eq!(
            arena_index,
            self.entries.len(),
            "the permanent-index element arena only appends"
        );
        self.entries.push(Some(node_id));
        unique_id
    }

    pub(crate) fn get(&self, id: ElementId) -> Option<NodeId> {
        self.entries.get(arena_index(id)?).copied().flatten()
    }

    pub(crate) fn retire(&mut self, id: ElementId) -> Option<NodeId> {
        self.entries.get_mut(arena_index(id)?)?.take()
    }
}
