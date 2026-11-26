//! Post-flush computed-style views lending Stylo [`ComputedValues`] directly
//! to seldon without cloning its `Arc` or re-entering Stylo's runtime
//! borrow checker.

use seldon::geometry::Size;
use seldon::style::containment::effective_containment;
use seldon::style::{
    Contain, ContainIntrinsicSize, ContentVisibility, CoreStyle, Display, FlexboxStyle,
    GridLanesStyle, GridStyle, LinearStyle, LynxTextStyle, PositionProperty, RelativeStyle,
    TextContainerStyle, TextRunStyle,
};
use stylo::properties::ComputedValues;
use stylo::values::computed::motion::OffsetPath;
use stylo::values::specified::box_::WillChangeBits;

use super::committed_box;
use crate::tree::node::Node;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayMode {
    None,
    Contents,
    Flex,
    Grid,
    GridLanes,
    Linear,
    Relative,
    Text,
    BlockFlow,
    Leaf,
}

pub(crate) fn display_mode<T>(node: &Node<T>, display: Display) -> DisplayMode {
    mode_of(display, node.is_lynx_text())
}

fn mode_of(display: Display, lynx_text: bool) -> DisplayMode {
    match display {
        Display::None => DisplayMode::None,
        Display::Contents => DisplayMode::Contents,
        _ if lynx_text => DisplayMode::Text,
        Display::Flex => DisplayMode::Flex,
        Display::Grid => DisplayMode::Grid,
        Display::GridLanes => DisplayMode::GridLanes,
        Display::Linear => DisplayMode::Linear,
        Display::LynxRelative => DisplayMode::Relative,
        Display::Block | Display::Inline => DisplayMode::BlockFlow,
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
    style.get_display().is_contents()
}

pub(crate) fn skips_contents<T>(node: &Node<T>, style: &ComputedValues) -> bool {
    if generates_no_box(style) {
        return false;
    }
    match *style.get_content_visibility() {
        ContentVisibility::Visible => false,
        ContentVisibility::Hidden => true,
        ContentVisibility::Auto => node.arenas().relevance(node.id()).skips(),
    }
}

pub(crate) fn establishes_fixed_containing_block<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> bool {
    if generates_no_box(style) {
        return false;
    }
    let box_style = style.get_box();
    let effects = style.get_effects();
    let filters = !effects.filter.0.is_empty() || !effects.backdrop_filter.0.is_empty();
    !box_style.transform.0.is_empty()
        || node.animates_transform()
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
            *style.get_contain(),
            *style.get_content_visibility(),
            skips_contents(node, style),
            *style.get_container_type(),
        )
        .intersects(Contain::LAYOUT | Contain::PAINT)
        || (filters && !is_root_element(node))
}

pub(crate) fn establishes_absolute_containing_block<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> bool {
    if generates_no_box(style) {
        return false;
    }
    *style.get_box().get_position() != PositionProperty::Static
        || node.arenas().top_layer().places_against_viewport(node.id())
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
    if node.arenas().top_layer().places_against_viewport(node.id()) {
        return PositionProperty::Fixed;
    }
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
    match *style.get_box().get_position() {
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

    pub(crate) const fn with_values(node: &'dom Node<T>, style: &'dom ComputedValues) -> Self {
        Self { node, style }
    }

    pub(crate) fn values(&self) -> &'dom ComputedValues {
        self.style
    }

    pub(crate) fn display_mode(&self) -> DisplayMode {
        display_mode(self.node, *self.style.get_display())
    }
}

impl<T> CoreStyle for StyleView<'_, T> {
    fn computed_values(&self) -> &ComputedValues {
        self.values()
    }

    fn position(&self) -> PositionProperty {
        resolve_position(self.node, self.values())
    }

    fn skips_contents(&self) -> bool {
        skips_contents(self.node, self.values())
    }

    fn natural_size(&self) -> Size<Option<f32>> {
        self.node.natural_size().dimensions()
    }

    fn contain_intrinsic_width(&self) -> ContainIntrinsicSize {
        committed_box::contain_intrinsic_width(self.node, self.values())
    }

    fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
        committed_box::contain_intrinsic_height(self.node, self.values())
    }

    #[inline]
    fn reads_percentage_basis(&self) -> bool {
        if self.node.views_own_layout_style(self.style) {
            self.node.layout_reads_percentage_basis()
        } else {
            reads_percentage_basis(self.node, self.style)
        }
    }
}

pub(crate) fn reads_percentage_basis<T>(node: &Node<T>, style: &ComputedValues) -> bool {
    super::paragraph::establishes_paragraph(display_mode(node, *style.get_display()))
        || seldon::style::box_model_reads_percentage_basis(&StyleView::with_values(node, style))
}

impl<T> FlexboxStyle for StyleView<'_, T> {}

impl<T> GridStyle for StyleView<'_, T> {}

impl<T> GridLanesStyle for StyleView<'_, T> {}

impl<T> LinearStyle for StyleView<'_, T> {}

impl<T> TextContainerStyle for StyleView<'_, T> {}

impl<T> LynxTextStyle for StyleView<'_, T> {}

impl<T> RelativeStyle for StyleView<'_, T> {}

struct ParagraphStyle<'a>(&'a ComputedValues);

impl CoreStyle for ParagraphStyle<'_> {
    fn computed_values(&self) -> &ComputedValues {
        self.0
    }
}

impl TextContainerStyle for ParagraphStyle<'_> {}

impl LynxTextStyle for ParagraphStyle<'_> {}

pub(crate) fn paragraph_limits_changed(old: &ComputedValues, new: &ComputedValues) -> bool {
    if old.custom_properties().non_inherited == new.custom_properties().non_inherited {
        return false;
    }

    let (old, new) = (ParagraphStyle(old), ParagraphStyle(new));
    old.text_maxline() != new.text_maxline() || old.text_maxlength() != new.text_maxlength()
}

pub(crate) fn converts_tail_color(style: &ComputedValues) -> bool {
    ParagraphStyle(style).tail_color_convert()
}

pub(crate) struct TextRunView<'dom> {
    run: &'dom ComputedValues,
}

impl std::fmt::Debug for TextRunView<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TextRunView")
    }
}

impl<'dom> TextRunView<'dom> {
    pub(crate) const fn from_values(run: &'dom ComputedValues) -> Self {
        Self { run }
    }

    pub(crate) fn of<T>(node: &'dom Node<T>) -> Self {
        Self {
            run: inline_style_of(node),
        }
    }
}

impl TextRunStyle for TextRunView<'_> {
    fn computed_text_values(&self) -> Option<&ComputedValues> {
        Some(self.run)
    }
}

pub(crate) fn inline_style_of<T>(node: &Node<T>) -> &ComputedValues {
    debug_assert!(node.is_text_node(), "text style requires a text node");
    node.flat_parent()
        .and_then(Node::layout_computed_style)
        .unwrap_or(&super::ANONYMOUS_STYLE)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use core::mem::size_of;

    use seldon::style::Display;

    use super::{DisplayMode, StyleView, TextRunView, mode_of};

    #[test]
    fn supported_lynx_displays_map_to_layout_modes() {
        assert_eq!(mode_of(Display::None, false), DisplayMode::None);
        assert_eq!(mode_of(Display::Contents, false), DisplayMode::Contents);
        assert_eq!(mode_of(Display::Flex, false), DisplayMode::Flex);
        assert_eq!(mode_of(Display::Grid, false), DisplayMode::Grid);
        assert_eq!(mode_of(Display::GridLanes, false), DisplayMode::GridLanes);
        assert_eq!(mode_of(Display::Linear, false), DisplayMode::Linear);
        assert_eq!(mode_of(Display::LynxRelative, false), DisplayMode::Relative);
        assert_eq!(mode_of(Display::Block, false), DisplayMode::BlockFlow);
        assert_eq!(mode_of(Display::Inline, false), DisplayMode::BlockFlow);
    }

    #[test]
    fn a_lynx_text_element_is_a_paragraph_under_every_display_but_none_and_contents() {
        assert_eq!(mode_of(Display::None, true), DisplayMode::None);
        assert_eq!(mode_of(Display::Contents, true), DisplayMode::Contents);
        for display in [
            Display::Flex,
            Display::Grid,
            Display::GridLanes,
            Display::Linear,
            Display::LynxRelative,
            Display::Block,
            Display::Inline,
        ] {
            assert_eq!(mode_of(display, true), DisplayMode::Text, "{display:?}");
        }
    }

    #[test]
    fn post_flush_style_views_stay_within_their_expected_footprint() {
        let word = size_of::<usize>();
        assert_eq!(size_of::<StyleView<'static, ()>>(), 2 * word);
        assert_eq!(size_of::<TextRunView<'static>>(), word);
    }
}
