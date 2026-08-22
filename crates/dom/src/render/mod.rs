//! The DOM-free render floor: decoded image resources and wgpu
//! submission/readback over the one workspace [`vello`] version.
//!
//! Nothing here knows about nodes, computed styles, layout, or paint order —
//! the document-aware painter builds a [`vello::Scene`] and this floor turns
//! scenes into pixels. Embedders configure wgpu/peniko/kurbo exclusively
//! through the crate-root [`crate::vello`] re-export.

pub mod gpu;
pub(crate) mod images;

pub use crate::render::images::{ImageKey, MAX_RENDERABLE_DIMENSION, OversizedImage};
