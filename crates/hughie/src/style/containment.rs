//! CSS Containment Level 2 (`css-contain-2`): the layout-relevant projection of the
//! `contain` property plus `contain-intrinsic-size`, in the stylo fork's own
//! computed-value vocabulary.

use crate::geometry::Size;
use crate::style::{Contain, ContainIntrinsicSize, ContentVisibility, CoreStyle};

#[must_use]
pub fn effective_containment(
    contain: Contain,
    content_visibility: ContentVisibility,
    skipped_contents: bool,
) -> Contain {
    let mut contain = contain;
    match content_visibility {
        ContentVisibility::Visible => {}
        ContentVisibility::Auto => {
            contain.insert(Contain::LAYOUT | Contain::PAINT | Contain::STYLE);
            if skipped_contents {
                contain.insert(Contain::SIZE);
            }
        }
        ContentVisibility::Hidden => {
            contain.insert(Contain::LAYOUT | Contain::PAINT | Contain::SIZE | Contain::STYLE);
        }
    }
    contain
}

#[must_use]
pub(crate) fn contain_intrinsic_length(value: &ContainIntrinsicSize) -> Option<f32> {
    match value {
        ContainIntrinsicSize::None | ContainIntrinsicSize::AutoNone => None,
        ContainIntrinsicSize::Length(length) | ContainIntrinsicSize::AutoLength(length) => {
            Some(length.0.px())
        }
    }
}

#[must_use]
pub(crate) fn size_containment<S: CoreStyle>(style: &S) -> Option<Size<Option<f32>>> {
    style.containment().contains(Contain::SIZE).then(|| {
        Size::new(
            contain_intrinsic_length(&style.contain_intrinsic_width()),
            contain_intrinsic_length(&style.contain_intrinsic_height()),
        )
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::values::computed::{Contain, Length};
    use stylo::values::generics::NonNegative;

    use super::*;

    fn intrinsic_len(value: f32) -> ContainIntrinsicSize {
        ContainIntrinsicSize::Length(NonNegative(Length::new(value)))
    }

    fn intrinsic_auto_len(value: f32) -> ContainIntrinsicSize {
        ContainIntrinsicSize::AutoLength(NonNegative(Length::new(value)))
    }

    #[test]
    fn effective_containment_folds_content_visibility() {
        assert_eq!(
            effective_containment(Contain::LAYOUT, ContentVisibility::Visible, false),
            Contain::LAYOUT
        );

        let auto = effective_containment(Contain::empty(), ContentVisibility::Auto, false);
        assert!(auto.contains(Contain::LAYOUT));
        assert!(auto.contains(Contain::PAINT));
        assert!(auto.contains(Contain::STYLE));
        assert!(!auto.contains(Contain::SIZE));
        let auto_skipped = effective_containment(Contain::empty(), ContentVisibility::Auto, true);
        assert!(auto_skipped.contains(Contain::SIZE));
        assert!(auto_skipped.contains(Contain::LAYOUT));

        for skipped in [false, true] {
            let hidden =
                effective_containment(Contain::empty(), ContentVisibility::Hidden, skipped);
            assert!(hidden.contains(Contain::LAYOUT));
            assert!(hidden.contains(Contain::PAINT));
            assert!(hidden.contains(Contain::STYLE));
            assert!(hidden.contains(Contain::SIZE));
        }
    }

    #[test]
    fn effective_containment_reads_effect_bits_not_markers() {
        let strict = effective_containment(Contain::STRICT, ContentVisibility::Visible, false);
        assert!(strict.contains(Contain::SIZE));
        assert!(strict.contains(Contain::LAYOUT));
        let content = effective_containment(Contain::CONTENT, ContentVisibility::Visible, false);
        assert!(content.contains(Contain::LAYOUT));
        assert!(!content.contains(Contain::SIZE));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn contain_intrinsic_length_treats_auto_as_length_and_auto_none_as_none() {
        assert_eq!(contain_intrinsic_length(&ContainIntrinsicSize::None), None);
        assert_eq!(
            contain_intrinsic_length(&ContainIntrinsicSize::AutoNone),
            None
        );
        assert_eq!(contain_intrinsic_length(&intrinsic_len(40.0)), Some(40.0));
        assert_eq!(
            contain_intrinsic_length(&intrinsic_auto_len(30.0)),
            Some(30.0)
        );
    }

    struct SizeContained;
    impl CoreStyle for SizeContained {
        fn display(&self) -> crate::style::Display {
            crate::style::Display::Flex
        }
        fn containment(&self) -> Contain {
            Contain::STRICT
        }
        fn contain_intrinsic_width(&self) -> ContainIntrinsicSize {
            intrinsic_len(50.0)
        }
        fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
            intrinsic_auto_len(30.0)
        }
    }

    struct LayoutOnly;
    impl CoreStyle for LayoutOnly {
        fn display(&self) -> crate::style::Display {
            crate::style::Display::Flex
        }
        fn containment(&self) -> Contain {
            Contain::LAYOUT
        }
    }

    struct InlineSizeOnly;
    impl CoreStyle for InlineSizeOnly {
        fn display(&self) -> crate::style::Display {
            crate::style::Display::Flex
        }
        fn containment(&self) -> Contain {
            Contain::INLINE_SIZE
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn size_containment_reports_both_axes_only_for_full_size_containment() {
        assert_eq!(
            size_containment(&SizeContained),
            Some(Size::new(Some(50.0), Some(30.0)))
        );
        assert_eq!(size_containment(&LayoutOnly), None);
        assert_eq!(size_containment(&InlineSizeOnly), None);
    }
}
