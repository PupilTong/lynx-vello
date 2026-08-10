//! The element tree and its Element PAPI operations.

use std::fmt;

use dom::{self, Document, NodeId, StylesheetOrigin};

use crate::arena::{ElementArena, LynxElement};
use crate::device::Viewport;
use crate::ua::{PageConfig, ua_stylesheet};
use crate::value::PapiValue;
use crate::{
    ElementId, FRAME_TAG, IMAGE_TAG, INHERITED_CSS_ID_NONE, NO_ELEMENT, PAGE_TAG, RAW_TEXT_TAG,
    RAW_TEXT_TEXT_ATTRIBUTE, SCROLL_VIEW_TAG, STYLE_ATTRIBUTE, TEXT_TAG, VIEW_TAG, WRAPPER_TAG,
};

/// Why an Element PAPI call was rejected.
///
/// The main-thread script is untrusted input: `docs/style-architecture.md`
/// requires this layer to validate handles before calling the crash-on-misuse
/// DOM core, so every fallible PAPI entry point returns this instead of
/// panicking.

// Not `Eq`: `NumericStyleKey` carries the rejected key verbatim so the
// message can name it, and the key crossed the host boundary as a `f64`.
#[derive(Clone, Debug, PartialEq)]
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
    /// The page element cannot be detached or dropped — the document element
    /// exists for the document's whole life.
    CannotRemovePage,
    /// An operation naming a child of a specific parent was given a node that
    /// is not one, which the DOM core would treat as a panic-worthy misuse.
    NotAChild { parent: ElementId, child: ElementId },
    /// `__AddInlineStyle` was called with a numeric CSS property id.
    NumericStyleKey(f64),
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
            Self::CannotRemovePage => formatter.write_str("the page element cannot be removed"),
            Self::NotAChild { parent, child } => {
                write!(formatter, "#{child} is not a child of #{parent}")
            }
            Self::NumericStyleKey(key) => write!(
                formatter,
                "__AddInlineStyle received the numeric CSS property id {key}; \
                 this engine has no Lynx property-id table, so only string \
                 property names are accepted"
            ),
        }
    }
}

impl std::error::Error for PapiError {}

/// Whether a node id names an element, for the navigation members that skip
/// everything else.
fn is_element(document: &Document<ElementId>, node: NodeId) -> bool {
    document.get(node).is_some_and(dom::Node::is_element)
}

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
    config: PageConfig,
}

/// The page's permanent unique id: the document element is pre-created with
/// the document, so the first live arena slot is always the page. Ids are
/// opaque handles to the main-thread script, which receives this one from
/// `__CreatePage` like any other.
pub(crate) const PAGE_UNIQUE_ID: ElementId = 1;

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
            config,
        };
        let page_node = tree.document.document_element().id();
        tree.elements
            .insert(PAGE_UNIQUE_ID, page_node, NO_ELEMENT, INHERITED_CSS_ID_NONE);
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
    ///
    /// This is `__GetPageElement`, whose web-core implementation returns the
    /// cached page or `undefined` for exactly the same reason
    /// (`createElementAPI.ts:526`).
    #[must_use]
    pub fn page(&self) -> Option<ElementId> {
        self.page_created.then_some(PAGE_UNIQUE_ID)
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

    // ---------------------------------------------------------------- create

    /// `__CreatePage(componentID, componentCSSID)`.
    ///
    /// Idempotent, like web-core's: a second call returns the page that
    /// already exists and ignores its arguments (`createElementAPI.ts:284`).
    /// The native engine constructs a fresh `PageElement` per call instead
    /// (`element_manager.cc:1298`); web-core is the contract this runtime
    /// reproduces.
    pub fn create_page(&mut self, component_id: &str, component_css_id: i32) -> ElementId {
        self.uncommitted = true;
        if !self.page_created {
            let page = self
                .elements
                .get_mut(PAGE_UNIQUE_ID)
                .expect("the page arena entry is permanent");
            // `componentID` is a *string* name, not the numeric unique id, and
            // web-core keeps it out of the DOM — `create_element_common` files
            // it in a side table. Recording it on the arena entry rather than
            // as an attribute keeps it invisible to selector matching; the DOM
            // payload remains only the context-owned unique id.
            page.set_component_id(component_id);
            page.set_css_id(component_css_id);
            page.set_component_css_id(component_css_id);
            self.page_created = true;
        }
        PAGE_UNIQUE_ID
    }

    /// `__CreateElement(tagName, parentComponentUniqueID)`.
    ///
    /// The tag is stored verbatim. web-core rewrites it through
    /// `LYNX_TAG_TO_HTML_TAG_MAP` (`view` → `x-view`, …) only because it
    /// renders into an HTML document and needs custom elements to exist;
    /// `__GetTag` reverse-maps it back, so the *observable* tag is the Lynx
    /// one, which is what this engine stores directly.
    pub fn create_element(
        &mut self,
        tag: &str,
        parent_component_unique_id: ElementId,
    ) -> ElementId {
        self.insert(tag, parent_component_unique_id)
    }

    /// `__CreateView(parentComponentUniqueID)`.
    pub fn create_view(&mut self, parent_component_unique_id: ElementId) -> ElementId {
        self.insert(VIEW_TAG, parent_component_unique_id)
    }

    /// `__CreateText(parentComponentUniqueID)`.
    pub fn create_text(&mut self, parent_component_unique_id: ElementId) -> ElementId {
        self.insert(TEXT_TAG, parent_component_unique_id)
    }

    /// `__CreateImage(parentComponentUniqueID)`.
    pub fn create_image(&mut self, parent_component_unique_id: ElementId) -> ElementId {
        self.insert(IMAGE_TAG, parent_component_unique_id)
    }

    /// `__CreateScrollView(parentComponentUniqueID)`.
    pub fn create_scroll_view(&mut self, parent_component_unique_id: ElementId) -> ElementId {
        self.insert(SCROLL_VIEW_TAG, parent_component_unique_id)
    }

    /// `__CreateWrapperElement(parentComponentUniqueID)`.
    pub fn create_wrapper_element(&mut self, parent_component_unique_id: ElementId) -> ElementId {
        self.insert(WRAPPER_TAG, parent_component_unique_id)
    }

    /// `__CreateFrame(parentComponentUniqueID)`.
    pub fn create_frame(&mut self, parent_component_unique_id: ElementId) -> ElementId {
        self.insert(FRAME_TAG, parent_component_unique_id)
    }

    /// `__CreateRawText(text)`.
    ///
    /// A `raw-text` **element** whose content is its `text` attribute, exactly
    /// as in web-core (`createElementAPI.ts:207-215`) and in the native engine
    /// (`RawTextElement`): the framework later rewrites the content with
    /// `__SetAttribute(element, "text", …)`, so the text has to live somewhere
    /// an attribute write can reach.
    ///
    /// This engine paints text from DOM text nodes, so the element also owns
    /// one mirroring that attribute. The mirror carries the [`NO_ELEMENT`]
    /// payload — it is not separately addressable, and no PAPI handle ever
    /// names it. The UA sheet gives `raw-text` `display: contents`, so the
    /// mirror is measured as an item of the enclosing `<text>`'s formatting
    /// context rather than inside a box of its own.
    ///
    /// web-core passes `-1` as the parent component here, which is out of
    /// range of its element map and therefore means "no CSS scope"; the same
    /// value is spelled [`NO_ELEMENT`] on this side.
    pub fn create_raw_text(&mut self, text: &str) -> ElementId {
        self.uncommitted = true;
        let unique_id = self.elements.reserve();
        let node = self.document.create_element(RAW_TEXT_TAG, unique_id);
        self.document
            .set_attribute(node, RAW_TEXT_TEXT_ATTRIBUTE, text);
        let mirror = self.document.create_text_node(text, NO_ELEMENT);
        self.document.append_child(node, mirror);
        self.elements
            .insert(unique_id, node, NO_ELEMENT, INHERITED_CSS_ID_NONE);
        self.elements
            .get_mut(unique_id)
            .expect("the element was just inserted")
            .set_text_mirror(mirror);
        unique_id
    }

    // ------------------------------------------------------------- structure

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
        self.link(parent, child, None)
    }

    /// `__InsertElementBefore(parent, child, reference)`.
    ///
    /// A [`NO_ELEMENT`] reference appends, matching `insertBefore(child, null)`
    /// (`pureElementPAPIs.ts:67-71`). A reference that is not a child of
    /// `parent` is a [`PapiError::NotAChild`] rather than a panic: the DOM
    /// core treats it as misuse, and the main-thread script is untrusted
    /// input. Inserting a node before itself is a no-op, as in the DOM.
    pub fn insert_element_before(
        &mut self,
        parent: ElementId,
        child: ElementId,
        reference: ElementId,
    ) -> Result<ElementId, PapiError> {
        if reference == NO_ELEMENT {
            return self.link(parent, child, None);
        }
        if reference == child {
            // `parent.insertBefore(child, child)` is a legal DOM no-op, and a
            // diffing framework does emit it. `dom` debug-asserts against it,
            // so it must not reach the core. It is only a no-op for a node
            // that already *is* a child of `parent`, though: the DOM runs
            // pre-insertion validity before it rewrites the reference to the
            // node's own next sibling, so a foreign or detached node throws
            // `NotFoundError` there rather than being silently dropped.
            let child_node = self
                .node_id(child)
                .ok_or(PapiError::UnknownElement(child))?;
            let parent_node = self
                .node_id(parent)
                .ok_or(PapiError::UnknownElement(parent))?;
            if self.document.get(child_node).and_then(dom::Node::parent_id) != Some(parent_node) {
                return Err(PapiError::NotAChild { parent, child });
            }
            return Ok(child);
        }
        let reference_node = self
            .node_id(reference)
            .ok_or(PapiError::UnknownElement(reference))?;
        self.link(parent, child, Some((reference, reference_node)))
    }

    /// `__RemoveElement(parent, child)` — detach only.
    ///
    /// The child stays fully alive: its handle keeps resolving, its
    /// attributes, classes, and dataset survive, and re-inserting it anywhere
    /// restores it. That is web-core's `parent.removeChild(child)`
    /// (`pureElementPAPIs.ts:81-84`) and the contract `ReactLynx`'s reconciler
    /// relies on — it removes and re-inserts the same elements while
    /// reordering. Destroying an element is [`Self::drop_element`], which no
    /// PAPI member reaches.
    pub fn remove_element(
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
            return Err(PapiError::CannotRemovePage);
        }
        if self.document.get(child_node).and_then(dom::Node::parent_id) != Some(parent_node) {
            return Err(PapiError::NotAChild { parent, child });
        }
        self.uncommitted = true;
        self.document.detach(child_node);
        Ok(child)
    }

    /// `__ReplaceElement(newElement, oldElement)` — new element first.
    ///
    /// web-core is `oldElement.replaceWith(newElement)`
    /// (`pureElementPAPIs.ts:86-89`), so the parent is read off the old
    /// element rather than passed in. Replacing a detached element is a no-op
    /// there; here it is [`PapiError::NotAChild`] with `parent` reported as
    /// [`NO_ELEMENT`], because silently discarding the new element would be
    /// the worse failure.
    pub fn replace_element(
        &mut self,
        new_element: ElementId,
        old_element: ElementId,
    ) -> Result<ElementId, PapiError> {
        let old_node = self
            .node_id(old_element)
            .ok_or(PapiError::UnknownElement(old_element))?;
        self.node_id(new_element)
            .ok_or(PapiError::UnknownElement(new_element))?;
        if old_element == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotRemovePage);
        }
        let Some(parent_node) = self.document.get(old_node).and_then(dom::Node::parent_id) else {
            return Err(PapiError::NotAChild {
                parent: NO_ELEMENT,
                child: old_element,
            });
        };
        if new_element == old_element {
            return Ok(new_element);
        }
        let parent = *self
            .document
            .get(parent_node)
            .expect("a live parent node")
            .payload();
        self.link(parent, new_element, Some((old_element, old_node)))?;
        self.document.detach(old_node);
        Ok(new_element)
    }

    /// `__SwapElement(a, b)` — exchange two elements' positions.
    ///
    /// web-core does it with a transient placeholder element and three
    /// `replaceWith` calls (`pureElementPAPIs.ts:152-160`); the native engine
    /// removes both and re-inserts by saved index
    /// (`renderer_functions.cc:3469-3485`). Both end in the same order. This
    /// takes the native shape, because inserting a placeholder into the tree
    /// would briefly make a non-Lynx element observable to layout.
    pub fn swap_element(&mut self, first: ElementId, second: ElementId) -> Result<(), PapiError> {
        if first == second {
            self.node_id(first)
                .ok_or(PapiError::UnknownElement(first))?;
            return Ok(());
        }
        let first_node = self
            .node_id(first)
            .ok_or(PapiError::UnknownElement(first))?;
        let second_node = self
            .node_id(second)
            .ok_or(PapiError::UnknownElement(second))?;
        if first == PAGE_UNIQUE_ID || second == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotReparentPage);
        }
        let first_parent = self
            .document
            .get(first_node)
            .and_then(dom::Node::parent_id)
            .ok_or(PapiError::NotAChild {
                parent: NO_ELEMENT,
                child: first,
            })?;
        let second_parent = self
            .document
            .get(second_node)
            .and_then(dom::Node::parent_id)
            .ok_or(PapiError::NotAChild {
                parent: NO_ELEMENT,
                child: second,
            })?;
        if self.document.is_ancestor(first_node, second_node)
            || self.document.is_ancestor(second_node, first_node)
        {
            return Err(PapiError::WouldCycle {
                parent: first,
                child: second,
            });
        }

        self.uncommitted = true;
        // Save *indices*, not successors. A successor cannot describe the
        // target position here, because for an adjacent pair one node's
        // successor is the other node, which is about to leave the tree — the
        // insert would then degrade to an append and land past every remaining
        // sibling. Indices survive both detaches, which is why the native
        // engine uses them (`renderer_functions.cc:3469-3485`).
        let first_index = self.child_index(first_parent, first_node);
        let second_index = self.child_index(second_parent, second_node);
        self.document.detach(first_node);
        self.document.detach(second_node);
        // `second` goes where `first` was, and vice versa. Lower index first,
        // so the earlier insert cannot shift the position the later one still
        // needs.
        let mut moves = [
            (first_parent, second_node, first_index),
            (second_parent, first_node, second_index),
        ];
        moves.sort_unstable_by_key(|(_, _, index)| *index);
        for (parent, node, index) in moves {
            let before = self
                .document
                .get(parent)
                .and_then(|parent| parent.child_ids().get(index).copied());
            self.document.insert_before(parent, node, before);
        }
        Ok(())
    }

    // --------------------------------------------------------------- queries

    /// `__GetParent(element)` — [`NO_ELEMENT`] when detached or when the
    /// parent is the document node rather than an element.
    #[must_use]
    pub fn parent_element(&self, id: ElementId) -> ElementId {
        self.relative(id, |document, node| {
            document.get(node).and_then(dom::Node::parent_id)
        })
    }

    /// `__FirstElement(element)` — `firstElementChild`, so a non-element child
    /// is skipped rather than ending the search. A `raw-text` element owns a
    /// mirror text node and can also be given element children, so the two
    /// answers really do differ.
    #[must_use]
    pub fn first_element(&self, id: ElementId) -> ElementId {
        self.relative(id, |document, node| {
            document
                .get(node)?
                .child_ids()
                .iter()
                .copied()
                .find(|child| is_element(document, *child))
        })
    }

    /// `__LastElement(element)` — `lastElementChild`.
    #[must_use]
    pub fn last_element(&self, id: ElementId) -> ElementId {
        self.relative(id, |document, node| {
            document
                .get(node)?
                .child_ids()
                .iter()
                .rev()
                .copied()
                .find(|child| is_element(document, *child))
        })
    }

    /// `__NextElement(element)` — `nextElementSibling`.
    #[must_use]
    pub fn next_element(&self, id: ElementId) -> ElementId {
        self.relative(id, |document, node| {
            let mut sibling = document.get(node)?.next_sibling();
            while let Some(node) = sibling {
                if node.is_element() {
                    return Some(node.id());
                }
                sibling = node.next_sibling();
            }
            None
        })
    }

    /// `__GetTag(element)` — the Lynx tag name.
    #[must_use]
    pub fn tag(&self, id: ElementId) -> Option<&str> {
        let node = self.node_id(id)?;
        self.document.get(node)?.tag_name()
    }

    /// `__GetElementUniqueID(element)`.
    ///
    /// Handles are already unique ids here, so this is a liveness probe.
    /// web-core and the native engine both answer `-1` rather than throwing
    /// for anything that is not an element (`pureElementPAPIs.ts:218-220`,
    /// `renderer_functions.cc:3953`), which is why it returns an `i64` and
    /// never a `Result`.
    #[must_use]
    pub fn unique_id(&self, id: ElementId) -> i64 {
        if self.element(id).is_some() {
            i64::from(id)
        } else {
            -1
        }
    }

    // ------------------------------------------------------------ attributes

    /// `__SetAttribute(element, key, value)`.
    ///
    /// A nullish value removes the attribute; everything else is written
    /// through ECMAScript string coercion, so `false` and `0` become
    /// `"false"` and `"0"` rather than removals.
    ///
    /// `text` on a `raw-text` element additionally rewrites the mirror text
    /// node — that attribute *is* the element's content.
    pub fn set_attribute(
        &mut self,
        id: ElementId,
        key: &str,
        value: &PapiValue,
    ) -> Result<(), PapiError> {
        if key == STYLE_ATTRIBUTE {
            // The `style` attribute *is* the inline-style block, so it has to
            // go through the same store `__AddInlineStyle` layers over —
            // otherwise the next `__AddInlineStyle` rebuilds the attribute
            // from a stale base and silently discards what was written here.
            // web-core has no such split: `__SetAttribute` is
            // `element.setAttribute('style', …)` and `__AddInlineStyle` is a
            // CSSOM `setProperty` on that same element, so the second merges
            // into the first.
            let text = (!value.is_nullish()).then(|| value.to_string());
            return self.set_inline_styles(id, text.as_deref());
        }
        let element = self.element(id).ok_or(PapiError::UnknownElement(id))?;
        let node = element.node_id();
        let mirror = element.text_mirror();
        self.uncommitted = true;
        if value.is_nullish() {
            self.document.remove_attribute(node, key);
        } else {
            self.document.set_attribute(node, key, &value.to_string());
        }
        if let Some(mirror) = mirror
            && key == RAW_TEXT_TEXT_ATTRIBUTE
        {
            let text = if value.is_nullish() {
                String::new()
            } else {
                value.to_string()
            };
            self.document.set_text_node_data(mirror, text);
        }
        Ok(())
    }

    /// `__GetAttributeByName(element, name)`.
    #[must_use]
    pub fn attribute(&self, id: ElementId, name: &str) -> Option<&str> {
        let node = self.node_id(id)?;
        self.document.get(node)?.attribute(name)
    }

    /// `__SetID(element, id)` — a nullish id clears it
    /// (`pureElementPAPIs.ts:140-141`).
    pub fn set_id(&mut self, id: ElementId, value: Option<&str>) -> Result<(), PapiError> {
        let node = self.node_id(id).ok_or(PapiError::UnknownElement(id))?;
        self.uncommitted = true;
        self.document.set_id_attribute(node, value);
        Ok(())
    }

    /// `__GetID(element)`.
    #[must_use]
    pub fn id_attribute(&self, id: ElementId) -> Option<&str> {
        let node = self.node_id(id)?;
        self.document.get(node)?.id_attribute()
    }

    /// `__AddClass(element, className)`.
    pub fn add_class(&mut self, id: ElementId, class: &str) -> Result<(), PapiError> {
        let node = self.node_id(id).ok_or(PapiError::UnknownElement(id))?;
        self.uncommitted = true;
        self.document.add_class(node, class);
        Ok(())
    }

    /// `__SetClasses(element, classNames)` — a nullish or empty argument
    /// removes the `class` attribute entirely (`pureElementPAPIs.ts:162-169`).
    ///
    /// The class list is ordered, not a set: `__SetClasses("c b a")` reads
    /// back as `["c", "b", "a"]`.
    pub fn set_classes(&mut self, id: ElementId, classes: Option<&str>) -> Result<(), PapiError> {
        let node = self.node_id(id).ok_or(PapiError::UnknownElement(id))?;
        self.uncommitted = true;
        match classes {
            Some(classes) if !classes.is_empty() => self.document.set_classes(node, classes),
            _ => self.document.remove_attribute(node, "class"),
        }
        Ok(())
    }

    /// The element's classes, in order — the Rust side of `__GetClasses`.
    pub fn classes(&self, id: ElementId) -> impl Iterator<Item = &str> {
        self.node_id(id)
            .and_then(|node| self.document.get(node))
            .map(dom::Node::classes)
            .into_iter()
            .flatten()
    }

    /// `__SetInlineStyles(element, value)` — replaces the whole `style`
    /// block, discarding any declaration a previous `__AddInlineStyle`
    /// layered on.
    ///
    /// web-core's `transform_vw`/`transform_vh`/`transform_rem` unit rewriting
    /// (`rpx`/`ppx`/`vw`/`vh` into `calc()` over CSS custom properties) is a
    /// web-target device-unit workaround; this engine resolves units in the
    /// cascade, so the text is passed through unchanged.
    pub fn set_inline_styles(
        &mut self,
        id: ElementId,
        value: Option<&str>,
    ) -> Result<(), PapiError> {
        let element = self
            .elements
            .get_mut(id)
            .ok_or(PapiError::UnknownElement(id))?;
        element.set_inline_base(value.unwrap_or_default());
        let css = element.inline_style();
        let node = element.node_id();
        self.uncommitted = true;
        self.commit_inline_style(node, &css);
        Ok(())
    }

    /// `__AddInlineStyle(element, key, value)` — merges one declaration into
    /// the block, or removes it when the value is nullish.
    ///
    /// Numeric keys are Lynx CSS property ids (`24` is `display`, `26`
    /// `height`, `51` `flex-shrink`). That table is native-engine vocabulary
    /// this crate does not carry, so a numeric key is a precise
    /// [`PapiError::NumericStyleKey`] rather than a silently dropped
    /// declaration.
    pub fn add_inline_style(
        &mut self,
        id: ElementId,
        property: &str,
        value: Option<&str>,
    ) -> Result<(), PapiError> {
        let element = self
            .elements
            .get_mut(id)
            .ok_or(PapiError::UnknownElement(id))?;
        element.set_inline_override(property, value);
        let css = element.inline_style();
        let node = element.node_id();
        self.uncommitted = true;
        self.commit_inline_style(node, &css);
        Ok(())
    }

    /// `__AddDataset(element, key, value)`.
    ///
    /// The value is stored on the runtime element *and* mirrored as a
    /// `data-<key>` attribute — but only when it is truthy, exactly as in
    /// web-core (`createElementAPI.ts:426-437`). A falsy value therefore
    /// stays readable through [`Self::data_by_key`] while the attribute is
    /// removed; the store, not the DOM, is the dataset.
    pub fn add_dataset(
        &mut self,
        id: ElementId,
        key: &str,
        value: PapiValue,
    ) -> Result<(), PapiError> {
        let element = self
            .elements
            .get_mut(id)
            .ok_or(PapiError::UnknownElement(id))?;
        let node = element.node_id();
        let attribute = format!("data-{key}");
        let mirrored = (!value.is_falsy()).then(|| value.to_string());
        element.set_data(key, value);
        self.uncommitted = true;
        match mirrored {
            Some(text) => self.document.set_attribute(node, &attribute, &text),
            None => self.document.remove_attribute(node, &attribute),
        }
        Ok(())
    }

    /// Clears the dataset — the first half of `__SetDataset`, which replaces
    /// the whole map rather than merging into it.
    pub fn clear_dataset(&mut self, id: ElementId) -> Result<(), PapiError> {
        let element = self
            .elements
            .get_mut(id)
            .ok_or(PapiError::UnknownElement(id))?;
        let keys: Vec<String> = element
            .dataset()
            .map(|(key, _)| format!("data-{key}"))
            .collect();
        let node = element.node_id();
        element.clear_dataset();
        self.uncommitted = true;
        for key in keys {
            self.document.remove_attribute(node, &key);
        }
        Ok(())
    }

    /// `__GetDataByKey(element, key)`.
    #[must_use]
    pub fn data_by_key(&self, id: ElementId, key: &str) -> Option<&PapiValue> {
        self.element(id)?.data_by_key(key)
    }

    // ------------------------------------------------------ component / scope

    /// `__SetCSSId(elements, cssId, entryName)`, for one element.
    ///
    /// **Recorded, not honored.** The id is the CSS fragment a scoped
    /// stylesheet would be matched under; this engine has no decoded
    /// `StyleInfo` ingestion yet, so there are no per-fragment rules to scope
    /// against. Storing it keeps the member's observable contract — later
    /// creations under this element inherit the id — and leaves one place for
    /// the scoping pass to read when it lands.
    pub fn set_css_id(
        &mut self,
        id: ElementId,
        css_id: i32,
        entry_name: Option<&str>,
    ) -> Result<(), PapiError> {
        let element = self
            .elements
            .get_mut(id)
            .ok_or(PapiError::UnknownElement(id))?;
        // Only the element's own scope. What later creations under it inherit
        // is its `component_css_id`, which only a component-creating member
        // writes — `set_css_id` does not touch it in web-core either
        // (`style_apis.rs:16-54` versus `main_thread_context.rs:88-99`).
        element.set_css_id(css_id);
        element.set_entry_name(entry_name);
        self.uncommitted = true;
        Ok(())
    }

    /// `__UpdateComponentID(element, componentID)`.
    pub fn update_component_id(
        &mut self,
        id: ElementId,
        component_id: &str,
    ) -> Result<(), PapiError> {
        self.elements
            .get_mut(id)
            .ok_or(PapiError::UnknownElement(id))?
            .set_component_id(component_id);
        self.uncommitted = true;
        Ok(())
    }

    /// `__GetComponentID(element)`.
    #[must_use]
    pub fn component_id(&self, id: ElementId) -> Option<&str> {
        self.element(id)?.component_id()
    }

    // ---------------------------------------------------------------- commit

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

    /// `__DropElement(element)` — destroys a subtree and retires every handle
    /// in it.
    ///
    /// This is the one member with no web-core counterpart, and the difference
    /// follows from the handle representation rather than from a missing
    /// feature. web-core hands the script real `HTMLElement`s, so it reclaims
    /// an element's engine-side storage from a `WeakRef` sweep once the script
    /// drops its last reference (`MainThreadWasmContext::gc`). A `u32` cannot
    /// be held weakly, so here reclamation is announced instead of inferred:
    /// the realm registers each new handle with a `FinalizationRegistry` and
    /// this is what the finalizer calls.
    ///
    /// It is the counterpart of [`Self::remove_element`], not a variant of it
    /// — that one detaches an element that is still referenced and still
    /// re-insertable.
    pub fn drop_element(&mut self, id: ElementId) -> Result<(), PapiError> {
        if id == PAGE_UNIQUE_ID {
            return Err(PapiError::CannotRemovePage);
        }
        let node = self.node_id(id).ok_or(PapiError::UnknownElement(id))?;
        self.uncommitted = true;
        let retired_ids = self.document.remove_subtree(node);
        for unique_id in retired_ids {
            // A `raw-text` mirror text node carries the null payload and has
            // no arena entry of its own.
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

    // --------------------------------------------------------------- internal

    /// Creates one element under a parent component, inheriting its CSS
    /// scope.
    ///
    /// `parent_component_unique_id` is *not* a parent: nothing is linked here.
    /// web-core uses it only to seed the new element's `l-css-id`, and
    /// deliberately tolerates a handle that names nothing — the lookup misses
    /// and the element is simply unscoped (`main_thread_context.rs:91-99`).
    /// Rejecting it would turn `ReactLynx`'s page-teardown race, where
    /// `__pageId` has been reset to `0`, into a hard failure.
    fn insert(&mut self, tag: &str, parent_component_unique_id: ElementId) -> ElementId {
        self.uncommitted = true;
        let css_id = self.elements.inherited_css_id(parent_component_unique_id);
        let unique_id = self.elements.reserve();
        let node = self.document.create_element(tag, unique_id);
        self.elements
            .insert(unique_id, node, parent_component_unique_id, css_id)
    }

    /// The shared validation and linking path behind `__AppendElement`,
    /// `__InsertElementBefore`, and `__ReplaceElement`.
    fn link(
        &mut self,
        parent: ElementId,
        child: ElementId,
        reference: Option<(ElementId, NodeId)>,
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
        if let Some((reference, reference_node)) = reference
            && self
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

        self.uncommitted = true;
        self.document
            .insert_before(parent_node, child_node, reference.map(|(_, node)| node));
        Ok(child)
    }

    /// A child's position in its parent's child list.
    fn child_index(&self, parent: NodeId, child: NodeId) -> usize {
        self.document
            .get(parent)
            .and_then(|parent| parent.child_ids().iter().position(|id| *id == child))
            .expect("a node's parent lists it among its children")
    }

    /// Resolves one tree-navigation query to a handle, answering
    /// [`NO_ELEMENT`] for "no such relative" and for any node that is not a
    /// live Lynx element — a `raw-text` mirror text node, or the document node
    /// above the page.
    fn relative(
        &self,
        id: ElementId,
        step: impl Fn(&Document<ElementId>, NodeId) -> Option<NodeId>,
    ) -> ElementId {
        let Some(node) = self.node_id(id) else {
            return NO_ELEMENT;
        };
        let Some(related) = step(&self.document, node) else {
            return NO_ELEMENT;
        };
        let Some(related) = self.document.get(related) else {
            return NO_ELEMENT;
        };
        if !related.is_element() {
            return NO_ELEMENT;
        }
        let candidate = *related.payload();
        if self.element(candidate).is_some() {
            candidate
        } else {
            NO_ELEMENT
        }
    }

    /// Writes the assembled declaration block, removing the attribute
    /// altogether when nothing is left — `dom`'s `set_inline_style("")` parses
    /// to an empty block, but web-core removes the attribute.
    fn commit_inline_style(&mut self, node: NodeId, css: &str) {
        if css.is_empty() {
            self.document.remove_attribute(node, "style");
        } else {
            self.document.set_inline_style(node, css);
        }
    }
}

#[cfg(test)]
mod tests {
    // Layout assertions read `rounded_layout`, whose values are already
    // rounded to whole device pixels, so exact float comparison is the
    // precise assertion here rather than a sloppy one.
    #![allow(clippy::float_cmp)]

    use dom::NodeId;
    use dom::stylo::values::computed::Display;

    use super::{Document, ElementTree, LynxElement, PAGE_UNIQUE_ID, PapiError, dom};
    use crate::device::Viewport;
    use crate::ua::PageConfig;
    use crate::value::PapiValue;
    use crate::{ElementId, NO_ELEMENT};

    /// The Ahem test face: every glyph is a solid `1em` square, so a string of
    /// `n` characters at `size` px measures exactly `n * size` by `size`. Taken
    /// from the same fixture `crates/dom/tests/layout.rs` measures text with.
    const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");

    fn tree() -> ElementTree {
        ElementTree::new(Viewport::new(393.0, 727.0), PageConfig::default())
    }

    /// The tags of `id`'s children, in document order.
    fn child_tags(tree: &ElementTree, id: ElementId) -> Vec<&str> {
        child_nodes(tree, id)
            .into_iter()
            .map(|node| {
                tree.document()
                    .get(node)
                    .expect("a live child node")
                    .tag_name()
                    .expect("an element child")
            })
            .collect()
    }

    /// `id`'s children as Lynx handles, in document order.
    fn child_ids(tree: &ElementTree, id: ElementId) -> Vec<ElementId> {
        child_nodes(tree, id)
            .into_iter()
            .map(|node| {
                *tree
                    .document()
                    .get(node)
                    .expect("a live child node")
                    .payload()
            })
            .collect()
    }

    fn child_nodes(tree: &ElementTree, id: ElementId) -> Vec<NodeId> {
        let node = tree.node_id(id).expect("a live element");
        tree.document()
            .get(node)
            .expect("a live node")
            .child_ids()
            .to_vec()
    }

    fn display_of(tree: &ElementTree, id: ElementId) -> Display {
        let node = tree.node_id(id).expect("a live element");
        tree.document()
            .get(node)
            .expect("a live node")
            .computed_style()
            .expect("a style committed by the last flush")
            .clone_display()
    }

    /// The element's laid-out box as `(x, y, width, height)`.
    fn rect(tree: &ElementTree, id: ElementId) -> (f32, f32, f32, f32) {
        node_rect(tree, tree.node_id(id).expect("a live element"))
    }

    fn node_rect(tree: &ElementTree, node: NodeId) -> (f32, f32, f32, f32) {
        let layout = tree
            .document()
            .rounded_layout(node)
            .expect("a node laid out by the last flush");
        (
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        )
    }

    fn string(text: &str) -> PapiValue {
        PapiValue::String(text.to_owned())
    }

    // ------------------------------------------------------------ page policy

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

    /// web-core memoizes the page and returns it from every later
    /// `__CreatePage`, ignoring the second call's arguments
    /// (`createElementAPI.ts:284`). The native engine instead constructs a
    /// fresh `PageElement` per call (`element_manager.cc:1298-1302`) — see
    /// divergence D6 in the native-suite report; web-core is the contract this
    /// runtime reproduces, so the *second* call must not rebind `componentID`
    /// or the CSS fragment id.
    #[test]
    fn create_page_is_idempotent_and_ignores_the_second_calls_arguments() {
        let mut tree = tree();
        let first = tree.create_page("page", 3);
        let second = tree.create_page("other", 7);
        assert_eq!(first, second);
        assert_eq!(tree.page(), Some(first));
        assert_eq!(tree.component_id(first), Some("page"));
        assert_eq!(tree.element(first).map(LynxElement::css_id), Some(3));
    }

    /// `componentID` is a string component *name*, and web-core files it in a
    /// side table (`create_element_common`) rather than in the DOM. It must not
    /// become selector-visible here either: the DOM core derives matching only
    /// from real DOM state, and an invented attribute would let author CSS from
    /// a bundle see something web-core never exposes.
    #[test]
    fn the_page_component_id_stays_out_of_the_dom() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        assert_eq!(tree.component_id(page), Some("card"));

        let node = tree.node_id(page).expect("a live page");
        assert_eq!(tree.document().get(node).unwrap().attributes().len(), 0);
    }

    #[test]
    fn update_component_id_rewrites_the_side_table_and_not_the_dom() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        let view = tree.create_view(page);
        tree.update_component_id(view, "comp-1").unwrap();
        assert_eq!(tree.component_id(view), Some("comp-1"));

        let node = tree.node_id(view).expect("a live view");
        assert_eq!(tree.document().get(node).unwrap().attributes().len(), 0);
    }

    #[test]
    fn a_flush_lays_the_page_out_to_the_viewport() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        tree.flush_element_tree();
        // The UA sheet sizes `page` to the viewport, so the flush produced
        // real geometry rather than a zero box.
        assert_eq!(rect(&tree, page), (0.0, 0.0, 393.0, 727.0));
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
    fn a_mutation_opens_the_commit_gate_and_a_flush_closes_it() {
        let mut tree = tree();
        assert!(!tree.has_uncommitted_mutations());
        let page = tree.create_page("card", 0);
        assert!(tree.has_uncommitted_mutations());
        tree.flush_element_tree();
        assert!(!tree.has_uncommitted_mutations());

        tree.add_class(page, "hero").unwrap();
        assert!(tree.has_uncommitted_mutations());
        tree.flush_element_tree();
        assert!(!tree.has_uncommitted_mutations());
    }

    // ------------------------------------------------------- arena / handles

    /// Ids are consecutive integers in creation order, starting at 1 —
    /// web-core's `unique_id_to_element_map.len()` over a `vec![None]`
    /// (`main_thread_context.rs:54,89`), pinned by `element-apis.spec.ts:699`
    /// (`ret0 + 1 === ret1`). The page holds id 1 because it is pre-created
    /// with the document.
    #[test]
    fn unique_ids_start_at_one_and_do_not_repeat() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let first = tree.create_view(NO_ELEMENT);
        let second = tree.create_view(NO_ELEMENT);
        assert_eq!(page, 1);
        assert_eq!(first, 2);
        assert_eq!(second, 3);
    }

    /// Monotonic, never-reused ids are an agreement between both reference
    /// engines: native `element_id_++` (`element_manager.cc:1733`) and
    /// web-core's `gc()`, which nulls a dead slot without shrinking the vector
    /// (`main_thread_context.rs:144-157`). Here the arena slot stays a
    /// permanent `None`, so no later element can answer to a retired handle.
    #[test]
    fn releasing_an_element_leaves_a_permanent_empty_arena_slot() {
        let mut tree = tree();
        let first = tree.create_view(NO_ELEMENT);
        let first_node = tree.node_id(first).unwrap();
        let first_unique_id = *tree.document().get(first_node).unwrap().payload();

        tree.drop_element(first).unwrap();
        assert!(tree.node_id(first).is_none());

        let second = tree.create_view(NO_ELEMENT);
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
        let parent = tree.create_view(NO_ELEMENT);
        let child = tree.create_view(NO_ELEMENT);
        tree.append_element(parent, child).unwrap();

        tree.drop_element(parent).unwrap();
        assert!(tree.node_id(parent).is_none());
        assert!(tree.node_id(child).is_none());
        assert_eq!(tree.elements.len(), 4);

        let next = tree.create_view(NO_ELEMENT);
        assert_eq!(next, 4);
    }

    /// `drop_element` is not a PAPI member; it is the disposal primitive a
    /// future sweep would use, and the page is not disposable because it is the
    /// permanent document element.
    #[test]
    fn drop_element_refuses_the_page_and_a_dead_handle() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        assert_eq!(
            tree.drop_element(page).unwrap_err(),
            PapiError::CannotRemovePage
        );
        let view = tree.create_view(NO_ELEMENT);
        tree.drop_element(view).unwrap();
        assert_eq!(
            tree.drop_element(view).unwrap_err(),
            PapiError::UnknownElement(view)
        );
    }

    /// The `raw-text` mirror is a DOM text node carrying the [`NO_ELEMENT`]
    /// payload, so the subtree sweep must skip it rather than look for an arena
    /// entry that was never created.
    #[test]
    fn dropping_a_raw_text_element_retires_only_its_arena_entry() {
        let mut tree = tree();
        let raw = tree.create_raw_text("hi");
        let arena_len = tree.elements.len();
        tree.drop_element(raw).unwrap();
        assert!(tree.element(raw).is_none());
        assert_eq!(tree.elements.len(), arena_len);
    }

    #[test]
    fn zero_is_the_no_element_sentinel() {
        let mut tree = tree();
        assert!(tree.element(NO_ELEMENT).is_none());
        assert_eq!(tree.parent_element(NO_ELEMENT), NO_ELEMENT);
        // It is a legal *argument*, meaning "no parent component".
        let view = tree.create_view(NO_ELEMENT);
        assert_eq!(tree.element(view).map(LynxElement::css_id), Some(0));
    }

    /// `__GetElementUniqueID` answers `-1` for anything that is not a live
    /// engine element rather than throwing, on both reference engines
    /// (`renderer_functions.cc:3953`, `pureElementPAPIs.ts:218-220`).
    #[test]
    fn unique_id_is_minus_one_for_a_handle_that_names_no_element() {
        let mut tree = tree();
        let view = tree.create_view(NO_ELEMENT);
        assert_eq!(tree.unique_id(view), i64::from(view));
        assert_eq!(tree.unique_id(NO_ELEMENT), -1);
        assert_eq!(tree.unique_id(9999), -1);
        tree.drop_element(view).unwrap();
        assert_eq!(tree.unique_id(view), -1);
    }

    /// `__ElementIsEqual` is reference identity on both reference engines
    /// (`renderer_functions.cc:3944`, `pureElementPAPIs.ts:49-52`). A `u32`
    /// handle can only reproduce that if one element has exactly one handle and
    /// no two live elements share one, which is what this asserts at the arena.
    #[test]
    fn one_element_has_exactly_one_handle() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let first = tree.create_view(NO_ELEMENT);
        let second = tree.create_view(NO_ELEMENT);
        let handles = [page, first, second];
        for handle in handles {
            assert_eq!(
                tree.element(handle).map(LynxElement::unique_id),
                Some(handle)
            );
        }
        assert_ne!(first, second);
        // Moving an element does not re-mint its handle.
        tree.append_element(page, first).unwrap();
        tree.append_element(second, first).unwrap();
        assert_eq!(tree.element(first).map(LynxElement::unique_id), Some(first));
    }

    #[test]
    fn the_document_payload_is_the_element_id() {
        fn assert_document_type(_: &Document<ElementId>) {}

        let mut tree = tree();
        assert_document_type(tree.document());
        let page = tree.create_page("page", 17);
        let view = tree.create_view(page);
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
        assert_eq!(tree.element(page).map(LynxElement::css_id), Some(17));
        assert_eq!(payload_unique_id, view);
    }

    // ------------------------------------------------------- create members

    /// Each dedicated creator's tag, as `__GetTag` reports it in web-core:
    /// `element-apis.spec.ts:479` (view), `:529` (text), `:534` (image),
    /// `:490` (scroll-view), `:545` (wrapper), `:539` (raw-text), `:484`
    /// (page). web-core stores mangled HTML tags (`x-view`, `div`, …) and
    /// reverse-maps them on read; there is no HTML here, so the Lynx tag is
    /// what the document stores.
    #[test]
    fn every_create_member_produces_its_lynx_tag() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let created = [
            (page, "page"),
            (tree.create_view(NO_ELEMENT), "view"),
            (tree.create_text(NO_ELEMENT), "text"),
            (tree.create_image(NO_ELEMENT), "image"),
            (tree.create_scroll_view(NO_ELEMENT), "scroll-view"),
            (tree.create_wrapper_element(NO_ELEMENT), "wrapper"),
            (tree.create_frame(NO_ELEMENT), "frame"),
            (tree.create_raw_text("hi"), "raw-text"),
        ];
        for (id, tag) in created {
            let node = tree.node_id(id).expect("a live element");
            assert_eq!(
                tree.document().get(node).unwrap().tag_name(),
                Some(tag),
                "handle {id}"
            );
            assert_eq!(tree.tag(id), Some(tag));
        }
    }

    /// `__CreateElement(tagName, …)` passes an unknown tag through unchanged
    /// (`createElementAPI.ts:227-237`), so the tag a bundle's author CSS
    /// selects on is whatever the bundle asked for.
    #[test]
    fn create_element_stores_an_arbitrary_tag_verbatim() {
        let mut tree = tree();
        let custom = tree.create_element("x-my-widget", NO_ELEMENT);
        assert_eq!(tree.tag(custom), Some("x-my-widget"));
        // `__CreateElement('view', …)` and `__CreateView(…)` agree here; in
        // web-core they only agree after the reverse tag map is applied.
        let view = tree.create_element("view", NO_ELEMENT);
        let dedicated = tree.create_view(NO_ELEMENT);
        assert_eq!(tree.tag(view), Some("view"));
        assert_eq!(tree.tag(dedicated), Some("view"));
    }

    // ------------------------------------------------------- css-id scoping

    /// web-core `element-apis.spec.ts:1486` — an element created with a
    /// component's unique id inherits that component's CSS fragment id, and
    /// inheritance is decided by the *creation argument*, not by where the
    /// element is later attached (the test appends the child to the page, not
    /// to the component).
    #[test]
    fn an_element_inherits_the_css_id_of_its_parent_component() {
        let mut tree = tree();
        let page = tree.create_page("page", 100);
        let text = tree.create_text(page);
        // Attached to the page, created under the page: inheritance already
        // happened at creation.
        tree.append_element(page, text).unwrap();
        assert_eq!(tree.element(text).map(LynxElement::css_id), Some(100));

        // A scope is not handed on transitively. web-core stores two fields
        // per element — `css_id`, the element's own scope, and
        // `component_css_id`, the scope its creations seed — and only
        // `__CreatePage`/`__CreateComponent` ever write the second
        // (`main_thread_context.rs:88-99`, `element_data.rs:26-27`). A plain
        // `text` is not a component, so creating under its handle seeds
        // nothing even though the text itself is scoped to 100.
        let inner = tree.create_view(text);
        assert_eq!(tree.element(inner).map(LynxElement::css_id), Some(0));
        assert_eq!(
            tree.element(text).map(LynxElement::component_css_id),
            Some(0)
        );
        // The page *is* a component, so its handle keeps seeding 100.
        let sibling = tree.create_view(page);
        assert_eq!(tree.element(sibling).map(LynxElement::css_id), Some(100));
    }

    /// web-core `element-apis.spec.ts:1512` — `0` is the "no CSS scope"
    /// sentinel and is never inherited; web-core observes it as the absence of
    /// the `l-css-id` attribute.
    #[test]
    fn css_id_zero_is_no_scope_and_is_not_inherited() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let text = tree.create_text(page);
        assert_eq!(tree.element(page).map(LynxElement::css_id), Some(0));
        assert_eq!(tree.element(text).map(LynxElement::css_id), Some(0));
    }

    /// A `parentComponentUniqueID` that resolves to nothing is *tolerated*, not
    /// rejected: web-core's `create_element_common` looks the handle up and
    /// falls back to `css_id = 0` on a miss
    /// (`main_thread_context.rs:91-99`). That tolerance is load-bearing —
    /// `__CreateRawText` deliberately passes `-1`
    /// (`createElementAPI.ts:210`), and `ReactLynx` passes a module-scope
    /// `__pageId` that its own teardown resets to `0`, so rejecting would turn
    /// a re-render race into a hard failure instead of an unscoped element.
    #[test]
    fn create_view_tolerates_an_unknown_parent_component() {
        let mut tree = tree();
        let ghost: ElementId = 9;
        assert!(tree.element(ghost).is_none());
        let view = tree.create_view(ghost);
        assert_eq!(tree.tag(view), Some("view"));
        assert_eq!(tree.element(view).map(LynxElement::css_id), Some(0));
        // The garbage handle is still recorded verbatim: it names no parent and
        // links nothing, so nothing else can go wrong with it.
        assert_eq!(
            tree.element(view)
                .map(LynxElement::parent_component_unique_id),
            Some(ghost)
        );
    }

    /// `__SetCSSId` sets the element's *own* scope and nothing else.
    ///
    /// web-core writes only `css_id` there (`style_apis.rs:16-54`); what later
    /// creations inherit is `component_css_id`, which that member never
    /// touches (`main_thread_context.rs:88-99`). The member that does change
    /// inheritance is `__UpdateComponentInfo`'s `cssID`, which this subset
    /// does not implement — so here nothing can change it after creation, and
    /// a CSS fragment id is read at creation time on both engines.
    #[test]
    fn set_css_id_does_not_change_what_later_creations_inherit() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let component = tree.create_view(page);
        let before = tree.create_view(component);
        tree.set_css_id(component, 42, Some("test_entry")).unwrap();
        let after = tree.create_view(component);

        assert_eq!(tree.element(component).map(LynxElement::css_id), Some(42));
        assert_eq!(tree.element(before).map(LynxElement::css_id), Some(0));
        assert_eq!(tree.element(after).map(LynxElement::css_id), Some(0));
        // `entryName` is recorded for the scoping pass that will need it.
        assert_eq!(
            tree.element(component).and_then(LynxElement::entry_name),
            Some("test_entry")
        );
    }

    /// A mutating member has to open the batch, or a frame producer sharing
    /// the tree could build between the mutation and the flush that commits
    /// it.
    #[test]
    fn set_css_id_and_update_component_id_open_a_batch() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        tree.flush_element_tree();
        assert!(!tree.has_uncommitted_mutations());
        tree.set_css_id(page, 7, None).unwrap();
        assert!(tree.has_uncommitted_mutations());

        tree.flush_element_tree();
        assert!(!tree.has_uncommitted_mutations());
        tree.update_component_id(page, "other").unwrap();
        assert!(tree.has_uncommitted_mutations());
    }

    #[test]
    fn set_css_id_and_update_component_id_reject_a_dead_handle() {
        let mut tree = tree();
        assert_eq!(
            tree.set_css_id(9, 1, None).unwrap_err(),
            PapiError::UnknownElement(9)
        );
        assert_eq!(
            tree.update_component_id(9, "c").unwrap_err(),
            PapiError::UnknownElement(9)
        );
    }

    // ------------------------------------------------------------- raw text

    /// web-core `element-apis.spec.ts:539` — `__CreateRawText(content)` makes a
    /// `raw-text` **element** whose content is readable as its `text`
    /// attribute. This engine paints from DOM text nodes, so the element also
    /// owns a mirror text node with the same content; the mirror is not
    /// separately addressable, which is why it carries the [`NO_ELEMENT`]
    /// payload.
    #[test]
    fn create_raw_text_carries_its_content_as_an_attribute_and_a_mirror_node() {
        let mut tree = tree();
        let raw = tree.create_raw_text("Text Element");
        assert_eq!(tree.tag(raw), Some("raw-text"));
        assert_eq!(tree.attribute(raw, "text"), Some("Text Element"));

        let node = tree.node_id(raw).expect("a live raw-text");
        let mirror = tree
            .element(raw)
            .and_then(LynxElement::text_mirror)
            .expect("a raw-text owns a mirror text node");
        assert_eq!(tree.document().get(node).unwrap().child_ids(), [mirror]);
        let mirror_node = tree.document().get(mirror).expect("a live mirror");
        assert!(mirror_node.is_text_node());
        assert_eq!(mirror_node.text(), Some("Text Element"));
        assert_eq!(*mirror_node.payload(), NO_ELEMENT);
    }

    /// web-core testing-library port `:151` — the framework rewrites raw-text
    /// content with `__SetAttribute(element, 'text', …)`, which is why the
    /// content lives in an attribute at all. Both stores must move together.
    #[test]
    fn set_attribute_text_rewrites_the_attribute_and_the_mirror() {
        let mut tree = tree();
        let raw = tree.create_raw_text("raw-text");
        let mirror = tree
            .element(raw)
            .and_then(LynxElement::text_mirror)
            .unwrap();
        tree.set_attribute(raw, "text", &string("Hello World"))
            .unwrap();

        assert_eq!(tree.attribute(raw, "text"), Some("Hello World"));
        assert_eq!(
            tree.document().get(mirror).unwrap().text(),
            Some("Hello World")
        );

        // Whitespace and case are preserved verbatim (testing-library port
        // `:191`, `:256`).
        tree.set_attribute(raw, "text", &string("  Step 1 of 4"))
            .unwrap();
        assert_eq!(
            tree.document().get(mirror).unwrap().text(),
            Some("  Step 1 of 4")
        );
    }

    /// A nullish value removes the attribute (`setElementPropertyOrAttribute.ts:10-13`);
    /// the mirror has no "absent" state, so it empties instead — a `raw-text`
    /// with no `text` attribute contributes no text.
    #[test]
    fn a_nullish_text_value_removes_the_attribute_and_empties_the_mirror() {
        let mut tree = tree();
        let raw = tree.create_raw_text("content");
        let mirror = tree
            .element(raw)
            .and_then(LynxElement::text_mirror)
            .unwrap();
        tree.set_attribute(raw, "text", &PapiValue::Null).unwrap();

        assert_eq!(tree.attribute(raw, "text"), None);
        assert_eq!(tree.document().get(mirror).unwrap().text(), Some(""));
    }

    /// Only the `text` attribute is mirrored, and only on a `raw-text`.
    #[test]
    fn another_attribute_on_a_raw_text_leaves_the_mirror_alone() {
        let mut tree = tree();
        let raw = tree.create_raw_text("content");
        let mirror = tree
            .element(raw)
            .and_then(LynxElement::text_mirror)
            .unwrap();
        tree.set_attribute(raw, "lang", &string("en")).unwrap();
        assert_eq!(tree.document().get(mirror).unwrap().text(), Some("content"));

        let view = tree.create_view(NO_ELEMENT);
        assert!(
            tree.element(view)
                .and_then(LynxElement::text_mirror)
                .is_none()
        );
        tree.set_attribute(view, "text", &string("ignored"))
            .unwrap();
        assert_eq!(tree.attribute(view, "text"), Some("ignored"));
    }

    /// The UA sheet's `raw-text { display: contents }` is what makes the mirror
    /// an item of the enclosing `<text>`'s formatting context rather than the
    /// content of a nested box: the `raw-text` element itself generates no box,
    /// while its mirror is measured with the inherited font.
    #[test]
    fn a_raw_text_generates_no_box_while_its_mirror_is_measured() {
        let mut tree = tree();
        assert_eq!(tree.register_fonts(AHEM), 1);
        tree.add_author_stylesheet(
            "page { font-family: Ahem; font-size: 16px; }\n\
             text { display: flex; align-items: flex-start; }",
        );
        let page = tree.create_page("page", 0);
        let text = tree.create_text(page);
        let raw = tree.create_raw_text("hi");
        tree.append_element(text, raw).unwrap();
        tree.append_element(page, text).unwrap();
        tree.flush_element_tree();

        assert_eq!(display_of(&tree, raw), Display::Contents);
        assert_eq!(rect(&tree, raw), (0.0, 0.0, 0.0, 0.0));

        // Two Ahem glyphs at the page's inherited 16px are exactly 32 x 16.
        let mirror = tree
            .element(raw)
            .and_then(LynxElement::text_mirror)
            .unwrap();
        assert_eq!(node_rect(&tree, mirror), (0.0, 0.0, 32.0, 16.0));

        // Rewriting the attribute re-measures the mirror.
        tree.set_attribute(raw, "text", &string("hello")).unwrap();
        tree.flush_element_tree();
        assert_eq!(node_rect(&tree, mirror), (0.0, 0.0, 80.0, 16.0));
    }

    /// The mirror is a text node, so it is not a Lynx element: the
    /// tree-navigation members must step over it rather than hand a script a
    /// handle nothing can name.
    #[test]
    fn tree_navigation_never_returns_the_raw_text_mirror() {
        let mut tree = tree();
        let raw = tree.create_raw_text("hi");
        assert_eq!(tree.first_element(raw), NO_ELEMENT);
        assert_eq!(tree.last_element(raw), NO_ELEMENT);
    }

    // ------------------------------------------------------------ structure

    #[test]
    fn append_element_returns_the_child_and_links_it() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let view = tree.create_view(NO_ELEMENT);
        assert_eq!(tree.append_element(page, view).unwrap(), view);
        assert_eq!(child_ids(&tree, page), [view]);
        assert_eq!(tree.parent_element(view), page);
    }

    #[test]
    fn append_element_reparents_rather_than_duplicating() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let first = tree.create_view(NO_ELEMENT);
        let second = tree.create_view(NO_ELEMENT);
        let moved = tree.create_view(NO_ELEMENT);
        tree.append_element(page, first).unwrap();
        tree.append_element(page, second).unwrap();
        tree.append_element(first, moved).unwrap();
        tree.append_element(second, moved).unwrap();

        assert!(child_ids(&tree, first).is_empty());
        assert_eq!(child_ids(&tree, second), [moved]);
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
        let outer = tree.create_view(NO_ELEMENT);
        let inner = tree.create_view(NO_ELEMENT);
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
        let view = tree.create_view(NO_ELEMENT);
        tree.append_element(page, view).unwrap();
        assert_eq!(
            tree.append_element(view, page).unwrap_err(),
            PapiError::CannotReparentPage
        );
    }

    /// web-core `element-apis.spec.ts:569` — three inserts, each before the
    /// previously inserted child, end in reverse insertion order. A nullish
    /// reference appends (`insertBefore(child, null)`,
    /// `pureElementPAPIs.ts:67-71`).
    #[test]
    fn insert_element_before_orders_children_by_their_reference() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let first = tree.create_view(NO_ELEMENT);
        let second = tree.create_image(NO_ELEMENT);
        let third = tree.create_text(NO_ELEMENT);

        assert_eq!(
            tree.insert_element_before(page, first, NO_ELEMENT).unwrap(),
            first
        );
        tree.insert_element_before(page, second, first).unwrap();
        tree.insert_element_before(page, third, second).unwrap();

        assert_eq!(child_ids(&tree, page), [third, second, first]);
        assert_eq!(child_tags(&tree, page), ["text", "image", "view"]);
    }

    /// `parent.insertBefore(child, child)` is a legal DOM no-op that a diffing
    /// framework does emit, and web-core passes it straight through. The DOM
    /// core here debug-asserts against it
    /// (`crates/dom/src/tree/document.rs:512`), so this layer must absorb it.
    #[test]
    fn insert_element_before_itself_keeps_the_child_where_it_is() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let first = tree.create_view(NO_ELEMENT);
        let second = tree.create_view(NO_ELEMENT);
        tree.append_element(page, first).unwrap();
        tree.append_element(page, second).unwrap();

        assert_eq!(
            tree.insert_element_before(page, first, first).unwrap(),
            first
        );
        assert_eq!(child_ids(&tree, page), [first, second]);
    }

    /// The self-reference is a no-op only when the child really is a child of
    /// that parent. The DOM checks pre-insertion validity *before* it rewrites
    /// the reference to the node's next sibling, so
    /// `parent.insertBefore(child, child)` on a child of a different parent
    /// throws `NotFoundError`; web-core hands the call straight to
    /// `insertBefore` (`pureElementPAPIs.ts:67-71`), so it throws there too.
    ///
    /// **Currently failing, and the implementation is what is wrong** —
    /// [`ElementTree::insert_element_before`]'s `reference == child` branch
    /// returns `Ok(child)` after merely resolving both handles, without
    /// checking the child's parent, so an insertion of a detached element is
    /// silently dropped instead of reported. The other reference paths all go
    /// through `link`, which does check.
    #[test]
    fn insert_element_before_itself_rejects_a_child_of_a_different_parent() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let detached = tree.create_view(NO_ELEMENT);
        assert_eq!(
            tree.insert_element_before(page, detached, detached)
                .unwrap_err(),
            PapiError::NotAChild {
                parent: page,
                child: detached,
            }
        );
        assert!(child_ids(&tree, page).is_empty());
    }

    /// A reference that is not a child of `parent` reaches
    /// `Document::insert_before`'s `.expect("insert_before reference must be a
    /// child of parent")` (`crates/dom/src/tree/document.rs:524`). The
    /// main-thread script is untrusted input, so this layer answers
    /// [`PapiError::NotAChild`] — naming the *reference* as the offending
    /// child — instead of letting the core panic.
    #[test]
    fn insert_element_before_a_foreign_reference_is_not_a_child_rather_than_a_panic() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let container = tree.create_view(NO_ELEMENT);
        let stranger = tree.create_view(NO_ELEMENT);
        let child = tree.create_view(NO_ELEMENT);
        tree.append_element(page, container).unwrap();
        tree.append_element(page, stranger).unwrap();

        assert_eq!(
            tree.insert_element_before(container, child, stranger)
                .unwrap_err(),
            PapiError::NotAChild {
                parent: container,
                child: stranger,
            }
        );
        // The rejected call moved nothing.
        assert!(child_ids(&tree, container).is_empty());
        assert_eq!(tree.parent_element(child), NO_ELEMENT);
    }

    #[test]
    fn insert_element_before_rejects_an_unknown_reference() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let child = tree.create_view(NO_ELEMENT);
        assert_eq!(
            tree.insert_element_before(page, child, 77).unwrap_err(),
            PapiError::UnknownElement(77)
        );
    }

    /// web-core `element-apis.spec.ts:559` and the testing-library port `:197`
    /// — removal is `parent.removeChild(child)` and nothing more
    /// (`pureElementPAPIs.ts:81-84`). The child stays fully alive, which is the
    /// contract `ReactLynx`'s reconciler relies on when it reorders children.
    #[test]
    fn remove_element_detaches_and_leaves_the_child_re_insertable() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let first = tree.create_view(NO_ELEMENT);
        let second = tree.create_view(NO_ELEMENT);
        tree.append_element(page, first).unwrap();
        tree.append_element(page, second).unwrap();
        tree.set_id(first, Some("child-0")).unwrap();
        tree.add_class(first, "hero").unwrap();

        assert_eq!(tree.remove_element(page, first).unwrap(), first);
        assert_eq!(child_ids(&tree, page), [second]);

        // Still live, still carrying its state, still addressable.
        assert_eq!(tree.unique_id(first), i64::from(first));
        assert_eq!(tree.id_attribute(first), Some("child-0"));
        assert_eq!(tree.classes(first).collect::<Vec<_>>(), ["hero"]);
        assert_eq!(tree.parent_element(first), NO_ELEMENT);

        tree.append_element(page, first).unwrap();
        assert_eq!(child_ids(&tree, page), [second, first]);
    }

    #[test]
    fn remove_element_rejects_a_child_of_another_parent_and_the_page() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let container = tree.create_view(NO_ELEMENT);
        let stranger = tree.create_view(NO_ELEMENT);
        tree.append_element(page, container).unwrap();
        tree.append_element(page, stranger).unwrap();

        assert_eq!(
            tree.remove_element(container, stranger).unwrap_err(),
            PapiError::NotAChild {
                parent: container,
                child: stranger,
            }
        );
        assert_eq!(
            tree.remove_element(page, page).unwrap_err(),
            PapiError::CannotRemovePage
        );
        assert_eq!(child_ids(&tree, page), [container, stranger]);
    }

    /// web-core `element-apis.spec.ts:625` — the argument order is **new
    /// element first**, because the implementation is
    /// `oldElement.replaceWith(newElement)` (`pureElementPAPIs.ts:86-89`) and
    /// therefore reads the parent off the old element.
    #[test]
    fn replace_element_takes_the_new_element_first() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let first = tree.create_view(NO_ELEMENT);
        let second = tree.create_image(NO_ELEMENT);
        let third = tree.create_text(NO_ELEMENT);
        let replacement = tree.create_scroll_view(NO_ELEMENT);
        for child in [first, second, third] {
            tree.append_element(page, child).unwrap();
        }

        assert_eq!(
            tree.replace_element(replacement, second).unwrap(),
            replacement
        );
        assert_eq!(child_ids(&tree, page), [first, replacement, third]);
        assert_eq!(child_tags(&tree, page), ["view", "scroll-view", "text"]);
        // The replaced element is detached, not destroyed.
        assert_eq!(tree.unique_id(second), i64::from(second));
        assert_eq!(tree.parent_element(second), NO_ELEMENT);
    }

    #[test]
    fn replace_element_by_itself_leaves_the_child_list_alone() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let view = tree.create_view(NO_ELEMENT);
        tree.append_element(page, view).unwrap();
        assert_eq!(tree.replace_element(view, view).unwrap(), view);
        assert_eq!(child_ids(&tree, page), [view]);
    }

    /// web-core's `replaceWith` on a parentless node is a silent no-op that
    /// discards the new element. Reporting it is the better failure, and the
    /// parent is reported as [`NO_ELEMENT`] because there is none to name.
    #[test]
    fn replace_element_on_a_detached_old_element_is_not_a_child() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let detached = tree.create_view(NO_ELEMENT);
        let replacement = tree.create_view(NO_ELEMENT);
        assert_eq!(
            tree.replace_element(replacement, detached).unwrap_err(),
            PapiError::NotAChild {
                parent: NO_ELEMENT,
                child: detached,
            }
        );
        assert_eq!(
            tree.replace_element(replacement, page).unwrap_err(),
            PapiError::CannotRemovePage
        );
    }

    /// web-core `element-apis.spec.ts:642` — swapping two non-adjacent
    /// children exchanges their positions and leaves everything else in place.
    #[test]
    fn swap_element_exchanges_two_positions() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let first = tree.create_view(NO_ELEMENT);
        let second = tree.create_image(NO_ELEMENT);
        let third = tree.create_text(NO_ELEMENT);
        for child in [first, second, third] {
            tree.append_element(page, child).unwrap();
        }

        tree.swap_element(first, third).unwrap();
        assert_eq!(child_ids(&tree, page), [third, second, first]);
        assert_eq!(child_tags(&tree, page), ["text", "image", "view"]);

        // The same call again restores the original order — the arguments are
        // unchanged, but `first` now sits after `third`, so this exercises the
        // (later, earlier) positional case as well.
        tree.swap_element(first, third).unwrap();
        assert_eq!(child_ids(&tree, page), [first, second, third]);
    }

    /// web-core `element-apis.spec.ts:642` — the *adjacent-sibling* swap, in
    /// both argument orders. It is the case both reference engines go out of
    /// their way to get right: web-core inserts a scratch placeholder so the
    /// second element has a position to be put back at
    /// (`pureElementPAPIs.ts:152-160`), and the native engine saves both
    /// indices before removing and re-inserts smaller-index-first
    /// (`renderer_functions.cc:3469-3485`). Both end in `[b, a, c]`.
    ///
    /// **Currently failing, and the implementation is what is wrong** —
    /// [`ElementTree::swap_element`] saves each node's *successor* rather than
    /// its index, and for an adjacent pair the first element's successor is the
    /// other swapped node. Once both are detached that successor is no longer a
    /// child, so the re-insert degrades to an append and lands past every
    /// remaining sibling instead of at the saved position: `[a, b, c]` comes
    /// back as `[a, c, b]`. Only an adjacent pair with a following sibling is
    /// affected; a non-adjacent pair, and an adjacent pair at the end of the
    /// child list, both land correctly.
    #[test]
    fn swap_element_of_adjacent_siblings_lands_the_same_way_round_in_both_orders() {
        for reversed in [false, true] {
            let mut tree = tree();
            let page = tree.create_page("page", 0);
            let first = tree.create_view(NO_ELEMENT);
            let second = tree.create_image(NO_ELEMENT);
            let third = tree.create_text(NO_ELEMENT);
            for child in [first, second, third] {
                tree.append_element(page, child).unwrap();
            }

            if reversed {
                tree.swap_element(second, first).unwrap();
            } else {
                tree.swap_element(first, second).unwrap();
            }
            assert_eq!(
                child_ids(&tree, page),
                [second, first, third],
                "reversed={reversed}"
            );
            assert_eq!(
                child_tags(&tree, page),
                ["image", "view", "text"],
                "reversed={reversed}"
            );
        }
    }

    #[test]
    fn swap_element_moves_children_between_two_parents() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let left = tree.create_view(NO_ELEMENT);
        let right = tree.create_view(NO_ELEMENT);
        let in_left = tree.create_image(NO_ELEMENT);
        let in_right = tree.create_text(NO_ELEMENT);
        tree.append_element(page, left).unwrap();
        tree.append_element(page, right).unwrap();
        tree.append_element(left, in_left).unwrap();
        tree.append_element(right, in_right).unwrap();

        tree.swap_element(in_left, in_right).unwrap();
        assert_eq!(child_ids(&tree, left), [in_right]);
        assert_eq!(child_ids(&tree, right), [in_left]);
    }

    #[test]
    fn swap_element_refuses_the_page_a_detached_node_and_an_ancestor_pair() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let outer = tree.create_view(NO_ELEMENT);
        let inner = tree.create_view(NO_ELEMENT);
        let detached = tree.create_view(NO_ELEMENT);
        tree.append_element(page, outer).unwrap();
        tree.append_element(outer, inner).unwrap();

        assert_eq!(
            tree.swap_element(page, outer).unwrap_err(),
            PapiError::CannotReparentPage
        );
        assert_eq!(
            tree.swap_element(outer, detached).unwrap_err(),
            PapiError::NotAChild {
                parent: NO_ELEMENT,
                child: detached,
            }
        );
        assert_eq!(
            tree.swap_element(outer, inner).unwrap_err(),
            PapiError::WouldCycle {
                parent: outer,
                child: inner,
            }
        );
        // Swapping an element with itself is a no-op, not an error.
        tree.swap_element(outer, outer).unwrap();
        assert_eq!(child_ids(&tree, page), [outer]);
        assert_eq!(
            tree.swap_element(77, 78).unwrap_err(),
            PapiError::UnknownElement(77)
        );
    }

    // --------------------------------------------------------- tree queries

    #[test]
    fn the_navigation_members_answer_no_element_for_an_absent_relative() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        assert_eq!(tree.first_element(page), NO_ELEMENT);
        assert_eq!(tree.last_element(page), NO_ELEMENT);
        assert_eq!(tree.next_element(page), NO_ELEMENT);
        // The page's parent is the document node, which is not an element.
        assert_eq!(tree.parent_element(page), NO_ELEMENT);

        let first = tree.create_view(NO_ELEMENT);
        let second = tree.create_image(NO_ELEMENT);
        tree.append_element(page, first).unwrap();
        tree.append_element(page, second).unwrap();

        assert_eq!(tree.first_element(page), first);
        assert_eq!(tree.last_element(page), second);
        assert_eq!(tree.next_element(first), second);
        assert_eq!(tree.next_element(second), NO_ELEMENT);
        assert_eq!(tree.parent_element(first), page);
    }

    // ----------------------------------------------------------- attributes

    /// `__SetAttribute` writes ECMAScript `String(value)` and removes only for
    /// `null`/`undefined`, so `false` and `0` are written as `"false"` and
    /// `"0"` (`setElementPropertyOrAttribute.ts:10-24`).
    #[test]
    fn set_attribute_writes_string_coercions_and_only_nullish_removes() {
        let mut tree = tree();
        let view = tree.create_view(NO_ELEMENT);
        tree.set_attribute(view, "scroll-x", &PapiValue::Boolean(true))
            .unwrap();
        assert_eq!(tree.attribute(view, "scroll-x"), Some("true"));

        tree.set_attribute(view, "scroll-x", &PapiValue::Boolean(false))
            .unwrap();
        assert_eq!(tree.attribute(view, "scroll-x"), Some("false"));

        tree.set_attribute(view, "count", &PapiValue::Number(0.0))
            .unwrap();
        assert_eq!(tree.attribute(view, "count"), Some("0"));

        tree.set_attribute(view, "count", &PapiValue::Undefined)
            .unwrap();
        assert_eq!(tree.attribute(view, "count"), None);
        assert_eq!(
            tree.set_attribute(9, "a", &PapiValue::Null).unwrap_err(),
            PapiError::UnknownElement(9)
        );
    }

    /// web-core `element-apis.spec.ts:707` probes the written attribute with
    /// `rootDom.querySelector('[test="test-value"]')`; the equivalent here is
    /// that an attribute selector in author CSS matches after a flush.
    #[test]
    fn an_attribute_reaches_selector_matching_after_a_flush() {
        let mut tree = tree();
        tree.add_author_stylesheet("[test=\"test-value\"] { display: flex; }");
        let page = tree.create_page("page", 0);
        let view = tree.create_view(NO_ELEMENT);
        tree.append_element(page, view).unwrap();
        tree.flush_element_tree();
        assert_eq!(display_of(&tree, view), Display::Linear);

        tree.set_attribute(view, "test", &string("test-value"))
            .unwrap();
        tree.flush_element_tree();
        assert_eq!(display_of(&tree, view), Display::Flex);
    }

    /// web-core `element-apis.spec.ts:516` — a nullish id clears it
    /// (`pureElementPAPIs.ts:140-141`), observed there through `#target`
    /// stopping to match.
    #[test]
    fn the_id_attribute_reaches_selector_matching_and_a_nullish_id_clears_it() {
        let mut tree = tree();
        tree.add_author_stylesheet("#target { display: flex; }");
        let page = tree.create_page("page", 0);
        let view = tree.create_view(NO_ELEMENT);
        tree.append_element(page, view).unwrap();
        tree.set_id(view, Some("target")).unwrap();
        tree.flush_element_tree();
        assert_eq!(tree.id_attribute(view), Some("target"));
        assert_eq!(display_of(&tree, view), Display::Flex);

        tree.set_id(view, None).unwrap();
        tree.flush_element_tree();
        assert_eq!(tree.id_attribute(view), None);
        assert_eq!(display_of(&tree, view), Display::Linear);
    }

    /// web-core `element-apis.spec.ts:744` — the class list is ordered, not a
    /// set: `__AddClass` three times reads back `['a','b','c']`, and
    /// `__SetClasses('c b a')` reads back `['c','b','a']`.
    #[test]
    fn classes_keep_their_order_and_reach_selector_matching() {
        let mut tree = tree();
        tree.add_author_stylesheet(".b { display: flex; }");
        let page = tree.create_page("page", 0);
        let view = tree.create_view(NO_ELEMENT);
        tree.append_element(page, view).unwrap();
        for class in ["a", "b", "c"] {
            tree.add_class(view, class).unwrap();
        }
        tree.flush_element_tree();
        assert_eq!(tree.classes(view).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(display_of(&tree, view), Display::Flex);

        // `__AddClass` is `classList.add` (`pureElementPAPIs.ts:171-176`), a set
        // operation: re-adding a class neither duplicates it nor moves it.
        tree.add_class(view, "b").unwrap();
        assert_eq!(tree.classes(view).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(tree.attribute(view, "class"), Some("a b c"));

        tree.set_classes(view, Some("c b a")).unwrap();
        tree.flush_element_tree();
        assert_eq!(tree.classes(view).collect::<Vec<_>>(), ["c", "b", "a"]);
        assert_eq!(display_of(&tree, view), Display::Flex);
    }

    /// `__SetClasses(el, null)` and `__SetClasses(el, '')` both remove the
    /// `class` attribute entirely (`pureElementPAPIs.ts:162-169`).
    #[test]
    fn set_classes_with_a_nullish_or_empty_value_removes_the_attribute() {
        let mut tree = tree();
        let view = tree.create_view(NO_ELEMENT);
        for empty in [None, Some("")] {
            tree.set_classes(view, Some("a b")).unwrap();
            assert_eq!(tree.attribute(view, "class"), Some("a b"));
            tree.set_classes(view, empty).unwrap();
            assert_eq!(tree.attribute(view, "class"), None);
            assert_eq!(tree.classes(view).count(), 0);
        }
    }

    // -------------------------------------------------------- inline styles

    /// `__SetInlineStyles` writes the whole `style` attribute, so it discards
    /// every declaration a previous `__AddInlineStyle` layered on; the two
    /// members are a replace and a merge, not two merges.
    #[test]
    fn set_inline_styles_replaces_the_whole_block() {
        let mut tree = tree();
        let view = tree.create_view(NO_ELEMENT);
        tree.add_inline_style(view, "height", Some("80px")).unwrap();
        assert_eq!(tree.attribute(view, "style"), Some("height:80px;"));

        tree.set_inline_styles(view, Some("width: 40px")).unwrap();
        assert_eq!(tree.attribute(view, "style"), Some("width: 40px"));

        // An absent block removes the attribute rather than leaving an empty
        // one behind.
        tree.set_inline_styles(view, None).unwrap();
        assert_eq!(tree.attribute(view, "style"), None);
    }

    #[test]
    fn add_inline_style_merges_and_later_declarations_win() {
        let mut tree = tree();
        let view = tree.create_view(NO_ELEMENT);
        tree.set_inline_styles(view, Some("width: 40px;")).unwrap();
        tree.add_inline_style(view, "height", Some("10px")).unwrap();
        tree.add_inline_style(view, "height", Some("20px")).unwrap();
        assert_eq!(
            tree.attribute(view, "style"),
            Some("width: 40px;height:20px;")
        );

        // A `None` value removes exactly that declaration and keeps the block.
        tree.add_inline_style(view, "height", None).unwrap();
        assert_eq!(tree.attribute(view, "style"), Some("width: 40px;"));
    }

    /// `__SetAttribute(element, "style", …)` and `__AddInlineStyle` write the
    /// same declaration block in web-core, and the second **merges into** the
    /// first: `__SetAttribute` is `element.setAttribute('style', String(value))`
    /// (`setElementPropertyOrAttribute.ts:22`) and `__AddInlineStyle`'s
    /// string-key path is a CSSOM `style.setProperty` on the element that
    /// attribute already populated (`style_apis.rs:82-101`). The native engine
    /// merges too — both land in the same `StyleMap`.
    ///
    /// **Currently failing, and the implementation is what is wrong** —
    /// [`ElementTree::set_attribute`] routes the `style` key straight into the
    /// DOM core without recording it as the arena element's inline base, so the
    /// next [`ElementTree::add_inline_style`] rebuilds the whole attribute from
    /// an empty base and silently discards every declaration the attribute
    /// write put there. [`ElementTree::set_inline_styles`] has no such hole: it
    /// writes the base it later rebuilds from.
    #[test]
    fn add_inline_style_merges_into_a_style_attribute_written_by_set_attribute() {
        let mut tree = tree();
        let view = tree.create_view(NO_ELEMENT);
        tree.set_attribute(view, "style", &string("color:red;"))
            .unwrap();
        assert_eq!(tree.attribute(view, "style"), Some("color:red;"));

        tree.add_inline_style(view, "height", Some("80px")).unwrap();
        let style = tree.attribute(view, "style").expect("a style attribute");
        assert!(
            style.contains("color:red"),
            "the attribute write must survive the merge, got {style:?}"
        );
        assert!(
            style.contains("height:80px"),
            "the merged declaration must be there, got {style:?}"
        );
    }

    #[test]
    fn inline_styles_reach_computed_style_and_layout() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let view = tree.create_view(NO_ELEMENT);
        tree.append_element(page, view).unwrap();
        tree.set_inline_styles(view, Some("width: 40px; height: 20px;"))
            .unwrap();
        tree.flush_element_tree();
        assert_eq!(rect(&tree, view), (0.0, 0.0, 40.0, 20.0));

        // The merge wins over the block it is layered on.
        tree.add_inline_style(view, "height", Some("30px")).unwrap();
        tree.flush_element_tree();
        assert_eq!(rect(&tree, view), (0.0, 0.0, 40.0, 30.0));

        // Replacing the block discards the merged declaration, so `height`
        // falls back to the linear container's own sizing.
        tree.set_inline_styles(view, Some("width: 40px;")).unwrap();
        tree.flush_element_tree();
        assert_eq!(rect(&tree, view), (0.0, 0.0, 40.0, 0.0));
    }

    /// web-core `element-apis.spec.ts:884` — `__SetAttribute(el, "style", …)`
    /// is the second entry point into the same declaration block, and on the
    /// client it writes the text through untransformed
    /// (`setElementPropertyOrAttribute.ts:22`), which is what this engine does
    /// for every entry point. What remains to pin is that the block reaches the
    /// cascade rather than sitting in an inert attribute — in the browser that
    /// is automatic, here it is [`Document::set_attribute`]'s `style` case.
    #[test]
    fn a_style_attribute_written_by_set_attribute_reaches_layout() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let view = tree.create_view(NO_ELEMENT);
        tree.append_element(page, view).unwrap();
        tree.set_attribute(view, "style", &string("width: 40px; height: 20px;"))
            .unwrap();
        tree.flush_element_tree();

        assert_eq!(
            tree.attribute(view, "style"),
            Some("width: 40px; height: 20px;")
        );
        assert_eq!(rect(&tree, view), (0.0, 0.0, 40.0, 20.0));
    }

    #[test]
    fn the_inline_style_members_reject_a_dead_handle() {
        let mut tree = tree();
        assert_eq!(
            tree.set_inline_styles(9, Some("width: 1px")).unwrap_err(),
            PapiError::UnknownElement(9)
        );
        assert_eq!(
            tree.add_inline_style(9, "width", Some("1px")).unwrap_err(),
            PapiError::UnknownElement(9)
        );
    }

    // -------------------------------------------------------------- dataset

    /// `__AddDataset` writes twice: the engine's dataset store always, and the
    /// mirrored `data-<key>` attribute only when the value is **truthy**
    /// (`createElementAPI.ts:426-437`). So `0`, `''` and `false` stay readable
    /// through `__GetDataByKey` while the attribute is removed — the store, not
    /// the DOM, is the dataset.
    #[test]
    fn a_falsy_dataset_value_stays_in_the_store_while_the_attribute_is_removed() {
        let mut tree = tree();
        let view = tree.create_view(NO_ELEMENT);
        tree.add_dataset(view, "testid", string("view-element"))
            .unwrap();
        assert_eq!(
            tree.data_by_key(view, "testid"),
            Some(&string("view-element"))
        );
        assert_eq!(tree.attribute(view, "data-testid"), Some("view-element"));

        for falsy in [
            PapiValue::Number(0.0),
            PapiValue::String(String::new()),
            PapiValue::Boolean(false),
        ] {
            tree.add_dataset(view, "testid", falsy.clone()).unwrap();
            assert_eq!(tree.data_by_key(view, "testid"), Some(&falsy));
            assert_eq!(tree.attribute(view, "data-testid"), None);
        }
    }

    /// The `data-` prefix is applied verbatim, with no camelCase-to-kebab
    /// mangling — unlike the HTML `dataset` API, and matching web-core.
    #[test]
    fn a_dataset_key_is_prefixed_verbatim_and_keeps_its_insertion_order() {
        let mut tree = tree();
        let view = tree.create_view(NO_ELEMENT);
        tree.add_dataset(view, "testId", string("a")).unwrap();
        tree.add_dataset(view, "other", string("b")).unwrap();
        tree.add_dataset(view, "testId", string("c")).unwrap();
        assert_eq!(tree.attribute(view, "data-testId"), Some("c"));
        assert_eq!(
            tree.element(view)
                .expect("a live view")
                .dataset()
                .map(|(key, _)| key)
                .collect::<Vec<_>>(),
            ["testId", "other"]
        );
    }

    /// `__SetDataset` replaces the whole map rather than merging into it, which
    /// this layer spells as [`ElementTree::clear_dataset`] followed by
    /// `__AddDataset` per entry. The clear must take the mirrored attributes
    /// with it.
    #[test]
    fn clearing_the_dataset_removes_every_mirrored_attribute() {
        let mut tree = tree();
        let view = tree.create_view(NO_ELEMENT);
        tree.add_dataset(view, "one", string("1")).unwrap();
        tree.add_dataset(view, "two", string("2")).unwrap();
        tree.clear_dataset(view).unwrap();

        assert_eq!(tree.data_by_key(view, "one"), None);
        assert_eq!(tree.attribute(view, "data-one"), None);
        assert_eq!(tree.attribute(view, "data-two"), None);
        assert_eq!(tree.element(view).expect("a live view").dataset().len(), 0);
        assert_eq!(
            tree.clear_dataset(9).unwrap_err(),
            PapiError::UnknownElement(9)
        );
    }

    // ------------------------------------------------------------- UA sheet

    #[test]
    fn the_ua_sheet_gives_every_element_lynx_defaults() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let view = tree.create_view(NO_ELEMENT);
        // The UA selector is `*`, not a tag list: `__CreateElement` accepts an
        // arbitrary tag, so a list could never be complete.
        let custom = tree.create_element("x-my-widget", NO_ELEMENT);
        tree.append_element(page, view).unwrap();
        tree.append_element(page, custom).unwrap();
        tree.flush_element_tree();

        for id in [view, custom] {
            let node = tree.node_id(id).unwrap();
            let style = tree.document().get(node).unwrap().computed_style().unwrap();
            assert_eq!(
                style.clone_box_sizing(),
                dom::stylo::computed_values::box_sizing::T::BorderBox
            );
            assert_eq!(style.clone_display(), Display::Linear);
        }
    }

    /// `raw-text` is the one tag the UA sheet singles out: it generates no box
    /// in Lynx either, and `display: contents` is the W3C spelling of that.
    #[test]
    fn the_ua_sheet_gives_raw_text_display_contents() {
        let mut tree = tree();
        let page = tree.create_page("page", 0);
        let text = tree.create_text(NO_ELEMENT);
        let raw = tree.create_raw_text("hi");
        tree.append_element(text, raw).unwrap();
        tree.append_element(page, text).unwrap();
        tree.flush_element_tree();

        assert_eq!(display_of(&tree, text), Display::Linear);
        assert_eq!(display_of(&tree, raw), Display::Contents);
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
        let view = tree.create_view(NO_ELEMENT);
        tree.append_element(page, view).unwrap();
        tree.flush_element_tree();

        let node = tree.node_id(view).unwrap();
        let style = tree.document().get(node).unwrap().computed_style().unwrap();
        // The switch is off, so nothing overrides the CSS initial value and
        // the element is no longer a Lynx linear box.
        assert_ne!(style.clone_display(), Display::Linear);
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
            let view = tree.create_view(NO_ELEMENT);
            tree.append_element(page, view).unwrap();
            tree.flush_element_tree();

            let node = tree.node_id(view).unwrap();
            let style = tree.document().get(node).unwrap().computed_style().unwrap();
            assert_eq!(style.clone_overflow_x(), expected, "visible={visible}");
            assert_eq!(style.clone_overflow_y(), expected, "visible={visible}");
        }
    }

    #[test]
    fn the_config_is_reported_back_unchanged() {
        let config = PageConfig {
            default_display_linear: false,
            default_overflow_visible: false,
            enable_css_selector: false,
        };
        let tree = ElementTree::new(Viewport::new(1.0, 1.0), config);
        assert_eq!(tree.config(), config);
    }

    /// A wide tree can be built and flushed without special-case bookkeeping.
    #[test]
    fn a_wide_tree_flushes() {
        let mut tree = tree();
        let page = tree.create_page("card", 0);
        for _ in 0..2000 {
            let view = tree.create_view(NO_ELEMENT);
            tree.append_element(page, view).unwrap();
        }
        tree.flush_element_tree();
        assert_eq!(child_ids(&tree, page).len(), 2000);
    }

    /// The page's permanent id is a handle like any other, so `PAGE_UNIQUE_ID`
    /// must stay the first live arena slot.
    #[test]
    fn the_page_holds_the_first_live_arena_slot() {
        let tree = tree();
        assert_eq!(PAGE_UNIQUE_ID, 1);
        assert_eq!(
            tree.element(PAGE_UNIQUE_ID).map(LynxElement::unique_id),
            Some(PAGE_UNIQUE_ID)
        );
    }
}
