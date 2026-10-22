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
    /// Durable child geometry. `content_independent`, per axis: the committing
    /// parent proved this input's `known_dimensions`, `parent_size` and
    /// `available_space` cannot change when only the node's own subtree
    /// content changes. Distinct from [`LayoutInput::definite_dimensions`]
    /// (CSS definiteness): a post-flexing size is definite even when it was
    /// measured from the very content whose change is in question. It never
    /// affects computed geometry and is outside the cache key; it is what
    /// licenses a host to relayout the node in place under its committed
    /// input. A measurement carries no such license, so only a commit has the
    /// field.
    Commit {
        content_independent: Size<bool>,
    },
}

impl Default for LayoutGoal {
    fn default() -> Self {
        Self::Commit {
            content_independent: Size::new(false, false),
        }
    }
}

impl LayoutGoal {
    /// Whether this goal asks for durable child geometry.
    #[must_use]
    pub const fn commits(self) -> bool {
        matches!(self, Self::Commit { .. })
    }

    /// The independence a commit carries; a measurement carries none.
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
    /// The caller reads this box's last baseline from the output, so a
    /// measurement must report it exactly. Without the request a flex
    /// container's measurement may give the item its last baseline comes
    /// from one synthesized from that item's border box, when its cross-size
    /// pass did not otherwise measure the item. A commit always reports the
    /// last baseline exactly, from the committed children, and needs no
    /// request.
    ///
    /// Set only by a caller that reads the value: a flex line's
    /// last-baseline-sharing group, a grid row's (its shims included), and a
    /// container measuring the child its own requested last baseline comes
    /// from. Never part of the computed geometry; the cache answers a request
    /// that carries it only from an entry computed with it, or from a
    /// commit.
    pub reads_last_baseline: bool,
    /// The caller reads this box's first baseline from the output, so a
    /// measurement must report it exactly. Without the request a linear
    /// container's measurement skips the probes of its fixed-size items that
    /// only its own first baseline would read, and reports one synthesized
    /// from their border boxes instead. A commit always reports the first
    /// baseline exactly, from the committed children, and needs no request.
    ///
    /// Set by every caller that reads the value from a measurement: a flex
    /// or grid container measuring a first-baseline group member, a grid
    /// row's baseline contributions, a flex, grid, grid-lanes or linear
    /// container measuring the items its own requested first baseline comes
    /// from, and a host's inline-level atoms. Never part of the computed
    /// geometry; the cache
    /// answers a request that carries it only from an entry computed with it,
    /// or from a commit.
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

    /// The same input, with [`Self::reads_last_baseline`] set as given.
    #[inline]
    #[must_use]
    pub const fn reading_last_baseline(mut self, reads: bool) -> Self {
        self.reads_last_baseline = reads;
        self
    }

    /// The same input, with [`Self::reads_first_baseline`] set as given.
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

/// What one layout pass reports back to its caller.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct LayoutOutput {
    pub size: Size<f32>,
    /// The box's scrollable overflow rectangle, in border-box coordinates:
    /// the union of its padding box and the reach of its contents, so it
    /// ends at the padding box's far edge at the least and never takes in
    /// the far border. See [`Layout::content_size`].
    pub content_size: Size<f32>,
    /// The box's first baselines, measured from its border-box origin: `y`
    /// is the alphabetic baseline of a horizontal first line (a distance
    /// down from the top edge), `x` the same for a vertical one. `None` is a
    /// box with no first baseline set (css-align-3 §9.1); whoever aligns it
    /// synthesizes one.
    pub first_baselines: Point<Option<f32>>,
    /// The box's last baselines, in the same coordinates as
    /// [`Self::first_baselines`]: the baseline set of its last line, which
    /// `last baseline` self-alignment and the containers' own last baselines
    /// read.
    pub last_baselines: Point<Option<f32>>,
}

impl LayoutOutput {
    pub const HIDDEN: Self = Self {
        size: Size::ZERO,
        content_size: Size::ZERO,
        first_baselines: Point::NONE,
        last_baselines: Point::NONE,
    };

    #[must_use]
    pub fn new(size: Size<f32>, content_size: Size<f32>) -> Self {
        Self {
            size,
            content_size,
            first_baselines: Point::NONE,
            last_baselines: Point::NONE,
        }
    }

    #[must_use]
    pub fn with_first_baselines(mut self, first_baselines: Point<Option<f32>>) -> Self {
        self.first_baselines = first_baselines;
        self
    }

    #[must_use]
    pub fn with_last_baselines(mut self, last_baselines: Point<Option<f32>>) -> Self {
        self.last_baselines = last_baselines;
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
    /// The box's scrollable overflow rectangle measured from its border-box
    /// origin ([css-overflow-3 §3.3](https://drafts.csswg.org/css-overflow-3/#scrollable)):
    /// the union of its own padding box and its contents' reach — each
    /// child's border box, its margin area in a scroll container, and its
    /// own `content_size` where its overflow is `visible`. An empty box's
    /// ends at its padding box's far edge, inside the far border, so a scroll
    /// container's scrolling area is `content_size` less the start border,
    /// never smaller than its padding box.
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

        let baselines = Point::new(None, Some(14.0));
        let output = LayoutOutput::new(Size::new(100.0, 20.0), Size::new(120.0, 30.0))
            .with_first_baselines(baselines);
        assert_eq!(output.first_baselines, baselines);
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
    fn last_baselines_are_their_own_field() {
        let output = LayoutOutput::new(Size::new(100.0, 40.0), Size::new(100.0, 40.0));
        assert_eq!(output.last_baselines, Point::NONE);
        assert_eq!(LayoutOutput::HIDDEN.last_baselines, Point::NONE);

        let first = Point::new(None, Some(14.0));
        let last = Point::new(None, Some(34.0));
        let output = output.with_first_baselines(first).with_last_baselines(last);
        assert_eq!(output.first_baselines, first);
        assert_eq!(output.last_baselines, last);
    }
}
