//! The element tree and its Element PAPI operations.

use std::fmt;

use dom::{self, Document, NodeId, StylesheetOrigin};

use crate::arena::{ElementArena, EventBinding, LynxElement};
use crate::device::Viewport;
use crate::ua::{PageConfig, ua_stylesheet};
use crate::{ElementId, IMAGE_TAG, PAGE_TAG, RAW_TEXT_TAG, TEXT_ATTRIBUTE, TEXT_TAG, VIEW_TAG};

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
    /// The page element cannot be given a parent — it is the permanent
    /// document element, pre-created with the document.
    CannotReparentPage,
    /// The page element cannot be dropped — the document element exists for
    /// the document's whole life (recorded limit: web-core would let a page
    /// be dropped and re-created, but no bundle does).
    CannotRemovePage,
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
            Self::CannotRemovePage => formatter.write_str("the page element cannot be dropped"),
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
    /// Whether `__CreatePage` has run. The page element itself is permanent —
    /// pre-created with the document — so this only gates the one-time
    /// binding of the page's component fields.
    page_created: bool,
    /// Whether a mutating PAPI call has run since the last
    /// [`Self::flush_element_tree`]. `__FlushElementTree` is the script's
    /// commit boundary; a frame producer sharing this tree across threads
    /// must not build from a half-applied batch (appended elements may not
    /// be styled yet), so it checks this before producing and falls back to
    /// its retained frame while a batch is open.
    uncommitted: bool,
    /// `__CreatePage`'s `componentID` — a string name web-core keeps in a side
    /// table rather than on the element, so it never reaches selectors.
    page_component_id: String,
    config: PageConfig,
}

/// The page's permanent unique id: the document element is pre-created with
/// the document, so the first live arena slot is always the page. Ids are
/// opaque handles to the main-thread script, which receives this one from
/// `__CreatePage` like any other.
pub(crate) const PAGE_UNIQUE_ID: ElementId = 1;

/// The DOM payload of a node that is not a Lynx element.
///
/// Arena slot zero is web-core's permanent "no element" sentinel, so this
/// payload resolves to no handle: [`ElementTree::element`] returns `None` for
/// it and every PAPI argument check rejects it. The runtime-owned text nodes
/// that materialize the `text` attribute carry it, which is what keeps them
/// out of the handle space the main-thread script sees.
pub(crate) const NO_ELEMENT: ElementId = 0;

impl ElementTree {
    /// Creates a tree for `viewport` with `config`'s UA cascade installed.
    /// The page element already exists as the document element;
    /// `__CreatePage` binds its component fields and returns its permanent
    /// id.
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

    /// The underlying document, for this crate's own unit tests only.
    ///
    /// DOM shape (append order, tags, committed styles) is this layer's
    /// semantics, so this layer's tests assert it; layers above observe
    /// through `ElementId`-vocabulary reads (`page`, `element`, `config`)
    /// and never see the document or `NodeId`. Production code has no
    /// consumer: mutation goes through the PAPI, and the engine drives
    /// rendering through the narrow methods this type forwards itself.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn document(&self) -> &Document<ElementId> {
        &self.document
    }

    /// Renders the document's retained scene if it is stale, returning
    /// whether a new scene was built.
    ///
    /// On the narrow mutable surface by the [`Self::handle_input`] admission
    /// rule: rendering flushes styles, layout, and paint state but creates,
    /// moves, and retires no element, so the handle table cannot
    /// desynchronise.
    pub fn render(&mut self) -> bool {
        self.document.render()
    }

    /// Whether a visual mutation has made the retained scene stale.
    #[must_use]
    pub fn needs_render(&self) -> bool {
        self.document.needs_render()
    }

    /// The scene retained by the last [`Self::render`].
    #[must_use]
    pub fn scene(&self) -> std::cell::Ref<'_, dom::vello::Scene> {
        self.document.scene()
    }

    /// Registers or updates decoded images for replaced content and CSS
    /// image values. Resource state only — no element is touched.
    pub fn images_mut(&mut self) -> &mut dom::ImageStore {
        self.document.images_mut()
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
    /// Deliberately returns nothing: the DOM-level response speaks `NodeId`,
    /// which stays out of this layer's public signatures. Dispatching through
    /// Lynx's own event model (`bindEvent`/`catchEvent` phases, the gesture
    /// arena, `hit-slop`) is the runtime layer's job, and when that layer
    /// arrives it gets `ElementId`-vocabulary queries designed for it — an
    /// unconsumed passthrough of the raw response is not that design.
    pub fn handle_input(&mut self, event: dom::input::InputEvent) {
        self.document.handle_input(event);
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

    /// The page element, once `__CreatePage` has run. The element itself is
    /// permanent; `None` only means the script has not named it yet.
    #[must_use]
    pub fn page(&self) -> Option<ElementId> {
        self.page_created.then_some(PAGE_UNIQUE_ID)
    }

    /// The `componentID` the page was created with; empty before
    /// `__CreatePage`.
    #[cfg(test)]
    #[must_use]
    pub fn page_component_id(&self) -> &str {
        &self.page_component_id
    }

    /// The DOM node a handle names, or `None` if the handle is not live.
    /// Crate-internal: `NodeId` stays out of this layer's public signatures;
    /// external liveness observation is [`Self::element`]`(id).is_some()`.
    #[must_use]
    pub(crate) fn node_id(&self, id: ElementId) -> Option<NodeId> {
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
        self.uncommitted = true;
        if !self.page_created {
            // `componentID` is a *string* name, not the numeric unique id, and
            // web-core keeps it out of the DOM — `create_element_common` files
            // it in a side table. Recording it here rather than as an
            // attribute keeps it invisible to selector matching; the DOM
            // payload remains only the context-owned unique id.
            component_id.clone_into(&mut self.page_component_id);
            self.elements
                .get_mut(PAGE_UNIQUE_ID)
                .expect("the page arena entry is permanent")
                .set_component_css_id(component_css_id);
            self.page_created = true;
        }
        PAGE_UNIQUE_ID
    }

    /// `__CreateView(parentComponentUniqueID)`.
    ///
    /// Creates a detached `view` element. `parent_component_unique_id` is `0`
    /// for "no parent component"; any other value must name a live element.
    pub fn create_view(
        &mut self,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        self.create_tagged(VIEW_TAG, parent_component_unique_id)
    }

    /// `__CreateElement(tagName, parentComponentUniqueID)` — a detached
    /// element with an arbitrary tag.
    ///
    /// web-core maps the Lynx tag to an HTML one because it renders into an
    /// HTML document; there is no HTML here, so the tag is kept verbatim, as
    /// it is for every other constructor.
    pub fn create_element(
        &mut self,
        tag: &str,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        self.create_tagged(tag, parent_component_unique_id)
    }

    /// `__CreateText(parentComponentUniqueID)` — a detached `text` element.
    pub fn create_text(
        &mut self,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        self.create_tagged(TEXT_TAG, parent_component_unique_id)
    }

    /// `__CreateImage(parentComponentUniqueID)` — a detached `image` element.
    pub fn create_image(
        &mut self,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        self.create_tagged(IMAGE_TAG, parent_component_unique_id)
    }

    /// `__CreateRawText(text)` — a detached `raw-text` element carrying `text`
    /// in its `text` attribute.
    ///
    /// web-core's constructor is exactly `createElement('raw-text')` plus
    /// `setAttribute('text', text)`, and passes no parent component, so
    /// neither does this.
    pub fn create_raw_text(&mut self, text: &str) -> ElementId {
        self.uncommitted = true;
        let id = self.insert(RAW_TEXT_TAG, 0, 0);
        self.write_attribute(id, TEXT_ATTRIBUTE, Some(text));
        id
    }

    /// The shared body of the tag-specific constructors.
    fn create_tagged(
        &mut self,
        tag: &str,
        parent_component_unique_id: ElementId,
    ) -> Result<ElementId, PapiError> {
        if parent_component_unique_id != NO_ELEMENT
            && self.node_id(parent_component_unique_id).is_none()
        {
            return Err(PapiError::UnknownElement(parent_component_unique_id));
        }
        self.uncommitted = true;
        Ok(self.insert(tag, parent_component_unique_id, 0))
    }

    /// `__GetElementUniqueID(element)`.
    ///
    /// The handle *is* the unique id here, so this is the liveness check the
    /// name implies: web-core reads the id web-core itself stamped on the DOM
    /// object, and a handle that never named an element has none.
    pub fn element_unique_id(&self, id: ElementId) -> Result<ElementId, PapiError> {
        self.element(id)
            .map(LynxElement::unique_id)
            .ok_or(PapiError::UnknownElement(id))
    }

    /// `__SetClasses(element, classNames)`.
    ///
    /// `None` is web-core's falsy `classname`, which removes the attribute
    /// rather than setting an empty one.
    pub fn set_classes(&mut self, id: ElementId, classes: Option<&str>) -> Result<(), PapiError> {
        let node = self.node_id(id).ok_or(PapiError::UnknownElement(id))?;
        self.uncommitted = true;
        match classes {
            Some(classes) if !classes.is_empty() => self.document.set_classes(node, classes),
            _ => self.document.remove_attribute(node, "class"),
        }
        Ok(())
    }

    /// `__SetID(element, id)`.
    ///
    /// `None` is web-core's falsy `id`, which removes the attribute.
    pub fn set_id(&mut self, id: ElementId, value: Option<&str>) -> Result<(), PapiError> {
        let node = self.node_id(id).ok_or(PapiError::UnknownElement(id))?;
        self.uncommitted = true;
        match value {
            Some(value) if !value.is_empty() => self.document.set_id_attribute(node, Some(value)),
            _ => self.document.set_id_attribute(node, None),
        }
        Ok(())
    }

    /// `__SetAttribute(element, key, value)`.
    ///
    /// A `None` value is web-core's nullish case, which removes the attribute.
    /// Everything else is already a string by the time it crosses the runtime
    /// boundary, matching web-core's `String(value)` coercion.
    pub fn set_attribute(
        &mut self,
        id: ElementId,
        name: &str,
        value: Option<&str>,
    ) -> Result<(), PapiError> {
        if self.node_id(id).is_none() {
            return Err(PapiError::UnknownElement(id));
        }
        self.uncommitted = true;
        self.write_attribute(id, name, value);
        Ok(())
    }

    /// `__SetInlineStyles(element, value)`.
    ///
    /// The declaration block replaces the previous one whole, which is what
    /// web-core's `setAttribute('style', …)` does; `None` is its falsy case and
    /// removes the attribute. The object form is serialized to a declaration
    /// block before it reaches this layer, because the Element PAPI boundary
    /// carries primitives only.
    pub fn set_inline_styles(&mut self, id: ElementId, css: Option<&str>) -> Result<(), PapiError> {
        let node = self.node_id(id).ok_or(PapiError::UnknownElement(id))?;
        self.uncommitted = true;
        match css {
            Some(css) if !css.is_empty() => self.document.set_inline_style(node, css),
            _ => self.document.remove_attribute(node, "style"),
        }
        Ok(())
    }

    /// `__SetCSSId(elements, cssId, entryName)`, for one element.
    ///
    /// The list form is flattened by the caller: the PAPI boundary carries
    /// primitives, and web-core's own implementation is a loop over unique ids
    /// too. `entryName` selects a CSS entry in a multi-entry bundle and is not
    /// recorded — this layer has one stylesheet set.
    ///
    /// Recorded limit: the id is stored, not honored. It scopes author rules to
    /// the components carrying it, and the runtime mounts every decoded sheet
    /// globally (which is what `enableRemoveCSSScope` bundles want anyway).
    pub fn set_css_id(&mut self, id: ElementId, css_id: i32) -> Result<(), PapiError> {
        self.elements
            .get_mut(id)
            .ok_or(PapiError::UnknownElement(id))?
            .set_component_css_id(css_id);
        Ok(())
    }

    /// `__AddEvent(element, eventType, eventName, handler)`.
    ///
    /// Records the binding on the element. Lynx and web-core both keep event
    /// bindings off the attribute set, so this one does not reach selectors
    /// either.
    ///
    /// Recorded limit: nothing dispatches the binding yet — there is no event
    /// model in this crate, and `handle_input` deliberately reports nothing.
    pub fn add_event(
        &mut self,
        id: ElementId,
        event_type: &str,
        name: &str,
        handler: Option<&str>,
    ) -> Result<(), PapiError> {
        self.elements
            .get_mut(id)
            .ok_or(PapiError::UnknownElement(id))?
            .set_event(EventBinding {
                event_type: event_type.to_owned(),
                name: name.to_owned(),
                handler: handler.map(str::to_owned),
            });
        Ok(())
    }

    /// The attribute write shared by `__SetAttribute` and `__CreateRawText`,
    /// on an already-validated handle.
    fn write_attribute(&mut self, id: ElementId, name: &str, value: Option<&str>) {
        let node = self.node_id(id).expect("the handle was validated");
        if name == TEXT_ATTRIBUTE {
            self.set_text_content(id, value);
        }
        match value {
            Some(value) => self.document.set_attribute(node, name, value),
            None => self.document.remove_attribute(node, name),
        }
    }

    /// Materializes the `text` attribute as this element's runtime-owned text
    /// node.
    ///
    /// The node is appended, which is web-core's ordering: its `raw-text` and
    /// `x-text` custom elements both append a `Text` node to their own light
    /// DOM when the attribute changes. The native engine emits the element's
    /// own content *before* its children instead, but no compiled bundle can
    /// tell the two apart — `ReactLynx` hoists a static string to the attribute
    /// only when it is the element's single child, so an element never carries
    /// both a `text` attribute and children.
    fn set_text_content(&mut self, id: ElementId, text: Option<&str>) {
        let existing = self
            .element(id)
            .expect("the handle was validated")
            .text_node();
        match (existing, text) {
            (Some(node), Some(text)) => self.document.set_text_node_data(node, text),
            (Some(node), None) => {
                self.document.remove_subtree(node);
                self.elements
                    .get_mut(id)
                    .expect("the handle was validated")
                    .set_text_node(None);
            }
            (None, Some(text)) => {
                let parent = self.node_id(id).expect("the handle was validated");
                let node = self.document.create_text_node(text, NO_ELEMENT);
                self.document.append_child(parent, node);
                self.elements
                    .get_mut(id)
                    .expect("the handle was validated")
                    .set_text_node(Some(node));
            }
            (None, None) => {}
        }
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
        if child == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotReparentPage);
        }
        if parent == child || self.document.is_ancestor(child_node, parent_node) {
            return Err(PapiError::WouldCycle { parent, child });
        }

        self.uncommitted = true;
        self.document.append_child(parent_node, child_node);
        Ok(child)
    }

    /// `__DropElement(id)`.
    ///
    /// The DOM subtree and every corresponding `LynxElement` are dropped
    /// together. Their `Vec` entries remain as permanent `None` tombstones, so
    /// no later creation can reuse any of their unique ids.
    pub fn drop_element(&mut self, id: ElementId) -> Result<(), PapiError> {
        if id == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotRemovePage);
        }
        let node = self.node_id(id).ok_or(PapiError::UnknownElement(id))?;
        self.uncommitted = true;
        let retired_ids = self.document.remove_subtree(node);
        for unique_id in retired_ids {
            // Runtime-owned text nodes carry the null unique id: they are DOM
            // nodes without a Lynx element, and go away with their owner.
            if unique_id == NO_ELEMENT {
                continue;
            }
            let retired = self.elements.retire(unique_id);
            debug_assert!(
                retired.is_some(),
                "a removed DOM node must have a live Lynx element"
            );
        }
        Ok(())
    }

    /// `__FlushElementTree()` — the single commit boundary.
    ///
    /// The page is permanently attached (it is the document element by
    /// construction), so a commit is exactly the style + layout pass that
    /// makes every pending mutation paint-eligible.
    pub fn flush_element_tree(&mut self) {
        self.document.layout();
        self.uncommitted = false;
    }

    /// Whether a mutating PAPI call has run since the last
    /// [`Self::flush_element_tree`] — the commit-boundary gate a shared
    /// frame producer checks before building.
    #[must_use]
    pub const fn has_uncommitted_mutations(&self) -> bool {
        self.uncommitted
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
        // The UA sheet sizes `page` to the viewport, so the flush produced
        // real geometry rather than a zero box.
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
    fn releasing_a_subtree_retires_every_lynx_element_in_it() {
        let mut tree = tree();
        let parent = tree.create_view(0).unwrap();
        let child = tree.create_view(0).unwrap();
        tree.append_element(parent, child).unwrap();

        tree.drop_element(parent).unwrap();
        assert!(tree.node_id(parent).is_none());
        assert!(tree.node_id(child).is_none());
        assert_eq!(tree.elements.len(), 4);

        let next = tree.create_view(0).unwrap();
        assert_eq!(next, 4);
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
    fn the_page_is_the_document_element_from_birth() {
        let mut tree = tree();
        let document_element = tree.document().document_element().id();
        let page = tree.create_page("page", 0);
        assert_eq!(tree.node_id(page), Some(document_element));

        // Flushes are plain re-commits; the attachment never changes.
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
        // The page is not yet script-visible, but it is real: the commit
        // styled it.
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

#[cfg(test)]
mod papi_tests {
    use super::{ElementTree, LynxElement, PapiError};
    use crate::device::Viewport;
    use crate::ua::PageConfig;

    fn tree() -> ElementTree {
        ElementTree::new(Viewport::new(393.0, 727.0), PageConfig::default())
    }

    /// The tag every constructor writes into the document, so author CSS type
    /// selectors from a bundle (`text { … }`) match what web-core's decoder
    /// would have rewritten to `x-text`.
    #[test]
    fn each_constructor_uses_its_lynx_tag_name() {
        let mut tree = tree();
        let view = tree.create_view(0).unwrap();
        let text = tree.create_text(0).unwrap();
        let image = tree.create_image(0).unwrap();
        let raw_text = tree.create_raw_text("hello");
        let div = tree.create_element("div", 0).unwrap();

        for (id, tag) in [
            (view, "view"),
            (text, "text"),
            (image, "image"),
            (raw_text, "raw-text"),
            (div, "div"),
        ] {
            let node = tree.node_id(id).unwrap();
            assert_eq!(tree.document().get(node).unwrap().tag_name(), Some(tag));
        }
    }

    #[test]
    fn create_raw_text_carries_its_content_in_the_text_attribute() {
        let mut tree = tree();
        let raw_text = tree.create_raw_text("Edit");
        let node = tree.node_id(raw_text).unwrap();
        assert_eq!(
            tree.document().get(node).unwrap().attribute("text"),
            Some("Edit")
        );
    }

    /// Lynx keeps text content in an attribute; `dom` measures and paints only
    /// DOM text nodes. The attribute stays selector-visible and the content
    /// becomes one runtime-owned child node.
    #[test]
    fn the_text_attribute_materializes_one_text_node_child() {
        let mut tree = tree();
        let text = tree.create_text(0).unwrap();
        tree.set_attribute(text, "text", Some("React")).unwrap();

        let node = tree.node_id(text).unwrap();
        let children = tree.document().get(node).unwrap().child_ids().to_vec();
        assert_eq!(children.len(), 1);
        let child = tree.document().get(children[0]).unwrap();
        assert!(child.is_text_node());
        assert_eq!(child.text(), Some("React"));
        // The attribute is still there for selectors.
        assert_eq!(
            tree.document().get(node).unwrap().attribute("text"),
            Some("React")
        );
    }

    #[test]
    fn re_setting_the_text_attribute_updates_the_same_node() {
        let mut tree = tree();
        let text = tree.create_raw_text("");
        tree.set_attribute(text, "text", Some("first")).unwrap();
        let node = tree.node_id(text).unwrap();
        let first = tree.document().get(node).unwrap().child_ids().to_vec();

        tree.set_attribute(text, "text", Some("second")).unwrap();
        let second = tree.document().get(node).unwrap().child_ids().to_vec();
        assert_eq!(first, second, "the text node is reused, not replaced");
        assert_eq!(
            tree.document().get(second[0]).unwrap().text(),
            Some("second")
        );
    }

    #[test]
    fn clearing_the_text_attribute_removes_the_text_node() {
        let mut tree = tree();
        let text = tree.create_text(0).unwrap();
        tree.set_attribute(text, "text", Some("gone")).unwrap();
        tree.set_attribute(text, "text", None).unwrap();

        let node = tree.node_id(text).unwrap();
        assert!(tree.document().get(node).unwrap().child_ids().is_empty());
        assert_eq!(tree.document().get(node).unwrap().attribute("text"), None);
        // Setting it again rebuilds the node.
        tree.set_attribute(text, "text", Some("back")).unwrap();
        assert_eq!(
            tree.document().get(node).unwrap().child_ids().len(),
            1,
            "the text node comes back"
        );
    }

    /// A runtime-owned text node is a DOM node with no Lynx element, so
    /// dropping its owner must not try to retire a handle for it.
    #[test]
    fn dropping_an_element_takes_its_runtime_owned_text_node_with_it() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        let text = tree.create_text(0).unwrap();
        tree.set_attribute(text, "text", Some("content")).unwrap();
        tree.append_element(page, text).unwrap();

        tree.drop_element(text).unwrap();
        assert!(tree.node_id(text).is_none());
        let page_node = tree.node_id(page).unwrap();
        assert!(
            tree.document()
                .get(page_node)
                .unwrap()
                .child_ids()
                .is_empty()
        );
    }

    #[test]
    fn set_classes_writes_the_class_attribute_and_clears_it() {
        let mut tree = tree();
        let view = tree.create_view(0).unwrap();
        tree.set_classes(view, Some("Banner Logo")).unwrap();
        let node = tree.node_id(view).unwrap();
        assert_eq!(
            tree.document().get(node).unwrap().attribute("class"),
            Some("Banner Logo")
        );

        tree.set_classes(view, None).unwrap();
        assert_eq!(tree.document().get(node).unwrap().attribute("class"), None);
    }

    #[test]
    fn set_id_writes_the_id_attribute_and_clears_it() {
        let mut tree = tree();
        let view = tree.create_view(0).unwrap();
        tree.set_id(view, Some("target")).unwrap();
        let node = tree.node_id(view).unwrap();
        assert_eq!(
            tree.document().get(node).unwrap().attribute("id"),
            Some("target")
        );

        tree.set_id(view, None).unwrap();
        assert_eq!(tree.document().get(node).unwrap().attribute("id"), None);
    }

    #[test]
    fn inline_styles_replace_the_previous_block_and_reach_computed_style() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        let view = tree.create_view(0).unwrap();
        tree.append_element(page, view).unwrap();
        tree.set_inline_styles(view, Some("width:10px;height:20px"))
            .unwrap();
        tree.flush_element_tree();

        let node = tree.node_id(view).unwrap();
        let layout = tree.document().rounded_layout(node).expect("laid out");
        assert!((layout.size.width - 10.0).abs() < f32::EPSILON);
        assert!((layout.size.height - 20.0).abs() < f32::EPSILON);

        // The whole block is replaced, not merged: the old `height` is gone.
        tree.set_inline_styles(view, Some("width:30px")).unwrap();
        tree.flush_element_tree();
        let layout = tree.document().rounded_layout(node).expect("laid out");
        assert!((layout.size.width - 30.0).abs() < f32::EPSILON);
        assert!(layout.size.height < 20.0);
    }

    #[test]
    fn clearing_inline_styles_removes_the_style_attribute() {
        let mut tree = tree();
        let view = tree.create_view(0).unwrap();
        tree.set_inline_styles(view, Some("width:10px")).unwrap();
        tree.set_inline_styles(view, None).unwrap();
        let node = tree.node_id(view).unwrap();
        assert_eq!(tree.document().get(node).unwrap().attribute("style"), None);
    }

    /// Author CSS from a bundle matches the Lynx tag names verbatim — there is
    /// no HTML document to rewrite them for.
    #[test]
    fn author_css_matches_lynx_tag_names_and_the_page_is_the_root() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        let text = tree.create_text(0).unwrap();
        tree.append_element(page, text).unwrap();
        tree.add_author_stylesheet(":root { --ink: rgb(1, 2, 3); } text { color: var(--ink); }");
        tree.flush_element_tree();

        let node = tree.node_id(text).unwrap();
        let style = tree
            .document()
            .get(node)
            .unwrap()
            .computed_style()
            .expect("committed style");
        let color = style
            .clone_color()
            .to_color_space(dom::stylo::color::ColorSpace::Srgb);
        let channel = |component: f32| (component * 255.0).round();
        assert_eq!(
            (
                channel(color.components.0),
                channel(color.components.1),
                channel(color.components.2)
            ),
            (1.0, 2.0, 3.0),
            "the custom property inherited from the page and resolved on text"
        );
    }

    /// A `<text>` lays its children out rather than swallowing them: web-core
    /// gives `x-text` a real container display, and the layout engine hides the
    /// children of anything it treats as a leaf.
    #[test]
    fn a_text_element_lays_out_its_raw_text_children() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        let text = tree.create_text(0).unwrap();
        let raw_text = tree.create_raw_text("Edit");
        tree.append_element(page, text).unwrap();
        tree.append_element(text, raw_text).unwrap();
        tree.flush_element_tree();

        let text_node = tree.node_id(text).unwrap();
        let layout = tree
            .document()
            .rounded_layout(text_node)
            .expect("the text element is laid out");
        assert!(
            layout.size.width > 0.0 && layout.size.height > 0.0,
            "a text element with content must have a box: {layout:?}"
        );
    }

    #[test]
    fn event_bindings_stay_off_the_attribute_set() {
        let mut tree = tree();
        let view = tree.create_view(0).unwrap();
        tree.add_event(view, "bindEvent", "tap", Some("-3:0:"))
            .unwrap();

        let events = tree.element(view).map(LynxElement::events).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "bindEvent");
        assert_eq!(events[0].name, "tap");
        assert_eq!(events[0].handler.as_deref(), Some("-3:0:"));

        // Lynx and web-core both keep bindings out of the attribute set, so
        // author CSS can never select on them.
        let node = tree.node_id(view).unwrap();
        assert_eq!(tree.document().get(node).unwrap().attributes().len(), 0);
    }

    #[test]
    fn every_mutating_member_rejects_a_dead_handle() {
        let mut tree = tree();
        let ghost = 99;
        assert_eq!(
            tree.set_classes(ghost, Some("x")).unwrap_err(),
            PapiError::UnknownElement(ghost)
        );
        assert_eq!(
            tree.set_attribute(ghost, "text", Some("x")).unwrap_err(),
            PapiError::UnknownElement(ghost)
        );
        assert_eq!(
            tree.set_inline_styles(ghost, Some("width:1px"))
                .unwrap_err(),
            PapiError::UnknownElement(ghost)
        );
        assert_eq!(
            tree.set_id(ghost, Some("x")).unwrap_err(),
            PapiError::UnknownElement(ghost)
        );
        assert_eq!(
            tree.set_css_id(ghost, 1).unwrap_err(),
            PapiError::UnknownElement(ghost)
        );
        assert_eq!(
            tree.add_event(ghost, "bindEvent", "tap", None).unwrap_err(),
            PapiError::UnknownElement(ghost)
        );
        assert_eq!(
            tree.element_unique_id(ghost).unwrap_err(),
            PapiError::UnknownElement(ghost)
        );
    }
}
