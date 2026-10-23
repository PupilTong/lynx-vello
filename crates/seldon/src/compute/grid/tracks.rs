//! Explicit-template expansion and implicit-track construction.

#![allow(clippy::cast_precision_loss)]

use core::ops::Range;

use stylo::values::computed::{
    GridTemplateComponent, Length, LengthPercentage, TrackBreadth, TrackSize,
};
use stylo::values::generics::grid::{RepeatCount, TrackListValue};

use super::placement;

pub(super) const GRID_LINE_LIMIT: i32 = 10_000;
const MAX_AXIS_TRACKS: usize = 10_000;
pub(super) const MAX_MATERIALIZED_TRACKS: usize = 20_000;
const AUTO_REPEAT_TRACK_FLOOR: f64 = 1.0;

/// The `auto` track size, which an empty `grid-auto-*` list stands for.
const AUTO_TRACK: &TrackSize = &TrackSize::Breadth(TrackBreadth::Auto);

/// The minimum half of a track size's `minmax()` form (css-grid-2 §7.2.1): a
/// lone `<flex>` and `fit-content()` have an `auto` minimum.
pub(super) fn min_breadth(size: &TrackSize) -> &TrackBreadth {
    match size {
        TrackSize::Breadth(TrackBreadth::Flex(_)) | TrackSize::FitContent(_) => &TrackBreadth::Auto,
        TrackSize::Breadth(breadth) | TrackSize::Minmax(breadth, _) => breadth,
    }
}

/// The maximum half of a track size's `minmax()` form: `fit-content()` has a
/// `max-content` maximum, capped by [`fit_content_limit`].
pub(super) fn max_breadth(size: &TrackSize) -> &TrackBreadth {
    match size {
        TrackSize::Breadth(breadth) | TrackSize::Minmax(_, breadth) => breadth,
        TrackSize::FitContent(_) => &TrackBreadth::MaxContent,
    }
}

/// The argument of a `fit-content()` track size.
pub(super) fn fit_content_limit(size: &TrackSize) -> Option<&LengthPercentage> {
    match size {
        TrackSize::FitContent(TrackBreadth::Breadth(limit)) => Some(limit),
        TrackSize::FitContent(_) => {
            unreachable!("fit-content() stores a <length-percentage> breadth by construction")
        }
        TrackSize::Breadth(_) | TrackSize::Minmax(..) => None,
    }
}

/// A concrete explicit track list after expanding every `repeat()` group:
/// the style's own track sizes, and the range of tracks the `auto-fit`
/// repetition expanded to, if the list has one (the grammar allows at most
/// one automatic repetition).
#[derive(Debug, Clone, Default)]
pub(super) struct ExpandedTemplate<'a> {
    pub(super) tracks: Vec<&'a TrackSize>,
    pub(super) auto_fit: Option<Range<usize>>,
}

#[derive(Debug, Clone, Copy)]
struct AutoRepeat {
    start: usize,
    len: usize,
    auto_fit: bool,
}

pub(super) fn expand_template(
    template: &GridTemplateComponent,
    definite_or_max_inner_size: Option<f32>,
    minimum_inner_size: Option<f32>,
    gap: f32,
) -> ExpandedTemplate<'_> {
    let list = match template {
        GridTemplateComponent::None => {
            return ExpandedTemplate::default();
        }
        GridTemplateComponent::TrackList(list) => list,
        GridTemplateComponent::Subgrid(_) => {
            unreachable!("subgrid is not parseable under the lynx grammar")
        }
    };

    let mut tracks = Vec::new();
    let mut auto_repeat = None;

    for component in list.values.iter() {
        if tracks.len() >= MAX_AXIS_TRACKS {
            break;
        }
        match component {
            TrackListValue::TrackSize(size) => tracks.push(size),
            TrackListValue::TrackRepeat(repetition) => {
                let repeated = &repetition.track_sizes[..];
                if repeated.is_empty() {
                    continue;
                }

                match repetition.count {
                    RepeatCount::Number(count) => {
                        let repetitions = usize::try_from(count).unwrap_or(1).max(1);
                        let requested = repeated.len().saturating_mul(repetitions);
                        let append = requested.min(MAX_AXIS_TRACKS - tracks.len());
                        tracks.extend(repeated.iter().cycle().take(append));
                    }
                    RepeatCount::AutoFill | RepeatCount::AutoFit => {
                        let start = tracks.len();
                        let append = repeated.len().min(MAX_AXIS_TRACKS - start);
                        tracks.extend(repeated.iter().take(append));
                        if auto_repeat.is_none() {
                            auto_repeat = Some(AutoRepeat {
                                start,
                                len: tracks.len() - start,
                                auto_fit: repetition.count == RepeatCount::AutoFit,
                            });
                        }
                    }
                }
            }
        }
    }

    let Some(group) = auto_repeat.filter(|group| group.len > 0) else {
        return ExpandedTemplate {
            tracks,
            auto_fit: None,
        };
    };

    let repetitions = automatic_repetition_count(
        &tracks,
        group,
        definite_or_max_inner_size,
        minimum_inner_size,
        gap,
    );
    let group_end = group.start + group.len;
    if repetitions > 1 {
        let extra_tracks = group
            .len
            .checked_mul(repetitions - 1)
            .expect("the repetition count is clamped to the track limit");
        debug_assert!(tracks.len() + extra_tracks <= MAX_AXIS_TRACKS);
        // The extra repetitions follow the group, before the tracks after it.
        let after = tracks.split_off(group_end);
        tracks.reserve(extra_tracks + after.len());
        for _ in 1..repetitions {
            tracks.extend_from_within(group.start..group_end);
        }
        tracks.extend(after);
    }

    ExpandedTemplate {
        tracks,
        auto_fit: group
            .auto_fit
            .then(|| group.start..group.start + group.len * repetitions),
    }
}

fn automatic_repetition_count(
    tracks: &[&TrackSize],
    group: AutoRepeat,
    definite_or_max_inner_size: Option<f32>,
    minimum_inner_size: Option<f32>,
    gap: f32,
) -> usize {
    let (basis, fulfill_minimum) =
        if let Some(value) = definite_or_max_inner_size.filter(|value| value.is_finite()) {
            (value, false)
        } else if let Some(value) = minimum_inner_size.filter(|value| value.is_finite()) {
            (value, true)
        } else {
            return 1;
        };
    let basis = basis.max(0.0);
    let inner_size = f64::from(basis);
    let gap = finite_non_negative(gap).unwrap_or(0.0);

    let mut one_repetition_tracks_size = 0.0;
    let mut repeated_tracks_size = 0.0;
    let group_end = group.start + group.len;
    for (index, track) in tracks.iter().enumerate() {
        let Some(size) = definite_repeat_breadth(track, basis) else {
            return 1;
        };
        let size = size.max(AUTO_REPEAT_TRACK_FLOOR);
        one_repetition_tracks_size += size;
        if (group.start..group_end).contains(&index) {
            repeated_tracks_size += size;
        }
    }

    let one_repetition_gaps = tracks.len().saturating_sub(1) as f64 * gap;
    let one_repetition_size = one_repetition_tracks_size + one_repetition_gaps;
    if inner_size <= one_repetition_size {
        return 1;
    }

    let added_repetition_size = repeated_tracks_size + group.len as f64 * gap;
    if added_repetition_size <= 0.0 {
        return 1;
    }

    let outside_tracks = tracks.len() - group.len;
    let maximum_repetitions = (MAX_AXIS_TRACKS - outside_tracks) / group.len;
    debug_assert!(maximum_repetitions >= 1);
    let maximum_extra = maximum_repetitions.saturating_sub(1);

    let extra_ratio = (inner_size - one_repetition_size) / added_repetition_size;
    let fitting_extra = if fulfill_minimum {
        extra_ratio.ceil()
    } else {
        extra_ratio.floor()
    };
    let fitting_extra = fitting_extra.min(maximum_extra as f64);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fitting_extra = fitting_extra as usize;
    1 + fitting_extra
}

#[inline]
fn definite_repeat_breadth(track: &TrackSize, basis: f32) -> Option<f64> {
    let fixed = |breadth: &TrackBreadth| match breadth {
        TrackBreadth::Breadth(value) => resolve_fixed_breadth(value, basis),
        TrackBreadth::MinContent
        | TrackBreadth::MaxContent
        | TrackBreadth::Auto
        | TrackBreadth::Flex(_) => None,
    };
    let minimum = fixed(min_breadth(track));
    let maximum = fixed(max_breadth(track));

    match (minimum, maximum) {
        (Some(minimum), Some(maximum)) => Some(maximum.max(minimum)),
        (Some(minimum), None) => Some(minimum),
        (None, Some(maximum)) => Some(maximum),
        (None, None) => None,
    }
}

#[inline]
fn resolve_fixed_breadth(breadth: &LengthPercentage, basis: f32) -> Option<f64> {
    finite_non_negative(breadth.resolve(Length::new(basis)).px())
}

#[inline]
fn finite_non_negative(value: f32) -> Option<f64> {
    value.is_finite().then(|| f64::from(value.max(0.0)))
}

/// One concrete track: its size, borrowed from the style, and whether it is
/// an empty `auto-fit` track, which collapses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct AxisTrackSpec<'a> {
    pub(super) sizing: &'a TrackSize,
    pub(super) collapsed: bool,
}

/// Whether any track breadth or `fit-content()` limit is a percentage, which
/// resolves against the container's inner size on the axis.
pub(super) fn specs_depend_on_basis(specs: &[AxisTrackSpec<'_>]) -> bool {
    let breadth_reads_basis = |breadth: &TrackBreadth| matches!(breadth, TrackBreadth::Breadth(lp) if lp.has_percentage());
    specs.iter().any(|spec| {
        breadth_reads_basis(min_breadth(spec.sizing))
            || breadth_reads_basis(max_breadth(spec.sizing))
            || fit_content_limit(spec.sizing).is_some_and(LengthPercentage::has_percentage)
    })
}

/// The concrete tracks of one axis, one per coordinate of `range` in order,
/// and the coordinate of the first. `auto_tracks` is the `grid-auto-*` list
/// and `occupied` the occupancy of `range`, which only an `auto-fit` track
/// reads.
pub(super) fn build_axis_tracks<'a>(
    explicit: &ExpandedTemplate<'a>,
    auto_tracks: &'a [TrackSize],
    range: placement::TrackSpan,
    occupied: &[bool],
) -> (i32, Vec<AxisTrackSpec<'a>>) {
    debug_assert!(range.start >= -GRID_LINE_LIMIT && range.end <= GRID_LINE_LIMIT);
    debug_assert!(range.start <= range.end);

    let start = range.start.clamp(-GRID_LINE_LIMIT, GRID_LINE_LIMIT);
    let end = range.end.clamp(start, GRID_LINE_LIMIT);
    let len = usize::try_from(end - start).expect("clamped grid span is non-negative");
    let mut result = Vec::with_capacity(len);
    let explicit_len = explicit.tracks.len();

    for (result_index, coordinate) in (start..end).enumerate() {
        let explicit_index = usize::try_from(coordinate)
            .ok()
            .filter(|&index| index < explicit_len);
        let (sizing, is_auto_fit) = if let Some(index) = explicit_index {
            (
                explicit.tracks[index],
                explicit
                    .auto_fit
                    .as_ref()
                    .is_some_and(|auto_fit| auto_fit.contains(&index)),
            )
        } else {
            (implicit_track(auto_tracks, coordinate, explicit_len), false)
        };

        let occupied_index =
            usize::try_from(i64::from(coordinate) - i64::from(range.start)).unwrap_or(result_index);
        let collapsed = is_auto_fit && !occupied.get(occupied_index).copied().unwrap_or(false);
        result.push(AxisTrackSpec { sizing, collapsed });
    }

    (start, result)
}

/// Which tracks of `range` some area covers, in the axis `select` picks:
/// what an `auto-fit` track reads to know whether it collapses.
pub(super) fn occupied_tracks<Select>(
    areas: &[placement::GridArea],
    range: placement::TrackSpan,
    select: Select,
) -> Vec<bool>
where
    Select: Fn(&placement::GridArea) -> placement::TrackSpan,
{
    let len = range.len();
    if len == 0 {
        return Vec::new();
    }
    let mut differences = vec![0_i64; len + 1];
    for area in areas {
        let span = select(area);
        let start = usize::try_from(span.start - range.start).unwrap();
        let end = usize::try_from(span.end - range.start).unwrap();
        differences[start] += 1;
        differences[end] -= 1;
    }
    let mut occupied = Vec::with_capacity(len);
    let mut covering = 0_i64;
    for difference in differences.into_iter().take(len) {
        covering += difference;
        occupied.push(covering > 0);
    }
    occupied
}

#[inline]
fn implicit_track(auto_tracks: &[TrackSize], coordinate: i32, explicit_len: usize) -> &TrackSize {
    if auto_tracks.is_empty() {
        return AUTO_TRACK;
    }

    let pattern_len = i64::try_from(auto_tracks.len()).expect("track count fits in i64");
    let coordinate = i64::from(coordinate);
    let index = if coordinate < 0 {
        coordinate.rem_euclid(pattern_len)
    } else {
        (coordinate - i64::try_from(explicit_len).expect("track count fits in i64"))
            .rem_euclid(pattern_len)
    };
    &auto_tracks[usize::try_from(index).expect("Euclidean remainder is non-negative")]
}

#[cfg(test)]
mod tests {
    use style_traits::values::specified::AllowedNumericType;
    use stylo::Zero;
    use stylo::typed_om::NumericBaseType;
    use stylo::values::computed::length_percentage::{CalcNode, CalcPercentageLeaf, ComputedLeaf};
    use stylo::values::computed::{Integer, Percentage, TrackList};
    use stylo::values::generics::Optional;
    use stylo::values::generics::grid::{Flex, TrackRepeat};

    use super::*;

    fn lp_px(value: f32) -> LengthPercentage {
        LengthPercentage::new_length(Length::new(value))
    }

    fn px_size(value: f32) -> TrackSize {
        TrackSize::Breadth(TrackBreadth::Breadth(lp_px(value)))
    }

    fn minmax(min: TrackBreadth, max: TrackBreadth) -> TrackSize {
        TrackSize::Minmax(min, max)
    }

    fn repeat(
        count: RepeatCount<Integer>,
        sizes: Vec<TrackSize>,
    ) -> TrackListValue<LengthPercentage, Integer> {
        TrackListValue::TrackRepeat(TrackRepeat {
            count,
            line_names: vec![stylo::OwnedSlice::default(); sizes.len() + 1].into(),
            track_sizes: sizes.into(),
        })
    }

    fn template(values: Vec<TrackListValue<LengthPercentage, Integer>>) -> GridTemplateComponent {
        let auto_repeat_index = values
            .iter()
            .position(|value| {
                matches!(
                    value,
                    TrackListValue::TrackRepeat(repetition)
                        if matches!(repetition.count, RepeatCount::AutoFill | RepeatCount::AutoFit)
                )
            })
            .unwrap_or(usize::MAX);
        GridTemplateComponent::TrackList(Box::new(TrackList {
            auto_repeat_index,
            line_names: vec![stylo::OwnedSlice::default(); values.len() + 1].into(),
            values: values.into(),
        }))
    }

    fn px(value: f32) -> TrackSize {
        px_size(value)
    }

    fn owned(tracks: &[&TrackSize]) -> Vec<TrackSize> {
        tracks.iter().map(|&track| track.clone()).collect()
    }

    #[test]
    fn normalization_expands_the_single_value_forms() {
        let fr = TrackSize::Breadth(TrackBreadth::Flex(Flex(2.0)));
        assert_eq!(*min_breadth(&fr), TrackBreadth::Auto);
        assert_eq!(*max_breadth(&fr), TrackBreadth::Flex(Flex(2.0)));
        assert_eq!(fit_content_limit(&fr), None);

        let fit = TrackSize::FitContent(TrackBreadth::Breadth(lp_px(40.0)));
        assert_eq!(*min_breadth(&fit), TrackBreadth::Auto);
        assert_eq!(*max_breadth(&fit), TrackBreadth::MaxContent);
        assert_eq!(fit_content_limit(&fit), Some(&lp_px(40.0)));

        let fixed = px(10.0);
        assert_eq!(*min_breadth(&fixed), TrackBreadth::Breadth(lp_px(10.0)));
        assert_eq!(*max_breadth(&fixed), TrackBreadth::Breadth(lp_px(10.0)));
    }

    #[test]
    fn none_template_expands_to_no_tracks() {
        let expanded = expand_template(&GridTemplateComponent::None, Some(100.0), None, 0.0);
        assert!(expanded.tracks.is_empty());
        assert_eq!(expanded.auto_fit, None);
    }

    #[test]
    fn fixed_repeat_expands_in_source_order() {
        let value = template(vec![
            TrackListValue::TrackSize(px_size(10.0)),
            repeat(RepeatCount::Number(3), vec![px_size(20.0), px_size(30.0)]),
            TrackListValue::TrackSize(px_size(40.0)),
        ]);
        let expanded = expand_template(&value, Some(500.0), None, 0.0);

        assert_eq!(
            owned(&expanded.tracks),
            vec![
                px(10.0),
                px(20.0),
                px(30.0),
                px(20.0),
                px(30.0),
                px(20.0),
                px(30.0),
                px(40.0),
            ]
        );
        assert_eq!(expanded.auto_fit, None);
    }

    #[test]
    fn auto_fill_counts_all_tracks_and_gaps() {
        let value = template(vec![
            TrackListValue::TrackSize(px_size(50.0)),
            repeat(
                RepeatCount::AutoFill,
                vec![
                    minmax(
                        TrackBreadth::Breadth(lp_px(20.0)),
                        TrackBreadth::Breadth(lp_px(40.0)),
                    ),
                    px_size(10.0),
                ],
            ),
            TrackListValue::TrackSize(px_size(30.0)),
        ]);
        let expanded = expand_template(&value, Some(265.0), None, 5.0);

        assert_eq!(expanded.tracks.len(), 8);
        assert_eq!(*expanded.tracks[0], px(50.0));
        assert_eq!(*expanded.tracks[7], px(30.0));
        assert_eq!(expanded.auto_fit, None);
    }

    #[test]
    fn auto_repeat_uses_one_pixel_floor_and_one_indefinite_fallback() {
        let value = template(vec![repeat(
            RepeatCount::AutoFit,
            vec![minmax(
                TrackBreadth::Breadth(LengthPercentage::zero()),
                TrackBreadth::Flex(Flex(1.0)),
            )],
        )]);

        let definite = expand_template(&value, Some(3.0), None, 0.0);
        assert_eq!(definite.tracks.len(), 3);
        assert_eq!(definite.auto_fit, Some(0..3));

        let indefinite = expand_template(&value, None, None, 0.0);
        assert_eq!(indefinite.tracks.len(), 1);
        assert_eq!(indefinite.auto_fit, Some(0..1));
    }

    #[test]
    fn auto_repeat_resolves_percent_and_calc_and_floors_max_by_min() {
        let calc = LengthPercentage::new_calc(
            CalcNode::Sum(
                vec![
                    CalcNode::Leaf(ComputedLeaf::Percentage(CalcPercentageLeaf::new(
                        0.1,
                        Optional::Some(NumericBaseType::Length),
                    ))),
                    CalcNode::Leaf(ComputedLeaf::Length(Length::new(10.0))),
                ]
                .into(),
            ),
            AllowedNumericType::All,
        );
        let value = template(vec![repeat(
            RepeatCount::AutoFill,
            vec![minmax(
                TrackBreadth::Breadth(LengthPercentage::new_percent(Percentage(0.3))),
                TrackBreadth::Breadth(calc),
            )],
        )]);
        let expanded = expand_template(&value, Some(100.0), None, 0.0);

        assert_eq!(expanded.tracks.len(), 3);
    }

    #[test]
    fn expansion_is_clamped_without_count_overflow() {
        let value = template(vec![repeat(
            RepeatCount::Number(i32::MAX),
            vec![px_size(1.0), px_size(2.0)],
        )]);
        let expanded = expand_template(&value, Some(f32::MAX), None, 0.0);

        assert_eq!(expanded.tracks.len(), MAX_AXIS_TRACKS);
        assert_eq!(expanded.auto_fit, None);
        assert_eq!(*expanded.tracks[MAX_AXIS_TRACKS - 2], px(1.0));
        assert_eq!(*expanded.tracks[MAX_AXIS_TRACKS - 1], px(2.0));
    }

    #[test]
    fn empty_repetitions_are_skipped() {
        let value = template(vec![
            repeat(RepeatCount::Number(7), Vec::new()),
            TrackListValue::TrackSize(px_size(10.0)),
        ]);
        let expanded = expand_template(&value, None, None, 0.0);

        assert_eq!(owned(&expanded.tracks), vec![px(10.0)]);
    }

    #[test]
    fn implicit_tracks_cycle_forward_and_backward() {
        let (ten, twenty) = (px(10.0), px(20.0));
        let explicit = ExpandedTemplate {
            tracks: vec![&ten, &twenty],
            auto_fit: Some(0..1),
        };
        let auto_tracks = [px(1.0), px(2.0), px(3.0)];
        let range = placement::TrackSpan { start: -3, end: 5 };
        let mut occupied = vec![false; 8];
        occupied[4] = true;

        let (first, tracks) = build_axis_tracks(&explicit, &auto_tracks, range, &occupied);
        assert_eq!(first, -3);
        assert_eq!(tracks.len(), 8);
        assert_eq!(
            tracks
                .iter()
                .map(|track| track.sizing.clone())
                .collect::<Vec<_>>(),
            vec![
                px(1.0),
                px(2.0),
                px(3.0),
                px(10.0),
                px(20.0),
                px(1.0),
                px(2.0),
                px(3.0),
            ]
        );
        assert!(tracks[3].collapsed);
        assert!(!tracks[4].collapsed);
    }

    #[test]
    fn empty_auto_pattern_defaults_to_auto() {
        let explicit = ExpandedTemplate::default();
        let (_, tracks) = build_axis_tracks(
            &explicit,
            &[],
            placement::TrackSpan { start: -1, end: 2 },
            &[false; 3],
        );
        assert!(
            tracks
                .iter()
                .all(|track| *track.sizing == TrackSize::Breadth(TrackBreadth::Auto))
        );
    }
}
