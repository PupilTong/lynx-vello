//! Post-flush computed-style views lending Stylo [`ComputedValues`] directly
//! to hughie without cloning its `Arc` or re-entering Stylo's runtime
//! borrow checker.

use hughie::style::containment::effective_containment;
use hughie::style::{
    Contain, ContentVisibility, CoreStyle, Display, PositionProperty, TextContainerStyle,
    TextRunStyle,
};
use stylo::properties::ComputedValues;
use stylo::values::computed::motion::OffsetPath;
use stylo::values::specified::box_::WillChangeBits;

use crate::tree::node::Node;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayMode {
    None,
    Contents,
    Flex,
    Grid,
    Linear,
    Relative,
    Leaf,
}

pub(crate) fn display_mode(display: Display) -> DisplayMode {
    match display {
        Display::None => DisplayMode::None,
        Display::Contents => DisplayMode::Contents,
        Display::Flex => DisplayMode::Flex,
        Display::Grid => DisplayMode::Grid,
        Display::Linear => DisplayMode::Linear,
        Display::LynxRelative => DisplayMode::Relative,
        unsupported => panic!(
            "Bobcat does not support Stylo computed display {unsupported:?} \
             (raw={:#06x}, outside={:?}, inside={:?})",
            unsupported.to_u16(),
            unsupported.outside(),
            unsupported.inside(),
        ),
    }
}

fn is_root_element<T>(node: &Node<T>) -> bool {
    node.flat_parent().is_none_or(Node::is_document)
}

pub(crate) fn generates_no_box(style: &ComputedValues) -> bool {
    style.clone_display().is_contents()
}

pub(crate) fn skips_contents(style: &ComputedValues) -> bool {
    !generates_no_box(style) && style.clone_content_visibility() == ContentVisibility::Hidden
}

pub(crate) fn establishes_fixed_containing_block<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> bool {
    if generates_no_box(style) {
        return false;
    }
    let box_style = style.get_box();
    !box_style.transform.0.is_empty()
        || !matches!(
            box_style.perspective,
            stylo::values::generics::box_::Perspective::None
        )
        || !matches!(box_style.offset_path, OffsetPath::None)
        || box_style.will_change.bits.intersects(
            WillChangeBits::TRANSFORM | WillChangeBits::PERSPECTIVE | WillChangeBits::CONTAIN,
        )
        || (box_style
            .will_change
            .bits
            .intersects(WillChangeBits::FIXPOS_CB_NON_SVG)
            && !is_root_element(node))
        || effective_containment(
            style.clone_contain(),
            style.clone_content_visibility(),
            skips_contents(style),
        )
        .intersects(Contain::LAYOUT | Contain::PAINT)
        || (!style.get_effects().filter.0.is_empty() && !is_root_element(node))
}

pub(crate) fn establishes_absolute_containing_block<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> bool {
    if generates_no_box(style) {
        return false;
    }
    style.clone_position() != PositionProperty::Static
        || style
            .get_box()
            .will_change
            .bits
            .intersects(WillChangeBits::POSITION)
        || establishes_fixed_containing_block(node, style)
}

pub(crate) fn box_parent<T>(node: &Node<T>) -> Option<&Node<T>> {
    let mut current = node.flat_parent()?;
    loop {
        let style = StyleView::try_of(current)?;
        if !generates_no_box(style.values()) {
            return Some(current);
        }
        current = current.flat_parent()?;
    }
}

pub(crate) fn resolve_position<T>(node: &Node<T>, style: &ComputedValues) -> PositionProperty {
    let parent_establishes = |fixed: bool| {
        box_parent(node).is_some_and(|parent| {
            StyleView::try_of(parent).is_some_and(|parent_style| {
                if fixed {
                    establishes_fixed_containing_block(parent, parent_style.values())
                } else {
                    establishes_absolute_containing_block(parent, parent_style.values())
                }
            })
        })
    };
    match style.clone_position() {
        computed @ (PositionProperty::Static
        | PositionProperty::Relative
        | PositionProperty::Sticky) => computed,
        PositionProperty::Absolute => {
            if parent_establishes(false) {
                PositionProperty::Absolute
            } else {
                PositionProperty::Fixed
            }
        }
        PositionProperty::Fixed => {
            if parent_establishes(true) {
                PositionProperty::Absolute
            } else {
                PositionProperty::Fixed
            }
        }
    }
}

/// The element style view hughie reads: a node handle for the
/// parent-dependent position lowering plus its post-flush computed values.
pub(crate) struct StyleView<'dom, T> {
    node: &'dom Node<T>,
    style: &'dom ComputedValues,
}

impl<T> std::fmt::Debug for StyleView<'_, T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("StyleView")
            .field(&self.node.id())
            .finish()
    }
}

impl<'dom, T> StyleView<'dom, T> {
    pub(crate) fn try_of(node: &'dom Node<T>) -> Option<Self> {
        Some(Self {
            node,
            style: node.layout_computed_style()?,
        })
    }

    pub(crate) fn of(node: &'dom Node<T>) -> Self {
        Self::try_of(node).unwrap_or(Self {
            node,
            style: &super::ANONYMOUS_STYLE,
        })
    }

    pub(crate) fn values(&self) -> &ComputedValues {
        self.style
    }
}

impl<T> CoreStyle for StyleView<'_, T> {
    fn computed_values(&self) -> &ComputedValues {
        self.values()
    }

    fn position(&self) -> PositionProperty {
        resolve_position(self.node, self.values())
    }
}

/// Text-only view: static anonymous-box geometry plus its parent's post-flush
/// inherited paragraph/run values.
pub(crate) struct TextStyleView<'dom> {
    text_style: &'dom ComputedValues,
}

impl std::fmt::Debug for TextStyleView<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TextStyleView")
    }
}

impl<'dom> TextStyleView<'dom> {
    pub(crate) fn of<T>(node: &'dom Node<T>) -> Self {
        debug_assert!(node.is_text_node(), "text style requires a text node");
        Self {
            text_style: node
                .flat_parent()
                .and_then(Node::layout_computed_style)
                .unwrap_or(&super::ANONYMOUS_STYLE),
        }
    }

    fn text_values(&self) -> &ComputedValues {
        self.text_style
    }
}

impl CoreStyle for TextStyleView<'_> {
    fn computed_values(&self) -> &ComputedValues {
        &super::ANONYMOUS_STYLE
    }

    fn inherited_values(&self) -> &ComputedValues {
        self.text_values()
    }
}

impl TextContainerStyle for TextStyleView<'_> {}

impl TextRunStyle for TextStyleView<'_> {
    fn computed_text_values(&self) -> Option<&ComputedValues> {
        Some(self.text_values())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use core::mem::size_of;

    use hughie::style::Display;
    use stylo::values::specified::box_::DisplayInside;

    use super::{DisplayMode, StyleView, TextStyleView, display_mode};

    #[test]
    fn supported_lynx_displays_map_to_layout_modes() {
        assert_eq!(display_mode(Display::None), DisplayMode::None);
        assert_eq!(display_mode(Display::Contents), DisplayMode::Contents);
        assert_eq!(display_mode(Display::Flex), DisplayMode::Flex);
        assert_eq!(display_mode(Display::Grid), DisplayMode::Grid);
        assert_eq!(display_mode(Display::Linear), DisplayMode::Linear);
        assert_eq!(display_mode(Display::LynxRelative), DisplayMode::Relative);
    }

    #[test]
    #[should_panic(expected = "Bobcat does not support Stylo computed display")]
    fn unsupported_stylo_display_panics_instead_of_becoming_a_leaf() {
        // Stylo's root `display: contents` fixup creates its private block-flow
        // encoding, giving this test a real computed value that Lynx cannot lay out.
        let unsupported = Display::Contents.equivalent_block_display(true);
        assert_eq!(unsupported.inside(), DisplayInside::Flow);

        let _ = display_mode(unsupported);
    }

    #[test]
    fn post_flush_style_views_stay_within_their_expected_footprint() {
        let word = size_of::<usize>();
        assert_eq!(size_of::<StyleView<'static, ()>>(), 2 * word);
        assert_eq!(size_of::<TextStyleView<'static>>(), word);
    }
}
