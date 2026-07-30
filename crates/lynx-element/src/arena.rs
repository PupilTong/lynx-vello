//! Context-owned Lynx element storage.

use dom::{Document, Node, NodeId};

/// A Lynx element handle: the unique id the Element PAPI hands to and takes
/// from the main-thread script.
///
/// Ids start at 1 and advance with the arena's high-water mark. Retiring an
/// element leaves its arena entry empty forever, so an id can never name a
/// later element.
pub type ElementId = i32;

fn arena_index(id: ElementId) -> Option<usize> {
    let index = usize::try_from(id).ok()?;
    (index != 0).then_some(index)
}

/// One Lynx runtime element.
///
/// The actual [`Node<i32>`] allocation remains owned by [`Document<i32>`]:
/// `dom` stores nodes in a reallocating slab, so retaining a node reference or
/// pointer here would be unsound. `LynxElement` owns the stable association via
/// [`NodeId`] and resolves the node against its document when needed.
#[derive(Debug)]
pub struct LynxElement {
    unique_id: ElementId,
    node_id: NodeId,
    /// The `parentComponentUniqueID` creation argument. Recorded but not yet
    /// honored because CSS-scope support has not landed.
    parent_component_unique_id: ElementId,
    /// The `componentCSSID` supplied when a page is created.
    component_css_id: i32,
    /// Levels of descendants below this element; a leaf is `0`. Maintained on
    /// append so the depth guard costs no tree walk.
    pub(crate) height: u32,
}

impl LynxElement {
    #[must_use]
    pub const fn unique_id(&self) -> ElementId {
        self.unique_id
    }

    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }

    /// Resolves the DOM node this runtime element owns the association with.
    #[must_use]
    pub fn node<'document>(
        &self,
        document: &'document Document<i32>,
    ) -> Option<&'document Node<i32>> {
        document.get(self.node_id)
    }

    #[must_use]
    pub const fn parent_component_unique_id(&self) -> ElementId {
        self.parent_component_unique_id
    }

    #[must_use]
    pub const fn component_css_id(&self) -> i32 {
        self.component_css_id
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
        i32::try_from(self.entries.len())
            .expect("a Lynx element arena exhausted its positive i32 unique ids")
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
            unique_id,
            node_id,
            parent_component_unique_id,
            component_css_id,
            height: 0,
        }));
        unique_id
    }

    pub(crate) fn get(&self, id: ElementId) -> Option<&LynxElement> {
        let element = self.entries.get(arena_index(id)?)?.as_ref()?;
        debug_assert_eq!(element.unique_id, id);
        Some(element)
    }

    pub(crate) fn get_mut(&mut self, id: ElementId) -> Option<&mut LynxElement> {
        let element = self.entries.get_mut(arena_index(id)?)?.as_mut()?;
        debug_assert_eq!(element.unique_id, id);
        Some(element)
    }

    pub(crate) fn retire(&mut self, id: ElementId) -> Option<LynxElement> {
        let element = self.entries.get_mut(arena_index(id)?)?.take()?;
        debug_assert_eq!(element.unique_id, id);
        Some(element)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}
