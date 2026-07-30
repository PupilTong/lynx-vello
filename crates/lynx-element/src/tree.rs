//! The element tree and its Element PAPI operations.

use std::fmt;
use std::num::NonZeroU32;

use dom::{Document, NodeId, StylesheetOrigin};

use crate::device::Viewport;
use crate::ua::{PageConfig, ua_stylesheet};
use crate::{PAGE_TAG, VIEW_TAG};

/// A Lynx element handle: the unique id the Element PAPI hands to and takes
/// from the main-thread script.
///
/// Ids are dense and start at 1 — web-core's `unique_id_to_element_map` is
/// seeded with one `None` so that index 0 can mean "no element" — and are
/// never recycled, unlike the `dom` `NodeId` they resolve to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElementId(NonZeroU32);

impl ElementId {
    /// The raw unique id, as the script sees it.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// Reads a handle back from the script boundary.
    ///
    /// Returns `None` for `0`, the sentinel the PAPI uses for "no element" —
    /// callers that accept that sentinel must handle it before calling here.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match NonZeroU32::new(raw) {
            Some(id) => Some(Self(id)),
            None => None,
        }
    }
}

impl fmt::Display for ElementId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "#{}", self.0)
    }
}

/// The Lynx payload every element carries in the DOM core's opaque slot.
///
/// The core neither reads nor derives selector state from it — it is this
/// layer's own bookkeeping, which is exactly the contract
/// `docs/style-architecture.md` sets for the generic payload `T`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(
    clippy::struct_field_names,
    reason = "each field is named after the Element PAPI argument it records, and those names \
              all end in `id`; renaming them would break the correspondence"
)]
pub struct ElementData {
    /// The handle the script holds for this element.
    pub unique_id: ElementId,
    /// The `parentComponentUniqueID` argument the element was created with;
    /// `0` means "no parent component". Recorded, not yet honored (there is no
    /// `__SetCSSId` to inherit a CSS fragment id into).
    pub parent_component_unique_id: u32,
    /// The `componentCSSID` a page was created with; `0` for non-page
    /// elements. Recorded for the same reason.
    pub component_css_id: i32,
}

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
    UnknownElement(u32),
    /// Appending `child` under `parent` would put a node inside its own
    /// subtree.
    WouldCycle { parent: ElementId, child: ElementId },
    /// The page element cannot be given a parent — it is the document element
    /// by construction, and `__FlushElementTree` is what attaches it.
    CannotReparentPage,
    /// The append would nest elements deeper than [`MAX_TREE_DEPTH`].
    TooDeep { limit: u32 },
}

impl fmt::Display for PapiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownElement(raw) => {
                write!(formatter, "no element has the unique id {raw}")
            }
            Self::WouldCycle { parent, child } => write!(
                formatter,
                "appending {child} under {parent} would form a cycle"
            ),
            Self::CannotReparentPage => {
                formatter.write_str("the page element cannot be given a parent")
            }
            Self::TooDeep { limit } => write!(
                formatter,
                "the element tree cannot nest deeper than {limit} levels"
            ),
        }
    }
}

impl std::error::Error for PapiError {}

/// How deep the Element PAPI lets a script nest elements.
///
/// `dom`'s layout, paint-order, and hit-test walks are recursive, so a
/// sufficiently deep tree overflows the thread stack and **aborts the
/// process** — not something untrusted main-thread script should be able to
/// do. Measured on a 2 MiB thread (libtest's default, the smallest stack this
/// code runs on) the wall is between 300 and 350 levels; this limit sits below
/// that with room to spare, and far above any real UI, which nests tens of
/// levels rather than hundreds.
///
/// This is a guard, not a fix. The fix is iterative traversal in `dom` and
/// `hughie`, after which the limit can rise or go away.
pub const MAX_TREE_DEPTH: u32 = 256;

/// What the handle table stores per live element.
#[derive(Clone, Copy, Debug)]
struct Slot {
    node: NodeId,
    /// Levels of descendants below this element; a leaf is `0`. Maintained on
    /// append so the depth guard costs no tree walk.
    height: u32,
}

/// One Lynx element tree: a `dom` document plus the unique-id handle space and
/// page policy the Element PAPI speaks in.
#[derive(Debug)]
pub struct ElementTree {
    document: Document<ElementData>,
    /// Handle table indexed by unique id. Slot 0 is the reserved "no element"
    /// sentinel; a retired element's slot becomes `None` and is never reused.
    nodes: Vec<Option<Slot>>,
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
            nodes: vec![None],
            page: None,
            page_component_id: String::new(),
            page_attached: false,
            config,
        }
    }

    /// The underlying document, for style/layout/paint queries.
    #[must_use]
    pub const fn document(&self) -> &Document<ElementData> {
        &self.document
    }

    /// The paint order for the current tree, laying it out first.
    ///
    /// This is the mutable operation a renderer needs. There is deliberately no
    /// `document_mut`: handing out `&mut Document` would let a caller remove or
    /// move nodes behind this layer's back, desynchronising the handle table,
    /// the page state, and the height cache — and the DOM core is
    /// crash-on-misuse, so the next PAPI call would panic rather than return
    /// [`PapiError`].
    pub fn paint_order(&mut self) -> dom::visual::PaintOrder {
        self.document.paint_order()
    }

    /// Feeds one host input event in, hit-testing it against `frame` and
    /// performing whatever UA default action it resolves to — today, scrolling
    /// an `overflow: scroll` box.
    ///
    /// This belongs on the narrow mutable surface for the same reason
    /// [`Self::set_viewport`] does: input cannot desynchronise the handle
    /// table. All it can reach is scroll offsets and per-pointer gesture
    /// state — no element is created, moved, or retired — so lending it out
    /// costs none of the invariants [`Self::paint_order`] protects.
    ///
    /// Dispatching the returned target through Lynx's own event model
    /// (`bindEvent`/`catchEvent` phases, the gesture arena, `hit-slop`) is the
    /// runtime layer's job, not this one's; it prevents the default action and
    /// takes over when it wants different behavior.
    pub fn handle_input(
        &mut self,
        frame: &dom::visual::PaintOrder,
        event: dom::input::InputEvent,
    ) -> dom::input::InputResponse {
        self.document.handle_input(frame, event)
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
    #[must_use]
    pub fn page_component_id(&self) -> &str {
        &self.page_component_id
    }

    /// Whether the page has been attached to the document — i.e. whether
    /// `__FlushElementTree` has committed at least once.
    #[must_use]
    pub const fn is_flushed(&self) -> bool {
        self.page_attached
    }

    /// The DOM node a handle names, or `None` if the handle is not live.
    #[must_use]
    pub fn node_id(&self, id: ElementId) -> Option<NodeId> {
        self.slot(id).map(|slot| slot.node)
    }

    fn slot(&self, id: ElementId) -> Option<Slot> {
        self.nodes.get(id.get() as usize).copied().flatten()
    }

    /// This element's distance from the root of the tree it currently sits in,
    /// counting at most `MAX_TREE_DEPTH + 1` steps — enough to decide the
    /// guard without walking an arbitrarily long chain.
    fn depth_of(&self, node: NodeId) -> u32 {
        let mut depth = 0;
        let mut current = node;
        while let Some(parent) = self.document.get(current).and_then(dom::Node::parent_id) {
            depth += 1;
            if depth > MAX_TREE_DEPTH {
                return depth;
            }
            current = parent;
        }
        depth
    }

    /// Republishes `element`'s height to its ancestors after it gained a
    /// subtree. Stops as soon as an ancestor is already tall enough, and can
    /// never walk further than the depth limit the guard enforces.
    fn raise_heights(&mut self, element: ElementId, height: u32) {
        let mut current = Some(element);
        let mut required = height;
        let mut steps = 0;
        while let Some(id) = current {
            let Some(mut slot) = self.slot(id) else {
                return;
            };
            if slot.height >= required {
                return;
            }
            slot.height = required;
            self.nodes[id.get() as usize] = Some(slot);
            required += 1;
            steps += 1;
            if steps > MAX_TREE_DEPTH {
                return;
            }
            current = self
                .document
                .get(slot.node)
                .and_then(dom::Node::parent_id)
                .and_then(|parent| self.handle_of(parent));
        }
    }

    /// An element's true height, from its children's recorded heights.
    fn height_from_children(&self, node: NodeId) -> u32 {
        let Some(element) = self.document.get(node) else {
            return 0;
        };
        element
            .child_ids()
            .iter()
            .filter_map(|&child| self.handle_of(child))
            .filter_map(|handle| self.slot(handle))
            .map(|slot| slot.height.saturating_add(1))
            .max()
            .unwrap_or(0)
    }

    /// Recomputes heights up the chain a subtree was just taken from.
    ///
    /// [`raise_heights`](Self::raise_heights) only ever grows a height, which
    /// is right for an append but leaves the *old* ancestors claiming a subtree
    /// they no longer have. Left stale, a node that is now a genuine leaf keeps
    /// its old height and gets refused as [`PapiError::TooDeep`] the next time
    /// something tries to append it somewhere deep.
    ///
    /// Stops at the first ancestor whose height is unchanged: nothing above it
    /// can have changed either.
    fn lower_heights(&mut self, from: ElementId) {
        let mut current = Some(from);
        for _ in 0..=MAX_TREE_DEPTH {
            let Some(id) = current else {
                return;
            };
            let Some(node) = self.node_id(id) else {
                return;
            };
            let height = self.height_from_children(node);
            let Some(mut slot) = self.slot(id) else {
                return;
            };
            if slot.height == height {
                return;
            }
            slot.height = height;
            self.nodes[id.get() as usize] = Some(slot);
            current = self
                .document
                .get(node)
                .and_then(dom::Node::parent_id)
                .and_then(|parent| self.handle_of(parent));
        }
    }

    /// The handle a DOM node belongs to, read back from its opaque payload.
    fn handle_of(&self, node: NodeId) -> Option<ElementId> {
        self.document
            .get(node)
            .filter(|node| node.is_element())
            .map(|node| node.payload().unique_id)
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
        // it invisible to selector matching, which is what the DOM core's
        // opaque-payload contract requires of this layer.
        component_id.clone_into(&mut self.page_component_id);
        self.page = Some(id);
        id
    }

    /// `__CreateView(parentComponentUniqueID)`.
    ///
    /// Creates a detached `view` element. `parent_component_unique_id` is `0`
    /// for "no parent component"; any other value must name a live element.
    pub fn create_view(&mut self, parent_component_unique_id: u32) -> Result<ElementId, PapiError> {
        if let Some(parent_component) = ElementId::from_raw(parent_component_unique_id)
            && self.node_id(parent_component).is_none()
        {
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
            .ok_or(PapiError::UnknownElement(parent.get()))?;
        let child_node = self
            .node_id(child)
            .ok_or(PapiError::UnknownElement(child.get()))?;
        if self.page == Some(child) {
            return Err(PapiError::CannotReparentPage);
        }
        if parent == child || self.document.is_ancestor(child_node, parent_node) {
            return Err(PapiError::WouldCycle { parent, child });
        }

        // Reject before mutating: `dom`'s recursive layout walk would abort the
        // process on a deep enough tree, and the main-thread script is
        // untrusted input. `height` makes this exact for grafted subtrees too —
        // joining two 200-level chains is caught, not just growing one leaf at
        // a time.
        let child_height = self.slot(child).map_or(0, |slot| slot.height);
        let depth = self.depth_of(parent_node) + 1 + child_height;
        if depth > MAX_TREE_DEPTH {
            return Err(PapiError::TooDeep {
                limit: MAX_TREE_DEPTH,
            });
        }

        // `append_child` detaches first, so the chain the child is leaving has
        // to give its height back.
        let previous_parent = self
            .document
            .get(child_node)
            .and_then(dom::Node::parent_id)
            .and_then(|node| self.handle_of(node));

        self.document.append_child(parent_node, child_node);
        self.raise_heights(parent, child_height + 1);
        if let Some(previous) = previous_parent
            && previous != parent
        {
            self.lower_heights(previous);
        }
        Ok(child)
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
        parent_component_unique_id: u32,
        component_css_id: i32,
    ) -> ElementId {
        let raw = u32::try_from(self.nodes.len())
            .expect("a Lynx element tree cannot hold more than u32::MAX elements");
        let unique_id = ElementId(NonZeroU32::new(raw).expect("slot 0 is the reserved sentinel"));
        let node = self.document.create_element(
            tag,
            ElementData {
                unique_id,
                parent_component_unique_id,
                component_css_id,
            },
        );
        self.nodes.push(Some(Slot { node, height: 0 }));
        unique_id
    }
}

#[cfg(test)]
mod tests {
    use super::{ElementId, ElementTree, MAX_TREE_DEPTH, PapiError};
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
        assert_eq!(page.get(), 1);
        assert_eq!(first.get(), 2);
        assert_eq!(second.get(), 3);
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
        assert!(ElementId::from_raw(0).is_none());
        let mut tree = tree();
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
        let ghost = ElementId::from_raw(99).unwrap();
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
            stylo::computed_values::box_sizing::T::BorderBox
        );
        assert_eq!(
            style.clone_display(),
            stylo::values::computed::Display::Linear
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
            stylo::values::computed::Display::Linear
        );
        // `box-sizing` is not gated by this switch and still applies.
        assert_eq!(
            style.clone_box_sizing(),
            stylo::computed_values::box_sizing::T::BorderBox
        );
    }

    #[test]
    fn the_overflow_page_config_switch_reaches_computed_style() {
        for (visible, expected) in [
            (true, stylo::values::computed::Overflow::Visible),
            (false, stylo::values::computed::Overflow::Hidden),
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

    /// Nesting past the limit is refused rather than allowed to overflow the
    /// stack during layout. Without the guard this aborts the whole process.
    #[test]
    fn nesting_past_the_limit_is_refused_rather_than_aborting() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        let mut deepest = page;
        // `page` is level 0, so MAX_TREE_DEPTH more appends fill the budget.
        for level in 0..MAX_TREE_DEPTH {
            let view = tree.create_view(0).unwrap();
            deepest = tree
                .append_element(deepest, view)
                .unwrap_or_else(|error| panic!("level {level} should fit: {error}"));
        }

        let one_too_many = tree.create_view(0).unwrap();
        assert_eq!(
            tree.append_element(deepest, one_too_many).unwrap_err(),
            PapiError::TooDeep {
                limit: MAX_TREE_DEPTH
            }
        );

        // The refused append changed nothing, and the tree still lays out.
        assert!(tree.flush_element_tree());
    }

    /// The guard tracks subtree height, so grafting two deep detached chains
    /// together is caught — not just growing one a leaf at a time.
    #[test]
    fn grafting_two_deep_subtrees_is_refused() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);

        let mut chain = |length: u32| {
            let root = tree.create_view(0).unwrap();
            let mut tip = root;
            for _ in 1..length {
                let view = tree.create_view(0).unwrap();
                tip = tree.append_element(tip, view).unwrap();
            }
            (root, tip)
        };
        // Two chains that each fit, but whose combined depth does not.
        let (first_root, first_tip) = chain(MAX_TREE_DEPTH / 2 + 1);
        let (second_root, _) = chain(MAX_TREE_DEPTH / 2 + 1);

        // Each chain is legal on its own, and one still fits under the page.
        tree.append_element(page, first_root).unwrap();
        assert_eq!(
            tree.append_element(first_tip, second_root).unwrap_err(),
            PapiError::TooDeep {
                limit: MAX_TREE_DEPTH
            }
        );
    }

    /// Height bookkeeping must not make a legal wide tree fail: thousands of
    /// siblings are fine, only depth is bounded.
    /// A node that has given its subtree away is a leaf again, and the guard
    /// must see that. The height cache only ever grew before, so a genuine leaf
    /// kept claiming its old depth and was refused as `TooDeep`.
    #[test]
    fn giving_a_subtree_away_lowers_the_recorded_height() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);

        // A deep anchor, so a stale height would push the check over the limit.
        let mut anchor = page;
        for _ in 0..55 {
            let view = tree.create_view(0).unwrap();
            anchor = tree.append_element(anchor, view).unwrap();
        }

        // A detached chain 205 levels tall, rooted at `holder`.
        let holder = tree.create_view(0).unwrap();
        let mut tip = holder;
        for _ in 0..205 {
            let view = tree.create_view(0).unwrap();
            tip = tree.append_element(tip, view).unwrap();
        }

        // Move everything below `holder` elsewhere; `holder` is a leaf now.
        let holder_node = tree.node_id(holder).unwrap();
        let child_node = tree.document().get(holder_node).unwrap().child_ids()[0];
        let child = tree.document().get(child_node).unwrap().payload().unique_id;
        let parking = tree.create_view(0).unwrap();
        tree.append_element(parking, child).unwrap();
        assert!(
            tree.document()
                .get(holder_node)
                .unwrap()
                .child_ids()
                .is_empty()
        );

        // 55 + 1 + 0 fits easily; 55 + 1 + 205 would not.
        tree.append_element(anchor, holder)
            .expect("a leaf must not be refused for a subtree it no longer has");
    }

    #[test]
    fn a_wide_tree_is_not_affected_by_the_depth_guard() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        for _ in 0..2000 {
            let view = tree.create_view(0).unwrap();
            tree.append_element(page, view).unwrap();
        }
        assert!(tree.flush_element_tree());
    }

    #[test]
    fn the_payload_carries_the_handle_back_from_a_node() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let view = tree.create_view(page.get()).unwrap();
        let node = tree.node_id(view).unwrap();
        let data = *tree.document().get(node).unwrap().payload();
        assert_eq!(data.unique_id, view);
        assert_eq!(data.parent_component_unique_id, page.get());
    }
}
