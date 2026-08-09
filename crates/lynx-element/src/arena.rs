//! Context-owned Lynx element storage.

use dom::NodeId;

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

/// One event binding recorded by `__AddEvent`.
///
/// Lynx keeps event bindings on the element itself rather than in the
/// attribute set, and web-core does the same (its bindings live in the
/// WASM element context, keyed by unique id, never as DOM attributes) — so
/// they must not become selector-visible here either.
///
/// The handler is the framework's cross-thread identifier string (`ReactLynx`
/// emits values like `"-3:0:"`). A `None` handler is web-core's "worklet
/// handler" case, which arrives as an object rather than a string; the
/// binding is still recorded, without the worklet payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventBinding {
    /// `bindEvent`, `catchEvent`, `capture-bind`, `capture-catch`.
    pub event_type: String,
    /// The Lynx event name, e.g. `tap`.
    pub name: String,
    /// The framework's cross-thread handler identifier.
    pub handler: Option<String>,
}

/// One Lynx runtime element.
///
/// The actual DOM node allocation remains owned by `Document<ElementId>`:
/// `dom` stores nodes in a reallocating slab, so retaining a node reference or
/// pointer here would be unsound. `LynxElement` owns the stable association via
/// [`NodeId`]; `ElementTree` resolves it against the document when needed.
#[derive(Debug)]
pub struct LynxElement {
    id: ElementId,
    node: NodeId,
    /// The `parentComponentUniqueID` creation argument. Recorded but not yet
    /// honored because CSS-scope support has not landed.
    parent_component: ElementId,
    /// The `componentCSSID` supplied when a page is created.
    component_css: i32,
    /// The runtime-owned DOM text node carrying this element's `text`
    /// attribute value.
    ///
    /// Lynx puts a text element's content in an attribute; `dom` measures and
    /// paints text only from DOM text nodes, so the attribute is materialized
    /// as one child node this layer owns. It is not a Lynx element: it carries
    /// the null unique id and is invisible to the Element PAPI.
    text_node: Option<NodeId>,
    /// `__AddEvent` bindings, in call order. Nothing dispatches them yet.
    events: Vec<EventBinding>,
}

impl LynxElement {
    #[must_use]
    pub const fn unique_id(&self) -> ElementId {
        self.id
    }

    /// Crate-internal: `NodeId` is `dom` vocabulary and stays out of this
    /// layer's public signatures — external observers speak `ElementId`.
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

    /// Binds the `componentCSSID` `__CreatePage` supplies to the pre-created
    /// page element.
    pub(crate) fn set_component_css_id(&mut self, component_css_id: i32) {
        self.component_css = component_css_id;
    }

    /// The runtime-owned text node materializing this element's `text`
    /// attribute, once one exists.
    #[must_use]
    pub(crate) const fn text_node(&self) -> Option<NodeId> {
        self.text_node
    }

    pub(crate) const fn set_text_node(&mut self, node: Option<NodeId>) {
        self.text_node = node;
    }

    /// The `__AddEvent` bindings recorded on this element, in call order.
    #[must_use]
    pub fn events(&self) -> &[EventBinding] {
        &self.events
    }

    /// Records one binding, replacing an existing one for the same
    /// type-and-name pair the way a re-render's repeated `__AddEvent` does.
    pub(crate) fn set_event(&mut self, binding: EventBinding) {
        match self.events.iter_mut().find(|existing| {
            existing.event_type == binding.event_type && existing.name == binding.name
        }) {
            Some(existing) => *existing = binding,
            None => self.events.push(binding),
        }
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
            text_node: None,
            events: Vec::new(),
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
