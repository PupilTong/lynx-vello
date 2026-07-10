//! Leaf layout: nodes whose content the engine cannot see.
//!
//! Text runs, images, and other replaced/host-rendered content are measured
//! by the **host** (in lynx-vello: the parley-based text engine) and boxed
//! by the **engine** (sizing styles, aspect ratio, min/max clamps, padding
//! and border floors). The seam between the two is a plain closure — no
//! trait object, no registration: the host's dispatch simply calls
//! [`compute_leaf_layout`] with a `measure` closure closing over whatever
//! content state it likes.

use crate::geometry::Size;
use crate::style::CoreStyle;
use crate::style::value::CalcHandle;
use crate::tree::{AvailableSpace, LayoutInput, LayoutOutput};

/// Sizes a content leaf, delegating content measurement to `measure`.
///
/// `measure(known_dimensions, available_space)` returns the content's size
/// in CSS pixels for the given constraints: `known_dimensions` are extents
/// already fixed by styles (measure the other axis against them — e.g. text
/// height for a known width); `available_space` constrains the free axes.
/// The closure is called **at most once** per invocation, and not at all
/// when styles fully determine the size — per-measurement caching stays in
/// the host's [`CacheTree`](crate::tree::CacheTree) at the dispatch layer,
/// keeping this function pure.
///
/// `resolve_calc` mirrors
/// [`LayoutTree::resolve_calc`](crate::tree::LayoutTree::resolve_calc)
/// (leaf layout takes the style view directly rather than a whole tree, so
/// the resolver is passed alongside it).
///
/// The returned size applies, in order: known dimensions verbatim; style
/// size/aspect-ratio (per [`SizingMode`](crate::tree::SizingMode)); measured
/// content size; min/max clamps; padding+border floor (a box is never
/// smaller than its own surrounds). `content_size` reports the unclamped
/// measured content.
///
/// # Panics
///
/// Protocol stub — implemented in milestone L1; calling this currently
/// panics with `todo!`.
pub fn compute_leaf_layout<Style, MeasureFn, CalcResolver>(
    input: LayoutInput,
    style: &Style,
    resolve_calc: CalcResolver,
    measure: MeasureFn,
) -> LayoutOutput
where
    Style: CoreStyle,
    MeasureFn: FnOnce(Size<Option<f32>>, Size<AvailableSpace>) -> Size<f32>,
    CalcResolver: Fn(CalcHandle, f32) -> f32,
{
    let _ = (input, style, &resolve_calc, measure);
    todo!("L1: leaf boxing around the host measure closure (see rustdoc)")
}
