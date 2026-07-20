//! Track and item alignment for Grid §11 / CSS Box Alignment.
//!
//! The style protocol speaks stylo's `AlignFlags`-based wrappers
//! (`ItemPlacement`/`SelfAlignment`/`ContentDistribution`/`JustifyItems`).
//! The shared [`normalize_item_alignment`]/[`normalize_content_alignment`]
//! helpers in `compute::util` reduce those flags once, at style-read time,
//! to the canonical keyword subset the sizing and placement passes below
//! compare against.
//!
//! [`normalize_item_alignment`]: super::super::util::normalize_item_alignment
//! [`normalize_content_alignment`]: super::super::util::normalize_content_alignment

#![allow(clippy::cast_precision_loss)]

use stylo::values::specified::align::AlignFlags;

use super::types::TrackSet;

pub(super) fn track_alignment_spacing(
    tracks: &TrackSet,
    container_size: f32,
    alignment: AlignFlags,
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
    alignment: AlignFlags,
) -> (f32, f32) {
    if alignment == AlignFlags::END || alignment == AlignFlags::FLEX_END {
        (free, 0.0)
    } else if alignment == AlignFlags::CENTER {
        (free / 2.0, 0.0)
    } else if alignment == AlignFlags::SPACE_BETWEEN && visible > 1 && free > 0.0 {
        (0.0, free / (visible - 1) as f32)
    } else if alignment == AlignFlags::SPACE_AROUND && visible > 0 && free > 0.0 {
        let spacing = free / visible as f32;
        (spacing / 2.0, spacing)
    } else if alignment == AlignFlags::SPACE_EVENLY && visible > 0 && free > 0.0 {
        let spacing = free / (visible + 1) as f32;
        (spacing, spacing)
    } else {
        // START/FLEX_START/STRETCH pack at start; distributed values without
        // positive free space use their safe start fallback.
        (0.0, 0.0)
    }
}

/// Positions tracks in logical start-to-end order. Inline RTL conversion is
/// deliberately deferred until child locations are materialized, keeping
/// placement and sizing coordinate systems identical.
pub(super) fn align_tracks(tracks: &mut TrackSet, container_size: f32, alignment: AlignFlags) {
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
    alignment: AlignFlags,
    container_axis_reversed: bool,
    self_axis_reversed: bool,
) -> f32 {
    if alignment == AlignFlags::END || alignment == AlignFlags::FLEX_END {
        if container_axis_reversed {
            0.0
        } else {
            free_space
        }
    } else if alignment == AlignFlags::CENTER {
        free_space / 2.0
    } else if alignment == AlignFlags::BASELINE {
        // First-baseline's fallback is safe self-start, so the alignment
        // subject's own writing direction controls the physical edge unless
        // overflow triggers `safe`'s fallback to the container's start edge.
        let reversed = if free_space < 0.0 {
            container_axis_reversed
        } else {
            self_axis_reversed
        };
        if reversed { free_space } else { 0.0 }
    } else {
        // START/FLEX_START/STRETCH: stretch falls back to the alignment
        // container's start edge.
        if container_axis_reversed {
            free_space
        } else {
            0.0
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
    fn distributed_alignment_places_tracks_for_each_spacing_mode() {
        let mut tracks = three_tracks();
        align_tracks(&mut tracks, 50.0, AlignFlags::SPACE_BETWEEN);
        assert_eq!(
            tracks
                .tracks
                .iter()
                .map(|track| track.position)
                .collect::<Vec<_>>(),
            [0.0, 20.0, 40.0]
        );

        let mut tracks = three_tracks();
        align_tracks(&mut tracks, 50.0, AlignFlags::SPACE_AROUND);
        assert_close(tracks.tracks[0].position, 10.0 / 3.0);
        assert_close(tracks.tracks[1].position, 20.0);
        assert_close(tracks.tracks[2].position, 110.0 / 3.0);

        let mut tracks = three_tracks();
        align_tracks(&mut tracks, 50.0, AlignFlags::SPACE_EVENLY);
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

        align_tracks(&mut tracks, 27.0, AlignFlags::START);
        assert_eq!(tracks.tracks[0].position, 0.0);
        assert_eq!(tracks.tracks[1].position, 10.0);
        assert_eq!(tracks.tracks[2].position, 10.0);
        assert_eq!(tracks.tracks[3].position, 17.0);
        assert_eq!(tracks.area_size(0, 4), 27.0);
        assert_eq!(tracks.area_size(0, 3), 10.0);
        assert_eq!(tracks.area_size(1, 4), 10.0);
        assert_eq!(tracks.area_size(1, 2), 0.0);

        align_tracks(&mut tracks, 50.0, AlignFlags::SPACE_BETWEEN);
        assert_eq!(tracks.tracks[0].position, 0.0);
        assert_eq!(tracks.tracks[1].position, 10.0);
        assert_eq!(tracks.tracks[2].position, 10.0);
        assert_eq!(tracks.tracks[3].position, 40.0);
        assert_eq!(tracks.area_size(0, 4), 50.0);
        assert_eq!(tracks.area_size(1, 4), 10.0);

        assert_eq!(
            item_alignment_offset(13.0, AlignFlags::FLEX_END, true, false),
            0.0
        );
        assert_eq!(
            item_alignment_offset(13.0, AlignFlags::STRETCH, true, false),
            13.0
        );
        assert_eq!(
            item_alignment_offset(13.0, AlignFlags::BASELINE, true, false),
            0.0
        );
        assert_eq!(
            item_alignment_offset(13.0, AlignFlags::BASELINE, false, true),
            13.0
        );
        assert_eq!(
            item_alignment_offset(-13.0, AlignFlags::BASELINE, false, true),
            0.0
        );
        assert_eq!(
            item_alignment_offset(-13.0, AlignFlags::BASELINE, true, false),
            -13.0
        );
    }
}
