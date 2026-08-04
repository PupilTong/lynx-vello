//! The element tree and its Element PAPI operations.

use std::fmt;

use dom::{self, Document, NodeId, StylesheetOrigin};

use crate::arena::{ElementArena, LynxElement};
use crate::device::Viewport;
use crate::ua::{PageConfig, ua_stylesheet};
use crate::{ElementId, PAGE_TAG, VIEW_TAG};

/// Why an Element PAPI call was rejected.
///
/// The main-thread script is untrusted input: `docs/style-architecture.md`
/// requires this layer to validate handles before calling the crash-on-misuse
/// DOM core, so every fallible PAPI entry point returns this instead of
/// panicking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PapiError {
    /// A handle that never named a live element.
    UnknownElement(ElementId),
    /// Appending `child` under `parent` would put a node inside its own
    /// subtree.
    WouldCycle { parent: ElementId, child: ElementId },
    /// The page element cannot be given a parent — it is the document element
    /// by construction, and `__FlushElementTree` is what attaches it.
    CannotReparentPage,
}

impl fmt::Display for PapiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownElement(raw) => {
                write!(formatter, "no element has the unique id {raw}")
            }
            Self::WouldCycle { parent, child } => write!(
                formatter,
                "appending #{child} under #{parent} would form a cycle"
            ),
            Self::CannotReparentPage => {
                formatter.write_str("the page element cannot be given a parent")
            }
        }
    }
}

impl std::error::Error for PapiError {}

/// One Lynx element tree: a `dom` document, an independent runtime-element
/// arena, and the page policy the Element PAPI speaks in.
#[derive(Debug)]
pub struct ElementTree {
    /// The DOM payload is only the key back into `elements`; all Lynx runtime
    /// state stays in the context-owned arena.
    document: Document<ElementId>,
    elements: ElementArena,
    page: Option<ElementId>,
    /// `__CreatePage`'s `componentID` — a string name web-core keeps in a side
    /// table rather than on the element, so it never reaches selectors.
    page_component_id: String,
    page_attached: bool,
    config: PageConfig,
}

impl ElementTree {
    /// Creates an empty tree for `viewport` with `config`'s UA cascade
    /// installed. No page exists until `__CreatePage`.
    #[must_use]
    pub fn new(viewport: Viewport, config: PageConfig) -> Self {
        let mut document = Document::new(viewport.device());
        document.add_stylesheet(&ua_stylesheet(config), StylesheetOrigin::UserAgent);
        Self {
            document,
            elements: ElementArena::new(),
            page: None,
            page_component_id: String::new(),
            page_attached: false,
            config,
        }
    }

    /// The underlying document for trusted workspace composition and tests.
    #[cfg(any(test, feature = "internal-document-access"))]
    #[doc(hidden)]
    #[must_use]
    pub const fn document(&self) -> &Document<ElementId> {
        &self.document
    }

    /// Mutable document access for trusted workspace composition.
    #[cfg(feature = "internal-document-access")]
    #[doc(hidden)]
    pub fn document_mut(&mut self) -> &mut Document<ElementId> {
        &mut self.document
    }

    /// Feeds one host input event in, building the private visual frame needed
    /// for hit testing and performing the resolved UA default action — today,
    /// scrolling an `overflow: scroll` box.
    ///
    /// This belongs on the narrow mutable surface for the same reason
    /// [`Self::set_viewport`] does: input cannot desynchronise the handle
    /// table. All it can reach is scroll offsets and per-pointer gesture
    /// state — no element is created, moved, or retired — so lending it out
    /// costs none of the tree invariants this layer protects.
    ///
    /// Dispatching the returned target through Lynx's own event model
    /// (`bindEvent`/`catchEvent` phases, the gesture arena, `hit-slop`) is the
    /// runtime layer's job, not this one's; it prevents the default action and
    /// takes over when it wants different behavior.
    pub fn handle_input(&mut self, event: dom::input::InputEvent) -> dom::input::InputResponse {
        self.document.handle_input(event)
    }

    /// Resizes the viewport, restyling and relaying out on the next flush.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.document.set_viewport(width, height);
    }

    /// Changes the number of device pixels per CSS pixel.
    ///
    /// Window embedders call this when a view moves between displays with
    /// different scale factors. Keeping it on this narrow surface avoids
    /// exposing the mutable [`Document`] and its tree-mutation methods.
    ///
    /// # Panics
    ///
    /// Panics on a non-finite or non-positive ratio: nothing downstream
    /// validates it, and a stored `0.0` or `NaN` scale silently corrupts
    /// every later cascade, layout, and paint.
    pub fn set_device_pixel_ratio(&mut self, device_pixel_ratio: f32) {
        assert!(
            device_pixel_ratio.is_finite() && device_pixel_ratio > 0.0,
            "device pixel ratio must be finite and positive, got {device_pixel_ratio}"
        );
        self.document.set_device_pixel_ratio(device_pixel_ratio);
    }

    /// Registers font data for text measurement, returning how many faces were
    /// added.
    pub fn register_fonts(&mut self, bytes: &[u8]) -> usize {
        self.document.register_fonts(bytes)
    }

    #[must_use]
    pub const fn config(&self) -> PageConfig {
        self.config
    }

    /// The page element, once `__CreatePage` has run.
    #[must_use]
    pub const fn page(&self) -> Option<ElementId> {
        self.page
    }

    /// The `componentID` the page was created with; empty before
    /// `__CreatePage`.
    #[cfg(test)]
    #[must_use]
    pub fn page_component_id(&self) -> &str {
        &self.page_component_id
    }

    /// Whether the page has been attached to the document — i.e. whether
    /// `__FlushElementTree` has committed at least once.
    #[cfg(test)]
    #[must_use]
    pub const fn is_flushed(&self) -> bool {
        self.page_attached
    }

    /// The DOM node a handle names, or `None` if the handle is not live.
    #[must_use]
    pub fn node_id(&self, id: ElementId) -> Option<NodeId> {
        self.element(id).map(LynxElement::node_id)
    }

    /// The live runtime element stored at `id`.
    #[must_use]
    pub fn element(&self, id: ElementId) -> Option<&LynxElement> {
        self.elements.get(id)
    }

    /// Mounts author CSS — the seam a decoded `.web.bundle` `StyleInfo`
    /// section will lower into once that lowering exists.
    pub fn add_author_stylesheet(&mut self, css: &str) {
        self.document.add_stylesheet(css, StylesheetOrigin::Author);
    }

    /// `__CreatePage(componentID, componentCSSID)`.
    ///
    /// Idempotent, like web-core's: a second call returns the page that
    /// already exists and ignores its arguments. The page is created detached;
    /// [`Self::flush_element_tree`] is what puts it in the document.
    pub fn create_page(&mut self, component_id: &str, component_css_id: i32) -> ElementId {
        if let Some(page) = self.page {
            return page;
        }
        let id = self.insert(PAGE_TAG, 0, component_css_id);
        // `componentID` is a *string* name, not the numeric unique id, and
        // web-core keeps it out of the DOM — `create_element_common` files it
        // in a side table. Recording it here rather than as an attribute keeps
        // it invisible to selector matching; the DOM payload remains only the
        // context-owned unique id.
        component_id.clone_into(&mut self.page_component_id);
        self.page = Some(id);
        id
    }

    /// `__CreateView(parentComponentUniqueID)`.
    ///
    /// Creates a detached `view` element. `parent_component_unique_id` is `0`
    /// for "no parent component"; any other value must name a live element.
    pub fn create_view(
        &mut self,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        if parent_component_unique_id != 0 && self.node_id(parent_component_unique_id).is_none() {
            return Err(PapiError::UnknownElement(parent_component_unique_id));
        }
        Ok(self.insert(VIEW_TAG, parent_component_unique_id, 0))
    }

    /// `__AppendElement(parent, child)`.
    ///
    /// Appends `child` as `parent`'s last child, detaching it from any current
    /// parent first, and returns it. web-core's TypeScript declares the return
    /// `void`, but both real implementations — the CSR `parent.appendChild`
    /// and the native `FiberAppendElement` — return the child, so that is the
    /// behavior mirrored here.
    pub fn append_element(
        &mut self,
        parent: ElementId,
        child: ElementId,
    ) -> Result<ElementId, PapiError> {
        let parent_node = self
            .node_id(parent)
            .ok_or(PapiError::UnknownElement(parent))?;
        let child_node = self
            .node_id(child)
            .ok_or(PapiError::UnknownElement(child))?;
        if self.page == Some(child) {
            return Err(PapiError::CannotReparentPage);
        }
        if parent == child || self.document.is_ancestor(child_node, parent_node) {
            return Err(PapiError::WouldCycle { parent, child });
        }

        self.document.append_child(parent_node, child_node);
        Ok(child)
    }

    /// `__DropElement(id)`.
    ///
    /// The DOM subtree and every corresponding `LynxElement` are dropped
    /// together. Their `Vec` entries remain as permanent `None` tombstones, so
    /// no later creation can reuse any of their unique ids.
    pub fn drop_element(&mut self, id: ElementId) -> bool {
        let Some(node) = self.node_id(id) else {
            return false;
        };
        let retired_ids = self.document.remove_subtree(node);
        for unique_id in retired_ids {
            let retired = self.elements.retire(unique_id);
            debug_assert!(
                retired.is_some(),
                "a removed DOM node must have a live Lynx element"
            );
        }

        if self.page == Some(id) {
            self.page = None;
            self.page_component_id.clear();
            self.page_attached = false;
        }
        true
    }

    /// `__FlushElementTree()` — the single commit boundary.
    ///
    /// web-core withholds exactly one thing until the first flush: the page
    /// root is not in the rendered document until then. We do the same, and
    /// then run the style + layout pass that makes every pending mutation
    /// paint-eligible. Returns `false` when there is no page to commit.
    pub fn flush_element_tree(&mut self) -> bool {
        let Some(page) = self.page else {
            return false;
        };
        let Some(page_node) = self.node_id(page) else {
            return false;
        };
        if !self.page_attached {
            self.document.append_document_element(page_node);
            self.page_attached = true;
        }
        self.document.layout();
        true
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
    fn a_window_embedder_can_update_the_device_pixel_ratio() {
        let mut tree = tree();
        tree.set_device_pixel_ratio(2.0);
        assert!((tree.document().device().device_pixel_ratio().get() - 2.0).abs() < f32::EPSILON);
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

        assert!(tree.drop_element(first));
        assert!(tree.node_id(first).is_none());

        let second = tree.create_view(0).unwrap();
        let second_node = tree.node_id(second).unwrap();
        let second_unique_id = *tree.document().get(second_node).unwrap().payload();

        assert_eq!(tree.elements.len(), 3);
        assert_eq!(first_unique_id, 1);
        assert_eq!(second_unique_id, 2);
        assert_eq!(second, first + 1);
        assert!(tree.node_id(first).is_none());
    }

    #[test]
    fn releasing_a_subtree_retires_every_lynx_element_in_it() {
        let mut tree = tree();
        let parent = tree.create_view(0).unwrap();
        let child = tree.create_view(0).unwrap();
        tree.append_element(parent, child).unwrap();

        assert!(tree.drop_element(parent));
        assert!(tree.node_id(parent).is_none());
        assert!(tree.node_id(child).is_none());
        assert_eq!(tree.elements.len(), 3);

        let next = tree.create_view(0).unwrap();
        assert_eq!(next, 3);
    }

    #[test]
    fn create_page_is_idempotent() {
        let mut tree = tree();
        let first = tree.create_page("page", 0);
        let second = tree.create_page("other", 7);
        assert_eq!(first, second);
        assert_eq!(tree.page(), Some(first));
        // The second call's arguments are ignored, like web-core's.
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

        // It must not be selector-visible: the DOM core derives matching only
        // from real DOM state, and an invented attribute would let author CSS
        // from a bundle see something web-core never exposes.
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
    fn the_page_joins_the_document_only_on_the_first_flush() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        assert!(!tree.is_flushed());
        assert!(tree.document().root_element().is_none());

        assert!(tree.flush_element_tree());
        assert!(tree.is_flushed());
        let page_node = tree.node_id(page).unwrap();
        assert_eq!(
            tree.document().root_element().map(dom::Node::id),
            Some(page_node)
        );

        // A second flush is a plain re-commit, not a second attach.
        assert!(tree.flush_element_tree());
        assert_eq!(
            tree.document().root_element().map(dom::Node::id),
            Some(page_node)
        );
    }

    #[test]
    fn flushing_without_a_page_is_a_no_op() {
        let mut tree = tree();
        assert!(!tree.flush_element_tree());
        assert!(!tree.is_flushed());
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
        // The switch is off, so nothing overrides the CSS initial value and
        // the element is no longer a Lynx linear box.
        assert_ne!(
            style.clone_display(),
            dom::stylo::values::computed::Display::Linear
        );
        // `box-sizing` is not gated by this switch and still applies.
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

    /// A wide tree can be built and flushed without special-case bookkeeping.
    #[test]
    fn a_wide_tree_flushes() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        for _ in 0..2000 {
            let view = tree.create_view(0).unwrap();
            tree.append_element(page, view).unwrap();
        }
        assert!(tree.flush_element_tree());
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
