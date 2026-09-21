//! CSS Containment Level 2 (`css-contain-2`): the layout-relevant projection of the
//! `contain` property plus `contain-intrinsic-size`, in the stylo fork's own
//! computed-value vocabulary. `container-type` (`css-contain-3`) folds in here
//! too, because a size query container *is* a contained box.

use crate::geometry::Size;
use crate::style::{Contain, ContainIntrinsicSize, ContainerType, ContentVisibility, CoreStyle};

/// The containment a box actually has: its `contain` value plus everything
/// `content-visibility` and `container-type` imply.
///
/// `skipped_contents` is the `content-visibility: auto` relevance answer,
/// which only a rendering update can establish — hughie never decides it.
#[must_use]
pub fn effective_containment(
    contain: Contain,
    content_visibility: ContentVisibility,
    skipped_contents: bool,
    container_type: ContainerType,
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
    // css-contain-3 §2.1: `container-type: inline-size` "applies layout
    // containment, style containment, and inline-size containment to the
    // principal box", and `size` the same with size containment. The two
    // keywords are mutually exclusive in the grammar, so the branch order only
    // settles a value the parser cannot produce.
    //
    // Gecko's `StyleAdjuster::adjust_for_contain` inserts `STYLE` and the size
    // bit but not `LAYOUT`; it reaches layout containment for a query container
    // by another road. This engine has only the one bit, so it inserts what the
    // spec text says.
    if container_type.intersects(ContainerType::INLINE_SIZE) {
        contain.insert(Contain::LAYOUT | Contain::STYLE | Contain::INLINE_SIZE);
    } else if container_type.intersects(ContainerType::SIZE) {
        contain.insert(Contain::LAYOUT | Contain::STYLE | Contain::SIZE);
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

/// The per-axis projection of size containment: the axes whose extent may not
/// come from the box's contents, each carrying the content-box extent that
/// stands in for those contents (`contain-intrinsic-*`, or zero where that is
/// `none`).
///
/// An axis reading `None` measures its contents the ordinary way. `contain:
/// size` contains both axes, `contain: inline-size` — and `container-type:
/// inline-size`, which implies it — only the width: this engine is
/// horizontal-writing-mode only, so inline is width and block is height.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct ContainedAxes(Size<Option<f32>>);

impl ContainedAxes {
    /// The two axes as a `Size`, for the algorithms that project through an
    /// `Axis` rather than naming width and height.
    #[inline]
    pub(crate) const fn extents(self) -> Size<Option<f32>> {
        self.0
    }

    #[inline]
    pub(crate) const fn width(self) -> Option<f32> {
        self.0.width
    }

    #[inline]
    pub(crate) const fn height(self) -> Option<f32> {
        self.0.height
    }

    /// Whether either axis is contained.
    #[inline]
    pub(crate) const fn any(self) -> bool {
        self.0.width.is_some() || self.0.height.is_some()
    }

    /// The whole box's substituted size, `Some` only when *both* axes are
    /// contained — the one case in which the contents never have to run.
    #[inline]
    pub(crate) const fn all(self) -> Option<Size<f32>> {
        match self.0 {
            Size {
                width: Some(width),
                height: Some(height),
            } => Some(Size::new(width, height)),
            _ => None,
        }
    }
}

#[must_use]
pub(crate) fn contained_axes<S: CoreStyle>(style: &S) -> ContainedAxes {
    let containment = style.containment();
    ContainedAxes(Size::new(
        containment
            .contains(Contain::INLINE_SIZE)
            .then(|| contain_intrinsic_length(&style.contain_intrinsic_width()).unwrap_or(0.0)),
        containment
            .contains(Contain::BLOCK_SIZE)
            .then(|| contain_intrinsic_length(&style.contain_intrinsic_height()).unwrap_or(0.0)),
    ))
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

    fn folded(contain: Contain, content_visibility: ContentVisibility, skipped: bool) -> Contain {
        effective_containment(contain, content_visibility, skipped, ContainerType::NORMAL)
    }

    #[test]
    fn effective_containment_folds_content_visibility() {
        assert_eq!(
            folded(Contain::LAYOUT, ContentVisibility::Visible, false),
            Contain::LAYOUT
        );

        let auto = folded(Contain::empty(), ContentVisibility::Auto, false);
        assert!(auto.contains(Contain::LAYOUT));
        assert!(auto.contains(Contain::PAINT));
        assert!(auto.contains(Contain::STYLE));
        assert!(!auto.contains(Contain::SIZE));
        let auto_skipped = folded(Contain::empty(), ContentVisibility::Auto, true);
        assert!(auto_skipped.contains(Contain::SIZE));
        assert!(auto_skipped.contains(Contain::LAYOUT));

        for skipped in [false, true] {
            let hidden = folded(Contain::empty(), ContentVisibility::Hidden, skipped);
            assert!(hidden.contains(Contain::LAYOUT));
            assert!(hidden.contains(Contain::PAINT));
            assert!(hidden.contains(Contain::STYLE));
            assert!(hidden.contains(Contain::SIZE));
        }
    }

    #[test]
    fn effective_containment_reads_effect_bits_not_markers() {
        let strict = folded(Contain::STRICT, ContentVisibility::Visible, false);
        assert!(strict.contains(Contain::SIZE));
        assert!(strict.contains(Contain::LAYOUT));
        let content = folded(Contain::CONTENT, ContentVisibility::Visible, false);
        assert!(content.contains(Contain::LAYOUT));
        assert!(!content.contains(Contain::SIZE));
    }

    #[test]
    fn effective_containment_folds_the_container_type() {
        let normal = effective_containment(
            Contain::empty(),
            ContentVisibility::Visible,
            false,
            ContainerType::NORMAL,
        );
        assert_eq!(normal, Contain::empty());

        let inline_size = effective_containment(
            Contain::empty(),
            ContentVisibility::Visible,
            false,
            ContainerType::INLINE_SIZE,
        );
        assert!(inline_size.contains(Contain::LAYOUT));
        assert!(inline_size.contains(Contain::STYLE));
        assert!(inline_size.contains(Contain::INLINE_SIZE));
        assert!(!inline_size.contains(Contain::BLOCK_SIZE));
        assert!(!inline_size.contains(Contain::SIZE));
        assert!(!inline_size.contains(Contain::PAINT));

        let size = effective_containment(
            Contain::empty(),
            ContentVisibility::Visible,
            false,
            ContainerType::SIZE,
        );
        assert!(size.contains(Contain::LAYOUT));
        assert!(size.contains(Contain::STYLE));
        assert!(size.contains(Contain::SIZE));
        assert!(!size.contains(Contain::PAINT));

        // `scroll-state` alone is not a size query container, so it contains
        // nothing.
        let scroll_state = effective_containment(
            Contain::empty(),
            ContentVisibility::Visible,
            false,
            ContainerType::SCROLL_STATE,
        );
        assert_eq!(scroll_state, Contain::empty());

        // The fold only ever adds: an authored `contain` keeps its own bits.
        let with_paint = effective_containment(
            Contain::PAINT,
            ContentVisibility::Visible,
            false,
            ContainerType::INLINE_SIZE,
        );
        assert!(with_paint.contains(Contain::PAINT | Contain::INLINE_SIZE));
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
        fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
            intrinsic_len(30.0)
        }
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

    #[test]
    #[allow(clippy::float_cmp)]
    fn contained_axes_projects_size_containment_per_axis() {
        let both = contained_axes(&SizeContained);
        assert_eq!(both.extents(), Size::new(Some(50.0), Some(30.0)));
        assert_eq!(both.all(), Some(Size::new(50.0, 30.0)));
        assert!(both.any());

        let none = contained_axes(&LayoutOnly);
        assert_eq!(none.extents(), Size::NONE);
        assert_eq!(none.all(), None);
        assert!(!none.any());

        // Only the width is contained, and the *height*'s
        // `contain-intrinsic-*` is not consulted for an axis that measures its
        // contents.
        let inline = contained_axes(&InlineSizeOnly);
        assert_eq!(inline.width(), Some(0.0));
        assert_eq!(inline.height(), None);
        assert_eq!(inline.all(), None);
        assert!(inline.any());
    }
}
