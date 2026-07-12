//! Starlight relative-layout style protocol.
//!
//! `display: relative` is an id-based container formatting context. It is a
//! Lynx/Starlight extension, not CSS `position: relative`: direct in-flow
//! children may constrain their physical margin edges to the parent or to
//! another child's edges. The host still owns `display` dispatch; this module
//! only describes the computed values consumed by
//! [`compute_relative_layout`](crate::compute::compute_relative_layout).

use crate::geometry::Edges;
use crate::style::CoreStyle;

/// A `relative-*` id or reference.
///
/// `-1` means no id/reference and `0` is the reserved parent reference. Every
/// other integer may identify an item. A host should pass computed integer
/// values directly; layout deliberately does not parse or repair syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct RelativeReference(i32);

impl RelativeReference {
    /// No id or constraint (`-1`).
    pub const NONE: Self = Self(-1);

    /// The relative container (`0`).
    pub const PARENT: Self = Self(0);

    /// Wraps a computed relative id/reference.
    #[must_use]
    pub const fn new(value: i32) -> Self {
        Self(value)
    }

    /// Returns the computed integer value.
    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }

    /// Whether this is the none value.
    #[must_use]
    pub const fn is_none(self) -> bool {
        self.0 == Self::NONE.0
    }

    /// Whether this references the parent.
    #[must_use]
    pub const fn is_parent(self) -> bool {
        self.0 == Self::PARENT.0
    }

    /// Whether this value may identify another relative item.
    #[must_use]
    pub const fn is_item(self) -> bool {
        !self.is_none() && !self.is_parent()
    }
}

impl Default for RelativeReference {
    fn default() -> Self {
        Self::NONE
    }
}

impl From<i32> for RelativeReference {
    fn from(value: i32) -> Self {
        Self(value)
    }
}

impl From<RelativeReference> for i32 {
    fn from(value: RelativeReference) -> Self {
        value.0
    }
}

/// Axes selected by `relative-center`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum RelativeCenter {
    /// Do not center an unconstrained axis.
    #[default]
    None,
    /// Center an unconstrained horizontal axis.
    Horizontal,
    /// Center an unconstrained vertical axis.
    Vertical,
    /// Center both unconstrained axes.
    Both,
}

impl RelativeCenter {
    /// Whether horizontal centering is selected.
    #[must_use]
    pub const fn is_horizontal(self) -> bool {
        matches!(self, Self::Horizontal | Self::Both)
    }

    /// Whether vertical centering is selected.
    #[must_use]
    pub const fn is_vertical(self) -> bool {
        matches!(self, Self::Vertical | Self::Both)
    }
}

/// Style of a node as a relative-layout container.
pub trait RelativeContainerStyle: CoreStyle {
    /// `relative-layout-once`: use one combined dependency order and measure
    /// each item once while walking it.
    ///
    /// The standalone protocol default is `false`. A Lynx compatibility host
    /// supplies its computed default (`true`) explicitly.
    fn relative_layout_once(&self) -> bool {
        false
    }
}

/// Style of a direct in-flow relative item.
pub trait RelativeItemStyle: CoreStyle {
    /// `relative-id`; `-1` does not identify the item and `0` is reserved for
    /// the parent reference.
    fn relative_id(&self) -> RelativeReference {
        RelativeReference::NONE
    }

    /// Physical same-side alignment references.
    ///
    /// The fields correspond to `relative-align-left`, `-right`, `-top`, and
    /// `-bottom` respectively.
    fn relative_align(&self) -> Edges<RelativeReference> {
        Edges::uniform(RelativeReference::NONE)
    }

    /// Physical adjacency references.
    ///
    /// The fields correspond to `relative-left-of`, `-right-of`, `-top-of`,
    /// and `-bottom-of`. Consequently, `right` constrains the item's start
    /// edge and `left` constrains its end edge (likewise `bottom`/`top`).
    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        Edges::uniform(RelativeReference::NONE)
    }

    /// `relative-center`.
    fn relative_center(&self) -> RelativeCenter {
        RelativeCenter::None
    }

    /// Stable layout ordering among relative items.
    fn order(&self) -> i32 {
        0
    }
}

impl<S: RelativeContainerStyle> RelativeContainerStyle for &S {
    fn relative_layout_once(&self) -> bool {
        (**self).relative_layout_once()
    }
}

impl<S: RelativeItemStyle> RelativeItemStyle for &S {
    fn relative_id(&self) -> RelativeReference {
        (**self).relative_id()
    }

    fn relative_align(&self) -> Edges<RelativeReference> {
        (**self).relative_align()
    }

    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        (**self).relative_adjacent()
    }

    fn relative_center(&self) -> RelativeCenter {
        (**self).relative_center()
    }

    fn order(&self) -> i32 {
        (**self).order()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Defaults;

    impl CoreStyle for Defaults {}
    impl RelativeContainerStyle for Defaults {}
    impl RelativeItemStyle for Defaults {}

    #[test]
    fn defaults_match_the_standalone_relative_surface() {
        let style = Defaults;

        assert!(!style.relative_layout_once());
        assert_eq!(style.relative_id(), RelativeReference::NONE);
        assert_eq!(
            style.relative_align(),
            Edges::uniform(RelativeReference::NONE)
        );
        assert_eq!(
            style.relative_adjacent(),
            Edges::uniform(RelativeReference::NONE)
        );
        assert_eq!(style.relative_center(), RelativeCenter::None);
        assert_eq!(style.order(), 0);
    }

    #[test]
    fn reference_and_center_helpers_cover_the_complete_value_space() {
        assert!(RelativeReference::NONE.is_none());
        assert_eq!(RelativeReference::default(), RelativeReference::NONE);
        assert!(RelativeReference::PARENT.is_parent());
        assert!(RelativeReference::new(-2).is_item());
        assert!(RelativeReference::new(7).is_item());
        assert_eq!(RelativeReference::new(7).get(), 7);
        assert_eq!(i32::from(RelativeReference::from(9)), 9);

        assert!(!RelativeCenter::None.is_horizontal());
        assert!(!RelativeCenter::None.is_vertical());
        assert!(RelativeCenter::Horizontal.is_horizontal());
        assert!(!RelativeCenter::Horizontal.is_vertical());
        assert!(!RelativeCenter::Vertical.is_horizontal());
        assert!(RelativeCenter::Vertical.is_vertical());
        assert!(RelativeCenter::Both.is_horizontal());
        assert!(RelativeCenter::Both.is_vertical());
    }
}
