//! The layout **wire format**: the value types that flow between host and
//! engine on every `compute_layout` call.

use crate::geometry::{Edges, Point, Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SizingMode {
    #[default]
    ApplySizeStyles,
    IgnoreSizeStyles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RequestedAxis {
    Horizontal,
    Vertical,
    #[default]
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutGoal {
    Measure(RequestedAxis),
    Commit { content_independent: Size<bool> },
}

impl Default for LayoutGoal {
    fn default() -> Self {
        Self::Commit {
            content_independent: Size::new(false, false),
        }
    }
}

impl LayoutGoal {
    #[must_use]
    pub const fn commits(self) -> bool {
        matches!(self, Self::Commit { .. })
    }

    #[must_use]
    pub const fn independence(self) -> Option<Size<bool>> {
        match self {
            Self::Commit {
                content_independent,
            } => Some(content_independent),
            Self::Measure(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AvailableSpace {
    Definite(f32),
    MinContent,
    #[default]
    MaxContent,
}

impl AvailableSpace {
    #[must_use]
    pub const fn is_definite(self) -> bool {
        matches!(self, Self::Definite(_))
    }

    #[must_use]
    pub const fn definite_value(self) -> Option<f32> {
        match self {
            Self::Definite(value) => Some(value),
            _ => None,
        }
    }
}

impl From<f32> for AvailableSpace {
    fn from(value: f32) -> Self {
        Self::Definite(value)
    }
}

impl From<Option<f32>> for AvailableSpace {
    fn from(value: Option<f32>) -> Self {
        value.map_or(Self::MaxContent, Self::Definite)
    }
}

impl Size<AvailableSpace> {
    pub const MAX_CONTENT: Self = Self {
        width: AvailableSpace::MaxContent,
        height: AvailableSpace::MaxContent,
    };

    pub const MIN_CONTENT: Self = Self {
        width: AvailableSpace::MinContent,
        height: AvailableSpace::MinContent,
    };

    #[must_use]
    pub fn definite_values(self) -> Size<Option<f32>> {
        Size {
            width: self.width.definite_value(),
            height: self.height.definite_value(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct LayoutInput {
    pub goal: LayoutGoal,
    pub sizing_mode: SizingMode,
    pub known_dimensions: Size<Option<f32>>,
    pub definite_dimensions: Size<bool>,
    pub parent_size: Size<Option<f32>>,
    pub available_space: Size<AvailableSpace>,
    pub reads_last_baseline: bool,
    pub reads_first_baseline: bool,
}

impl LayoutInput {
    #[inline]
    #[must_use]
    pub fn new(
        goal: LayoutGoal,
        known_dimensions: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
    ) -> Self {
        Self {
            goal,
            sizing_mode: SizingMode::ApplySizeStyles,
            known_dimensions,
            definite_dimensions: known_dimensions.map(|value| value.is_some()),
            parent_size,
            available_space,
            reads_last_baseline: false,
            reads_first_baseline: false,
        }
    }

    #[inline]
    #[must_use]
    pub const fn reading_last_baseline(mut self, reads: bool) -> Self {
        self.reads_last_baseline = reads;
        self
    }

    #[inline]
    #[must_use]
    pub const fn reading_first_baseline(mut self, reads: bool) -> Self {
        self.reads_first_baseline = reads;
        self
    }

    #[inline]
    #[must_use]
    pub fn commit(
        known_dimensions: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
        content_independent: Size<bool>,
    ) -> Self {
        Self::new(
            LayoutGoal::Commit {
                content_independent,
            },
            known_dimensions,
            parent_size,
            available_space,
        )
    }

    #[inline]
    #[must_use]
    pub fn measure(
        known_dimensions: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
        requested_axis: RequestedAxis,
    ) -> Self {
        Self::new(
            LayoutGoal::Measure(requested_axis),
            known_dimensions,
            parent_size,
            available_space,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct LayoutOutput {
    pub size: Size<f32>,
    pub content_size: Size<f32>,
    pub first_baseline: Option<f32>,
    pub last_baseline: Option<f32>,
}

impl LayoutOutput {
    pub const HIDDEN: Self = Self {
        size: Size::ZERO,
        content_size: Size::ZERO,
        first_baseline: None,
        last_baseline: None,
    };

    #[must_use]
    pub fn new(size: Size<f32>, content_size: Size<f32>) -> Self {
        Self {
            size,
            content_size,
            first_baseline: None,
            last_baseline: None,
        }
    }

    #[must_use]
    pub fn with_first_baseline(mut self, first_baseline: Option<f32>) -> Self {
        self.first_baseline = first_baseline;
        self
    }

    #[must_use]
    pub fn with_last_baseline(mut self, last_baseline: Option<f32>) -> Self {
        self.last_baseline = last_baseline;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct Layout {
    pub order: u32,
    pub location: Point<f32>,
    pub size: Size<f32>,
    pub content_size: Size<f32>,
    pub border: Edges<f32>,
    pub padding: Edges<f32>,
    pub margin: Edges<f32>,
}

impl Layout {
    #[must_use]
    pub fn with_order(order: u32) -> Self {
        Self {
            order,
            ..Self::default()
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn available_space_conversions_preserve_definite_and_intrinsic_constraints() {
        assert!(AvailableSpace::Definite(12.0).is_definite());
        assert!(!AvailableSpace::MinContent.is_definite());
        assert!(!AvailableSpace::MaxContent.is_definite());

        assert_eq!(AvailableSpace::Definite(12.0).definite_value(), Some(12.0));
        assert_eq!(AvailableSpace::MinContent.definite_value(), None);
        assert_eq!(AvailableSpace::MaxContent.definite_value(), None);
        assert_eq!(AvailableSpace::default(), AvailableSpace::MaxContent);
        assert_eq!(AvailableSpace::from(18.0), AvailableSpace::Definite(18.0));
        assert_eq!(
            AvailableSpace::from(Some(20.0)),
            AvailableSpace::Definite(20.0)
        );
        assert_eq!(AvailableSpace::from(None), AvailableSpace::MaxContent);

        let options =
            Size::new(AvailableSpace::Definite(30.0), AvailableSpace::MinContent).definite_values();
        assert_eq!(options, Size::new(Some(30.0), None));
    }

    #[test]
    fn layout_wire_constructors_set_only_their_documented_fields() {
        let known = Size::new(Some(100.0), None);
        let parent = Size::new(Some(200.0), Some(150.0));
        let available = Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent);

        let independent = Size::new(true, false);
        let commit = LayoutInput::commit(known, parent, available, independent);
        assert_eq!(
            commit.goal,
            LayoutGoal::Commit {
                content_independent: independent
            }
        );
        assert!(commit.goal.commits());
        assert_eq!(commit.goal.independence(), Some(independent));
        assert_eq!(commit.sizing_mode, SizingMode::ApplySizeStyles);
        assert_eq!(commit.known_dimensions, known);

        let measure = LayoutInput::measure(known, parent, available, RequestedAxis::Horizontal);
        assert_eq!(measure.goal, LayoutGoal::Measure(RequestedAxis::Horizontal));
        assert!(!measure.goal.commits());
        assert_eq!(measure.goal.independence(), None);
        assert_eq!(LayoutGoal::default(), LayoutInput::default().goal);
        assert_eq!(
            LayoutGoal::default().independence(),
            Some(Size::new(false, false))
        );

        let output = LayoutOutput::new(Size::new(100.0, 20.0), Size::new(120.0, 30.0))
            .with_first_baseline(Some(14.0));
        assert_eq!(output.first_baseline, Some(14.0));
        assert_eq!(Layout::with_order(7).order, 7);
    }

    #[test]
    fn a_last_baseline_request_is_opt_in() {
        let measure = LayoutInput::measure(
            Size::NONE,
            Size::NONE,
            Size::MAX_CONTENT,
            RequestedAxis::Both,
        );
        assert!(!measure.reads_last_baseline);
        assert!(!LayoutInput::default().reads_last_baseline);
        let requested = measure.reading_last_baseline(true);
        assert!(requested.reads_last_baseline);
        assert_eq!(requested.goal, measure.goal);
        assert!(!requested.reading_last_baseline(false).reads_last_baseline);
    }

    #[test]
    fn a_first_baseline_request_is_opt_in() {
        let measure = LayoutInput::measure(
            Size::NONE,
            Size::NONE,
            Size::MAX_CONTENT,
            RequestedAxis::Both,
        );
        assert!(!measure.reads_first_baseline);
        assert!(!LayoutInput::default().reads_first_baseline);
        let requested = measure.reading_first_baseline(true);
        assert!(requested.reads_first_baseline);
        assert!(!requested.reads_last_baseline);
        assert_eq!(requested.goal, measure.goal);
        assert!(!requested.reading_first_baseline(false).reads_first_baseline);
    }

    #[test]
    fn the_last_baseline_is_its_own_field() {
        let output = LayoutOutput::new(Size::new(100.0, 40.0), Size::new(100.0, 40.0));
        assert_eq!(output.last_baseline, None);
        assert_eq!(LayoutOutput::HIDDEN.last_baseline, None);

        let output = output
            .with_first_baseline(Some(14.0))
            .with_last_baseline(Some(34.0));
        assert_eq!(output.first_baseline, Some(14.0));
        assert_eq!(output.last_baseline, Some(34.0));
    }
}
