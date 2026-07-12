//! Track and item alignment for Grid §11 / CSS Box Alignment.

#![allow(clippy::cast_precision_loss)]

use super::types::TrackSet;
use crate::style::AlignContent;

/// Positions tracks in logical start-to-end order. Inline RTL conversion is
/// deliberately deferred until child locations are materialized, keeping
/// placement and sizing coordinate systems identical.
pub(super) fn align_tracks(tracks: &mut TrackSet, container_size: f32, alignment: AlignContent) {
    // Positional alignment is unsafe unless the style protocol explicitly
    // represents a `safe` qualifier (it currently does not), so center/end
    // retain negative free space and may overflow both edges. Distributed
    // values use their safe fallback when space is negative.
    let free = container_size - tracks.used_size();
    let visible = tracks
        .tracks
        .iter()
        .filter(|track| !track.collapsed)
        .count();
    let (offset, distributed_gap) = match alignment {
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
    };

    let mut cursor = offset;
    let mut previous_visible = false;
    for track in &mut tracks.tracks {
        if track.collapsed {
            track.position = cursor;
            previous_visible = false;
            continue;
        }
        if previous_visible {
            cursor += tracks.gap + distributed_gap;
        }
        track.position = cursor;
        cursor += track.base;
        previous_visible = true;
    }
}

/// Offset of an item inside the free space of one grid-area axis.
#[inline]
pub(super) fn item_alignment_offset(
    free_space: f32,
    alignment: crate::style::AlignItems,
    axis_reversed: bool,
) -> f32 {
    match alignment {
        crate::style::AlignItems::Start | crate::style::AlignItems::FlexStart => {
            if axis_reversed {
                free_space
            } else {
                0.0
            }
        }
        crate::style::AlignItems::End | crate::style::AlignItems::FlexEnd => {
            if axis_reversed {
                0.0
            } else {
                free_space
            }
        }
        crate::style::AlignItems::Center => free_space / 2.0,
        crate::style::AlignItems::Baseline | crate::style::AlignItems::Stretch => 0.0,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::style::{AlignItems, TrackSizingFunction};

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
    fn collapsed_track_breaks_the_gutter_and_reversed_end_stays_at_zero() {
        let mut tracks = TrackSet {
            tracks: vec![track(10.0, false), track(0.0, true), track(10.0, false)],
            gap: 7.0,
            first_coordinate: 0,
        };
        align_tracks(&mut tracks, 20.0, AlignContent::Start);
        assert_eq!(tracks.tracks[0].position, 0.0);
        assert_eq!(tracks.tracks[1].position, 10.0);
        assert_eq!(tracks.tracks[2].position, 10.0);
        assert_eq!(item_alignment_offset(13.0, AlignItems::FlexEnd, true), 0.0);
    }
}
