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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LayoutGoal {
    Measure(RequestedAxis),
    #[default]
    Commit,
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

/// Everything an algorithm may know about a node before laying it out.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct LayoutInput {
    pub goal: LayoutGoal,
    pub sizing_mode: SizingMode,
    pub known_dimensions: Size<Option<f32>>,
    pub definite_dimensions: Size<bool>,
    pub parent_size: Size<Option<f32>>,
    pub available_space: Size<AvailableSpace>,
}

impl LayoutInput {
    #[must_use]
    pub fn commit(
        known_dimensions: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
    ) -> Self {
        Self {
            goal: LayoutGoal::Commit,
            sizing_mode: SizingMode::ApplySizeStyles,
            known_dimensions,
            definite_dimensions: known_dimensions.map(|value| value.is_some()),
            parent_size,
            available_space,
        }
    }

    #[must_use]
    pub fn measure(
        known_dimensions: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
        requested_axis: RequestedAxis,
    ) -> Self {
        Self {
            goal: LayoutGoal::Measure(requested_axis),
            sizing_mode: SizingMode::ApplySizeStyles,
            known_dimensions,
            definite_dimensions: known_dimensions.map(|value| value.is_some()),
            parent_size,
            available_space,
        }
    }
}

/// What one layout pass reports back to its caller.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct LayoutOutput {
    pub size: Size<f32>,
    pub content_size: Size<f32>,
    pub first_baselines: Point<Option<f32>>,
}

impl LayoutOutput {
    pub const HIDDEN: Self = Self {
        size: Size::ZERO,
        content_size: Size::ZERO,
        first_baselines: Point::NONE,
    };

    #[must_use]
    pub fn new(size: Size<f32>, content_size: Size<f32>) -> Self {
        Self {
            size,
            content_size,
            first_baselines: Point::NONE,
        }
    }

    #[must_use]
    pub fn with_first_baselines(mut self, first_baselines: Point<Option<f32>>) -> Self {
        self.first_baselines = first_baselines;
        self
    }
}

/// The durable, host-stored layout of one node.
#[derive(Debug, PartialEq, Default)]
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

        let commit = LayoutInput::commit(known, parent, available);
        assert_eq!(commit.goal, LayoutGoal::Commit);
        assert_eq!(commit.sizing_mode, SizingMode::ApplySizeStyles);
        assert_eq!(commit.known_dimensions, known);

        let measure = LayoutInput::measure(known, parent, available, RequestedAxis::Horizontal);
        assert_eq!(measure.goal, LayoutGoal::Measure(RequestedAxis::Horizontal));

        let baselines = Point::new(None, Some(14.0));
        let output = LayoutOutput::new(Size::new(100.0, 20.0), Size::new(120.0, 30.0))
            .with_first_baselines(baselines);
        assert_eq!(output.first_baselines, baselines);
        assert_eq!(Layout::with_order(7).order, 7);
    }
}
