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
//! members. This crate implements six globals that make a tree exist, mutate,
//! retire, and become visible (five web-core members plus the engine's explicit
//! lifetime primitive):
//!
//! | PAPI | Method |
//! | --- | --- |
//! | `__CreatePage(componentID, componentCSSID)` | [`ElementTree::create_page`] |
//! | `__CreateView(parentComponentUniqueID)` | [`ElementTree::create_view`] |
//! | `__AppendElement(parent, child)` | [`ElementTree::append_element`] |
//! | `__RemoveElement(parent, child)` | [`ElementTree::remove_element`] |
//! | `__DropElement(element)` | [`ElementTree::drop_element`] |
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
//! - **Runtime identity is a unique id; JavaScript owns an opaque wrapper.** [`ElementTree`] speaks
//!   [`ElementId`] internally, matching the native engine's identity (`__GetElementUniqueID`).
//!   `bobcat-core`'s optional `QuickJS` adapter exposes numeric host methods under `bobcat`, then a
//!   JavaScript `WeakRef` arena and `FinalizationRegistry` own the wrappers and retire collected
//!   elements; this engine-neutral crate sees only the id.
//! - **Remove detaches; Drop retires.** [`ElementTree::remove_element`] preserves the child and its
//!   whole subtree so the same wrapper can be appended again. [`ElementTree::drop_element`]
//!   permanently retires the named element and DOM node; its direct light-DOM children each become
//!   detached live subtrees, preserving their descendants and unique ids. This layer does not
//!   expose shadow-root creation, so every element it can retire satisfies the DOM core's
//!   single-node-drop precondition.
//! - **Unique ids and arena slots are never recycled.** The context owns a
//!   `Vec<Option<LynxElement>>` whose slot zero is the permanent null sentinel. [`ElementId`] is
//!   simply `u32`, and every positive id is also its direct arena index. `__DropElement` retires
//!   its target through [`ElementTree::drop_element`], which takes that value and leaves a
//!   permanent `None` tombstone. `Document<ElementId>` stores that same unique id. `dom` may reuse
//!   its private `NodeId` slots, but no stale script identity can ever name a later element.
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

pub(crate) const VIEW_TAG: &str = "view";
