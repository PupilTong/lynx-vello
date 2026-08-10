//! Context-owned Lynx element storage.

use dom::NodeId;

use crate::value::PapiValue;
use crate::{ElementId, INHERITED_CSS_ID_NONE};

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

/// The creation-time fields an element is born with.
///
/// Grouped rather than passed positionally because every `__Create*` member
/// supplies a different subset and a five-argument constructor invites the
/// silent transposition this type prevents.
pub(crate) struct ElementSeed {
    pub(crate) node: NodeId,
    pub(crate) parent_component: ElementId,
    pub(crate) css_id: i32,
    pub(crate) component_css_id: i32,
    pub(crate) component_id: Option<String>,
    pub(crate) entry_name: Option<String>,
    /// The `raw-text` mirror text node — see [`crate::tree::ElementTree::create_raw_text`].
    pub(crate) text_mirror: Option<NodeId>,
}

impl ElementSeed {
    /// The seed of an ordinary element: no component identity of its own,
    /// inheriting whatever CSS scope its parent component carries.
    pub(crate) const fn plain(node: NodeId, parent_component: ElementId, css_id: i32) -> Self {
        Self {
            node,
            parent_component,
            css_id,
            // Only a component-creating member seeds a scope for its
            // descendants; an ordinary element hands `0` down whatever its
            // own scope is.
            component_css_id: INHERITED_CSS_ID_NONE,
            component_id: None,
            entry_name: None,
            text_mirror: None,
        }
    }
}

/// One Lynx runtime element.
///
/// The actual DOM node allocation remains owned by `Document<ElementId>`:
/// `dom` stores nodes in a reallocating slab, so retaining a node reference or
/// pointer here would be unsound. `LynxElement` owns the stable association via
/// [`NodeId`]; `ElementTree` resolves it against the document when needed.
///
/// Everything web-core keeps in its per-element `LynxElementData` side table
/// rather than in the DOM lives here for the same reason: `dom` derives
/// selector matching only from real DOM state, so runtime bookkeeping that
/// author CSS must not see cannot be stored as an attribute.
#[derive(Debug)]
pub struct LynxElement {
    id: ElementId,
    node: NodeId,
    /// The `parentComponentUniqueID` creation argument. Recorded verbatim;
    /// its only *honored* effect is the CSS-scope inheritance below.
    parent_component: ElementId,
    /// The element's own CSS fragment id — web-core's `l-css-id`. Set by
    /// `__SetCSSId`, by a component/page creation argument, and otherwise
    /// inherited from `parent_component` at creation.
    css_id: i32,
    /// The CSS fragment id this element seeds into elements created with its
    /// handle as `parentComponentUniqueID`.
    ///
    /// Separate from `css_id` because web-core keeps the two apart
    /// (`element_data.rs:26-27`): `create_element_common` reads the parent
    /// component's `component_css_id` (`main_thread_context.rs:88-99`) while
    /// `set_css_id` writes only `css_id` (`style_apis.rs:16-54`). Collapsing
    /// them would let `__SetCSSId` retroactively rescope later creations, and
    /// would let an ordinary `view` seed a scope it never owned.
    component_css_id: i32,
    /// `__CreatePage`/`__CreateComponent`'s `componentID`, as later read by
    /// `__GetComponentID` and rewritten by `__UpdateComponentID`. A *string*
    /// name, unrelated to the numeric unique id, and deliberately not a DOM
    /// attribute.
    component_id: Option<String>,
    /// The component's `entryName`, minus web-core's `__Card__` sentinel.
    entry_name: Option<String>,
    /// `__AddDataset`/`__SetDataset` storage, ordered by first insertion the
    /// way a JavaScript object's string keys are.
    dataset: Vec<(String, PapiValue)>,
    /// The last whole-block `__SetInlineStyles` text.
    inline_base: String,
    /// `__AddInlineStyle` declarations layered over `inline_base`, in call
    /// order. Later declarations win in the cascade, which is exactly the
    /// merge semantics the member needs.
    inline_overrides: Vec<(String, String)>,
    /// For a `raw-text` element, the DOM text node mirroring its `text`
    /// attribute. `None` for every other element.
    text_mirror: Option<NodeId>,
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

    /// The element's own CSS fragment id (web-core's `l-css-id`).
    #[must_use]
    pub const fn css_id(&self) -> i32 {
        self.css_id
    }

    /// The CSS fragment id elements created under this one inherit.
    #[must_use]
    pub const fn component_css_id(&self) -> i32 {
        self.component_css_id
    }

    /// The `componentID` string, for the page and for component elements.
    #[must_use]
    pub fn component_id(&self) -> Option<&str> {
        self.component_id.as_deref()
    }

    #[must_use]
    pub fn entry_name(&self) -> Option<&str> {
        self.entry_name.as_deref()
    }

    #[must_use]
    pub fn data_by_key(&self, key: &str) -> Option<&PapiValue> {
        self.dataset
            .iter()
            .find(|(stored, _)| stored == key)
            .map(|(_, value)| value)
    }

    #[must_use]
    pub fn dataset(&self) -> impl ExactSizeIterator<Item = (&str, &PapiValue)> {
        self.dataset
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }

    #[must_use]
    pub(crate) const fn text_mirror(&self) -> Option<NodeId> {
        self.text_mirror
    }

    pub(crate) fn set_css_id(&mut self, css_id: i32) {
        self.css_id = css_id;
    }

    pub(crate) fn set_component_css_id(&mut self, component_css_id: i32) {
        self.component_css_id = component_css_id;
    }

    /// Records `__SetCSSId`'s `entryName`. Which bundle entry a fragment id
    /// belongs to only matters once scoped stylesheets are mounted, so it is
    /// stored and otherwise unread.
    pub(crate) fn set_entry_name(&mut self, entry_name: Option<&str>) {
        if let Some(entry_name) = entry_name {
            self.entry_name = Some(entry_name.to_owned());
        }
    }

    pub(crate) fn set_component_id(&mut self, component_id: &str) {
        self.component_id = Some(component_id.to_owned());
    }

    /// Records one dataset entry, preserving the position of a key that is
    /// already present.
    pub(crate) fn set_data(&mut self, key: &str, value: PapiValue) {
        if let Some(slot) = self
            .dataset
            .iter_mut()
            .find(|(stored, _)| stored == key)
            .map(|(_, stored)| stored)
        {
            *slot = value;
        } else {
            self.dataset.push((key.to_owned(), value));
        }
    }

    pub(crate) fn clear_dataset(&mut self) {
        self.dataset.clear();
    }

    /// Replaces the whole inline-style block, discarding every declaration a
    /// previous `__AddInlineStyle` layered on — `__SetInlineStyles` writes the
    /// `style` attribute wholesale.
    pub(crate) fn set_inline_base(&mut self, css: &str) {
        css.clone_into(&mut self.inline_base);
        self.inline_overrides.clear();
    }

    /// Layers one declaration over the block. `None` removes it.
    pub(crate) fn set_inline_override(&mut self, property: &str, value: Option<&str>) {
        let existing = self
            .inline_overrides
            .iter()
            .position(|(stored, _)| stored == property);
        match (existing, value) {
            (Some(index), Some(value)) => value.clone_into(&mut self.inline_overrides[index].1),
            (Some(index), None) => drop(self.inline_overrides.remove(index)),
            (None, Some(value)) => self
                .inline_overrides
                .push((property.to_owned(), value.to_owned())),
            (None, None) => {}
        }
    }

    /// The `style` attribute text for the element's current declarations.
    pub(crate) fn inline_style(&self) -> String {
        let mut css = self.inline_base.clone();
        for (property, value) in &self.inline_overrides {
            if !css.is_empty() && !css.trim_end().ends_with(';') {
                css.push(';');
            }
            css.push_str(property);
            css.push(':');
            css.push_str(value);
            css.push(';');
        }
        css
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

    pub(crate) fn insert(&mut self, unique_id: ElementId, seed: ElementSeed) -> ElementId {
        let arena_index = arena_index(unique_id).expect("a reserved element unique id is positive");
        assert_eq!(
            arena_index,
            self.entries.len(),
            "the permanent-index element arena only appends"
        );
        self.entries.push(Some(LynxElement {
            id: unique_id,
            node: seed.node,
            parent_component: seed.parent_component,
            css_id: seed.css_id,
            component_css_id: seed.component_css_id,
            component_id: seed.component_id,
            entry_name: seed.entry_name,
            dataset: Vec::new(),
            inline_base: String::new(),
            inline_overrides: Vec::new(),
            text_mirror: seed.text_mirror,
        }));
        unique_id
    }

    /// The CSS fragment id an element created under `parent_component`
    /// inherits.
    ///
    /// Reads the parent's `component_css_id`, not its `css_id` — see the field
    /// docs. A handle that names no live element yields "no scope" rather than
    /// an error: web-core's `create_element_common` looks the id up and falls
    /// back to `0` on a miss, which is what lets `__CreateRawText` pass `-1`
    /// and what keeps a collected component from poisoning later creations.
    pub(crate) fn inherited_css_id(&self, parent_component: ElementId) -> i32 {
        self.get(parent_component)
            .map_or(INHERITED_CSS_ID_NONE, LynxElement::component_css_id)
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
