//! Context-owned Lynx element storage.

use dom::{Document, Node, NodeId};

use crate::ElementId;

/// A Lynx element handle: the unique id the Element PAPI hands to and takes
/// from the main-thread script.
///
/// Ids start at 1 and advance with the arena's high-water mark. Retiring an
/// element leaves its arena entry empty forever, so an id can never name a
/// later element.
fn arena_index(id: ElementId) -> Option<usize> {
    let index = usize::try_from(id).ok()?;
    (index != 0).then_some(index)
}

/// One Lynx runtime element.
///
/// The actual [`Node<ElementId>`] allocation remains owned by
/// [`Document<ElementId>`]:
/// `dom` stores nodes in a reallocating slab, so retaining a node reference or
/// pointer here would be unsound. `LynxElement` owns the stable association via
/// [`NodeId`] and resolves the node against its document when needed.
#[derive(Debug)]
pub struct LynxElement {
    id: ElementId,
    node: NodeId,
    /// The `parentComponentUniqueID` creation argument. Recorded but not yet
    /// honored because CSS-scope support has not landed.
    parent_component: ElementId,
    /// The `componentCSSID` supplied when a page is created.
    component_css: i32,
}

impl LynxElement {
    #[must_use]
    pub const fn unique_id(&self) -> ElementId {
        self.id
    }

    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node
    }

    /// Resolves the DOM node this runtime element owns the association with.
    #[must_use]
    pub fn node<'document>(
        &self,
        document: &'document Document<ElementId>,
    ) -> Option<&'document Node<ElementId>> {
        document.get(self.node)
    }

    #[must_use]
    pub const fn parent_component_unique_id(&self) -> ElementId {
        self.parent_component
    }

    #[must_use]
    pub const fn component_css_id(&self) -> i32 {
        self.component_css
    }
}

/// The Lynx context's permanent-index element arena.
///
/// Every creation appends one `Some(LynxElement)`. Retirement takes the value
/// and permanently leaves `None`; neither that slot nor its unique id is ever
/// reused. Slot zero is the permanent "no element" sentinel, so every live
/// element's unique id is also its direct vector index.
#[derive(Debug)]
pub(crate) struct ElementArena {
    entries: Vec<Option<LynxElement>>,
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

    pub(crate) fn insert(
        &mut self,
        unique_id: ElementId,
        node_id: NodeId,
        parent_component_unique_id: ElementId,
        component_css_id: i32,
    ) -> ElementId {
        let arena_index = arena_index(unique_id).expect("a reserved element unique id is positive");
        assert_eq!(
            arena_index,
            self.entries.len(),
            "the permanent-index element arena only appends"
        );
        self.entries.push(Some(LynxElement {
            id: unique_id,
            node: node_id,
            parent_component: parent_component_unique_id,
            component_css: component_css_id,
        }));
        unique_id
    }

    pub(crate) fn get(&self, id: ElementId) -> Option<&LynxElement> {
        let element = self.entries.get(arena_index(id)?)?.as_ref()?;
        debug_assert_eq!(element.id, id);
        Some(element)
    }

    pub(crate) fn retire(&mut self, id: ElementId) -> Option<LynxElement> {
        let element = self.entries.get_mut(arena_index(id)?)?.take()?;
        debug_assert_eq!(element.id, id);
        Some(element)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}
