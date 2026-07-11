//! CSS Box Alignment (Level 3) vocabulary shared by flexbox and grid.
//!
//! The style traits expose alignment as `Option<…>` where `None` means the
//! property's `normal`/`auto` keyword — its meaning is context-dependent
//! (`normal` behaves as `stretch` on flex/grid containers; `auto` on an item
//! defers to the container's `*-items` value), and that resolution is
//! algorithm business, not protocol business.

/// Alignment of items within their container on one axis: the value space of
/// `align-items`/`align-self` (both flex and grid) and
/// `justify-items`/`justify-self` (grid only — in flexbox the main axis has
/// no per-item justification).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlignItems {
    /// Align to the start edge of the alignment container.
    Start,
    /// Align to the end edge of the alignment container.
    End,
    /// Flexbox-compat: start of the *flex* axis (differs from `Start` in
    /// `*-reverse` directions).
    FlexStart,
    /// Flexbox-compat: end of the *flex* axis.
    FlexEnd,
    /// Center within the alignment container.
    Center,
    /// Align first baselines.
    Baseline,
    /// Stretch to fill the alignment container (respecting min/max clamps).
    Stretch,
}

/// `align-self` shares `align-items`' value space (`None` = `auto`).
pub type AlignSelf = AlignItems;
/// `justify-items` (grid) shares `align-items`' value space.
pub type JustifyItems = AlignItems;
/// `justify-self` (grid) shares `align-items`' value space (`None` = `auto`).
pub type JustifySelf = AlignItems;

/// Distribution of free space between/around content on one axis: the value
/// space of `align-content` and `justify-content`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlignContent {
    /// Pack toward the start edge of the container.
    Start,
    /// Pack toward the end edge of the container.
    End,
    /// Flexbox-compat: start of the *flex* axis.
    FlexStart,
    /// Flexbox-compat: end of the *flex* axis.
    FlexEnd,
    /// Center within the container.
    Center,
    /// Stretch lines/tracks to fill the container.
    Stretch,
    /// Even gaps between items, none at the edges.
    SpaceBetween,
    /// Even gaps between and around items (edge gaps equal to inner gaps).
    SpaceEvenly,
    /// Even gaps around each item (edge gaps half the inner gaps).
    SpaceAround,
}

/// `justify-content` shares `align-content`'s value space.
pub type JustifyContent = AlignContent;
