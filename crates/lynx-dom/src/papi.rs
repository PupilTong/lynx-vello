//! The [`Document`] — owner of the arena and the Element-PAPI surface.
//!
//! Methods here are shaped after Lynx's JS Element PAPI (renamed to `snake_case`
//! Rust). JS bindings live in a later runtime crate; this is the pure native
//! tree + mutation layer. There is deliberately **no** flush/resolution driver:
//! the `lynx-style` crate drives restyling and reads the dirty state this layer
//! maintains (see [`Document::has_dirty`] / [`Document::clear_dirty`]).

use rustc_hash::FxHashMap;
use stylo::context::QuirksMode;
use stylo::properties::declaration_block::{parse_one_declaration_into, parse_style_attribute};
use stylo::properties::{
    ComputedValues, Importance, PropertyDeclarationBlock, PropertyId, SourcePropertyDeclaration,
};
use stylo::servo_arc::Arc;
use stylo::shared_lock::SharedRwLock;
use stylo::stylesheets::{CssRuleType, Origin, UrlExtraData};
use stylo_atoms::Atom;
use stylo_traits::ParsingMode;
use thiserror::Error;

use crate::arena::{Arena, ElemRef, ElementId};
use crate::node::{EventKind, EventReg, Node};
use crate::state::PseudoState;
use crate::tag::NodeKind;

/// An error from a tree-mutating [`Document`] operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Error)]
pub enum DomError {
    /// A handle did not resolve to a live element.
    #[error("element {0:?} is stale or does not exist")]
    StaleElement(ElementId),
    /// A `remove`/`replace` target was not a child of the given parent.
    #[error("element {child:?} is not a child of {parent:?}")]
    NotAChild {
        /// The claimed parent.
        parent: ElementId,
        /// The element that was not actually its child.
        child: ElementId,
    },
    /// Performing the insertion would make an element its own ancestor.
    #[error("linking {ancestor:?} under {descendant:?} would create a cycle")]
    WouldCycle {
        /// The element being inserted (would become an ancestor of itself).
        ancestor: ElementId,
        /// The intended parent (a descendant of `ancestor`).
        descendant: ElementId,
    },
    /// An `insert_element_before` reference node was not a child of the parent.
    #[error("insertion reference {0:?} is not a child of the parent")]
    BadInsertReference(ElementId),
}

/// The document: a generational [`Arena`] plus the current page root and a
/// `unique_id` → [`ElementId`] index.
#[derive(Debug)]
pub struct Document {
    arena: Arena,
    page: Option<ElementId>,
    by_unique_id: FxHashMap<i32, ElementId>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// Create an empty document with a freshly minted [`SharedRwLock`].
    ///
    /// Suitable for DOM-only use; to style the tree, build it with
    /// [`Document::with_lock`] using the `StyleEngine`'s shared lock so the
    /// cascade's guards match this document's inline style blocks.
    #[must_use]
    pub fn new() -> Self {
        Self::from_arena(Arena::new())
    }

    /// Create an empty document backed by an explicit [`SharedRwLock`] and
    /// [`UrlExtraData`] (typically the `StyleEngine`'s, so inline styles parse
    /// against the same lock the cascade guards).
    #[must_use]
    pub fn with_lock(lock: SharedRwLock, url_data: UrlExtraData) -> Self {
        Self::from_arena(Arena::with_lock(lock, url_data))
    }

    fn from_arena(arena: Arena) -> Self {
        Self {
            arena,
            page: None,
            by_unique_id: FxHashMap::default(),
        }
    }

    /// Borrow the underlying arena.
    #[must_use]
    pub const fn arena(&self) -> &Arena {
        &self.arena
    }

    /// Mutably borrow the underlying arena.
    ///
    /// The `lynx-style` crate uses this to write resolved computed styles and
    /// clear dirty bits after a resolution pass.
    pub const fn arena_mut(&mut self) -> &mut Arena {
        &mut self.arena
    }

    /// The document's [`SharedRwLock`] (guarding inline style blocks).
    #[must_use]
    pub fn shared_lock(&self) -> &SharedRwLock {
        self.arena.shared_lock()
    }

    /// The document's base [`UrlExtraData`] (used to parse inline styles).
    #[must_use]
    pub fn url_data(&self) -> &UrlExtraData {
        self.arena.url_data()
    }

    // --- element creation -------------------------------------------------

    fn create(&mut self, kind: NodeKind, tag: &str) -> ElementId {
        let id = self.arena.insert(Node::new(kind, tag));
        let uid = self
            .arena
            .get(id)
            .expect("freshly inserted element is live")
            .unique_id;
        self.by_unique_id.insert(uid, id);
        id
    }

    /// Create the `<page>` root element and record it as the document's page.
    pub fn create_page(&mut self) -> ElementId {
        let id = self.create(NodeKind::Page, "page");
        self.page = Some(id);
        // The root always needs an initial style pass.
        self.arena.mark_style_dirty(id);
        id
    }

    /// Create a `<view>` element.
    pub fn create_view(&mut self) -> ElementId {
        self.create(NodeKind::View, "view")
    }

    /// Create a `<text>` element.
    pub fn create_text(&mut self) -> ElementId {
        self.create(NodeKind::Text, "text")
    }

    /// Create a `<raw-text>` leaf carrying literal text content.
    pub fn create_raw_text(&mut self, text: impl Into<String>) -> ElementId {
        let id = self.create(NodeKind::RawText, "raw-text");
        if let Some(node) = self.arena.get_mut(id) {
            node.text = Some(text.into());
        }
        id
    }

    /// Create an `<image>` element.
    pub fn create_image(&mut self) -> ElementId {
        self.create(NodeKind::Image, "image")
    }

    /// Create a `<scroll-view>` element.
    pub fn create_scroll_view(&mut self) -> ElementId {
        self.create(NodeKind::ScrollView, "scroll-view")
    }

    /// Create a `<list>` element.
    pub fn create_list(&mut self) -> ElementId {
        self.create(NodeKind::List, "list")
    }

    /// Create a `<component>` boundary element.
    pub fn create_component(&mut self) -> ElementId {
        self.create(NodeKind::Component, "component")
    }

    /// Create a `<wrapper>` element.
    pub fn create_wrapper(&mut self) -> ElementId {
        self.create(NodeKind::Wrapper, "wrapper")
    }

    /// Create an element from an arbitrary Lynx tag name. The tag is classified
    /// via [`NodeKind::from_tag`].
    pub fn create_element(&mut self, tag: &str) -> ElementId {
        let kind = NodeKind::from_tag(tag);
        self.create(kind, tag)
    }

    // --- tree mutation ----------------------------------------------------

    /// Append `child` as the last child of `parent`.
    pub fn append_element(&mut self, child: ElementId, parent: ElementId) -> Result<(), DomError> {
        self.insert_element_before(child, parent, None)
    }

    /// Insert `child` into `parent` immediately before `before`, or append it
    /// when `before` is `None`.
    ///
    /// `child` is first detached from any current parent. Re-inserting within
    /// the same parent reorders it.
    pub fn insert_element_before(
        &mut self,
        child: ElementId,
        parent: ElementId,
        before: Option<ElementId>,
    ) -> Result<(), DomError> {
        if !self.arena.contains(child) {
            return Err(DomError::StaleElement(child));
        }
        if !self.arena.contains(parent) {
            return Err(DomError::StaleElement(parent));
        }
        if child == parent || self.is_ancestor(child, parent) {
            return Err(DomError::WouldCycle {
                ancestor: child,
                descendant: parent,
            });
        }
        if let Some(reference) = before {
            // A same-parent move (`before == child`) is resolved after detach;
            // any other reference must currently be a child of `parent`.
            if reference != child && !self.is_child_of(reference, parent) {
                return Err(DomError::BadInsertReference(reference));
            }
        }

        self.detach(child);

        let index = match before {
            None => self.children_len(parent),
            Some(reference) => self
                .child_position(parent, reference)
                .unwrap_or_else(|| self.children_len(parent)),
        };

        if let Some(parent_node) = self.arena.get_mut(parent) {
            parent_node.children.insert(index, child);
        }
        if let Some(child_node) = self.arena.get_mut(child) {
            child_node.parent = Some(parent);
        }
        // Coarse: a structural change re-dirties the parent's whole subtree.
        self.arena.mark_subtree_dirty(parent);
        Ok(())
    }

    /// Remove `child` from `parent`, dropping `child`'s entire subtree from the
    /// arena and the `unique_id` index.
    pub fn remove_element(&mut self, parent: ElementId, child: ElementId) -> Result<(), DomError> {
        let Some(child_node) = self.arena.get(child) else {
            return Err(DomError::StaleElement(child));
        };
        if child_node.parent != Some(parent) {
            return Err(DomError::NotAChild { parent, child });
        }

        if let Some(parent_node) = self.arena.get_mut(parent) {
            parent_node.children.retain(|&c| c != child);
        }
        self.drop_subtree(child);
        self.arena.mark_subtree_dirty(parent);
        Ok(())
    }

    /// Replace `old` with `new` in the tree, keeping `old`'s position. `new` is
    /// detached from any current parent first; `old`'s subtree is then dropped.
    pub fn replace_element(&mut self, new: ElementId, old: ElementId) -> Result<(), DomError> {
        if new == old {
            return Ok(());
        }
        let Some(old_node) = self.arena.get(old) else {
            return Err(DomError::StaleElement(old));
        };
        let Some(parent) = old_node.parent else {
            return Err(DomError::NotAChild {
                parent: old,
                child: old,
            });
        };
        self.insert_element_before(new, parent, Some(old))?;
        self.remove_element(parent, old)
    }

    /// The first child of `parent`, if any.
    #[must_use]
    pub fn first_element(&self, parent: ElementId) -> Option<ElementId> {
        self.arena.get(parent)?.children.first().copied()
    }

    /// The next sibling of `node`, if any.
    #[must_use]
    pub fn next_element(&self, node: ElementId) -> Option<ElementId> {
        let parent = self.arena.get(node)?.parent?;
        let siblings = &self.arena.get(parent)?.children;
        let pos = siblings.iter().position(|&c| c == node)?;
        siblings.get(pos + 1).copied()
    }

    /// The parent of `node`, if any.
    #[must_use]
    pub fn get_parent(&self, node: ElementId) -> Option<ElementId> {
        self.arena.get(node)?.parent
    }

    // --- styling / attributes ---------------------------------------------

    /// Replace an element's classes from a whitespace-separated list.
    pub fn set_classes(&mut self, id: ElementId, classes: &str) -> Result<(), DomError> {
        match self.arena.get_mut(id) {
            Some(node) => {
                node.classes = classes.split_whitespace().map(Atom::from).collect();
            }
            None => return Err(DomError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Add a single class (no-op if already present).
    pub fn add_class(&mut self, id: ElementId, class: &str) -> Result<(), DomError> {
        match self.arena.get_mut(id) {
            Some(node) => {
                let class = Atom::from(class);
                if !node.classes.contains(&class) {
                    node.classes.push(class);
                }
            }
            None => return Err(DomError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Replace an element's inline style, parsing the whole declaration block
    /// through stylo (Lynx's `__SetInlineStyles`). An empty string clears it.
    pub fn set_inline_styles(&mut self, id: ElementId, text: &str) -> Result<(), DomError> {
        if !self.arena.contains(id) {
            return Err(DomError::StaleElement(id));
        }
        let block = if text.is_empty() {
            None
        } else {
            let parsed = parse_style_attribute(
                text,
                self.arena.url_data(),
                None,
                QuirksMode::NoQuirks,
                CssRuleType::Style,
            );
            Some(Arc::new(self.arena.shared_lock().wrap(parsed)))
        };
        if let Some(node) = self.arena.get_mut(id) {
            node.inline_block = block;
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Parse a single `name: value` declaration through stylo and merge it into
    /// the element's inline style block (Lynx's `__AddInlineStyle`).
    ///
    /// Mirrors Paws' `update_inline_style`: only the one new declaration is
    /// parsed and folded into a clone of the existing block, avoiding a
    /// whole-attribute re-parse. An unparseable property/value is dropped.
    pub fn add_inline_style(
        &mut self,
        id: ElementId,
        name: &str,
        value: &str,
    ) -> Result<(), DomError> {
        if !self.arena.contains(id) {
            return Err(DomError::StaleElement(id));
        }

        let Ok(property_id) = PropertyId::parse_unchecked(name, None) else {
            // Unknown non-custom property: drop it (M2 has no debug logging yet).
            return Ok(());
        };

        let mut source = SourcePropertyDeclaration::default();
        if parse_one_declaration_into(
            &mut source,
            property_id,
            value,
            Origin::Author,
            self.arena.url_data(),
            None,
            ParsingMode::DEFAULT,
            QuirksMode::NoQuirks,
            CssRuleType::Style,
        )
        .is_err()
        {
            return Ok(());
        }

        let lock = self.arena.shared_lock();
        let mut block = match self
            .arena
            .get(id)
            .and_then(|node| node.inline_block.as_ref())
        {
            Some(existing) => {
                let guard = lock.read();
                existing.read_with(&guard).clone()
            }
            None => PropertyDeclarationBlock::new(),
        };
        block.extend(source.drain(), Importance::Normal);
        let wrapped = Arc::new(lock.wrap(block));

        if let Some(node) = self.arena.get_mut(id) {
            node.inline_block = Some(wrapped);
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Set a plain attribute.
    ///
    /// Note: unlike the DOM, a plain `"id"` attribute is stored as an ordinary
    /// attribute here — Lynx sets the id selector separately via
    /// [`Document::set_id`] (its `__SetID`).
    pub fn set_attribute(
        &mut self,
        id: ElementId,
        name: &str,
        value: &str,
    ) -> Result<(), DomError> {
        match self.arena.get_mut(id) {
            Some(node) => {
                node.attrs.insert(name.into(), value.to_owned());
            }
            None => return Err(DomError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Set an element's id selector value (Lynx's `__SetID`). An empty string
    /// clears it.
    pub fn set_id(&mut self, id: ElementId, id_selector: &str) -> Result<(), DomError> {
        match self.arena.get_mut(id) {
            Some(node) => {
                node.id_attr = if id_selector.is_empty() {
                    None
                } else {
                    Some(Atom::from(id_selector))
                };
            }
            None => return Err(DomError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Set the `css_id` (style scope) on a batch of elements.
    pub fn set_css_id(&mut self, ids: &[ElementId], css_id: i32) -> Result<(), DomError> {
        if let Some(&bad) = ids.iter().find(|&&id| !self.arena.contains(id)) {
            return Err(DomError::StaleElement(bad));
        }
        for &id in ids {
            if let Some(node) = self.arena.get_mut(id) {
                node.css_id = css_id;
            }
        }
        for &id in ids {
            self.arena.mark_attribute_changed(id);
        }
        Ok(())
    }

    /// Replace an element's `data-*` dataset.
    pub fn set_dataset<I, K, V>(&mut self, id: ElementId, entries: I) -> Result<(), DomError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<Box<str>>,
        V: Into<String>,
    {
        match self.arena.get_mut(id) {
            Some(node) => {
                node.dataset = entries
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect();
            }
            None => return Err(DomError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Add or overwrite a single `data-*` dataset entry.
    pub fn add_dataset(&mut self, id: ElementId, key: &str, value: &str) -> Result<(), DomError> {
        match self.arena.get_mut(id) {
            Some(node) => {
                node.dataset.insert(key.into(), value.to_owned());
            }
            None => return Err(DomError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    /// Register an event binding on an element. (Does not affect style, so no
    /// invalidation.)
    pub fn add_event(
        &mut self,
        id: ElementId,
        kind: EventKind,
        name: &str,
        handler: &str,
    ) -> Result<(), DomError> {
        match self.arena.get_mut(id) {
            Some(node) => node.events.push(EventReg {
                name: name.into(),
                kind,
                handler: handler.into(),
            }),
            None => return Err(DomError::StaleElement(id)),
        }
        Ok(())
    }

    /// Toggle one or more pseudo-class flags on an element.
    pub fn set_pseudo_state(
        &mut self,
        id: ElementId,
        state: PseudoState,
        on: bool,
    ) -> Result<(), DomError> {
        match self.arena.get_mut(id) {
            Some(node) => node.element_state.set(state.to_element_state(), on),
            None => return Err(DomError::StaleElement(id)),
        }
        self.arena.mark_attribute_changed(id);
        Ok(())
    }

    // --- getters ----------------------------------------------------------

    /// An element's Lynx tag name.
    #[must_use]
    pub fn get_tag(&self, id: ElementId) -> Option<&str> {
        self.arena.get(id).map(Node::tag_str)
    }

    /// An element's plain attribute map.
    #[must_use]
    pub fn get_attributes(&self, id: ElementId) -> Option<&FxHashMap<Box<str>, String>> {
        self.arena.get(id).map(|node| &node.attrs)
    }

    /// An element's Lynx `unique_id`.
    #[must_use]
    pub fn get_element_unique_id(&self, id: ElementId) -> Option<i32> {
        self.arena.get(id).map(|node| node.unique_id)
    }

    /// An element's active dynamic pseudo-classes, as a [`PseudoState`].
    #[must_use]
    pub fn pseudo_state(&self, id: ElementId) -> Option<PseudoState> {
        self.arena
            .get(id)
            .map(|node| PseudoState::from_element_state(node.element_state))
    }

    /// Resolve a Lynx `unique_id` back to its [`ElementId`].
    #[must_use]
    pub fn element_by_unique_id(&self, unique_id: i32) -> Option<ElementId> {
        self.by_unique_id.get(&unique_id).copied()
    }

    /// The document's `<page>` root, if one has been created.
    #[must_use]
    pub const fn get_page_element(&self) -> Option<ElementId> {
        self.page
    }

    /// Borrow an element's [`Node`], if live.
    #[must_use]
    pub fn node(&self, id: ElementId) -> Option<&Node> {
        self.arena.get(id)
    }

    /// A read-only navigation handle for an element, if live.
    #[must_use]
    pub fn elem(&self, id: ElementId) -> Option<ElemRef<'_>> {
        self.arena.elem_ref(id)
    }

    /// An element's resolved computed style, if it has been styled.
    #[must_use]
    pub fn computed(&self, id: ElementId) -> Option<&Arc<ComputedValues>> {
        self.arena.get(id).and_then(Node::computed)
    }

    /// Store an element's resolved computed style and clear its `style_dirty`
    /// bit. Called by the `lynx-style` crate after resolving an element.
    pub fn set_computed(
        &mut self,
        id: ElementId,
        style: Arc<ComputedValues>,
    ) -> Result<(), DomError> {
        match self.arena.get_mut(id) {
            Some(node) => {
                node.computed = Some(style);
                node.style_dirty = false;
            }
            None => return Err(DomError::StaleElement(id)),
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
                .is_some_and(|node| node.style_dirty || node.dirty_descendants),
            None => false,
        }
    }

    /// Clear every element's dirty bits (called after a restyle pass).
    pub fn clear_dirty(&mut self) {
        self.arena.clear_dirty();
    }

    // --- private tree helpers ---------------------------------------------

    /// Whether `ancestor` is a strict ancestor of `descendant`.
    fn is_ancestor(&self, ancestor: ElementId, descendant: ElementId) -> bool {
        let mut next = self.arena.get(descendant).and_then(|node| node.parent);
        while let Some(current) = next {
            if current == ancestor {
                return true;
            }
            next = self.arena.get(current).and_then(|node| node.parent);
        }
        false
    }

    fn child_position(&self, parent: ElementId, child: ElementId) -> Option<usize> {
        self.arena
            .get(parent)?
            .children
            .iter()
            .position(|&c| c == child)
    }

    fn children_len(&self, parent: ElementId) -> usize {
        self.arena.get(parent).map_or(0, |node| node.children.len())
    }

    fn is_child_of(&self, child: ElementId, parent: ElementId) -> bool {
        self.child_position(parent, child).is_some()
    }

    /// Detach `child` from its current parent, if any, marking the old parent's
    /// subtree dirty.
    fn detach(&mut self, child: ElementId) {
        let old_parent = match self.arena.get(child) {
            Some(node) => node.parent,
            None => return,
        };
        if let Some(parent) = old_parent {
            if let Some(parent_node) = self.arena.get_mut(parent) {
                parent_node.children.retain(|&c| c != child);
            }
            self.arena.mark_subtree_dirty(parent);
        }
        if let Some(child_node) = self.arena.get_mut(child) {
            child_node.parent = None;
        }
    }

    /// Remove `root` and all its descendants from the arena and the `unique_id`
    /// index.
    fn drop_subtree(&mut self, root: ElementId) {
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            if let Some(node) = self.arena.remove(current) {
                self.by_unique_id.remove(&node.unique_id);
                stack.extend_from_slice(&node.children);
            }
        }
    }
}
