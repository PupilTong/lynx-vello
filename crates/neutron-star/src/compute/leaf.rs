//! Leaf layout for the engine's two closed content paths.

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

pub fn compute_leaf_layout<Style: CoreStyle>(
    input: LayoutInput,
    style: &Style,
    natural_size: NaturalSize,
) -> LayoutOutput {
    compute_leaf_layout_with_measurement(
        input,
        style,
        natural_size.aspect_ratio(),
        false,
        |measure_input| natural_size.measure(measure_input),
    )
}

#[allow(clippy::too_many_lines)]
pub(crate) fn compute_leaf_layout_with_measurement<Style, Measure>(
    input: LayoutInput,
    style: &Style,
    natural_aspect_ratio: Option<f32>,
    requires_known_measurement: bool,
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
    } = resolve_leaf_sizing(input, style, natural_aspect_ratio);

    node_size = clamp_resolved_size(
        node_size,
        input.known_dimensions,
        min_size,
        max_size,
        padding_border_size,
    );

    let contained_intrinsic = crate::style::containment::size_containment(style);
    if (measurement_axis.is_some_and(|axis| axis != RequestedAxis::Both)
        || (!requires_known_measurement && contained_intrinsic.is_none()))
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

    let measurement = if let Some(intrinsic) = contained_intrinsic {
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

#[cfg(feature = "layout-test-utils")]
#[doc(hidden)]
pub fn compute_leaf_layout_with_measurement_for_testing<Style, Measure>(
    input: LayoutInput,
    style: &Style,
    natural_aspect_ratio: Option<f32>,
    measure: Measure,
) -> LayoutOutput
where
    Style: CoreStyle,
    Measure: FnMut(LeafMeasureInput) -> LeafMetrics,
{
    compute_leaf_layout_with_measurement(input, style, natural_aspect_ratio, true, measure)
}

/// Content-box constraints consumed by neutron-star's closed leaf engines.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct LeafMeasureInput {
    pub known_dimensions: Size<Option<f32>>,
    pub available_space: Size<AvailableSpace>,
    pub goal: LayoutGoal,
}

impl LeafMeasureInput {
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
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct NaturalSize {
    dimensions: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
}

impl NaturalSize {
    pub const NONE: Self = Self {
        dimensions: Size::NONE,
        aspect_ratio: None,
    };

    #[must_use]
    pub fn new(dimensions: Size<Option<f32>>, aspect_ratio: Option<f32>) -> Self {
        Self {
            dimensions: dimensions.map(sanitize_dimension),
            aspect_ratio: sanitize_ratio(aspect_ratio),
        }
    }

    #[must_use]
    pub fn from_size(size: Size<f32>) -> Self {
        Self::new(
            Size::new(Some(size.width), Some(size.height)),
            ratio_from_dimensions(size.width, size.height),
        )
    }

    #[must_use]
    pub const fn dimensions(self) -> Size<Option<f32>> {
        self.dimensions
    }

    #[must_use]
    pub const fn aspect_ratio(self) -> Option<f32> {
        self.aspect_ratio
    }

    fn measure(self, input: LeafMeasureInput) -> LeafMetrics {
        let natural = self.dimensions();
        let ratio = self.aspect_ratio();
        let size = apply_aspect_ratio(input.known_dimensions, ratio)
            .or(apply_aspect_ratio(natural, ratio))
            .unwrap_or(Size::ZERO);
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
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct LeafMetrics {
    pub size: Size<f32>,
    pub first_baselines: Point<Option<f32>>,
}

impl LeafMetrics {
    #[must_use]
    pub fn new(size: Size<f32>) -> Self {
        Self {
            size,
            first_baselines: Point::NONE,
        }
    }

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
    aspect_ratio: PreferredAspectRatio,
    padding_border_size: Size<f32>,
) -> Size<Option<f32>> {
    let Some((ratio, sizing_box)) = aspect_ratio.components() else {
        return size;
    };
    if !originally_indefinite.width
        || !originally_indefinite.height
        || !ratio.is_finite()
        || ratio <= 0.0
    {
        return size;
    }

    match measurement_axis {
        Some(RequestedAxis::Vertical) => {
            let sizing_height = sizing_box_axis(
                size.height.unwrap_or(0.0),
                padding_border_size.height,
                sizing_box,
            );
            size.width = Some(border_box_axis(
                sizing_height * ratio,
                padding_border_size.width,
                sizing_box,
            ));
        }
        None | Some(RequestedAxis::Horizontal | RequestedAxis::Both) => {
            let sizing_width = sizing_box_axis(
                size.width.unwrap_or(0.0),
                padding_border_size.width,
                sizing_box,
            );
            size.height = Some(border_box_axis(
                sizing_width / ratio,
                padding_border_size.height,
                sizing_box,
            ));
        }
    }
    size
}

/// A preferred ratio together with the box whose width/height it relates.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PreferredAspectRatio(f32);

impl PreferredAspectRatio {
    const NONE: Self = Self(0.0);

    fn content_box(ratio: f32) -> Self {
        debug_assert!(ratio.is_finite() && ratio > 0.0);
        Self(ratio)
    }

    fn border_box(ratio: f32) -> Self {
        debug_assert!(ratio.is_finite() && ratio > 0.0);
        Self(-ratio)
    }

    #[inline]
    fn components(self) -> Option<(f32, box_sizing::T)> {
        if self.0 > 0.0 {
            Some((self.0, box_sizing::T::ContentBox))
        } else if self.0 < 0.0 {
            Some((-self.0, box_sizing::T::BorderBox))
        } else {
            None
        }
    }
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
    aspect_ratio: PreferredAspectRatio,
}

#[inline(always)]
#[allow(
    clippy::inline_always,
    reason = "keeps the compact leaf sizing state in its caller on the measured hot path"
)]
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
        SizingMode::IgnoreSizeStyles => (
            input.known_dimensions,
            Size::NONE,
            Size::NONE,
            PreferredAspectRatio::NONE,
        ),
        SizingMode::ApplySizeStyles => {
            let aspect_ratio =
                preferred_aspect_ratio(style.aspect_ratio(), natural_aspect_ratio, box_sizing);

            let style_size = resolve_size(style.size(), input.parent_size);
            let preferred_border_box = input.known_dimensions.or(apply_box_sizing(
                style_size,
                box_sizing,
                padding_border_size,
            ));
            let ratio_applied_border_box =
                aspect_ratio
                    .components()
                    .map_or(preferred_border_box, |(ratio, sizing_box)| {
                        let ratio_box = border_box_to_sizing_box(
                            preferred_border_box,
                            sizing_box,
                            padding_border_size,
                        );
                        apply_box_sizing(
                            apply_aspect_ratio(ratio_box, Some(ratio)),
                            sizing_box,
                            padding_border_size,
                        )
                    });
            let node_size = input.known_dimensions.or(ratio_applied_border_box);
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
    }
}

fn preferred_aspect_ratio(
    value: stylo::values::computed::AspectRatio,
    natural_aspect_ratio: Option<f32>,
    box_sizing: box_sizing::T,
) -> PreferredAspectRatio {
    let specified = used_aspect_ratio(value);
    let ratio = if value.auto {
        natural_aspect_ratio.or(specified)
    } else {
        specified
    };
    let Some(ratio) = ratio else {
        return PreferredAspectRatio::NONE;
    };
    if value.auto || box_sizing == box_sizing::T::ContentBox {
        PreferredAspectRatio::content_box(ratio)
    } else {
        PreferredAspectRatio::border_box(ratio)
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
    use stylo::values::computed::{
        AspectRatio, Display, Length, LengthPercentage, MaxSize, NonNegativeLengthPercentage,
        Size as StyleSize,
    };
    use stylo::values::generics::NonNegative;
    use stylo::values::generics::position::PreferredRatio;
    use stylo::values::generics::ratio::Ratio;

    use super::*;
    use crate::compute::compute_cached_layout;
    use crate::tree::{LayoutSlot, LayoutTree};

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

    #[derive(Default)]
    struct LeafHostState {
        layout: LayoutSlot,
        artifacts: ArtifactCache,
    }

    impl LeafHostState {
        fn invalidate_content(&mut self) {
            self.layout.clear_layout_cache();
            self.artifacts.committed = None;
            self.artifacts.probe = None;
        }
    }

    #[derive(Debug, Default)]
    struct LeafTree;

    impl LayoutTree for LeafTree {
        type NodeId = ();
        type State = LeafHostState;
        type Style<'tree> = &'static EmptyStyle;
        type ChildIter<'tree> = core::iter::Empty<()>;

        fn children(&self, _node: ()) -> Self::ChildIter<'_> {
            core::iter::empty()
        }

        fn style(&self, _node: ()) -> Self::Style<'_> {
            &EmptyStyle
        }

        fn layout<'state>(&self, state: &'state Self::State, _node: ()) -> &'state LayoutSlot {
            &state.layout
        }

        fn layout_mut<'state>(
            &self,
            state: &'state mut Self::State,
            _node: (),
        ) -> &'state mut LayoutSlot {
            &mut state.layout
        }

        fn compute_layout(
            &self,
            _state: &mut Self::State,
            _node: (),
            _input: LayoutInput,
        ) -> LayoutOutput {
            unreachable!("leaf tests drive compute_cached_layout directly")
        }

        fn clear_layout_cache(&self, state: &mut Self::State, _node: ()) {
            state.invalidate_content();
        }
    }

    #[test]
    fn committed_artifact_survives_probes_and_box_cache_hits() {
        let tree = LeafTree;
        let mut state = LeafHostState::default();
        let commit_input = LayoutInput::commit(Size::NONE, Size::NONE, Size::MAX_CONTENT);

        let committed = compute_cached_layout(
            &tree,
            &mut state,
            (),
            commit_input,
            |_tree, state, (), input| {
                let mut measurer = CachingMeasurer {
                    artifacts: &mut state.artifacts,
                };
                compute_leaf_layout_with_measurement(input, &EmptyStyle, None, true, |input| {
                    measurer.measure(input)
                })
            },
        );
        assert_eq!(state.artifacts.shape_calls, 1);
        assert_eq!(
            state
                .artifacts
                .committed
                .as_ref()
                .expect("commit must retain a paint artifact")
                .paint_data,
            [b'C']
        );

        let probe_input = LayoutInput::measure(
            Size::NONE,
            Size::NONE,
            Size::MAX_CONTENT,
            RequestedAxis::Both,
        );
        let probe = {
            let mut probe_measurer = CachingMeasurer {
                artifacts: &mut state.artifacts,
            };
            compute_leaf_layout_with_measurement(probe_input, &EmptyStyle, None, true, |input| {
                probe_measurer.measure(input)
            })
        };
        assert_eq!(probe.size, committed.size);
        assert_eq!(state.artifacts.shape_calls, 2);
        assert_eq!(
            state
                .artifacts
                .committed
                .as_ref()
                .expect("probe must not evict the committed artifact")
                .paint_data,
            [b'C']
        );
        assert_eq!(
            state
                .artifacts
                .probe
                .as_ref()
                .expect("probe artifact must use its own slot")
                .paint_data,
            [b'P']
        );

        let cached = compute_cached_layout(
            &tree,
            &mut state,
            (),
            commit_input,
            |_tree, _state, (), _input| panic!("committed cache hit must skip shaping"),
        );
        assert_eq!(cached, committed);
        assert_eq!(state.artifacts.shape_calls, 2);
        assert!(state.artifacts.committed.is_some());

        state.invalidate_content();
        assert!(state.layout.layout_cache_is_empty());
        assert!(state.artifacts.committed.is_none());
        assert!(state.artifacts.probe.is_none());
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
        let natural_height = NaturalSize::new(Size::new(None, Some(11.0)), None);
        assert_eq!(
            natural_height
                .measure(LeafMeasureInput::new(
                    Size::new(Some(22.0), None),
                    Size::MAX_CONTENT,
                    LayoutGoal::Commit,
                ))
                .size,
            Size::new(22.0, 11.0)
        );
        let invalid = NaturalSize::new(Size::new(Some(f32::NAN), Some(-1.0)), Some(0.0));
        assert_eq!(
            invalid.measure(LeafMeasureInput::default()).size,
            Size::ZERO
        );
    }

    struct BoxStyle {
        size: Size<StyleSize>,
        padding: Edges<NonNegativeLengthPercentage>,
        box_sizing: box_sizing::T,
        aspect_ratio: AspectRatio,
    }

    impl BoxStyle {
        fn new(size: Size<StyleSize>) -> Self {
            Self {
                size,
                padding: Edges::uniform(NonNegative(LengthPercentage::new_length(Length::new(
                    0.0,
                )))),
                box_sizing: box_sizing::T::ContentBox,
                aspect_ratio: AspectRatio::auto(),
            }
        }

        fn with_padding_and_border_box(mut self, padding: f32) -> Self {
            self.padding = Edges::uniform(NonNegative(LengthPercentage::new_length(Length::new(
                padding,
            ))));
            self.box_sizing = box_sizing::T::BorderBox;
            self
        }
    }

    impl CoreStyle for BoxStyle {
        fn display(&self) -> Display {
            Display::Flex
        }

        fn size(&self) -> Size<&StyleSize> {
            self.size.as_ref()
        }

        fn padding(&self) -> Edges<&NonNegativeLengthPercentage> {
            self.padding.as_ref()
        }

        fn box_sizing(&self) -> box_sizing::T {
            self.box_sizing
        }

        fn aspect_ratio(&self) -> AspectRatio {
            self.aspect_ratio
        }
    }

    fn aspect_ratio(auto: bool, width: f32, height: f32) -> AspectRatio {
        AspectRatio {
            auto,
            ratio: PreferredRatio::Ratio(Ratio(NonNegative(width), NonNegative(height))),
        }
    }

    #[test]
    fn natural_ratio_uses_the_content_box_with_border_box_sizing() {
        let natural = NaturalSize::from_size(Size::new(100.0, 50.0));
        let fixed_width = BoxStyle::new(Size::new(size_px(100.0), StyleSize::auto()))
            .with_padding_and_border_box(10.0);
        let fixed = compute_leaf_layout(LayoutInput::default(), &fixed_width, natural);
        assert_eq!(fixed.size, Size::new(100.0, 60.0));
        assert_eq!(fixed.content_size, Size::new(100.0, 60.0));

        let automatic = BoxStyle::new(Size::new(StyleSize::auto(), StyleSize::auto()))
            .with_padding_and_border_box(10.0);
        let intrinsic = compute_leaf_layout(LayoutInput::default(), &automatic, natural);
        assert_eq!(intrinsic.size, Size::new(120.0, 70.0));
        assert_eq!(intrinsic.content_size, Size::new(120.0, 70.0));
    }

    #[test]
    fn auto_ratio_fallback_uses_content_box_but_plain_ratio_uses_box_sizing() {
        let base = BoxStyle::new(Size::new(size_px(100.0), StyleSize::auto()))
            .with_padding_and_border_box(10.0);
        let automatic = BoxStyle {
            aspect_ratio: aspect_ratio(true, 2.0, 1.0),
            ..base
        };
        let automatic_output =
            compute_leaf_layout(LayoutInput::default(), &automatic, NaturalSize::NONE);
        assert_eq!(automatic_output.size, Size::new(100.0, 60.0));

        let explicit = BoxStyle {
            aspect_ratio: aspect_ratio(false, 2.0, 1.0),
            ..automatic
        };
        let explicit_output =
            compute_leaf_layout(LayoutInput::default(), &explicit, NaturalSize::NONE);
        assert_eq!(explicit_output.size, Size::new(100.0, 50.0));
    }

    #[test]
    fn single_axis_probe_skips_measurement_after_resolving_style_sizes() {
        let style = BoxStyle::new(Size::new(size_px(40.0), size_px(20.0)));
        let input = LayoutInput::measure(
            Size::NONE,
            Size::new(Some(100.0), Some(100.0)),
            Size::new(
                AvailableSpace::Definite(100.0),
                AvailableSpace::Definite(100.0),
            ),
            RequestedAxis::Horizontal,
        );

        let output = compute_leaf_layout_with_measurement(input, &style, None, true, |_input| {
            panic!("a fully resolved single-axis probe must not measure content")
        });

        assert_eq!(output.size, Size::new(40.0, 20.0));
    }

    #[test]
    fn measured_aspect_ratio_respects_requested_axis_and_sizing_box() {
        let both_indefinite = Size::new(true, true);
        let padding_border = Size::new(10.0, 10.0);

        let vertical = apply_measured_aspect_ratio(
            Size::new(Some(40.0), Some(50.0)),
            both_indefinite,
            Some(RequestedAxis::Vertical),
            PreferredAspectRatio::content_box(2.0),
            padding_border,
        );
        assert_eq!(vertical, Size::new(Some(90.0), Some(50.0)));

        let horizontal = apply_measured_aspect_ratio(
            Size::new(Some(100.0), Some(20.0)),
            both_indefinite,
            Some(RequestedAxis::Both),
            PreferredAspectRatio::content_box(2.0),
            padding_border,
        );
        assert_eq!(horizontal, Size::new(Some(100.0), Some(55.0)));

        let border_box = apply_measured_aspect_ratio(
            Size::new(Some(100.0), Some(20.0)),
            both_indefinite,
            None,
            PreferredAspectRatio::border_box(2.0),
            padding_border,
        );
        assert_eq!(border_box, Size::new(Some(100.0), Some(50.0)));

        let unchanged = Size::new(Some(30.0), Some(40.0));
        assert_eq!(
            apply_measured_aspect_ratio(
                unchanged,
                Size::new(false, true),
                None,
                PreferredAspectRatio::content_box(2.0),
                padding_border,
            ),
            unchanged
        );
        assert_eq!(
            apply_measured_aspect_ratio(
                unchanged,
                both_indefinite,
                None,
                PreferredAspectRatio(0.0),
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
    fn clear_layout_cache_capability_invalidates_box_and_retained_artifacts_together() {
        let tree = LeafTree;
        let mut state = LeafHostState::default();
        state.artifacts.committed = Some(RetainedArtifact {
            metrics: LeafMetrics::new(Size::new(1.0, 1.0)),
            paint_data: vec![b'C'],
        });
        state.artifacts.probe = Some(RetainedArtifact {
            metrics: LeafMetrics::new(Size::new(1.0, 1.0)),
            paint_data: vec![b'P'],
        });

        tree.clear_layout_cache(&mut state, ());

        assert!(state.layout.layout_cache_is_empty());
        assert!(state.artifacts.committed.is_none());
        assert!(state.artifacts.probe.is_none());
    }
}
