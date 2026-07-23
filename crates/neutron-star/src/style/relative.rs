//! Starlight relative-layout style protocol.

use stylo::computed_values::{relative_center, relative_layout_once};
use stylo::values::computed::lynx_layout::{RelativeAlign, RelativeReference};

use crate::geometry::Edges;
use crate::style::CoreStyle;

pub const RELATIVE_REFERENCE_NONE: RelativeReference = -1;

pub const RELATIVE_REFERENCE_PARENT: RelativeReference = 0;

pub trait RelativeContainerStyle: CoreStyle {
    fn relative_layout_once(&self) -> relative_layout_once::T {
        relative_layout_once::T::True
    }
}

pub trait RelativeItemStyle: CoreStyle {
    fn relative_id(&self) -> RelativeReference {
        RELATIVE_REFERENCE_NONE
    }

    fn relative_align(&self) -> Edges<RelativeAlign> {
        Edges::uniform(RELATIVE_REFERENCE_NONE)
    }

    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        Edges::uniform(RELATIVE_REFERENCE_NONE)
    }

    fn relative_center(&self) -> relative_center::T {
        relative_center::T::None
    }

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
