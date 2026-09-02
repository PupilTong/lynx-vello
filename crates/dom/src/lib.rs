#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// `hint::likely` is still unstable, on the newest nightly as much as the
// pinned one. It marks the branch every tree walk runs into — the staleness
// check in `TreeArenas` — so the live path stays fall-through.
#![feature(likely_unlikely)]
#![deny(unreachable_pub)]
// The crate's whole `unsafe` surface is two blocks — the arena-set backpointer
// deref and Stylo's `TElement::ensure_data` contract call — and each states the
// invariant it rests on. The workspace-wide `unsafe_code = "warn"` says a block
// may exist; this says it must explain itself.
#![warn(clippy::undocumented_unsafe_blocks)]

//! `dom` — a generic, stylo-integrated W3C-DOM-subset document tree.

#[cfg(test)]
extern crate self as dom;

#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_common;

pub mod event;
pub mod input;
pub mod layout;
mod paint;
pub mod render;
pub mod scroll;
mod style;
mod tree;
mod visual;

pub use euclid::default::{Point2D, Size2D, Vector2D};
pub use hughie::text::FontBlob;
pub use stylo;
pub use stylo_dom::ElementState;
pub use vello;

pub use crate::paint::plan::CompositePlan;
pub use crate::render::image::{
    FrameImages, ImageEvent, ImageInbox, ImageReports, MAX_RENDERABLE_DIMENSION, NoImages,
    is_renderable,
};
pub use crate::style::animation::AnimationTick;
pub use crate::style::device::Device;
#[doc(hidden)]
pub use crate::style::device::standards_device;
pub use crate::style::engine::{CssDeclaration, CssKeyframe, CssRule, StylesheetOrigin};
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub use crate::style::flush::install_style_thread_pool;
pub use crate::style::query::InvalidSelector;
pub use crate::tree::custom::CustomElement;
pub use crate::tree::document::{Document, NodeId};
#[doc(hidden)]
pub use crate::tree::node::ChildrenIter;
pub use crate::tree::node::Node;
pub use crate::tree::shadow::ShadowRootMode;
pub use crate::visual::frame::ENCODE_WINDOW_SCROLLPORTS;
pub use crate::visual::{AnimationSlot, CommittedFrame, HitTarget, ScrollSlot};
