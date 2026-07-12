//! Explicit-template expansion and implicit-track construction.
//!
//! This module keeps the sequence work at the edge of the Grid algorithm:
//! borrowed host iterators are expanded once into compact, parallel vectors,
//! then placement coordinates are mapped to concrete track sizing functions.

// Track counts are bounded to 20,000, well inside f64's exact integer range.
#![allow(clippy::cast_precision_loss)]

use super::placement;
use crate::style::{
    CalcHandle, GridTemplateComponent, GridTemplateRepetition, LengthPercentage,
    MaxTrackSizingFunction, MinTrackSizingFunction, RepetitionCount, TrackSizingFunction,
};

/// Grid line coordinates are clamped to `[-10_000, 10_000]` by placement.
/// The corresponding half-open track span can therefore contain 20,000
/// tracks at most.
const GRID_LINE_LIMIT: i32 = 10_000;
// An explicit template starts at line zero, so only the non-negative half of
// the UA-supported line range is addressable. Leading implicit tracks can
// still make the final materialized axis 20,000 tracks wide.
const MAX_AXIS_TRACKS: usize = 10_000;
/// Maximum number of tracks materialized after adding leading implicit
/// tracks. Also bounds hostile/infinite host auto-track iterators.
pub(super) const MAX_MATERIALIZED_TRACKS: usize = 20_000;
const AUTO_REPEAT_TRACK_FLOOR: f64 = 1.0;

/// A concrete explicit track list after expanding every `repeat()` group.
#[derive(Debug, Clone, Default)]
pub(super) struct ExpandedTemplate {
    pub(super) tracks: Vec<TrackSizingFunction>,
    /// Parallel to `tracks`; true only for tracks originating in an
    /// `auto-fit` repetition.
    pub(super) auto_fit: Vec<bool>,
}

#[derive(Debug, Clone, Copy)]
struct AutoRepeat {
    start: usize,
    len: usize,
}

/// Expands a borrowed template iterator into concrete tracks.
///
/// CSS syntax permits at most one automatic repetition. Invalid host input
/// containing more is handled deterministically by expanding later automatic
/// repetitions once, which is also the indefinite-size fallback.
pub(super) fn expand_template<I, R>(
    components: I,
    definite_or_max_inner_size: Option<f32>,
    minimum_inner_size: Option<f32>,
    gap: f32,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> ExpandedTemplate
where
    I: Iterator<Item = GridTemplateComponent<R>>,
    R: GridTemplateRepetition,
{
    let mut tracks = Vec::new();
    let mut auto_fit = Vec::new();
    let mut auto_repeat = None;

    // A direct protocol host can provide an unbounded iterator even though a
    // parsed CSS track list is finite. Every valid component contributes at
    // least one track, so inspecting more components than the UA track limit
    // cannot affect a supported template. Capping both inspected components
    // and emitted tracks also makes invalid infinite empty repetitions
    // terminate deterministically.
    let mut components = components.take(MAX_AXIS_TRACKS);
    while tracks.len() < MAX_AXIS_TRACKS {
        let Some(component) = components.next() else {
            break;
        };
        match component {
            GridTemplateComponent::Single(track) => {
                push_track(&mut tracks, &mut auto_fit, track, false);
            }
            GridTemplateComponent::Repeat(repetition) => {
                let count = repetition.count();
                let repeated_tracks = repetition.tracks();
                let repeated_len = repeated_tracks.len();
                if repeated_len == 0 {
                    continue;
                }

                match count {
                    RepetitionCount::Count(count) => {
                        // A zero count cannot be produced by valid CSS, but
                        // treating it as one keeps direct protocol hosts safe.
                        let repetitions = usize::from(count.max(1));
                        let requested = repeated_len.saturating_mul(repetitions);
                        let append = requested.min(MAX_AXIS_TRACKS - tracks.len());
                        tracks.extend(repeated_tracks.cycle().take(append));
                        auto_fit.resize(tracks.len(), false);
                    }
                    RepetitionCount::AutoFill | RepetitionCount::AutoFit => {
                        let is_auto_fit = count == RepetitionCount::AutoFit;
                        let start = tracks.len();
                        let append = repeated_len.min(MAX_AXIS_TRACKS - start);
                        tracks.extend(repeated_tracks.take(append));
                        auto_fit.resize(tracks.len(), is_auto_fit);

                        let group = AutoRepeat {
                            start,
                            len: tracks.len() - start,
                        };
                        if auto_repeat.is_none() {
                            auto_repeat = Some(group);
                        }
                    }
                }
            }
        }
    }

    let Some(group) = auto_repeat else {
        return ExpandedTemplate { tracks, auto_fit };
    };
    if group.len == 0 {
        return ExpandedTemplate { tracks, auto_fit };
    }

    let repetitions = automatic_repetition_count(
        &tracks,
        group,
        definite_or_max_inner_size,
        minimum_inner_size,
        gap,
        resolve_calc,
    );
    if repetitions == 1 {
        return ExpandedTemplate { tracks, auto_fit };
    }

    let extra_tracks = group
        .len
        .checked_mul(repetitions - 1)
        .expect("the repetition count is clamped to the track limit");
    let final_len = tracks
        .len()
        .checked_add(extra_tracks)
        .expect("the repetition count is clamped to the track limit");
    debug_assert!(final_len <= MAX_AXIS_TRACKS);

    // Rebuild once instead of repeatedly inserting into the middle of the
    // template. This is linear even when the auto-repeat precedes a long tail.
    let group_end = group.start + group.len;
    let mut expanded_tracks = Vec::with_capacity(final_len);
    let mut expanded_auto_fit = Vec::with_capacity(final_len);
    expanded_tracks.extend_from_slice(&tracks[..group_end]);
    expanded_auto_fit.extend_from_slice(&auto_fit[..group_end]);
    for _ in 1..repetitions {
        expanded_tracks.extend_from_slice(&tracks[group.start..group_end]);
        expanded_auto_fit.extend_from_slice(&auto_fit[group.start..group_end]);
    }
    expanded_tracks.extend_from_slice(&tracks[group_end..]);
    expanded_auto_fit.extend_from_slice(&auto_fit[group_end..]);

    ExpandedTemplate {
        tracks: expanded_tracks,
        auto_fit: expanded_auto_fit,
    }
}

#[inline]
fn push_track(
    tracks: &mut Vec<TrackSizingFunction>,
    auto_fit: &mut Vec<bool>,
    track: TrackSizingFunction,
    is_auto_fit: bool,
) {
    if tracks.len() < MAX_AXIS_TRACKS {
        tracks.push(track);
        auto_fit.push(is_auto_fit);
    }
}

fn automatic_repetition_count(
    tracks: &[TrackSizingFunction],
    group: AutoRepeat,
    definite_or_max_inner_size: Option<f32>,
    minimum_inner_size: Option<f32>,
    gap: f32,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
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
        let Some(size) = definite_repeat_breadth(*track, basis, resolve_calc) else {
            // If even one track has no definite counting breadth, the auto
            // repetition is required to occur exactly once.
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

    // Adding another repetition adds its tracks, its internal gaps, and the
    // gutter separating it from the preceding repetition: `group.len` gaps.
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

/// Resolves the definite counting breadth from Grid §7.2.3.2.
///
/// The definite maximum is preferred; a definite minimum is the fallback,
/// and floors the maximum when both are definite.
#[inline]
fn definite_repeat_breadth(
    track: TrackSizingFunction,
    basis: f32,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Option<f64> {
    let minimum = match track.min {
        MinTrackSizingFunction::Fixed(value) => resolve_fixed_breadth(value, basis, resolve_calc),
        MinTrackSizingFunction::MinContent
        | MinTrackSizingFunction::MaxContent
        | MinTrackSizingFunction::Auto => None,
    };
    let maximum = match track.max {
        MaxTrackSizingFunction::Fixed(value) => resolve_fixed_breadth(value, basis, resolve_calc),
        MaxTrackSizingFunction::MinContent
        | MaxTrackSizingFunction::MaxContent
        | MaxTrackSizingFunction::Auto
        | MaxTrackSizingFunction::Fr(_)
        | MaxTrackSizingFunction::FitContent(_) => None,
    };

    match (minimum, maximum) {
        (Some(minimum), Some(maximum)) => Some(maximum.max(minimum)),
        (Some(minimum), None) => Some(minimum),
        (None, Some(maximum)) => Some(maximum),
        (None, None) => None,
    }
}

#[inline]
fn resolve_fixed_breadth(
    breadth: LengthPercentage,
    basis: f32,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Option<f64> {
    let value = match breadth {
        LengthPercentage::Length(value) => value,
        LengthPercentage::Percent(fraction) => basis * fraction,
        LengthPercentage::Calc(handle) => resolve_calc(handle, basis),
    };
    finite_non_negative(value)
}

#[inline]
fn finite_non_negative(value: f32) -> Option<f64> {
    value.is_finite().then(|| f64::from(value.max(0.0)))
}

/// One concrete track spanning `coordinate..coordinate + 1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct AxisTrackSpec {
    pub(super) coordinate: i32,
    pub(super) sizing: TrackSizingFunction,
    pub(super) auto_fit: bool,
    pub(super) collapsed: bool,
}

/// Builds the final explicit + implicit track sequence for one axis.
///
/// `occupied` is parallel to the returned coordinate range. Empty explicit
/// auto-fit tracks collapse after placement; implicit tracks never do.
pub(super) fn build_axis_tracks(
    explicit: &ExpandedTemplate,
    auto_tracks: &[TrackSizingFunction],
    range: placement::TrackSpan,
    occupied: &[bool],
) -> Vec<AxisTrackSpec> {
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
                explicit.auto_fit.get(index).copied().unwrap_or(false),
            )
        } else {
            (implicit_track(auto_tracks, coordinate, explicit_len), false)
        };

        // Valid placement ranges are already clamped, so `result_index` is
        // the occupied index. Account for defensive start clamping as well.
        let occupied_index =
            usize::try_from(i64::from(coordinate) - i64::from(range.start)).unwrap_or(result_index);
        let collapsed = is_auto_fit && !occupied.get(occupied_index).copied().unwrap_or(false);
        result.push(AxisTrackSpec {
            coordinate,
            sizing,
            auto_fit: is_auto_fit,
            collapsed,
        });
    }

    result
}

#[inline]
fn implicit_track(
    auto_tracks: &[TrackSizingFunction],
    coordinate: i32,
    explicit_len: usize,
) -> TrackSizingFunction {
    if auto_tracks.is_empty() {
        return TrackSizingFunction::AUTO;
    }

    let pattern_len = i64::try_from(auto_tracks.len()).expect("track count fits in i64");
    let coordinate = i64::from(coordinate);
    let index = if coordinate < 0 {
        // The last leading implicit track receives the last specified size.
        coordinate.rem_euclid(pattern_len)
    } else {
        // The first trailing implicit track receives the first specified size.
        (coordinate - i64::try_from(explicit_len).expect("track count fits in i64"))
            .rem_euclid(pattern_len)
    };
    auto_tracks[usize::try_from(index).expect("Euclidean remainder is non-negative")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Repetition {
        count: RepetitionCount,
        tracks: Vec<TrackSizingFunction>,
    }

    impl GridTemplateRepetition for Repetition {
        type Tracks<'a>
            = core::iter::Copied<core::slice::Iter<'a, TrackSizingFunction>>
        where
            Self: 'a;

        fn count(&self) -> RepetitionCount {
            self.count
        }

        fn tracks(&self) -> Self::Tracks<'_> {
            self.tracks.iter().copied()
        }
    }

    fn px(value: f32) -> TrackSizingFunction {
        TrackSizingFunction::fixed(LengthPercentage::length(value))
    }

    fn no_calc(_handle: CalcHandle, _basis: f32) -> f32 {
        unreachable!("test track has no calc() value")
    }

    #[test]
    fn fixed_repeat_expands_in_source_order() {
        let repeated = Repetition {
            count: RepetitionCount::Count(3),
            tracks: vec![px(20.0), px(30.0)],
        };
        let template = expand_template(
            vec![
                GridTemplateComponent::Single(px(10.0)),
                GridTemplateComponent::Repeat(&repeated),
                GridTemplateComponent::Single(px(40.0)),
            ]
            .into_iter(),
            Some(500.0),
            None,
            0.0,
            &no_calc,
        );

        assert_eq!(
            template.tracks,
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
        assert_eq!(template.auto_fit, vec![false; 8]);
    }

    #[test]
    fn auto_fill_counts_all_tracks_and_gaps() {
        let repeated = Repetition {
            count: RepetitionCount::AutoFill,
            tracks: vec![
                TrackSizingFunction::minmax(
                    MinTrackSizingFunction::Fixed(LengthPercentage::length(20.0)),
                    MaxTrackSizingFunction::Fixed(LengthPercentage::length(40.0)),
                ),
                px(10.0),
            ],
        };
        let template = expand_template(
            vec![
                GridTemplateComponent::Single(px(50.0)),
                GridTemplateComponent::Repeat(&repeated),
                GridTemplateComponent::Single(px(30.0)),
            ]
            .into_iter(),
            Some(265.0),
            None,
            5.0,
            &no_calc,
        );

        assert_eq!(template.tracks.len(), 8);
        assert_eq!(template.tracks[0], px(50.0));
        assert_eq!(template.tracks[7], px(30.0));
        assert!(template.auto_fit.iter().all(|is_auto_fit| !is_auto_fit));
    }

    #[test]
    fn auto_repeat_uses_one_pixel_floor_and_one_indefinite_fallback() {
        let repeated = Repetition {
            count: RepetitionCount::AutoFit,
            tracks: vec![TrackSizingFunction::minmax(
                MinTrackSizingFunction::Fixed(LengthPercentage::ZERO),
                MaxTrackSizingFunction::Fr(1.0),
            )],
        };

        let definite = expand_template(
            core::iter::once(GridTemplateComponent::Repeat(&repeated)),
            Some(3.0),
            None,
            0.0,
            &no_calc,
        );
        assert_eq!(definite.tracks.len(), 3);
        assert_eq!(definite.auto_fit, vec![true; 3]);

        let indefinite = expand_template(
            core::iter::once(GridTemplateComponent::Repeat(&repeated)),
            None,
            None,
            0.0,
            &no_calc,
        );
        assert_eq!(indefinite.tracks.len(), 1);
        assert_eq!(indefinite.auto_fit, vec![true]);
    }

    #[test]
    fn auto_repeat_resolves_percent_and_calc_and_floors_max_by_min() {
        let repeated = Repetition {
            count: RepetitionCount::AutoFill,
            tracks: vec![TrackSizingFunction::minmax(
                MinTrackSizingFunction::Fixed(LengthPercentage::percent(0.3)),
                MaxTrackSizingFunction::Fixed(LengthPercentage::Calc(CalcHandle::from_raw(7))),
            )],
        };
        let resolve = |handle: CalcHandle, basis: f32| {
            assert_eq!(handle.raw(), 7);
            basis * 0.2
        };
        let template = expand_template(
            core::iter::once(GridTemplateComponent::Repeat(&repeated)),
            Some(100.0),
            None,
            0.0,
            &resolve,
        );

        // max=20px is floored by min=30px, hence three repetitions.
        assert_eq!(template.tracks.len(), 3);
    }

    #[test]
    fn expansion_is_clamped_without_count_overflow() {
        let repeated = Repetition {
            count: RepetitionCount::Count(u16::MAX),
            tracks: vec![px(1.0), px(2.0)],
        };
        let template = expand_template(
            core::iter::once(GridTemplateComponent::Repeat(&repeated)),
            Some(f32::MAX),
            None,
            0.0,
            &no_calc,
        );

        assert_eq!(template.tracks.len(), MAX_AXIS_TRACKS);
        assert_eq!(template.auto_fit.len(), MAX_AXIS_TRACKS);
        assert_eq!(template.tracks[MAX_AXIS_TRACKS - 2], px(1.0));
        assert_eq!(template.tracks[MAX_AXIS_TRACKS - 1], px(2.0));
    }

    #[test]
    fn infinite_single_track_components_stop_at_the_axis_limit() {
        let mut yielded = 0;
        let components = core::iter::from_fn(|| {
            yielded += 1;
            Some(GridTemplateComponent::<Repetition>::Single(px(1.0)))
        });

        let template = expand_template(components, None, None, 0.0, &no_calc);

        assert_eq!(yielded, MAX_AXIS_TRACKS);
        assert_eq!(template.tracks.len(), MAX_AXIS_TRACKS);
        assert_eq!(template.auto_fit.len(), MAX_AXIS_TRACKS);
    }

    #[test]
    fn infinite_empty_repetitions_stop_at_the_component_limit() {
        let repeated = Repetition {
            count: RepetitionCount::Count(1),
            tracks: Vec::new(),
        };
        let mut yielded = 0;
        let components = core::iter::from_fn(|| {
            yielded += 1;
            Some(GridTemplateComponent::Repeat(&repeated))
        });

        let template = expand_template(components, None, None, 0.0, &no_calc);

        assert_eq!(yielded, MAX_AXIS_TRACKS);
        assert!(template.tracks.is_empty());
        assert!(template.auto_fit.is_empty());
    }

    #[test]
    fn implicit_tracks_cycle_forward_and_backward() {
        let explicit = ExpandedTemplate {
            tracks: vec![px(10.0), px(20.0)],
            auto_fit: vec![true, false],
        };
        let auto_tracks = [px(1.0), px(2.0), px(3.0)];
        let range = placement::TrackSpan { start: -3, end: 5 };
        let mut occupied = vec![false; 8];
        occupied[4] = true;

        let tracks = build_axis_tracks(&explicit, &auto_tracks, range, &occupied);
        assert_eq!(
            tracks
                .iter()
                .map(|track| track.coordinate)
                .collect::<Vec<_>>(),
            vec![-3, -2, -1, 0, 1, 2, 3, 4]
        );
        assert_eq!(
            tracks.iter().map(|track| track.sizing).collect::<Vec<_>>(),
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
        assert!(tracks[3].auto_fit);
        assert!(tracks[3].collapsed);
        assert!(!tracks[4].auto_fit);
        assert!(!tracks[4].collapsed);
    }

    #[test]
    fn empty_auto_pattern_defaults_to_auto() {
        let explicit = ExpandedTemplate::default();
        let tracks = build_axis_tracks(
            &explicit,
            &[],
            placement::TrackSpan { start: -1, end: 2 },
            &[false; 3],
        );
        assert!(
            tracks
                .iter()
                .all(|track| track.sizing == TrackSizingFunction::AUTO)
        );
    }
}
