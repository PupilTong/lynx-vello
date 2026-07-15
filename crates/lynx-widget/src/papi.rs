//! The [`WidgetTree`] — owner of the document and the Element-PAPI surface.
//!
//! Methods here are shaped after Lynx's JS Element PAPI (renamed to `snake_case`
//! Rust). Each mirrors a `__*` PAPI opcode, so the method names keep the
//! `element` wording of the opcode they map to (e.g. [`append_element`] ↔
//! `__AppendElement`, [`insert_element_before`] ↔ `__InsertElementBefore`,
//! [`remove_element`] ↔ `__RemoveElement`,
//! [`replace_element`] ↔ `__ReplaceElement`, [`create_element`] ↔
//! `__CreateElement`) even though the values they carry are [`Widget`]s. JS
//! bindings live in a later runtime crate; this is the pure native validation
//! layer. [`WidgetTree`] also adapts `stylo-dom`'s generic cascade and reads
//! the dirty state maintained here (see
//! [`WidgetTree::has_dirty`] / [`WidgetTree::clear_dirty`]).
//!
//! This layer validates PAPI semantics — foreign/invalid handles, cycles, insertion
//! reference resolution, error mapping, the `unique_id` minting + index, the
//! `css_id` batch — and **delegates** the actual tree mutation and inline-style
//! parsing to the [`stylo_dom`] crate's [`Document`] primitives, because their
//! invalidation is style-system logic. The Lynx-specific per-widget data lives
//! in the element's [`WidgetState`] payload.
//!
//! [`append_element`]: WidgetTree::append_element
//! [`insert_element_before`]: WidgetTree::insert_element_before
//! [`remove_element`]: WidgetTree::remove_element
//! [`replace_element`]: WidgetTree::replace_element
//! [`create_element`]: WidgetTree::create_element

use std::fmt;
use std::rc::Rc;

use rustc_hash::FxHashMap;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;
use stylo_atoms::Atom;
use stylo_dom::{Document, ElementId, PseudoState};
use thiserror::Error;

use crate::handle::TreeIdentity;
use crate::kind::WidgetKind;
use crate::state::{EventKind, EventReg, WidgetState};
use crate::style::EngineMetrics;
use crate::ua::PageConfig;
use crate::{Widget, WidgetHandle};

/// An error from a tree-mutating [`WidgetTree`] operation.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum WidgetError {
    /// A same-tree handle did not resolve to a live element. This indicates a
    /// lifecycle-invariant violation: strong handles must prevent reclamation.
    #[error("widget {0:?} is stale or does not exist")]
    StaleElement(WidgetHandle),
    /// A handle minted by a different Widget tree was passed to this tree.
    #[error("widget {0:?} belongs to a different WidgetTree")]
    ForeignElement(WidgetHandle),
    /// A `remove`/`replace` target was not a child of the given parent.
    #[error("widget {child:?} is not a child of {parent:?}")]
    NotAChild {
        /// The claimed parent.
        parent: WidgetHandle,
        /// The element that was not actually its child.
        child: WidgetHandle,
    },
    /// Performing the insertion would make an element its own ancestor.
    #[error("linking {ancestor:?} under {descendant:?} would create a cycle")]
    WouldCycle {
        /// The element being inserted (would become an ancestor of itself).
        ancestor: WidgetHandle,
        /// The intended parent (a descendant of `ancestor`).
        descendant: WidgetHandle,
    },
    /// An `insert_element_before` reference node was not a child of the parent.
    #[error("insertion reference {0:?} is not a child of the parent")]
    BadInsertReference(WidgetHandle),
}

/// The widget tree: an independent stylo [`Document`] of [`Widget`]s plus the current
/// page root, the Lynx `unique_id` counter, a canonical strong-handle
/// registry, and a `unique_id` → internal-id index.
pub struct WidgetTree {
    document: Document<WidgetState>,
    pub(crate) page_config: PageConfig,
    identity: Rc<TreeIdentity>,
    pub(crate) page: Option<ElementId>,
    /// The next Lynx `unique_id` to mint (1-based; 0 stays reserved as
    /// "unset").
    next_unique_id: i32,
    by_unique_id: FxHashMap<i32, ElementId>,
    /// Exactly one canonical `Rc` for every live document node. Any strong
    /// count above one is an external owner and blocks detached-subtree GC.
    handles: FxHashMap<ElementId, WidgetHandle>,
}

impl fmt::Debug for WidgetTree {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let page_unique_id = self
            .page
            .and_then(|id| self.handles.get(&id))
            .map(|handle| handle.unique_id());
        formatter
            .debug_struct("WidgetTree")
            .field("page_unique_id", &page_unique_id)
            .field("live_nodes", &self.handles.len())
            .field("next_unique_id", &self.next_unique_id)
            .finish_non_exhaustive()
    }
}

impl Default for WidgetTree {
    fn default() -> Self {
        Self::new()
    }
}

impl WidgetTree {
    /// Create a standalone Widget tree with a zero-sized default viewport.
    #[must_use]
    pub fn new() -> Self {
        Self::with_metrics(EngineMetrics::new(0.0, 0.0, 1.0))
    }

    pub(crate) fn from_document(document: Document<WidgetState>, page_config: PageConfig) -> Self {
        Self {
            document,
            page_config,
            identity: Rc::new(TreeIdentity),
            page: None,
            // Lynx `unique_id`s are 1-based; 0 stays reserved as "unset".
            next_unique_id: 1,
            by_unique_id: FxHashMap::default(),
            handles: FxHashMap::default(),
        }
    }

    /// Borrow the underlying generic document.
    #[must_use]
    pub(crate) const fn document(&self) -> &Document<WidgetState> {
        &self.document
    }

    /// Mutably borrow the underlying generic document.
    pub(crate) const fn document_mut(&mut self) -> &mut Document<WidgetState> {
        &mut self.document
    }

    pub(crate) fn id_for(&self, handle: &WidgetHandle) -> Result<ElementId, WidgetError> {
        if !Rc::ptr_eq(&self.identity, &handle.tree) {
            return Err(WidgetError::ForeignElement(handle.clone()));
        }
        let Some(canonical) = self.handles.get(&handle.id) else {
            debug_assert!(false, "a strong NodeHandle outlived its document node");
            return Err(WidgetError::StaleElement(handle.clone()));
        };
        debug_assert!(
            Rc::ptr_eq(canonical, handle),
            "one live node must have exactly one canonical NodeHandle allocation"
        );
        debug_assert!(
            self.document.contains(handle.id),
            "the handle registry must not contain a reclaimed node"
        );
        Ok(handle.id)
    }

    fn handle_for(&self, id: ElementId) -> WidgetHandle {
        self.handles
            .get(&id)
            .unwrap_or_else(|| panic!("live node has no canonical NodeHandle"))
            .clone()
    }

    #[cfg(debug_assertions)]
    fn assert_lifecycle_invariants(&self) {
        self.document.assert_tree_integrity();
        assert_eq!(
            self.document.len(),
            self.handles.len(),
            "every live node must have exactly one registry entry"
        );
        for (&id, handle) in &self.handles {
            assert_eq!(handle.id, id, "registry key must match its NodeHandle");
            assert!(
                Rc::ptr_eq(&handle.tree, &self.identity),
                "registry handle must belong to this tree"
            );
            assert!(
                self.document.contains(id),
                "registry must not retain a physically reclaimed node"
            );
        }
    }

    #[cfg(not(debug_assertions))]
    fn assert_lifecycle_invariants(&self) {}

    /// Reclaim detached subtrees after their last external strong handle has
    /// been dropped.
    ///
    /// Connected nodes are retained by the page tree. A detached subtree is
    /// reclaimed atomically only when every node in it has `Rc::strong_count
    /// == 1`, meaning the registry is its sole remaining owner. A strong handle
    /// to any descendant therefore retains the entire detached subtree.
    ///
    /// # Panics
    ///
    /// Panics when the document, canonical-handle registry, or tree topology
    /// violates its lifecycle invariants. These checks deliberately make an
    /// ownership-model bug fail at the collection boundary in debug and test
    /// builds.
    pub fn collect_garbage(&mut self) {
        self.assert_lifecycle_invariants();
        let retained_roots: Vec<_> = self.page.into_iter().collect();
        let handles = &self.handles;
        let removed = self.document.collect_detached(&retained_roots, |id| {
            let handle = handles
                .get(&id)
                .unwrap_or_else(|| panic!("live node has no canonical NodeHandle"));
            Rc::strong_count(handle) > 1
        });
        for (id, state) in removed {
            let handle = self
                .handles
                .remove(&id)
                .expect("collector returned an unregistered node");
            assert_eq!(
                Rc::strong_count(&handle),
                1,
                "collector attempted to reclaim an externally held node"
            );
            self.by_unique_id.remove(&state.unique_id);
        }
        self.assert_lifecycle_invariants();
    }

    // --- element creation -------------------------------------------------

    fn create(&mut self, kind: WidgetKind, tag: &str) -> WidgetHandle {
        self.collect_garbage();
        let unique_id = self.next_unique_id;
        self.next_unique_id = self.next_unique_id.wrapping_add(1);
        let id = self
            .document
            .create_element(tag, WidgetState::new(kind, unique_id));
        let handle = Rc::new(crate::NodeHandle::new(self.identity.clone(), id, unique_id));
        assert!(
            self.handles.insert(id, handle.clone()).is_none(),
            "new node reused a slot that still had a live handle"
        );
        self.by_unique_id.insert(unique_id, id);
        self.assert_lifecycle_invariants();
        handle
    }

    /// Create the `<page>` root element and record it as the tree's page.
    pub fn create_page(&mut self) -> WidgetHandle {
        let handle = self.create(WidgetKind::Page, "page");
        let id = handle.id;
        self.page = Some(id);
        // The root always needs an initial style pass.
        self.document.mark_style_dirty(id);
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
        if let Some(widget_text) = self.document.text_mut(handle.id) {
            *widget_text = Some(text.into());
        }
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
    /// the same parent reorders it. Validation (handle ownership, cycles, the
    /// insertion reference) happens here; the unlink/link is delegated to the
    /// [`Document`] tree primitives, which carry the structural invalidation.
    pub fn insert_element_before(
        &mut self,
        child: &WidgetHandle,
        parent: &WidgetHandle,
        before: Option<&WidgetHandle>,
    ) -> Result<(), WidgetError> {
        let child_id = self.id_for(child)?;
        let parent_id = self.id_for(parent)?;
        if child_id == parent_id || self.document.is_ancestor(child_id, parent_id) {
            return Err(WidgetError::WouldCycle {
                ancestor: child.clone(),
                descendant: parent.clone(),
            });
        }
        let before = match before {
            Some(reference) => Some((reference, self.id_for(reference)?)),
            None => None,
        };
        if let Some((reference, reference_id)) = before {
            if reference_id == child_id {
                // DOM pre-insert: the reference resolves to `child`'s next
                // sibling, so `insertBefore(n, n)` keeps `n` exactly where it
                // is — a structural no-op (web-core parity).
                return if self.document.is_child_of(child_id, parent_id) {
                    Ok(())
                } else {
                    Err(WidgetError::BadInsertReference(reference.clone()))
                };
            }
            if !self.document.is_child_of(reference_id, parent_id) {
                return Err(WidgetError::BadInsertReference(reference.clone()));
            }
        }

        self.document.detach(child_id);

        let index = match before {
            None => self.document.children_len(parent_id),
            Some((_, reference_id)) => self
                .document
                .child_position(parent_id, reference_id)
                .unwrap_or_else(|| self.document.children_len(parent_id)),
        };
        self.document.attach_at(parent_id, child_id, index);
        self.assert_lifecycle_invariants();
        Ok(())
    }

    /// Remove `child` from `parent`, **detaching** it — the subtree stays
    /// alive in the document (and in the `unique_id` index) and can be
    /// re-inserted later.
    ///
    /// This matches the Element PAPI contract: web-core's `__RemoveElement` is
    /// DOM `removeChild` (the element remains usable), and Lynx list recycling
    /// re-attaches previously removed subtrees. Physical reclamation happens
    /// only through [`collect_garbage`](Self::collect_garbage), after the last
    /// external strong handle to every node in the detached subtree is gone.
    pub fn remove_element(
        &mut self,
        parent: &WidgetHandle,
        child: &WidgetHandle,
    ) -> Result<(), WidgetError> {
        let parent_id = self.id_for(parent)?;
        let child_id = self.id_for(child)?;
        let Some(child_widget) = self.document.get(child_id) else {
            return Err(WidgetError::StaleElement(child.clone()));
        };
        if child_widget.parent != Some(parent_id) {
            return Err(WidgetError::NotAChild {
                parent: parent.clone(),
                child: child.clone(),
            });
        }

        // `detach` unlinks and applies the structural invalidation (parent
        // subtree + parent's following siblings, for `:empty` + `+`/`~`).
        self.document.detach(child_id);
        self.assert_lifecycle_invariants();
        Ok(())
    }

    /// Replace `old` with `new` in the tree, keeping `old`'s position. `new`
    /// is detached from any current parent first; `old` ends up detached but
    /// alive (like DOM `replaceChild`, which returns the old node).
    ///
    /// Replacing a detached `old` is a no-op, matching DOM `replaceWith` on a
    /// parentless node.
    pub fn replace_element(
        &mut self,
        new: &WidgetHandle,
        old: &WidgetHandle,
    ) -> Result<(), WidgetError> {
        let new_id = self.id_for(new)?;
        let old_id = self.id_for(old)?;
        if new_id == old_id {
            return Ok(());
        }
        let Some(old_widget) = self.document.get(old_id) else {
            return Err(WidgetError::StaleElement(old.clone()));
        };
        let Some(parent) = old_widget.parent else {
            return Ok(());
        };
        let parent = self.handle_for(parent);
        self.insert_element_before(new, &parent, Some(old))?;
        self.remove_element(&parent, old)
    }

    /// The first child of `parent`, if any.
    #[must_use]
    pub fn first_element(&self, parent: &WidgetHandle) -> Option<WidgetHandle> {
        let parent = self.id_for(parent).ok()?;
        self.document
            .get(parent)?
            .children
            .first()
            .copied()
            .map(|id| self.handle_for(id))
    }

    /// The next sibling of `widget`, if any.
    #[must_use]
    pub fn next_element(&self, widget: &WidgetHandle) -> Option<WidgetHandle> {
        let widget = self.id_for(widget).ok()?;
        let parent = self.document.get(widget)?.parent?;
        let siblings = &self.document.get(parent)?.children;
        let pos = siblings.iter().position(|&c| c == widget)?;
        siblings.get(pos + 1).copied().map(|id| self.handle_for(id))
    }

    /// The parent of `widget`, if any.
    #[must_use]
    pub fn get_parent(&self, widget: &WidgetHandle) -> Option<WidgetHandle> {
        let widget = self.id_for(widget).ok()?;
        self.document
            .get(widget)?
            .parent
            .map(|id| self.handle_for(id))
    }

    /// The children of `parent` in document order.
    #[must_use]
    pub fn children(&self, parent: &WidgetHandle) -> Option<Vec<WidgetHandle>> {
        let parent = self.id_for(parent).ok()?;
        Some(
            self.document
                .get(parent)?
                .children
                .iter()
                .map(|&id| self.handle_for(id))
                .collect(),
        )
    }

    // --- styling / attributes ---------------------------------------------

    /// Replace an element's classes from a whitespace-separated list.
    pub fn set_classes(&mut self, handle: &WidgetHandle, classes: &str) -> Result<(), WidgetError> {
        let id = self.id_for(handle)?;
        // Snapshot the old class list before mutating so the flush can run
        // invalidation-set matching against old vs. new.
        self.document.note_class_change(id);
        if let Some(widget_classes) = self.document.classes_mut(id) {
            *widget_classes = classes.split_whitespace().map(Atom::from).collect();
        }
        Ok(())
    }

    /// Add a single class (no-op if already present).
    pub fn add_class(&mut self, handle: &WidgetHandle, class: &str) -> Result<(), WidgetError> {
        let id = self.id_for(handle)?;
        let class = Atom::from(class);
        match self.document.get(id) {
            Some(widget) => {
                if widget.classes.contains(&class) {
                    return Ok(());
                }
            }
            None => unreachable!("a validated handle must resolve"),
        }
        self.document.note_class_change(id);
        if let Some(widget_classes) = self.document.classes_mut(id) {
            widget_classes.push(class);
        }
        Ok(())
    }

    /// Replace an element's inline style, parsing the whole declaration block
    /// through stylo (Lynx's `__SetInlineStyles`). An empty string clears it.
    ///
    /// The parse is delegated to the [`Document`] inline-style primitive.
    pub fn set_inline_styles(
        &mut self,
        handle: &WidgetHandle,
        text: &str,
    ) -> Result<(), WidgetError> {
        let id = self.id_for(handle)?;
        self.document.set_inline_styles(id, text);
        Ok(())
    }

    /// Parse a single `name: value` declaration through stylo and merge it into
    /// the element's inline style block (Lynx's `__AddInlineStyle`).
    ///
    /// The parse/merge is delegated to the [`Document`] inline-style primitive; an
    /// unparseable property/value is dropped.
    pub fn add_inline_style(
        &mut self,
        handle: &WidgetHandle,
        name: &str,
        value: &str,
    ) -> Result<(), WidgetError> {
        let id = self.id_for(handle)?;
        self.document.add_inline_style(id, name, value);
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
        let id = self.id_for(handle)?;
        self.document.note_attribute_change(id, name);
        if let Some(widget_attrs) = self.document.attrs_mut(id) {
            widget_attrs.insert(name.into(), value.to_owned());
        }
        Ok(())
    }

    /// Set an element's id selector value (Lynx's `__SetID`). An empty string
    /// clears it.
    pub fn set_id(&mut self, handle: &WidgetHandle, id_selector: &str) -> Result<(), WidgetError> {
        let id = self.id_for(handle)?;
        self.document.note_id_change(id);
        if let Some(widget_id) = self.document.id_attr_mut(id) {
            *widget_id = if id_selector.is_empty() {
                None
            } else {
                Some(Atom::from(id_selector))
            };
        }
        Ok(())
    }

    /// Set the `css_id` (style scope) on a batch of elements.
    pub fn set_css_id<'a>(
        &mut self,
        handles: impl IntoIterator<Item = &'a WidgetHandle>,
        css_id: i32,
    ) -> Result<(), WidgetError> {
        let ids: Vec<_> = handles
            .into_iter()
            .map(|handle| self.id_for(handle))
            .collect::<Result<_, _>>()?;
        for id in ids {
            // The css_id is reflected as the synthetic `l-css-id` attribute,
            // so snapshot it as an attribute change.
            self.document.note_attribute_change(id, "l-css-id");
            if let Some(widget_state) = self.document.ext_mut(id) {
                widget_state.css_id = css_id;
            }
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
        let id = self.id_for(handle)?;
        // Snapshot the old values first, and name every old reflected
        // attribute while they still exist.
        self.document.note_other_attributes_change(id);
        let old_keys: Vec<String> = self
            .document
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
            self.document.note_attribute_change(id, key);
        }

        if let Some(widget_state) = self.document.ext_mut(id) {
            widget_state.dataset = entries
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect();
        }

        // Names that only exist in the new dataset still need to be flagged
        // as changed; the snapshot keeps the pre-mutation values.
        let new_keys: Vec<String> = self
            .document
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
                self.document.note_attribute_change(id, key);
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
        let id = self.id_for(handle)?;
        self.document
            .note_attribute_change(id, &format!("data-{key}"));
        if let Some(widget_state) = self.document.ext_mut(id) {
            widget_state.dataset.insert(key.into(), value.to_owned());
        }
        Ok(())
    }

    /// Register an event binding on an element. (Does not affect style, so no
    /// invalidation.)
    pub fn add_event(
        &mut self,
        handle: &WidgetHandle,
        kind: EventKind,
        name: &str,
        handler: &str,
    ) -> Result<(), WidgetError> {
        let id = self.id_for(handle)?;
        match self.document.ext_mut(id) {
            Some(widget_state) => widget_state.events.push(EventReg {
                name: name.into(),
                kind,
                handler: handler.into(),
            }),
            None => unreachable!("a validated handle must resolve"),
        }
        Ok(())
    }

    /// Toggle one or more pseudo-class flags on an element.
    pub fn set_pseudo_state(
        &mut self,
        handle: &WidgetHandle,
        state: PseudoState,
        on: bool,
    ) -> Result<(), WidgetError> {
        let id = self.id_for(handle)?;
        self.document.note_state_change(id);
        if let Some(element_state) = self.document.element_state_mut(id) {
            element_state.set(state.to_element_state(), on);
        }
        Ok(())
    }

    // --- getters ----------------------------------------------------------

    /// Whether `handle` belongs to this tree and still names a live node.
    #[must_use]
    pub fn contains(&self, handle: &WidgetHandle) -> bool {
        self.id_for(handle).is_ok()
    }

    /// An element's Lynx tag name.
    #[must_use]
    pub fn get_tag(&self, handle: &WidgetHandle) -> Option<&str> {
        let id = self.id_for(handle).ok()?;
        self.document.get(id).map(Widget::tag_str)
    }

    /// An element's plain attribute map.
    #[must_use]
    pub fn get_attributes(&self, handle: &WidgetHandle) -> Option<&FxHashMap<Box<str>, String>> {
        let id = self.id_for(handle).ok()?;
        self.document.get(id).map(|widget| &widget.attrs)
    }

    /// The selector-facing `id` value set through [`set_id`](Self::set_id).
    #[must_use]
    pub fn get_id_selector(&self, handle: &WidgetHandle) -> Option<&str> {
        let id = self.id_for(handle).ok()?;
        self.document.get(id)?.id_attr.as_deref()
    }

    /// An element's Lynx `unique_id`.
    #[must_use]
    pub fn get_element_unique_id(&self, handle: &WidgetHandle) -> Option<i32> {
        self.id_for(handle).ok().map(|_| handle.unique_id())
    }

    /// An element's active dynamic pseudo-classes, as a [`PseudoState`].
    #[must_use]
    pub fn pseudo_state(&self, handle: &WidgetHandle) -> Option<PseudoState> {
        let id = self.id_for(handle).ok()?;
        self.document
            .get(id)
            .map(|widget| PseudoState::from_element_state(widget.element_state))
    }

    /// Resolve a Lynx `unique_id` back to its strong node handle.
    #[must_use]
    pub fn element_by_unique_id(&self, unique_id: i32) -> Option<WidgetHandle> {
        self.by_unique_id
            .get(&unique_id)
            .copied()
            .map(|id| self.handle_for(id))
    }

    /// The tree's `<page>` root, if one has been created.
    #[must_use]
    pub fn get_page_element(&self) -> Option<WidgetHandle> {
        self.page.map(|id| self.handle_for(id))
    }

    /// An element's Lynx widget kind.
    #[must_use]
    pub fn get_kind(&self, handle: &WidgetHandle) -> Option<WidgetKind> {
        let id = self.id_for(handle).ok()?;
        self.document.get(id).map(|widget| widget.ext.kind)
    }

    /// An element's literal character data, if any.
    #[must_use]
    pub fn get_text(&self, handle: &WidgetHandle) -> Option<&str> {
        let id = self.id_for(handle).ok()?;
        self.document.get(id)?.text.as_deref()
    }

    /// An element's class names.
    #[must_use]
    pub fn get_classes(&self, handle: &WidgetHandle) -> Option<Vec<&str>> {
        let id = self.id_for(handle).ok()?;
        Some(
            self.document
                .get(id)?
                .classes
                .iter()
                .map(|class| &**class)
                .collect(),
        )
    }

    /// An element's Lynx-specific state payload.
    ///
    /// This exposes no arena identity or topology; navigation always returns
    /// strong [`WidgetHandle`] values through the dedicated methods above.
    #[must_use]
    pub fn get_state(&self, handle: &WidgetHandle) -> Option<&WidgetState> {
        let id = self.id_for(handle).ok()?;
        self.document.get(id).map(|widget| &widget.ext)
    }

    /// Whether an inline declaration block is present.
    #[must_use]
    pub fn has_inline_styles(&self, handle: &WidgetHandle) -> Option<bool> {
        let id = self.id_for(handle).ok()?;
        self.document
            .get(id)
            .map(|widget| widget.inline_block.is_some())
    }

    /// Number of parsed declarations in the inline-style block.
    #[must_use]
    pub fn inline_style_declaration_count(&self, handle: &WidgetHandle) -> Option<usize> {
        let id = self.id_for(handle).ok()?;
        self.document.inline_style_declaration_count(id)
    }

    /// Whether this node itself has pending style work.
    #[must_use]
    pub fn is_style_dirty(&self, handle: &WidgetHandle) -> Option<bool> {
        let id = self.id_for(handle).ok()?;
        self.document.get(id).map(Widget::is_style_dirty)
    }

    /// Whether this node has pending work in its descendant subtree.
    #[must_use]
    pub fn has_dirty_descendants(&self, handle: &WidgetHandle) -> Option<bool> {
        let id = self.id_for(handle).ok()?;
        self.document.get(id).map(Widget::has_dirty_descendants)
    }

    /// An element's resolved computed style, if it has been styled.
    ///
    /// The style lives in stylo's per-element data; the `Arc` clone is cheap.
    #[must_use]
    pub fn computed(&self, handle: &WidgetHandle) -> Option<Arc<ComputedValues>> {
        let id = self.id_for(handle).ok()?;
        self.document.get(id).and_then(Widget::computed_style)
    }

    /// Store an element's resolved computed style and clear its `style_dirty`
    /// bit. Used with the standalone
    /// [`WidgetTree::resolve_widget`]
    /// path; [`WidgetTree::flush_styles`]
    /// stores styles itself.
    pub fn set_computed(
        &mut self,
        handle: &WidgetHandle,
        style: Arc<ComputedValues>,
    ) -> Result<(), WidgetError> {
        let id = self.id_for(handle)?;
        if self.document.store_computed_style(id, style) {
            Ok(())
        } else {
            unreachable!("a validated handle must resolve")
        }
    }

    // --- dirty state ------------------------------------------------------

    /// Whether the tree has any pending style work, checked at the page root.
    #[must_use]
    pub fn has_dirty(&self) -> bool {
        match self.page {
            Some(page) => self
                .document
                .get(page)
                .is_some_and(|widget| widget.is_style_dirty() || widget.has_dirty_descendants()),
            None => false,
        }
    }

    /// Clear every element's dirty bits (called after a restyle pass).
    pub fn clear_dirty(&mut self) {
        self.document.clear_dirty();
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::WidgetTree;

    #[test]
    #[should_panic(expected = "every live node must have exactly one registry entry")]
    fn debug_check_catches_reclamation_behind_a_strong_handle() {
        let mut tree = WidgetTree::new();
        let handle = tree.create_view();

        // Deliberately bypass WidgetTree's collector and lie to the generic
        // Document collector. The next debug invariant check must report the
        // broken handle/node ownership relation immediately.
        let removed = tree.document.collect_detached(&[], |_| false);
        assert_eq!(removed.len(), 1);
        assert!(!tree.document.contains(handle.id));
        tree.assert_lifecycle_invariants();
    }

    #[test]
    #[should_panic(expected = "exactly once in its parent's child list")]
    fn debug_check_catches_unvalidated_topology_mutation() {
        let mut tree = WidgetTree::new();
        let page = tree.create_page();
        let child = tree.create_view();
        tree.append_element(&child, &page).unwrap();

        // Deliberately bypass PAPI validation and insert the child twice.
        tree.document.attach_at(page.id, child.id, 1);
        tree.assert_lifecycle_invariants();
    }

    #[test]
    #[should_panic(expected = "tree topology contains a parent cycle")]
    fn debug_check_catches_an_internally_created_parent_cycle() {
        let mut tree = WidgetTree::new();
        let page = tree.create_page();
        let child = tree.create_view();
        tree.append_element(&child, &page).unwrap();

        // The public PAPI rejects this as WouldCycle. Deliberately bypass it
        // so the debug checker proves a pointer-consistent cycle is still
        // diagnosed rather than leaking forever or hanging an ancestor walk.
        tree.document.attach_at(child.id, page.id, 0);
        tree.assert_lifecycle_invariants();
    }
}
