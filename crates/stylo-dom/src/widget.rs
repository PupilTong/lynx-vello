//! The unified [`Widget`] element struct and its event-registration types.

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

use crate::arena::WidgetId;
use crate::kind::WidgetKind;

/// Which bind/catch channel an event handler was authored on.
///
/// These mirror Lynx's five mutually-exclusive event attribute namespaces
/// (`bindEvent`, `catchEvent`, `capture-bind`, `capture-catch`,
/// `global-bindEvent`). The phase/propagation semantics they imply are the
/// runtime's concern; here they are stored verbatim.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventKind {
    /// `bind*` — bubble-phase listener.
    Bind,
    /// `catch*` — bubble-phase listener that also halts propagation.
    Catch,
    /// `capture-bind*` — capture-phase listener.
    CaptureBind,
    /// `capture-catch*` — capture-phase listener that also halts propagation.
    CaptureCatch,
    /// `global-bind*` — page/component-wide listener, no capture/bubble phase.
    GlobalBind,
}

/// A single event binding on an element.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EventReg {
    /// The event name (e.g. `"tap"`).
    pub name: Box<str>,
    /// The channel this handler was authored on.
    pub kind: EventKind,
    /// Opaque handler identifier. A later runtime crate resolves this to an
    /// actual callback; for now it is stored as an uninterpreted string.
    pub handler: Box<str>,
}

/// A single element in the widget tree.
///
/// One struct covers every [`WidgetKind`]; `kind` discriminates. Text content is
/// carried in [`Widget::text`] for `raw-text` leaves.
///
/// # Thread-safety
///
/// A `Widget` owns stylo's per-element interior-mutable state — [`stylo_data`]
/// (an [`UnsafeCell`]) and [`selector_flags`] (an [`AtomicRefCell`]) — so it is
/// deliberately **not** `Sync` (the `UnsafeCell` makes it so automatically) and
/// the whole crate assumes a **single-threaded flush**: style resolution and
/// tree mutation never run concurrently on the same [`Arena`](crate::Arena).
/// The [`traits`](crate::traits) impls document this invariant at each `unsafe`
/// site.
///
/// [`stylo_data`]: Widget::stylo_data
/// [`selector_flags`]: Widget::selector_flags
pub struct Widget {
    /// The parent element, or `None` for the root / a detached element.
    pub parent: Option<WidgetId>,
    /// Child elements, in document order.
    pub children: Vec<WidgetId>,
    /// The element kind.
    pub kind: WidgetKind,
    /// The Lynx tag name, interned as a stylo [`LocalName`] atom so
    /// `selectors::Element::has_local_name` is a cheap atom comparison.
    pub tag: LocalName,
    /// The Lynx `unique_id`, assigned by the [`Arena`](crate::Arena) on
    /// insertion (1-based, monotonically increasing).
    pub unique_id: i32,
    /// The `css_id` scoping this element's styles: `0` means unset / global.
    /// Stamped directly by Lynx's `__SetCSSId` (there is no `<component>`
    /// element in this engine to inherit it from). It is also exposed to
    /// selector matching as the synthetic `l-css-id` attribute.
    pub css_id: i32,
    /// The element's classes, interned as atoms.
    pub classes: SmallVec<[Atom; 4]>,
    /// The element's `id` selector value (set via `set_id`, i.e. Lynx's
    /// `__SetID`), distinct from a plain `id` attribute.
    pub id_attr: Option<Atom>,
    /// Plain attributes.
    pub attrs: FxHashMap<Box<str>, String>,
    /// `data-*` dataset entries (keys stored without the `data-` prefix).
    pub dataset: FxHashMap<Box<str>, String>,
    /// Active dynamic pseudo-classes (`:hover` / `:active` / `:focus`) as stylo
    /// state bits. Written through the `WidgetTree::set_pseudo_state` PAPI in
    /// the `lynx-dom` crate.
    pub element_state: ElementState,

    /// The element's parsed inline style block (Lynx's `style` attribute /
    /// `__AddInlineStyle` / `__SetInlineStyles`), locked under the arena's
    /// [`SharedRwLock`](stylo::shared_lock::SharedRwLock). `None` when no inline
    /// style is set.
    pub inline_block: Option<Arc<Locked<PropertyDeclarationBlock>>>,

    /// The resolved computed style, written back by the `lynx-style` crate at
    /// flush time. `None` until first resolved.
    pub computed: Option<Arc<ComputedValues>>,

    /// stylo's per-element style data (`ElementData`), created lazily via
    /// `TElement::ensure_data`. Only touched through the [`traits`](crate::traits)
    /// impls under the single-threaded-flush invariant.
    pub stylo_data: UnsafeCell<Option<ElementDataWrapper>>,

    /// Selector flags accumulated by stylo during matching (e.g. "has a
    /// `:hover` rule that depends on me").
    pub selector_flags: AtomicRefCell<ElementSelectorFlags>,

    /// Whether this element itself needs its style recomputed.
    pub style_dirty: bool,
    /// Whether some descendant of this element needs its style recomputed.
    pub dirty_descendants: bool,

    /// Literal text content, for `raw-text` leaves.
    pub text: Option<String>,

    /// Event bindings on this element.
    pub events: SmallVec<[EventReg; 2]>,
}

impl Widget {
    /// Create a detached widget of the given kind and tag.
    ///
    /// `unique_id` is left `0` until the [`Arena`](crate::Arena) assigns it on
    /// insertion.
    #[must_use]
    pub fn new(kind: WidgetKind, tag: &str) -> Self {
        Self {
            parent: None,
            children: Vec::new(),
            kind,
            tag: LocalName::from(tag),
            unique_id: 0,
            css_id: 0,
            classes: SmallVec::new(),
            id_attr: None,
            attrs: FxHashMap::default(),
            dataset: FxHashMap::default(),
            element_state: ElementState::empty(),
            inline_block: None,
            computed: None,
            stylo_data: UnsafeCell::new(None),
            selector_flags: AtomicRefCell::new(ElementSelectorFlags::empty()),
            style_dirty: false,
            dirty_descendants: false,
            text: None,
            events: SmallVec::new(),
        }
    }

    /// The element's Lynx tag name as a string slice.
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

impl fmt::Debug for Widget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `stylo_data` (an `UnsafeCell`) is deliberately omitted: it is not
        // `Debug`, and reading it would need the single-threaded-flush
        // invariant we cannot assert from here.
        f.debug_struct("Widget")
            .field("kind", &self.kind)
            .field("tag", &self.tag_str())
            .field("unique_id", &self.unique_id)
            .field("css_id", &self.css_id)
            .field("classes", &self.classes)
            .field("id_attr", &self.id_attr)
            .field("element_state", &self.element_state)
            .field("has_inline_block", &self.inline_block.is_some())
            .field("has_computed", &self.computed.is_some())
            .field("style_dirty", &self.style_dirty)
            .field("dirty_descendants", &self.dirty_descendants)
            .field("children", &self.children)
            .finish_non_exhaustive()
    }
}
