//! Retained sticky constraints, sampled against the compositor's live offsets.
//!
//! Constraints live in untransformed layout space. Their resulting displacement
//! is mapped through the sticky box's parent transform only after solving them,
//! exactly like relative positioning. A descendant inherits that displacement;
//! a fixed descendant escapes it through the builder's containing-block context.

use euclid::default::{Transform3D, Vector2D};
use smallvec::SmallVec;

use super::sticky::StickyAxis;
use super::{PaintOrder, ScrollSlot};
use crate::paint::compose::snap_offset;

#[derive(Debug)]
pub(crate) struct StickySlot {
    pub(crate) parent: Option<u32>,
    pub(crate) scroll: [Option<u32>; 2],
    /// Sticky movement shared with the selected scrollport cancels out.
    pub(crate) scroll_sticky: [Option<u32>; 2],
    pub(crate) axes: [StickyAxis; 2],
    pub(crate) parent_transform: Transform3D<f32>,
}

/// The sampled displacements of one frame's sticky slots.
pub(crate) type StickySamples = SmallVec<[StickySample; 4]>;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct StickySample {
    /// Cumulative movement before transforms, for nested sticky constraints.
    layout: Vector2D<f32>,
    /// Cumulative movement in viewport CSS pixels, for paint and hit testing.
    pub(crate) translation: Vector2D<f32>,
}

impl PaintOrder {
    /// Every sticky slot's displacement at these offsets, in slot order.
    /// Inline for the few sticky boxes a page has, so sampling — which
    /// happens on every compose and every hit test — allocates nothing.
    pub(crate) fn sample_stickies(
        &self,
        ratio: f32,
        offsets: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
    ) -> StickySamples {
        let mut samples = StickySamples::with_capacity(self.stickies.len());
        for slot in &self.stickies {
            let inherited = slot
                .parent
                .map_or_else(StickySample::default, |parent| samples[parent as usize]);
            let axis = |index: usize| {
                let scroll = slot.scroll[index].map_or(0.0, |scroll| {
                    let entry = &self.slots[scroll as usize];
                    let offset = snap_offset(offsets(entry).unwrap_or(entry.offset), ratio);
                    component(offset, index)
                });
                let shared = slot.scroll_sticky[index].map_or(0.0, |ancestor| {
                    component(samples[ancestor as usize].layout, index)
                });
                slot.axes[index].offset(scroll, component(inherited.layout, index) - shared)
            };
            let own = Vector2D::new(axis(0), axis(1));
            let transform = &slot.parent_transform;
            let mapped = Vector2D::new(
                transform.m11 * own.x + transform.m21 * own.y,
                transform.m12 * own.x + transform.m22 * own.y,
            );
            samples.push(StickySample {
                layout: inherited.layout + own,
                // Insets retain subpixel precision. Only the scroll input
                // follows the engine's per-scrollport device-grid snapping.
                translation: inherited.translation + mapped,
            });
        }
        samples
    }

    /// A conservative range for content movement relative to its effect group.
    /// Shared sticky chains cancel exactly; differing chains include every
    /// permitted displacement so an offscreen normal box can still stick.
    pub(crate) fn sticky_range(
        &self,
        mut content: Option<u32>,
        frame: Option<u32>,
    ) -> (Vector2D<f32>, Vector2D<f32>) {
        if content == frame {
            return (Vector2D::zero(), Vector2D::zero());
        }
        let mut low = Vector2D::zero();
        let mut high = Vector2D::zero();
        while let Some(index) = content {
            if Some(index) == frame {
                return (low, high);
            }
            let slot = &self.stickies[index as usize];
            let (x0, x1) = slot.axes[0].offset_bounds();
            let (y0, y1) = slot.axes[1].offset_bounds();
            let transform = &slot.parent_transform;
            let x = [transform.m11 * x0, transform.m11 * x1];
            let y = [transform.m21 * y0, transform.m21 * y1];
            low.x += x[0].min(x[1]) + y[0].min(y[1]);
            high.x += x[0].max(x[1]) + y[0].max(y[1]);
            let x = [transform.m12 * x0, transform.m12 * x1];
            let y = [transform.m22 * y0, transform.m22 * y1];
            low.y += x[0].min(x[1]) + y[0].min(y[1]);
            high.y += x[0].max(x[1]) + y[0].max(y[1]);
            content = slot.parent;
        }
        if frame.is_some() {
            let (frame_low, frame_high) = self.sticky_range(frame, None);
            low -= frame_high;
            high -= frame_low;
        }
        (low, high)
    }
}

fn component(vector: Vector2D<f32>, axis: usize) -> f32 {
    if axis == 0 { vector.x } else { vector.y }
}
