//! The [`WidgetTree`] — owner of the document and the Element-PAPI surface.
//!
//! Methods here are shaped after Lynx's JS Element PAPI (renamed to `snake_case`
//! Rust). Each mirrors a `__*` PAPI opcode, so the method names keep the
//! `element` wording of the opcode they map to (e.g. [`append_element`] ↔
//! `__AppendElement`, [`insert_element_before`] ↔ `__InsertElementBefore`,
//! [`remove_element`] ↔ `__RemoveElement`, [`replace_element`] ↔
//! `__ReplaceElement`, [`create_element`] ↔ `__CreateElement`) even though
//! the values they carry are [`Widget`]s. JS
//! bindings live in a later runtime crate; this is the pure native validation
//! layer. [`crate::StyleEngine`] adapts `w3c-dom`'s generic cascade to the
//! Widget tree and reads the dirty state maintained here (see
//! [`WidgetTree::has_dirty`] / [`WidgetTree::clear_dirty`]).
//!
//! This layer validates PAPI semantics — stale handles, cycles, insertion
//! reference resolution, error mapping, the `unique_id` minting + index, the
//! `css_id` batch — and **delegates** every DOM operation to
//! [`w3c_dom::Document`] methods, which carry their own style invalidation.
//! PAPI opcodes arrive from the scripting runtime with untrusted handles, so
//! the validation here is what turns would-be contract violations (which the
//! DOM core treats as crashes) into [`WidgetError`]s. The Lynx-specific
//! per-widget data lives in each node's [`WidgetState`] payload.
//!
//! [`append_element`]: WidgetTree::append_element
//! [`insert_element_before`]: WidgetTree::insert_element_before
//! [`remove_element`]: WidgetTree::remove_element
//! [`replace_element`]: WidgetTree::replace_element
//! [`create_element`]: WidgetTree::create_element

use rustc_hash::FxHashMap;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;
use thiserror::Error;
use w3c_dom::{Document, ElementState};

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

/// The widget tree: one [`Document`] of [`Widget`]s plus the Lynx `unique_id`
/// counter and a `unique_id` → [`WidgetId`] index. The `<page>` root is the
/// document root.
#[derive(Debug)]
pub struct WidgetTree {
    doc: Document<WidgetState>,
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
        Self::from_document(Document::new())
    }

    pub(crate) fn from_document(doc: Document<WidgetState>) -> Self {
        Self {
            doc,
            // Lynx `unique_id`s are 1-based; 0 stays reserved as "unset".
            next_unique_id: 1,
            by_unique_id: FxHashMap::default(),
        }
    }

    /// Borrow the underlying document.
    #[must_use]
    pub const fn document(&self) -> &Document<WidgetState> {
        &self.doc
    }

    /// Mutably borrow the underlying document.
    ///
    /// The Widget style adapter uses this to flush and to schedule
    /// device-change restyles; everything it exposes carries its own
    /// invalidation.
    pub const fn document_mut(&mut self) -> &mut Document<WidgetState> {
        &mut self.doc
    }

    // --- element creation -------------------------------------------------

    fn create(&mut self, kind: WidgetKind, tag: &str) -> WidgetId {
        let unique_id = self.next_unique_id;
        self.next_unique_id = self.next_unique_id.wrapping_add(1);
        let id = self.doc.create_node(tag, WidgetState::new(kind, unique_id));
        self.by_unique_id.insert(unique_id, id);
        id
    }

    /// Create the `<page>` root element and record it as the document root
    /// (which also schedules its initial style pass).
    pub fn create_page(&mut self) -> WidgetId {
        let id = self.create(WidgetKind::Page, "page");
        self.doc.set_root(id);
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
        self.doc.set_text(id, Some(text.into()));
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
    /// insertion reference) happens here — PAPI handles are untrusted — and
    /// the link itself is delegated to [`Document::insert_before`], which
    /// carries the structural invalidation.
    pub fn insert_element_before(
        &mut self,
        child: WidgetId,
        parent: WidgetId,
        before: Option<WidgetId>,
    ) -> Result<(), WidgetError> {
        if !self.doc.contains(child) {
            return Err(WidgetError::StaleElement(child));
        }
        if !self.doc.contains(parent) {
            return Err(WidgetError::StaleElement(parent));
        }
        if child == parent || self.doc.is_ancestor(child, parent) {
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
                return if self.doc.child_position(parent, child).is_some() {
                    Ok(())
                } else {
                    Err(WidgetError::BadInsertReference(reference))
                };
            }
            if self.doc.child_position(parent, reference).is_none() {
                return Err(WidgetError::BadInsertReference(reference));
            }
        }

        self.doc.insert_before(parent, child, before);
        Ok(())
    }

    /// Remove `child` from `parent` and **free its entire subtree** (arena
    /// slots and `unique_id` index entries). All handles into the subtree
    /// become stale; removing the page also clears the document root.
    ///
    /// Deviation from web-core, by design: browser `__RemoveElement` is DOM
    /// `removeChild` — the element stays alive for whatever still references
    /// it, and the garbage collector reclaims it eventually. This engine has
    /// no GC and implements no list recycling (there is no consumer for
    /// detached-but-alive subtrees), so freeing immediately is the leak-free
    /// equivalent. Content that should stop rendering *without* being
    /// destroyed keeps its place in the tree — that is `content-visibility`'s
    /// job, not a detach-and-hold pool's.
    pub fn remove_element(&mut self, parent: WidgetId, child: WidgetId) -> Result<(), WidgetError> {
        let Some(child_widget) = self.doc.get(child) else {
            return Err(WidgetError::StaleElement(child));
        };
        if child_widget.parent_id() != Some(parent) {
            return Err(WidgetError::NotAChild { parent, child });
        }

        // `remove_subtree` detaches (with the structural invalidation at the
        // old location) and frees; harvest the returned widget states to keep
        // the `unique_id` index consistent.
        for state in self.doc.remove_subtree(child) {
            self.by_unique_id.remove(&state.unique_id);
        }
        Ok(())
    }

    /// Replace `old` with `new` in the tree, keeping `old`'s position. `new`
    /// is detached from any current parent first; `old` and its subtree are
    /// **freed** (see [`remove_element`](Self::remove_element) — no GC, no
    /// recycling, so nothing may linger detached-but-alive).
    ///
    /// Replacing a detached `old` is a no-op, matching DOM `replaceWith` on a
    /// parentless node — reachable only for a freshly created, never-attached
    /// `old`.
    pub fn replace_element(&mut self, new: WidgetId, old: WidgetId) -> Result<(), WidgetError> {
        if new == old {
            return Ok(());
        }
        let Some(old_widget) = self.doc.get(old) else {
            return Err(WidgetError::StaleElement(old));
        };
        let Some(parent) = old_widget.parent_id() else {
            return Ok(());
        };
        self.insert_element_before(new, parent, Some(old))?;
        self.remove_element(parent, old)
    }

    /// The first child of `parent`, if any.
    #[must_use]
    pub fn first_element(&self, parent: WidgetId) -> Option<WidgetId> {
        self.doc.get(parent)?.child_ids().first().copied()
    }

    /// The next sibling of `widget`, if any.
    #[must_use]
    pub fn next_element(&self, widget: WidgetId) -> Option<WidgetId> {
        let parent = self.doc.get(widget)?.parent_id()?;
        let siblings = self.doc.get(parent)?.child_ids();
        let pos = siblings.iter().position(|&c| c == widget)?;
        siblings.get(pos + 1).copied()
    }

    /// The parent of `widget`, if any.
    #[must_use]
    pub fn get_parent(&self, widget: WidgetId) -> Option<WidgetId> {
        self.doc.get(widget)?.parent_id()
    }

    // --- styling / attributes ---------------------------------------------

    /// Resolve a live widget for a mutation opcode, mapping staleness to the
    /// PAPI error.
    fn check_live(&self, id: WidgetId) -> Result<(), WidgetError> {
        if self.doc.contains(id) {
            Ok(())
        } else {
            Err(WidgetError::StaleElement(id))
        }
    }

    /// Replace an element's classes from a whitespace-separated list.
    pub fn set_classes(&mut self, id: WidgetId, classes: &str) -> Result<(), WidgetError> {
        self.check_live(id)?;
        self.doc.set_classes(id, classes);
        Ok(())
    }

    /// Add a single class (no-op if already present).
    pub fn add_class(&mut self, id: WidgetId, class: &str) -> Result<(), WidgetError> {
        self.check_live(id)?;
        self.doc.add_class(id, class);
        Ok(())
    }

    /// Replace an element's inline style, parsing the whole declaration block
    /// through stylo (Lynx's `__SetInlineStyles`). An empty string clears it.
    pub fn set_inline_styles(&mut self, id: WidgetId, text: &str) -> Result<(), WidgetError> {
        self.check_live(id)?;
        self.doc.set_inline_style(id, text);
        Ok(())
    }

    /// Parse a single `name: value` declaration through stylo and merge it into
    /// the element's inline style block (Lynx's `__AddInlineStyle`).
    ///
    /// An unparseable property/value is dropped (CSS error handling).
    pub fn add_inline_style(
        &mut self,
        id: WidgetId,
        name: &str,
        value: &str,
    ) -> Result<(), WidgetError> {
        self.check_live(id)?;
        self.doc.add_inline_style(id, name, value);
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
        self.check_live(id)?;
        self.doc.set_attribute(id, name, value);
        Ok(())
    }

    /// Set an element's id selector value (Lynx's `__SetID`). An empty string
    /// clears it.
    pub fn set_id(&mut self, id: WidgetId, id_selector: &str) -> Result<(), WidgetError> {
        self.check_live(id)?;
        self.doc
            .set_id_attr(id, (!id_selector.is_empty()).then_some(id_selector));
        Ok(())
    }

    /// Set the `css_id` (style scope) on a batch of elements.
    pub fn set_css_id(&mut self, ids: &[WidgetId], css_id: i32) -> Result<(), WidgetError> {
        if let Some(&bad) = ids.iter().find(|&&id| !self.doc.contains(id)) {
            return Err(WidgetError::StaleElement(bad));
        }
        for &id in ids {
            // The css_id is reflected as the synthetic `l-css-id` attribute;
            // snapshot it before the payload mutation.
            self.doc.note_external_attribute_change(id, "l-css-id");
            self.doc.ext_mut(id).css_id = css_id;
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
        self.check_live(id)?;
        // Snapshot the old values first, and name every old reflected
        // attribute while they still exist.
        self.doc.note_external_attributes_change(id);
        let old_keys: Vec<String> = self
            .doc
            .get(id)
            .map(|widget| {
                widget
                    .ext()
                    .dataset
                    .keys()
                    .map(|key| format!("data-{key}"))
                    .collect()
            })
            .unwrap_or_default();
        for key in &old_keys {
            self.doc.note_external_attribute_change(id, key);
        }

        self.doc.ext_mut(id).dataset = entries
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();

        // Names that only exist in the new dataset still need to be flagged
        // as changed; the snapshot keeps the pre-mutation values.
        let new_keys: Vec<String> = self
            .doc
            .get(id)
            .map(|widget| {
                widget
                    .ext()
                    .dataset
                    .keys()
                    .map(|key| format!("data-{key}"))
                    .collect()
            })
            .unwrap_or_default();
        for key in &new_keys {
            if !old_keys.contains(key) {
                self.doc.note_external_attribute_change(id, key);
            }
        }
        Ok(())
    }

    /// Add or overwrite a single `data-*` dataset entry.
    pub fn add_dataset(&mut self, id: WidgetId, key: &str, value: &str) -> Result<(), WidgetError> {
        self.check_live(id)?;
        self.doc
            .note_external_attribute_change(id, &format!("data-{key}"));
        self.doc
            .ext_mut(id)
            .dataset
            .insert(key.into(), value.to_owned());
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
        self.check_live(id)?;
        self.doc.ext_mut(id).events.push(EventReg {
            name: name.into(),
            kind,
            handler: handler.into(),
        });
        Ok(())
    }

    /// Toggle one or more dynamic pseudo-class flags (`:hover` / `:active` /
    /// `:focus`, as [`ElementState`] bits) on an element.
    pub fn set_pseudo_state(
        &mut self,
        id: WidgetId,
        state: ElementState,
        on: bool,
    ) -> Result<(), WidgetError> {
        self.check_live(id)?;
        self.doc.set_state(id, state, on);
        Ok(())
    }

    // --- getters ----------------------------------------------------------

    /// An element's Lynx tag name.
    #[must_use]
    pub fn get_tag(&self, id: WidgetId) -> Option<&str> {
        self.doc.get(id).map(Widget::tag)
    }

    /// An element's plain attribute map.
    #[must_use]
    pub fn get_attributes(&self, id: WidgetId) -> Option<&FxHashMap<Box<str>, String>> {
        self.doc.get(id).map(Widget::attrs)
    }

    /// An element's Lynx `unique_id`.
    #[must_use]
    pub fn get_element_unique_id(&self, id: WidgetId) -> Option<i32> {
        self.doc.get(id).map(|widget| widget.ext().unique_id)
    }

    /// An element's active dynamic pseudo-classes, as [`ElementState`] bits.
    #[must_use]
    pub fn pseudo_state(&self, id: WidgetId) -> Option<ElementState> {
        self.doc.get(id).map(Widget::element_state)
    }

    /// Resolve a Lynx `unique_id` back to its [`WidgetId`].
    #[must_use]
    pub fn element_by_unique_id(&self, unique_id: i32) -> Option<WidgetId> {
        self.by_unique_id.get(&unique_id).copied()
    }

    /// The tree's `<page>` root (the document root), if one has been created.
    #[must_use]
    pub fn get_page_element(&self) -> Option<WidgetId> {
        self.doc.root()
    }

    /// Borrow an element's [`Widget`], if live.
    #[must_use]
    pub fn widget(&self, id: WidgetId) -> Option<&Widget> {
        self.doc.get(id)
    }

    /// A read-only navigation handle for an element, if live (the same
    /// `&Widget` as [`widget`](Self::widget); kept as the name PAPI-side
    /// callers navigate through).
    #[must_use]
    pub fn widget_ref(&self, id: WidgetId) -> Option<WidgetRef<'_>> {
        self.doc.get(id)
    }

    /// An element's resolved computed style, if it has been styled.
    ///
    /// The style lives in stylo's per-element data; the `Arc` clone is cheap.
    #[must_use]
    pub fn computed(&self, id: WidgetId) -> Option<Arc<ComputedValues>> {
        self.doc.get(id).and_then(Widget::computed_style)
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
        self.check_live(id)?;
        self.doc.store_computed_style(id, style);
        Ok(())
    }

    // --- dirty state ------------------------------------------------------

    /// Whether the tree has any pending style work, checked at the page root.
    #[must_use]
    pub fn has_dirty(&self) -> bool {
        self.doc.needs_flush()
    }

    /// Clear every element's dirty bits (called after a restyle pass).
    pub fn clear_dirty(&mut self) {
        self.doc.clear_dirty();
    }
}
