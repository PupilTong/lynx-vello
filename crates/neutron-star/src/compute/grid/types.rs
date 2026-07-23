//! Transient, algorithm-private Grid state.

use stylo::computed_values::{box_sizing, direction};
use stylo::values::computed::{
    LengthPercentage, MaxSize as StyleMaxSize, Overflow, PositionProperty, Size as StyleSize,
    TrackBreadth, TrackSize,
};
use stylo::values::specified::align::AlignFlags;

use super::placement::GridArea;
use crate::compute::util::ItemKey;
use crate::geometry::{Edges, Point, Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    pub(super) const ALL: [Self; 2] = [Self::Horizontal, Self::Vertical];

    #[inline]
    pub(super) const fn other(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }

    #[inline]
    pub(super) fn size<T>(self, size: Size<T>) -> T {
        match self {
            Self::Horizontal => size.width,
            Self::Vertical => size.height,
        }
    }

    #[inline]
    pub(super) fn set_size<T>(self, size: &mut Size<T>, value: T) {
        match self {
            Self::Horizontal => size.width = value,
            Self::Vertical => size.height = value,
        }
    }

    #[inline]
    pub(super) fn start<T: Copy>(self, edges: Edges<T>) -> T {
        match self {
            Self::Horizontal => edges.left,
            Self::Vertical => edges.top,
        }
    }

    #[inline]
    pub(super) fn end<T: Copy>(self, edges: Edges<T>) -> T {
        match self {
            Self::Horizontal => edges.right,
            Self::Vertical => edges.bottom,
        }
    }

    #[inline]
    pub(super) fn sum(self, edges: Edges<f32>) -> f32 {
        self.start(edges) + self.end(edges)
    }

    #[inline]
    pub(super) fn size_ref<T>(self, size: &Size<T>) -> &T {
        match self {
            Self::Horizontal => &size.width,
            Self::Vertical => &size.height,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum IntrinsicSize {
    MinContent,
    MaxContent,
    FitContent(LengthPercentage),
    None,
}

impl IntrinsicSize {
    pub(super) fn from_size(value: &StyleSize) -> Self {
        match value {
            StyleSize::MinContent => Self::MinContent,
            StyleSize::MaxContent => Self::MaxContent,
            StyleSize::FitContentFunction(limit) => Self::FitContent(limit.0.clone()),
            StyleSize::Auto
            | StyleSize::LengthPercentage(_)
            | StyleSize::FitContent
            | StyleSize::Stretch
            | StyleSize::WebkitFillAvailable => Self::None,
            StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
                unreachable!("anchor positioning is pref-disabled under lynx")
            }
        }
    }

    pub(super) fn from_max_size(value: &StyleMaxSize) -> Self {
        match value {
            StyleMaxSize::MinContent => Self::MinContent,
            StyleMaxSize::MaxContent => Self::MaxContent,
            StyleMaxSize::FitContentFunction(limit) => Self::FitContent(limit.0.clone()),
            StyleMaxSize::None
            | StyleMaxSize::LengthPercentage(_)
            | StyleMaxSize::FitContent
            | StyleMaxSize::Stretch
            | StyleMaxSize::WebkitFillAvailable => Self::None,
            StyleMaxSize::AnchorSizeFunction(_) | StyleMaxSize::AnchorContainingCalcFunction(_) => {
                unreachable!("anchor positioning is pref-disabled under lynx")
            }
        }
    }
}

/// The normalized `minmax()` halves of one track sizing function.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TrackSizingFunction {
    pub(super) min: TrackBreadth,
    pub(super) max: TrackBreadth,
    pub(super) fit_content: Option<LengthPercentage>,
}

impl TrackSizingFunction {
    pub(super) const AUTO: Self = Self {
        min: TrackBreadth::Auto,
        max: TrackBreadth::Auto,
        fit_content: None,
    };

    pub(super) fn from_style(size: &TrackSize) -> Self {
        match size {
            TrackSize::Breadth(TrackBreadth::Flex(flex)) => Self {
                min: TrackBreadth::Auto,
                max: TrackBreadth::Flex(*flex),
                fit_content: None,
            },
            TrackSize::Breadth(breadth) => Self {
                min: breadth.clone(),
                max: breadth.clone(),
                fit_content: None,
            },
            TrackSize::Minmax(min, max) => Self {
                min: min.clone(),
                max: max.clone(),
                fit_content: None,
            },
            TrackSize::FitContent(TrackBreadth::Breadth(limit)) => Self {
                min: TrackBreadth::Auto,
                max: TrackBreadth::MaxContent,
                fit_content: Some(limit.clone()),
            },
            TrackSize::FitContent(_) => {
                unreachable!("fit-content() stores a <length-percentage> breadth by construction")
            }
        }
    }
}

impl Default for TrackSizingFunction {
    fn default() -> Self {
        Self::AUTO
    }
}

/// One placed in-flow item with resolved box values and local contribution
/// caches.  Contributions are invalidated only for the bounded column/row
/// reruns required by Grid sizing.
#[derive(Debug, Clone)]
pub(super) struct GridItem<N> {
    pub(super) key: ItemKey<N>,
    pub(super) area: GridArea,
    pub(super) position: PositionProperty,
    pub(super) align_self: AlignFlags,
    pub(super) justify_self: AlignFlags,
    pub(super) direction: direction::T,
    pub(super) aspect_ratio: Option<f32>,
    pub(super) box_sizing: box_sizing::T,
    pub(super) overflow: Point<Overflow>,
    pub(super) preferred_behaves_auto_or_depends: Size<bool>,
    pub(super) minimum_is_auto: Size<bool>,
    pub(super) intrinsic_preferred: Size<IntrinsicSize>,
    pub(super) intrinsic_min: Size<IntrinsicSize>,
    pub(super) intrinsic_max: Size<IntrinsicSize>,
    pub(super) preferred_size: Size<Option<f32>>,
    pub(super) min_size: Size<Option<f32>>,
    pub(super) max_size: Size<Option<f32>>,
    pub(super) margin: Edges<f32>,
    pub(super) margin_auto: Edges<bool>,
    pub(super) padding: Edges<f32>,
    pub(super) border: Edges<f32>,
    pub(super) inset: Edges<Option<f32>>,
    pub(super) raw_min_content: Size<Option<f32>>,
    pub(super) raw_max_content: Size<Option<f32>>,
    pub(super) minimum_contribution: Size<Option<f32>>,
    pub(super) min_content_contribution: Size<Option<f32>>,
    pub(super) max_content_contribution: Size<Option<f32>>,
    pub(super) measured_baselines: Point<Option<f32>>,
    pub(super) baseline_shim: f32,
}

impl<N> GridItem<N> {
    #[inline]
    pub(super) fn span(&self, axis: Axis) -> usize {
        let span = match axis {
            Axis::Horizontal => self.area.column,
            Axis::Vertical => self.area.row,
        };
        usize::try_from(span.end - span.start).unwrap_or(1).max(1)
    }

    #[inline]
    pub(super) fn clear_contribution_cache(&mut self, axis: Axis) {
        axis.set_size(&mut self.minimum_contribution, None);
        axis.set_size(&mut self.min_content_contribution, None);
        axis.set_size(&mut self.max_content_contribution, None);
        axis.set_size(&mut self.raw_min_content, None);
        axis.set_size(&mut self.raw_max_content, None);
        if axis == Axis::Vertical {
            self.measured_baselines.y = None;
            self.baseline_shim = 0.0;
        }
    }
}

/// Cold style plus hot used values for one concrete explicit/implicit track.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct Track {
    pub(super) sizing: TrackSizingFunction,
    pub(super) base: f32,
    pub(super) growth_limit: f32,
    pub(super) fit_content_limit: f32,
    pub(super) flex_factor: f32,
    pub(super) flexible: bool,
    pub(super) intrinsic_min: bool,
    pub(super) intrinsic_max: bool,
    pub(super) auto_max: bool,
    pub(super) infinitely_growable: bool,
    pub(super) collapsed: bool,
    pub(super) position: f32,
}

impl Track {
    #[inline]
    pub(super) fn is_flexible(&self) -> bool {
        self.flexible
    }
}

/// Concrete tracks and the used gap in one physical axis.
#[derive(Debug, Default)]
pub(super) struct TrackSet {
    pub(super) tracks: Vec<Track>,
    pub(super) gap: f32,
    pub(super) first_coordinate: i32,
    pub(super) collapsed_line_positions: Option<Vec<f32>>,
}

impl TrackSet {
    #[inline]
    pub(super) fn index_of(&self, coordinate: i32) -> usize {
        usize::try_from(coordinate - self.first_coordinate)
            .expect("placement coordinates are inside the materialized grid")
    }

    #[inline]
    pub(super) fn span_indices(&self, start: i32, end: i32) -> core::ops::Range<usize> {
        self.index_of(start)..self.index_of(end)
    }

    pub(super) fn used_size(&self) -> f32 {
        self.tracks.iter().map(|track| track.base).sum::<f32>() + self.total_gap()
    }

    pub(super) fn total_gap(&self) -> f32 {
        let visible = self.tracks.iter().filter(|track| !track.collapsed).count();
        #[allow(clippy::cast_precision_loss)]
        {
            self.gap * visible.saturating_sub(1) as f32
        }
    }

    pub(super) fn rebuild_positions(&mut self) {
        self.rebuild_positions_with_spacing(0.0, 0.0);
    }

    pub(super) fn rebuild_aligned_positions(&mut self, offset: f32, distributed_gap: f32) {
        self.rebuild_positions_with_spacing(offset, distributed_gap);
    }

    fn rebuild_positions_with_spacing(&mut self, offset: f32, distributed_gap: f32) {
        let mut cursor = offset;
        let mut previous_visible = false;
        let mut saw_collapsed = false;
        for track in &mut self.tracks {
            if track.collapsed {
                saw_collapsed = true;
                track.position = cursor;
                continue;
            }
            if previous_visible {
                cursor += self.gap + distributed_gap;
            }
            track.position = cursor;
            cursor += track.base;
            previous_visible = true;
        }

        if saw_collapsed {
            let positions = self.collapsed_line_positions.get_or_insert_with(Vec::new);
            positions.resize(self.tracks.len(), 0.0);
            let mut next_visible_position = None;
            for (index, track) in self.tracks.iter().enumerate().rev() {
                if track.collapsed {
                    positions[index] = next_visible_position.unwrap_or(track.position);
                } else {
                    positions[index] = track.position;
                    next_visible_position = Some(track.position);
                }
            }
        } else {
            self.collapsed_line_positions = None;
        }
    }

    #[inline]
    pub(super) fn area_size(&self, start: i32, end: i32) -> f32 {
        (self.end_line_position(end) - self.line_position(start)).max(0.0)
    }

    pub(super) fn definite_max_area_size(
        &self,
        start: i32,
        end: i32,
        distributed_gap: f32,
    ) -> Option<f32> {
        let range = self.span_indices(start, end);
        let visible = self.tracks[range.clone()]
            .iter()
            .filter(|track| !track.collapsed)
            .count();
        #[allow(clippy::cast_precision_loss)]
        let mut size = (self.gap + distributed_gap) * visible.saturating_sub(1) as f32;
        for track in &self.tracks[range] {
            if track.collapsed {
                continue;
            }
            if !track.growth_limit.is_finite() {
                return None;
            }
            size += track.growth_limit;
        }
        Some(size)
    }

    pub(super) fn line_position(&self, coordinate: i32) -> f32 {
        if coordinate <= self.first_coordinate {
            return self.tracks.first().map_or(0.0, |track| track.position);
        }
        let track_count = i32::try_from(self.tracks.len()).expect("grid tracks are clamped");
        if coordinate >= self.first_coordinate + track_count {
            return self
                .tracks
                .iter()
                .rev()
                .find(|track| !track.collapsed)
                .or_else(|| self.tracks.last())
                .map_or(0.0, |track| track.position + track.base);
        }
        let index = self.index_of(coordinate);
        let track = &self.tracks[index];
        if !track.collapsed {
            return track.position;
        }
        self.collapsed_line_positions
            .as_ref()
            .map_or(track.position, |positions| positions[index])
    }

    pub(super) fn end_line_position(&self, coordinate: i32) -> f32 {
        if coordinate <= self.first_coordinate {
            return self.tracks.first().map_or(0.0, |track| track.position);
        }
        let end_coordinate = self.first_coordinate
            + i32::try_from(self.tracks.len()).expect("grid tracks are clamped");
        let previous = coordinate.min(end_coordinate) - 1;
        let track = &self.tracks[self.index_of(previous)];
        track.position + track.base
    }
}
