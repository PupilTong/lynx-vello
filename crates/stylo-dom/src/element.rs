//! The unified [`Element`] struct — the HTML-DOM-subset node.

use std::cell::UnsafeCell;
use std::fmt;

use atomic_refcell::AtomicRefCell;
use dom::ElementState;
use rustc_hash::FxHashMap;
use selectors::matching::ElementSelectorFlags;
use smallvec::SmallVec;
use stylo::LocalName;
use stylo::data::ElementDataWrapper;
use stylo::properties::{ComputedValues, PropertyDeclarationBlock};
use stylo::servo_arc::Arc;
use stylo::shared_lock::Locked;
use stylo_atoms::Atom;

use crate::arena::ElementId;

/// A single element in the document tree.
///
/// The fields model a strict subset of the HTML DOM: tree links, tag, id,
/// classes, attributes, dynamic pseudo-class state, an inline style block,
/// character data, and the per-element style bookkeeping stylo needs. Anything
/// beyond that subset belongs to the embedder and lives in the [`ext`] payload
/// (see [`ExternalState`](crate::ExternalState)).
///
/// # Thread-safety
///
/// An `Element` owns stylo's per-element interior-mutable state — [`stylo_data`]
/// (an [`UnsafeCell`]) and [`selector_flags`] (an [`AtomicRefCell`]) — so it is
/// deliberately **not** `Sync` (the `UnsafeCell` makes it so automatically) and
/// the whole crate assumes a **single-threaded flush**: style resolution and
/// tree mutation never run concurrently on the same [`Arena`](crate::Arena).
/// The [`traits`](crate::traits) impls document this invariant at each `unsafe`
/// site.
///
/// [`ext`]: Element::ext
/// [`stylo_data`]: Element::stylo_data
/// [`selector_flags`]: Element::selector_flags
pub struct Element<T> {
    /// The parent element, or `None` for the root / a detached element.
    pub parent: Option<ElementId>,
    /// Child elements, in document order.
    pub children: Vec<ElementId>,
    /// The tag name, interned as a stylo [`LocalName`] atom so
    /// `selectors::Element::has_local_name` is a cheap atom comparison.
    pub tag: LocalName,
    /// The element's classes, interned as atoms.
    pub classes: SmallVec<[Atom; 4]>,
    /// The element's `id` selector value, distinct from a plain `id` attribute
    /// (the embedder decides whether/how the two are linked).
    pub id_attr: Option<Atom>,
    /// Plain attributes. Synthetic / reflected attributes beyond this map are
    /// served by the [`ext`](Element::ext) payload's
    /// [`extra_attr_value`](crate::ExternalState::extra_attr_value) hook.
    pub attrs: FxHashMap<Box<str>, String>,
    /// Active dynamic pseudo-classes (`:hover` / `:active` / `:focus`) as stylo
    /// state bits, written by the embedder (see
    /// [`PseudoState`](crate::PseudoState) for the bridge type).
    pub element_state: ElementState,

    /// The element's parsed inline style block (the `style` attribute), locked
    /// under the arena's [`SharedRwLock`](stylo::shared_lock::SharedRwLock).
    /// `None` when no inline style is set.
    pub inline_block: Option<Arc<Locked<PropertyDeclarationBlock>>>,

    /// The resolved computed style, written back by the style engine at flush
    /// time. `None` until first resolved.
    pub computed: Option<Arc<ComputedValues>>,

    /// stylo's per-element style data (`ElementData`), created lazily via
    /// `TElement::ensure_data`. Only touched through the
    /// [`traits`](crate::traits) impls under the single-threaded-flush
    /// invariant.
    pub stylo_data: UnsafeCell<Option<ElementDataWrapper>>,

    /// Selector flags accumulated by stylo during matching (e.g. "has a
    /// `:hover` rule that depends on me").
    pub selector_flags: AtomicRefCell<ElementSelectorFlags>,

    /// Whether this element itself needs its style recomputed.
    pub style_dirty: bool,
    /// Whether some descendant of this element needs its style recomputed.
    pub dirty_descendants: bool,

    /// Literal character-data content, for text leaves.
    pub text: Option<String>,

    /// The embedder's external-state payload (see
    /// [`ExternalState`](crate::ExternalState)).
    pub ext: T,
}

impl<T> Element<T> {
    /// Create a detached element with the given tag and external state.
    #[must_use]
    pub fn new(tag: &str, ext: T) -> Self {
        Self {
            parent: None,
            children: Vec::new(),
            tag: LocalName::from(tag),
            classes: SmallVec::new(),
            id_attr: None,
            attrs: FxHashMap::default(),
            element_state: ElementState::empty(),
            inline_block: None,
            computed: None,
            stylo_data: UnsafeCell::new(None),
            selector_flags: AtomicRefCell::new(ElementSelectorFlags::empty()),
            style_dirty: false,
            dirty_descendants: false,
            text: None,
            ext,
        }
    }

    /// The element's tag name as a string slice.
    #[must_use]
    pub fn tag_str(&self) -> &str {
        self.tag.0.as_ref()
    }

    /// The resolved computed style, if this element has been styled.
    #[must_use]
    pub fn computed(&self) -> Option<&Arc<ComputedValues>> {
        self.computed.as_ref()
    }
}

impl<T: fmt::Debug> fmt::Debug for Element<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `stylo_data` (an `UnsafeCell`) is deliberately omitted: it is not
        // `Debug`, and reading it would need the single-threaded-flush
        // invariant we cannot assert from here.
        f.debug_struct("Element")
            .field("tag", &self.tag_str())
            .field("classes", &self.classes)
            .field("id_attr", &self.id_attr)
            .field("element_state", &self.element_state)
            .field("has_inline_block", &self.inline_block.is_some())
            .field("has_computed", &self.computed.is_some())
            .field("style_dirty", &self.style_dirty)
            .field("dirty_descendants", &self.dirty_descendants)
            .field("children", &self.children)
            .field("ext", &self.ext)
            .finish_non_exhaustive()
    }
}
