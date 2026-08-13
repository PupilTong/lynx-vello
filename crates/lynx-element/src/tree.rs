//! The element tree and its Element PAPI operations.

use std::fmt;

use dom::{self, Document, FontBlob, NodeId, StylesheetOrigin};

use crate::arena::{ElementArena, LynxElement};
use crate::device::Viewport;
use crate::ua::{PageConfig, ua_stylesheet};
use crate::{
    ElementId, IMAGE_TAG, LIST_TAG, PAGE_TAG, RAW_TEXT_TAG, SCROLL_VIEW_TAG, TEXT_TAG, VIEW_TAG,
    WRAPPER_TAG,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PapiError {
    UnknownElement(ElementId),
    NotAChild { parent: ElementId, child: ElementId },
    WouldCycle { parent: ElementId, child: ElementId },
    CannotReparentPage,
    CannotRemovePage,
}

impl fmt::Display for PapiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownElement(raw) => {
                write!(formatter, "no element has the unique id {raw}")
            }
            Self::NotAChild { parent, child } => {
                write!(formatter, "element #{child} is not a child of #{parent}")
            }
            Self::WouldCycle { parent, child } => write!(
                formatter,
                "placing #{child} under #{parent} would form a cycle"
            ),
            Self::CannotReparentPage => {
                formatter.write_str("the page element cannot be given a parent")
            }
            Self::CannotRemovePage => formatter.write_str("the page element cannot be removed"),
        }
    }
}

impl std::error::Error for PapiError {}

/// A DOM document paired with Lynx runtime handles and page policy.
#[derive(Debug)]
pub struct ElementTree {
    document: Document<ElementId>,
    elements: ElementArena,
    page_created: bool,
    uncommitted: bool,
    page_component_id: String,
    config: PageConfig,
}

pub(crate) const PAGE_UNIQUE_ID: ElementId = 1;

impl ElementTree {
    /// Creates an element tree with its permanent page element and UA cascade.
    #[must_use]
    pub fn new(viewport: Viewport, config: PageConfig) -> Self {
        let mut document = Document::new(viewport.device(), PAGE_TAG, PAGE_UNIQUE_ID);
        document.add_stylesheet(&ua_stylesheet(config), StylesheetOrigin::UserAgent);
        let elements = ElementArena::new();
        assert_eq!(
            elements.reserve(),
            PAGE_UNIQUE_ID,
            "the first live arena slot is reserved for the page"
        );
        let mut tree = Self {
            document,
            elements,
            page_created: false,
            uncommitted: false,
            page_component_id: String::new(),
            config,
        };
        let page_node = tree.document.document_element().id();
        tree.elements.insert(PAGE_UNIQUE_ID, page_node, 0, 0);
        tree
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) const fn document(&self) -> &Document<ElementId> {
        &self.document
    }

    /// Rebuilds the retained scene when stale and reports whether it changed.
    pub fn render(&mut self) -> bool {
        self.document.render()
    }

    /// Returns whether the retained scene is stale.
    #[must_use]
    pub fn needs_render(&self) -> bool {
        self.document.needs_render()
    }

    /// Borrows the scene retained by the last render.
    #[must_use]
    pub fn scene(&self) -> std::cell::Ref<'_, dom::vello::Scene> {
        self.document.scene()
    }

    /// Mutably borrows decoded image resources.
    pub fn images_mut(&mut self) -> &mut dom::ImageStore {
        self.document.images_mut()
    }

    /// Routes a host input event and applies its resolved default action.
    pub fn handle_input(&mut self, event: dom::input::InputEvent) {
        self.document.handle_input(event);
    }

    /// Changes the CSS viewport size for the next flush.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.document.set_viewport(width, height);
    }

    /// Changes the number of device pixels per CSS pixel.
    /// Panics if the ratio is not finite and positive.
    pub fn set_device_pixel_ratio(&mut self, device_pixel_ratio: f32) {
        assert!(
            device_pixel_ratio.is_finite() && device_pixel_ratio > 0.0,
            "device pixel ratio must be finite and positive, got {device_pixel_ratio}"
        );
        self.document.set_device_pixel_ratio(device_pixel_ratio);
    }

    /// Registers shared font data and returns the number of added faces.
    pub fn register_fonts(&mut self, data: FontBlob) -> usize {
        self.document.register_fonts(data)
    }

    #[must_use]
    /// Returns the page configuration.
    pub const fn config(&self) -> PageConfig {
        self.config
    }

    /// Returns the page id after `create_page` has run.
    #[must_use]
    pub fn page(&self) -> Option<ElementId> {
        self.page_created.then_some(PAGE_UNIQUE_ID)
    }

    /// Returns the page component id recorded by `create_page`.
    #[cfg(test)]
    #[must_use]
    pub fn page_component_id(&self) -> &str {
        &self.page_component_id
    }

    #[must_use]
    pub(crate) fn node_id(&self, id: ElementId) -> Option<NodeId> {
        self.element(id).map(LynxElement::node_id)
    }

    /// Returns the live runtime element for an id.
    #[must_use]
    pub fn element(&self, id: ElementId) -> Option<&LynxElement> {
        self.elements.get(id)
    }

    /// Adds an author stylesheet.
    pub fn add_author_stylesheet(&mut self, css: &str) {
        self.document.add_stylesheet(css, StylesheetOrigin::Author);
    }

    /// Binds and returns the permanent page id, ignoring repeated calls.
    pub fn create_page(&mut self, component_id: &str, component_css_id: i32) -> ElementId {
        self.uncommitted = true;
        if !self.page_created {
            component_id.clone_into(&mut self.page_component_id);
            self.elements
                .get_mut(PAGE_UNIQUE_ID)
                .expect("the page arena entry is permanent")
                .set_component_css_id(component_css_id);
            self.page_created = true;
        }
        PAGE_UNIQUE_ID
    }

    /// Creates a detached element with a Lynx tag for a live parent component or sentinel zero.
    pub fn create_element(
        &mut self,
        tag: &str,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        self.validate_parent_component(parent_component_unique_id)?;
        self.uncommitted = true;
        Ok(self.insert(tag, parent_component_unique_id, 0))
    }

    /// Creates a detached `wrapper` for a live parent component or sentinel zero.
    pub fn create_wrapper_element(
        &mut self,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        self.create_element(WRAPPER_TAG, parent_component_unique_id)
    }

    /// Creates a detached `text` for a live parent component or sentinel zero.
    pub fn create_text(
        &mut self,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        self.create_element(TEXT_TAG, parent_component_unique_id)
    }

    /// Creates a detached `image` for a live parent component or sentinel zero.
    pub fn create_image(
        &mut self,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        self.create_element(IMAGE_TAG, parent_component_unique_id)
    }

    /// Creates a detached `view` for a live parent component or sentinel zero.
    pub fn create_view(
        &mut self,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        self.create_element(VIEW_TAG, parent_component_unique_id)
    }

    /// Creates a detached `scroll-view` for a live parent component or sentinel zero.
    pub fn create_scroll_view(
        &mut self,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        self.create_element(SCROLL_VIEW_TAG, parent_component_unique_id)
    }

    /// Creates a detached `raw-text` leaf whose `text` attribute carries its literal contents.
    pub fn create_raw_text(&mut self, text: &str) -> ElementId {
        self.uncommitted = true;
        let unique_id = self.insert(RAW_TEXT_TAG, 0, 0);
        let node = self
            .node_id(unique_id)
            .expect("a just-inserted raw-text element is live");
        self.document.set_attribute(node, "text", text);
        unique_id
    }

    /// Creates a detached `list` for a live parent component or sentinel zero.
    ///
    /// List callback storage and execution belong to the future list PAPI implementation; this
    /// operation establishes only the element identity and tag.
    pub fn create_list(
        &mut self,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        self.create_element(LIST_TAG, parent_component_unique_id)
    }

    /// Reparents `child` as the last child of `parent` and returns it.
    pub fn append_element(
        &mut self,
        parent: ElementId,
        child: ElementId,
    ) -> Result<ElementId, PapiError> {
        self.insert_element_before(parent, child, None)
    }

    /// Reparents `child` before `reference`, or appends it when the reference is absent.
    pub fn insert_element_before(
        &mut self,
        parent: ElementId,
        child: ElementId,
        reference: Option<ElementId>,
    ) -> Result<ElementId, PapiError> {
        let parent_node = self.require_node(parent)?;
        let child_node = self.require_node(child)?;
        let reference_node = reference.map(|id| self.require_node(id)).transpose()?;
        self.validate_insertion(parent, parent_node, child, child_node)?;
        if let (Some(reference), Some(reference_node)) = (reference, reference_node) {
            if self
                .document
                .get(reference_node)
                .and_then(dom::Node::parent_id)
                != Some(parent_node)
            {
                return Err(PapiError::NotAChild {
                    parent,
                    child: reference,
                });
            }
            if reference == child {
                return Ok(child);
            }
        }

        self.uncommitted = true;
        self.document
            .insert_before(parent_node, child_node, reference_node);
        Ok(child)
    }

    /// Detaches `child` from `parent` without retiring either element and returns the child.
    pub fn remove_element(
        &mut self,
        parent: ElementId,
        child: ElementId,
    ) -> Result<ElementId, PapiError> {
        let parent_node = self.require_node(parent)?;
        let child_node = self.require_node(child)?;
        if child == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotRemovePage);
        }
        if self.document.get(child_node).and_then(dom::Node::parent_id) != Some(parent_node) {
            return Err(PapiError::NotAChild { parent, child });
        }

        self.uncommitted = true;
        self.document.remove_element(child_node);
        Ok(child)
    }

    /// Replaces `old_element` in place with `new_element`, leaving the old element detached.
    ///
    /// Replacing a detached element or replacing an element with itself is a no-op, matching the
    /// Element PAPI's `ChildNode.replaceWith` behavior.
    pub fn replace_element(
        &mut self,
        new_element: ElementId,
        old_element: ElementId,
    ) -> Result<(), PapiError> {
        let new_node = self.require_node(new_element)?;
        let old_node = self.require_node(old_element)?;
        if new_element == old_element {
            return Ok(());
        }
        if old_element == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotRemovePage);
        }
        let Some(parent_node) = self.document.get(old_node).and_then(dom::Node::parent_id) else {
            return Ok(());
        };
        if new_element == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotReparentPage);
        }
        let parent = *self
            .document
            .get(parent_node)
            .expect("a live element's parent must be live")
            .payload();
        self.validate_insertion(parent, parent_node, new_element, new_node)?;

        self.uncommitted = true;
        self.document
            .insert_before(parent_node, new_node, Some(old_node));
        self.document.remove_element(old_node);
        Ok(())
    }

    /// Drops one element and permanently retires its id.
    ///
    /// Its direct children are detached but remain live, along with their descendants. Each of
    /// those elements is retired only when the JavaScript VM drops its own handle.
    pub fn drop_element(&mut self, id: ElementId) -> Result<(), PapiError> {
        if id == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotRemovePage);
        }
        let node = self.node_id(id).ok_or(PapiError::UnknownElement(id))?;
        self.uncommitted = true;
        let unique_id = self.document.drop_element(node);
        debug_assert_eq!(unique_id, id, "the DOM payload must match its arena id");
        let retired = self.elements.retire(unique_id);
        debug_assert!(
            retired.is_some(),
            "a removed DOM node must have a live Lynx element"
        );
        Ok(())
    }

    /// Commits pending mutations through style and layout.
    pub fn flush_element_tree(&mut self) {
        self.document.layout();
        self.uncommitted = false;
    }

    /// Returns whether a PAPI mutation is awaiting a flush.
    #[must_use]
    pub const fn has_uncommitted_mutations(&self) -> bool {
        self.uncommitted
    }

    fn validate_parent_component(
        &self,
        parent_component_unique_id: ElementId,
    ) -> Result<(), PapiError> {
        if parent_component_unique_id != 0 && self.node_id(parent_component_unique_id).is_none() {
            return Err(PapiError::UnknownElement(parent_component_unique_id));
        }
        Ok(())
    }

    fn require_node(&self, id: ElementId) -> Result<NodeId, PapiError> {
        self.node_id(id).ok_or(PapiError::UnknownElement(id))
    }

    fn validate_insertion(
        &self,
        parent: ElementId,
        parent_node: NodeId,
        child: ElementId,
        child_node: NodeId,
    ) -> Result<(), PapiError> {
        if child == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotReparentPage);
        }
        if parent == child || self.document.is_ancestor(child_node, parent_node) {
            return Err(PapiError::WouldCycle { parent, child });
        }
        Ok(())
    }

    fn insert(
        &mut self,
        tag: &str,
        parent_component_unique_id: ElementId,
        component_css_id: i32,
    ) -> ElementId {
        let unique_id = self.elements.reserve();
        let node = self.document.create_element(tag, unique_id);
        self.elements.insert(
            unique_id,
            node,
            parent_component_unique_id,
            component_css_id,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Document, ElementId, ElementTree, LynxElement, PapiError, dom};
    use crate::device::Viewport;
    use crate::ua::PageConfig;

    fn tree() -> ElementTree {
        ElementTree::new(Viewport::new(393.0, 727.0), PageConfig::default())
    }

    #[test]
    fn a_flush_lays_the_page_out_to_the_viewport() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        tree.flush_element_tree();
        let page_node = tree.node_id(page).expect("a live page");
        let layout = tree
            .document()
            .rounded_layout(page_node)
            .expect("the page is laid out after the flush");
        assert!((layout.size.width - 393.0).abs() < f32::EPSILON);
        assert!((layout.size.height - 727.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_window_embedder_can_update_the_device_pixel_ratio() {
        let mut tree = tree();
        tree.set_device_pixel_ratio(2.0);
        assert!((tree.document().device_pixel_ratio() - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    #[should_panic(expected = "device pixel ratio must be finite and positive")]
    fn a_non_positive_device_pixel_ratio_panics() {
        let mut tree = tree();
        tree.set_device_pixel_ratio(0.0);
    }

    #[test]
    fn unique_ids_start_at_one_and_do_not_repeat() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let first = tree.create_view(0).unwrap();
        let second = tree.create_view(0).unwrap();
        assert_eq!(page, 1);
        assert_eq!(first, 2);
        assert_eq!(second, 3);
    }

    #[test]
    fn releasing_an_element_leaves_a_permanent_empty_arena_slot() {
        let mut tree = tree();
        let first = tree.create_view(0).unwrap();
        let first_node = tree.node_id(first).unwrap();
        let first_unique_id = *tree.document().get(first_node).unwrap().payload();

        tree.drop_element(first).unwrap();
        assert!(tree.node_id(first).is_none());

        let second = tree.create_view(0).unwrap();
        let second_node = tree.node_id(second).unwrap();
        let second_unique_id = *tree.document().get(second_node).unwrap().payload();

        assert_eq!(tree.elements.len(), 4);
        assert_eq!(first_unique_id, 2);
        assert_eq!(second_unique_id, 3);
        assert_eq!(second, first + 1);
        assert!(tree.node_id(first).is_none());
    }

    #[test]
    fn releasing_an_element_retires_only_it_and_detaches_its_descendants() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let parent = tree.create_view(0).unwrap();
        let child = tree.create_view(0).unwrap();
        let grandchild = tree.create_view(0).unwrap();
        tree.append_element(page, parent).unwrap();
        tree.append_element(parent, child).unwrap();
        tree.append_element(child, grandchild).unwrap();

        tree.drop_element(parent).unwrap();
        assert!(tree.node_id(parent).is_none());
        let child_node = tree.node_id(child).expect("the child remains live");
        let grandchild_node = tree
            .node_id(grandchild)
            .expect("the grandchild remains live");
        assert_eq!(tree.document().get(child_node).unwrap().parent_id(), None);
        assert_eq!(
            tree.document().get(child_node).unwrap().child_ids(),
            &[grandchild_node],
            "the surviving descendant subtree keeps its internal links"
        );
        assert_eq!(
            tree.document().get(grandchild_node).unwrap().parent_id(),
            Some(child_node)
        );
        assert!(!tree.document().is_connected(child_node));
        assert!(!tree.document().is_connected(grandchild_node));
        assert_eq!(tree.elements.len(), 5);

        tree.append_element(page, child).unwrap();
        assert!(tree.document().is_connected(child_node));
        assert!(tree.document().is_connected(grandchild_node));

        let next = tree.create_view(0).unwrap();
        assert_eq!(next, 5);
    }

    #[test]
    fn create_page_is_idempotent() {
        let mut tree = tree();
        let first = tree.create_page("page", 0);
        let second = tree.create_page("other", 7);
        assert_eq!(first, second);
        assert_eq!(tree.page(), Some(first));
        assert_eq!(tree.page_component_id(), "page");
        assert_eq!(
            tree.element(first).map(LynxElement::component_css_id),
            Some(0)
        );
    }

    #[test]
    fn the_page_component_id_stays_out_of_the_dom() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        assert_eq!(tree.page_component_id(), "card");

        let node = tree.node_id(page).unwrap();
        assert_eq!(tree.document().get(node).unwrap().attributes().len(), 0);
    }

    #[test]
    fn zero_is_the_no_element_sentinel() {
        let mut tree = tree();
        assert!(tree.element(0).is_none());
        assert!(tree.create_view(0).is_ok());
    }

    #[test]
    fn create_view_rejects_an_unknown_parent_component() {
        let mut tree = tree();
        assert_eq!(
            tree.create_view(9).unwrap_err(),
            PapiError::UnknownElement(9)
        );
    }

    #[test]
    fn reactlynx_create_functions_use_lynx_tags_and_record_the_parent_component() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        let elements = [
            (
                tree.create_element("custom-widget", page).unwrap(),
                "custom-widget",
            ),
            (tree.create_wrapper_element(page).unwrap(), "wrapper"),
            (tree.create_text(page).unwrap(), "text"),
            (tree.create_image(page).unwrap(), "image"),
            (tree.create_view(page).unwrap(), "view"),
            (tree.create_scroll_view(page).unwrap(), "scroll-view"),
            (tree.create_list(page).unwrap(), "list"),
        ];

        for (id, expected_tag) in elements {
            let element = tree.element(id).expect("the created element is live");
            assert_eq!(element.parent_component_unique_id(), page);
            let node = tree.document().get(element.node_id()).unwrap();
            assert_eq!(node.tag_name(), Some(expected_tag));
        }
    }

    #[test]
    fn raw_text_stores_its_literal_text_and_uses_the_null_component_sentinel() {
        let mut tree = tree();
        let raw_text = tree.create_raw_text("Hello, Lynx");
        let element = tree.element(raw_text).expect("the raw text is live");
        assert_eq!(element.parent_component_unique_id(), 0);

        let node = tree.document().get(element.node_id()).unwrap();
        assert_eq!(node.tag_name(), Some("raw-text"));
        assert_eq!(
            node.attributes().find(|(name, _)| *name == "text"),
            Some(("text", "Hello, Lynx"))
        );
    }

    #[test]
    fn every_parent_component_create_function_rejects_an_unknown_component() {
        let mut tree = tree();
        for result in [
            tree.create_element("custom-widget", 9),
            tree.create_wrapper_element(9),
            tree.create_text(9),
            tree.create_image(9),
            tree.create_view(9),
            tree.create_scroll_view(9),
            tree.create_list(9),
        ] {
            assert_eq!(result.unwrap_err(), PapiError::UnknownElement(9));
        }
    }

    #[test]
    fn append_element_returns_the_child_and_links_it() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let view = tree.create_view(0).unwrap();
        assert_eq!(tree.append_element(page, view).unwrap(), view);

        let page_node = tree.node_id(page).unwrap();
        let view_node = tree.node_id(view).unwrap();
        assert_eq!(
            tree.document().get(page_node).unwrap().child_ids(),
            [view_node]
        );
    }

    #[test]
    fn append_element_reparents_rather_than_duplicating() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let first = tree.create_view(0).unwrap();
        let second = tree.create_view(0).unwrap();
        let moved = tree.create_view(0).unwrap();
        tree.append_element(page, first).unwrap();
        tree.append_element(page, second).unwrap();
        tree.append_element(first, moved).unwrap();
        tree.append_element(second, moved).unwrap();

        let first_node = tree.node_id(first).unwrap();
        let second_node = tree.node_id(second).unwrap();
        let moved_node = tree.node_id(moved).unwrap();
        assert!(
            tree.document()
                .get(first_node)
                .unwrap()
                .child_ids()
                .is_empty()
        );
        assert_eq!(
            tree.document().get(second_node).unwrap().child_ids(),
            [moved_node]
        );
    }

    #[test]
    fn append_element_rejects_unknown_handles() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let ghost: ElementId = 99;
        assert_eq!(
            tree.append_element(page, ghost).unwrap_err(),
            PapiError::UnknownElement(99)
        );
        assert_eq!(
            tree.append_element(ghost, page).unwrap_err(),
            PapiError::UnknownElement(99)
        );
    }

    #[test]
    fn append_element_rejects_cycles() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let outer = tree.create_view(0).unwrap();
        let inner = tree.create_view(0).unwrap();
        tree.append_element(page, outer).unwrap();
        tree.append_element(outer, inner).unwrap();

        assert_eq!(
            tree.append_element(inner, outer).unwrap_err(),
            PapiError::WouldCycle {
                parent: inner,
                child: outer,
            }
        );
        assert_eq!(
            tree.append_element(outer, outer).unwrap_err(),
            PapiError::WouldCycle {
                parent: outer,
                child: outer,
            }
        );
    }

    #[test]
    fn append_element_refuses_to_reparent_the_page() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let view = tree.create_view(0).unwrap();
        tree.append_element(page, view).unwrap();
        assert_eq!(
            tree.append_element(view, page).unwrap_err(),
            PapiError::CannotReparentPage
        );
    }

    #[test]
    fn tree_mutations_insert_remove_and_replace_without_retiring_handles() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let first = tree.create_view(0).unwrap();
        let second = tree.create_view(0).unwrap();
        let third = tree.create_view(0).unwrap();
        let replacement = tree.create_view(0).unwrap();
        let second_child = tree.create_view(0).unwrap();
        tree.append_element(second, second_child).unwrap();

        assert_eq!(
            tree.insert_element_before(page, first, None).unwrap(),
            first
        );
        assert_eq!(
            tree.insert_element_before(page, second, Some(first))
                .unwrap(),
            second
        );
        tree.append_element(page, third).unwrap();
        tree.insert_element_before(page, third, Some(second))
            .unwrap();

        let page_node = tree.node_id(page).unwrap();
        let first_node = tree.node_id(first).unwrap();
        let second_node = tree.node_id(second).unwrap();
        let third_node = tree.node_id(third).unwrap();
        let replacement_node = tree.node_id(replacement).unwrap();
        let second_child_node = tree.node_id(second_child).unwrap();
        assert_eq!(
            tree.document().get(page_node).unwrap().child_ids(),
            [third_node, second_node, first_node]
        );

        assert_eq!(tree.remove_element(page, second).unwrap(), second);
        assert_eq!(
            tree.document().get(page_node).unwrap().child_ids(),
            [third_node, first_node]
        );
        assert_eq!(tree.document().get(second_node).unwrap().parent_id(), None);
        assert!(
            tree.element(second).is_some(),
            "remove must not retire the handle"
        );
        assert_eq!(
            tree.document().get(second_child_node).unwrap().parent_id(),
            Some(second_node),
            "remove must preserve the detached subtree"
        );
        assert!(tree.element(second_child).is_some());

        tree.replace_element(replacement, first).unwrap();
        assert_eq!(
            tree.document().get(page_node).unwrap().child_ids(),
            [third_node, replacement_node]
        );
        assert_eq!(tree.document().get(first_node).unwrap().parent_id(), None);
        assert!(
            tree.element(first).is_some(),
            "replace must leave the old handle live but detached"
        );
    }

    #[test]
    fn insert_and_remove_require_the_reference_or_child_to_belong_to_the_parent() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let other_parent = tree.create_view(0).unwrap();
        let reference = tree.create_view(0).unwrap();
        let child = tree.create_view(0).unwrap();
        tree.append_element(page, other_parent).unwrap();
        tree.append_element(other_parent, reference).unwrap();

        assert_eq!(
            tree.insert_element_before(page, child, Some(reference))
                .unwrap_err(),
            PapiError::NotAChild {
                parent: page,
                child: reference,
            }
        );
        assert_eq!(
            tree.remove_element(page, reference).unwrap_err(),
            PapiError::NotAChild {
                parent: page,
                child: reference,
            }
        );
        assert_eq!(
            tree.insert_element_before(page, child, Some(99))
                .unwrap_err(),
            PapiError::UnknownElement(99)
        );
    }

    #[test]
    fn insert_and_replace_reject_cycles_and_page_reparenting() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let outer = tree.create_view(0).unwrap();
        let inner = tree.create_view(0).unwrap();
        tree.append_element(page, outer).unwrap();
        tree.append_element(outer, inner).unwrap();

        assert_eq!(
            tree.insert_element_before(inner, outer, None).unwrap_err(),
            PapiError::WouldCycle {
                parent: inner,
                child: outer,
            }
        );
        assert_eq!(
            tree.replace_element(outer, inner).unwrap_err(),
            PapiError::WouldCycle {
                parent: outer,
                child: outer,
            }
        );
        assert_eq!(
            tree.insert_element_before(outer, page, None).unwrap_err(),
            PapiError::CannotReparentPage
        );
        assert_eq!(
            tree.remove_element(outer, page).unwrap_err(),
            PapiError::CannotRemovePage
        );
        assert_eq!(
            tree.replace_element(inner, page).unwrap_err(),
            PapiError::CannotRemovePage
        );
        assert_eq!(
            tree.replace_element(page, inner).unwrap_err(),
            PapiError::CannotReparentPage
        );
    }

    #[test]
    fn self_insert_self_replace_and_detached_replace_are_no_ops() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let child = tree.create_view(0).unwrap();
        let detached = tree.create_view(0).unwrap();
        let unused_replacement = tree.create_view(0).unwrap();
        tree.append_element(page, child).unwrap();
        tree.flush_element_tree();

        assert_eq!(
            tree.insert_element_before(page, child, Some(child))
                .unwrap(),
            child
        );
        tree.replace_element(child, child).unwrap();
        tree.replace_element(unused_replacement, detached).unwrap();
        assert!(!tree.has_uncommitted_mutations());

        let page_node = tree.node_id(page).unwrap();
        let child_node = tree.node_id(child).unwrap();
        assert_eq!(
            tree.document().get(page_node).unwrap().child_ids(),
            [child_node]
        );
    }

    #[test]
    fn the_page_is_the_document_element_from_birth() {
        let mut tree = tree();
        let document_element = tree.document().document_element().id();
        let page = tree.create_page("page", 0);
        assert_eq!(tree.node_id(page), Some(document_element));

        tree.flush_element_tree();
        assert_eq!(tree.document().document_element().id(), document_element);
        tree.flush_element_tree();
        assert_eq!(tree.document().document_element().id(), document_element);
    }

    #[test]
    fn flushing_before_create_page_commits_the_permanent_page() {
        let mut tree = tree();
        assert!(tree.page().is_none());
        tree.flush_element_tree();
        assert!(tree.page().is_none());
        assert!(
            tree.document()
                .document_element()
                .computed_style()
                .is_some()
        );
    }

    #[test]
    fn the_ua_sheet_gives_every_element_lynx_defaults() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let view = tree.create_view(0).unwrap();
        tree.append_element(page, view).unwrap();
        tree.flush_element_tree();

        let view_node = tree.node_id(view).unwrap();
        let style = tree
            .document()
            .get(view_node)
            .unwrap()
            .computed_style()
            .unwrap();
        assert_eq!(
            style.clone_box_sizing(),
            dom::stylo::computed_values::box_sizing::T::BorderBox
        );
        assert_eq!(
            style.clone_display(),
            dom::stylo::values::computed::Display::Linear
        );
    }

    #[test]
    fn the_display_page_config_switch_reaches_computed_style() {
        let mut tree = ElementTree::new(
            Viewport::new(393.0, 727.0),
            PageConfig {
                default_display_linear: false,
                ..PageConfig::default()
            },
        );
        let page = tree.create_page("page", 0);
        let view = tree.create_view(0).unwrap();
        tree.append_element(page, view).unwrap();
        tree.flush_element_tree();

        let view_node = tree.node_id(view).unwrap();
        let style = tree
            .document()
            .get(view_node)
            .unwrap()
            .computed_style()
            .unwrap();
        assert_ne!(
            style.clone_display(),
            dom::stylo::values::computed::Display::Linear
        );
        assert_eq!(
            style.clone_box_sizing(),
            dom::stylo::computed_values::box_sizing::T::BorderBox
        );
    }

    #[test]
    fn the_overflow_page_config_switch_reaches_computed_style() {
        for (visible, expected) in [
            (true, dom::stylo::values::computed::Overflow::Visible),
            (false, dom::stylo::values::computed::Overflow::Hidden),
        ] {
            let mut tree = ElementTree::new(
                Viewport::new(393.0, 727.0),
                PageConfig {
                    default_overflow_visible: visible,
                    ..PageConfig::default()
                },
            );
            let page = tree.create_page("page", 0);
            let view = tree.create_view(0).unwrap();
            tree.append_element(page, view).unwrap();
            tree.flush_element_tree();

            let view_node = tree.node_id(view).unwrap();
            let style = tree
                .document()
                .get(view_node)
                .unwrap()
                .computed_style()
                .unwrap();
            assert_eq!(style.clone_overflow_x(), expected, "visible={visible}");
            assert_eq!(style.clone_overflow_y(), expected, "visible={visible}");
        }
    }

    #[test]
    fn a_wide_tree_flushes() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        for _ in 0..2000 {
            let view = tree.create_view(0).unwrap();
            tree.append_element(page, view).unwrap();
        }
        tree.flush_element_tree();
    }

    #[test]
    fn the_document_payload_is_the_element_id() {
        fn assert_document_type(_: &Document<ElementId>) {}

        let mut tree = tree();
        assert_document_type(tree.document());
        let page = tree.create_page("page", 17);
        let view = tree.create_view(page).unwrap();
        let node = tree.node_id(view).unwrap();
        let payload_unique_id = *tree.document().get(node).unwrap().payload();
        let element = tree.elements.get(payload_unique_id).unwrap();

        assert_eq!(element.unique_id(), view);
        assert_eq!(element.node_id(), node);
        assert_eq!(
            tree.element(view)
                .map(LynxElement::parent_component_unique_id),
            Some(page)
        );
        assert_eq!(
            tree.element(page).map(LynxElement::component_css_id),
            Some(17)
        );
        assert_eq!(payload_unique_id, view);
    }
}
