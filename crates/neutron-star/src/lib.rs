#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! **neutron-star** — a trait-first, statically-dispatched Flexbox, Grid, and
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
        AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutNode, LayoutOutput, RequestedAxis,
        SizingMode,
    };
}
