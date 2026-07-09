//! The [`WidgetTree`] — owner of the arena and the Element-PAPI surface.
//!
//! Methods here are shaped after Lynx's JS Element PAPI (renamed to `snake_case`
//! Rust). Each mirrors a `__*` PAPI opcode, so the method names keep the
//! `element` wording of the opcode they map to (e.g. [`append_element`] ↔
//! `__AppendElement`, [`insert_element_before`] ↔ `__InsertElementBefore`,
//! [`remove_element`] ↔ `__RemoveElement`, [`destroy_element`],
//! [`replace_element`] ↔ `__ReplaceElement`, [`create_element`] ↔
//! `__CreateElement`) even though the values they carry are [`Widget`]s. JS
//! bindings live in a later runtime crate; this is the pure native validation
//! layer. There is deliberately **no** flush/resolution driver: the `lynx-style`
//! crate drives restyling and reads the dirty state maintained here (see
//! [`WidgetTree::has_dirty`] / [`WidgetTree::clear_dirty`]).
//!
//! This layer validates PAPI semantics — stale handles, cycles, insertion
//! reference resolution, error mapping, the `unique_id` minting + index, the
//! `css_id` batch — and **delegates** the actual tree mutation and inline-style
//! parsing to the [`stylo_dom`] crate's [`Arena`] primitives, because their
//! invalidation is style-system logic. The Lynx-specific per-widget data lives
//! in the element's [`WidgetState`] payload.
//!
//! [`append_element`]: WidgetTree::append_element
//! [`insert_element_before`]: WidgetTree::insert_element_before
//! [`remove_element`]: WidgetTree::remove_element
//! [`destroy_element`]: WidgetTree::destroy_element
//! [`replace_element`]: WidgetTree::replace_element
//! [`create_element`]: WidgetTree::create_element

use rustc_hash::FxHashMap;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;
use stylo::shared_lock::SharedRwLock;
use stylo::stylesheets::UrlExtraData;
use stylo_atoms::Atom;
use stylo_dom::{Arena, Element, PseudoState};
use thiserror::Error;

use crate::kind::WidgetKind;
use crate::state::{EventKind, EventReg, WidgetState};
use crate::{Widget, WidgetId, WidgetRef};

/// An error from a tree-mutating [`WidgetTree`] operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Error)]
pub enum WidgetError {
    /// A handle did not resolve to a live element.
    #[error("widget {0:?} is stale or does not exist")]
    StaleElement(WidgetId),
    /// A `remove`/`replace` target was not a child of the given parent.
    #[error("widget {child:?} is not a child of {parent:?}")]
    NotAChild {
        /// The claimed parent.
        parent: WidgetId,
        /// The element that was not actually its child.
        child: WidgetId,
    },
    /// Performing the insertion would make an element its own ancestor.
    #[error("linking {ancestor:?} under {descendant:?} would create a cycle")]
    WouldCycle {
        /// The element being inserted (would become an ancestor of itself).
        ancestor: WidgetId,
        /// The intended parent (a descendant of `ancestor`).
        descendant: WidgetId,
    },
    /// An `insert_element_before` reference node was not a child of the parent.
    #[error("insertion reference {0:?} is not a child of the parent")]
    BadInsertReference(WidgetId),
}

/// The widget tree: a generational [`Arena`] of [`Widget`]s plus the current
/// page root, the Lynx `unique_id` counter, and a `unique_id` → [`WidgetId`]
/// index.
#[derive(Debug)]
pub struct WidgetTree {
    arena: Arena<WidgetState>,
    page: Option<WidgetId>,
    /// The next Lynx `unique_id` to mint (1-based; 0 stays reserved as
    /// "unset").
    next_unique_id: i32,
    by_unique_id: FxHashMap<i32, WidgetId>,
}

impl Default for WidgetTree {
    fn default() -> Self {
        Self::new()
    }
}

impl WidgetTree {
    /// Create an empty widget tree with a freshly minted [`SharedRwLock`].
    ///
    /// Suitable for DOM-only use; to style the tree, build it with
    /// [`WidgetTree::with_lock`] using the `StyleEngine`'s shared lock so the
    /// cascade's guards match this tree's inline style blocks.
    #[must_use]
    pub fn new() -> Self {
        Self::from_arena(Arena::new())
    }

    /// Create an empty widget tree backed by an explicit [`SharedRwLock`] and
    /// [`UrlExtraData`] (typically the `StyleEngine`'s, so inline styles parse
    /// against the same lock the cascade guards).
    #[must_use]
    pub fn with_lock(lock: SharedRwLock, url_data: UrlExtraData) -> Self {
        Self::from_arena(Arena::with_lock(lock, url_data))
    }

    fn from_arena(arena: Arena<WidgetState>) -> Self {
        Self {
            arena,
            page: None,
            // Lynx `unique_id`s are 1-based; 0 stays reserved as "unset".
            next_unique_id: 1,
            by_unique_id: FxHashMap::default(),
        }
    }

    /// Borrow the underlying arena.
    #[must_use]
    pub const fn arena(&self) -> &Arena<WidgetState> {
        &self.arena
    }

    /// Mutably borrow the underlying arena.
    ///
    /// The `lynx-style` crate uses this to write resolved computed styles and
    /// clear dirty bits after a resolution pass.
    pub const fn arena_mut(&mut self) -> &mut Arena<WidgetState> {
        &mut self.arena
    }

    /// The tree's [`SharedRwLock`] (guarding inline style blocks).
    #[must_use]
    pub fn shared_lock(&self) -> &SharedRwLock {
        self.arena.shared_lock()
    }

    /// The tree's base [`UrlExtraData`] (used to parse inline styles).
    #[must_use]
    pub fn url_data(&self) -> &UrlExtraData {
        self.arena.url_data()
    }

    // --- element creation -------------------------------------------------

    fn create(&mut self, kind: WidgetKind, tag: &str) -> WidgetId {
        let unique_id = self.next_unique_id;
        self.next_unique_id = self.next_unique_id.wrapping_add(1);
        let id = self
            .arena
            .insert(Element::new(tag, WidgetState::new(kind, unique_id)));
        self.by_unique_id.insert(unique_id, id);
        id
    }

    /// Create the `<page>` root element and record it as the tree's page.
    pub fn create_page(&mut self) -> WidgetId {
        let id = self.create(WidgetKind::Page, "page");
        self.page = Some(id);
        // The root always needs an initial style pass.
        self.arena.mark_style_dirty(id);
        id
    }

    /// Create a `<view>` element.
    pub fn create_view(&mut self) -> WidgetId {
        self.create(WidgetKind::View, "view")
    }

    /// Create a `<text>` element.
    pub fn create_text(&mut self) -> WidgetId {
        self.create(WidgetKind::Text, "text")
    }

    /// Create a `<raw-text>` leaf carrying literal text content.
    pub fn create_raw_text(&mut self, text: impl Into<String>) -> WidgetId {
        let id = self.create(WidgetKind::RawText, "raw-text");
        if let Some(widget) = self.arena.get_mut(id) {
            widget.text = Some(text.into());
        }
        id
    }

    /// Create an `<image>` element.
    pub fn create_image(&mut self) -> WidgetId {
        self.create(WidgetKind::Image, "image")
    }

    /// Create a `<scroll-view>` element.
    pub fn create_scroll_view(&mut self) -> WidgetId {
        self.create(WidgetKind::ScrollView, "scroll-view")
    }

    /// Create a `<list>` element.
    pub fn create_list(&mut self) -> WidgetId {
        self.create(WidgetKind::List, "list")
    }

    /// Create a `<wrapper>` element.
    pub fn create_wrapper(&mut self) -> WidgetId {
        self.create(WidgetKind::Wrapper, "wrapper")
    }

    /// Create an element from an arbitrary Lynx tag name. The tag is classified
    /// via [`WidgetKind::from_tag`].
    pub fn create_element(&mut self, tag: &str) -> WidgetId {
        let kind = WidgetKind::from_tag(tag);
        self.create(kind, tag)
    }

    // --- tree mutation ----------------------------------------------------

    /// Append `child` as the last child of `parent`.
    pub fn append_element(&mut self, child: WidgetId, parent: WidgetId) -> Result<(), WidgetError> {
        self.insert_element_before(child, parent, None)
    }

    /// Insert `child` into `parent` immediately before `before`, or append it
    /// when `before` is `None`.
    ///
    /// `child` is first detached from any current parent. Re-inserting within
    /// the same parent reorders it. Validation (stale handles, cycles, the
    /// insertion reference) happens here; the unlink/link is delegated to the
    /// [`Arena`] tree primitives, which carry the structural invalidation.
    pub fn insert_element_before(
        &mut self,
        child: WidgetId,
        parent: WidgetId,
        before: Option<WidgetId>,
    ) -> Result<(), WidgetError> {
        if !self.arena.contains(child) {
            return Err(WidgetError::StaleElement(child));
        }
        if !self.arena.contains(parent) {
            return Err(WidgetError::StaleElement(parent));
        }
        if child == parent || self.arena.is_ancestor(child, parent) {
            return Err(WidgetError::WouldCycle {
                ancestor: child,
                descendant: parent,
            });
        }
        if let Some(reference) = before {
            if reference == child {
                // DOM pre-insert: the reference resolves to `child`'s next
                // sibling, so `insertBefore(n, n)` keeps `n` exactly where it
                // is — a structural no-op (web-core parity).
                return if self.arena.is_child_of(child, parent) {
                    Ok(())
                } else {
                    Err(WidgetError::BadInsertReference(reference))
                };
            }
            if !self.arena.is_child_of(reference, parent) {
                return Err(WidgetError::BadInsertReference(reference));
            }
        }

        self.arena.detach(child);

        let index = match before {
            None => self.arena.children_len(parent),
            Some(reference) => self
                .arena
                .child_position(parent, reference)
                .unwrap_or_else(|| self.arena.children_len(parent)),
        };
        self.arena.attach_at(parent, child, index);
        Ok(())
    }

    /// Remove `child` from `parent`, **detaching** it — the subtree stays
    /// alive in the arena (and in the `unique_id` index) and can be
    /// re-inserted later.
    ///
    /// This matches the Element PAPI contract: web-core's `__RemoveElement` is
    /// DOM `removeChild` (the element remains usable), and Lynx list recycling
    /// re-attaches previously removed subtrees. Use
    /// [`destroy_element`](Self::destroy_element) to actually free a subtree.
    pub fn remove_element(&mut self, parent: WidgetId, child: WidgetId) -> Result<(), WidgetError> {
        let Some(child_widget) = self.arena.get(child) else {
            return Err(WidgetError::StaleElement(child));
        };
        if child_widget.parent != Some(parent) {
            return Err(WidgetError::NotAChild { parent, child });
        }

        // `detach` unlinks and applies the structural invalidation (parent
        // subtree + parent's following siblings, for `:empty` + `+`/`~`).
        self.arena.detach(child);
        Ok(())
    }

    /// Detach `id` (if attached) and free its entire subtree from the arena
    /// and the `unique_id` index. All handles into the subtree become stale.
    ///
    /// This is the explicit destruction step [`remove_element`](Self::remove_element)
    /// deliberately does not perform; the runtime layer decides when a
    /// detached subtree is truly dead (web-core relies on GC for this).
    pub fn destroy_element(&mut self, id: WidgetId) -> Result<(), WidgetError> {
        if !self.arena.contains(id) {
            return Err(WidgetError::StaleElement(id));
        }
        self.arena.detach(id);
        // The arena returns the freed widgets' state; harvest the unique_ids
        // out of it to keep the index consistent.
        for state in self.arena.drop_subtree(id) {
            self.by_unique_id.remove(&state.unique_id);
        }
        Ok(())
    }

    /// Replace `old` with `new` in the tree, keeping `old`'s position. `new`
    /// is detached from any current parent first; `old` ends up detached but
    /// alive (like DOM `replaceChild`, which returns the old node).
    ///
    /// Replacing a detached `old` is a no-op, matching DOM `replaceWith` on a
    /// parentless node.
    pub fn replace_element(&mut self, new: WidgetId, old: WidgetId) -> Result<(), WidgetError> {
        if new == old {
            return Ok(());
        }
        let Some(old_widget) = self.arena.get(old) else {
            return Err(WidgetError::StaleElement(old));
        };
        let Some(parent) = old_widget.parent else {
            return Ok(());
        };
        self.insert_element_before(new, parent, Some(old))?;
        self.remove_element(parent, old)
    }

    /// The first child of `parent`, if any.
    #[must_use]
    pub fn first_element(&self, parent: WidgetId) -> Option<WidgetId> {
        self.arena.get(parent)?.children.first().copied()
    }

    /// The next sibling of `widget`, if any.
    #[must_use]
    pub fn next_element(&self, widget: WidgetId) -> Option<WidgetId> {
        let parent = self.arena.get(widget)?.parent?;
        let siblings = &self.arena.get(parent)?.children;
        let pos = siblings.iter().position(|&c| c == widget)?;
        siblings.get(pos + 1).copied()
    }

    /// The parent of `widget`, if any.
    #[must_use]
    pub fn get_parent(&self, widget: WidgetId) -> Option<WidgetId> {
        self.arena.get(widget)?.parent
    }

    // --- styling / attributes ---------------------------------------------

    /// Replace an element's classes from a whitespace-separated list.
    pub fn set_classes(&mut self, id: WidgetId, classes: &str) -> Result<(), WidgetError> {
        match self.arena.get_mut(id) {
            Some(widget) => {
                widget.classes = classes.split_whitespace().map(Atom::from).collect();
            }
            None => return Err(WidgetError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Add a single class (no-op if already present).
    pub fn add_class(&mut self, id: WidgetId, class: &str) -> Result<(), WidgetError> {
        match self.arena.get_mut(id) {
            Some(widget) => {
                let class = Atom::from(class);
                if !widget.classes.contains(&class) {
                    widget.classes.push(class);
                }
            }
            None => return Err(WidgetError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Replace an element's inline style, parsing the whole declaration block
    /// through stylo (Lynx's `__SetInlineStyles`). An empty string clears it.
    ///
    /// The parse is delegated to the [`Arena`] inline-style primitive.
    pub fn set_inline_styles(&mut self, id: WidgetId, text: &str) -> Result<(), WidgetError> {
        if !self.arena.contains(id) {
            return Err(WidgetError::StaleElement(id));
        }
        self.arena.set_inline_styles(id, text);
        Ok(())
    }

    /// Parse a single `name: value` declaration through stylo and merge it into
    /// the element's inline style block (Lynx's `__AddInlineStyle`).
    ///
    /// The parse/merge is delegated to the [`Arena`] inline-style primitive; an
    /// unparseable property/value is dropped.
    pub fn add_inline_style(
        &mut self,
        id: WidgetId,
        name: &str,
        value: &str,
    ) -> Result<(), WidgetError> {
        if !self.arena.contains(id) {
            return Err(WidgetError::StaleElement(id));
        }
        self.arena.add_inline_style(id, name, value);
        Ok(())
    }

    /// Set a plain attribute.
    ///
    /// Note: unlike the DOM, a plain `"id"` attribute is stored as an ordinary
    /// attribute here — Lynx sets the id selector separately via
    /// [`WidgetTree::set_id`] (its `__SetID`).
    pub fn set_attribute(
        &mut self,
        id: WidgetId,
        name: &str,
        value: &str,
    ) -> Result<(), WidgetError> {
        match self.arena.get_mut(id) {
            Some(widget) => {
                widget.attrs.insert(name.into(), value.to_owned());
            }
            None => return Err(WidgetError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Set an element's id selector value (Lynx's `__SetID`). An empty string
    /// clears it.
    pub fn set_id(&mut self, id: WidgetId, id_selector: &str) -> Result<(), WidgetError> {
        match self.arena.get_mut(id) {
            Some(widget) => {
                widget.id_attr = if id_selector.is_empty() {
                    None
                } else {
                    Some(Atom::from(id_selector))
                };
            }
            None => return Err(WidgetError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Set the `css_id` (style scope) on a batch of elements.
    pub fn set_css_id(&mut self, ids: &[WidgetId], css_id: i32) -> Result<(), WidgetError> {
        if let Some(&bad) = ids.iter().find(|&&id| !self.arena.contains(id)) {
            return Err(WidgetError::StaleElement(bad));
        }
        for &id in ids {
            if let Some(widget) = self.arena.get_mut(id) {
                widget.ext.css_id = css_id;
            }
        }
        for &id in ids {
            self.arena.mark_attribute_changed(id);
        }
        Ok(())
    }

    /// Replace an element's `data-*` dataset.
    pub fn set_dataset<I, K, V>(&mut self, id: WidgetId, entries: I) -> Result<(), WidgetError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<Box<str>>,
        V: Into<String>,
    {
        match self.arena.get_mut(id) {
            Some(widget) => {
                widget.ext.dataset = entries
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect();
            }
            None => return Err(WidgetError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Add or overwrite a single `data-*` dataset entry.
    pub fn add_dataset(&mut self, id: WidgetId, key: &str, value: &str) -> Result<(), WidgetError> {
        match self.arena.get_mut(id) {
            Some(widget) => {
                widget.ext.dataset.insert(key.into(), value.to_owned());
            }
            None => return Err(WidgetError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Register an event binding on an element. (Does not affect style, so no
    /// invalidation.)
    pub fn add_event(
        &mut self,
        id: WidgetId,
        kind: EventKind,
        name: &str,
        handler: &str,
    ) -> Result<(), WidgetError> {
        match self.arena.get_mut(id) {
            Some(widget) => widget.ext.events.push(EventReg {
                name: name.into(),
                kind,
                handler: handler.into(),
            }),
            None => return Err(WidgetError::StaleElement(id)),
        }
        Ok(())
    }

    /// Toggle one or more pseudo-class flags on an element.
    pub fn set_pseudo_state(
        &mut self,
        id: WidgetId,
        state: PseudoState,
        on: bool,
    ) -> Result<(), WidgetError> {
        match self.arena.get_mut(id) {
            Some(widget) => widget.element_state.set(state.to_element_state(), on),
            None => return Err(WidgetError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    // --- getters ----------------------------------------------------------

    /// An element's Lynx tag name.
    #[must_use]
    pub fn get_tag(&self, id: WidgetId) -> Option<&str> {
        self.arena.get(id).map(Widget::tag_str)
    }

    /// An element's plain attribute map.
    #[must_use]
    pub fn get_attributes(&self, id: WidgetId) -> Option<&FxHashMap<Box<str>, String>> {
        self.arena.get(id).map(|widget| &widget.attrs)
    }

    /// An element's Lynx `unique_id`.
    #[must_use]
    pub fn get_element_unique_id(&self, id: WidgetId) -> Option<i32> {
        self.arena.get(id).map(|widget| widget.ext.unique_id)
    }

    /// An element's active dynamic pseudo-classes, as a [`PseudoState`].
    #[must_use]
    pub fn pseudo_state(&self, id: WidgetId) -> Option<PseudoState> {
        self.arena
            .get(id)
            .map(|widget| PseudoState::from_element_state(widget.element_state))
    }

    /// Resolve a Lynx `unique_id` back to its [`WidgetId`].
    #[must_use]
    pub fn element_by_unique_id(&self, unique_id: i32) -> Option<WidgetId> {
        self.by_unique_id.get(&unique_id).copied()
    }

    /// The tree's `<page>` root, if one has been created.
    #[must_use]
    pub const fn get_page_element(&self) -> Option<WidgetId> {
        self.page
    }

    /// Borrow an element's [`Widget`], if live.
    #[must_use]
    pub fn widget(&self, id: WidgetId) -> Option<&Widget> {
        self.arena.get(id)
    }

    /// A read-only navigation handle for an element, if live.
    #[must_use]
    pub fn widget_ref(&self, id: WidgetId) -> Option<WidgetRef<'_>> {
        self.arena.element_ref(id)
    }

    /// An element's resolved computed style, if it has been styled.
    #[must_use]
    pub fn computed(&self, id: WidgetId) -> Option<&Arc<ComputedValues>> {
        self.arena.get(id).and_then(Widget::computed)
    }

    /// Store an element's resolved computed style and clear its `style_dirty`
    /// bit. Called by the `lynx-style` crate after resolving an element.
    pub fn set_computed(
        &mut self,
        id: WidgetId,
        style: Arc<ComputedValues>,
    ) -> Result<(), WidgetError> {
        match self.arena.get_mut(id) {
            Some(widget) => {
                widget.computed = Some(style);
                widget.style_dirty = false;
            }
            None => return Err(WidgetError::StaleElement(id)),
        }
        Ok(())
    }

    // --- dirty state ------------------------------------------------------

    /// Whether the tree has any pending style work, checked at the page root.
    #[must_use]
    pub fn has_dirty(&self) -> bool {
        match self.page {
            Some(page) => self
                .arena
                .get(page)
                .is_some_and(|widget| widget.style_dirty || widget.dirty_descendants),
            None => false,
        }
    }

    /// Clear every element's dirty bits (called after a restyle pass).
    pub fn clear_dirty(&mut self) {
        self.arena.clear_dirty();
    }
}
