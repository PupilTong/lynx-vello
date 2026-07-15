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
//! layer. [`crate::StyleEngine`] adapts `stylo-dom`'s generic cascade to the
//! Widget tree and reads the dirty state maintained here (see
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
use stylo_atoms::Atom;
use stylo_dom::{Arena, PseudoState};
use thiserror::Error;

use crate::kind::WidgetKind;
use crate::state::{EventKind, EventReg, WidgetState};
use crate::{Widget, WidgetId};

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
    /// Create a standalone Widget tree for DOM-only use.
    ///
    /// A tree that will be styled should be created with
    /// [`StyleEngine::new_widget_tree`](crate::StyleEngine::new_widget_tree),
    /// which binds it to the generic style engine's private context.
    #[must_use]
    pub fn new() -> Self {
        Self::from_arena(Arena::new())
    }

    pub(crate) fn from_arena(arena: Arena<WidgetState>) -> Self {
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
    /// The Widget style adapter uses this to write resolved computed styles
    /// and clear dirty bits after a resolution pass.
    pub const fn arena_mut(&mut self) -> &mut Arena<WidgetState> {
        &mut self.arena
    }

    // --- element creation -------------------------------------------------

    fn create(&mut self, kind: WidgetKind, tag: &str) -> WidgetId {
        let unique_id = self.next_unique_id;
        self.next_unique_id = self.next_unique_id.wrapping_add(1);
        let id = self
            .arena
            .create_element(tag, WidgetState::new(kind, unique_id));
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
        if let Some(widget_text) = self.arena.text_mut(id) {
            *widget_text = Some(text.into());
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
        if !self.arena.contains(id) {
            return Err(WidgetError::StaleElement(id));
        }
        // Snapshot the old class list before mutating so the flush can run
        // invalidation-set matching against old vs. new.
        self.arena.note_class_change(id);
        if let Some(widget_classes) = self.arena.classes_mut(id) {
            *widget_classes = classes.split_whitespace().map(Atom::from).collect();
        }
        Ok(())
    }

    /// Add a single class (no-op if already present).
    pub fn add_class(&mut self, id: WidgetId, class: &str) -> Result<(), WidgetError> {
        let class = Atom::from(class);
        match self.arena.get(id) {
            Some(widget) => {
                if widget.classes.contains(&class) {
                    return Ok(());
                }
            }
            None => return Err(WidgetError::StaleElement(id)),
        }
        self.arena.note_class_change(id);
        if let Some(widget_classes) = self.arena.classes_mut(id) {
            widget_classes.push(class);
        }
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
        if !self.arena.contains(id) {
            return Err(WidgetError::StaleElement(id));
        }
        self.arena.note_attribute_change(id, name);
        if let Some(widget_attrs) = self.arena.attrs_mut(id) {
            widget_attrs.insert(name.into(), value.to_owned());
        }
        Ok(())
    }

    /// Set an element's id selector value (Lynx's `__SetID`). An empty string
    /// clears it.
    pub fn set_id(&mut self, id: WidgetId, id_selector: &str) -> Result<(), WidgetError> {
        if !self.arena.contains(id) {
            return Err(WidgetError::StaleElement(id));
        }
        self.arena.note_id_change(id);
        if let Some(widget_id) = self.arena.id_attr_mut(id) {
            *widget_id = if id_selector.is_empty() {
                None
            } else {
                Some(Atom::from(id_selector))
            };
        }
        Ok(())
    }

    /// Set the `css_id` (style scope) on a batch of elements.
    pub fn set_css_id(&mut self, ids: &[WidgetId], css_id: i32) -> Result<(), WidgetError> {
        if let Some(&bad) = ids.iter().find(|&&id| !self.arena.contains(id)) {
            return Err(WidgetError::StaleElement(bad));
        }
        for &id in ids {
            // The css_id is reflected as the synthetic `l-css-id` attribute,
            // so snapshot it as an attribute change.
            self.arena.note_attribute_change(id, "l-css-id");
            if let Some(widget_state) = self.arena.ext_mut(id) {
                widget_state.css_id = css_id;
            }
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
        if !self.arena.contains(id) {
            return Err(WidgetError::StaleElement(id));
        }
        // Snapshot the old values first, and name every old reflected
        // attribute while they still exist.
        self.arena.note_other_attributes_change(id);
        let old_keys: Vec<String> = self
            .arena
            .get(id)
            .map(|widget| {
                widget
                    .ext
                    .dataset
                    .keys()
                    .map(|key| format!("data-{key}"))
                    .collect()
            })
            .unwrap_or_default();
        for key in &old_keys {
            self.arena.note_attribute_change(id, key);
        }

        if let Some(widget_state) = self.arena.ext_mut(id) {
            widget_state.dataset = entries
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect();
        }

        // Names that only exist in the new dataset still need to be flagged
        // as changed; the snapshot keeps the pre-mutation values.
        let new_keys: Vec<String> = self
            .arena
            .get(id)
            .map(|widget| {
                widget
                    .ext
                    .dataset
                    .keys()
                    .map(|key| format!("data-{key}"))
                    .collect()
            })
            .unwrap_or_default();
        for key in &new_keys {
            if !old_keys.contains(key) {
                self.arena.note_attribute_change(id, key);
            }
        }
        Ok(())
    }

    /// Add or overwrite a single `data-*` dataset entry.
    pub fn add_dataset(&mut self, id: WidgetId, key: &str, value: &str) -> Result<(), WidgetError> {
        if !self.arena.contains(id) {
            return Err(WidgetError::StaleElement(id));
        }
        self.arena.note_attribute_change(id, &format!("data-{key}"));
        if let Some(widget_state) = self.arena.ext_mut(id) {
            widget_state.dataset.insert(key.into(), value.to_owned());
        }
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
        match self.arena.ext_mut(id) {
            Some(widget_state) => widget_state.events.push(EventReg {
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
        if !self.arena.contains(id) {
            return Err(WidgetError::StaleElement(id));
        }
        self.arena.note_state_change(id);
        if let Some(element_state) = self.arena.element_state_mut(id) {
            element_state.set(state.to_element_state(), on);
        }
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

    /// Borrow an element for read-only tree navigation, if live.
    #[must_use]
    pub fn widget_ref(&self, id: WidgetId) -> Option<&Widget> {
        self.arena.node(id)
    }

    /// An element's resolved computed style, if it has been styled.
    ///
    /// The style lives in stylo's per-element data; the `Arc` clone is cheap.
    #[must_use]
    pub fn computed(&self, id: WidgetId) -> Option<Arc<ComputedValues>> {
        self.arena.get(id).and_then(Widget::computed_style)
    }

    /// Store an element's resolved computed style and clear its `style_dirty`
    /// bit. Used with the standalone
    /// [`StyleEngine::resolve_widget`](crate::StyleEngine::resolve_widget)
    /// path; [`StyleEngine::flush_widget_tree`](crate::StyleEngine::flush_widget_tree)
    /// stores styles itself.
    pub fn set_computed(
        &mut self,
        id: WidgetId,
        style: Arc<ComputedValues>,
    ) -> Result<(), WidgetError> {
        if self.arena.store_computed_style(id, style) {
            Ok(())
        } else {
            Err(WidgetError::StaleElement(id))
        }
    }

    // --- dirty state ------------------------------------------------------

    /// Whether the tree has any pending style work, checked at the page root.
    #[must_use]
    pub fn has_dirty(&self) -> bool {
        match self.page {
            Some(page) => self
                .arena
                .get(page)
                .is_some_and(|widget| widget.is_style_dirty() || widget.has_dirty_descendants()),
            None => false,
        }
    }

    /// Clear every element's dirty bits (called after a restyle pass).
    pub fn clear_dirty(&mut self) {
        self.arena.clear_dirty();
    }
}
