//! The native element tree behind the JavaScript `bobcat` object.
//!
//! This module is the native half of the Element PAPI split:
//!
//! ```text
//! main-thread script ──▶ element-papi.js (packages/bobcat-element) ──▶ bobcat.* ──▶ ElementTree ──▶ dom
//!                        PAPI surface, tags, unique ids, handles        this module
//! ```
//!
//! The JavaScript runtime owns the Lynx vocabulary: PAPI member names and
//! arities, tag names, unique-id allocation, and handle identity and
//! collection. This module owns what must stay native:
//! the [`dom::Document`], the `page` root policy, the UA cascade defaults,
//! structural validation (existence, cycles, membership — `dom`'s mutation
//! entry points assume validated input, so these checks cannot be delegated
//! to script), the uncommitted-batch flag the presenter gates on, and the
//! style + layout commit.
//!
//! # Recorded limits
//!
//! - **Unique ids are allocated by script and never recycled.** [`ElementId`] is `u32`; the
//!   id-to-node table only appends, and ids must arrive in ascending sequence
//!   ([`PapiError::NonSequentialId`] otherwise). Retiring an element leaves a permanent `None`
//!   tombstone, so no stale script identity can ever name a later element. `dom` may reuse its
//!   private `NodeId` slots.
//! - **The page is permanent.** [`Document`] creates the `page` root at construction with unique id
//!   1; it can never be removed, dropped, or reparented.
//! - **The UA sheet covers the three documented Lynx computed defaults** (`display: linear`,
//!   `box-sizing: border-box`, `overflow: hidden`) under their two page-config switches. Lynx's
//!   wider default set is not modelled.
//! - **No `rpx`/`ppx` view-unit policy yet.** The device is built from CSS pixels and a
//!   device-pixel ratio only.
//! - **There is no runtime tree-depth cap in this layer.** Hardening recursive walks belongs in
//!   `dom` and `hughie`.

use std::fmt;

use dom::{self, Document, FontBlob, NodeId, StylesheetOrigin};

/// The script-visible element identity: a never-recycled unique id.
pub type ElementId = u32;

pub(crate) const PAGE_TAG: &str = "page";

pub(crate) const PAGE_UNIQUE_ID: ElementId = 1;

/// Why a native tree operation was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PapiError {
    UnknownElement(ElementId),
    NotAChild { parent: ElementId, child: ElementId },
    WouldCycle { parent: ElementId, child: ElementId },
    CannotReparentPage,
    CannotRemovePage,
    NonSequentialId { id: ElementId, expected: ElementId },
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
            Self::NonSequentialId { id, expected } => write!(
                formatter,
                "a new element must take the next unique id {expected}, got {id}"
            ),
        }
    }
}

impl std::error::Error for PapiError {}

/// Page configuration that controls the Lynx UA cascade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageConfig {
    /// Whether elements default to `display: linear`.
    pub default_display_linear: bool,
    /// Whether elements default to visible overflow.
    pub default_overflow_visible: bool,
    /// Whether author CSS selector matching is enabled.
    pub enable_css_selector: bool,
}

impl Default for PageConfig {
    fn default() -> Self {
        Self {
            default_display_linear: true,
            default_overflow_visible: true,
            enable_css_selector: true,
        }
    }
}

/// The Lynx UA stylesheet: embedder cascade policy `dom` must not know.
#[must_use]
fn ua_stylesheet(config: PageConfig) -> String {
    let display = if config.default_display_linear {
        "display: linear;"
    } else {
        ""
    };
    let overflow = if config.default_overflow_visible {
        ""
    } else {
        "overflow: hidden;"
    };
    format!(
        "page, view {{ box-sizing: border-box; {display} {overflow} }}\n\
         page {{ width: 100%; height: 100%; }}\n"
    )
}

/// A viewport measured in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Viewport width in CSS pixels.
    pub width: f32,
    /// Viewport height in CSS pixels.
    pub height: f32,
    /// Physical pixels per CSS pixel.
    pub device_pixel_ratio: f32,
}

impl Viewport {
    /// Creates a viewport with a device-pixel ratio of 1.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            device_pixel_ratio: 1.0,
        }
    }

    #[must_use]
    /// Returns this viewport with a new device-pixel ratio.
    pub const fn with_device_pixel_ratio(mut self, device_pixel_ratio: f32) -> Self {
        self.device_pixel_ratio = device_pixel_ratio;
        self
    }

    fn device(self) -> dom::Device {
        dom::Device::new(self.width, self.height, self.device_pixel_ratio)
    }
}

/// A DOM document keyed by script-allocated unique ids, plus page policy.
#[derive(Debug)]
pub struct ElementTree {
    document: Document<ElementId>,
    /// Unique id to DOM node, indexed directly by id. Slot zero is the
    /// permanent null sentinel; retired ids leave permanent tombstones.
    nodes: Vec<Option<NodeId>>,
    page_created: bool,
    uncommitted: bool,
    config: PageConfig,
}

impl ElementTree {
    /// Creates an element tree with its permanent page element and UA cascade.
    #[must_use]
    pub fn new(viewport: Viewport, config: PageConfig) -> Self {
        let mut document = Document::new(viewport.device(), PAGE_TAG, PAGE_UNIQUE_ID);
        document.add_stylesheet(&ua_stylesheet(config), StylesheetOrigin::UserAgent);
        let page_node = document.document_element().id();
        Self {
            document,
            nodes: vec![None, Some(page_node)],
            page_created: false,
            uncommitted: false,
            config,
        }
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

    /// Returns whether an element with this unique id is live.
    #[must_use]
    pub fn is_live(&self, id: ElementId) -> bool {
        self.node_id(id).is_some()
    }

    fn next_unique_id(&self) -> ElementId {
        ElementId::try_from(self.nodes.len()).expect("the element table exhausted its u32 ids")
    }

    #[must_use]
    fn node_id(&self, id: ElementId) -> Option<NodeId> {
        let index = usize::try_from(id).ok()?;
        if index == 0 {
            return None;
        }
        *self.nodes.get(index)?
    }

    /// Adds an author stylesheet.
    pub fn add_author_stylesheet(&mut self, css: &str) {
        self.document.add_stylesheet(css, StylesheetOrigin::Author);
    }

    /// Marks the permanent page element live and the batch uncommitted;
    /// repeated calls are no-ops beyond the batch flag.
    pub fn create_page(&mut self) {
        self.uncommitted = true;
        self.page_created = true;
    }

    /// Creates a detached element carrying the script-allocated unique id.
    ///
    /// Ids must arrive in ascending sequence: the table only appends, which
    /// is what keeps retired ids permanently unaddressable.
    pub fn create_element(&mut self, id: ElementId, tag: &str) -> Result<(), PapiError> {
        let expected = self.next_unique_id();
        if id != expected {
            return Err(PapiError::NonSequentialId { id, expected });
        }
        self.uncommitted = true;
        let node = self.document.create_element(tag, id);
        self.nodes.push(Some(node));
        Ok(())
    }

    /// Sets one attribute on a live element.
    pub fn set_attribute(
        &mut self,
        id: ElementId,
        name: &str,
        value: &str,
    ) -> Result<(), PapiError> {
        let node = self.require_node(id)?;
        self.uncommitted = true;
        self.document.set_attribute(node, name, value);
        Ok(())
    }

    /// Reparents `child` before `reference`, or appends it when the reference
    /// is absent. Inserting an element before itself is a no-op.
    pub fn insert_before(
        &mut self,
        parent: ElementId,
        child: ElementId,
        reference: Option<ElementId>,
    ) -> Result<(), PapiError> {
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
                return Ok(());
            }
        }

        self.uncommitted = true;
        self.document
            .insert_before(parent_node, child_node, reference_node);
        Ok(())
    }

    /// Detaches `child` from `parent` without retiring either element.
    pub fn remove_element(&mut self, parent: ElementId, child: ElementId) -> Result<(), PapiError> {
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
        Ok(())
    }

    /// Replaces `old_element` in place with `new_element`, leaving the old
    /// element detached.
    ///
    /// Replacing a detached element or replacing an element with itself is a
    /// no-op, matching the Element PAPI's `ChildNode.replaceWith` behavior.
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
    /// Its direct children are detached but remain live, along with their
    /// descendants. Each of those elements is retired only when script drops
    /// it in turn — explicitly, or when its collected handle's drop is
    /// delivered.
    pub fn drop_element(&mut self, id: ElementId) -> Result<(), PapiError> {
        if id == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotRemovePage);
        }
        let node = self.require_node(id)?;
        self.uncommitted = true;
        let unique_id = self.document.drop_element(node);
        debug_assert_eq!(unique_id, id, "the DOM payload must match its table id");
        let index = usize::try_from(id).expect("a live element id indexed the table");
        self.nodes[index] = None;
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
}

#[cfg(test)]
mod tests {
    use super::{Document, ElementId, ElementTree, PapiError, Viewport, dom, ua_stylesheet};
    use crate::tree::PageConfig;

    fn tree() -> ElementTree {
        ElementTree::new(Viewport::new(393.0, 727.0), PageConfig::default())
    }

    /// Creates the next element as a `view`, asserting sequential allocation
    /// the way the JavaScript runtime drives this API.
    fn create_view(tree: &mut ElementTree, id: ElementId) -> ElementId {
        tree.create_element(id, "view").expect("sequential id");
        id
    }

    #[test]
    fn a_flush_lays_the_page_out_to_the_viewport() {
        let mut tree = tree();
        tree.create_page();
        tree.flush_element_tree();
        let page_node = tree.document().document_element().id();
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
    fn element_ids_are_sequential_and_start_after_the_page() {
        let mut tree = tree();
        assert_eq!(
            tree.create_element(3, "view").unwrap_err(),
            PapiError::NonSequentialId { id: 3, expected: 2 }
        );
        create_view(&mut tree, 2);
        create_view(&mut tree, 3);
        assert!(tree.is_live(2));
        assert!(tree.is_live(3));
    }

    #[test]
    fn dropping_an_element_leaves_a_permanent_tombstone() {
        let mut tree = tree();
        create_view(&mut tree, 2);
        tree.drop_element(2).unwrap();
        assert!(!tree.is_live(2));

        assert_eq!(
            tree.create_element(2, "view").unwrap_err(),
            PapiError::NonSequentialId { id: 2, expected: 3 }
        );
        create_view(&mut tree, 3);
        assert!(!tree.is_live(2));
        assert!(tree.is_live(3));
    }

    #[test]
    fn dropping_an_element_retires_only_it_and_detaches_its_descendants() {
        let mut tree = tree();
        tree.create_page();
        let page = 1;
        let parent = create_view(&mut tree, 2);
        let child = create_view(&mut tree, 3);
        let grandchild = create_view(&mut tree, 4);
        tree.insert_before(page, parent, None).unwrap();
        tree.insert_before(parent, child, None).unwrap();
        tree.insert_before(child, grandchild, None).unwrap();

        tree.drop_element(parent).unwrap();
        assert!(!tree.is_live(parent));
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

        tree.insert_before(page, child, None).unwrap();
        assert!(tree.document().is_connected(child_node));
        assert!(tree.document().is_connected(grandchild_node));
    }

    #[test]
    fn create_page_is_idempotent() {
        let mut tree = tree();
        assert!(tree.page().is_none());
        tree.create_page();
        assert_eq!(tree.page(), Some(1));
        tree.create_page();
        assert_eq!(tree.page(), Some(1));
    }

    #[test]
    fn zero_is_the_no_element_sentinel() {
        let tree = tree();
        assert!(!tree.is_live(0));
        assert!(tree.is_live(1));
    }

    #[test]
    fn insert_before_appends_and_links_the_child() {
        let mut tree = tree();
        tree.create_page();
        let view = create_view(&mut tree, 2);
        tree.insert_before(1, view, None).unwrap();

        let page_node = tree.node_id(1).unwrap();
        let view_node = tree.node_id(view).unwrap();
        assert_eq!(
            tree.document().get(page_node).unwrap().child_ids(),
            [view_node]
        );
    }

    #[test]
    fn insert_before_reparents_rather_than_duplicating() {
        let mut tree = tree();
        tree.create_page();
        let first = create_view(&mut tree, 2);
        let second = create_view(&mut tree, 3);
        let moved = create_view(&mut tree, 4);
        tree.insert_before(1, first, None).unwrap();
        tree.insert_before(1, second, None).unwrap();
        tree.insert_before(first, moved, None).unwrap();
        tree.insert_before(second, moved, None).unwrap();

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
    fn insert_before_rejects_unknown_ids() {
        let mut tree = tree();
        tree.create_page();
        assert_eq!(
            tree.insert_before(1, 99, None).unwrap_err(),
            PapiError::UnknownElement(99)
        );
        assert_eq!(
            tree.insert_before(99, 1, None).unwrap_err(),
            PapiError::UnknownElement(99)
        );
    }

    #[test]
    fn insert_before_rejects_cycles() {
        let mut tree = tree();
        tree.create_page();
        let outer = create_view(&mut tree, 2);
        let inner = create_view(&mut tree, 3);
        tree.insert_before(1, outer, None).unwrap();
        tree.insert_before(outer, inner, None).unwrap();

        assert_eq!(
            tree.insert_before(inner, outer, None).unwrap_err(),
            PapiError::WouldCycle {
                parent: inner,
                child: outer,
            }
        );
        assert_eq!(
            tree.insert_before(outer, outer, None).unwrap_err(),
            PapiError::WouldCycle {
                parent: outer,
                child: outer,
            }
        );
    }

    #[test]
    fn insert_before_refuses_to_reparent_the_page() {
        let mut tree = tree();
        tree.create_page();
        let view = create_view(&mut tree, 2);
        tree.insert_before(1, view, None).unwrap();
        assert_eq!(
            tree.insert_before(view, 1, None).unwrap_err(),
            PapiError::CannotReparentPage
        );
    }

    #[test]
    fn tree_mutations_insert_remove_and_replace_without_retiring_ids() {
        let mut tree = tree();
        tree.create_page();
        let first = create_view(&mut tree, 2);
        let second = create_view(&mut tree, 3);
        let third = create_view(&mut tree, 4);
        let replacement = create_view(&mut tree, 5);
        let second_child = create_view(&mut tree, 6);
        tree.insert_before(second, second_child, None).unwrap();

        tree.insert_before(1, first, None).unwrap();
        tree.insert_before(1, second, Some(first)).unwrap();
        tree.insert_before(1, third, None).unwrap();
        tree.insert_before(1, third, Some(second)).unwrap();

        let page_node = tree.node_id(1).unwrap();
        let first_node = tree.node_id(first).unwrap();
        let second_node = tree.node_id(second).unwrap();
        let third_node = tree.node_id(third).unwrap();
        let replacement_node = tree.node_id(replacement).unwrap();
        let second_child_node = tree.node_id(second_child).unwrap();
        assert_eq!(
            tree.document().get(page_node).unwrap().child_ids(),
            [third_node, second_node, first_node]
        );

        tree.remove_element(1, second).unwrap();
        assert_eq!(
            tree.document().get(page_node).unwrap().child_ids(),
            [third_node, first_node]
        );
        assert_eq!(tree.document().get(second_node).unwrap().parent_id(), None);
        assert!(tree.is_live(second), "remove must not retire the id");
        assert_eq!(
            tree.document().get(second_child_node).unwrap().parent_id(),
            Some(second_node),
            "remove must preserve the detached subtree"
        );
        assert!(tree.is_live(second_child));

        tree.replace_element(replacement, first).unwrap();
        assert_eq!(
            tree.document().get(page_node).unwrap().child_ids(),
            [third_node, replacement_node]
        );
        assert_eq!(tree.document().get(first_node).unwrap().parent_id(), None);
        assert!(
            tree.is_live(first),
            "replace must leave the old id live but detached"
        );
    }

    #[test]
    fn insert_and_remove_require_the_reference_or_child_to_belong_to_the_parent() {
        let mut tree = tree();
        tree.create_page();
        let other_parent = create_view(&mut tree, 2);
        let reference = create_view(&mut tree, 3);
        let child = create_view(&mut tree, 4);
        tree.insert_before(1, other_parent, None).unwrap();
        tree.insert_before(other_parent, reference, None).unwrap();

        assert_eq!(
            tree.insert_before(1, child, Some(reference)).unwrap_err(),
            PapiError::NotAChild {
                parent: 1,
                child: reference,
            }
        );
        assert_eq!(
            tree.remove_element(1, reference).unwrap_err(),
            PapiError::NotAChild {
                parent: 1,
                child: reference,
            }
        );
        assert_eq!(
            tree.insert_before(1, child, Some(99)).unwrap_err(),
            PapiError::UnknownElement(99)
        );
    }

    #[test]
    fn insert_and_replace_reject_cycles_and_page_reparenting() {
        let mut tree = tree();
        tree.create_page();
        let outer = create_view(&mut tree, 2);
        let inner = create_view(&mut tree, 3);
        tree.insert_before(1, outer, None).unwrap();
        tree.insert_before(outer, inner, None).unwrap();

        assert_eq!(
            tree.insert_before(inner, outer, None).unwrap_err(),
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
            tree.insert_before(outer, 1, None).unwrap_err(),
            PapiError::CannotReparentPage
        );
        assert_eq!(
            tree.remove_element(outer, 1).unwrap_err(),
            PapiError::CannotRemovePage
        );
        assert_eq!(
            tree.replace_element(inner, 1).unwrap_err(),
            PapiError::CannotRemovePage
        );
        assert_eq!(
            tree.replace_element(1, inner).unwrap_err(),
            PapiError::CannotReparentPage
        );
    }

    #[test]
    fn self_insert_self_replace_and_detached_replace_are_no_ops() {
        let mut tree = tree();
        tree.create_page();
        let child = create_view(&mut tree, 2);
        let detached = create_view(&mut tree, 3);
        let unused_replacement = create_view(&mut tree, 4);
        tree.insert_before(1, child, None).unwrap();
        tree.flush_element_tree();

        tree.insert_before(1, child, Some(child)).unwrap();
        tree.replace_element(child, child).unwrap();
        tree.replace_element(unused_replacement, detached).unwrap();
        assert!(!tree.has_uncommitted_mutations());

        let page_node = tree.node_id(1).unwrap();
        let child_node = tree.node_id(child).unwrap();
        assert_eq!(
            tree.document().get(page_node).unwrap().child_ids(),
            [child_node]
        );
    }

    #[test]
    fn dropping_the_page_is_rejected_before_liveness() {
        let mut tree = tree();
        assert_eq!(
            tree.drop_element(1).unwrap_err(),
            PapiError::CannotRemovePage
        );
        assert_eq!(
            tree.drop_element(99).unwrap_err(),
            PapiError::UnknownElement(99)
        );
    }

    #[test]
    fn set_attribute_reaches_the_dom_and_requires_liveness() {
        let mut tree = tree();
        let raw_text = create_view(&mut tree, 2);
        tree.set_attribute(raw_text, "text", "Hello, Lynx").unwrap();
        let node = tree.node_id(raw_text).unwrap();
        assert_eq!(
            tree.document()
                .get(node)
                .unwrap()
                .attributes()
                .find(|(name, _)| *name == "text"),
            Some(("text", "Hello, Lynx"))
        );
        assert_eq!(
            tree.set_attribute(99, "text", "x").unwrap_err(),
            PapiError::UnknownElement(99)
        );
    }

    #[test]
    fn the_page_is_the_document_element_from_birth() {
        let mut tree = tree();
        let document_element = tree.document().document_element().id();
        tree.create_page();
        assert_eq!(tree.node_id(1), Some(document_element));

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
        tree.create_page();
        let view = create_view(&mut tree, 2);
        tree.insert_before(1, view, None).unwrap();
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
        tree.create_page();
        let view = create_view(&mut tree, 2);
        tree.insert_before(1, view, None).unwrap();
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
            tree.create_page();
            let view = create_view(&mut tree, 2);
            tree.insert_before(1, view, None).unwrap();
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
    fn default_config_is_linear_and_overflow_visible() {
        let config = PageConfig::default();
        assert!(config.default_display_linear);
        assert!(config.default_overflow_visible);

        let sheet = ua_stylesheet(config);
        assert!(sheet.contains("display: linear;"));
        assert!(!sheet.contains("overflow: hidden;"));
        assert!(sheet.contains("box-sizing: border-box;"));
    }

    #[test]
    fn ua_switches_drop_the_declarations_they_gate() {
        let sheet = ua_stylesheet(PageConfig {
            default_display_linear: false,
            default_overflow_visible: false,
            enable_css_selector: true,
        });
        assert!(!sheet.contains("display: linear;"));
        assert!(sheet.contains("overflow: hidden;"));
    }

    #[test]
    fn a_wide_tree_flushes() {
        let mut tree = tree();
        tree.create_page();
        for id in 2..2002 {
            create_view(&mut tree, id);
            tree.insert_before(1, id, None).unwrap();
        }
        tree.flush_element_tree();
    }

    #[test]
    fn the_document_payload_is_the_element_id() {
        fn assert_document_type(_: &Document<ElementId>) {}

        let mut tree = tree();
        assert_document_type(tree.document());
        tree.create_page();
        let view = create_view(&mut tree, 2);
        let node = tree.node_id(view).unwrap();
        assert_eq!(*tree.document().get(node).unwrap().payload(), view);
    }
}
