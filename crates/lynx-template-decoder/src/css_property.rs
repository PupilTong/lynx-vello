//! CSS property model of the `StyleInfo` section.
//!
//! These types mirror
//! `packages/web-platform/web-core/src/template/template_sections/style_info/css_property.rs`
//! in lynx-stack **exactly** — field order and enum variant order define the
//! rkyv wire format. Do not reorder anything here.

use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};

/// Canonical property names, indexed by [`CssPropertyId`] discriminant.
/// Mirrors `STYLE_PROPERTY_MAP` in web-core (index 0 is the Unknown slot).
pub const STYLE_PROPERTY_MAP: &[&str] = &[
    "",
    "top",
    "left",
    "right",
    "bottom",
    "position",
    "box-sizing",
    "background-color",
    "border-left-color",
    "border-right-color",
    "border-top-color",
    "border-bottom-color",
    "border-radius",
    "border-top-left-radius",
    "border-bottom-left-radius",
    "border-top-right-radius",
    "border-bottom-right-radius",
    "border-width",
    "border-left-width",
    "border-right-width",
    "border-top-width",
    "border-bottom-width",
    "color",
    "opacity",
    "display",
    "overflow",
    "height",
    "width",
    "max-width",
    "min-width",
    "max-height",
    "min-height",
    "padding",
    "padding-left",
    "padding-right",
    "padding-top",
    "padding-bottom",
    "margin",
    "margin-left",
    "margin-right",
    "margin-top",
    "margin-bottom",
    "white-space",
    "letter-spacing",
    "text-align",
    "line-height",
    "text-overflow",
    "font-size",
    "font-weight",
    "flex",
    "flex-grow",
    "flex-shrink",
    "flex-basis",
    "flex-direction",
    "flex-wrap",
    "align-items",
    "align-self",
    "align-content",
    "justify-content",
    "background",
    "border-color",
    "font-family",
    "font-style",
    "transform",
    "animation",
    "animation-name",
    "animation-duration",
    "animation-timing-function",
    "animation-delay",
    "animation-iteration-count",
    "animation-direction",
    "animation-fill-mode",
    "animation-play-state",
    "line-spacing",
    "border-style",
    "order",
    "box-shadow",
    "transform-origin",
    "linear-orientation",
    "linear-weight-sum",
    "linear-weight",
    "linear-gravity",
    "linear-layout-gravity",
    "layout-animation-create-duration",
    "layout-animation-create-timing-function",
    "layout-animation-create-delay",
    "layout-animation-create-property",
    "layout-animation-delete-duration",
    "layout-animation-delete-timing-function",
    "layout-animation-delete-delay",
    "layout-animation-delete-property",
    "layout-animation-update-duration",
    "layout-animation-update-timing-function",
    "layout-animation-update-delay",
    "adapt-font-size",
    "aspect-ratio",
    "text-decoration",
    "text-shadow",
    "background-image",
    "background-position",
    "background-origin",
    "background-repeat",
    "background-size",
    "border",
    "visibility",
    "border-right",
    "border-left",
    "border-top",
    "border-bottom",
    "transition",
    "transition-property",
    "transition-duration",
    "transition-delay",
    "transition-timing-function",
    "content",
    "border-left-style",
    "border-right-style",
    "border-top-style",
    "border-bottom-style",
    "implicit-animation",
    "overflow-x",
    "overflow-y",
    "word-break",
    "background-clip",
    "outline",
    "outline-color",
    "outline-style",
    "outline-width",
    "vertical-align",
    "caret-color",
    "direction",
    "relative-id",
    "relative-align-top",
    "relative-align-right",
    "relative-align-bottom",
    "relative-align-left",
    "relative-top-of",
    "relative-right-of",
    "relative-bottom-of",
    "relative-left-of",
    "relative-layout-once",
    "relative-center",
    "enter-transition-name",
    "exit-transition-name",
    "pause-transition-name",
    "resume-transition-name",
    "flex-flow",
    "z-index",
    "text-decoration-color",
    "linear-cross-gravity",
    "margin-inline-start",
    "margin-inline-end",
    "padding-inline-start",
    "padding-inline-end",
    "border-inline-start-color",
    "border-inline-end-color",
    "border-inline-start-width",
    "border-inline-end-width",
    "border-inline-start-style",
    "border-inline-end-style",
    "border-start-start-radius",
    "border-end-start-radius",
    "border-start-end-radius",
    "border-end-end-radius",
    "relative-align-inline-start",
    "relative-align-inline-end",
    "relative-inline-start-of",
    "relative-inline-end-of",
    "inset-inline-start",
    "inset-inline-end",
    "mask-image",
    "grid-template-columns",
    "grid-template-rows",
    "grid-auto-columns",
    "grid-auto-rows",
    "grid-column-span",
    "grid-row-span",
    "grid-column-start",
    "grid-column-end",
    "grid-row-start",
    "grid-row-end",
    "grid-column-gap",
    "grid-row-gap",
    "justify-items",
    "justify-self",
    "grid-auto-flow",
    "filter",
    "list-main-axis-gap",
    "list-cross-axis-gap",
    "linear-direction",
    "perspective",
    "cursor",
    "text-indent",
    "clip-path",
    "text-stroke",
    "text-stroke-width",
    "text-stroke-color",
    "-x-auto-font-size",
    "-x-auto-font-size-preset-sizes",
    "mask",
    "mask-repeat",
    "mask-position",
    "mask-clip",
    "mask-origin",
    "mask-size",
    "gap",
    "column-gap",
    "row-gap",
    "image-rendering",
    "hyphens",
    "-x-app-region",
    "-x-animation-color-interpolation",
    "-x-handle-color",
    "-x-handle-size",
    "offset-path",
    "offset-distance",
];

/// Interned CSS property id. Mirrors web-core's `CSSPropertyEnum`.
#[expect(missing_docs, reason = "216 self-describing CSS property variants")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
#[repr(u32)]
pub enum CssPropertyId {
    Unknown = 0,
    Top = 1,
    Left = 2,
    Right = 3,
    Bottom = 4,
    Position = 5,
    BoxSizing = 6,
    BackgroundColor = 7,
    BorderLeftColor = 8,
    BorderRightColor = 9,
    BorderTopColor = 10,
    BorderBottomColor = 11,
    BorderRadius = 12,
    BorderTopLeftRadius = 13,
    BorderBottomLeftRadius = 14,
    BorderTopRightRadius = 15,
    BorderBottomRightRadius = 16,
    BorderWidth = 17,
    BorderLeftWidth = 18,
    BorderRightWidth = 19,
    BorderTopWidth = 20,
    BorderBottomWidth = 21,
    Color = 22,
    Opacity = 23,
    Display = 24,
    Overflow = 25,
    Height = 26,
    Width = 27,
    MaxWidth = 28,
    MinWidth = 29,
    MaxHeight = 30,
    MinHeight = 31,
    Padding = 32,
    PaddingLeft = 33,
    PaddingRight = 34,
    PaddingTop = 35,
    PaddingBottom = 36,
    Margin = 37,
    MarginLeft = 38,
    MarginRight = 39,
    MarginTop = 40,
    MarginBottom = 41,
    WhiteSpace = 42,
    LetterSpacing = 43,
    TextAlign = 44,
    LineHeight = 45,
    TextOverflow = 46,
    FontSize = 47,
    FontWeight = 48,
    Flex = 49,
    FlexGrow = 50,
    FlexShrink = 51,
    FlexBasis = 52,
    FlexDirection = 53,
    FlexWrap = 54,
    AlignItems = 55,
    AlignSelf = 56,
    AlignContent = 57,
    JustifyContent = 58,
    Background = 59,
    BorderColor = 60,
    FontFamily = 61,
    FontStyle = 62,
    Transform = 63,
    Animation = 64,
    AnimationName = 65,
    AnimationDuration = 66,
    AnimationTimingFunction = 67,
    AnimationDelay = 68,
    AnimationIterationCount = 69,
    AnimationDirection = 70,
    AnimationFillMode = 71,
    AnimationPlayState = 72,
    LineSpacing = 73,
    BorderStyle = 74,
    Order = 75,
    BoxShadow = 76,
    TransformOrigin = 77,
    LinearOrientation = 78,
    LinearWeightSum = 79,
    LinearWeight = 80,
    LinearGravity = 81,
    LinearLayoutGravity = 82,
    LayoutAnimationCreateDuration = 83,
    LayoutAnimationCreateTimingFunction = 84,
    LayoutAnimationCreateDelay = 85,
    LayoutAnimationCreateProperty = 86,
    LayoutAnimationDeleteDuration = 87,
    LayoutAnimationDeleteTimingFunction = 88,
    LayoutAnimationDeleteDelay = 89,
    LayoutAnimationDeleteProperty = 90,
    LayoutAnimationUpdateDuration = 91,
    LayoutAnimationUpdateTimingFunction = 92,
    LayoutAnimationUpdateDelay = 93,
    AdaptFontSize = 94,
    AspectRatio = 95,
    TextDecoration = 96,
    TextShadow = 97,
    BackgroundImage = 98,
    BackgroundPosition = 99,
    BackgroundOrigin = 100,
    BackgroundRepeat = 101,
    BackgroundSize = 102,
    Border = 103,
    Visibility = 104,
    BorderRight = 105,
    BorderLeft = 106,
    BorderTop = 107,
    BorderBottom = 108,
    Transition = 109,
    TransitionProperty = 110,
    TransitionDuration = 111,
    TransitionDelay = 112,
    TransitionTimingFunction = 113,
    Content = 114,
    BorderLeftStyle = 115,
    BorderRightStyle = 116,
    BorderTopStyle = 117,
    BorderBottomStyle = 118,
    ImplicitAnimation = 119,
    OverflowX = 120,
    OverflowY = 121,
    WordBreak = 122,
    BackgroundClip = 123,
    Outline = 124,
    OutlineColor = 125,
    OutlineStyle = 126,
    OutlineWidth = 127,
    VerticalAlign = 128,
    CaretColor = 129,
    Direction = 130,
    RelativeId = 131,
    RelativeAlignTop = 132,
    RelativeAlignRight = 133,
    RelativeAlignBottom = 134,
    RelativeAlignLeft = 135,
    RelativeToTop = 136,
    RelativeToRight = 137,
    RelativeToBottom = 138,
    RelativeToLeft = 139,
    RelativeToLayoutOnce = 140,
    RelativeToCenter = 141,
    EnterTransitionName = 142,
    ExitTransitionName = 143,
    PauseTransitionName = 144,
    ResumeTransitionName = 145,
    FlexFlow = 146,
    ZIndex = 147,
    TextDecorationColor = 148,
    LinearCrossGravity = 149,
    MarginInlineStart = 150,
    MarginInlineEnd = 151,
    PaddingInlineStart = 152,
    PaddingInlineEnd = 153,
    BorderInlineStartColor = 154,
    BorderInlineEndColor = 155,
    BorderInlineStartWidth = 156,
    BorderInlineEndWidth = 157,
    BorderInlineStartStyle = 158,
    BorderInlineEndStyle = 159,
    BorderStartStartRadius = 160,
    BorderEndStartRadius = 161,
    BorderStartEndRadius = 162,
    BorderEndEndRadius = 163,
    RelativeToAlignInlineStart = 164,
    RelativeToAlignInlineEnd = 165,
    RelativeToInlineStartOf = 166,
    RelativeToInlineEndOf = 167,
    InsetInlineStart = 168,
    InsetInlineEnd = 169,
    MaskImage = 170,
    GridTemplateColumns = 171,
    GridTemplateRows = 172,
    GridAutoColumns = 173,
    GridAutoRows = 174,
    GridColumnSpan = 175,
    GridRowSpan = 176,
    GridColumnStart = 177,
    GridColumnEnd = 178,
    GridRowStart = 179,
    GridRowEnd = 180,
    GridColumnGap = 181,
    GridRowGap = 182,
    JustifyItems = 183,
    JustifySelf = 184,
    GridAutoFlow = 185,
    Filter = 186,
    ListMainAxisGap = 187,
    ListCrossAxisGap = 188,
    LinearDirection = 189,
    Perspective = 190,
    Cursor = 191,
    TextIndent = 192,
    ClipPath = 193,
    TextStroke = 194,
    TextStrokeWidth = 195,
    TextStrokeColor = 196,
    XAutoFontSize = 197,
    XAutoFontSizePresetSizes = 198,
    Mask = 199,
    MaskRepeat = 200,
    MaskPosition = 201,
    MaskClip = 202,
    MaskOrigin = 203,
    MaskSize = 204,
    Gap = 205,
    ColumnGap = 206,
    RowGap = 207,
    ImageRendering = 208,
    Hyphens = 209,
    XAppRegion = 210,
    XAnimationColorInterpolation = 211,
    XHandleColor = 212,
    XHandleSize = 213,
    OffsetPath = 214,
    OffsetDistance = 215,
}

impl CssPropertyId {
    /// The canonical hyphenated property name, or `""` for [`Self::Unknown`].
    #[must_use]
    pub fn name(self) -> &'static str {
        STYLE_PROPERTY_MAP[self as u32 as usize]
    }

    /// Converts a wire/property-table discriminant to its enum value.
    ///
    /// The web `CSSPropertyEnum` is intentionally contiguous because its
    /// discriminants index [`STYLE_PROPERTY_MAP`].
    #[must_use]
    #[expect(
        unsafe_code,
        reason = "the range check covers every value of the contiguous repr(u32) wire enum"
    )]
    pub fn from_u32(id: u32) -> Option<Self> {
        if id <= Self::OffsetDistance as u32 {
            // SAFETY: every integer from Unknown (0) through OffsetDistance
            // (215) is an explicitly declared discriminant of this repr(u32)
            // enum, with no gaps.
            Some(unsafe { std::mem::transmute::<u32, Self>(id) })
        } else {
            None
        }
    }
}

/// A CSS property reference: an interned id, or `Unknown` plus the raw name.
/// Mirrors web-core's `CSSProperty`.
#[derive(Debug, Clone, PartialEq, Eq, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct CssProperty {
    /// Interned property id.
    pub id: CssPropertyId,
    /// Raw property name; `Some` only when `id` is [`CssPropertyId::Unknown`].
    pub unknown_name: Option<String>,
}

impl CssProperty {
    /// Builds a property reference from its canonical or custom name.
    #[must_use]
    pub fn from_name(name: impl Into<String>) -> Self {
        let name = name.into();
        let id = STYLE_PROPERTY_MAP
            .iter()
            .position(|candidate| *candidate == name.as_str())
            .and_then(|index| u32::try_from(index).ok())
            .and_then(CssPropertyId::from_u32)
            .unwrap_or(CssPropertyId::Unknown);
        Self {
            unknown_name: (id == CssPropertyId::Unknown).then_some(name),
            id,
        }
    }

    /// Builds an interned property reference from its numeric wire id.
    #[must_use]
    pub fn from_u32(id: u32) -> Option<Self> {
        CssPropertyId::from_u32(id).map(|id| Self {
            id,
            unknown_name: None,
        })
    }

    /// The property name as written in the source CSS.
    #[must_use]
    pub fn name(&self) -> &str {
        if self.id == CssPropertyId::Unknown {
            self.unknown_name.as_deref().unwrap_or("")
        } else {
            self.id.name()
        }
    }
}

/// One token of a declaration value, as produced by web-core's CSS tokenizer.
/// Mirrors web-core's `ValueToken`.
#[derive(Debug, Clone, PartialEq, Eq, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct ValueToken {
    /// Token type; see [`token_types`].
    pub token_type: u8,
    /// Raw token text (whitespace tokens carry the whitespace itself).
    pub value: String,
}

/// A single `property: value` declaration. Mirrors web-core's
/// `ParsedDeclaration`.
#[derive(Debug, Clone, PartialEq, Eq, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct ParsedDeclaration {
    /// The property being set.
    pub property_id: CssProperty,
    /// The declaration value, tokenized. Concatenating the token values
    /// reproduces the source text (whitespace is preserved as tokens).
    pub value_token_list: Vec<ValueToken>,
    /// Always `false` in practice — Lynx does not support `!important`.
    pub is_important: bool,
}

impl ParsedDeclaration {
    /// Reassembles the declaration value text from its tokens.
    #[must_use]
    pub fn value_text(&self) -> String {
        self.value_token_list
            .iter()
            .map(|token| token.value.as_str())
            .collect()
    }
}

/// CSS token type constants, mirroring web-core's `css_tokenizer/token_types.rs`.
pub mod token_types {
    #![expect(missing_docs, reason = "names follow the CSS Syntax spec")]
    pub const EOF_TOKEN: u8 = 0;
    pub const IDENT_TOKEN: u8 = 1;
    pub const FUNCTION_TOKEN: u8 = 2;
    pub const AT_KEYWORD_TOKEN: u8 = 3;
    pub const HASH_TOKEN: u8 = 4;
    pub const STRING_TOKEN: u8 = 5;
    pub const BAD_STRING_TOKEN: u8 = 6;
    pub const URL_TOKEN: u8 = 7;
    pub const BAD_URL_TOKEN: u8 = 8;
    pub const DELIM_TOKEN: u8 = 9;
    pub const NUMBER_TOKEN: u8 = 10;
    pub const PERCENTAGE_TOKEN: u8 = 11;
    pub const DIMENSION_TOKEN: u8 = 12;
    pub const WHITESPACE_TOKEN: u8 = 13;
    pub const CDO_TOKEN: u8 = 14;
    pub const CDC_TOKEN: u8 = 15;
    pub const COLON_TOKEN: u8 = 16;
    pub const SEMICOLON_TOKEN: u8 = 17;
    pub const COMMA_TOKEN: u8 = 18;
    pub const LEFT_SQUARE_BRACKET_TOKEN: u8 = 19;
    pub const RIGHT_SQUARE_BRACKET_TOKEN: u8 = 20;
    pub const LEFT_PARENTHESES_TOKEN: u8 = 21;
    pub const RIGHT_PARENTHESES_TOKEN: u8 = 22;
    pub const LEFT_CURLY_BRACKET_TOKEN: u8 = 23;
    pub const RIGHT_CURLY_BRACKET_TOKEN: u8 = 24;
    pub const COMMENT_TOKEN: u8 = 25;
}
