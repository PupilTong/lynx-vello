//! The tree protocol: how the engine sees the host's node tree.

mod io;

pub use io::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis, SizingMode,
};

use crate::geometry::Point;
use crate::style::CoreStyle;

pub trait LayoutNode: Copy + core::fmt::Debug {
    type Style: CoreStyle;

    type ChildIter: Iterator<Item = Self>;

    fn children(self) -> Self::ChildIter;

    fn child_count(self) -> usize {
        self.children().count()
    }

    fn style(self) -> Self::Style;

    fn compute_layout(self, input: LayoutInput) -> LayoutOutput;

    fn set_unrounded_layout(self, layout: Layout);

    fn with_unrounded_layout<R>(self, read: impl FnOnce(&Layout) -> R) -> R;

    #[inline]
    fn clone_unrounded_layout(self) -> Layout {
        self.with_unrounded_layout(Layout::clone)
    }

    fn set_rounded_layout(self, layout: Layout);

    fn set_static_position(self, static_position: Point<f32>);

    fn cached_layout(self, input: LayoutInput) -> Option<LayoutOutput>;

    fn store_cached_layout(self, input: LayoutInput, output: LayoutOutput);

    fn clear_layout_cache(self);
}
