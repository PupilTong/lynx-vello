//! `lynx-element` — the Lynx runtime element layer.
//!
//! This is the crate the layering diagrams in `docs/style-architecture.md` and
//! `docs/layout-architecture.md` drew as the dashed "future Lynx runtime
//! adapter" box: it owns the vocabulary the generic [`dom`] core is
//! forbidden to know about — Lynx tag names, Element-PAPI opcodes, unique-id
//! handle lifetime, `<page>` root policy, view/device construction, and the
//! Lynx UA cascade defaults.
//!
//! ```text
//! bobcat-cli  ──▶  bobcat-core  ──▶  lynx-element  ──▶  dom
//!                                      handles + UA       resources/GPU
//! ```
//!
//! # Element PAPI scope
//!
//! web-core's main-thread global object carries 61 `__`-prefixed Element PAPI
//! members. This crate implements the set a compiled `ReactLynx` bundle issues
//! to build, style, and commit its **first screen** — the sequence
//! `examples/react`'s bundle actually makes, plus the two members the
//! `web-core-e2e` fixtures add:
//!
//! | PAPI | Method |
//! | --- | --- |
//! | `__CreatePage(componentID, componentCSSID)` | [`ElementTree::create_page`] |
//! | `__CreateElement(tagName, parentComponentUniqueID)` | [`ElementTree::create_element`] |
//! | `__CreateView(parentComponentUniqueID)` | [`ElementTree::create_view`] |
//! | `__CreateText(parentComponentUniqueID)` | [`ElementTree::create_text`] |
//! | `__CreateImage(parentComponentUniqueID)` | [`ElementTree::create_image`] |
//! | `__CreateRawText(text)` | [`ElementTree::create_raw_text`] |
//! | `__GetElementUniqueID(element)` | [`ElementTree::element_unique_id`] |
//! | `__SetClasses(element, classNames)` | [`ElementTree::set_classes`] |
//! | `__SetID(element, id)` | [`ElementTree::set_id`] |
//! | `__SetAttribute(element, key, value)` | [`ElementTree::set_attribute`] |
//! | `__SetInlineStyles(element, value)` | [`ElementTree::set_inline_styles`] |
//! | `__SetCSSId(elements, cssId, entryName)` | [`ElementTree::set_css_id`] |
//! | `__AddEvent(element, eventType, eventName, handler)` | [`ElementTree::add_event`] |
//! | `__AppendElement(parent, child)` | [`ElementTree::append_element`] |
//! | `__DropElement(element)` | [`ElementTree::drop_element`] |
//! | `__FlushElementTree()` | [`ElementTree::flush_element_tree`] |
//!
//! Everything else — the tree-editing members an *update* needs
//! (`__InsertElementBefore`, `__RemoveElement`, `__ReplaceElement`),
//! components, lists, querying, animation, and the whole read side — is not
//! implemented. Calling into this crate is the whole Element PAPI surface that
//! exists today; a script that needs more fails at the missing global, not
//! silently rendering wrong.
//!
//! # The text content model
//!
//! Lynx carries text in an **attribute on an element**: `__CreateRawText(s)` is
//! `createElement('raw-text')` plus `setAttribute('text', s)`, and a `<text>`
//! whose single child is a static string is compiled to `<text text="…"/>`
//! instead. [`dom`], meanwhile, measures and paints text only from DOM text
//! nodes. This crate bridges the two: writing the `text` attribute keeps it
//! selector-visible *and* materializes the value as one runtime-owned text-node
//! child, which carries the null unique id so it stays out of the handle space
//! the script sees. Clearing the attribute removes that node again.
//!
//! # Recorded limits
//!
//! - **The runtime identity and JavaScript handle are the same unique id.** [`ElementTree`] speaks
//!   [`ElementId`] internally, matching the native engine's identity (`__GetElementUniqueID`), and
//!   `bobcat-core`'s optional `QuickJS` runtime carries it directly over its primitives-only
//!   boundary.
//! - **Unique ids and arena slots are never recycled.** The context owns a
//!   `Vec<Option<LynxElement>>` whose slot zero is the permanent null sentinel. [`ElementId`] is
//!   simply `u32`, and every positive id is also its direct arena index. `__DropElement` retires a
//!   subtree through [`ElementTree::drop_element`], which takes each value and leaves a permanent
//!   `None` tombstone. `Document<ElementId>` stores that same unique id. `dom` may reuse its
//!   private `NodeId` slots, but no stale script identity can ever name a later element.
//! - **There is no runtime tree-depth cap in this layer.** `ElementTree` keeps no depth-specific
//!   state or traversal helpers; hardening recursive walks belongs in `dom` and `hughie`.
//! - **`parentComponentUniqueID` and `componentCSSID` are recorded, not honored.** web-core uses
//!   them to scope author rules to the components carrying a CSS fragment id. Every decoded sheet
//!   is mounted globally here instead, which is what an `enableRemoveCSSScope` bundle wants and
//!   wrong for a scoped one. `__SetCSSId`'s `entryName` is dropped outright — this layer has one
//!   stylesheet set, not a multi-entry one.
//! - **`__GetElementUniqueID` rejects a dead handle** where web-core returns `-1`. The handle *is*
//!   the unique id here, so there is nothing to return for one that never named an element.
//! - **Event bindings are recorded, never dispatched.** `__AddEvent` files the binding on the
//!   element (off the attribute set, as Lynx and web-core both do) for the future event layer;
//!   nothing reads it, and a worklet handler — an object at the runtime boundary — is recorded
//!   without its payload.
//! - **`<text>` has no inline formatting context.** Lynx flows a text element's `raw-text` and
//!   nested `<text>` children into one paragraph; here each text node is its own leaf box laid out
//!   as a flex item, so runs do not wrap together and per-run leading and trailing whitespace is
//!   trimmed. Merging them needs a per-run paint brush that `dom` does not have yet.
//! - **`<image>` loads nothing.** The `src` attribute is recorded and the box is sized by CSS (the
//!   UA `contain: strict` is what keeps a Lynx image from taking a bitmap's natural size), but no
//!   fetch, decode, or paint of the bitmap exists at any layer above this one.
//! - **The UA sheet models the web target's tag defaults** for `page`, `view`, `text`, `raw-text`
//!   and `image` only, under the two page-config switches. `defaultOverflowVisible` is applied to
//!   the page as well as to views, where web-core relaxes views only. Lynx's wider default set and
//!   its other built-in tags are not modelled.
//! - **No `ppx` view-unit policy.** `rpx` needs none — the vendored stylo fork resolves it against
//!   the device viewport width — but `ppx` has no counterpart in the fork.

mod arena;
mod device;
mod tree;
mod ua;

/// A Lynx element's stable unique id and Element-PAPI handle.
/// The layer below, re-exported whole: `bobcat-core` and the product reach
/// the document/render/style stack exclusively through this door.
pub use dom;

pub type ElementId = u32;

pub use crate::arena::{EventBinding, LynxElement};
pub use crate::device::Viewport;
pub use crate::tree::{ElementTree, PapiError};
pub use crate::ua::PageConfig;

/// The Lynx tag name of the page root element.
///
/// The generic core stores whatever tag string it is given; Lynx's selector
/// semantics treat `page` as the document element (see
/// `docs/tracking/css-selectors-cascade.md`), and web-core's `__CreatePage`
/// creates the one element every other element hangs off.
pub(crate) const PAGE_TAG: &str = "page";

/// The Lynx tag name `__CreateView` constructs.
///
/// web-core maps the Lynx tag `view` to the custom element `x-view` because it
/// renders into an HTML document. There is no HTML here, so the Lynx tag name
/// is kept verbatim — it is what author CSS from a `.web.bundle` selects on.
pub(crate) const VIEW_TAG: &str = "view";

/// The Lynx tag name `__CreateText` constructs (web-core's `x-text`).
pub(crate) const TEXT_TAG: &str = "text";

/// The Lynx tag name `__CreateRawText` constructs.
///
/// web-core creates a real `raw-text` element and puts the string in its `text`
/// attribute rather than in a DOM text node
/// (`createElementAPI.ts`'s `__CreateRawText`), so a raw text is an element
/// here too, and [`TEXT_ATTRIBUTE`] carries its content.
pub(crate) const RAW_TEXT_TAG: &str = "raw-text";

/// The Lynx tag name `__CreateImage` constructs (web-core's `x-image`).
pub(crate) const IMAGE_TAG: &str = "image";

/// The attribute Lynx carries text content in.
///
/// It is a real attribute on a real element in both Lynx and web-core, not a
/// child node — `<text text="React"/>` is how `ReactLynx` compiles a static
/// string child. `dom` measures and paints text only from DOM text nodes, so
/// [`ElementTree`] materializes the attribute value as one runtime-owned text
/// node child while keeping the attribute itself selector-visible.
pub(crate) const TEXT_ATTRIBUTE: &str = "text";
