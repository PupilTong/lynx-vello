//! The computed-style **view**: lending stylo [`ComputedValues`] fields to
//! neutron-star.

use std::ops::Deref;

use neutron_star::geometry::{Edges, Point, Size};
use neutron_star::style::{
    AspectRatio, Au, BorderSideWidth, Contain, ContainIntrinsicSize, ContentDistribution,
    CoreStyle, Display, FlexBasis, FlexContainerStyle, FlexItemStyle, FontFamily,
    FontFeatureSettings, FontStyle, FontVariationSettings, FontWeight, GridAutoFlow,
    GridContainerStyle, GridItemStyle, GridLine, GridTemplateComponent, ImplicitGridTracks, Inset,
    ItemPlacement, JustifyItems, LetterSpacing, LineHeight, LinearContainerStyle, LinearItemStyle,
    Margin, MaxSize, NonNegativeLengthPercentage, NonNegativeLengthPercentageOrNormal,
    NonNegativeNumber, Overflow, PositionProperty, RelativeAlign, RelativeContainerStyle,
    RelativeItemStyle, RelativeReference, SelfAlignment, StyleSize, TextAlign, TextContainerStyle,
    TextIndent, TextRunStyle, WordBreak, box_sizing, direction, flex_direction, flex_wrap,
    linear_direction, relative_center, relative_layout_once, text_wrap_mode, visibility,
    white_space_collapse,
};
use stylo::data::ElementDataRef;
use stylo::properties::ComputedValues;
use stylo::properties::style_structs::Position as PositionStruct;
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

fn lower_relative_logical(physical: i32, logical: i32) -> i32 {
    if physical == -1 { logical } else { physical }
}

enum StyleBorrow<'dom> {
    Computed(ElementDataRef<'dom>),
    Anonymous,
}

impl Deref for StyleBorrow<'_> {
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

/// The computed-style view neutron-star reads: the node handle (for the
/// parent-dependent `resolve_position`) plus its `ComputedValues`.
pub struct StyleView<'dom, T> {
    node: &'dom Node<T>,
    style: StyleBorrow<'dom>,
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
            style: StyleBorrow::Computed(node.borrow_computed_style()?),
        })
    }

    pub(crate) fn of(node: &'dom Node<T>) -> Self {
        Self::try_of(node).unwrap_or(Self {
            node,
            style: StyleBorrow::Anonymous,
        })
    }

    pub(crate) fn values(&self) -> &ComputedValues {
        &self.style
    }

    fn position_struct(&self) -> &PositionStruct {
        self.style.get_position()
    }
}

/// Text-only style view: anonymous-box geometry plus inherited paragraph and
/// run values. Keeping this separate means only literal text pays for the
/// second guarded computed-style borrow.
pub(crate) struct TextStyleView<'dom, T> {
    box_style: StyleView<'dom, T>,
    text_style: StyleBorrow<'dom>,
}

impl<T> std::fmt::Debug for TextStyleView<'_, T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("TextStyleView")
            .field(&self.box_style.node.id())
            .finish()
    }
}

impl<'dom, T> TextStyleView<'dom, T> {
    pub(crate) fn of(node: &'dom Node<T>) -> Self {
        debug_assert!(node.is_text_node(), "text style requires a text node");
        Self {
            box_style: StyleView::of(node),
            text_style: node
                .parent()
                .and_then(Node::borrow_computed_style)
                .map_or(StyleBorrow::Anonymous, StyleBorrow::Computed),
        }
    }

    fn text_values(&self) -> &ComputedValues {
        &self.text_style
    }
}

impl<T> CoreStyle for TextStyleView<'_, T> {
    fn display(&self) -> Display {
        self.box_style.display()
    }

    fn visibility(&self) -> visibility::T {
        self.box_style.visibility()
    }

    fn position(&self) -> PositionProperty {
        self.box_style.position()
    }

    fn inset(&self) -> Edges<&Inset> {
        self.box_style.inset()
    }

    fn size(&self) -> Size<&StyleSize> {
        self.box_style.size()
    }

    fn min_size(&self) -> Size<&StyleSize> {
        self.box_style.min_size()
    }

    fn max_size(&self) -> Size<&MaxSize> {
        self.box_style.max_size()
    }

    fn aspect_ratio(&self) -> AspectRatio {
        self.box_style.aspect_ratio()
    }

    fn margin(&self) -> Edges<&Margin> {
        self.box_style.margin()
    }

    fn padding(&self) -> Edges<&NonNegativeLengthPercentage> {
        self.box_style.padding()
    }

    fn border(&self) -> Edges<BorderSideWidth> {
        self.box_style.border()
    }

    fn overflow(&self) -> Point<Overflow> {
        self.box_style.overflow()
    }

    fn box_sizing(&self) -> box_sizing::T {
        self.box_style.box_sizing()
    }

    fn direction(&self) -> direction::T {
        self.text_values().clone_direction()
    }
}

impl<T> CoreStyle for StyleView<'_, T> {
    fn display(&self) -> Display {
        self.style.clone_display()
    }

    fn visibility(&self) -> visibility::T {
        self.style.clone_visibility()
    }

    fn position(&self) -> PositionProperty {
        resolve_position(self.node, &self.style)
    }

    fn inset(&self) -> Edges<&Inset> {
        let position = self.position_struct();
        Edges {
            left: &position.left,
            right: &position.right,
            top: &position.top,
            bottom: &position.bottom,
        }
    }

    fn size(&self) -> Size<&StyleSize> {
        let position = self.position_struct();
        Size::new(&position.width, &position.height)
    }

    fn min_size(&self) -> Size<&StyleSize> {
        let position = self.position_struct();
        Size::new(&position.min_width, &position.min_height)
    }

    fn max_size(&self) -> Size<&MaxSize> {
        let position = self.position_struct();
        Size::new(&position.max_width, &position.max_height)
    }

    fn aspect_ratio(&self) -> AspectRatio {
        self.style.clone_aspect_ratio()
    }

    fn margin(&self) -> Edges<&Margin> {
        let margin = self.style.get_margin();
        Edges {
            left: &margin.margin_left,
            right: &margin.margin_right,
            top: &margin.margin_top,
            bottom: &margin.margin_bottom,
        }
    }

    fn padding(&self) -> Edges<&NonNegativeLengthPercentage> {
        let padding = self.style.get_padding();
        Edges {
            left: &padding.padding_left,
            right: &padding.padding_right,
            top: &padding.padding_top,
            bottom: &padding.padding_bottom,
        }
    }

    fn border(&self) -> Edges<BorderSideWidth> {
        let border = self.style.get_border();
        let used = |width: Au, style: stylo::values::specified::BorderStyle| {
            BorderSideWidth(if style.none_or_hidden() { Au(0) } else { width })
        };
        Edges {
            left: used(border.border_left_width.0, border.border_left_style),
            right: used(border.border_right_width.0, border.border_right_style),
            top: used(border.border_top_width.0, border.border_top_style),
            bottom: used(border.border_bottom_width.0, border.border_bottom_style),
        }
    }

    fn overflow(&self) -> Point<Overflow> {
        Point::new(self.style.clone_overflow_x(), self.style.clone_overflow_y())
    }

    fn box_sizing(&self) -> box_sizing::T {
        self.style.clone_box_sizing()
    }

    fn direction(&self) -> direction::T {
        self.style.clone_direction()
    }

    fn containment(&self) -> Contain {
        effective_containment(&self.style, false)
    }

    fn contain_intrinsic_width(&self) -> ContainIntrinsicSize {
        self.style.clone_contain_intrinsic_width()
    }

    fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
        self.style.clone_contain_intrinsic_height()
    }

    fn skips_contents(&self) -> bool {
        skips_contents(&self.style)
    }
}

impl<T> FlexContainerStyle for StyleView<'_, T> {
    fn flex_direction(&self) -> flex_direction::T {
        self.style.clone_flex_direction()
    }

    fn flex_wrap(&self) -> flex_wrap::T {
        self.style.clone_flex_wrap()
    }

    fn gap(&self) -> Size<&NonNegativeLengthPercentageOrNormal> {
        let position = self.position_struct();
        Size::new(&position.column_gap, &position.row_gap)
    }

    fn align_content(&self) -> ContentDistribution {
        self.position_struct().align_content
    }

    fn align_items(&self) -> ItemPlacement {
        self.position_struct().align_items
    }

    fn justify_content(&self) -> ContentDistribution {
        self.position_struct().justify_content
    }
}

impl<T> TextContainerStyle for TextStyleView<'_, T> {
    fn text_align(&self) -> TextAlign {
        self.text_values().get_inherited_text().clone_text_align()
    }

    fn text_wrap_mode(&self) -> text_wrap_mode::T {
        self.text_values()
            .get_inherited_text()
            .clone_text_wrap_mode()
    }

    fn white_space_collapse(&self) -> white_space_collapse::T {
        self.text_values()
            .get_inherited_text()
            .clone_white_space_collapse()
    }

    fn word_break(&self) -> WordBreak {
        self.text_values().get_inherited_text().clone_word_break()
    }

    fn text_indent(&self) -> TextIndent {
        self.text_values().get_inherited_text().clone_text_indent()
    }
}

impl<T> TextRunStyle for TextStyleView<'_, T> {
    fn font_family(&self) -> FontFamily {
        self.text_values().get_font().clone_font_family()
    }

    fn font_size(&self) -> f32 {
        self.text_values()
            .get_font()
            .clone_font_size()
            .computed_size()
            .px()
    }

    fn font_weight(&self) -> FontWeight {
        self.text_values().get_font().clone_font_weight()
    }

    fn font_style(&self) -> FontStyle {
        self.text_values().get_font().clone_font_style()
    }

    fn letter_spacing(&self) -> LetterSpacing {
        self.text_values()
            .get_inherited_text()
            .clone_letter_spacing()
    }

    fn line_height(&self) -> LineHeight {
        self.text_values().get_font().clone_line_height()
    }

    fn font_feature_settings(&self) -> FontFeatureSettings {
        self.text_values().get_font().clone_font_feature_settings()
    }

    fn font_variation_settings(&self) -> FontVariationSettings {
        self.text_values()
            .get_font()
            .clone_font_variation_settings()
    }
}

impl<T> FlexItemStyle for StyleView<'_, T> {
    fn flex_basis(&self) -> &FlexBasis {
        &self.position_struct().flex_basis
    }

    fn flex_grow(&self) -> NonNegativeNumber {
        self.position_struct().flex_grow
    }

    fn flex_shrink(&self) -> NonNegativeNumber {
        self.position_struct().flex_shrink
    }

    fn align_self(&self) -> SelfAlignment {
        self.position_struct().align_self
    }

    fn order(&self) -> i32 {
        self.position_struct().order
    }
}

impl<T> GridContainerStyle for StyleView<'_, T> {
    fn grid_template_rows(&self) -> &GridTemplateComponent {
        &self.position_struct().grid_template_rows
    }

    fn grid_template_columns(&self) -> &GridTemplateComponent {
        &self.position_struct().grid_template_columns
    }

    fn grid_auto_rows(&self) -> &ImplicitGridTracks {
        &self.position_struct().grid_auto_rows
    }

    fn grid_auto_columns(&self) -> &ImplicitGridTracks {
        &self.position_struct().grid_auto_columns
    }

    fn grid_auto_flow(&self) -> GridAutoFlow {
        self.position_struct().grid_auto_flow
    }

    fn gap(&self) -> Size<&NonNegativeLengthPercentageOrNormal> {
        FlexContainerStyle::gap(self)
    }

    fn align_content(&self) -> ContentDistribution {
        FlexContainerStyle::align_content(self)
    }

    fn justify_content(&self) -> ContentDistribution {
        FlexContainerStyle::justify_content(self)
    }

    fn align_items(&self) -> ItemPlacement {
        FlexContainerStyle::align_items(self)
    }

    fn justify_items(&self) -> JustifyItems {
        self.position_struct().justify_items
    }
}

impl<T> GridItemStyle for StyleView<'_, T> {
    fn grid_row_start(&self) -> &GridLine {
        &self.position_struct().grid_row_start
    }

    fn grid_row_end(&self) -> &GridLine {
        &self.position_struct().grid_row_end
    }

    fn grid_column_start(&self) -> &GridLine {
        &self.position_struct().grid_column_start
    }

    fn grid_column_end(&self) -> &GridLine {
        &self.position_struct().grid_column_end
    }

    fn align_self(&self) -> SelfAlignment {
        FlexItemStyle::align_self(self)
    }

    fn justify_self(&self) -> SelfAlignment {
        self.position_struct().justify_self
    }

    fn order(&self) -> i32 {
        FlexItemStyle::order(self)
    }
}

impl<T> LinearContainerStyle for StyleView<'_, T> {
    fn linear_direction(&self) -> linear_direction::T {
        self.style.clone_linear_direction()
    }

    fn linear_weight_sum(&self) -> NonNegativeNumber {
        self.style.clone_linear_weight_sum()
    }

    fn justify_content(&self) -> ContentDistribution {
        FlexContainerStyle::justify_content(self)
    }

    fn align_items(&self) -> ItemPlacement {
        FlexContainerStyle::align_items(self)
    }
}

impl<T> LinearItemStyle for StyleView<'_, T> {
    fn linear_weight(&self) -> NonNegativeNumber {
        self.style.clone_linear_weight()
    }

    fn align_self(&self) -> SelfAlignment {
        FlexItemStyle::align_self(self)
    }

    fn order(&self) -> i32 {
        FlexItemStyle::order(self)
    }
}

impl<T> RelativeContainerStyle for StyleView<'_, T> {
    fn relative_layout_once(&self) -> relative_layout_once::T {
        self.style.clone_relative_layout_once()
    }
}

impl<T> RelativeItemStyle for StyleView<'_, T> {
    fn relative_id(&self) -> RelativeReference {
        self.style.clone_relative_id()
    }

    fn relative_align(&self) -> Edges<RelativeAlign> {
        let ltr = self.style.clone_direction() == direction::T::Ltr;
        let (inline_start, inline_end) = (
            self.style.clone_relative_align_inline_start(),
            self.style.clone_relative_align_inline_end(),
        );
        let (logical_left, logical_right) = if ltr {
            (inline_start, inline_end)
        } else {
            (inline_end, inline_start)
        };
        Edges {
            left: lower_relative_logical(self.style.clone_relative_align_left(), logical_left),
            right: lower_relative_logical(self.style.clone_relative_align_right(), logical_right),
            top: self.style.clone_relative_align_top(),
            bottom: self.style.clone_relative_align_bottom(),
        }
    }

    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        let ltr = self.style.clone_direction() == direction::T::Ltr;
        let (inline_start, inline_end) = (
            self.style.clone_relative_inline_start_of(),
            self.style.clone_relative_inline_end_of(),
        );
        let (logical_left, logical_right) = if ltr {
            (inline_start, inline_end)
        } else {
            (inline_end, inline_start)
        };
        Edges {
            left: lower_relative_logical(self.style.clone_relative_left_of(), logical_left),
            right: lower_relative_logical(self.style.clone_relative_right_of(), logical_right),
            top: self.style.clone_relative_top_of(),
            bottom: self.style.clone_relative_bottom_of(),
        }
    }

    fn relative_center(&self) -> relative_center::T {
        self.style.clone_relative_center()
    }

    fn order(&self) -> i32 {
        FlexItemStyle::order(self)
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
        assert_eq!(size_of::<TextStyleView<'static, ()>>(), 7 * word);
    }
}
