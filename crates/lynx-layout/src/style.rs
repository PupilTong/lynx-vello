//! Computed-style vocabulary for Lynx's `display: linear` algorithm.
//!
//! These values are the layout-facing result of style resolution, not raw CSS
//! parser tokens. A concrete Lynx/stylo bridge owns compatibility precedence
//! between `linear-direction` and the legacy `linear-orientation` spelling and
//! exposes the resulting effective orientation through
//! [`LinearContainerStyle::linear_orientation`].

use neutron_star::style::{AlignItems, AlignSelf, CoreStyle, JustifyContent};

/// Main-axis orientation of a linear container.
///
/// The horizontal/vertical spellings are compatibility aliases for the
/// corresponding row/column directions. They remain distinct variants at the
/// public protocol boundary because all eight values are accepted computed
/// inputs; algorithms should use [`is_horizontal`](Self::is_horizontal) and
/// [`is_reverse`](Self::is_reverse) rather than matching aliases separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LinearOrientation {
    /// Horizontal main axis, natural direction.
    Row,
    /// Horizontal main axis, reversed direction.
    RowReverse,
    /// Vertical main axis, natural direction.
    Column,
    /// Vertical main axis, reversed direction.
    ColumnReverse,
    /// Compatibility alias of [`Row`](Self::Row).
    Horizontal,
    /// Compatibility alias of [`RowReverse`](Self::RowReverse).
    HorizontalReverse,
    /// Compatibility alias of [`Column`](Self::Column).
    #[default]
    Vertical,
    /// Compatibility alias of [`ColumnReverse`](Self::ColumnReverse).
    VerticalReverse,
}

impl LinearOrientation {
    /// Whether the main axis is horizontal.
    #[must_use]
    pub const fn is_horizontal(self) -> bool {
        matches!(
            self,
            Self::Row | Self::RowReverse | Self::Horizontal | Self::HorizontalReverse
        )
    }

    /// Whether the main axis is vertical.
    #[must_use]
    pub const fn is_vertical(self) -> bool {
        !self.is_horizontal()
    }

    /// Whether the orientation reverses its main axis before physical export.
    #[must_use]
    pub const fn is_reverse(self) -> bool {
        matches!(
            self,
            Self::RowReverse
                | Self::ColumnReverse
                | Self::HorizontalReverse
                | Self::VerticalReverse
        )
    }
}

/// Current-property spelling for [`LinearOrientation`].
///
/// `linear-direction` and the legacy `linear-orientation` feed the same
/// computed value space; the future stylo bridge owns their cascade and
/// compatibility precedence.
pub type LinearDirection = LinearOrientation;

/// Container-level main-axis packing for linear items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LinearGravity {
    /// Defer to `justify-content` mapping.
    #[default]
    None,
    /// Pack at logical main-start.
    Start,
    /// Pack at logical main-end.
    End,
    /// Center along the main axis.
    Center,
    /// Distribute non-negative free space between items.
    SpaceBetween,
    /// Pack at the physical left edge when applicable.
    Left,
    /// Pack at the physical right edge when applicable.
    Right,
    /// Pack at the physical top edge when applicable.
    Top,
    /// Pack at the physical bottom edge when applicable.
    Bottom,
    /// Center on the physical horizontal axis when applicable.
    CenterHorizontal,
    /// Center on the physical vertical axis when applicable.
    CenterVertical,
}

/// Container fallback for cross-axis item alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LinearCrossGravity {
    /// No linear-specific fallback.
    #[default]
    None,
    /// Align at logical cross-start.
    Start,
    /// Align at logical cross-end.
    End,
    /// Center on the cross axis.
    Center,
    /// Stretch auto-sized items across a definite cross size.
    Stretch,
}

/// Per-item cross-axis alignment inside a linear container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LinearLayoutGravity {
    /// Defer to standard and container-level alignment fallbacks.
    #[default]
    None,
    /// Align at logical cross-start.
    Start,
    /// Align at logical cross-end.
    End,
    /// Center on the cross axis.
    Center,
    /// Stretch across a definite cross size.
    Stretch,
    /// Align to the physical left edge when applicable.
    Left,
    /// Align to the physical right edge when applicable.
    Right,
    /// Align to the physical top edge when applicable.
    Top,
    /// Align to the physical bottom edge when applicable.
    Bottom,
    /// Center on the physical horizontal axis when applicable.
    CenterHorizontal,
    /// Center on the physical vertical axis when applicable.
    CenterVertical,
    /// Compatibility fill alias; forces the definite cross-axis constraint.
    FillHorizontal,
    /// Compatibility fill alias; forces the definite cross-axis constraint.
    FillVertical,
}

/// Computed style of a node as a linear container.
///
/// Defaults are the standalone linear protocol's initial values. A concrete
/// Lynx bridge may materialize compatibility defaults before returning this
/// view.
pub trait LinearContainerStyle: CoreStyle {
    /// Effective `linear-orientation` / `linear-direction` value.
    fn linear_orientation(&self) -> LinearOrientation {
        LinearOrientation::Vertical
    }

    /// `linear-gravity` main-axis packing.
    fn linear_gravity(&self) -> LinearGravity {
        LinearGravity::None
    }

    /// `linear-cross-gravity` cross-axis fallback.
    fn linear_cross_gravity(&self) -> LinearCrossGravity {
        LinearCrossGravity::None
    }

    /// `linear-weight-sum`; a positive value overrides the distribution
    /// denominator, while zero requests the sum of participating item weights.
    fn linear_weight_sum(&self) -> f32 {
        0.0
    }

    /// `justify-content`, used when [`linear_gravity`](Self::linear_gravity)
    /// is [`LinearGravity::None`]. `None` represents `normal`.
    fn justify_content(&self) -> Option<JustifyContent> {
        None
    }

    /// `align-items`, consulted after the linear-specific cross-axis
    /// fallbacks. `None` represents `normal`.
    fn align_items(&self) -> Option<AlignItems> {
        None
    }
}

/// Computed style of a node as an item of a linear container.
pub trait LinearItemStyle: CoreStyle {
    /// `linear-layout-gravity` per-item cross-axis alignment.
    fn linear_layout_gravity(&self) -> LinearLayoutGravity {
        LinearLayoutGravity::None
    }

    /// `linear-weight`; only positive values participate in weighted
    /// main-size distribution.
    fn linear_weight(&self) -> f32 {
        0.0
    }

    /// Standard `align-self`; `None` represents `auto`.
    fn align_self(&self) -> Option<AlignSelf> {
        None
    }

    /// Standard `order`; lower values are laid out first, stably within ties.
    fn order(&self) -> i32 {
        0
    }
}

impl<S: LinearContainerStyle> LinearContainerStyle for &S {
    fn linear_orientation(&self) -> LinearOrientation {
        (**self).linear_orientation()
    }

    fn linear_gravity(&self) -> LinearGravity {
        (**self).linear_gravity()
    }

    fn linear_cross_gravity(&self) -> LinearCrossGravity {
        (**self).linear_cross_gravity()
    }

    fn linear_weight_sum(&self) -> f32 {
        (**self).linear_weight_sum()
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        (**self).justify_content()
    }

    fn align_items(&self) -> Option<AlignItems> {
        (**self).align_items()
    }
}

impl<S: LinearItemStyle> LinearItemStyle for &S {
    fn linear_layout_gravity(&self) -> LinearLayoutGravity {
        (**self).linear_layout_gravity()
    }

    fn linear_weight(&self) -> f32 {
        (**self).linear_weight()
    }

    fn align_self(&self) -> Option<AlignSelf> {
        (**self).align_self()
    }

    fn order(&self) -> i32 {
        (**self).order()
    }
}
