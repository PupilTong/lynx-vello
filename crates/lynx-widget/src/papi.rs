//! The [`WidgetTree`] — owner of the document and the Element-PAPI surface.
//!
//! Methods here are shaped after Lynx's JS Element PAPI (renamed to `snake_case`
//! Rust). Each mirrors a `__*` PAPI opcode, so the method names keep the
//! `element` wording of the opcode they map to (e.g. [`append_element`] ↔
//! `__AppendElement`, [`insert_element_before`] ↔ `__InsertElementBefore`,
//! [`remove_element`] ↔ `__RemoveElement`, [`replace_element`] ↔
//! `__ReplaceElement`, [`create_element`] ↔ `__CreateElement`) even though
//! the values they carry are [`Widget`]s. JS bindings live in a later runtime
//! crate; this is the pure native validation layer. [`crate::StyleEngine`]
//! adapts `w3c-dom`'s generic cascade to the Widget tree and reads the dirty
//! state maintained here (see [`WidgetTree::has_dirty`] /
//! [`WidgetTree::clear_dirty`]).
//!
//! This layer validates PAPI semantics — foreign/stale handles, cycles,
//! insertion reference resolution, root protection, error mapping, the
//! `unique_id` minting + index, the `css_id` batch — and **delegates** every
//! DOM operation to [`w3c_dom::Document`] methods, which carry their own
//! style invalidation. PAPI opcodes arrive from the scripting runtime, so the
//! validation here is what turns would-be contract violations (which the DOM
//! core treats as crashes) into [`WidgetError`]s. The Lynx-specific
//! per-widget data lives in each node's [`WidgetState`] payload.
//!
//! # Ownership and lifetime
//!
//! Widgets are **owned by the scripting engine**: `ReactLynx` runs inside the
//! JS engine, and each JS element wrapper holds a [`WidgetHandle`] clone. The
//! tree is storage plus structure — never lifetime policy:
//!
//! - The PAPI traffics **exclusively in handles**. A handle carries its tree's identity (using it
//!   on another tree is [`WidgetError::ForeignWidget`], not silent same-slot aliasing), and a live
//!   handle **retains** its node — nothing a wrapper can still reach is ever freed.
//! - Structural opcodes ([`remove_element`], [`replace_element`], …) attach and detach; they never
//!   free. Detached subtrees are first-class DOM citizens (browser `removeChild` semantics): alive,
//!   mutable, re-insertable.
//! - Freeing is a **consequence of ownership**, not an opcode: when the last handle into a detached
//!   subtree drops (wrapper finalizers, in the runtime), the subtree is reclaimed atomically at the
//!   next operation boundary — the native equivalent of the browser GC collecting unreferenced
//!   detached nodes. There is no public disposal API. See [`crate::handle`].
//!
//! [`append_element`]: WidgetTree::append_element
//! [`insert_element_before`]: WidgetTree::insert_element_before
//! [`remove_element`]: WidgetTree::remove_element
//! [`replace_element`]: WidgetTree::replace_element
//! [`create_element`]: WidgetTree::create_element

use std::num::NonZeroU64;
use std::sync::{Arc as StdArc, Mutex, Weak};

use rustc_hash::FxHashMap;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;
use thiserror::Error;
use w3c_dom::{Document, ElementState, Node, NodeId};

use crate::handle::{HandleInner, Reaper, WidgetHandle};
use crate::kind::WidgetKind;
use crate::state::{EventKind, EventReg, WidgetState};
use crate::{Widget, WidgetRef};

/// An error from a [`WidgetTree`] operation on untrusted PAPI input.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Error)]
pub enum WidgetError {
    /// A handle from a different [`WidgetTree`] was used on this one.
    #[error("widget {0:?} belongs to a different tree")]
    ForeignWidget(NodeId),
    /// A handle did not resolve to a live element.
    #[error("widget {0:?} is stale or does not exist")]
    StaleElement(NodeId),
    /// A `remove`/`replace` target was not a child of the given parent.
    #[error("widget {child:?} is not a child of {parent:?}")]
    NotAChild {
        /// The claimed parent.
        parent: NodeId,
        /// The element that was not actually its child.
        child: NodeId,
    },
    /// Performing the insertion would make an element its own ancestor.
    #[error("linking {ancestor:?} under {descendant:?} would create a cycle")]
    WouldCycle {
        /// The element being inserted (would become an ancestor of itself).
        ancestor: NodeId,
        /// The intended parent (a descendant of `ancestor`).
        descendant: NodeId,
    },
    /// An `insert_element_before` reference node was not a child of the parent.
    #[error("insertion reference {0:?} is not a child of the parent")]
    BadInsertReference(NodeId),
    /// The `<page>` root cannot be linked under a parent.
    #[error("the page root {0:?} cannot be reparented")]
    CannotReparentRoot(NodeId),
}

/// The widget tree: one [`Document`] of [`Widget`]s plus the Lynx `unique_id`
/// counter/index and the canonical [`WidgetHandle`] registry. The `<page>`
/// root is the document root.
#[derive(Debug)]
pub struct WidgetTree {
    doc: Document<WidgetState>,
    /// The next Lynx `unique_id` to mint (1-based; 0 stays reserved as
    /// "unset").
    next_unique_id: i32,
    by_unique_id: FxHashMap<i32, NodeId>,
    /// Canonical handle registry: at most one live [`HandleInner`] per node,
    /// so `Arc` strong counts are exactly the outstanding external
    /// references. Interior-mutable so `&self` navigation can mint handles.
    handles: Mutex<FxHashMap<NodeId, Weak<HandleInner>>>,
    /// Drop-notification queue shared with every handle (see
    /// [`crate::handle`]).
    reaper: StdArc<Reaper>,
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
            handles: Mutex::new(FxHashMap::default()),
            reaper: Reaper::new(),
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

    // --- handles ------------------------------------------------------------

    /// Resolve a handle for use on **this** tree, mapping the two ways
    /// untrusted input can be wrong to typed errors.
    ///
    /// `StaleElement` is defensive: a live handle retains its node, so a
    /// same-tree handle whose node is gone indicates a bug in this layer —
    /// debug builds assert.
    fn resolve(&self, handle: &WidgetHandle) -> Result<NodeId, WidgetError> {
        let id = handle.id();
        if handle.tree_token() != self.doc.token() {
            return Err(WidgetError::ForeignWidget(id));
        }
        if !self.doc.contains(id) {
            debug_assert!(
                false,
                "a live same-tree WidgetHandle must retain its node (registry bug)"
            );
            return Err(WidgetError::StaleElement(id));
        }
        Ok(id)
    }

    /// The canonical handle for a live node: clones the existing one when any
    /// external clone is alive, mints (and registers) a fresh one otherwise.
    fn handle_for(&self, id: NodeId) -> WidgetHandle {
        debug_assert!(self.doc.contains(id), "handle_for on a freed node");
        let mut registry = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = registry.get(&id).and_then(Weak::upgrade) {
            return WidgetHandle { inner: existing };
        }
        let inner = StdArc::new(HandleInner::new(self.doc.token(), id, &self.reaper));
        registry.insert(id, StdArc::downgrade(&inner));
        WidgetHandle { inner }
    }

    /// Drain the handle-drop queue and reclaim every **detached** subtree
    /// that no longer contains any externally retained node.
    ///
    /// Called at every mutating operation boundary and before each flush; a
    /// single relaxed load when nothing was dropped. Attached nodes are never
    /// collected — the tree retains document content, like the browser DOM.
    pub(crate) fn sweep_dropped(&mut self) {
        let Some(dropped) = self.reaper.take_dropped() else {
            return;
        };
        for id in dropped {
            // Lazily drop the dead registry entry (a re-lookup may already
            // have re-minted one; only remove if truly dead).
            {
                let mut registry = self
                    .handles
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if registry
                    .get(&id)
                    .is_some_and(|weak| weak.strong_count() == 0)
                {
                    registry.remove(&id);
                }
            }
            if !self.doc.contains(id) {
                continue; // already reclaimed by an earlier entry in this batch
            }
            // Find the top of the tree `id` sits in.
            let mut top = id;
            while let Some(parent) = self.doc.get(top).and_then(Node::parent_id) {
                top = parent;
            }
            if self.doc.root() == Some(top) {
                continue; // attached content: retained by the tree itself
            }
            let (retained, subtree) = self.subtree_retention(top);
            if retained {
                continue;
            }
            // No external handle anywhere in the detached subtree: reclaim it
            // atomically — slots, unique_id index, registry entries.
            for state in self.doc.remove_subtree(top) {
                self.by_unique_id.remove(&state.unique_id);
            }
            let mut registry = self
                .handles
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for freed in subtree {
                registry.remove(&freed);
            }
        }
    }

    /// Whether any node in the subtree rooted at `root` has a live external
    /// handle, plus the subtree's ids (reused by the reclamation path).
    fn subtree_retention(&self, root: NodeId) -> (bool, Vec<NodeId>) {
        let registry = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut retained = false;
        let mut ids = Vec::new();
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            ids.push(current);
            if registry
                .get(&current)
                .is_some_and(|weak| weak.strong_count() > 0)
            {
                retained = true;
            }
            if let Some(node) = self.doc.get(current) {
                stack.extend_from_slice(node.child_ids());
            }
        }
        (retained, ids)
    }

    // --- element creation -------------------------------------------------

    fn create(&mut self, kind: WidgetKind, tag: &str) -> WidgetHandle {
        self.sweep_dropped();
        let unique_id = self.next_unique_id;
        self.next_unique_id = self.next_unique_id.wrapping_add(1);
        let id = self.doc.create_node(tag, WidgetState::new(kind, unique_id));
        self.by_unique_id.insert(unique_id, id);
        self.handle_for(id)
    }

    /// Create the `<page>` root element and record it as the document root
    /// (which also schedules its initial style pass).
    pub fn create_page(&mut self) -> WidgetHandle {
        let handle = self.create(WidgetKind::Page, "page");
        self.doc.set_root(handle.id());
        handle
    }

    /// Create a `<view>` element.
    pub fn create_view(&mut self) -> WidgetHandle {
        self.create(WidgetKind::View, "view")
    }

    /// Create a `<text>` element.
    pub fn create_text(&mut self) -> WidgetHandle {
        self.create(WidgetKind::Text, "text")
    }

    /// Create a `<raw-text>` leaf carrying literal text content.
    pub fn create_raw_text(&mut self, text: impl Into<String>) -> WidgetHandle {
        let handle = self.create(WidgetKind::RawText, "raw-text");
        self.doc.set_text(handle.id(), Some(text.into()));
        handle
    }

    /// Create an `<image>` element.
    pub fn create_image(&mut self) -> WidgetHandle {
        self.create(WidgetKind::Image, "image")
    }

    /// Create a `<scroll-view>` element.
    pub fn create_scroll_view(&mut self) -> WidgetHandle {
        self.create(WidgetKind::ScrollView, "scroll-view")
    }

    /// Create a `<list>` element.
    pub fn create_list(&mut self) -> WidgetHandle {
        self.create(WidgetKind::List, "list")
    }

    /// Create a `<wrapper>` element.
    pub fn create_wrapper(&mut self) -> WidgetHandle {
        self.create(WidgetKind::Wrapper, "wrapper")
    }

    /// Create an element from an arbitrary Lynx tag name. The tag is classified
    /// via [`WidgetKind::from_tag`].
    pub fn create_element(&mut self, tag: &str) -> WidgetHandle {
        let kind = WidgetKind::from_tag(tag);
        self.create(kind, tag)
    }

    // --- tree mutation ----------------------------------------------------

    /// Append `child` as the last child of `parent`.
    pub fn append_element(
        &mut self,
        child: &WidgetHandle,
        parent: &WidgetHandle,
    ) -> Result<(), WidgetError> {
        self.insert_element_before(child, parent, None)
    }

    /// Insert `child` into `parent` immediately before `before`, or append it
    /// when `before` is `None`.
    ///
    /// `child` is first detached from any current parent. Re-inserting within
    /// the same parent reorders it. Validation (foreign handles, cycles, root
    /// protection, the insertion reference) happens here — PAPI input is
    /// untrusted — and the link itself is delegated to
    /// [`Document::insert_before`], which carries the structural
    /// invalidation.
    pub fn insert_element_before(
        &mut self,
        child: &WidgetHandle,
        parent: &WidgetHandle,
        before: Option<&WidgetHandle>,
    ) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let child_id = self.resolve(child)?;
        let parent_id = self.resolve(parent)?;
        if self.doc.root() == Some(child_id) {
            // A reparented root would let removing its new ancestor free the
            // root out from under the document (the DOM core enforces the
            // same invariant by panicking; PAPI input gets a typed error).
            return Err(WidgetError::CannotReparentRoot(child_id));
        }
        if child_id == parent_id || self.doc.is_ancestor(child_id, parent_id) {
            return Err(WidgetError::WouldCycle {
                ancestor: child_id,
                descendant: parent_id,
            });
        }
        let before_id = before.map(|handle| self.resolve(handle)).transpose()?;
        if let Some(reference) = before_id {
            if reference == child_id {
                // DOM pre-insert: the reference resolves to `child`'s next
                // sibling, so `insertBefore(n, n)` keeps `n` exactly where it
                // is — a structural no-op (web-core parity).
                return if self.doc.child_position(parent_id, child_id).is_some() {
                    Ok(())
                } else {
                    Err(WidgetError::BadInsertReference(reference))
                };
            }
            if self.doc.child_position(parent_id, reference).is_none() {
                return Err(WidgetError::BadInsertReference(reference));
            }
        }

        self.doc.insert_before(parent_id, child_id, before_id);
        Ok(())
    }

    /// Remove `child` from `parent`, **detaching** it — DOM `removeChild`
    /// (web-core's `__RemoveElement`). The subtree stays alive, fully
    /// mutable, and re-insertable: detached nodes are ordinary browser
    /// behavior and first-class here.
    ///
    /// Removal says nothing about lifetime — structural opcodes never free.
    /// The detached subtree lives for as long as any [`WidgetHandle`] into it
    /// does, and is reclaimed automatically once the last one drops (see
    /// [`crate::handle`]).
    pub fn remove_element(
        &mut self,
        parent: &WidgetHandle,
        child: &WidgetHandle,
    ) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let parent_id = self.resolve(parent)?;
        let child_id = self.resolve(child)?;
        if self.doc.get(child_id).and_then(Node::parent_id) != Some(parent_id) {
            return Err(WidgetError::NotAChild {
                parent: parent_id,
                child: child_id,
            });
        }

        // `detach` unlinks and applies the structural invalidation (parent
        // subtree + parent's following siblings, for `:empty` + `+`/`~`).
        self.doc.detach(child_id);
        Ok(())
    }

    /// Replace `old` with `new` in the tree, keeping `old`'s position. `new`
    /// is detached from any current parent first; `old` ends up detached but
    /// alive (like DOM `replaceChild`, which returns the old node to its
    /// owner).
    ///
    /// Replacing a detached `old` is a no-op, matching DOM `replaceWith` on a
    /// parentless node.
    pub fn replace_element(
        &mut self,
        new: &WidgetHandle,
        old: &WidgetHandle,
    ) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let new_id = self.resolve(new)?;
        let old_id = self.resolve(old)?;
        if new_id == old_id {
            return Ok(());
        }
        let Some(parent_id) = self.doc.get(old_id).and_then(Node::parent_id) else {
            return Ok(());
        };
        if self.doc.root() == Some(new_id) {
            return Err(WidgetError::CannotReparentRoot(new_id));
        }
        if new_id == parent_id || self.doc.is_ancestor(new_id, parent_id) {
            return Err(WidgetError::WouldCycle {
                ancestor: new_id,
                descendant: parent_id,
            });
        }
        self.doc.insert_before(parent_id, new_id, Some(old_id));
        self.doc.detach(old_id);
        Ok(())
    }

    /// The first child of `parent`, if any.
    pub fn first_element(
        &self,
        parent: &WidgetHandle,
    ) -> Result<Option<WidgetHandle>, WidgetError> {
        let parent_id = self.resolve(parent)?;
        Ok(self
            .doc
            .get(parent_id)
            .and_then(|node| node.child_ids().first().copied())
            .map(|id| self.handle_for(id)))
    }

    /// The next sibling of `widget`, if any.
    pub fn next_element(&self, widget: &WidgetHandle) -> Result<Option<WidgetHandle>, WidgetError> {
        let id = self.resolve(widget)?;
        Ok(self
            .doc
            .get(id)
            .and_then(Node::next_sibling)
            .map(|node| self.handle_for(node.id())))
    }

    /// The parent of `widget`, if any.
    pub fn get_parent(&self, widget: &WidgetHandle) -> Result<Option<WidgetHandle>, WidgetError> {
        let id = self.resolve(widget)?;
        Ok(self
            .doc
            .get(id)
            .and_then(Node::parent_id)
            .map(|parent| self.handle_for(parent)))
    }

    // --- styling / attributes ---------------------------------------------

    /// Replace an element's classes from a whitespace-separated list.
    pub fn set_classes(&mut self, handle: &WidgetHandle, classes: &str) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let id = self.resolve(handle)?;
        self.doc.set_classes(id, classes);
        Ok(())
    }

    /// Add a single class (no-op if already present).
    pub fn add_class(&mut self, handle: &WidgetHandle, class: &str) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let id = self.resolve(handle)?;
        self.doc.add_class(id, class);
        Ok(())
    }

    /// Replace an element's inline style, parsing the whole declaration block
    /// through stylo (Lynx's `__SetInlineStyles`). An empty string clears it.
    pub fn set_inline_styles(
        &mut self,
        handle: &WidgetHandle,
        text: &str,
    ) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let id = self.resolve(handle)?;
        self.doc.set_inline_style(id, text);
        Ok(())
    }

    /// Parse a single `name: value` declaration through stylo and merge it into
    /// the element's inline style block (Lynx's `__AddInlineStyle`).
    ///
    /// An unparseable property/value is dropped (CSS error handling).
    pub fn add_inline_style(
        &mut self,
        handle: &WidgetHandle,
        name: &str,
        value: &str,
    ) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let id = self.resolve(handle)?;
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
        handle: &WidgetHandle,
        name: &str,
        value: &str,
    ) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let id = self.resolve(handle)?;
        self.doc.set_attribute(id, name, value);
        Ok(())
    }

    /// Set an element's id selector value (Lynx's `__SetID`). An empty string
    /// clears it.
    pub fn set_id(&mut self, handle: &WidgetHandle, id_selector: &str) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let id = self.resolve(handle)?;
        self.doc
            .set_id_attr(id, (!id_selector.is_empty()).then_some(id_selector));
        Ok(())
    }

    /// Set the `css_id` (style scope) on a batch of elements.
    pub fn set_css_id(
        &mut self,
        handles: &[&WidgetHandle],
        css_id: i32,
    ) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let ids = handles
            .iter()
            .map(|handle| self.resolve(handle))
            .collect::<Result<Vec<_>, _>>()?;
        for id in ids {
            // The css_id is reflected as the synthetic `l-css-id` attribute;
            // snapshot it before the payload mutation.
            self.doc.note_external_attribute_change(id, "l-css-id");
            self.doc.ext_mut(id).css_id = css_id;
        }
        Ok(())
    }

    /// Replace an element's `data-*` dataset.
    pub fn set_dataset<I, K, V>(
        &mut self,
        handle: &WidgetHandle,
        entries: I,
    ) -> Result<(), WidgetError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<Box<str>>,
        V: Into<String>,
    {
        self.sweep_dropped();
        let id = self.resolve(handle)?;
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
    pub fn add_dataset(
        &mut self,
        handle: &WidgetHandle,
        key: &str,
        value: &str,
    ) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let id = self.resolve(handle)?;
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
        handle: &WidgetHandle,
        kind: EventKind,
        name: &str,
        event_handler: &str,
    ) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let id = self.resolve(handle)?;
        self.doc.ext_mut(id).events.push(EventReg {
            name: name.into(),
            kind,
            handler: event_handler.into(),
        });
        Ok(())
    }

    /// Toggle one or more dynamic pseudo-class flags (`:hover` / `:active` /
    /// `:focus`, as [`ElementState`] bits) on an element.
    pub fn set_pseudo_state(
        &mut self,
        handle: &WidgetHandle,
        state: ElementState,
        on: bool,
    ) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let id = self.resolve(handle)?;
        self.doc.set_state(id, state, on);
        Ok(())
    }

    // --- getters ----------------------------------------------------------

    /// An element's Lynx tag name.
    pub fn get_tag(&self, handle: &WidgetHandle) -> Result<&str, WidgetError> {
        let id = self.resolve(handle)?;
        self.doc
            .get(id)
            .map(Widget::tag)
            .ok_or(WidgetError::StaleElement(id))
    }

    /// An element's plain attribute map.
    pub fn get_attributes(
        &self,
        handle: &WidgetHandle,
    ) -> Result<&FxHashMap<Box<str>, String>, WidgetError> {
        let id = self.resolve(handle)?;
        self.doc
            .get(id)
            .map(Widget::attrs)
            .ok_or(WidgetError::StaleElement(id))
    }

    /// An element's Lynx `unique_id`.
    pub fn get_element_unique_id(&self, handle: &WidgetHandle) -> Result<i32, WidgetError> {
        let id = self.resolve(handle)?;
        self.doc
            .get(id)
            .map(|widget| widget.ext().unique_id)
            .ok_or(WidgetError::StaleElement(id))
    }

    /// An element's active dynamic pseudo-classes, as [`ElementState`] bits.
    pub fn pseudo_state(&self, handle: &WidgetHandle) -> Result<ElementState, WidgetError> {
        let id = self.resolve(handle)?;
        self.doc
            .get(id)
            .map(Widget::element_state)
            .ok_or(WidgetError::StaleElement(id))
    }

    /// Resolve a Lynx `unique_id` back to its element, as the canonical
    /// handle. `None` for never-assigned ids and for elements already
    /// reclaimed (all handles dropped while detached).
    #[must_use]
    pub fn element_by_unique_id(&self, unique_id: i32) -> Option<WidgetHandle> {
        let id = *self.by_unique_id.get(&unique_id)?;
        self.doc.contains(id).then(|| self.handle_for(id))
    }

    /// The tree's `<page>` root (the document root), if one has been created.
    #[must_use]
    pub fn get_page_element(&self) -> Option<WidgetHandle> {
        self.doc.root().map(|id| self.handle_for(id))
    }

    /// Borrow an element's [`Widget`].
    pub fn widget(&self, handle: &WidgetHandle) -> Result<&Widget, WidgetError> {
        let id = self.resolve(handle)?;
        self.doc.get(id).ok_or(WidgetError::StaleElement(id))
    }

    /// A read-only navigation reference for an element (the same `&Widget` as
    /// [`widget`](Self::widget); kept as the name PAPI-side callers navigate
    /// through).
    pub fn widget_ref(&self, handle: &WidgetHandle) -> Result<WidgetRef<'_>, WidgetError> {
        self.widget(handle)
    }

    /// An element's resolved computed style, if it has been styled.
    ///
    /// The style lives in stylo's per-element data; the `Arc` clone is cheap.
    pub fn computed(
        &self,
        handle: &WidgetHandle,
    ) -> Result<Option<Arc<ComputedValues>>, WidgetError> {
        let id = self.resolve(handle)?;
        Ok(self.doc.get(id).and_then(Widget::computed_style))
    }

    /// Store an element's resolved computed style and clear its `style_dirty`
    /// bit. Used with the standalone
    /// [`StyleEngine::resolve_widget`](crate::StyleEngine::resolve_widget)
    /// path; [`StyleEngine::flush_widget_tree`](crate::StyleEngine::flush_widget_tree)
    /// stores styles itself.
    pub fn set_computed(
        &mut self,
        handle: &WidgetHandle,
        style: Arc<ComputedValues>,
    ) -> Result<(), WidgetError> {
        self.sweep_dropped();
        let id = self.resolve(handle)?;
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

    /// This tree's identity token (every handle it mints carries it).
    #[must_use]
    pub fn token(&self) -> NonZeroU64 {
        self.doc.token()
    }

    /// Reclaim now instead of at the next operation boundary: drain the
    /// handle-drop notifications and free every detached subtree with no
    /// externally retained node.
    ///
    /// Purely a scheduling hook (the same sweep runs automatically before
    /// every mutating opcode and every flush) — it cannot free anything a
    /// live [`WidgetHandle`] still reaches. The runtime layer calls this at
    /// its frame boundary after wrapper finalizers ran.
    pub fn collect(&mut self) {
        self.sweep_dropped();
    }
}
