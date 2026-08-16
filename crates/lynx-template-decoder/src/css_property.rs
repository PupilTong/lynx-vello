//! rkyv 0.7 wire model for `StyleInfo` CSS properties; declaration and enum
//! ordering is serialized ABI.

use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};

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
    RelativeTopOf = 136,
    RelativeRightOf = 137,
    RelativeBottomOf = 138,
    RelativeLeftOf = 139,
    RelativeLayoutOnce = 140,
    RelativeCenter = 141,
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
    RelativeAlignInlineStart = 164,
    RelativeAlignInlineEnd = 165,
    RelativeInlineStartOf = 166,
    RelativeInlineEndOf = 167,
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
    #[must_use]
    pub fn name(self) -> &'static str {
        STYLE_PROPERTY_MAP[self as u32 as usize]
    }
}

/// A CSS property reference: an interned id, or `Unknown` plus the raw name.
/// Mirrors web-core's `CSSProperty`.
#[derive(Debug, Clone, PartialEq, Eq, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct CssProperty {
    pub id: CssPropertyId,
    pub unknown_name: Option<String>,
}

impl CssProperty {
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
    pub token_type: u8,
    pub value: String,
}

/// A single `property: value` declaration. Mirrors web-core's
/// `ParsedDeclaration`.
#[derive(Debug, Clone, PartialEq, Eq, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct ParsedDeclaration {
    pub property: CssProperty,
    pub value_tokens: Vec<ValueToken>,
    pub is_important: bool,
}

impl ParsedDeclaration {
    /// The declaration's value, exactly as authored.
    ///
    /// Value tokens partition the original value text without loss — they
    /// carry their own delimiters, quotes and separating whitespace — so
    /// concatenating them reproduces it byte for byte. That includes a
    /// trailing `!important`; see [`Self::value_and_importance`].
    #[must_use]
    pub fn value_text(&self) -> String {
        let mut text = String::with_capacity(self.value_text_len());
        self.write_value_text(&mut text);
        text
    }

    /// Appends [`Self::value_text`] to `out`, without allocating a temporary.
    pub fn write_value_text(&self, out: &mut String) {
        out.reserve(self.value_text_len());
        for token in &self.value_tokens {
            out.push_str(&token.value);
        }
    }

    /// The value with any trailing `!important` removed, plus whether the
    /// declaration carries one.
    ///
    /// [`Self::is_important`] alone is not enough. The producing toolchain
    /// leaves that field `false` and appends ` !important` to the *value*
    /// instead (`@lynx-js/css-serializer` builds the value as
    /// `toString(node.value) + (node.important ? ' !important' : '')`), so on
    /// the wire the marker arrives as ordinary value tokens. A consumer that
    /// hands the raw value to a CSS value parser does not merely lose the
    /// importance: the marker is not part of any value grammar, so the whole
    /// declaration fails to parse and is dropped.
    ///
    /// The match is on the token sequence — an `important` ident preceded by a
    /// `!` delimiter, either side of which may carry whitespace or comments —
    /// so a value that merely ends in the ident `important`, or in a string
    /// containing the word, is left alone.
    #[must_use]
    pub fn value_and_importance(&self) -> (String, bool) {
        let end = self.value_end_before_important();
        let important = end < self.value_tokens.len();
        let mut text = String::with_capacity(self.value_text_len());
        for token in &self.value_tokens[..end] {
            text.push_str(&token.value);
        }
        (text, important || self.is_important)
    }

    fn value_text_len(&self) -> usize {
        self.value_tokens
            .iter()
            .map(|token| token.value.len())
            .sum()
    }

    /// The token count remaining once a trailing `!important` is removed, or
    /// the full count when there is none.
    fn value_end_before_important(&self) -> usize {
        let significant = |index: usize| {
            self.value_tokens[..index].iter().rposition(|token| {
                !matches!(
                    token.token_type,
                    token_types::WHITESPACE_TOKEN | token_types::COMMENT_TOKEN
                )
            })
        };
        let all = self.value_tokens.len();
        let Some(keyword) = significant(all) else {
            return all;
        };
        let token = &self.value_tokens[keyword];
        if token.token_type != token_types::IDENT_TOKEN
            || !token.value.eq_ignore_ascii_case("important")
        {
            return all;
        }
        let Some(bang) = significant(keyword) else {
            return all;
        };
        let token = &self.value_tokens[bang];
        if token.token_type != token_types::DELIM_TOKEN || token.value != "!" {
            return all;
        }
        // Drop the whitespace that separated the marker from the value too.
        significant(bang).map_or(0, |last| last + 1)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn declaration(tokens: &[(u8, &str)]) -> ParsedDeclaration {
        ParsedDeclaration {
            property: CssProperty {
                id: CssPropertyId::Color,
                unknown_name: None,
            },
            value_tokens: tokens
                .iter()
                .map(|(token_type, value)| ValueToken {
                    token_type: *token_type,
                    value: (*value).to_owned(),
                })
                .collect(),
            is_important: false,
        }
    }

    const WS: u8 = token_types::WHITESPACE_TOKEN;
    const IDENT: u8 = token_types::IDENT_TOKEN;
    const BANG: u8 = token_types::DELIM_TOKEN;
    const STRING: u8 = token_types::STRING_TOKEN;
    const COMMENT: u8 = token_types::COMMENT_TOKEN;

    #[test]
    fn a_trailing_important_marker_is_split_off_the_value() {
        let declaration =
            declaration(&[(IDENT, "red"), (WS, " "), (BANG, "!"), (IDENT, "important")]);
        assert_eq!(declaration.value_text(), "red !important");
        assert_eq!(declaration.value_and_importance(), ("red".to_owned(), true));
    }

    #[test]
    fn the_marker_is_recognized_without_whitespace_and_in_any_case() {
        for tokens in [
            vec![(IDENT, "red"), (BANG, "!"), (IDENT, "important")],
            vec![(IDENT, "red"), (WS, " "), (BANG, "!"), (IDENT, "IMPORTANT")],
            vec![
                (IDENT, "red"),
                (WS, " "),
                (BANG, "!"),
                (WS, " "),
                (IDENT, "Important"),
            ],
            vec![
                (IDENT, "red"),
                (COMMENT, "/* x */"),
                (BANG, "!"),
                (IDENT, "important"),
            ],
        ] {
            assert_eq!(
                declaration(&tokens).value_and_importance(),
                ("red".to_owned(), true),
                "{tokens:?}"
            );
        }
    }

    #[test]
    fn a_value_that_only_looks_like_the_marker_is_left_alone() {
        // The `!` delimiter is required, so a bare keyword is a value...
        let keyword = declaration(&[(IDENT, "important")]);
        assert_eq!(
            keyword.value_and_importance(),
            ("important".to_owned(), false)
        );

        // ...as is the word inside a string, which keeps its quotes.
        let quoted = declaration(&[(STRING, "\"x !important\"")]);
        assert_eq!(
            quoted.value_and_importance(),
            ("\"x !important\"".to_owned(), false)
        );
    }

    /// CSS whitespace is only U+0009/000A/000C/000D/0020. A code point such as
    /// U+00A0 is an ident code point, so it belongs to the value and must not
    /// be trimmed away — matching the token boundaries is what keeps that
    /// distinction, which trimming the concatenated text would lose.
    #[test]
    fn a_non_ascii_space_stays_part_of_the_value() {
        let declaration = declaration(&[
            (IDENT, "red\u{a0}"),
            (WS, " "),
            (BANG, "!"),
            (IDENT, "important"),
        ]);
        assert_eq!(
            declaration.value_and_importance(),
            ("red\u{a0}".to_owned(), true)
        );
    }

    #[test]
    fn the_wire_flag_is_still_honored_when_it_is_set() {
        let mut declaration = declaration(&[(IDENT, "red")]);
        declaration.is_important = true;
        assert_eq!(declaration.value_and_importance(), ("red".to_owned(), true));
    }

    #[test]
    fn a_value_that_is_only_a_marker_leaves_an_empty_value() {
        let declaration = declaration(&[(BANG, "!"), (IDENT, "important")]);
        assert_eq!(declaration.value_and_importance(), (String::new(), true));
    }
}
