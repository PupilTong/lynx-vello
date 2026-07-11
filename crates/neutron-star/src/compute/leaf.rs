//! Leaf layout: nodes whose content the engine cannot see.
//!
//! Text runs, images, and other replaced/host-rendered content are measured
//! by the **host** (in lynx-vello: the parley-based text engine) and boxed
//! by the **engine** (sizing styles, aspect ratio, min/max clamps, padding
//! and border floors). The seam between the two is a plain closure — no
//! trait object, no registration: the host's dispatch simply calls
//! [`compute_leaf_layout`] with a `measure` closure closing over whatever
//! content state it likes.

use super::util::{
    apply_aspect_ratio, apply_box_sizing, auto_edges_to_zero, clamp, resolve_edges,
    resolve_optional_edges, resolve_size, scrollbar_size, subtract_available_space,
};
use crate::geometry::{Edges, Point, Size};
use crate::style::value::CalcHandle;
use crate::style::{BoxSizing, CoreStyle};
use crate::tree::{
    AvailableSpace, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis, SizingMode,
};

/// Sizes a content leaf, delegating content measurement to `measure`.
///
/// `measure(known_dimensions, available_space)` returns the content's size
/// and optional first baselines for the given constraints. `known_dimensions`
/// are resolved or caller-decided border-box dimensions converted to
/// content-box extents (measure the other axis against them — e.g. text
/// height for a known width); `available_space` constrains the free axes.
/// The closure is called at most once. A single-axis size probe whose box is
/// already fully known can skip it; full layout and both-axis probes still
/// measure so text/image overflow and baselines remain available.
///
/// `resolve_calc` mirrors
/// [`LayoutTree::resolve_calc`](crate::tree::LayoutTree::resolve_calc)
/// (leaf layout takes the style view directly rather than a whole tree, so
/// the resolver is passed alongside it).
///
/// The returned size applies, in order: known dimensions verbatim; style
/// size/aspect-ratio (per [`SizingMode`](crate::tree::SizingMode)); measured
/// content size; min/max clamps; and a padding+border floor on axes the
/// engine resolves itself. Caller-supplied known dimensions remain verbatim.
/// `content_size` is the border-origin scrollable extent, including measured
/// overflow.
#[allow(clippy::too_many_lines)]
pub fn compute_leaf_layout<Style, MeasureFn, CalcResolver, Measurement>(
    input: LayoutInput,
    style: &Style,
    resolve_calc: CalcResolver,
    measure: MeasureFn,
) -> LayoutOutput
where
    Style: CoreStyle,
    MeasureFn: FnOnce(Size<Option<f32>>, Size<AvailableSpace>) -> Measurement,
    Measurement: Into<LeafMeasurement>,
    CalcResolver: Fn(CalcHandle, f32) -> f32,
{
    let measurement_axis = match input.goal {
        LayoutGoal::Measure(axis) => Some(axis),
        LayoutGoal::Commit => None,
    };

    let LeafSizing {
        margin,
        padding_border_size,
        content_box_inset,
        content_origin,
        mut node_size,
        min_size,
        max_size,
        aspect_ratio,
        box_sizing,
    } = resolve_leaf_sizing(input, style, &resolve_calc);

    node_size = clamp_resolved_size(
        node_size,
        input.known_dimensions,
        min_size,
        max_size,
        padding_border_size,
    );

    // A single-axis probe needs no content or baseline information once the
    // box is fully fixed. Both-axis probes are used by baseline alignment.
    if measurement_axis.is_some_and(|axis| axis != RequestedAxis::Both)
        && node_size.width.is_some()
        && node_size.height.is_some()
    {
        let size = finalize_size(
            node_size.unwrap_or(Size::ZERO),
            input.known_dimensions,
            min_size,
            max_size,
            padding_border_size,
        );
        return LayoutOutput::new(size, size);
    }

    let measure_known_dimensions = Size::new(
        node_size
            .width
            .map(|width| (width - content_box_inset.width).max(0.0)),
        node_size
            .height
            .map(|height| (height - content_box_inset.height).max(0.0)),
    );
    let available_space = Size::new(
        measurement_available_space(
            measure_known_dimensions.width,
            input.available_space.width,
            margin.horizontal_sum(),
            content_box_inset.width,
            min_size.width,
            max_size.width,
        ),
        measurement_available_space(
            measure_known_dimensions.height,
            input.available_space.height,
            margin.vertical_sum(),
            content_box_inset.height,
            min_size.height,
            max_size.height,
        ),
    );

    let measurement: LeafMeasurement = measure(measure_known_dimensions, available_space).into();
    let measured_content = measurement.size;
    debug_assert!(
        measured_content.width.is_finite() && measured_content.height.is_finite(),
        "leaf measurements must be finite"
    );

    let measured_border_box = Size::new(
        measured_content.width.max(0.0) + content_box_inset.width,
        measured_content.height.max(0.0) + content_box_inset.height,
    );
    let originally_indefinite = Size::new(node_size.width.is_none(), node_size.height.is_none());
    node_size = node_size.or(Size::new(
        Some(measured_border_box.width),
        Some(measured_border_box.height),
    ));
    node_size = apply_measured_aspect_ratio(
        node_size,
        originally_indefinite,
        measurement_axis,
        aspect_ratio,
        box_sizing,
        padding_border_size,
    );

    let size = finalize_size(
        node_size.unwrap_or(measured_border_box),
        input.known_dimensions,
        min_size,
        max_size,
        padding_border_size,
    );

    let content_size = size.zip_map(measured_border_box, f32::max);
    let first_baselines = Point::new(
        measurement
            .first_baselines
            .x
            .map(|baseline| content_origin.x + baseline),
        measurement
            .first_baselines
            .y
            .map(|baseline| content_origin.y + baseline),
    );
    LayoutOutput::new(size, content_size).with_first_baselines(first_baselines)
}

/// Host measurement returned to [`compute_leaf_layout`].
///
/// This contains content-box data only; leaf layout adds box-model surrounds,
/// applies constraints, and converts baselines to border-box coordinates.
///
/// `size` is the measured content-box extent. `first_baselines` contains
/// offsets from the content-box origin. Returning a plain [`Size<f32>`]
/// remains supported through [`From`], for leaves without baseline
/// information.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LeafMeasurement {
    /// Measured content-box size.
    pub size: Size<f32>,
    /// First baseline offsets from the content-box origin.
    pub first_baselines: Point<Option<f32>>,
}

impl LeafMeasurement {
    /// A measurement without baselines.
    #[must_use]
    pub fn new(size: Size<f32>) -> Self {
        Self {
            size,
            first_baselines: Point::NONE,
        }
    }

    /// Adds first baseline offsets.
    #[must_use]
    pub fn with_first_baselines(mut self, first_baselines: Point<Option<f32>>) -> Self {
        self.first_baselines = first_baselines;
        self
    }
}

impl From<Size<f32>> for LeafMeasurement {
    fn from(size: Size<f32>) -> Self {
        Self::new(size)
    }
}

fn apply_measured_aspect_ratio(
    mut size: Size<Option<f32>>,
    originally_indefinite: Size<bool>,
    measurement_axis: Option<RequestedAxis>,
    aspect_ratio: Option<f32>,
    box_sizing: BoxSizing,
    padding_border_size: Size<f32>,
) -> Size<Option<f32>> {
    let Some(ratio) = aspect_ratio else {
        return size;
    };
    if !originally_indefinite.width
        || !originally_indefinite.height
        || !ratio.is_finite()
        || ratio <= 0.0
    {
        return size;
    }

    // With both preferred axes automatic, the inline axis is normally the
    // ratio-determining axis. A vertical-only probe can invert that choice.
    match measurement_axis {
        Some(RequestedAxis::Vertical) => {
            let sizing_height = sizing_box_axis(
                size.height.unwrap_or(0.0),
                padding_border_size.height,
                box_sizing,
            );
            size.width = Some(border_box_axis(
                sizing_height * ratio,
                padding_border_size.width,
                box_sizing,
            ));
        }
        None | Some(RequestedAxis::Horizontal | RequestedAxis::Both) => {
            let sizing_width = sizing_box_axis(
                size.width.unwrap_or(0.0),
                padding_border_size.width,
                box_sizing,
            );
            size.height = Some(border_box_axis(
                sizing_width / ratio,
                padding_border_size.height,
                box_sizing,
            ));
        }
    }
    size
}

/// Resolved box-model and sizing inputs reused throughout one leaf-layout
/// call; all optional dimensions are border-box sizes.
struct LeafSizing {
    margin: Edges<f32>,
    padding_border_size: Size<f32>,
    content_box_inset: Size<f32>,
    content_origin: Point<f32>,
    node_size: Size<Option<f32>>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    box_sizing: BoxSizing,
}

fn resolve_leaf_sizing(
    input: LayoutInput,
    style: &impl CoreStyle,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> LeafSizing {
    let inline_basis = input.parent_size.width;
    let margin = auto_edges_to_zero(resolve_optional_edges(
        style.margin(),
        inline_basis,
        resolve_calc,
    ));
    let padding = resolve_edges(style.padding(), inline_basis, resolve_calc);
    let border = resolve_edges(style.border(), inline_basis, resolve_calc);
    let padding_border_size = Size::new(
        padding.horizontal_sum() + border.horizontal_sum(),
        padding.vertical_sum() + border.vertical_sum(),
    );
    let scrollbars = scrollbar_size(style);
    let content_box_inset = Size::new(
        padding_border_size.width + scrollbars.width,
        padding_border_size.height + scrollbars.height,
    );
    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    let box_sizing = style.box_sizing();

    let (node_size, min_size, max_size, aspect_ratio) = match input.sizing_mode {
        SizingMode::ContentSize => (input.known_dimensions, Size::NONE, Size::NONE, None),
        SizingMode::InherentSize => {
            let aspect_ratio = style.aspect_ratio();

            // Aspect ratio operates on the box selected by box-sizing. The
            // caller's known dimensions are always border-box, so translate
            // them into that sizing box before deriving the opposite axis.
            let style_size = resolve_size(style.size(), input.parent_size, resolve_calc);
            let known_sizing_box =
                border_box_to_sizing_box(input.known_dimensions, box_sizing, padding_border_size);
            let preferred_sizing_box =
                apply_aspect_ratio(known_sizing_box.or(style_size), aspect_ratio);
            let node_size = input.known_dimensions.or(apply_box_sizing(
                preferred_sizing_box,
                box_sizing,
                padding_border_size,
            ));
            let min_size = apply_box_sizing(
                resolve_size(style.min_size(), input.parent_size, resolve_calc),
                box_sizing,
                padding_border_size,
            );
            let max_size = apply_box_sizing(
                resolve_size(style.max_size(), input.parent_size, resolve_calc),
                box_sizing,
                padding_border_size,
            );
            let min_max_definite = min_size.zip_map(max_size, |min, max| match (min, max) {
                (Some(min), Some(max)) if max <= min => Some(min),
                _ => None,
            });

            (
                node_size.or(min_max_definite),
                min_size,
                max_size,
                aspect_ratio,
            )
        }
    };

    LeafSizing {
        margin,
        padding_border_size,
        content_box_inset,
        content_origin,
        node_size,
        min_size,
        max_size,
        aspect_ratio,
        box_sizing,
    }
}

#[inline]
fn border_box_to_sizing_box(
    value: Size<Option<f32>>,
    box_sizing: BoxSizing,
    padding_border_size: Size<f32>,
) -> Size<Option<f32>> {
    Size::new(
        value
            .width
            .map(|value| sizing_box_axis(value, padding_border_size.width, box_sizing)),
        value
            .height
            .map(|value| sizing_box_axis(value, padding_border_size.height, box_sizing)),
    )
}

#[inline]
fn sizing_box_axis(value: f32, padding_border: f32, box_sizing: BoxSizing) -> f32 {
    if box_sizing == BoxSizing::ContentBox {
        (value - padding_border).max(0.0)
    } else {
        value
    }
}

#[inline]
fn border_box_axis(value: f32, padding_border: f32, box_sizing: BoxSizing) -> f32 {
    if box_sizing == BoxSizing::ContentBox {
        value + padding_border
    } else {
        value
    }
}

fn clamp_resolved_size(
    value: Size<Option<f32>>,
    known_dimensions: Size<Option<f32>>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    floor: Size<f32>,
) -> Size<Option<f32>> {
    Size::new(
        value.width.map(|value| {
            if known_dimensions.width.is_some() {
                value
            } else {
                clamp(value, min_size.width, max_size.width).max(floor.width)
            }
        }),
        value.height.map(|value| {
            if known_dimensions.height.is_some() {
                value
            } else {
                clamp(value, min_size.height, max_size.height).max(floor.height)
            }
        }),
    )
}

fn measurement_available_space(
    known_content_size: Option<f32>,
    available_space: AvailableSpace,
    margin: f32,
    content_box_inset: f32,
    min_size: Option<f32>,
    max_size: Option<f32>,
) -> AvailableSpace {
    if let Some(known) = known_content_size {
        return AvailableSpace::Definite(known);
    }

    let outer = subtract_available_space(available_space, margin);
    let constrained = match outer {
        AvailableSpace::Definite(value) => {
            AvailableSpace::Definite(clamp(value, min_size, max_size))
        }
        intrinsic => intrinsic,
    };
    subtract_available_space(constrained, content_box_inset)
}

#[inline]
fn finalize_size(
    candidate: Size<f32>,
    known_dimensions: Size<Option<f32>>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    padding_border_floor: Size<f32>,
) -> Size<f32> {
    Size::new(
        known_dimensions.width.unwrap_or_else(|| {
            clamp(candidate.width, min_size.width, max_size.width).max(padding_border_floor.width)
        }),
        known_dimensions.height.unwrap_or_else(|| {
            clamp(candidate.height, min_size.height, max_size.height)
                .max(padding_border_floor.height)
        }),
    )
}
