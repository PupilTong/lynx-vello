//! The [`Element`] node variant in the HTML-DOM subset.

use std::cell::UnsafeCell;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, AtomicUsize, Ordering};

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

use crate::arena::NodeId;

/// Bit set in [`Element::snapshot_flags`] when a pre-mutation snapshot has
/// been recorded for this element in the arena's
/// [`SnapshotMap`](stylo::selector_parser::SnapshotMap).
pub(crate) const SNAPSHOT_PRESENT: u8 = 1 << 0;
/// Bit set once stylo's invalidation pass has consumed the snapshot.
pub(crate) const SNAPSHOT_HANDLED: u8 = 1 << 1;

/// A single element in the document tree.
///
/// The fields model a strict subset of the HTML DOM: tree links, tag, id,
/// classes, attributes, dynamic pseudo-class state, an inline style block,
/// and the per-element style bookkeeping stylo needs. Character data lives in
/// distinct [`TextNode`](crate::TextNode)s. Anything
/// beyond that subset belongs to the embedder and lives in the [`ext`] payload
/// (see [`ExternalState`](crate::ExternalState)).
///
/// # Thread-safety
///
/// stylo's restyle traversal may run **in parallel** (rayon workers sharing
/// `&Arena`), so every piece of element state that stylo touches during a
/// traversal is either
///
/// - atomic ([`selector_flags`], [`style_dirty`], [`dirty_descendants`], [`snapshot_flags`],
///   [`children_to_process`]), or
/// - owned by exactly one worker at a time under stylo's traversal discipline ([`stylo_data`], an
///   [`UnsafeCell`]; see [`crate::traits`] for the per-access safety arguments).
///
/// Everything else (tag/classes/attrs/`ext`) is **immutable during a
/// flush**: mutation requires `&mut Arena`, which
/// [`StyleEngine::flush_tree`](crate::StyleEngine::flush_tree) holds
/// exclusively for the whole traversal.
///
/// [`ext`]: Element::ext
/// [`stylo_data`]: Element::stylo_data
/// [`selector_flags`]: Element::selector_flags
/// [`style_dirty`]: Element::style_dirty
/// [`dirty_descendants`]: Element::dirty_descendants
/// [`snapshot_flags`]: Element::snapshot_flags
/// [`children_to_process`]: Element::children_to_process
pub struct Element<T> {
    /// The parent element, or `None` for the root / a detached element.
    pub parent: Option<NodeId>,
    /// Child nodes, in document order.
    pub children: Vec<NodeId>,
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

    /// stylo's per-element style data (`ElementData`), created lazily via
    /// `TElement::ensure_data`. The resolved computed style lives here (see
    /// [`computed_style`](Element::computed_style)). Only touched through the
    /// [`traits`](crate::traits) impls under stylo's traversal discipline.
    pub stylo_data: UnsafeCell<Option<ElementDataWrapper>>,

    /// Selector flags accumulated by stylo during matching (e.g. "has a
    /// child-position-dependent rule"), stored as the raw
    /// [`ElementSelectorFlags`] bits. Atomic because parallel workers matching
    /// sibling elements may both push `for_parent()` flags onto the shared
    /// parent.
    pub selector_flags: AtomicUsize,

    /// Whether this element itself has pending style work (embedder-visible
    /// dirty signal; stylo's own scheduling uses `ElementData::hint`).
    pub style_dirty: AtomicBool,
    /// Whether some descendant of this element has pending style work. This is
    /// the bit stylo's traversal walks down
    /// ([`TElement::has_dirty_descendants`](stylo::dom::TElement::has_dirty_descendants)).
    pub dirty_descendants: AtomicBool,

    /// Snapshot lifecycle bits ([`SNAPSHOT_PRESENT`] / [`SNAPSHOT_HANDLED`]),
    /// mirroring `TElement::{has_snapshot, handled_snapshot}`.
    pub(crate) snapshot_flags: AtomicU8,

    /// Bottom-up traversal bookkeeping
    /// (`TElement::{store_children_to_process, did_process_child}`). Unused
    /// while the style traversal has no postorder pass, but kept sound for
    /// when one appears.
    pub(crate) children_to_process: AtomicIsize,

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
            stylo_data: UnsafeCell::new(None),
            selector_flags: AtomicUsize::new(0),
            style_dirty: AtomicBool::new(false),
            dirty_descendants: AtomicBool::new(false),
            snapshot_flags: AtomicU8::new(0),
            children_to_process: AtomicIsize::new(0),
            ext,
        }
    }

    /// The element's tag name as a string slice.
    #[must_use]
    pub fn tag_str(&self) -> &str {
        self.tag.0.as_ref()
    }

    /// Whether stylo has ever created per-element style data here (i.e. the
    /// element has been through a style pass).
    ///
    /// Must not be called while a style flush is running on the element's
    /// arena (impossible through the public API: a flush holds `&mut Arena`).
    #[must_use]
    pub fn has_style_data(&self) -> bool {
        // SAFETY: reads only the `Option` discriminant; no flush is running
        // (flushes require `&mut Arena`, we hold `&self` from that arena).
        #[expect(unsafe_code, reason = "UnsafeCell discriminant read outside any flush")]
        unsafe {
            (*self.stylo_data.get()).is_some()
        }
    }

    /// The resolved computed style, if this element has been styled.
    ///
    /// The style lives in stylo's per-element `ElementData`; this clones the
    /// `Arc` out of it. Must not be called while a style flush is running on
    /// the element's arena (impossible through the public API: a flush holds
    /// `&mut Arena`).
    #[must_use]
    pub fn computed_style(&self) -> Option<Arc<ComputedValues>> {
        // SAFETY: no flush is running (flushes require `&mut Arena`, and we
        // hold `&self` borrowed from that arena), so reading the slot and
        // taking a shared borrow of the wrapper cannot race.
        #[expect(unsafe_code, reason = "UnsafeCell read outside any flush")]
        let slot = unsafe { (*self.stylo_data.get()).as_ref() };
        slot.and_then(|wrapper| wrapper.borrow().styles.primary.clone())
    }

    /// Store a resolved computed style, creating the stylo `ElementData` slot
    /// if needed. Used by the standalone
    /// [`StyleEngine::resolve`](crate::StyleEngine::resolve) path; the flush
    /// traversal writes styles through stylo itself.
    pub fn set_computed_style(&mut self, style: Arc<ComputedValues>) {
        let slot = self.stylo_data.get_mut();
        let wrapper = slot.get_or_insert_with(ElementDataWrapper::default);
        wrapper.borrow_mut().styles.primary = Some(style);
    }

    /// The accumulated stylo selector flags.
    #[must_use]
    pub fn selector_flags(&self) -> ElementSelectorFlags {
        ElementSelectorFlags::from_bits_retain(self.selector_flags.load(Ordering::Relaxed))
    }

    /// Whether this element itself has pending style work.
    ///
    /// A scheduling breadcrumb, not ground truth: the authoritative "does the
    /// tree need a flush" signal is the root's bits. In one corner the
    /// breadcrumb can go stale — a descendant of a subtree that became
    /// `display: none` in the same flush keeps its bit set (stylo prunes the
    /// none-subtree from traversal and drops its style data; the bit clears
    /// the next time the element is scheduled while reachable).
    #[must_use]
    pub fn is_style_dirty(&self) -> bool {
        self.style_dirty.load(Ordering::Relaxed)
    }

    /// Whether a descendant has pending style work.
    #[must_use]
    pub fn has_dirty_descendants(&self) -> bool {
        self.dirty_descendants.load(Ordering::Relaxed)
    }

    pub(crate) fn set_style_dirty(&self, dirty: bool) {
        self.style_dirty.store(dirty, Ordering::Relaxed);
    }

    pub(crate) fn set_dirty_descendants_bit(&self, dirty: bool) {
        self.dirty_descendants.store(dirty, Ordering::Relaxed);
    }

    pub(crate) fn snapshot_present(&self) -> bool {
        self.snapshot_flags.load(Ordering::Relaxed) & SNAPSHOT_PRESENT != 0
    }

    pub(crate) fn snapshot_handled(&self) -> bool {
        self.snapshot_flags.load(Ordering::Relaxed) & SNAPSHOT_HANDLED != 0
    }

    pub(crate) fn set_snapshot_present(&self) {
        self.snapshot_flags
            .fetch_or(SNAPSHOT_PRESENT, Ordering::Relaxed);
    }

    pub(crate) fn set_snapshot_handled(&self) {
        self.snapshot_flags
            .fetch_or(SNAPSHOT_HANDLED, Ordering::Relaxed);
    }

    pub(crate) fn clear_snapshot_flags(&self) {
        self.snapshot_flags.store(0, Ordering::Relaxed);
    }

    /// Mutable access to the stylo `ElementData` wrapper, if it exists.
    ///
    /// Safe because it goes through `&mut self`: exclusive access to the
    /// element means no traversal is concurrently touching the `UnsafeCell`.
    pub(crate) fn stylo_data_mut(&mut self) -> Option<&mut ElementDataWrapper> {
        self.stylo_data.get_mut().as_mut()
    }
}

impl<T: fmt::Debug> fmt::Debug for Element<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `stylo_data` (an `UnsafeCell`) is deliberately omitted: it is not
        // `Debug`, and reading it here would need the no-concurrent-flush
        // invariant we cannot assert from a generic Debug impl.
        f.debug_struct("Element")
            .field("tag", &self.tag_str())
            .field("classes", &self.classes)
            .field("id_attr", &self.id_attr)
            .field("element_state", &self.element_state)
            .field("has_inline_block", &self.inline_block.is_some())
            .field("style_dirty", &self.is_style_dirty())
            .field("dirty_descendants", &Element::has_dirty_descendants(self))
            .field("children", &self.children)
            .field("ext", &self.ext)
            .finish_non_exhaustive()
    }
}
