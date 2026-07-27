//! Track and item alignment for Grid §11 / CSS Box Alignment.

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
        (0.0, 0.0)
    }
}

pub(super) fn align_tracks(tracks: &mut TrackSet, container_size: f32, alignment: AlignFlags) {
    let (offset, distributed_gap) = track_alignment_spacing(tracks, container_size, alignment);

    tracks.rebuild_aligned_positions(offset, distributed_gap);
}

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
        let reversed = if free_space < 0.0 {
            container_axis_reversed
        } else {
            self_axis_reversed
        };
        if reversed { free_space } else { 0.0 }
    } else if container_axis_reversed {
        free_space
    } else {
        0.0
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
