#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The workspace sets `unsafe_code = "warn"`, which any module can silence with a
// local `allow`. This crate holds no `unsafe` at all, and that is a property
// worth a machine check rather than a convention: `forbid` cannot be overridden
// from inside the crate, so introducing `unsafe` here has to be a deliberate
// edit to this line.
#![forbid(unsafe_code)]

//! **hughie** — a trait-first, statically-dispatched Flexbox, Grid, and
//! Starlight Linear/Relative engine for host-owned trees.

pub mod cache;
pub mod compute;
pub mod geometry;
pub mod invalidate;
pub mod style;
pub mod text;
pub mod tree;

pub mod prelude {
    pub use crate::compute::{LeafMeasureInput, LeafMetrics, NaturalSize};
    pub use crate::geometry::{Edges, Line, Point, Size};
    pub use crate::style::{CoreStyle, TextContainerStyle, TextRunStyle};
    pub use crate::tree::{
        AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutSlot, LayoutTree,
        RequestedAxis, SizingMode,
    };
}
