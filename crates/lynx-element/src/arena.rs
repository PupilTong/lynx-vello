//! Context-owned Lynx element storage.

use dom::NodeId;

use crate::ElementId;

fn arena_index(id: ElementId) -> Option<usize> {
    let index = usize::try_from(id).ok()?;
    (index != 0).then_some(index)
}

/// A runtime element associated with a stable DOM node and Element PAPI id.
#[derive(Debug)]
pub struct LynxElement {
    id: ElementId,
    node: NodeId,
    parent_component: ElementId,
    component_css: i32,
}

impl LynxElement {
    #[must_use]
    pub const fn unique_id(&self) -> ElementId {
        self.id
    }

    #[must_use]
    pub(crate) const fn node_id(&self) -> NodeId {
        self.node
    }

    #[must_use]
    pub const fn parent_component_unique_id(&self) -> ElementId {
        self.parent_component
    }

    #[must_use]
    pub const fn component_css_id(&self) -> i32 {
        self.component_css
    }

    pub(crate) fn set_component_css_id(&mut self, component_css_id: i32) {
        self.component_css = component_css_id;
    }
}

/// Permanent-index storage for runtime elements.
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

    pub(crate) fn get_mut(&mut self, id: ElementId) -> Option<&mut LynxElement> {
        let element = self.entries.get_mut(arena_index(id)?)?.as_mut()?;
        debug_assert_eq!(element.id, id);
        Some(element)
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
