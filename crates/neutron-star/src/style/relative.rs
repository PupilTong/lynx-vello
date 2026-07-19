//! Starlight relative-layout style protocol.
//!
//! `display: relative` is an id-based container formatting context. It is a
//! Lynx/Starlight extension, not CSS `position: relative`: direct in-flow
//! children may constrain their physical margin edges to the parent or to
//! another child's edges. The host still owns `display` dispatch; this module
//! only describes the computed values consumed by
//! [`compute_relative_layout`](crate::compute::compute_relative_layout).
//!
//! References are stylo's computed `lynx-layout` integers
//! ([`RelativeReference`]/[`RelativeAlign`], both `i32`): `-1` means no
//! id/reference, `0` is the reserved parent reference, and every other
//! integer may identify an item. A host passes computed integer values
//! directly; layout deliberately does not parse or repair syntax.

use stylo::computed_values::{relative_center, relative_layout_once};
use stylo::values::computed::lynx_layout::{RelativeAlign, RelativeReference};

use crate::geometry::Edges;
use crate::style::CoreStyle;

/// The reserved "no id / no constraint" reference value (`-1`).
pub const RELATIVE_REFERENCE_NONE: RelativeReference = -1;

/// The reserved "the relative container" reference value (`0`).
pub const RELATIVE_REFERENCE_PARENT: RelativeReference = 0;

/// Style of a node as a relative-layout container.
pub trait RelativeContainerStyle: CoreStyle {
    /// `relative-layout-once`: use one combined dependency order and measure
    /// each item once while walking it. The fork initial (Lynx's computed
    /// default) is `true`.
    fn relative_layout_once(&self) -> relative_layout_once::T {
        relative_layout_once::T::True
    }
}

/// Style of a direct in-flow relative item.
pub trait RelativeItemStyle: CoreStyle {
    /// `relative-id`; `-1` does not identify the item and `0` is reserved for
    /// the parent reference.
    fn relative_id(&self) -> RelativeReference {
        RELATIVE_REFERENCE_NONE
    }

    /// Physical same-side alignment references.
    ///
    /// The fields correspond to `relative-align-left`, `-right`, `-top`, and
    /// `-bottom` respectively.
    fn relative_align(&self) -> Edges<RelativeAlign> {
        Edges::uniform(RELATIVE_REFERENCE_NONE)
    }

    /// Physical adjacency references.
    ///
    /// The fields correspond to `relative-left-of`, `-right-of`, `-top-of`,
    /// and `-bottom-of`. Consequently, `right` constrains the item's start
    /// edge and `left` constrains its end edge (likewise `bottom`/`top`).
    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        Edges::uniform(RELATIVE_REFERENCE_NONE)
    }

    /// `relative-center`: which unconstrained axes to center.
    fn relative_center(&self) -> relative_center::T {
        relative_center::T::None
    }

    /// Stable layout ordering among relative items.
    fn order(&self) -> i32 {
        0
    }
}

impl<S: RelativeContainerStyle> RelativeContainerStyle for &S {
    fn relative_layout_once(&self) -> relative_layout_once::T {
        (**self).relative_layout_once()
    }
}

impl<S: RelativeItemStyle> RelativeItemStyle for &S {
    fn relative_id(&self) -> RelativeReference {
        (**self).relative_id()
    }

    fn relative_align(&self) -> Edges<RelativeAlign> {
        (**self).relative_align()
    }

    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        (**self).relative_adjacent()
    }

    fn relative_center(&self) -> relative_center::T {
        (**self).relative_center()
    }

    fn order(&self) -> i32 {
        (**self).order()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::values::computed::Display;

    use super::*;

    #[derive(Debug)]
    struct Defaults;

    impl CoreStyle for Defaults {
        fn display(&self) -> Display {
            Display::LynxRelative
        }
    }
    impl RelativeContainerStyle for Defaults {}
    impl RelativeItemStyle for Defaults {}

    #[test]
    fn defaults_match_the_fork_initial_values() {
        let style = Defaults;

        assert_eq!(style.relative_layout_once(), relative_layout_once::T::True);
        assert_eq!(style.relative_id(), RELATIVE_REFERENCE_NONE);
        assert_eq!(
            style.relative_align(),
            Edges::uniform(RELATIVE_REFERENCE_NONE)
        );
        assert_eq!(
            style.relative_adjacent(),
            Edges::uniform(RELATIVE_REFERENCE_NONE)
        );
        assert_eq!(style.relative_center(), relative_center::T::None);
        assert_eq!(style.order(), 0);
        assert_ne!(RELATIVE_REFERENCE_NONE, RELATIVE_REFERENCE_PARENT);
    }
}
