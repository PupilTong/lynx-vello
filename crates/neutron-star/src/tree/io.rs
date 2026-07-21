//! The layout **wire format**: the value types that flow between host and
//! engine on every `compute_child_layout` call.
//!
//! One node-layout exchange is `LayoutInput → LayoutOutput`; the durable
//! per-node result the host stores is [`Layout`]. All three are `Copy` PODs.
//! [`LayoutInput`] and [`LayoutOutput`] are `#[non_exhaustive]` because the
//! protocol is expected to grow (e.g. block-layout margin collapsing adds
//! fields); construct them with the provided constructors, or via
//! `..Default::default()`-style field assignment on a `default()` value.

use crate::geometry::{Edges, Point, Size};

/// Whether a measurement respects the node's own sizing styles.
///
/// This distinction is CSS's "content-based size" vs "used size": when a
/// flex/grid algorithm needs an item's *content contribution* it asks with
/// [`SizingMode::ContentSize`] (styles applied by the caller instead), while
/// ordinary child layout uses [`SizingMode::InherentSize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SizingMode {
    /// Apply the node's own `size`/`min-size`/`max-size`/`aspect-ratio`.
    #[default]
    InherentSize,
    /// Ignore the node's own sizing styles; measure pure content.
    ContentSize,
}

/// Which axes a [`LayoutGoal::Measure`] probe actually needs.
///
/// A hint, not a contract: algorithms may compute both axes anyway (they
/// often fall out together), but a host/leaf can use this to skip expensive
/// work — e.g. text needs no line-breaking to answer a width-only probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RequestedAxis {
    /// Only the horizontal size is needed.
    Horizontal,
    /// Only the vertical size is needed.
    Vertical,
    /// Both sizes are needed.
    #[default]
    Both,
}

/// What the caller wants from a layout pass over one node.
///
/// Measurement is side-effect free: child layouts must not be stored. A
/// commit produces final sizes and positions for the node's children and
/// stores them through
/// [`LayoutNode::set_unrounded_layout`](crate::tree::LayoutNode::set_unrounded_layout).
/// Hidden-subtree zeroing is a separate operation provided by
/// [`hide_subtree`](crate::compute::hide_subtree), not a
/// sizing goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LayoutGoal {
    /// Only compute the node's size along the requested axes.
    Measure(RequestedAxis),
    /// Produce and store final child geometry.
    #[default]
    Commit,
}

/// The space a layout pass may size a node into, per axis (CSS Sizing's
/// *available space*).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AvailableSpace {
    /// A definite number of CSS pixels is available.
    Definite(f32),
    /// Size under a min-content constraint (as small as possible without
    /// overflowing content).
    MinContent,
    /// Size under a max-content constraint (ideal unconstrained size).
    MaxContent,
}

impl AvailableSpace {
    /// Is this a definite pixel amount?
    #[must_use]
    pub const fn is_definite(self) -> bool {
        matches!(self, Self::Definite(_))
    }

    /// The definite pixel amount, if any.
    #[must_use]
    pub const fn into_option(self) -> Option<f32> {
        match self {
            Self::Definite(value) => Some(value),
            _ => None,
        }
    }
}

impl Default for AvailableSpace {
    /// `MaxContent` — the unconstrained default.
    fn default() -> Self {
        Self::MaxContent
    }
}

impl From<f32> for AvailableSpace {
    fn from(value: f32) -> Self {
        Self::Definite(value)
    }
}

impl From<Option<f32>> for AvailableSpace {
    /// `None` becomes [`AvailableSpace::MaxContent`].
    fn from(value: Option<f32>) -> Self {
        value.map_or(Self::MaxContent, Self::Definite)
    }
}

impl Size<AvailableSpace> {
    /// Max-content constraint on both axes.
    pub const MAX_CONTENT: Self = Self {
        width: AvailableSpace::MaxContent,
        height: AvailableSpace::MaxContent,
    };

    /// Min-content constraint on both axes.
    pub const MIN_CONTENT: Self = Self {
        width: AvailableSpace::MinContent,
        height: AvailableSpace::MinContent,
    };

    /// Drops the intrinsic-constraint variants, keeping definite pixels.
    #[must_use]
    pub fn into_options(self) -> Size<Option<f32>> {
        Size {
            width: self.width.into_option(),
            height: self.height.into_option(),
        }
    }
}

/// Everything an algorithm may know about a node before laying it out.
///
/// Semantics of the sizing fields (all in CSS pixels, all border-box):
///
/// - `known_dimensions` — sizes already **decided** by the caller (e.g. a stretched flex item's
///   cross size). An algorithm must return exactly these where present.
/// - `definite_dimensions` — whether each decided size is also *definite* for percentage
///   propagation. Flex sizing can decide a used size that remains indefinite under Flexbox §9.8;
///   geometry and percentage definiteness therefore cannot share one sentinel.
/// - `parent_size` — the parent's content-box size where definite; the basis for resolving this
///   node's percentage styles.
/// - `available_space` — the constraint to size into. `Definite` here does **not** force a size
///   (that's `known_dimensions`); it's the space to wrap/shrink against.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct LayoutInput {
    /// Whether this call measures the node or commits child geometry.
    pub goal: LayoutGoal,
    /// Whether this node's own sizing styles apply.
    pub sizing_mode: SizingMode,
    /// Border-box sizes already decided by the caller.
    pub known_dimensions: Size<Option<f32>>,
    /// Whether each known dimension establishes a definite percentage basis
    /// for this node's descendants.
    pub definite_dimensions: Size<bool>,
    /// The parent's definite content-box size (percentage basis).
    pub parent_size: Size<Option<f32>>,
    /// The space to size into.
    pub available_space: Size<AvailableSpace>,
}

impl LayoutInput {
    /// A full-layout request. `sizing_mode` defaults to
    /// [`SizingMode::InherentSize`]; assign fields to deviate.
    #[must_use]
    pub fn perform_layout(
        known_dimensions: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
    ) -> Self {
        Self {
            goal: LayoutGoal::Commit,
            sizing_mode: SizingMode::InherentSize,
            known_dimensions,
            definite_dimensions: known_dimensions.map(|value| value.is_some()),
            parent_size,
            available_space,
        }
    }

    /// A measurement probe (no child layouts are stored).
    #[must_use]
    pub fn compute_size(
        known_dimensions: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
        requested_axis: RequestedAxis,
    ) -> Self {
        Self {
            goal: LayoutGoal::Measure(requested_axis),
            sizing_mode: SizingMode::InherentSize,
            known_dimensions,
            definite_dimensions: known_dimensions.map(|value| value.is_some()),
            parent_size,
            available_space,
        }
    }
}

/// What one layout pass reports back to its caller.
///
/// This is the *transient* answer the parent algorithm consumes; the durable
/// per-node record is [`Layout`], stored separately via
/// [`LayoutNode::set_unrounded_layout`](crate::tree::LayoutNode::set_unrounded_layout).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct LayoutOutput {
    /// The node's border-box size.
    pub size: Size<f32>,
    /// The node's scrollable-overflow size: the extent of content measured
    /// from the border-box origin, ≥ `size` minus borders. This is the node's
    /// **own** scroll range and the host's scroll extent. It feeds an
    /// ancestor's `content_size` only when the node is *not* a scroll container
    /// (`overflow: visible`): a scroll container traps its interior overflow
    /// and contributes only its border box upward (CSS Overflow 3 §3.3). A
    /// layout-contained box with `overflow: visible` reports its border box
    /// here (descendant overflow is ink overflow, css-contain-2 §3.3).
    pub content_size: Size<f32>,
    /// First-baseline offsets from the border-box origin, per axis, if the
    /// node has baselines (`y` is the horizontal-text baseline used by
    /// flexbox `align-items: baseline`).
    pub first_baselines: Point<Option<f32>>,
}

impl LayoutOutput {
    /// The all-zero output a host returns after [`hide_subtree`](crate::compute::hide_subtree).
    pub const HIDDEN: Self = Self {
        size: Size::ZERO,
        content_size: Size::ZERO,
        first_baselines: Point::NONE,
    };

    /// An output with no baselines.
    #[must_use]
    pub fn new(size: Size<f32>, content_size: Size<f32>) -> Self {
        Self {
            size,
            content_size,
            first_baselines: Point::NONE,
        }
    }

    /// Adds first-baseline information.
    #[must_use]
    pub fn with_first_baselines(mut self, first_baselines: Point<Option<f32>>) -> Self {
        self.first_baselines = first_baselines;
        self
    }
}

/// The durable, host-stored layout of one node.
///
/// Coordinate contract: `location` is the offset of this node's **border-box
/// origin from its parent's border-box origin**, before any transform, with
/// ancestor scroll offsets *not* applied (scrolling is presentation, applied
/// by the host/renderer). Relative-position insets are already applied.
/// Values are unrounded CSS pixels until
/// [`round_layout`](crate::compute::round_layout) writes the rounded copy
/// via [`LayoutNode::set_final_layout`](crate::tree::LayoutNode::set_final_layout).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct Layout {
    /// Paint/traversal order among siblings: the node's index after sorting
    /// by style `order` (stable within equal values). Not related to
    /// `z-index`, which the host's paint layer owns.
    pub order: u32,
    /// Border-box origin relative to the parent's border-box origin.
    pub location: Point<f32>,
    /// Border-box size.
    pub size: Size<f32>,
    /// Scrollable-overflow size (see [`LayoutOutput::content_size`]).
    pub content_size: Size<f32>,
    /// Used border widths.
    pub border: Edges<f32>,
    /// Used padding.
    pub padding: Edges<f32>,
    /// Used margins (`auto` margins resolved).
    pub margin: Edges<f32>,
}

impl Layout {
    /// A zeroed layout with the given sibling order.
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

        assert_eq!(AvailableSpace::Definite(12.0).into_option(), Some(12.0));
        assert_eq!(AvailableSpace::MinContent.into_option(), None);
        assert_eq!(AvailableSpace::MaxContent.into_option(), None);
        assert_eq!(AvailableSpace::default(), AvailableSpace::MaxContent);
        assert_eq!(AvailableSpace::from(18.0), AvailableSpace::Definite(18.0));
        assert_eq!(
            AvailableSpace::from(Some(20.0)),
            AvailableSpace::Definite(20.0)
        );
        assert_eq!(AvailableSpace::from(None), AvailableSpace::MaxContent);

        let options =
            Size::new(AvailableSpace::Definite(30.0), AvailableSpace::MinContent).into_options();
        assert_eq!(options, Size::new(Some(30.0), None));
    }

    #[test]
    fn layout_wire_constructors_set_only_their_documented_fields() {
        let known = Size::new(Some(100.0), None);
        let parent = Size::new(Some(200.0), Some(150.0));
        let available = Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent);

        let commit = LayoutInput::perform_layout(known, parent, available);
        assert_eq!(commit.goal, LayoutGoal::Commit);
        assert_eq!(commit.sizing_mode, SizingMode::InherentSize);
        assert_eq!(commit.known_dimensions, known);

        let measure =
            LayoutInput::compute_size(known, parent, available, RequestedAxis::Horizontal);
        assert_eq!(measure.goal, LayoutGoal::Measure(RequestedAxis::Horizontal));

        let baselines = Point::new(None, Some(14.0));
        let output = LayoutOutput::new(Size::new(100.0, 20.0), Size::new(120.0, 30.0))
            .with_first_baselines(baselines);
        assert_eq!(output.first_baselines, baselines);
        assert_eq!(Layout::with_order(7).order, 7);
    }
}
