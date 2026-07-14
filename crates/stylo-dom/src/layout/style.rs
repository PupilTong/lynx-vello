//! Lazy translation from stylo computed values to `neutron-star` styles.
//!
//! Scalar layout values are translated on each protocol accessor. Sequence
//! values (grid tracks and text font settings) are exposed through borrowed
//! iterators, so neutron-star remains the only layer that materializes the
//! scratch data its algorithms need. Percent-bearing `calc()` expressions
//! cross the dependency boundary as stable pointers into the source epoch's
//! `ComputedValues`; [`resolve_calc`] is the single place that turns those
//! opaque handles back into stylo values.

use std::slice;

use neutron_star::geometry::{Edges, Line, Point, Size};
use neutron_star::style::{
    AlignContent, AlignItems, AlignSelf, BoxGenerationMode, BoxSizing, CalcHandle, CoreStyle,
    Dimension, Direction, FlexContainerStyle, FlexDirection, FlexItemStyle, FlexWrap, FontFamily,
    FontFeatureSetting, FontStyle, FontVariationSetting, FontWeight, GenericFontFamily,
    GridAutoFlow, GridContainerStyle, GridItemStyle, GridLine, GridPlacement,
    GridTemplateComponent, GridTemplateRepetition, JustifyContent, JustifyItems, JustifySelf,
    LengthPercentage, LengthPercentageAuto, LineHeight, LinearContainerStyle, LinearItemStyle,
    LinearOrientation, MaxTrackSizingFunction, MinTrackSizingFunction, Overflow, Position,
    RelativeCenter, RelativeContainerStyle, RelativeItemStyle, RelativeReference, RepetitionCount,
    TextAlign, TextContainerStyle, TextRunStyle, TrackSizingFunction, Visibility, WhiteSpace,
    WordBreak,
};
use rustc_hash::FxHashSet;
use stylo::properties::ComputedValues;
use stylo::properties::generated::longhands::box_sizing::computed_value::T as StyloBoxSizing;
use stylo::properties::longhands::position::computed_value::T as StyloPosition;
use stylo::servo_arc::Arc;
use stylo::values::computed::font::{
    FontFamily as StyloFontFamily, FontFeatureSettings as StyloFontFeatureSettings,
    FontVariationSettings as StyloFontVariationSettings, GenericFontFamily as StyloGenericFamily,
    SingleFontFamily,
};
use stylo::values::computed::length_percentage::{
    CalcLengthPercentage, Unpacked as UnpackedLengthPercentage,
};
use stylo::values::computed::{
    CSSPixelLength, GridLine as StyloGridLine, GridTemplateComponent as StyloGridTemplate,
    ImplicitGridTracks, LengthPercentage as StyloLengthPercentage,
};
use stylo::values::generics::NonNegative;
use stylo::values::generics::flex::GenericFlexBasis;
use stylo::values::generics::grid::{
    RepeatCount, TrackBreadth, TrackListValue, TrackRepeat, TrackSize,
};
use stylo::values::generics::length::{GenericMargin, GenericMaxSize, GenericSize};
use stylo::values::generics::position::{
    GenericAspectRatio, Inset as GenericInset, PreferredRatio,
};
use stylo::values::specified::align::{AlignFlags, ContentDistribution};
use stylo::values::specified::border::BorderStyle;
use stylo::values::specified::box_::{Display, DisplayInside, Overflow as StyloOverflow};
use stylo_atoms::atom;

type StyloSize = GenericSize<NonNegative<StyloLengthPercentage>>;
type StyloMaxSize = GenericMaxSize<NonNegative<StyloLengthPercentage>>;
type StyloMargin = GenericMargin<StyloLengthPercentage>;
type StyloInset = GenericInset<stylo::values::computed::Percentage, StyloLengthPercentage>;
type StyloGap = stylo::values::generics::length::GenericLengthPercentageOrNormal<
    NonNegative<StyloLengthPercentage>,
>;
type StyloTrackSize = TrackSize<StyloLengthPercentage>;
type StyloTrackRepeat = TrackRepeat<StyloLengthPercentage, i32>;
type StyloTrackListValue = TrackListValue<StyloLengthPercentage, i32>;

/// The layout-relevant category of a computed `display` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutDisplay {
    /// `display: none`.
    None,
    /// `display: contents`; the formatting-tree projection must flatten this
    /// element before selecting a box algorithm.
    Contents,
    /// An ordinary CSS flow formatting context that neutron-star does not yet
    /// implement.
    Flow,
    /// CSS Flexbox.
    Flex,
    /// CSS Grid.
    Grid,
    /// Lynx Linear layout.
    Linear,
    /// Lynx Relative layout.
    Relative,
}

/// A cheap borrowed style view over one immutable stylo computed style.
#[derive(Debug, Clone, Copy)]
pub struct ComputedLayoutStyle<'a> {
    computed: &'a ComputedValues,
}

impl<'a> ComputedLayoutStyle<'a> {
    /// Creates a neutron-star style view for `computed`.
    #[must_use]
    pub const fn new(computed: &'a ComputedValues) -> Self {
        Self { computed }
    }

    /// Returns the host dispatch mode for the computed `display` value.
    #[must_use]
    pub fn layout_display(self) -> LayoutDisplay {
        classify_display(self.computed.clone_display())
    }

    /// The underlying computed values for host-only decisions.
    #[must_use]
    pub const fn computed(self) -> &'a ComputedValues {
        self.computed
    }
}

fn classify_display(display: Display) -> LayoutDisplay {
    match display.inside() {
        DisplayInside::None => LayoutDisplay::None,
        DisplayInside::Contents => LayoutDisplay::Contents,
        DisplayInside::Flex => LayoutDisplay::Flex,
        DisplayInside::Grid => LayoutDisplay::Grid,
        DisplayInside::LynxLinear => LayoutDisplay::Linear,
        DisplayInside::LynxRelative => LayoutDisplay::Relative,
        _ => LayoutDisplay::Flow,
    }
}

impl CoreStyle for ComputedLayoutStyle<'_> {
    fn box_generation_mode(&self) -> BoxGenerationMode {
        match self.layout_display() {
            LayoutDisplay::None => BoxGenerationMode::None,
            // `display: contents` nodes are flattened by the formatting-tree
            // projection before a neutron-star style is requested. Treating
            // one as `None` here would incorrectly suppress its descendants.
            LayoutDisplay::Contents
            | LayoutDisplay::Flow
            | LayoutDisplay::Flex
            | LayoutDisplay::Grid
            | LayoutDisplay::Linear
            | LayoutDisplay::Relative => BoxGenerationMode::Normal,
        }
    }

    fn visibility(&self) -> Visibility {
        use stylo::computed_values::visibility::T;

        match self.computed.clone_visibility() {
            T::Visible => Visibility::Visible,
            T::Hidden => Visibility::Hidden,
            T::Collapse => Visibility::Collapse,
        }
    }

    fn position(&self) -> Position {
        match self.computed.clone_position() {
            // The generic protocol's local absolute variant assumes the
            // formatting parent is already the containing block. The DOM
            // projection does not yet discover/hoist across static ancestors;
            // this mapping is therefore intentionally documented as pending
            // full CSS Positioned Layout conformance.
            StyloPosition::Absolute => Position::Absolute,
            StyloPosition::Fixed => Position::AbsoluteHoisted,
            StyloPosition::Static | StyloPosition::Relative | StyloPosition::Sticky => {
                Position::Relative
            }
        }
    }

    fn inset(&self) -> Edges<LengthPercentageAuto> {
        if matches!(
            self.computed.clone_position(),
            StyloPosition::Static | StyloPosition::Sticky
        ) {
            // The protocol's in-flow variant also represents `static`, but
            // static insets have no effect. Sticky insets belong to the
            // runtime's future sticky post-pass, not relative box offsets.
            return Edges::uniform(LengthPercentageAuto::Auto);
        }
        let position = self.computed.get_position();
        Edges {
            left: inset(&position.left),
            right: inset(&position.right),
            top: inset(&position.top),
            bottom: inset(&position.bottom),
        }
    }

    fn size(&self) -> Size<Dimension> {
        let position = self.computed.get_position();
        Size::new(dimension(&position.width), dimension(&position.height))
    }

    fn min_size(&self) -> Size<Dimension> {
        let position = self.computed.get_position();
        Size::new(
            dimension(&position.min_width),
            dimension(&position.min_height),
        )
    }

    fn max_size(&self) -> Size<Dimension> {
        let position = self.computed.get_position();
        Size::new(
            max_dimension(&position.max_width),
            max_dimension(&position.max_height),
        )
    }

    fn aspect_ratio(&self) -> Option<f32> {
        aspect_ratio(self.computed.get_position().aspect_ratio)
    }

    fn margin(&self) -> Edges<LengthPercentageAuto> {
        let margin = self.computed.get_margin();
        Edges {
            left: margin_value(&margin.margin_left),
            right: margin_value(&margin.margin_right),
            top: margin_value(&margin.margin_top),
            bottom: margin_value(&margin.margin_bottom),
        }
    }

    fn padding(&self) -> Edges<LengthPercentage> {
        let padding = self.computed.get_padding();
        Edges {
            left: length_percentage(&padding.padding_left.0),
            right: length_percentage(&padding.padding_right.0),
            top: length_percentage(&padding.padding_top.0),
            bottom: length_percentage(&padding.padding_bottom.0),
        }
    }

    fn border(&self) -> Edges<LengthPercentage> {
        let border = self.computed.get_border();
        Edges {
            left: border_width(&border.border_left_width, border.border_left_style),
            right: border_width(&border.border_right_width, border.border_right_style),
            top: border_width(&border.border_top_width, border.border_top_style),
            bottom: border_width(&border.border_bottom_width, border.border_bottom_style),
        }
    }

    fn overflow(&self) -> Point<Overflow> {
        Point::new(
            overflow(self.computed.clone_overflow_x()),
            overflow(self.computed.clone_overflow_y()),
        )
    }

    fn box_sizing(&self) -> BoxSizing {
        match self.computed.clone_box_sizing() {
            StyloBoxSizing::ContentBox => BoxSizing::ContentBox,
            StyloBoxSizing::BorderBox => BoxSizing::BorderBox,
        }
    }

    fn direction(&self) -> Direction {
        use stylo::computed_values::direction::T;

        match self.computed.clone_direction() {
            T::Ltr => Direction::Ltr,
            T::Rtl => Direction::Rtl,
        }
    }
}

impl FlexContainerStyle for ComputedLayoutStyle<'_> {
    fn flex_direction(&self) -> FlexDirection {
        use stylo::computed_values::flex_direction::T;

        match self.computed.get_position().flex_direction {
            T::Row => FlexDirection::Row,
            T::RowReverse => FlexDirection::RowReverse,
            T::Column => FlexDirection::Column,
            T::ColumnReverse => FlexDirection::ColumnReverse,
        }
    }

    fn flex_wrap(&self) -> FlexWrap {
        use stylo::computed_values::flex_wrap::T;

        match self.computed.get_position().flex_wrap {
            T::Nowrap => FlexWrap::NoWrap,
            T::Wrap => FlexWrap::Wrap,
            T::WrapReverse => FlexWrap::WrapReverse,
        }
    }

    fn gap(&self) -> Size<LengthPercentage> {
        gap(self.computed)
    }

    fn align_content(&self) -> Option<AlignContent> {
        content_alignment(self.computed.get_position().align_content, self.direction())
    }

    fn align_items(&self) -> Option<AlignItems> {
        item_alignment(self.computed.get_position().align_items.0, self.direction())
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        content_alignment(
            self.computed.get_position().justify_content,
            self.direction(),
        )
    }
}

impl FlexItemStyle for ComputedLayoutStyle<'_> {
    fn flex_basis(&self) -> Dimension {
        match &self.computed.get_position().flex_basis {
            GenericFlexBasis::Content => Dimension::MaxContent,
            GenericFlexBasis::Size(size) => dimension(size),
        }
    }

    fn flex_grow(&self) -> f32 {
        self.computed.get_position().flex_grow.0
    }

    fn flex_shrink(&self) -> f32 {
        self.computed.get_position().flex_shrink.0
    }

    fn align_self(&self) -> Option<AlignSelf> {
        item_alignment(self.computed.get_position().align_self.0, self.direction())
    }

    fn order(&self) -> i32 {
        self.computed.get_position().order
    }
}

impl GridContainerStyle for ComputedLayoutStyle<'_> {
    type Repetition<'a>
        = ComputedGridRepetition<'a>
    where
        Self: 'a;
    type TemplateTracks<'a>
        = ComputedGridTemplateTracks<'a>
    where
        Self: 'a;
    type AutoTracks<'a>
        = ComputedGridTracks<'a>
    where
        Self: 'a;

    fn grid_template_rows(&self) -> Self::TemplateTracks<'_> {
        ComputedGridTemplateTracks::new(&self.computed.get_position().grid_template_rows)
    }

    fn grid_template_columns(&self) -> Self::TemplateTracks<'_> {
        ComputedGridTemplateTracks::new(&self.computed.get_position().grid_template_columns)
    }

    fn grid_auto_rows(&self) -> Self::AutoTracks<'_> {
        ComputedGridTracks::new(&self.computed.get_position().grid_auto_rows)
    }

    fn grid_auto_columns(&self) -> Self::AutoTracks<'_> {
        ComputedGridTracks::new(&self.computed.get_position().grid_auto_columns)
    }

    fn grid_auto_flow(&self) -> GridAutoFlow {
        use stylo::computed_values::grid_auto_flow::T;

        let value = self.computed.get_position().grid_auto_flow;
        match (value.contains(T::ROW), value.contains(T::DENSE)) {
            (true, false) => GridAutoFlow::Row,
            (true, true) => GridAutoFlow::RowDense,
            (false, false) => GridAutoFlow::Column,
            (false, true) => GridAutoFlow::ColumnDense,
        }
    }

    fn gap(&self) -> Size<LengthPercentage> {
        gap(self.computed)
    }

    fn align_content(&self) -> Option<AlignContent> {
        content_alignment(self.computed.get_position().align_content, self.direction())
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        content_alignment(
            self.computed.get_position().justify_content,
            self.direction(),
        )
    }

    fn align_items(&self) -> Option<AlignItems> {
        item_alignment(self.computed.get_position().align_items.0, self.direction())
    }

    fn justify_items(&self) -> Option<JustifyItems> {
        item_alignment(
            (self.computed.get_position().justify_items.computed.0).0,
            self.direction(),
        )
    }
}

impl GridItemStyle for ComputedLayoutStyle<'_> {
    fn grid_row(&self) -> Line<GridPlacement> {
        let position = self.computed.get_position();
        Line::new(
            grid_placement(&position.grid_row_start),
            grid_placement(&position.grid_row_end),
        )
    }

    fn grid_column(&self) -> Line<GridPlacement> {
        let position = self.computed.get_position();
        Line::new(
            grid_placement(&position.grid_column_start),
            grid_placement(&position.grid_column_end),
        )
    }

    fn align_self(&self) -> Option<AlignSelf> {
        item_alignment(self.computed.get_position().align_self.0, self.direction())
    }

    fn justify_self(&self) -> Option<JustifySelf> {
        item_alignment(
            self.computed.get_position().justify_self.0,
            self.direction(),
        )
    }

    fn order(&self) -> i32 {
        self.computed.get_position().order
    }
}

impl LinearContainerStyle for ComputedLayoutStyle<'_> {
    fn linear_orientation(&self) -> LinearOrientation {
        use stylo::properties::longhands::linear_direction::computed_value::T;

        match self.computed.clone_linear_direction() {
            T::Row => LinearOrientation::Row,
            T::RowReverse => LinearOrientation::RowReverse,
            T::Column => LinearOrientation::Column,
            T::ColumnReverse => LinearOrientation::ColumnReverse,
        }
    }

    fn linear_weight_sum(&self) -> f32 {
        self.computed.clone_linear_weight_sum().0
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        content_alignment(
            self.computed.get_position().justify_content,
            self.direction(),
        )
    }

    fn align_items(&self) -> Option<AlignItems> {
        item_alignment(self.computed.get_position().align_items.0, self.direction())
    }
}

impl LinearItemStyle for ComputedLayoutStyle<'_> {
    fn linear_weight(&self) -> f32 {
        self.computed.clone_linear_weight().0
    }

    fn align_self(&self) -> Option<AlignSelf> {
        item_alignment(self.computed.get_position().align_self.0, self.direction())
    }

    fn order(&self) -> i32 {
        self.computed.get_position().order
    }
}

impl RelativeContainerStyle for ComputedLayoutStyle<'_> {
    fn relative_layout_once(&self) -> bool {
        use stylo::properties::longhands::relative_layout_once::computed_value::T;

        self.computed.clone_relative_layout_once() == T::True
    }
}

impl RelativeItemStyle for ComputedLayoutStyle<'_> {
    fn relative_id(&self) -> RelativeReference {
        RelativeReference::new(self.computed.clone_relative_id())
    }

    fn relative_align(&self) -> Edges<RelativeReference> {
        let mut edges = Edges {
            left: RelativeReference::new(self.computed.clone_relative_align_left()),
            right: RelativeReference::new(self.computed.clone_relative_align_right()),
            top: RelativeReference::new(self.computed.clone_relative_align_top()),
            bottom: RelativeReference::new(self.computed.clone_relative_align_bottom()),
        };
        let inline_start =
            RelativeReference::new(self.computed.clone_relative_align_inline_start());
        let inline_end = RelativeReference::new(self.computed.clone_relative_align_inline_end());
        lower_logical_references(&mut edges, inline_start, inline_end, self.direction());
        edges
    }

    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        let mut edges = Edges {
            left: RelativeReference::new(self.computed.clone_relative_left_of()),
            right: RelativeReference::new(self.computed.clone_relative_right_of()),
            top: RelativeReference::new(self.computed.clone_relative_top_of()),
            bottom: RelativeReference::new(self.computed.clone_relative_bottom_of()),
        };
        let inline_start = RelativeReference::new(self.computed.clone_relative_inline_start_of());
        let inline_end = RelativeReference::new(self.computed.clone_relative_inline_end_of());
        lower_logical_references(&mut edges, inline_start, inline_end, self.direction());
        edges
    }

    fn relative_center(&self) -> RelativeCenter {
        use stylo::properties::longhands::relative_center::computed_value::T;

        match self.computed.clone_relative_center() {
            T::None => RelativeCenter::None,
            T::Horizontal => RelativeCenter::Horizontal,
            T::Vertical => RelativeCenter::Vertical,
            T::Both => RelativeCenter::Both,
        }
    }

    fn order(&self) -> i32 {
        self.computed.get_position().order
    }
}

impl TextContainerStyle for ComputedLayoutStyle<'_> {
    fn text_align(&self) -> TextAlign {
        use stylo::values::computed::text::TextAlign as T;

        match self.computed.clone_text_align() {
            T::Left | T::MozLeft => TextAlign::Left,
            T::Right | T::MozRight => TextAlign::Right,
            T::Center | T::MozCenter => TextAlign::Center,
            T::Start => TextAlign::Start,
            T::End => TextAlign::End,
            T::Justify => TextAlign::Justify,
        }
    }

    fn white_space(&self) -> WhiteSpace {
        use stylo::computed_values::text_wrap_mode::T;

        match self.computed.clone_text_wrap_mode() {
            T::Wrap => WhiteSpace::Normal,
            T::Nowrap => WhiteSpace::NoWrap,
        }
    }

    fn word_break(&self) -> WordBreak {
        use stylo::values::computed::text::WordBreak as T;

        match self.computed.clone_word_break() {
            T::Normal => WordBreak::Normal,
            T::BreakAll => WordBreak::BreakAll,
            T::KeepAll => WordBreak::KeepAll,
        }
    }

    fn text_indent(&self) -> LengthPercentage {
        // Borrow the value retained by the epoch. A `LengthPercentage` clone
        // deep-clones its boxed calc expression, so taking an opaque handle
        // from `clone_text_indent()` would leave a dangling pointer when that
        // temporary is dropped.
        length_percentage(&self.computed.get_inherited_text().text_indent.length)
    }
}

/// An owning computed text-run style suitable for storage beside source runs.
///
/// Stylo's generated scalar accessors are cheap, while the three sequence
/// accessors return owned computed values. Capturing those sequences once here
/// lets `TextRunStyle` lend them without allocation during shaping.
#[derive(Debug, Clone)]
pub struct ComputedTextRunStyle {
    computed: Arc<ComputedValues>,
    font_family: StyloFontFamily,
    font_feature_settings: StyloFontFeatureSettings,
    font_variation_settings: StyloFontVariationSettings,
}

impl ComputedTextRunStyle {
    /// Captures a computed style for use by one or more immutable text runs.
    #[must_use]
    pub fn new(computed: Arc<ComputedValues>) -> Self {
        let font_family = computed.clone_font_family();
        let font_feature_settings = computed.clone_font_feature_settings();
        let font_variation_settings = computed.clone_font_variation_settings();
        Self {
            computed,
            font_family,
            font_feature_settings,
            font_variation_settings,
        }
    }

    /// Borrows the underlying computed values.
    #[must_use]
    pub fn computed(&self) -> &ComputedValues {
        &self.computed
    }
}

impl TextRunStyle for ComputedTextRunStyle {
    type FontFamilies<'a>
        = ComputedFontFamilies<'a>
    where
        Self: 'a;
    type FontFeatureSettings<'a>
        = ComputedFontFeatureSettings<'a>
    where
        Self: 'a;
    type FontVariationSettings<'a>
        = ComputedFontVariationSettings<'a>
    where
        Self: 'a;

    fn font_families(&self) -> Self::FontFamilies<'_> {
        ComputedFontFamilies {
            inner: self.font_family.families.list.iter(),
        }
    }

    fn font_size(&self) -> f32 {
        self.computed.clone_font_size().computed_size().px()
    }

    fn font_weight(&self) -> FontWeight {
        let weight = self.computed.clone_font_weight().value();
        match weight {
            value if value < 150.0 => FontWeight::W100,
            value if value < 250.0 => FontWeight::W200,
            value if value < 350.0 => FontWeight::W300,
            value if value < 450.0 => FontWeight::W400,
            value if value < 550.0 => FontWeight::W500,
            value if value < 650.0 => FontWeight::W600,
            value if value < 750.0 => FontWeight::W700,
            value if value < 850.0 => FontWeight::W800,
            _ => FontWeight::W900,
        }
    }

    fn font_style(&self) -> FontStyle {
        use stylo::values::computed::font::FontStyle as T;

        let style = self.computed.clone_font_style();
        if style == T::NORMAL {
            FontStyle::Normal
        } else if style == T::ITALIC {
            FontStyle::Italic
        } else {
            FontStyle::Oblique
        }
    }

    fn letter_spacing(&self) -> f32 {
        self.computed
            .clone_letter_spacing()
            .0
            .resolve(CSSPixelLength::new(0.0))
            .px()
    }

    fn line_height(&self) -> LineHeight {
        use stylo::values::generics::font::GenericLineHeight;

        match self.computed.clone_line_height() {
            GenericLineHeight::Normal => LineHeight::Normal,
            GenericLineHeight::Number(value) => LineHeight::Factor(value.0),
            GenericLineHeight::Length(value) => LineHeight::Length(value.0.px()),
        }
    }

    fn white_space(&self) -> Option<WhiteSpace> {
        use stylo::computed_values::text_wrap_mode::T;

        Some(match self.computed.clone_text_wrap_mode() {
            T::Wrap => WhiteSpace::Normal,
            T::Nowrap => WhiteSpace::NoWrap,
        })
    }

    fn word_break(&self) -> Option<WordBreak> {
        use stylo::values::computed::text::WordBreak as T;

        Some(match self.computed.clone_word_break() {
            T::Normal => WordBreak::Normal,
            T::BreakAll => WordBreak::BreakAll,
            T::KeepAll => WordBreak::KeepAll,
        })
    }

    fn font_feature_settings(&self) -> Self::FontFeatureSettings<'_> {
        ComputedFontFeatureSettings {
            inner: self.font_feature_settings.0.iter(),
        }
    }

    fn font_variation_settings(&self) -> Self::FontVariationSettings<'_> {
        ComputedFontVariationSettings {
            inner: self.font_variation_settings.0.iter(),
        }
    }
}

/// Borrowed iterator over a stylo font-family list.
#[derive(Debug, Clone)]
pub struct ComputedFontFamilies<'a> {
    inner: slice::Iter<'a, SingleFontFamily>,
}

impl<'a> Iterator for ComputedFontFamilies<'a> {
    type Item = FontFamily<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next()? {
                SingleFontFamily::FamilyName(name) => {
                    return Some(FontFamily::Named(name.name.as_ref()));
                }
                SingleFontFamily::Generic(generic) => {
                    if let Some(generic) = generic_font_family(*generic) {
                        return Some(FontFamily::Generic(generic));
                    }
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.inner.len()))
    }
}

/// Borrowed iterator over computed OpenType feature settings.
#[derive(Debug, Clone)]
pub struct ComputedFontFeatureSettings<'a> {
    inner: slice::Iter<'a, stylo::values::generics::font::FeatureTagValue<i32>>,
}

impl Iterator for ComputedFontFeatureSettings<'_> {
    type Item = FontFeatureSetting;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|setting| {
            let value =
                u16::try_from(setting.value.clamp(0, i32::from(u16::MAX))).unwrap_or(u16::MAX);
            (setting.tag.0.to_be_bytes(), value)
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for ComputedFontFeatureSettings<'_> {}

/// Borrowed iterator over computed OpenType variation settings.
#[derive(Debug, Clone)]
pub struct ComputedFontVariationSettings<'a> {
    inner: slice::Iter<'a, stylo::values::generics::font::VariationValue<f32>>,
}

impl Iterator for ComputedFontVariationSettings<'_> {
    type Item = FontVariationSetting;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner
            .next()
            .map(|setting| (setting.tag.0.to_be_bytes(), setting.value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for ComputedFontVariationSettings<'_> {}

/// Borrowed view of one stylo `repeat(...)` component.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComputedGridRepetition<'a> {
    inner: &'a StyloTrackRepeat,
}

impl GridTemplateRepetition for ComputedGridRepetition<'_> {
    type Tracks<'a>
        = ComputedGridTrackSlice<'a>
    where
        Self: 'a;

    fn count(&self) -> RepetitionCount {
        repetition_count(self.inner.count)
    }

    fn tracks(&self) -> Self::Tracks<'_> {
        ComputedGridTrackSlice {
            inner: self.inner.track_sizes.iter(),
        }
    }
}

/// Borrowed iterator over template track components.
#[derive(Debug, Clone)]
pub struct ComputedGridTemplateTracks<'a> {
    inner: slice::Iter<'a, StyloTrackListValue>,
}

impl<'a> ComputedGridTemplateTracks<'a> {
    fn new(template: &'a StyloGridTemplate) -> Self {
        let values = match template {
            StyloGridTemplate::TrackList(list) => list.values.as_ref(),
            StyloGridTemplate::None
            | StyloGridTemplate::Subgrid(_)
            | StyloGridTemplate::Masonry => &[],
        };
        Self {
            inner: values.iter(),
        }
    }
}

impl<'a> Iterator for ComputedGridTemplateTracks<'a> {
    type Item = GridTemplateComponent<ComputedGridRepetition<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|value| match value {
            TrackListValue::TrackSize(track) => GridTemplateComponent::Single(track_size(track)),
            TrackListValue::TrackRepeat(repetition) => {
                GridTemplateComponent::Repeat(ComputedGridRepetition { inner: repetition })
            }
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for ComputedGridTemplateTracks<'_> {}

/// Borrowed cloneable iterator over implicit grid tracks.
#[derive(Debug, Clone)]
pub struct ComputedGridTracks<'a> {
    inner: slice::Iter<'a, StyloTrackSize>,
}

impl<'a> ComputedGridTracks<'a> {
    fn new(tracks: &'a ImplicitGridTracks) -> Self {
        Self {
            inner: tracks.0.iter(),
        }
    }
}

impl Iterator for ComputedGridTracks<'_> {
    type Item = TrackSizingFunction;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(track_size)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for ComputedGridTracks<'_> {}

/// Borrowed cloneable iterator used inside `repeat(...)`.
#[derive(Debug, Clone)]
pub struct ComputedGridTrackSlice<'a> {
    inner: slice::Iter<'a, StyloTrackSize>,
}

impl Iterator for ComputedGridTrackSlice<'_> {
    type Item = TrackSizingFunction;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(track_size)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for ComputedGridTrackSlice<'_> {}

fn length_percentage(value: &StyloLengthPercentage) -> LengthPercentage {
    match value.unpack() {
        UnpackedLengthPercentage::Length(length) => LengthPercentage::Length(length.px()),
        UnpackedLengthPercentage::Percentage(percentage) => LengthPercentage::Percent(percentage.0),
        UnpackedLengthPercentage::Calc(calc) => LengthPercentage::Calc(calc_handle(calc)),
    }
}

fn length_percentage_auto(value: &StyloLengthPercentage) -> LengthPercentageAuto {
    length_percentage(value).into()
}

fn calc_handle(calc: &CalcLengthPercentage) -> CalcHandle {
    let address = std::ptr::from_ref(calc).expose_provenance();
    CalcHandle::from_raw(
        u64::try_from(address).expect("CalcHandle requires at most 64-bit pointers"),
    )
}

/// Resolves a handle emitted by this module during the current source epoch.
///
/// The layout snapshot owns every `Arc<ComputedValues>` from which a handle is
/// produced, so the pointee is stable until recursion ends. Keeping the cast in
/// this adapter prevents neutron-star from depending on stylo or exposing raw
/// pointers in its protocol.
///
/// # Panics
///
/// Panics when `handle` was not registered by the current source epoch.
#[expect(
    unsafe_code,
    reason = "opaque calc handles borrow allocations retained by the source epoch"
)]
#[must_use]
pub(super) fn resolve_calc(handle: CalcHandle, basis: f32, valid_handles: &FxHashSet<u64>) -> f32 {
    assert!(
        valid_handles.contains(&handle.raw()),
        "calc handle does not belong to this DOM layout source epoch"
    );
    let address =
        usize::try_from(handle.raw()).expect("CalcHandle must originate on the current target");
    let pointer = std::ptr::with_exposed_provenance::<CalcLengthPercentage>(address);
    // SAFETY: `calc_handle` is the only producer used by this module's source
    // views. The source epoch retains the owning computed-style Arcs while any
    // translated value can reach neutron-star, so the pointee remains valid and
    // immutable for this call.
    unsafe { &*pointer }
        .resolve(CSSPixelLength::new(basis))
        .px()
}

fn dimension(value: &StyloSize) -> Dimension {
    match value {
        GenericSize::LengthPercentage(value) => length_percentage(&value.0).into(),
        GenericSize::Auto => Dimension::Auto,
        GenericSize::MinContent => Dimension::MinContent,
        GenericSize::MaxContent => Dimension::MaxContent,
        // The keyword form is the available-space fit-content formula. A 100%
        // symbolic limit preserves that basis until the layout algorithm knows
        // the containing size.
        GenericSize::FitContent => Dimension::FitContent(LengthPercentage::Percent(1.0)),
        GenericSize::FitContentFunction(value) => {
            Dimension::FitContent(length_percentage(&value.0))
        }
        GenericSize::Stretch | GenericSize::WebkitFillAvailable => Dimension::Percent(1.0),
        GenericSize::AnchorSizeFunction(_) | GenericSize::AnchorContainingCalcFunction(_) => {
            Dimension::Auto
        }
    }
}

fn max_dimension(value: &StyloMaxSize) -> Dimension {
    match value {
        GenericMaxSize::LengthPercentage(value) => length_percentage(&value.0).into(),
        GenericMaxSize::None => Dimension::Auto,
        GenericMaxSize::MinContent => Dimension::MinContent,
        GenericMaxSize::MaxContent => Dimension::MaxContent,
        GenericMaxSize::FitContent => Dimension::FitContent(LengthPercentage::Percent(1.0)),
        GenericMaxSize::FitContentFunction(value) => {
            Dimension::FitContent(length_percentage(&value.0))
        }
        GenericMaxSize::Stretch | GenericMaxSize::WebkitFillAvailable => Dimension::Percent(1.0),
        GenericMaxSize::AnchorSizeFunction(_) | GenericMaxSize::AnchorContainingCalcFunction(_) => {
            Dimension::Auto
        }
    }
}

fn margin_value(value: &StyloMargin) -> LengthPercentageAuto {
    match value {
        GenericMargin::Auto => LengthPercentageAuto::Auto,
        GenericMargin::LengthPercentage(value) => length_percentage_auto(value),
        GenericMargin::AnchorSizeFunction(_) | GenericMargin::AnchorContainingCalcFunction(_) => {
            LengthPercentageAuto::Auto
        }
    }
}

fn inset(value: &StyloInset) -> LengthPercentageAuto {
    match value {
        GenericInset::LengthPercentage(value) => length_percentage_auto(value),
        GenericInset::Auto
        | GenericInset::AnchorFunction(_)
        | GenericInset::AnchorSizeFunction(_)
        | GenericInset::AnchorContainingCalcFunction(_) => LengthPercentageAuto::Auto,
    }
}

fn border_width(
    width: &stylo::values::computed::BorderSideWidth,
    style: BorderStyle,
) -> LengthPercentage {
    if style.none_or_hidden() {
        LengthPercentage::ZERO
    } else {
        LengthPercentage::Length(width.0.to_f32_px())
    }
}

fn overflow(value: StyloOverflow) -> Overflow {
    match value {
        StyloOverflow::Visible => Overflow::Visible,
        StyloOverflow::Clip => Overflow::Clip,
        StyloOverflow::Hidden => Overflow::Hidden,
        StyloOverflow::Scroll | StyloOverflow::Auto => Overflow::Scroll,
    }
}

fn aspect_ratio(value: GenericAspectRatio<NonNegative<f32>>) -> Option<f32> {
    match value.ratio {
        PreferredRatio::Ratio(value) if (value.1).0 > 0.0 => Some((value.0).0 / (value.1).0),
        PreferredRatio::None | PreferredRatio::Ratio(_) => None,
    }
}

fn gap(computed: &ComputedValues) -> Size<LengthPercentage> {
    use stylo::values::generics::length::GenericLengthPercentageOrNormal;

    let position = computed.get_position();
    let convert = |value: &StyloGap| match value {
        GenericLengthPercentageOrNormal::Normal => LengthPercentage::ZERO,
        GenericLengthPercentageOrNormal::LengthPercentage(value) => length_percentage(&value.0),
    };
    Size::new(convert(&position.column_gap), convert(&position.row_gap))
}

fn content_alignment(value: ContentDistribution, direction: Direction) -> Option<AlignContent> {
    match value.primary().value() {
        AlignFlags::START
        | AlignFlags::SELF_START
        // The current engine protocol has no baseline content-distribution
        // variant. CSS aligns the fallback edge when no shared baseline exists.
        | AlignFlags::BASELINE
        | AlignFlags::LAST_BASELINE => Some(AlignContent::Start),
        AlignFlags::END | AlignFlags::SELF_END => Some(AlignContent::End),
        AlignFlags::LEFT => Some(if direction == Direction::Ltr {
            AlignContent::Start
        } else {
            AlignContent::End
        }),
        AlignFlags::RIGHT => Some(if direction == Direction::Ltr {
            AlignContent::End
        } else {
            AlignContent::Start
        }),
        AlignFlags::FLEX_START => Some(AlignContent::FlexStart),
        AlignFlags::FLEX_END => Some(AlignContent::FlexEnd),
        AlignFlags::CENTER => Some(AlignContent::Center),
        AlignFlags::STRETCH => Some(AlignContent::Stretch),
        AlignFlags::SPACE_BETWEEN => Some(AlignContent::SpaceBetween),
        AlignFlags::SPACE_AROUND => Some(AlignContent::SpaceAround),
        AlignFlags::SPACE_EVENLY => Some(AlignContent::SpaceEvenly),
        _ => None,
    }
}

fn item_alignment(value: AlignFlags, direction: Direction) -> Option<AlignItems> {
    match value.value() {
        AlignFlags::STRETCH => Some(AlignItems::Stretch),
        AlignFlags::START | AlignFlags::SELF_START => Some(AlignItems::Start),
        AlignFlags::END | AlignFlags::SELF_END => Some(AlignItems::End),
        AlignFlags::LEFT => Some(if direction == Direction::Ltr {
            AlignItems::Start
        } else {
            AlignItems::End
        }),
        AlignFlags::RIGHT => Some(if direction == Direction::Ltr {
            AlignItems::End
        } else {
            AlignItems::Start
        }),
        AlignFlags::FLEX_START => Some(AlignItems::FlexStart),
        AlignFlags::FLEX_END => Some(AlignItems::FlexEnd),
        AlignFlags::CENTER => Some(AlignItems::Center),
        AlignFlags::BASELINE | AlignFlags::LAST_BASELINE => Some(AlignItems::Baseline),
        _ => None,
    }
}

fn grid_placement(value: &StyloGridLine) -> GridPlacement {
    if value.is_auto() || value.ident.0 != atom!("") {
        return GridPlacement::Auto;
    }

    if value.is_span {
        let span = u16::try_from(value.line_num.clamp(1, i32::from(u16::MAX))).unwrap_or(u16::MAX);
        GridPlacement::Span(span)
    } else {
        i16::try_from(value.line_num)
            .ok()
            .filter(|line| *line != 0)
            .map_or(GridPlacement::Auto, |line| {
                GridPlacement::Line(GridLine::new(line))
            })
    }
}

fn repetition_count(value: RepeatCount<i32>) -> RepetitionCount {
    match value {
        RepeatCount::Number(count) => {
            let count = u16::try_from(count.clamp(1, i32::from(u16::MAX))).unwrap_or(u16::MAX);
            RepetitionCount::Count(count)
        }
        RepeatCount::AutoFill => RepetitionCount::AutoFill,
        RepeatCount::AutoFit => RepetitionCount::AutoFit,
    }
}

fn track_size(value: &StyloTrackSize) -> TrackSizingFunction {
    match value {
        TrackSize::Breadth(breadth) => single_track_breadth(breadth),
        TrackSize::Minmax(minimum, maximum) => {
            TrackSizingFunction::minmax(min_track_breadth(minimum), max_track_breadth(maximum))
        }
        TrackSize::FitContent(TrackBreadth::Breadth(limit)) => {
            TrackSizingFunction::fit_content(length_percentage(limit))
        }
        TrackSize::FitContent(_) => TrackSizingFunction::AUTO,
    }
}

fn single_track_breadth(value: &TrackBreadth<StyloLengthPercentage>) -> TrackSizingFunction {
    match value {
        TrackBreadth::Breadth(value) => TrackSizingFunction::fixed(length_percentage(value)),
        TrackBreadth::Flex(value) => TrackSizingFunction::fr(value.0),
        TrackBreadth::Auto => TrackSizingFunction::AUTO,
        TrackBreadth::MinContent => TrackSizingFunction::minmax(
            MinTrackSizingFunction::MinContent,
            MaxTrackSizingFunction::MinContent,
        ),
        TrackBreadth::MaxContent => TrackSizingFunction::minmax(
            MinTrackSizingFunction::MaxContent,
            MaxTrackSizingFunction::MaxContent,
        ),
    }
}

fn min_track_breadth(value: &TrackBreadth<StyloLengthPercentage>) -> MinTrackSizingFunction {
    match value {
        TrackBreadth::Breadth(value) => MinTrackSizingFunction::Fixed(length_percentage(value)),
        TrackBreadth::MinContent => MinTrackSizingFunction::MinContent,
        TrackBreadth::MaxContent => MinTrackSizingFunction::MaxContent,
        TrackBreadth::Auto | TrackBreadth::Flex(_) => MinTrackSizingFunction::Auto,
    }
}

fn max_track_breadth(value: &TrackBreadth<StyloLengthPercentage>) -> MaxTrackSizingFunction {
    match value {
        TrackBreadth::Breadth(value) => MaxTrackSizingFunction::Fixed(length_percentage(value)),
        TrackBreadth::Flex(value) => MaxTrackSizingFunction::Fr(value.0),
        TrackBreadth::MinContent => MaxTrackSizingFunction::MinContent,
        TrackBreadth::MaxContent => MaxTrackSizingFunction::MaxContent,
        TrackBreadth::Auto => MaxTrackSizingFunction::Auto,
    }
}

fn lower_logical_references(
    edges: &mut Edges<RelativeReference>,
    inline_start: RelativeReference,
    inline_end: RelativeReference,
    direction: Direction,
) {
    match direction {
        Direction::Ltr => {
            if !inline_start.is_none() {
                edges.left = inline_start;
            }
            if !inline_end.is_none() {
                edges.right = inline_end;
            }
        }
        Direction::Rtl => {
            if !inline_start.is_none() {
                edges.right = inline_start;
            }
            if !inline_end.is_none() {
                edges.left = inline_end;
            }
        }
    }
}

fn generic_font_family(value: StyloGenericFamily) -> Option<GenericFontFamily> {
    match value {
        StyloGenericFamily::None => None,
        StyloGenericFamily::Serif => Some(GenericFontFamily::Serif),
        StyloGenericFamily::SansSerif => Some(GenericFontFamily::SansSerif),
        StyloGenericFamily::Monospace => Some(GenericFontFamily::Monospace),
        StyloGenericFamily::Cursive => Some(GenericFontFamily::Cursive),
        StyloGenericFamily::Fantasy => Some(GenericFontFamily::Fantasy),
        StyloGenericFamily::SystemUi => Some(GenericFontFamily::SystemUi),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use euclid::{Scale, Size2D};
    use stylo::context::QuirksMode;
    use stylo::device::Device;
    use stylo::device::servo::FontMetricsProvider;
    use stylo::font_metrics::FontMetrics;
    use stylo::media_queries::MediaType;
    use stylo::properties::style_structs::Font;
    use stylo::queries::values::PrefersColorScheme;
    use stylo::servo::media_features::PointerCapabilities;
    use stylo::values::computed::{CSSPixelLength, Length};
    use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};
    use stylo_atoms::Atom;
    use stylo_traits::{CSSPixel, DevicePixel};

    use super::*;
    use crate::{Element, StyleEngine, StylesheetOrigin};

    #[derive(Debug)]
    struct TestFontMetricsProvider;

    impl FontMetricsProvider for TestFontMetricsProvider {
        fn query_font_metrics(
            &self,
            _vertical: bool,
            _font: &Font,
            base_size: CSSPixelLength,
            _flags: QueryFontMetricsFlags,
        ) -> FontMetrics {
            FontMetrics {
                ascent: Length::new(base_size.px()),
                ..FontMetrics::default()
            }
        }

        fn base_size_for_generic(&self, _generic: StyloGenericFamily) -> Length {
            Length::new(FONT_MEDIUM_PX)
        }
    }

    fn device(width: f32, height: f32) -> Device {
        Device::new(
            MediaType::screen(),
            QuirksMode::NoQuirks,
            Size2D::<f32, CSSPixel>::new(width, height),
            Size2D::<f32, DevicePixel>::new(width, height),
            Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
            Box::new(TestFontMetricsProvider),
            ComputedValues::initial_values_with_font_override(Font::initial_values()),
            PrefersColorScheme::Light,
            PointerCapabilities::empty(),
            PointerCapabilities::empty(),
        )
    }

    fn computed(css: &str) -> Arc<ComputedValues> {
        let mut engine = StyleEngine::new(device(750.0, 1334.0));
        engine.add_stylesheet_str(&format!(".target {{ {css} }}"), StylesheetOrigin::Author);
        let mut arena = engine.new_arena();
        let mut element = Element::new("div", ());
        element.classes.push(Atom::from("target"));
        let target = arena.insert(element);
        engine.resolve(arena.element_ref(target).unwrap(), None)
    }

    #[test]
    fn preserves_calc_intrinsic_sizes_and_core_box_values() {
        let computed = computed(
            "display: grid; width: calc(50% - 3px); min-width: min-content; \
             max-height: max-content; margin-left: auto; padding-top: 2%; \
             position: fixed; overflow-x: scroll; box-sizing: border-box; \
             text-indent: calc(10% + 2px);",
        );
        let style = ComputedLayoutStyle::new(&computed);

        assert_eq!(style.layout_display(), LayoutDisplay::Grid);
        assert_eq!(style.position(), Position::AbsoluteHoisted);
        assert_eq!(style.box_sizing(), BoxSizing::BorderBox);
        assert_eq!(style.overflow().x, Overflow::Scroll);
        assert_eq!(style.margin().left, LengthPercentageAuto::Auto);
        assert_eq!(style.padding().top, LengthPercentage::Percent(0.02));
        assert_eq!(style.min_size().width, Dimension::MinContent);
        assert_eq!(style.max_size().height, Dimension::MaxContent);

        let Dimension::Calc(handle) = style.size().width else {
            panic!("width must preserve its calc expression")
        };
        let valid_handles = FxHashSet::from_iter([handle.raw()]);
        assert_eq!(resolve_calc(handle, 100.0, &valid_handles), 47.0);
        let LengthPercentage::Calc(handle) = style.text_indent() else {
            panic!("text-indent must preserve its calc expression")
        };
        let valid_handles = FxHashSet::from_iter([handle.raw()]);
        assert_eq!(resolve_calc(handle, 100.0, &valid_handles), 12.0);
    }

    #[test]
    fn lends_grid_tracks_without_materializing_adapter_vectors() {
        let computed = computed(
            "grid-template-columns: 10px repeat(2, minmax(min-content, 1fr)); \
             grid-auto-rows: fit-content(40%); grid-auto-flow: column dense;",
        );
        let style = ComputedLayoutStyle::new(&computed);
        let mut template = style.grid_template_columns();

        assert_eq!(
            template.next(),
            std::option::Option::Some(GridTemplateComponent::Single(TrackSizingFunction::fixed(
                LengthPercentage::Length(10.0)
            )))
        );
        let std::option::Option::Some(GridTemplateComponent::Repeat(repetition)) = template.next()
        else {
            panic!("second component must remain a repeat group")
        };
        assert_eq!(repetition.count(), RepetitionCount::Count(2));
        assert_eq!(
            repetition.tracks().collect::<Vec<_>>(),
            vec![TrackSizingFunction::minmax(
                MinTrackSizingFunction::MinContent,
                MaxTrackSizingFunction::Fr(1.0),
            )]
        );
        assert_eq!(style.grid_auto_flow(), GridAutoFlow::ColumnDense);
        assert_eq!(
            style.grid_auto_rows().collect::<Vec<_>>(),
            vec![TrackSizingFunction::fit_content(LengthPercentage::Percent(
                0.4
            ))]
        );
    }

    #[test]
    fn text_run_snapshot_lends_font_sequences_and_metrics() {
        let computed = computed(
            "font-family: 'Inter', monospace; font-size: 20px; font-weight: 700; \
             font-style: italic; letter-spacing: 2px; line-height: 1.5; \
             white-space: nowrap; word-break: break-all; \
             font-feature-settings: 'kern' 0, 'liga' 1; \
             font-variation-settings: 'wght' 650;",
        );
        let style = ComputedTextRunStyle::new(computed);

        assert_eq!(style.font_size(), 20.0);
        assert_eq!(style.font_weight().value(), 700);
        assert_eq!(style.font_style(), FontStyle::Italic);
        assert_eq!(style.letter_spacing(), 2.0);
        assert_eq!(style.line_height(), LineHeight::Factor(1.5));
        assert_eq!(TextRunStyle::white_space(&style), Some(WhiteSpace::NoWrap));
        assert_eq!(TextRunStyle::word_break(&style), Some(WordBreak::BreakAll));
        assert_eq!(
            style.font_families().collect::<Vec<_>>(),
            vec![
                FontFamily::Named("Inter"),
                FontFamily::Generic(GenericFontFamily::Monospace),
            ]
        );
        assert_eq!(
            style.font_feature_settings().collect::<Vec<_>>(),
            vec![(*b"kern", 0), (*b"liga", 1)]
        );
        assert_eq!(
            style.font_variation_settings().collect::<Vec<_>>(),
            vec![(*b"wght", 650.0)]
        );
    }

    #[test]
    fn static_insets_do_not_become_relative_offsets() {
        let computed = computed("position: static; left: 10px; top: 20px;");
        let style = ComputedLayoutStyle::new(&computed);

        assert_eq!(style.position(), Position::Relative);
        assert_eq!(style.inset(), Edges::uniform(LengthPercentageAuto::Auto));
    }

    #[test]
    fn distinguishes_standard_flow_contents_and_lynx_layout_modes() {
        let parsed_display = |css| {
            let computed = computed(css);
            ComputedLayoutStyle::new(&computed).layout_display()
        };

        assert_eq!(parsed_display("display: none"), LayoutDisplay::None);
        // Lynx's author grammar intentionally rejects `contents`; Stylo still
        // carries the value internally for UA/formatting-tree projections.
        assert_eq!(classify_display(Display::Contents), LayoutDisplay::Contents);
        assert_eq!(parsed_display("display: block"), LayoutDisplay::Flow);
        assert_eq!(parsed_display("display: flex"), LayoutDisplay::Flex);
        assert_eq!(parsed_display("display: grid"), LayoutDisplay::Grid);
        assert_eq!(parsed_display("display: linear"), LayoutDisplay::Linear);
        assert_eq!(parsed_display("display: relative"), LayoutDisplay::Relative);
    }
}
