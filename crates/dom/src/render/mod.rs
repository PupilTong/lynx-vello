//! The DOM-free render floor: the embedder's image contract and wgpu
//! submission/readback over the one workspace [`vello`] version.
//!
//! Nothing here knows about nodes, computed styles, layout, or paint order —
//! the document-aware painter builds a [`vello::Scene`] and this floor turns
//! scenes into pixels. Embedders configure wgpu/peniko/kurbo exclusively
//! through the crate-root [`crate::vello`] re-export.
//!
//! [`blur`] is the one exception, and a narrow one: a `filter: blur()` bake
//! has to replay part of a committed frame's compose program, so it reads
//! [`crate::CommittedFrame`]'s filter side table and calls the frame's own
//! `bake_filter`. It still knows nothing about nodes, styles, layout or
//! paint order — the frame hands it device-pixel geometry and an opaque op
//! range.

pub mod blur;
pub mod gpu;
pub(crate) mod image;

pub use crate::render::image::MAX_RENDERABLE_DIMENSION;
