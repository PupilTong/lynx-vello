//! Retained sticky constraints, sampled against the compositor's live offsets.
//!
//! Constraints live in untransformed layout space. Their resulting displacement
//! is mapped through the sticky box's parent transform only after solving them,
//! exactly like relative positioning. Each sticky box is one node of the
//! frame's space tree, applying only its own mapped shift, so a descendant
//! inherits the displacement through its space path and a fixed descendant
//! escapes it through the builder's containing-block context.

use euclid::default::{Transform3D, Vector2D};

use super::sticky::StickyAxis;
use super::{PaintOrder, ScrollSlot, SlotSamples};
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

/// The sampled displacements of a frame's sticky slots.
pub(crate) type StickySamples = SlotSamples<StickySample>;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct StickySample {
    /// Cumulative movement before transforms, the input of nested sticky
    /// constraints.
    layout: Vector2D<f32>,
    /// This box's own movement mapped through its parent transform, in
    /// viewport CSS px: its sticky node's shift in the space tree.
    pub(crate) mapped: Vector2D<f32>,
}

impl PaintOrder {
    /// Every sticky slot's displacement at these offsets, in slot order —
    /// what a hit test reads.
    pub(crate) fn sample_stickies(
        &self,
        ratio: f32,
        offsets: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
    ) -> StickySamples {
        let every =
            0..u32::try_from(self.stickies.len()).expect("a frame cannot hold 2^32 sticky boxes");
        self.solve_stickies(every, ratio, offsets)
    }

    /// [`Self::sample_stickies`] over only `composed`, the boxes a program
    /// composes and the boxes those solve against; see
    /// [`Self::mark_composed_spaces`].
    pub(crate) fn sample_composed_stickies(
        &self,
        composed: &[u32],
        ratio: f32,
        offsets: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
    ) -> StickySamples {
        self.solve_stickies(composed.iter().copied(), ratio, offsets)
    }

    /// Solves `slots`, ascending and closed under each box's solve inputs.
    fn solve_stickies(
        &self,
        slots: impl Iterator<Item = u32>,
        ratio: f32,
        offsets: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
    ) -> StickySamples {
        let mut samples = StickySamples::default();
        for index in slots {
            let slot = &self.stickies[index as usize];
            let inherited = slot
                .parent
                .map_or_else(StickySample::default, |parent| *samples.get(parent));
            let axis = |axis: usize| {
                let scroll = slot.scroll[axis].map_or(0.0, |scroll| {
                    let entry = &self.slots[scroll as usize];
                    let offset = snap_offset(offsets(entry).unwrap_or(entry.offset), ratio);
                    component(offset, axis)
                });
                let shared = slot.scroll_sticky[axis].map_or(0.0, |ancestor| {
                    component(samples.get(ancestor).layout, axis)
                });
                slot.axes[axis].offset(scroll, component(inherited.layout, axis) - shared)
            };
            let own = Vector2D::new(axis(0), axis(1));
            let transform = &slot.parent_transform;
            let mapped = Vector2D::new(
                transform.m11 * own.x + transform.m21 * own.y,
                transform.m12 * own.x + transform.m22 * own.y,
            );
            samples.push(
                index,
                StickySample {
                    layout: inherited.layout + own,
                    // Insets retain subpixel precision. Only the scroll input
                    // follows the engine's per-scrollport device-grid snapping.
                    mapped,
                },
            );
        }
        samples
    }

    /// The `(low, high)` range one sticky node's own mapped shift covers —
    /// every displacement its constraints permit, in viewport CSS px.
    pub(crate) fn sticky_slot_range(&self, slot: u32) -> (Vector2D<f32>, Vector2D<f32>) {
        let slot = &self.stickies[slot as usize];
        let (x0, x1) = slot.axes[0].offset_bounds();
        let (y0, y1) = slot.axes[1].offset_bounds();
        let transform = &slot.parent_transform;
        let axis = |a: f32, b: f32| {
            let x = [a * x0, a * x1];
            let y = [b * y0, b * y1];
            (
                x[0].min(x[1]) + y[0].min(y[1]),
                x[0].max(x[1]) + y[0].max(y[1]),
            )
        };
        let (low_x, high_x) = axis(transform.m11, transform.m21);
        let (low_y, high_y) = axis(transform.m12, transform.m22);
        (Vector2D::new(low_x, low_y), Vector2D::new(high_x, high_y))
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
            let (slot_low, slot_high) = self.sticky_slot_range(index);
            low += slot_low;
            high += slot_high;
            content = self.stickies[index as usize].parent;
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
