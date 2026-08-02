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
//! bobcat-cli  ──▶  bobcat-core  ──▶  lynx-element  ──▶  dom  ──▶  pulsar
//!                                      handles + UA       resources/GPU
//! ```
//!
//! # Element PAPI scope
//!
//! web-core's main-thread global object carries 61 `__`-prefixed Element PAPI
//! members. This crate implements the five that make a tree exist, mutate, retire, and
//! become visible:
//!
//! | PAPI | Method |
//! | --- | --- |
//! | `__CreatePage(componentID, componentCSSID)` | [`ElementTree::create_page`] |
//! | `__CreateView(parentComponentUniqueID)` | [`ElementTree::create_view`] |
//! | `__AppendElement(parent, child)` | [`ElementTree::append_element`] |
//! | `__DropElement(element)` | [`ElementTree::drop_element`] |
//! | `__FlushElementTree()` | [`ElementTree::flush_element_tree`] |
//!
//! Everything else — attributes, classes, inline styles, `__SetCSSId`, events,
//! the other `__Create*` constructors, querying, list callbacks — is not
//! implemented yet. Calling into this crate is the whole Element PAPI surface
//! that exists today; a script that needs more will fail at the missing global,
//! not silently render wrong.
//!
//! # Public boundary
//!
//! The default API contains the element handle type, the five concrete PAPI
//! operations, page/viewport configuration, and the input/viewport methods a
//! live host calls. It deliberately has no getters for the page, configuration,
//! DOM node ids, arena entries, or flush state, and no forwarding methods for
//! stylesheets, fonts, rendering, scenes, or images. Those were either internal
//! implementation details, test probes, or placeholders without a production
//! consumer.
//!
//! The non-default `internal-document-access` feature exposes the owned
//! [`dom::Document`] only to trusted workspace composition: `bobcat-cli` uses
//! it for its private frame pipeline, and cross-crate render tests use the same
//! real boundary. It is not an embedder convenience and must not be used for
//! topology mutation, which would desynchronise the handle arena.
//!
//! # Recorded limits
//!
//! - **The runtime identity and JavaScript handle are the same unique id.** [`ElementTree`] speaks
//!   [`ElementId`] internally, matching the native engine's identity (`__GetElementUniqueID`), and
//!   `bobcat-core`'s optional `QuickJS` runtime carries it directly over its primitives-only
//!   boundary.
//! - **Unique ids and arena slots are never recycled.** The context owns a
//!   `Vec<Option<dom::NodeId>>` whose slot zero is the permanent null sentinel. [`ElementId`] is
//!   simply `u32`, and every positive id is also its direct arena index. `__DropElement` retires a
//!   subtree through [`ElementTree::drop_element`], which takes each value and leaves a permanent
//!   `None` tombstone. `Document<ElementId>` stores that same unique id. `dom` may reuse its
//!   private `NodeId` slots, but no stale script identity can ever name a later element.
//! - **There is no runtime tree-depth cap in this layer.** `ElementTree` keeps no depth-specific
//!   state or traversal helpers; hardening recursive walks belongs in `dom` and `hughie`.
//! - **Unimplemented component metadata is not retained.** The `QuickJS` host validates the types
//!   of `componentID`, `componentCSSID`, and `parentComponentUniqueID` at the PAPI boundary, but
//!   this crate neither accepts nor stores them: no implemented operation consumes component
//!   identity or CSS scope. Likewise, `enableCSSSelector` belongs to future decoded `StyleInfo`
//!   ingestion and is not a [`PageConfig`] field until such a consumer exists.
//! - **The UA sheet covers the three documented Lynx computed defaults** (`display: linear`,
//!   `box-sizing: border-box`, `overflow: hidden`) under their two page-config switches. Lynx's
//!   wider default set is not modelled.
//! - **No `rpx`/`ppx` view-unit policy yet.** The device is built from CSS pixels and a
//!   device-pixel ratio only.

mod arena;
mod device;
mod tree;
mod ua;

/// A Lynx element's stable unique id and Element-PAPI handle.
pub type ElementId = u32;

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
