//! Guarded computed-style views lending Stylo [`ComputedValues`] directly to
//! neutron-star.

use std::ops::Deref;

use neutron_star::style::{
    Contain, CoreStyle, Display, PositionProperty, TextContainerStyle, TextRunStyle,
};
use stylo::data::ElementDataRef;
use stylo::properties::ComputedValues;
use stylo::values::computed::motion::OffsetPath;
use stylo::values::specified::box_::{DisplayInside, DisplayOutside, WillChangeBits};

use crate::contain::{ContentVisibility, effective_containment};
use crate::node::Node;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayMode {
    None,
    Flex,
    Grid,
    Linear,
    Relative,
    Leaf,
}

pub(crate) fn display_mode(display: Display) -> DisplayMode {
    if display.outside() == DisplayOutside::None {
        return if display.inside() == DisplayInside::Contents {
            DisplayMode::Leaf
        } else {
            DisplayMode::None
        };
    }
    match display.inside() {
        DisplayInside::None => DisplayMode::None,
        DisplayInside::Flex => DisplayMode::Flex,
        DisplayInside::Grid => DisplayMode::Grid,
        DisplayInside::LynxLinear => DisplayMode::Linear,
        DisplayInside::LynxRelative => DisplayMode::Relative,
        DisplayInside::Contents | DisplayInside::Flow => DisplayMode::Leaf,
    }
}

fn is_root_element<T>(node: &Node<T>) -> bool {
    node.parent().is_none_or(Node::is_document)
}

pub(crate) fn skips_contents(style: &ComputedValues) -> bool {
    style.clone_content_visibility() == ContentVisibility::Hidden
}

pub(crate) fn establishes_fixed_containing_block<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> bool {
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
        || effective_containment(style, skips_contents(style))
            .intersects(Contain::LAYOUT | Contain::PAINT)
        || (!style.get_effects().filter.0.is_empty() && !is_root_element(node))
}

pub(crate) fn establishes_absolute_containing_block<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> bool {
    style.clone_position() != PositionProperty::Static
        || style
            .get_box()
            .will_change
            .bits
            .intersects(WillChangeBits::POSITION)
        || establishes_fixed_containing_block(node, style)
}

pub(crate) fn resolve_position<T>(node: &Node<T>, style: &ComputedValues) -> PositionProperty {
    let parent_establishes = |fixed: bool| {
        node.parent().is_some_and(|parent| {
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

/// Keeps a node's Stylo element-data read guard alive while layout borrows its
/// primary computed values. No [`Arc`](stylo::servo_arc::Arc) is cloned.
enum NodeStyleGuard<'dom> {
    Computed(ElementDataRef<'dom>),
    Anonymous,
}

impl Deref for NodeStyleGuard<'_> {
    type Target = ComputedValues;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Computed(data) => data
                .styles
                .primary
                .as_ref()
                .expect("computed-style borrow was validated at construction"),
            Self::Anonymous => &super::ANONYMOUS_STYLE,
        }
    }
}

/// The element style view neutron-star reads: a node handle for the
/// parent-dependent position lowering plus its guarded computed values.
pub struct StyleView<'dom, T> {
    node: &'dom Node<T>,
    style: NodeStyleGuard<'dom>,
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
            style: NodeStyleGuard::Computed(node.borrow_computed_style()?),
        })
    }

    pub(crate) fn of(node: &'dom Node<T>) -> Self {
        Self::try_of(node).unwrap_or(Self {
            node,
            style: NodeStyleGuard::Anonymous,
        })
    }

    pub(crate) fn values(&self) -> &ComputedValues {
        &self.style
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

/// Text-only view: static anonymous-box geometry plus one guarded borrow of
/// its parent's inherited paragraph/run values.
pub(crate) struct TextStyleView<'dom> {
    text_style: NodeStyleGuard<'dom>,
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
                .parent()
                .and_then(Node::borrow_computed_style)
                .map_or(NodeStyleGuard::Anonymous, NodeStyleGuard::Computed),
        }
    }

    fn text_values(&self) -> &ComputedValues {
        &self.text_style
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

    use super::{StyleView, TextStyleView};

    #[test]
    fn guarded_style_views_stay_within_their_expected_footprint() {
        let word = size_of::<usize>();
        assert_eq!(size_of::<StyleView<'static, ()>>(), 4 * word);
        assert_eq!(size_of::<TextStyleView<'static>>(), 3 * word);
    }
}
