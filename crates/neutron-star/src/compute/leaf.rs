//! Leaf layout for the engine's two closed content paths.
//!
//! Replaced content (currently images) supplies an already-decoded
//! [`NaturalSize`]. Text is measured by neutron-star's concrete Parley engine
//! and enters the same private box-layout routine. There is deliberately no
//! host measurement trait or callback in the public protocol: arbitrary host
//! content is not a supported leaf kind.

use stylo::computed_values::box_sizing;

use super::util::{
    apply_aspect_ratio, apply_box_sizing, auto_edges_to_zero, clamp, resolve_border,
    resolve_margins, resolve_max_sizes, resolve_padding, resolve_size, subtract_available_space,
    used_aspect_ratio,
};
use crate::geometry::{Edges, Point, Size};
use crate::style::CoreStyle;
use crate::tree::{
    AvailableSpace, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis, SizingMode,
};

/// Sizes replaced content from its decoded natural dimensions and ratio.
///
/// The returned size applies, in order: known dimensions verbatim; style
/// size/aspect-ratio (per [`SizingMode`](crate::tree::SizingMode)); natural
/// content size; min/max clamps; and a padding+border floor on axes the
/// engine resolves itself. Caller-supplied known dimensions remain verbatim.
/// `content_size` is the border-origin scrollable extent, including measured
/// overflow.
pub fn compute_leaf_layout<Style: CoreStyle>(
    input: LayoutInput,
    style: &Style,
    natural_size: NaturalSize,
) -> LayoutOutput {
    compute_leaf_layout_with_measurement(
        input,
        style,
        natural_size.used_aspect_ratio(),
        |measure_input| natural_size.measure(measure_input),
    )
}

/// Shared box-model routine used only by the closed natural-size and Parley
/// content paths.
#[allow(clippy::too_many_lines)]
pub(crate) fn compute_leaf_layout_with_measurement<Style, Measure>(
    input: LayoutInput,
    style: &Style,
    natural_aspect_ratio: Option<f32>,
    mut measure: Measure,
) -> LayoutOutput
where
    Style: CoreStyle,
    Measure: FnMut(LeafMeasureInput) -> LeafMetrics,
{
    let measurement_axis = match input.goal {
        LayoutGoal::Measure(axis) => Some(axis),
        LayoutGoal::Commit => None,
    };

    let LeafSizing {
        margin,
        padding_border_size,
        content_origin,
        mut node_size,
        min_size,
        max_size,
        aspect_ratio,
        box_sizing,
    } = resolve_leaf_sizing(input, style, natural_aspect_ratio);

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
            .map(|width| (width - padding_border_size.width).max(0.0)),
        node_size
            .height
            .map(|height| (height - padding_border_size.height).max(0.0)),
    );
    let available_space = Size::new(
        measurement_available_space(
            measure_known_dimensions.width,
            input.available_space.width,
            margin.horizontal_sum(),
            padding_border_size.width,
            min_size.width,
            max_size.width,
        ),
        measurement_available_space(
            measure_known_dimensions.height,
            input.available_space.height,
            margin.vertical_sum(),
            padding_border_size.height,
            min_size.height,
            max_size.height,
        ),
    );

    // Size containment (`contain: size`/`strict`, `content-visibility`) sizes
    // the leaf **as if it had no content**: the measurer is never called and
    // `contain-intrinsic-{width,height}` (both `None` ⇒ zero, collapsing to
    // border + padding) stands in for the measured content. Baselines are
    // therefore absent, matching layout containment.
    let measurement = if let Some(intrinsic) = crate::style::containment::size_containment(style) {
        LeafMetrics {
            size: Size::new(
                intrinsic.width.unwrap_or(0.0),
                intrinsic.height.unwrap_or(0.0),
            ),
            first_baselines: Point::NONE,
        }
    } else {
        measure(LeafMeasureInput::new(
            measure_known_dimensions,
            available_space,
            input.goal,
        ))
    };
    let measured_content = measurement.size;
    debug_assert!(
        measured_content.width.is_finite() && measured_content.height.is_finite(),
        "leaf measurements must be finite"
    );

    let measured_border_box = Size::new(
        measured_content.width.max(0.0) + padding_border_size.width,
        measured_content.height.max(0.0) + padding_border_size.height,
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

/// Content-box constraints consumed by neutron-star's closed leaf engines.
///
/// The dimensions have already been translated through the leaf's box model:
/// known dimensions are content-box extents, and available space excludes its
/// margins, padding, and borders. [`Self::goal`]
/// lets the Parley path distinguish a transient probe from the committed
/// layout whose rich artifact remains available for painting.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct LeafMeasureInput {
    /// Content-box dimensions already decided by the caller.
    pub known_dimensions: Size<Option<f32>>,
    /// Space available to unconstrained content-box axes.
    pub available_space: Size<AvailableSpace>,
    /// Whether this is a sizing probe or a geometry commit.
    pub goal: LayoutGoal,
}

impl LeafMeasureInput {
    /// Creates a leaf-measurement request.
    #[must_use]
    pub const fn new(
        known_dimensions: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
        goal: LayoutGoal,
    ) -> Self {
        Self {
            known_dimensions,
            available_space,
            goal,
        }
    }
}

/// Decoded intrinsic data for replaced content.
///
/// The dimensions are content-box CSS pixels. Either dimension may be absent
/// (for example, ratio-only vector content); `aspect_ratio` is width / height.
/// Invalid or negative values are discarded by the constructors. `NaturalSize::NONE`
/// represents content whose metadata has not loaded yet and therefore
/// contributes zero intrinsic size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct NaturalSize {
    /// Natural width and height, independently optional.
    dimensions: Size<Option<f32>>,
    /// Natural width / height ratio.
    aspect_ratio: Option<f32>,
}

impl NaturalSize {
    /// No decoded natural dimensions or ratio.
    pub const NONE: Self = Self {
        dimensions: Size::NONE,
        aspect_ratio: None,
    };

    /// Creates natural data from independently optional dimensions and ratio.
    #[must_use]
    pub fn new(dimensions: Size<Option<f32>>, aspect_ratio: Option<f32>) -> Self {
        Self {
            dimensions: dimensions.map(sanitize_dimension),
            aspect_ratio: sanitize_ratio(aspect_ratio),
        }
    }

    /// Creates natural data from a decoded two-dimensional size.
    #[must_use]
    pub fn from_size(size: Size<f32>) -> Self {
        Self::new(
            Size::new(Some(size.width), Some(size.height)),
            ratio_from_dimensions(size.width, size.height),
        )
    }

    /// Returns the sanitized optional natural dimensions.
    #[must_use]
    pub const fn dimensions(self) -> Size<Option<f32>> {
        self.dimensions
    }

    /// Returns the sanitized natural width / height ratio.
    #[must_use]
    pub const fn aspect_ratio(self) -> Option<f32> {
        self.aspect_ratio
    }

    fn used_dimensions(self) -> Size<Option<f32>> {
        self.dimensions
    }

    fn used_aspect_ratio(self) -> Option<f32> {
        self.aspect_ratio
    }

    fn measure(self, input: LeafMeasureInput) -> LeafMetrics {
        let natural = self.used_dimensions();
        let ratio = self.used_aspect_ratio();
        let size = match input.known_dimensions {
            Size {
                width: Some(width),
                height: Some(height),
            } => Size::new(width, height),
            Size {
                width: Some(width),
                height: None,
            } => Size::new(
                width,
                ratio
                    .map(|ratio| width / ratio)
                    .or(natural.height)
                    .unwrap_or(0.0),
            ),
            Size {
                width: None,
                height: Some(height),
            } => Size::new(
                ratio
                    .map(|ratio| height * ratio)
                    .or(natural.width)
                    .unwrap_or(0.0),
                height,
            ),
            Size {
                width: None,
                height: None,
            } => match natural {
                Size {
                    width: Some(width),
                    height: Some(height),
                } => Size::new(width, height),
                Size {
                    width: Some(width),
                    height: None,
                } => Size::new(width, ratio.map_or(0.0, |ratio| width / ratio)),
                Size {
                    width: None,
                    height: Some(height),
                } => Size::new(ratio.map_or(0.0, |ratio| height * ratio), height),
                Size {
                    width: None,
                    height: None,
                } => Size::ZERO,
            },
        };
        LeafMetrics::new(size)
    }
}

impl From<Size<f32>> for NaturalSize {
    fn from(size: Size<f32>) -> Self {
        Self::from_size(size)
    }
}

fn sanitize_dimension(value: Option<f32>) -> Option<f32> {
    value.filter(|value| value.is_finite() && *value >= 0.0)
}

fn sanitize_ratio(value: Option<f32>) -> Option<f32> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

fn ratio_from_dimensions(width: f32, height: f32) -> Option<f32> {
    sanitize_ratio((height > 0.0).then_some(width / height))
}

/// Metrics produced by neutron-star's concrete content engines.
///
/// This contains content-box data only; leaf layout adds box-model surrounds,
/// applies constraints, and converts baselines to border-box coordinates.
///
/// `size` is the measured content-box extent. `first_baselines` contains
/// offsets from the content-box origin. Natural-size leaves do not report a
/// baseline; Parley text does.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct LeafMetrics {
    /// Measured content-box size.
    pub size: Size<f32>,
    /// First baseline offsets from the content-box origin.
    pub first_baselines: Point<Option<f32>>,
}

impl LeafMetrics {
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

impl From<Size<f32>> for LeafMetrics {
    fn from(size: Size<f32>) -> Self {
        Self::new(size)
    }
}

fn apply_measured_aspect_ratio(
    mut size: Size<Option<f32>>,
    originally_indefinite: Size<bool>,
    measurement_axis: Option<RequestedAxis>,
    aspect_ratio: Option<f32>,
    box_sizing: box_sizing::T,
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
    content_origin: Point<f32>,
    node_size: Size<Option<f32>>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    box_sizing: box_sizing::T,
}

fn resolve_leaf_sizing(
    input: LayoutInput,
    style: &impl CoreStyle,
    natural_aspect_ratio: Option<f32>,
) -> LeafSizing {
    let inline_basis = input.parent_size.width;
    let margin = auto_edges_to_zero(resolve_margins(style.margin(), inline_basis));
    let padding = resolve_padding(style.padding(), inline_basis);
    let border = resolve_border(&style.border());
    let padding_border_size = Size::new(
        padding.horizontal_sum() + border.horizontal_sum(),
        padding.vertical_sum() + border.vertical_sum(),
    );
    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    let box_sizing = style.box_sizing();

    let (node_size, min_size, max_size, aspect_ratio) = match input.sizing_mode {
        SizingMode::ContentSize => (input.known_dimensions, Size::NONE, Size::NONE, None),
        SizingMode::InherentSize => {
            let aspect_ratio = preferred_aspect_ratio(style.aspect_ratio(), natural_aspect_ratio);

            // Aspect ratio operates on the box selected by box-sizing. The
            // caller's known dimensions are always border-box, so translate
            // them into that sizing box before deriving the opposite axis.
            let style_size = resolve_size(style.size(), input.parent_size);
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
                resolve_size(style.min_size(), input.parent_size),
                box_sizing,
                padding_border_size,
            );
            let max_size = apply_box_sizing(
                resolve_max_sizes(style.max_size(), input.parent_size),
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
        content_origin,
        node_size,
        min_size,
        max_size,
        aspect_ratio,
        box_sizing,
    }
}

fn preferred_aspect_ratio(
    value: stylo::values::computed::AspectRatio,
    natural_aspect_ratio: Option<f32>,
) -> Option<f32> {
    let specified = used_aspect_ratio(value);
    if value.auto {
        sanitize_ratio(natural_aspect_ratio).or(specified)
    } else {
        specified
    }
}

#[inline]
fn border_box_to_sizing_box(
    value: Size<Option<f32>>,
    box_sizing: box_sizing::T,
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
fn sizing_box_axis(value: f32, padding_border: f32, box_sizing: box_sizing::T) -> f32 {
    if box_sizing == box_sizing::T::ContentBox {
        (value - padding_border).max(0.0)
    } else {
        value
    }
}

#[inline]
fn border_box_axis(value: f32, padding_border: f32, box_sizing: box_sizing::T) -> f32 {
    if box_sizing == box_sizing::T::ContentBox {
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests {
    use core::cell::RefCell;

    use stylo::values::computed::{Display, Length, LengthPercentage, MaxSize, Size as StyleSize};
    use stylo::values::generics::NonNegative;

    use super::*;
    use crate::cache::Cache;
    use crate::compute::compute_cached_layout;
    use crate::tree::{Layout, LayoutNode};

    #[derive(Default)]
    struct EmptyStyle;

    impl CoreStyle for EmptyStyle {
        fn display(&self) -> Display {
            Display::Flex
        }
    }

    struct RetainedArtifact {
        metrics: LeafMetrics,
        paint_data: Vec<u8>,
    }

    #[derive(Default)]
    struct ArtifactCache {
        committed: Option<RetainedArtifact>,
        probe: Option<RetainedArtifact>,
        shape_calls: usize,
    }

    struct CachingMeasurer<'a> {
        artifacts: &'a mut ArtifactCache,
    }

    impl CachingMeasurer<'_> {
        fn measure(&mut self, input: LeafMeasureInput) -> LeafMetrics {
            self.artifacts.shape_calls += 1;
            let (slot, paint_tag) = match input.goal {
                LayoutGoal::Commit => (&mut self.artifacts.committed, b'C'),
                LayoutGoal::Measure(_) => (&mut self.artifacts.probe, b'P'),
            };
            *slot = Some(RetainedArtifact {
                metrics: LeafMetrics::new(Size::new(40.0, 12.0)),
                paint_data: vec![paint_tag],
            });
            slot.as_ref().expect("artifact was just populated").metrics
        }
    }

    /// A one-leaf test host: the box cache and the retained shaping
    /// artifacts live in host-owned interior-mutable slots reached through a
    /// `Copy` node handle.
    #[derive(Default)]
    struct LeafHostState {
        box_cache: RefCell<Cache>,
        artifacts: RefCell<ArtifactCache>,
    }

    impl LeafHostState {
        fn invalidate_content(&self) {
            self.box_cache.borrow_mut().clear();
            let mut artifacts = self.artifacts.borrow_mut();
            artifacts.committed = None;
            artifacts.probe = None;
        }
    }

    #[derive(Clone, Copy)]
    struct LeafRef<'t> {
        host: &'t LeafHostState,
    }

    impl core::fmt::Debug for LeafRef<'_> {
        fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            formatter.write_str("LeafRef")
        }
    }

    impl LayoutNode for LeafRef<'_> {
        type Style = &'static EmptyStyle;
        type ChildIter = core::iter::Empty<Self>;

        fn children(self) -> Self::ChildIter {
            core::iter::empty()
        }

        fn style(self) -> Self::Style {
            &EmptyStyle
        }

        fn compute_child_layout(self, _input: LayoutInput) -> LayoutOutput {
            unreachable!("leaf tests drive compute_cached_layout directly")
        }

        fn set_unrounded_layout(self, _layout: Layout) {
            unreachable!("leaf tests store no durable geometry")
        }

        fn with_unrounded_layout<R>(self, _read: impl FnOnce(&Layout) -> R) -> R {
            unreachable!("leaf tests store no durable geometry")
        }

        fn set_final_layout(self, _layout: Layout) {
            unreachable!("leaf tests store no durable geometry")
        }

        fn set_static_position(self, _static_position: Point<f32>) {
            unreachable!("leaf tests store no durable geometry")
        }

        fn cache_get(self, input: LayoutInput) -> Option<LayoutOutput> {
            self.host.box_cache.borrow().get(input)
        }

        fn cache_store(self, input: LayoutInput, output: LayoutOutput) {
            self.host.box_cache.borrow_mut().store(input, output);
        }

        fn cache_clear(self) {
            self.host.invalidate_content();
        }
    }

    #[test]
    fn committed_artifact_survives_probes_and_box_cache_hits() {
        let host = LeafHostState::default();
        let node = LeafRef { host: &host };
        let commit_input = LayoutInput::perform_layout(Size::NONE, Size::NONE, Size::MAX_CONTENT);

        let committed = compute_cached_layout(node, commit_input, |node, input| {
            let mut artifacts = node.host.artifacts.borrow_mut();
            let mut measurer = CachingMeasurer {
                artifacts: &mut artifacts,
            };
            compute_leaf_layout_with_measurement(input, &EmptyStyle, None, |input| {
                measurer.measure(input)
            })
        });
        assert_eq!(host.artifacts.borrow().shape_calls, 1);
        assert_eq!(
            host.artifacts
                .borrow()
                .committed
                .as_ref()
                .expect("commit must retain a paint artifact")
                .paint_data,
            [b'C']
        );

        let probe_input = LayoutInput::compute_size(
            Size::NONE,
            Size::NONE,
            Size::MAX_CONTENT,
            RequestedAxis::Both,
        );
        let probe = {
            let mut artifacts = host.artifacts.borrow_mut();
            let mut probe_measurer = CachingMeasurer {
                artifacts: &mut artifacts,
            };
            compute_leaf_layout_with_measurement(probe_input, &EmptyStyle, None, |input| {
                probe_measurer.measure(input)
            })
        };
        assert_eq!(probe.size, committed.size);
        assert_eq!(host.artifacts.borrow().shape_calls, 2);
        assert_eq!(
            host.artifacts
                .borrow()
                .committed
                .as_ref()
                .expect("probe must not evict the committed artifact")
                .paint_data,
            [b'C']
        );
        assert_eq!(
            host.artifacts
                .borrow()
                .probe
                .as_ref()
                .expect("probe artifact must use its own slot")
                .paint_data,
            [b'P']
        );

        let cached = compute_cached_layout(node, commit_input, |_node, _input| {
            panic!("committed cache hit must skip shaping")
        });
        assert_eq!(cached, committed);
        assert_eq!(host.artifacts.borrow().shape_calls, 2);
        assert!(host.artifacts.borrow().committed.is_some());

        host.invalidate_content();
        assert!(host.box_cache.borrow().is_empty());
        assert!(host.artifacts.borrow().committed.is_none());
        assert!(host.artifacts.borrow().probe.is_none());
    }

    #[test]
    fn natural_size_supplies_replaced_content_without_a_host_callback() {
        let output = compute_leaf_layout(
            LayoutInput::default(),
            &EmptyStyle,
            NaturalSize::from_size(Size::new(23.0, 9.0)),
        );

        assert_eq!(output.size, Size::new(23.0, 9.0));
        assert_eq!(output.first_baselines, Point::NONE);
    }

    #[test]
    fn natural_size_derives_a_missing_axis_and_rejects_invalid_data() {
        let ratio_only = NaturalSize::new(Size::new(Some(40.0), None), Some(2.0));
        assert_eq!(
            ratio_only.measure(LeafMeasureInput::default()).size,
            Size::new(40.0, 20.0)
        );
        let invalid = NaturalSize::new(Size::new(Some(f32::NAN), Some(-1.0)), Some(0.0));
        assert_eq!(
            invalid.measure(LeafMeasureInput::default()).size,
            Size::ZERO
        );
    }

    #[test]
    fn measured_aspect_ratio_respects_requested_axis_and_sizing_box() {
        let both_indefinite = Size::new(true, true);
        let padding_border = Size::new(10.0, 10.0);

        let vertical = apply_measured_aspect_ratio(
            Size::new(Some(40.0), Some(50.0)),
            both_indefinite,
            Some(RequestedAxis::Vertical),
            Some(2.0),
            box_sizing::T::ContentBox,
            padding_border,
        );
        assert_eq!(vertical, Size::new(Some(90.0), Some(50.0)));

        let horizontal = apply_measured_aspect_ratio(
            Size::new(Some(100.0), Some(20.0)),
            both_indefinite,
            Some(RequestedAxis::Both),
            Some(2.0),
            box_sizing::T::ContentBox,
            padding_border,
        );
        assert_eq!(horizontal, Size::new(Some(100.0), Some(55.0)));

        let border_box = apply_measured_aspect_ratio(
            Size::new(Some(100.0), Some(20.0)),
            both_indefinite,
            None,
            Some(2.0),
            box_sizing::T::BorderBox,
            padding_border,
        );
        assert_eq!(border_box, Size::new(Some(100.0), Some(50.0)));

        let unchanged = Size::new(Some(30.0), Some(40.0));
        assert_eq!(
            apply_measured_aspect_ratio(
                unchanged,
                Size::new(false, true),
                None,
                Some(2.0),
                box_sizing::T::ContentBox,
                padding_border,
            ),
            unchanged
        );
        assert_eq!(
            apply_measured_aspect_ratio(
                unchanged,
                both_indefinite,
                None,
                Some(0.0),
                box_sizing::T::ContentBox,
                padding_border,
            ),
            unchanged
        );
    }

    struct ConflictingMinMaxStyle {
        min_size: Size<StyleSize>,
        max_size: Size<MaxSize>,
    }

    fn size_px(value: f32) -> StyleSize {
        StyleSize::LengthPercentage(NonNegative(LengthPercentage::new_length(Length::new(
            value,
        ))))
    }

    fn max_px(value: f32) -> MaxSize {
        MaxSize::LengthPercentage(NonNegative(LengthPercentage::new_length(Length::new(
            value,
        ))))
    }

    impl ConflictingMinMaxStyle {
        fn new() -> Self {
            Self {
                min_size: Size::new(size_px(80.0), size_px(70.0)),
                max_size: Size::new(max_px(40.0), max_px(90.0)),
            }
        }
    }

    impl CoreStyle for ConflictingMinMaxStyle {
        fn display(&self) -> Display {
            Display::Flex
        }

        fn min_size(&self) -> Size<&StyleSize> {
            self.min_size.as_ref()
        }

        fn max_size(&self) -> Size<&MaxSize> {
            self.max_size.as_ref()
        }
    }

    #[test]
    fn sizing_helpers_honor_min_precedence_known_dimensions_and_box_sizing() {
        let sizing =
            resolve_leaf_sizing(LayoutInput::default(), &ConflictingMinMaxStyle::new(), None);
        assert_eq!(sizing.node_size, Size::new(Some(80.0), None));

        assert_eq!(
            border_box_to_sizing_box(
                Size::new(Some(30.0), Some(20.0)),
                box_sizing::T::ContentBox,
                Size::new(8.0, 6.0),
            ),
            Size::new(Some(22.0), Some(14.0))
        );
        assert_eq!(sizing_box_axis(30.0, 8.0, box_sizing::T::BorderBox), 30.0);
        assert_eq!(border_box_axis(22.0, 8.0, box_sizing::T::ContentBox), 30.0);
        assert_eq!(border_box_axis(22.0, 8.0, box_sizing::T::BorderBox), 22.0);

        assert_eq!(
            clamp_resolved_size(
                Size::new(Some(5.0), Some(50.0)),
                Size::new(Some(5.0), None),
                Size::new(Some(20.0), Some(10.0)),
                Size::new(Some(30.0), Some(40.0)),
                Size::new(10.0, 12.0),
            ),
            Size::new(Some(5.0), Some(40.0))
        );
    }

    #[test]
    fn cache_clear_capability_invalidates_box_and_retained_artifacts_together() {
        let host = LeafHostState::default();
        host.artifacts.borrow_mut().committed = Some(RetainedArtifact {
            metrics: LeafMetrics::new(Size::new(1.0, 1.0)),
            paint_data: vec![b'C'],
        });
        host.artifacts.borrow_mut().probe = Some(RetainedArtifact {
            metrics: LeafMetrics::new(Size::new(1.0, 1.0)),
            paint_data: vec![b'P'],
        });

        LayoutNode::cache_clear(LeafRef { host: &host });

        assert!(host.box_cache.borrow().is_empty());
        assert!(host.artifacts.borrow().committed.is_none());
        assert!(host.artifacts.borrow().probe.is_none());
    }
}
