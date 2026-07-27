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
//! bobcat-quickjs (MTS globals)  ──▶  lynx-element  ──▶  dom  ──▶  vendor/stylo
//!   __CreateView, __AppendElement    handles + UA sheet   DOM + CSS core
//! ```
//!
//! # Element PAPI scope
//!
//! web-core's main-thread global object carries 61 `__`-prefixed Element PAPI
//! members. This crate implements the four that make a tree exist, mutate, and
//! become visible:
//!
//! | PAPI | Method |
//! | --- | --- |
//! | `__CreatePage(componentID, componentCSSID)` | [`ElementTree::create_page`] |
//! | `__CreateView(parentComponentUniqueID)` | [`ElementTree::create_view`] |
//! | `__AppendElement(parent, child)` | [`ElementTree::append_element`] |
//! | `__FlushElementTree()` | [`ElementTree::flush_element_tree`] |
//!
//! Everything else — attributes, classes, inline styles, `__SetCSSId`, events,
//! the other `__Create*` constructors, querying, list callbacks — is not
//! implemented yet. Calling into this crate is the whole Element PAPI surface
//! that exists today; a script that needs more will fail at the missing global,
//! not silently render wrong.
//!
//! # Recorded limits
//!
//! - **Handles are unique ids, not element objects.** web-core's CSR target returns a live
//!   `HTMLElement` from `__CreateView` and stamps a symbol-keyed unique id on it; its SSR target
//!   returns a plain `{ [uniqueIdSymbol]: id }` record. We follow the SSR shape: an [`ElementId`]
//!   *is* the handle, matching how the native engine identifies elements (`__GetElementUniqueID`)
//!   and what the script boundary in `bobcat-quickjs` can carry. A `ReactLynx` bundle that passes
//!   element objects around would need an object-carrying script boundary first.
//! - **Unique ids are never recycled.** web-core allocates dense indices from 1 (its map is seeded
//!   with one `None` at index 0); so do we, and a removed element's id stays retired. `dom`'s
//!   `NodeId` *is* reusable, which is exactly why this layer keeps its own handle space.
//! - **`parentComponentUniqueID` is recorded, not honored.** web-core uses it only to inherit the
//!   parent component's CSS fragment id (`l-css-id`). Without `__SetCSSId` there is no CSS-scope
//!   machinery to inherit into, so the argument is validated and stored on the element and
//!   otherwise unused.
//! - **The UA sheet covers the three documented Lynx computed defaults** (`display: linear`,
//!   `box-sizing: border-box`, `overflow: hidden`) under their two page-config switches. Lynx's
//!   wider default set is not modelled.
//! - **No `rpx`/`ppx` view-unit policy yet.** The device is built from CSS pixels and a
//!   device-pixel ratio only.

mod device;
mod tree;
mod ua;

pub use crate::device::{LynxFontMetricsProvider, Viewport};
pub use crate::tree::{ElementId, ElementTree, MAX_TREE_DEPTH, PapiError};
pub use crate::ua::{PageConfig, ua_stylesheet};

/// The Lynx tag name of the page root element.
///
/// The generic core stores whatever tag string it is given; Lynx's selector
/// semantics treat `page` as the document element (see
/// `docs/tracking/css-selectors-cascade.md`), and web-core's `__CreatePage`
/// creates the one element every other element hangs off.
pub const PAGE_TAG: &str = "page";

/// The Lynx tag name `__CreateView` constructs.
///
/// web-core maps the Lynx tag `view` to the custom element `x-view` because it
/// renders into an HTML document. There is no HTML here, so the Lynx tag name
/// is kept verbatim — it is what author CSS from a `.web.bundle` selects on.
pub const VIEW_TAG: &str = "view";
