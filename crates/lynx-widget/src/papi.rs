//! The [`WidgetTree`] — owner of the document and the Element-PAPI surface.

use std::sync::{Arc as StdArc, Mutex, Weak};

use rustc_hash::FxHashMap;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;
use thiserror::Error;
use w3c_dom::{Document, ElementState, Node, NodeId};

use crate::Widget;
use crate::handle::{HandleInner, Reaper, WidgetHandle};
use crate::kind::WidgetKind;
use crate::state::{EventBinding, EventBindingKind, WidgetState};
use crate::style::{ViewMetrics, build_device};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Error)]
pub enum WidgetError {
    #[error("widget {0:?} belongs to a different context")]
    ForeignWidget(NodeId),
    #[error("widget {0:?} is stale or does not exist")]
    StaleWidget(NodeId),
    #[error("widget {child:?} is not a child of {parent:?}")]
    NotAChild { parent: NodeId, child: NodeId },
    #[error("linking {ancestor:?} under {descendant:?} would create a cycle")]
    WouldCycle {
        ancestor: NodeId,
        descendant: NodeId,
    },
    #[error("insertion reference {0:?} is not a child of the parent")]
    InvalidSiblingReference(NodeId),
    #[error("the page root {0:?} cannot be reparented")]
    CannotReparentRoot(NodeId),
}

/// The widget tree: one [`Document`] of [`Widget`]s plus the Lynx `unique_id`
/// counter/index, its own `<page>` root, and the canonical [`WidgetHandle`]
/// registry. `w3c-dom::Document` remains the DOM document node; this layer
/// decides which of its elements is Lynx's root.
#[derive(Debug)]
pub struct WidgetTree {
    doc: Document<WidgetState>,
    page: Option<NodeId>,
    next_unique_id: i32,
    by_unique_id: FxHashMap<i32, NodeId>,
    handles: Mutex<FxHashMap<NodeId, Weak<HandleInner>>>,
    reaper: StdArc<Reaper>,
}

/// The live-handle status and node membership of a detached subtree.
struct SubtreeRetention {
    has_live_handle: bool,
    node_ids: Vec<NodeId>,
}

impl WidgetTree {
    #[must_use]
    pub fn new(metrics: ViewMetrics) -> Self {
        Self::from_document(Document::new(build_device(metrics)))
    }

    pub(crate) fn from_document(doc: Document<WidgetState>) -> Self {
        Self {
            doc,
            page: None,
            next_unique_id: 1,
            by_unique_id: FxHashMap::default(),
            handles: Mutex::new(FxHashMap::default()),
            reaper: Reaper::new(),
        }
    }

    #[must_use]
    pub const fn document(&self) -> &Document<WidgetState> {
        &self.doc
    }

    pub(crate) const fn document_mut(&mut self) -> &mut Document<WidgetState> {
        &mut self.doc
    }

    fn resolve_handle(&self, handle: &WidgetHandle) -> Result<NodeId, WidgetError> {
        let id = handle.id();
        if !handle.belongs_to(&self.reaper) {
            return Err(WidgetError::ForeignWidget(id));
        }
        if !self.doc.contains_node(id) {
            debug_assert!(
                false,
                "a live same-tree WidgetHandle must retain its node (registry bug)"
            );
            return Err(WidgetError::StaleWidget(id));
        }
        Ok(id)
    }

    fn canonical_handle(&self, id: NodeId) -> WidgetHandle {
        debug_assert!(
            self.doc.get(id).is_some_and(Node::is_element),
            "WidgetHandle can only identify a live element"
        );
        let mut registry = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = registry.get(&id).and_then(Weak::upgrade) {
            return WidgetHandle { inner: existing };
        }
        let inner = StdArc::new(HandleInner::new(id, &self.reaper));
        registry.insert(id, StdArc::downgrade(&inner));
        WidgetHandle { inner }
    }

    pub(crate) fn reclaim_detached_subtrees(&mut self) {
        let Some(dropped) = self.reaper.take_dropped() else {
            return;
        };
        for id in dropped {
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
            if !self.doc.contains_node(id) {
                continue;
            }
            if self.doc.is_connected(id) {
                continue;
            }
            let mut top = id;
            while let Some(parent) = self.doc.get(top).and_then(Node::parent_id) {
                top = parent;
            }
            let retention = self.subtree_retention(top);
            if retention.has_live_handle {
                continue;
            }
            for state in self.doc.remove_subtree(top) {
                self.by_unique_id.remove(&state.unique_id);
            }
            let mut registry = self
                .handles
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for freed in retention.node_ids {
                registry.remove(&freed);
            }
        }
    }

    fn subtree_retention(&self, root: NodeId) -> SubtreeRetention {
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
        SubtreeRetention {
            has_live_handle: retained,
            node_ids: ids,
        }
    }

    fn create_widget(&mut self, kind: WidgetKind, tag: &str) -> WidgetHandle {
        self.reclaim_detached_subtrees();
        let unique_id = self.next_unique_id;
        self.next_unique_id = self.next_unique_id.wrapping_add(1);
        let id = self
            .doc
            .create_element(tag, WidgetState::new(kind, unique_id));
        self.doc.set_attribute(id, "l-css-id", "0");
        self.by_unique_id.insert(unique_id, id);
        self.canonical_handle(id)
    }

    pub fn create_page(&mut self) -> WidgetHandle {
        assert!(self.page.is_none(), "WidgetTree already has a <page> root");
        let handle = self.create_widget(WidgetKind::Page, "page");
        self.doc.append_document_element(handle.id());
        self.page = Some(handle.id());
        handle
    }

    pub fn create_view(&mut self) -> WidgetHandle {
        self.create_widget(WidgetKind::View, "view")
    }

    pub fn create_text(&mut self) -> WidgetHandle {
        self.create_widget(WidgetKind::Text, "text")
    }

    pub fn create_raw_text(&mut self, text: impl Into<String>) -> WidgetHandle {
        let handle = self.create_widget(WidgetKind::RawText, "raw-text");
        self.doc
            .set_element_text_content(handle.id(), Some(text.into()));
        handle
    }

    pub fn create_image(&mut self) -> WidgetHandle {
        self.create_widget(WidgetKind::Image, "image")
    }

    pub fn create_scroll_view(&mut self) -> WidgetHandle {
        self.create_widget(WidgetKind::ScrollView, "scroll-view")
    }

    pub fn create_list(&mut self) -> WidgetHandle {
        self.create_widget(WidgetKind::List, "list")
    }

    pub fn create_wrapper(&mut self) -> WidgetHandle {
        self.create_widget(WidgetKind::Wrapper, "wrapper")
    }

    pub fn create_element(&mut self, tag: &str) -> WidgetHandle {
        let kind = WidgetKind::from_tag_name(tag);
        self.create_widget(kind, tag)
    }

    pub fn append_child(
        &mut self,
        parent: &WidgetHandle,
        child: &WidgetHandle,
    ) -> Result<(), WidgetError> {
        self.insert_before(parent, child, None)
    }

    pub fn insert_before(
        &mut self,
        parent: &WidgetHandle,
        child: &WidgetHandle,
        before: Option<&WidgetHandle>,
    ) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let child_id = self.resolve_handle(child)?;
        let parent_id = self.resolve_handle(parent)?;
        if self.page == Some(child_id) {
            return Err(WidgetError::CannotReparentRoot(child_id));
        }
        if child_id == parent_id || self.doc.is_ancestor(child_id, parent_id) {
            return Err(WidgetError::WouldCycle {
                ancestor: child_id,
                descendant: parent_id,
            });
        }
        let before_id = before
            .map(|handle| self.resolve_handle(handle))
            .transpose()?;
        if let Some(reference) = before_id {
            if reference == child_id {
                return if self.doc.child_position(parent_id, child_id).is_some() {
                    Ok(())
                } else {
                    Err(WidgetError::InvalidSiblingReference(reference))
                };
            }
            if self.doc.child_position(parent_id, reference).is_none() {
                return Err(WidgetError::InvalidSiblingReference(reference));
            }
        }

        self.doc.insert_before(parent_id, child_id, before_id);
        Ok(())
    }

    pub fn remove_child(
        &mut self,
        parent: &WidgetHandle,
        child: &WidgetHandle,
    ) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let parent_id = self.resolve_handle(parent)?;
        let child_id = self.resolve_handle(child)?;
        if self.doc.get(child_id).and_then(Node::parent_id) != Some(parent_id) {
            return Err(WidgetError::NotAChild {
                parent: parent_id,
                child: child_id,
            });
        }

        self.doc.detach(child_id);
        Ok(())
    }

    pub fn replace_with(
        &mut self,
        old: &WidgetHandle,
        replacement: &WidgetHandle,
    ) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let old_id = self.resolve_handle(old)?;
        let replacement_id = self.resolve_handle(replacement)?;
        if replacement_id == old_id {
            return Ok(());
        }
        let Some(parent_id) = self.doc.get(old_id).and_then(Node::parent_id) else {
            return Ok(());
        };
        if self.page == Some(replacement_id) {
            return Err(WidgetError::CannotReparentRoot(replacement_id));
        }
        if replacement_id == parent_id || self.doc.is_ancestor(replacement_id, parent_id) {
            return Err(WidgetError::WouldCycle {
                ancestor: replacement_id,
                descendant: parent_id,
            });
        }
        self.doc
            .insert_before(parent_id, replacement_id, Some(old_id));
        self.doc.detach(old_id);
        Ok(())
    }

    pub fn first_child(&self, parent: &WidgetHandle) -> Result<Option<WidgetHandle>, WidgetError> {
        let parent_id = self.resolve_handle(parent)?;
        Ok(self
            .doc
            .get(parent_id)
            .and_then(|node| node.child_ids().first().copied())
            .map(|id| self.canonical_handle(id)))
    }

    pub fn next_sibling(&self, widget: &WidgetHandle) -> Result<Option<WidgetHandle>, WidgetError> {
        let id = self.resolve_handle(widget)?;
        Ok(self
            .doc
            .get(id)
            .and_then(Node::next_sibling)
            .map(|node| self.canonical_handle(node.id())))
    }

    pub fn parent(&self, widget: &WidgetHandle) -> Result<Option<WidgetHandle>, WidgetError> {
        let id = self.resolve_handle(widget)?;
        Ok(self
            .doc
            .get(id)
            .and_then(Node::parent_id)
            .filter(|&parent| self.doc.get(parent).is_some_and(Node::is_element))
            .map(|parent| self.canonical_handle(parent)))
    }

    pub fn set_classes(&mut self, handle: &WidgetHandle, classes: &str) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let id = self.resolve_handle(handle)?;
        self.doc.set_classes(id, classes);
        Ok(())
    }

    pub fn add_class(&mut self, handle: &WidgetHandle, class: &str) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let id = self.resolve_handle(handle)?;
        self.doc.add_class(id, class);
        Ok(())
    }

    pub fn set_inline_styles(
        &mut self,
        handle: &WidgetHandle,
        text: &str,
    ) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let id = self.resolve_handle(handle)?;
        self.doc.set_inline_style(id, text);
        Ok(())
    }

    pub fn add_inline_style(
        &mut self,
        handle: &WidgetHandle,
        name: &str,
        value: &str,
    ) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let id = self.resolve_handle(handle)?;
        self.doc.add_inline_style(id, name, value);
        Ok(())
    }

    pub fn set_attribute(
        &mut self,
        handle: &WidgetHandle,
        name: &str,
        value: &str,
    ) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let id = self.resolve_handle(handle)?;
        self.doc.set_attribute(id, name, value);
        Ok(())
    }

    pub fn set_id_attribute(
        &mut self,
        handle: &WidgetHandle,
        id_attribute: &str,
    ) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let id = self.resolve_handle(handle)?;
        self.doc
            .set_id_attribute(id, (!id_attribute.is_empty()).then_some(id_attribute));
        Ok(())
    }

    pub fn set_css_id(
        &mut self,
        handles: &[&WidgetHandle],
        css_id: i32,
    ) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let ids = handles
            .iter()
            .map(|handle| self.resolve_handle(handle))
            .collect::<Result<Vec<_>, _>>()?;
        let css_id = css_id.to_string();
        for id in ids {
            self.doc.set_attribute(id, "l-css-id", &css_id);
        }
        Ok(())
    }

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
        self.reclaim_detached_subtrees();
        let id = self.resolve_handle(handle)?;
        let old_names: Vec<Box<str>> = self
            .doc
            .get(id)
            .map(|widget| {
                widget
                    .attributes()
                    .filter(|(name, _)| name.starts_with("data-"))
                    .map(|(name, _)| Box::<str>::from(name))
                    .collect()
            })
            .unwrap_or_default();
        for name in old_names {
            self.doc.remove_attribute(id, &name);
        }
        for (key, value) in entries {
            let key: Box<str> = key.into();
            let value: String = value.into();
            self.doc.set_attribute(id, &format!("data-{key}"), &value);
        }
        Ok(())
    }

    pub fn set_dataset_entry(
        &mut self,
        handle: &WidgetHandle,
        key: &str,
        value: &str,
    ) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let id = self.resolve_handle(handle)?;
        self.doc.set_attribute(id, &format!("data-{key}"), value);
        Ok(())
    }

    pub fn add_event_binding(
        &mut self,
        handle: &WidgetHandle,
        kind: EventBindingKind,
        name: &str,
        event_handler: &str,
    ) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let id = self.resolve_handle(handle)?;
        let state = self
            .doc
            .get(id)
            .ok_or(WidgetError::StaleWidget(id))?
            .payload();
        state.push_event(EventBinding {
            name: name.into(),
            kind,
            handler: event_handler.into(),
        });
        Ok(())
    }

    pub fn enable_pseudo_state(
        &mut self,
        handle: &WidgetHandle,
        state: ElementState,
    ) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let id = self.resolve_handle(handle)?;
        self.doc.add_element_state(id, state);
        Ok(())
    }

    pub fn disable_pseudo_state(
        &mut self,
        handle: &WidgetHandle,
        state: ElementState,
    ) -> Result<(), WidgetError> {
        self.reclaim_detached_subtrees();
        let id = self.resolve_handle(handle)?;
        self.doc.remove_element_state(id, state);
        Ok(())
    }

    pub fn tag_name(&self, handle: &WidgetHandle) -> Result<&str, WidgetError> {
        let id = self.resolve_handle(handle)?;
        let widget = self.doc.get(id).ok_or(WidgetError::StaleWidget(id))?;
        debug_assert!(
            widget.is_element(),
            "WidgetTree handles always identify element nodes"
        );
        widget.tag_name().ok_or(WidgetError::StaleWidget(id))
    }

    pub fn attributes<'a>(
        &'a self,
        handle: &WidgetHandle,
    ) -> Result<impl ExactSizeIterator<Item = (&'a str, &'a str)> + 'a, WidgetError> {
        let id = self.resolve_handle(handle)?;
        let widget = self.doc.get(id).ok_or(WidgetError::StaleWidget(id))?;
        Ok(widget.attributes())
    }

    pub fn unique_id(&self, handle: &WidgetHandle) -> Result<i32, WidgetError> {
        let id = self.resolve_handle(handle)?;
        self.doc
            .get(id)
            .map(|widget| widget.payload().unique_id)
            .ok_or(WidgetError::StaleWidget(id))
    }

    pub fn pseudo_state(&self, handle: &WidgetHandle) -> Result<ElementState, WidgetError> {
        let id = self.resolve_handle(handle)?;
        self.doc
            .get(id)
            .map(Widget::element_state)
            .ok_or(WidgetError::StaleWidget(id))
    }

    #[must_use]
    pub fn widget_by_unique_id(&self, unique_id: i32) -> Option<WidgetHandle> {
        let id = *self.by_unique_id.get(&unique_id)?;
        self.doc
            .contains_node(id)
            .then(|| self.canonical_handle(id))
    }

    #[must_use]
    pub fn page_root(&self) -> Option<WidgetHandle> {
        self.page.map(|id| self.canonical_handle(id))
    }

    pub fn widget(&self, handle: &WidgetHandle) -> Result<&Widget, WidgetError> {
        let id = self.resolve_handle(handle)?;
        self.doc.get(id).ok_or(WidgetError::StaleWidget(id))
    }

    pub fn computed_style(
        &self,
        handle: &WidgetHandle,
    ) -> Result<Option<Arc<ComputedValues>>, WidgetError> {
        let id = self.resolve_handle(handle)?;
        Ok(self.doc.get(id).and_then(Widget::computed_style))
    }

    pub fn reclaim_detached_widgets(&mut self) {
        self.reclaim_detached_subtrees();
    }
}
