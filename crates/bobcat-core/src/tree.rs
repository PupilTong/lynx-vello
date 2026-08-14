//! The native element tree behind the JavaScript `bobcat` object.
//!
//! ```text
//! main-thread script ──▶ element-papi.js ──▶ bobcat.* ──▶ ElementTree ──▶ dom
//! ```
//!
//! The Element PAPI runtime (`packages/bobcat-element`) owns the PAPI
//! surface, tag vocabulary, and handle lifecycle; this module owns the
//! [`dom::Document`], the `page` root policy, the UA cascade defaults, the
//! uncommitted-batch flag the presenter gates on, and the style + layout
//! commit. Element identity is the DOM [`NodeId`]; there is no separate id
//! space and no input validation — misuse panics in `dom`, and the host
//! boundary converts the unwind into a JavaScript exception.

use dom::{self, Document, FontBlob, NodeId, StylesheetOrigin};

pub(crate) const PAGE_TAG: &str = "page";

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

/// A DOM document plus Lynx page policy.
#[derive(Debug)]
pub struct ElementTree {
    document: Document<()>,
    page_created: bool,
    uncommitted: bool,
    config: PageConfig,
}

impl ElementTree {
    /// Creates an element tree with its permanent page element and UA cascade.
    #[must_use]
    pub fn new(viewport: Viewport, config: PageConfig) -> Self {
        let mut document = Document::new(viewport.device(), PAGE_TAG, ());
        document.add_stylesheet(&ua_stylesheet(config), StylesheetOrigin::UserAgent);
        Self {
            document,
            page_created: false,
            uncommitted: false,
            config,
        }
    }

    /// Borrows the document for observation.
    #[must_use]
    pub const fn document(&self) -> &Document<()> {
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

    /// Returns whether `create_page` has run.
    #[must_use]
    pub const fn page_created(&self) -> bool {
        self.page_created
    }

    /// Adds an author stylesheet.
    pub fn add_author_stylesheet(&mut self, css: &str) {
        self.document.add_stylesheet(css, StylesheetOrigin::Author);
    }

    /// Marks the permanent page live and the batch uncommitted, and returns
    /// the page node.
    pub fn create_page(&mut self) -> NodeId {
        self.uncommitted = true;
        self.page_created = true;
        self.document.document_element().id()
    }

    /// Creates a detached element and returns its node.
    pub fn create_element(&mut self, tag: &str) -> NodeId {
        self.uncommitted = true;
        self.document.create_element(tag, ())
    }

    /// Sets one attribute.
    pub fn set_attribute(&mut self, node: NodeId, name: &str, value: &str) {
        self.uncommitted = true;
        self.document.set_attribute(node, name, value);
    }

    /// Reparents `child` before `reference`, or appends it when the reference
    /// is absent. Inserting an element before itself is a no-op.
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) {
        if reference == Some(child) {
            return;
        }
        self.uncommitted = true;
        self.document.insert_before(parent, child, reference);
    }

    /// Detaches `child` from its parent; a no-op when already detached.
    pub fn remove_element(&mut self, child: NodeId) {
        self.uncommitted = true;
        self.document.remove_element(child);
    }

    /// Replaces `old_element` in place, leaving it detached but live.
    /// Replacing a detached element or an element with itself is a no-op,
    /// matching the Element PAPI's `ChildNode.replaceWith` behavior.
    pub fn replace_element(&mut self, new_element: NodeId, old_element: NodeId) {
        if new_element == old_element {
            return;
        }
        let Some(parent) = self
            .document
            .get(old_element)
            .and_then(dom::Node::parent_id)
        else {
            return;
        };
        self.uncommitted = true;
        self.document
            .insert_before(parent, new_element, Some(old_element));
        self.document.remove_element(old_element);
    }

    /// Frees one element, detaching its direct children.
    pub fn drop_element(&mut self, node: NodeId) {
        self.uncommitted = true;
        self.document.drop_element(node);
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
}

#[cfg(test)]
mod tests {
    use super::{ElementTree, PageConfig, Viewport, dom, ua_stylesheet};

    fn tree() -> ElementTree {
        ElementTree::new(Viewport::new(393.0, 727.0), PageConfig::default())
    }

    #[test]
    fn a_flush_lays_the_page_out_to_the_viewport() {
        let mut tree = tree();
        let page = tree.create_page();
        tree.flush_element_tree();
        let layout = tree
            .document()
            .rounded_layout(page)
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
    fn create_page_is_idempotent_and_returns_the_document_element() {
        let mut tree = tree();
        assert!(!tree.page_created());
        let first = tree.create_page();
        let second = tree.create_page();
        assert!(tree.page_created());
        assert_eq!(first, second);
        assert_eq!(first, tree.document().document_element().id());
    }

    #[test]
    fn dropping_an_element_frees_it_and_detaches_its_descendants() {
        let mut tree = tree();
        let page = tree.create_page();
        let parent = tree.create_element("view");
        let child = tree.create_element("view");
        let grandchild = tree.create_element("view");
        tree.insert_before(page, parent, None);
        tree.insert_before(parent, child, None);
        tree.insert_before(child, grandchild, None);

        tree.drop_element(parent);
        assert!(tree.document().get(parent).is_none());
        assert_eq!(tree.document().get(child).unwrap().parent_id(), None);
        assert_eq!(
            tree.document().get(child).unwrap().child_ids(),
            &[grandchild],
            "the surviving descendant subtree keeps its internal links"
        );
        assert!(!tree.document().is_connected(child));
        assert!(!tree.document().is_connected(grandchild));

        tree.insert_before(page, child, None);
        assert!(tree.document().is_connected(child));
        assert!(tree.document().is_connected(grandchild));
    }

    #[test]
    #[should_panic(expected = "cannot drop the permanent document element")]
    fn dropping_the_page_panics() {
        let mut tree = tree();
        let page = tree.create_page();
        tree.drop_element(page);
    }

    #[test]
    fn insert_remove_and_replace_link_and_detach_without_freeing() {
        let mut tree = tree();
        let page = tree.create_page();
        let first = tree.create_element("view");
        let second = tree.create_element("view");
        let replacement = tree.create_element("view");
        tree.insert_before(page, first, None);
        tree.insert_before(page, second, Some(first));

        assert_eq!(
            tree.document().get(page).unwrap().child_ids(),
            [second, first]
        );

        tree.remove_element(second);
        assert_eq!(tree.document().get(page).unwrap().child_ids(), [first]);
        assert!(tree.document().get(second).is_some());
        tree.remove_element(second);

        tree.replace_element(replacement, first);
        assert_eq!(
            tree.document().get(page).unwrap().child_ids(),
            [replacement]
        );
        assert!(tree.document().get(first).is_some());
        assert_eq!(tree.document().get(first).unwrap().parent_id(), None);
    }

    #[test]
    fn self_insert_self_replace_and_detached_replace_are_no_ops() {
        let mut tree = tree();
        let page = tree.create_page();
        let child = tree.create_element("view");
        let detached = tree.create_element("view");
        let unused_replacement = tree.create_element("view");
        tree.insert_before(page, child, None);
        tree.flush_element_tree();

        tree.insert_before(page, child, Some(child));
        tree.replace_element(child, child);
        tree.replace_element(unused_replacement, detached);
        assert!(!tree.has_uncommitted_mutations());
        assert_eq!(tree.document().get(page).unwrap().child_ids(), [child]);
    }

    #[test]
    fn set_attribute_reaches_the_dom() {
        let mut tree = tree();
        let raw_text = tree.create_element("raw-text");
        tree.set_attribute(raw_text, "text", "Hello, Lynx");
        assert_eq!(
            tree.document()
                .get(raw_text)
                .unwrap()
                .attributes()
                .find(|(name, _)| *name == "text"),
            Some(("text", "Hello, Lynx"))
        );
    }

    #[test]
    fn flushing_before_create_page_commits_the_permanent_page() {
        let mut tree = tree();
        assert!(!tree.page_created());
        tree.flush_element_tree();
        assert!(!tree.page_created());
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
        let page = tree.create_page();
        let view = tree.create_element("view");
        tree.insert_before(page, view, None);
        tree.flush_element_tree();

        let style = tree.document().get(view).unwrap().computed_style().unwrap();
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
        let page = tree.create_page();
        let view = tree.create_element("view");
        tree.insert_before(page, view, None);
        tree.flush_element_tree();

        let style = tree.document().get(view).unwrap().computed_style().unwrap();
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
            let page = tree.create_page();
            let view = tree.create_element("view");
            tree.insert_before(page, view, None);
            tree.flush_element_tree();

            let style = tree.document().get(view).unwrap().computed_style().unwrap();
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
        let page = tree.create_page();
        for _ in 0..2000 {
            let view = tree.create_element("view");
            tree.insert_before(page, view, None);
        }
        tree.flush_element_tree();
    }
}
