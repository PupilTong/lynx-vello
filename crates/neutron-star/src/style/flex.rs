//! Flexbox style protocol (CSS Flexible Box Layout Module Level 1).
//!
//! Two traits, mirroring the spec's split of responsibilities: the
//! **container** decides axes, wrapping, and distribution
//! ([`FlexContainerStyle`]); each **item** decides its own flexibility and
//! self-alignment ([`FlexItemStyle`]). The L1 flexbox algorithm reads
//! containers through [`FlexSource::flex_container_style`] and items through
//! [`FlexSource::flex_item_style`].
//!
//! [`FlexSource::flex_container_style`]: crate::tree::FlexSource::flex_container_style
//! [`FlexSource::flex_item_style`]: crate::tree::FlexSource::flex_item_style

use crate::geometry::Size;
use crate::style::CoreStyle;
use crate::style::alignment::{AlignContent, AlignItems, AlignSelf, JustifyContent};
use crate::style::value::{Dimension, LengthPercentage};

/// `flex-direction`: which physical axis is the main axis, and its direction.
///
/// Note `Row`/`RowReverse` are additionally flipped by
/// [`CoreStyle::direction`] being [`Direction::Rtl`](crate::style::Direction)
/// — the flip is applied inside the algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FlexDirection {
    /// Main axis horizontal, items flow left→right (in `ltr`).
    #[default]
    Row,
    /// Main axis vertical, items flow top→bottom.
    Column,
    /// Main axis horizontal, items flow right→left (in `ltr`).
    RowReverse,
    /// Main axis vertical, items flow bottom→top.
    ColumnReverse,
}

impl FlexDirection {
    /// Is the main axis the horizontal axis?
    #[must_use]
    pub const fn is_row(self) -> bool {
        matches!(self, Self::Row | Self::RowReverse)
    }

    /// Is the main axis the vertical axis?
    #[must_use]
    pub const fn is_column(self) -> bool {
        !self.is_row()
    }

    /// Does the main axis run against its natural direction?
    #[must_use]
    pub const fn is_reverse(self) -> bool {
        matches!(self, Self::RowReverse | Self::ColumnReverse)
    }
}

/// `flex-wrap`: single-line vs multi-line flex containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FlexWrap {
    /// Single line; items shrink (or overflow) rather than wrap.
    #[default]
    NoWrap,
    /// Multi-line; new lines stack in the cross-axis direction.
    Wrap,
    /// Multi-line; new lines stack against the cross-axis direction.
    WrapReverse,
}

/// Style of a node *as a flex container*.
///
/// Defaults are the CSS initial values.
pub trait FlexContainerStyle: CoreStyle {
    /// `flex-direction`.
    fn flex_direction(&self) -> FlexDirection {
        FlexDirection::Row
    }

    /// `flex-wrap`.
    fn flex_wrap(&self) -> FlexWrap {
        FlexWrap::NoWrap
    }

    /// `gap` (`column-gap` is `width`, `row-gap` is `height`).
    ///
    /// Percentage basis: the container's content-box size in the gap's axis.
    fn gap(&self) -> Size<LengthPercentage> {
        Size::new(LengthPercentage::ZERO, LengthPercentage::ZERO)
    }

    /// `align-content` — cross-axis distribution of lines. `None` = `normal`.
    fn align_content(&self) -> Option<AlignContent> {
        None
    }

    /// `align-items` — default cross-axis alignment of items. `None` =
    /// `normal` (which behaves as `stretch` here).
    fn align_items(&self) -> Option<AlignItems> {
        None
    }

    /// `justify-content` — main-axis distribution of items. `None` =
    /// `normal`.
    fn justify_content(&self) -> Option<JustifyContent> {
        None
    }
}

/// Style of a node *as a flex item* (i.e. as read by its parent container's
/// layout).
///
/// Defaults are the CSS initial values.
pub trait FlexItemStyle: CoreStyle {
    /// `flex-basis`.
    ///
    /// Percentage basis: the container's content-box main-axis size.
    fn flex_basis(&self) -> Dimension {
        Dimension::Auto
    }

    /// `flex-grow`.
    fn flex_grow(&self) -> f32 {
        0.0
    }

    /// `flex-shrink`.
    fn flex_shrink(&self) -> f32 {
        1.0
    }

    /// `align-self`. `None` = `auto` (defer to the container's
    /// `align-items`).
    fn align_self(&self) -> Option<AlignSelf> {
        None
    }

    /// `order` — layout/paint reordering among siblings; lower comes first.
    /// Lynx supports this standard property, so it is first-class protocol.
    fn order(&self) -> i32 {
        0
    }
}

impl<S: FlexContainerStyle> FlexContainerStyle for &S {
    fn flex_direction(&self) -> FlexDirection {
        (**self).flex_direction()
    }

    fn flex_wrap(&self) -> FlexWrap {
        (**self).flex_wrap()
    }

    fn gap(&self) -> Size<LengthPercentage> {
        (**self).gap()
    }

    fn align_content(&self) -> Option<AlignContent> {
        (**self).align_content()
    }

    fn align_items(&self) -> Option<AlignItems> {
        (**self).align_items()
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        (**self).justify_content()
    }
}

impl<S: FlexItemStyle> FlexItemStyle for &S {
    fn flex_basis(&self) -> Dimension {
        (**self).flex_basis()
    }

    fn flex_grow(&self) -> f32 {
        (**self).flex_grow()
    }

    fn flex_shrink(&self) -> f32 {
        (**self).flex_shrink()
    }

    fn align_self(&self) -> Option<AlignSelf> {
        (**self).align_self()
    }

    fn order(&self) -> i32 {
        (**self).order()
    }
}
