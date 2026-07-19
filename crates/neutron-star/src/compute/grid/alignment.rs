//! Track and item alignment for Grid §11 / CSS Box Alignment.
//!
//! The style protocol speaks stylo's `AlignFlags`-based wrappers
//! (`ItemPlacement`/`SelfAlignment`/`ContentDistribution`/`JustifyItems`).
//! This module normalizes those flags once, at style-read time, into the
//! engine-private [`AlignItems`]/[`AlignContent`] enums the sizing and
//! placement passes match on. Normalization policy (design amendment F):
//! the engine interprets the flags it understands; `SAFE`/`UNSAFE` are
//! stripped (safe fallbacks are what the algorithm already does for
//! distributed values); `LEFT`/`RIGHT` map through the container's inline
//! direction where the axis is horizontal and to `start` otherwise;
//! last-baseline uses its specified fallback (the end edge); anything the
//! engine does not understand falls back to start/normal behavior rather
//! than crashing, because cascade-less hosts may fabricate flag values.

#![allow(clippy::cast_precision_loss)]

use stylo::values::specified::align::AlignFlags;

use super::types::TrackSet;

/// Normalized item self-alignment (the value space of
/// `align-items`/`align-self`/`justify-items`/`justify-self` after flag
/// interpretation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AlignItems {
    /// Align to the start edge of the alignment container.
    Start,
    /// Align to the end edge of the alignment container.
    End,
    /// Flexbox-compat start of the flex axis (identical to `Start` in grid).
    FlexStart,
    /// Flexbox-compat end of the flex axis (identical to `End` in grid).
    FlexEnd,
    /// Center within the alignment container.
    Center,
    /// Align first baselines.
    Baseline,
    /// Stretch to fill the alignment container.
    Stretch,
}

/// Normalized content distribution (the value space of
/// `align-content`/`justify-content` after flag interpretation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AlignContent {
    /// Pack toward the start edge of the container.
    Start,
    /// Pack toward the end edge of the container.
    End,
    /// Flexbox-compat start of the flex axis.
    FlexStart,
    /// Flexbox-compat end of the flex axis.
    FlexEnd,
    /// Center within the container.
    Center,
    /// Stretch tracks to fill the container.
    Stretch,
    /// Even gaps between items, none at the edges.
    SpaceBetween,
    /// Even gaps between and around items.
    SpaceEvenly,
    /// Even gaps around each item (edge gaps half the inner gaps).
    SpaceAround,
}

/// Interprets one `AlignFlags` value as item self-alignment.
///
/// `None` means `auto`/`normal` — the caller applies its contextual default
/// (defer to the container's `*-items` value, or `stretch`). `inline_axis`
/// says whether the aligned axis is the horizontal one (where the physical
/// `left`/`right` keywords are meaningful); `rtl` is the alignment
/// container's inline direction.
pub(super) fn normalize_item_alignment(
    flags: AlignFlags,
    inline_axis: bool,
    rtl: bool,
) -> Option<AlignItems> {
    let value = flags.value();
    if value == AlignFlags::AUTO || value == AlignFlags::NORMAL {
        None
    } else if value == AlignFlags::START {
        Some(AlignItems::Start)
    } else if value == AlignFlags::END {
        Some(AlignItems::End)
    } else if value == AlignFlags::FLEX_START {
        Some(AlignItems::FlexStart)
    } else if value == AlignFlags::FLEX_END {
        Some(AlignItems::FlexEnd)
    } else if value == AlignFlags::CENTER {
        Some(AlignItems::Center)
    } else if value == AlignFlags::BASELINE {
        Some(AlignItems::Baseline)
    } else if value == AlignFlags::LAST_BASELINE {
        // Last-baseline sharing is not implemented; its specified fallback
        // alignment is the end edge (CSS Box Alignment §4.2).
        Some(AlignItems::End)
    } else if value == AlignFlags::STRETCH {
        Some(AlignItems::Stretch)
    } else if value == AlignFlags::LEFT && inline_axis {
        Some(if rtl {
            AlignItems::End
        } else {
            AlignItems::Start
        })
    } else if value == AlignFlags::RIGHT && inline_axis {
        Some(if rtl {
            AlignItems::Start
        } else {
            AlignItems::End
        })
    } else {
        // Physical keywords in the block axis, `self-start`/`self-end`
        // (unreachable from the lynx grammar), and unknown fabricated values
        // fall back to start (design amendment F).
        Some(AlignItems::Start)
    }
}

/// Interprets one `AlignFlags` value as content distribution.
///
/// `None` means `normal` — the grid algorithm's default is `stretch`.
pub(super) fn normalize_content_alignment(
    flags: AlignFlags,
    inline_axis: bool,
    rtl: bool,
) -> Option<AlignContent> {
    let value = flags.value();
    if value == AlignFlags::AUTO || value == AlignFlags::NORMAL {
        None
    } else if value == AlignFlags::START {
        Some(AlignContent::Start)
    } else if value == AlignFlags::END {
        Some(AlignContent::End)
    } else if value == AlignFlags::FLEX_START {
        Some(AlignContent::FlexStart)
    } else if value == AlignFlags::FLEX_END {
        Some(AlignContent::FlexEnd)
    } else if value == AlignFlags::CENTER {
        Some(AlignContent::Center)
    } else if value == AlignFlags::STRETCH {
        Some(AlignContent::Stretch)
    } else if value == AlignFlags::SPACE_BETWEEN {
        Some(AlignContent::SpaceBetween)
    } else if value == AlignFlags::SPACE_AROUND {
        Some(AlignContent::SpaceAround)
    } else if value == AlignFlags::SPACE_EVENLY {
        Some(AlignContent::SpaceEvenly)
    } else if value == AlignFlags::LEFT && inline_axis {
        Some(if rtl {
            AlignContent::End
        } else {
            AlignContent::Start
        })
    } else if value == AlignFlags::RIGHT && inline_axis {
        Some(if rtl {
            AlignContent::Start
        } else {
            AlignContent::End
        })
    } else {
        // Baseline content-alignment (unimplemented) and unknown fabricated
        // values fall back to their specified fallback: start.
        Some(AlignContent::Start)
    }
}

pub(super) fn track_alignment_spacing(
    tracks: &TrackSet,
    container_size: f32,
    alignment: AlignContent,
) -> (f32, f32) {
    let free = container_size - tracks.used_size();
    let visible = tracks
        .tracks
        .iter()
        .filter(|track| !track.collapsed)
        .count();
    alignment_spacing_from_free_space(free, visible, alignment)
}

pub(super) fn alignment_spacing_from_free_space(
    free: f32,
    visible: usize,
    alignment: AlignContent,
) -> (f32, f32) {
    match alignment {
        AlignContent::End | AlignContent::FlexEnd => (free, 0.0),
        AlignContent::Center => (free / 2.0, 0.0),
        AlignContent::SpaceBetween if visible > 1 && free > 0.0 => {
            (0.0, free / (visible - 1) as f32)
        }
        AlignContent::SpaceAround if visible > 0 && free > 0.0 => {
            let spacing = free / visible as f32;
            (spacing / 2.0, spacing)
        }
        AlignContent::SpaceEvenly if visible > 0 && free > 0.0 => {
            let spacing = free / (visible + 1) as f32;
            (spacing, spacing)
        }
        AlignContent::Start
        | AlignContent::FlexStart
        | AlignContent::Stretch
        | AlignContent::SpaceBetween
        | AlignContent::SpaceAround
        | AlignContent::SpaceEvenly => (0.0, 0.0),
    }
}

/// Positions tracks in logical start-to-end order. Inline RTL conversion is
/// deliberately deferred until child locations are materialized, keeping
/// placement and sizing coordinate systems identical.
pub(super) fn align_tracks(tracks: &mut TrackSet, container_size: f32, alignment: AlignContent) {
    // Positional alignment is unsafe (the `SAFE` flag is stripped during
    // normalization), so center/end retain negative free space and may
    // overflow both edges. Distributed values use their safe fallback when
    // space is negative.
    let (offset, distributed_gap) = track_alignment_spacing(tracks, container_size, alignment);

    tracks.rebuild_aligned_positions(offset, distributed_gap);
}

/// Offset of an item inside the free space of one grid-area axis.
#[inline]
pub(super) fn item_alignment_offset(
    free_space: f32,
    alignment: AlignItems,
    container_axis_reversed: bool,
    self_axis_reversed: bool,
) -> f32 {
    match alignment {
        AlignItems::Start | AlignItems::FlexStart | AlignItems::Stretch => {
            // Stretch falls back to the alignment container's start edge.
            if container_axis_reversed {
                free_space
            } else {
                0.0
            }
        }
        AlignItems::End | AlignItems::FlexEnd => {
            if container_axis_reversed {
                0.0
            } else {
                free_space
            }
        }
        AlignItems::Center => free_space / 2.0,
        // First-baseline's fallback is safe self-start, so the alignment
        // subject's own writing direction controls the physical edge unless
        // overflow triggers `safe`'s fallback to the container's start edge.
        AlignItems::Baseline => {
            let reversed = if free_space < 0.0 {
                container_axis_reversed
            } else {
                self_axis_reversed
            };
            if reversed { free_space } else { 0.0 }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests {
    use super::super::types::TrackSizingFunction;
    use super::*;

    fn track(base: f32, collapsed: bool) -> super::super::types::Track {
        super::super::types::Track {
            sizing: TrackSizingFunction::AUTO,
            base,
            growth_limit: base,
            fit_content_limit: f32::INFINITY,
            flex_factor: 0.0,
            flexible: false,
            intrinsic_min: false,
            intrinsic_max: false,
            auto_max: false,
            infinitely_growable: false,
            collapsed,
            position: 0.0,
        }
    }

    fn three_tracks() -> TrackSet {
        TrackSet {
            tracks: vec![track(10.0, false), track(10.0, false), track(10.0, false)],
            gap: 0.0,
            first_coordinate: 0,
            collapsed_line_positions: None,
        }
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= 0.001,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn align_flags_normalize_with_physical_and_fallback_handling() {
        assert_eq!(
            normalize_item_alignment(AlignFlags::AUTO, false, false),
            None
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::NORMAL, false, false),
            None
        );
        // The SAFE qualifier is stripped; the value nibble decides.
        assert_eq!(
            normalize_item_alignment(AlignFlags::CENTER | AlignFlags::SAFE, false, false),
            Some(AlignItems::Center)
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::LEFT, true, false),
            Some(AlignItems::Start)
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::LEFT, true, true),
            Some(AlignItems::End)
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::RIGHT, true, true),
            Some(AlignItems::Start)
        );
        // Physical keywords in the block axis fall back to start.
        assert_eq!(
            normalize_item_alignment(AlignFlags::RIGHT, false, false),
            Some(AlignItems::Start)
        );
        // Last baseline uses its specified fallback: the end edge.
        assert_eq!(
            normalize_item_alignment(AlignFlags::LAST_BASELINE, false, false),
            Some(AlignItems::End)
        );
        // Values outside the engine's understood set fall back to start.
        assert_eq!(
            normalize_item_alignment(AlignFlags::SELF_END, false, false),
            Some(AlignItems::Start)
        );

        assert_eq!(
            normalize_content_alignment(AlignFlags::NORMAL, false, false),
            None
        );
        assert_eq!(
            normalize_content_alignment(AlignFlags::SPACE_BETWEEN, true, false),
            Some(AlignContent::SpaceBetween)
        );
        assert_eq!(
            normalize_content_alignment(AlignFlags::RIGHT, true, false),
            Some(AlignContent::End)
        );
        // Baseline content alignment falls back to start.
        assert_eq!(
            normalize_content_alignment(AlignFlags::BASELINE, false, false),
            Some(AlignContent::Start)
        );
    }

    #[test]
    fn distributed_alignment_places_tracks_for_each_spacing_mode() {
        let mut tracks = three_tracks();
        align_tracks(&mut tracks, 50.0, AlignContent::SpaceBetween);
        assert_eq!(
            tracks
                .tracks
                .iter()
                .map(|track| track.position)
                .collect::<Vec<_>>(),
            [0.0, 20.0, 40.0]
        );

        let mut tracks = three_tracks();
        align_tracks(&mut tracks, 50.0, AlignContent::SpaceAround);
        assert_close(tracks.tracks[0].position, 10.0 / 3.0);
        assert_close(tracks.tracks[1].position, 20.0);
        assert_close(tracks.tracks[2].position, 110.0 / 3.0);

        let mut tracks = three_tracks();
        align_tracks(&mut tracks, 50.0, AlignContent::SpaceEvenly);
        assert_eq!(
            tracks
                .tracks
                .iter()
                .map(|track| track.position)
                .collect::<Vec<_>>(),
            [5.0, 20.0, 35.0]
        );
    }

    #[test]
    fn collapsed_track_gutters_overlap_and_reversed_fallbacks_use_start() {
        let mut tracks = TrackSet {
            tracks: vec![
                track(10.0, false),
                track(0.0, true),
                track(0.0, true),
                track(10.0, false),
            ],
            gap: 7.0,
            first_coordinate: 0,
            collapsed_line_positions: None,
        };
        assert_eq!(tracks.used_size(), 27.0);
        tracks.rebuild_positions();
        assert_eq!(tracks.tracks[0].position, 0.0);
        assert_eq!(tracks.tracks[1].position, 10.0);
        assert_eq!(tracks.tracks[2].position, 10.0);
        assert_eq!(tracks.tracks[3].position, 17.0);
        assert_eq!(tracks.line_position(1), 17.0);
        assert_eq!(tracks.line_position(2), 17.0);

        align_tracks(&mut tracks, 27.0, AlignContent::Start);
        assert_eq!(tracks.tracks[0].position, 0.0);
        assert_eq!(tracks.tracks[1].position, 10.0);
        assert_eq!(tracks.tracks[2].position, 10.0);
        assert_eq!(tracks.tracks[3].position, 17.0);
        assert_eq!(tracks.area_size(0, 4), 27.0);
        assert_eq!(tracks.area_size(0, 3), 10.0);
        assert_eq!(tracks.area_size(1, 4), 10.0);
        assert_eq!(tracks.area_size(1, 2), 0.0);

        align_tracks(&mut tracks, 50.0, AlignContent::SpaceBetween);
        assert_eq!(tracks.tracks[0].position, 0.0);
        assert_eq!(tracks.tracks[1].position, 10.0);
        assert_eq!(tracks.tracks[2].position, 10.0);
        assert_eq!(tracks.tracks[3].position, 40.0);
        assert_eq!(tracks.area_size(0, 4), 50.0);
        assert_eq!(tracks.area_size(1, 4), 10.0);

        assert_eq!(
            item_alignment_offset(13.0, AlignItems::FlexEnd, true, false),
            0.0
        );
        assert_eq!(
            item_alignment_offset(13.0, AlignItems::Stretch, true, false),
            13.0
        );
        assert_eq!(
            item_alignment_offset(13.0, AlignItems::Baseline, true, false),
            0.0
        );
        assert_eq!(
            item_alignment_offset(13.0, AlignItems::Baseline, false, true),
            13.0
        );
        assert_eq!(
            item_alignment_offset(-13.0, AlignItems::Baseline, false, true),
            0.0
        );
        assert_eq!(
            item_alignment_offset(-13.0, AlignItems::Baseline, true, false),
            -13.0
        );
    }
}
