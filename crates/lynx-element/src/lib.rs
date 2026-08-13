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
//! This crate implements every element constructor used by `ReactLynx`'s Snapshot backend except
//! `__CreateFrame`, plus the operations that currently make the resulting tree mutate, retire,
//! and become visible:
//!
//! | PAPI | Method |
//! | --- | --- |
//! | `__CreatePage(componentID, componentCSSID)` | [`ElementTree::create_page`] |
//! | `__CreateElement(tag, parentComponentUniqueID)` | [`ElementTree::create_element`] |
//! | `__CreateWrapperElement(parentComponentUniqueID)` | [`ElementTree::create_wrapper_element`] |
//! | `__CreateText(parentComponentUniqueID)` | [`ElementTree::create_text`] |
//! | `__CreateImage(parentComponentUniqueID)` | [`ElementTree::create_image`] |
//! | `__CreateView(parentComponentUniqueID)` | [`ElementTree::create_view`] |
//! | `__CreateScrollView(parentComponentUniqueID)` | [`ElementTree::create_scroll_view`] |
//! | `__CreateRawText(text)` | [`ElementTree::create_raw_text`] |
//! | `__CreateList(parentComponentUniqueID, ...)` | [`ElementTree::create_list`] |
//! | `__AppendElement(parent, child)` | [`ElementTree::append_element`] |
//! | `__DropElement(element)` | [`ElementTree::drop_element`] |
//! | `__FlushElementTree()` | [`ElementTree::flush_element_tree`] |
//!
//! Everything else — attributes, classes, inline styles, `__SetCSSId`, events,
//! `__CreateFrame`, querying, and list callback execution — is not implemented yet. Creating a
//! list records its element identity and tag only; the current runtime binding does not retain its
//! JavaScript callbacks. Calling into this crate is the whole Element PAPI surface that exists
//! today; a script that needs more will fail at the missing global, not silently render wrong.
//!
//! # Recorded limits
//!
//! - **Runtime identity is a unique id; JavaScript receives an opaque weak-ref object.**
//!   [`ElementTree`] speaks [`ElementId`] internally, matching the native engine's identity
//!   (`__GetElementUniqueID`). `bobcat-core`'s optional `QuickJS` runtime stores that arena id in
//!   an unforgeable object. Collecting the object calls back into [`ElementTree::drop_element`],
//!   which retires only that Lynx element and corresponding DOM node. Its descendants stay live but
//!   detached until the VM reports their own handles dropped; `__DropElement` is the explicit early
//!   path through the same operation.
//! - **Unique ids and arena slots are never recycled.** The context owns a
//!   `Vec<Option<LynxElement>>` whose slot zero is the permanent null sentinel. [`ElementId`] is
//!   simply `u32`, and every positive id is also its direct arena index. Each call to
//!   [`ElementTree::drop_element`] takes exactly one value and leaves a permanent `None` tombstone.
//!   `Document<ElementId>` stores that same unique id. `dom` may reuse its private `NodeId` slots,
//!   but no stale script identity can ever name a later element.
//! - **There is no runtime tree-depth cap in this layer.** `ElementTree` keeps no depth-specific
//!   state or traversal helpers; hardening recursive walks belongs in `dom` and `hughie`.
//! - **`parentComponentUniqueID` is recorded, not honored.** web-core uses it only to inherit the
//!   parent component's CSS fragment id (`l-css-id`). Without `__SetCSSId` there is no CSS-scope
//!   machinery to inherit into, so the argument is validated and stored on the element and
//!   otherwise unused.
//! - **The UA sheet covers the three documented Lynx computed defaults** (`display: linear`,
//!   `box-sizing: border-box`, `overflow: hidden`) under their two page-config switches. Lynx's
//!   wider default set is not modelled.
//! - **No `rpx`/`ppx` view-unit policy yet.** The device is built from CSS pixels and a
//!   device-pixel ratio only.

mod arena;
mod device;
mod tree;
mod ua;

pub use dom;

pub type ElementId = u32;

pub use crate::arena::LynxElement;
pub use crate::device::Viewport;
pub use crate::tree::{ElementTree, PapiError};
pub use crate::ua::PageConfig;

pub(crate) const PAGE_TAG: &str = "page";

pub(crate) const FRAME_TAG: &str = "frame";
pub(crate) const IMAGE_TAG: &str = "image";
pub(crate) const LIST_TAG: &str = "list";
pub(crate) const RAW_TEXT_TAG: &str = "raw-text";
pub(crate) const SCROLL_VIEW_TAG: &str = "scroll-view";
pub(crate) const TEXT_TAG: &str = "text";
pub(crate) const VIEW_TAG: &str = "view";
pub(crate) const WRAPPER_TAG: &str = "wrapper";
