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
//! web-core's main-thread global object carries 62 `__`-prefixed Element PAPI
//! members (`web-core/ts/types/IElementPAPI.ts`). The subset implemented here
//! is defined by one question: **what does a compiled `ReactLynx` app call to
//! render its first frame and then apply a state-change patch?** That set was
//! read off `lynx-stack/packages/react` — both the runtime's own call sites and
//! the calls its SWC transform emits into compiled snapshot `create`/`update`
//! functions — and is implemented in full, minus the members that would need a
//! subsystem this engine does not have yet.
//!
//! | PAPI | Method |
//! | --- | --- |
//! | `__CreatePage(componentID, componentCSSID)` | [`ElementTree::create_page`] |
//! | `__CreateElement(tagName, parentComponentUniqueID)` | [`ElementTree::create_element`] |
//! | `__CreateView(parentComponentUniqueID)` | [`ElementTree::create_view`] |
//! | `__CreateText(parentComponentUniqueID)` | [`ElementTree::create_text`] |
//! | `__CreateImage(parentComponentUniqueID)` | [`ElementTree::create_image`] |
//! | `__CreateScrollView(parentComponentUniqueID)` | [`ElementTree::create_scroll_view`] |
//! | `__CreateWrapperElement(parentComponentUniqueID)` | [`ElementTree::create_wrapper_element`] |
//! | `__CreateFrame(parentComponentUniqueID)` | [`ElementTree::create_frame`] |
//! | `__CreateRawText(text)` | [`ElementTree::create_raw_text`] |
//! | `__AppendElement(parent, child)` | [`ElementTree::append_element`] |
//! | `__InsertElementBefore(parent, child, reference)` | [`ElementTree::insert_element_before`] |
//! | `__RemoveElement(parent, child)` | [`ElementTree::remove_element`] |
//! | `__ReplaceElement(newElement, oldElement)` | [`ElementTree::replace_element`] |
//! | `__SwapElement(a, b)` | [`ElementTree::swap_element`] |
//! | `__GetParent(element)` | [`ElementTree::parent_element`] |
//! | `__FirstElement(element)` | [`ElementTree::first_element`] |
//! | `__LastElement(element)` | [`ElementTree::last_element`] |
//! | `__NextElement(element)` | [`ElementTree::next_element`] |
//! | `__GetTag(element)` | [`ElementTree::tag`] |
//! | `__GetElementUniqueID(element)` | [`ElementTree::unique_id`] |
//! | `__GetPageElement()` | [`ElementTree::page`] |
//! | `__SetAttribute(element, key, value)` | [`ElementTree::set_attribute`] |
//! | `__GetAttributeByName(element, name)` | [`ElementTree::attribute`] |
//! | `__SetID(element, id)` / `__GetID(element)` | [`ElementTree::set_id`] / [`ElementTree::id_attribute`] |
//! | `__AddClass(element, className)` | [`ElementTree::add_class`] |
//! | `__SetClasses(element, classNames)` | [`ElementTree::set_classes`] |
//! | `__GetClasses(element)` | [`ElementTree::classes`] |
//! | `__SetInlineStyles(element, value)` | [`ElementTree::set_inline_styles`] |
//! | `__AddInlineStyle(element, key, value)` | [`ElementTree::add_inline_style`] |
//! | `__AddDataset(element, key, value)` | [`ElementTree::add_dataset`] |
//! | `__SetDataset(element, dataset)` | [`ElementTree::clear_dataset`] + [`ElementTree::add_dataset`] |
//! | `__GetDataByKey(element, key)` | [`ElementTree::data_by_key`] |
//! | `__SetCSSId(elements, cssId, entryName)` | [`ElementTree::set_css_id`] |
//! | `__UpdateComponentID(element, componentID)` | [`ElementTree::update_component_id`] |
//! | `__GetComponentID(element)` | [`ElementTree::component_id`] |
//! | `__FlushElementTree()` | [`ElementTree::flush_element_tree`] |
//!
//! One member has no web-core counterpart and is not an Element-PAPI name
//! there:
//!
//! | Member | Method |
//! | --- | --- |
//! | `__DropElement(element)` | [`ElementTree::drop_element`] |
//!
//! web-core hands the script real `HTMLElement` objects, so it reclaims an
//! element's engine-side storage from a `WeakRef` sweep once the script drops
//! its last reference. Handles here are `u32` numbers, which cannot be held
//! weakly, so reclamation is announced rather than inferred: the realm
//! registers each new handle with a `FinalizationRegistry` and `__DropElement`
//! is what that finalizer calls. It is the counterpart of `__RemoveElement`,
//! not a variant of it.
//!
//! ## Deliberately not implemented
//!
//! Each of these needs a subsystem that does not exist below this layer, so a
//! stub would render or behave wrong rather than fail. A bundle reaching for
//! one gets a `ReferenceError` naming the missing global, which is the
//! intended failure.
//!
//! - **Events** — `__AddEvent`, `__GetEvent`, `__GetEvents`, `__SetEvents`. `dom` has no
//!   `EventTarget`, and Lynx's `bind`/`catch`/`capture-bind`/ `capture-catch`/`global-bind` phase
//!   model plus the gesture arena is the runtime layer's design problem, not this crate's.
//! - **Lists** — `__CreateList`, `__UpdateListCallbacks`, and `__SetAttribute`'s `update-list-info`
//!   overload. All three take JavaScript callbacks the host must call back into; the `QuickJS`
//!   bridge below is strictly leaf-call today.
//! - **Worklets, gestures, animation, UI methods** — `__ElementAnimate`, `__InvokeUIMethod`,
//!   `__SetGestureDetector`, `__RemoveGestureDetector`.
//! - **Selector queries** — `__QuerySelector`, `__QuerySelectorAll`, `__GetComputedStyleByKey`. All
//!   read-side, all needing a serialization or query surface `dom` does not publish.
//! - **Element templates** — the `*ElementTemplate` family, which is an experimental second
//!   `ReactLynx` backend (`__USE_ELEMENT_TEMPLATE__`) and exists only in the native engine.
//! - **`__CreateComponent`** — no `ReactLynx` code path calls it; component subtrees are ordinary
//!   elements carrying a CSS scope.
//!
//! # Recorded limits
//!
//! - **The runtime identity and JavaScript handle are the same unique id.** [`ElementTree`] speaks
//!   [`ElementId`] internally, matching the native engine's identity (`__GetElementUniqueID`), and
//!   `bobcat-core`'s optional `QuickJS` runtime carries it directly over its primitives-only
//!   boundary.
//! - **Unique ids and arena slots are never recycled.** The context owns a
//!   `Vec<Option<LynxElement>>` whose slot zero is the permanent null sentinel. [`ElementId`] is
//!   simply `u32`, and every positive id is also its direct arena index. `Document<ElementId>`
//!   stores that same unique id. `dom` may reuse its private `NodeId` slots, but no stale script
//!   identity can ever name a later element.
//! - **Removal and disposal are different members.** `__RemoveElement` detaches and no more — the
//!   element stays alive, keeps its state, and can be re-inserted, which is what `ReactLynx`'s
//!   reconciler relies on when it reorders children. `__DropElement` is what retires the handle and
//!   takes the storage, leaving a permanent `None` tombstone.
//! - **Registering handles with a `FinalizationRegistry` is not wired up yet.** The `__Create*`
//!   members will do it, and until they do, an element the script forgets about is retained until
//!   `__DropElement` is called explicitly.
//! - **There is no runtime tree-depth cap in this layer.** [`ElementTree`] keeps no depth-specific
//!   state or traversal helpers; hardening recursive walks belongs in `dom` and `hughie`.
//! - **`parentComponentUniqueID` is honored only as CSS scope.** It names no parent and links
//!   nothing: web-core uses it to seed the new element's CSS fragment id, and tolerates a handle
//!   that resolves to nothing by leaving the element unscoped. Both behaviors are reproduced.
//! - **`__SetCSSId` is recorded, not honored.** There is no decoded `StyleInfo` ingestion yet, so
//!   no per-fragment rules exist to scope against. The id is stored and inherited so the member's
//!   observable contract holds.
//! - **`__AddInlineStyle` accepts string property names only.** Its numeric-key form indexes Lynx's
//!   native CSS property id table, which this crate does not carry.
//! - **`__SetInlineStyles` does not rewrite units.** web-core turns `rpx`/`ppx`/`vw`/`vh` into
//!   `calc()` over CSS custom properties because a browser cannot resolve Lynx units; this engine
//!   resolves units in the cascade.
//! - **The UA sheet covers the three documented Lynx computed defaults** (`display: linear`,
//!   `box-sizing: border-box`, `overflow: hidden`) under their two page-config switches, plus
//!   `raw-text { display: contents }`. Lynx's wider default set is not modelled.
//! - **No `rpx`/`ppx` view-unit policy yet.** The device is built from CSS pixels and a
//!   device-pixel ratio only.

mod arena;
mod device;
mod tree;
mod ua;
mod value;

/// The layer below, re-exported whole: `bobcat-core` and the product reach
/// the document/render/style stack exclusively through this door.
pub use dom;

/// A Lynx element's stable unique id and Element-PAPI handle.
pub type ElementId = u32;

pub use crate::arena::LynxElement;
pub use crate::device::Viewport;
pub use crate::tree::{ElementTree, PapiError};
pub use crate::ua::PageConfig;
pub use crate::value::PapiValue;

/// The "no element" handle.
///
/// web-core reserves it the same way — `create_element_common` indexes a
/// vector whose slot zero can never hold an element, and the framework passes
/// `0` for "this element has no parent component". A PAPI member that must act
/// on an element rejects it; one that merely reads a relationship returns it.
pub const NO_ELEMENT: ElementId = 0;

/// The CSS fragment id meaning "no scope".
///
/// web-core spells the same thing `0`: `__SetCSSId(elements, null, …)`
/// coalesces to `0`, and an element whose parent component has id `0` inherits
/// nothing.
pub(crate) const INHERITED_CSS_ID_NONE: i32 = 0;

/// The Lynx tag name of the page root element.
///
/// The generic core stores whatever tag string it is given; Lynx's selector
/// semantics treat `page` as the document element (see
/// `docs/tracking/css-selectors-cascade.md`), and web-core's `__CreatePage`
/// creates the one element every other element hangs off.
pub(crate) const PAGE_TAG: &str = "page";

/// The Lynx tag names the dedicated `__Create*` members construct.
///
/// web-core maps each of these to a custom element (`view` → `x-view`,
/// `text` → `x-text`, …) because it renders into an HTML document, and
/// `__GetTag` maps them back. There is no HTML here, so the Lynx tag name is
/// stored verbatim — it is what author CSS from a `.web.bundle` selects on,
/// and what `__GetTag` must return.
pub(crate) const VIEW_TAG: &str = "view";
pub(crate) const TEXT_TAG: &str = "text";
pub(crate) const IMAGE_TAG: &str = "image";
pub(crate) const SCROLL_VIEW_TAG: &str = "scroll-view";
pub(crate) const WRAPPER_TAG: &str = "wrapper";
pub(crate) const FRAME_TAG: &str = "frame";
pub(crate) const RAW_TEXT_TAG: &str = "raw-text";

/// The attribute a `raw-text` element carries its content in, on every Lynx
/// target.
pub(crate) const RAW_TEXT_TEXT_ATTRIBUTE: &str = "text";

/// The inline-style attribute. `__SetAttribute` writing it and
/// `__AddInlineStyle` layering a declaration over it address one block, so
/// both route through the same store.
pub(crate) const STYLE_ATTRIBUTE: &str = "style";
