//! rkyv 0.7 wire model for `StyleInfo` CSS properties; declaration and enum
//! ordering is serialized ABI.

use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};

/// Declares the `StyleInfo` property-id wire enum together with the
/// id-indexed CSS name table, so the two stay one ordered list. Declaration
/// order is serialized ABI.
macro_rules! css_properties {
    ($($variant:ident = $id:literal => $css:literal,)*) => {
        #[expect(missing_docs, reason = "self-describing CSS property variants")]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, Archive, RkyvDeserialize, RkyvSerialize,
        )]
        #[archive(check_bytes)]
        #[archive_attr(derive(Debug))]
        #[repr(u32)]
        pub enum CssPropertyId {
            $($variant = $id,)*
        }

        /// CSS property names indexed by [`CssPropertyId`].
        pub const STYLE_PROPERTY_MAP: &[&str] = &[$($css,)*];
    };
}

css_properties! {
    Unknown = 0 => "",
    Top = 1 => "top",
    Left = 2 => "left",
    Right = 3 => "right",
    Bottom = 4 => "bottom",
    Position = 5 => "position",
    BoxSizing = 6 => "box-sizing",
    BackgroundColor = 7 => "background-color",
    BorderLeftColor = 8 => "border-left-color",
    BorderRightColor = 9 => "border-right-color",
    BorderTopColor = 10 => "border-top-color",
    BorderBottomColor = 11 => "border-bottom-color",
    BorderRadius = 12 => "border-radius",
    BorderTopLeftRadius = 13 => "border-top-left-radius",
    BorderBottomLeftRadius = 14 => "border-bottom-left-radius",
    BorderTopRightRadius = 15 => "border-top-right-radius",
    BorderBottomRightRadius = 16 => "border-bottom-right-radius",
    BorderWidth = 17 => "border-width",
    BorderLeftWidth = 18 => "border-left-width",
    BorderRightWidth = 19 => "border-right-width",
    BorderTopWidth = 20 => "border-top-width",
    BorderBottomWidth = 21 => "border-bottom-width",
    Color = 22 => "color",
    Opacity = 23 => "opacity",
    Display = 24 => "display",
    Overflow = 25 => "overflow",
    Height = 26 => "height",
    Width = 27 => "width",
    MaxWidth = 28 => "max-width",
    MinWidth = 29 => "min-width",
    MaxHeight = 30 => "max-height",
    MinHeight = 31 => "min-height",
    Padding = 32 => "padding",
    PaddingLeft = 33 => "padding-left",
    PaddingRight = 34 => "padding-right",
    PaddingTop = 35 => "padding-top",
    PaddingBottom = 36 => "padding-bottom",
    Margin = 37 => "margin",
    MarginLeft = 38 => "margin-left",
    MarginRight = 39 => "margin-right",
    MarginTop = 40 => "margin-top",
    MarginBottom = 41 => "margin-bottom",
    WhiteSpace = 42 => "white-space",
    LetterSpacing = 43 => "letter-spacing",
    TextAlign = 44 => "text-align",
    LineHeight = 45 => "line-height",
    TextOverflow = 46 => "text-overflow",
    FontSize = 47 => "font-size",
    FontWeight = 48 => "font-weight",
    Flex = 49 => "flex",
    FlexGrow = 50 => "flex-grow",
    FlexShrink = 51 => "flex-shrink",
    FlexBasis = 52 => "flex-basis",
    FlexDirection = 53 => "flex-direction",
    FlexWrap = 54 => "flex-wrap",
    AlignItems = 55 => "align-items",
    AlignSelf = 56 => "align-self",
    AlignContent = 57 => "align-content",
    JustifyContent = 58 => "justify-content",
    Background = 59 => "background",
    BorderColor = 60 => "border-color",
    FontFamily = 61 => "font-family",
    FontStyle = 62 => "font-style",
    Transform = 63 => "transform",
    Animation = 64 => "animation",
    AnimationName = 65 => "animation-name",
    AnimationDuration = 66 => "animation-duration",
    AnimationTimingFunction = 67 => "animation-timing-function",
    AnimationDelay = 68 => "animation-delay",
    AnimationIterationCount = 69 => "animation-iteration-count",
    AnimationDirection = 70 => "animation-direction",
    AnimationFillMode = 71 => "animation-fill-mode",
    AnimationPlayState = 72 => "animation-play-state",
    LineSpacing = 73 => "line-spacing",
    BorderStyle = 74 => "border-style",
    Order = 75 => "order",
    BoxShadow = 76 => "box-shadow",
    TransformOrigin = 77 => "transform-origin",
    LinearOrientation = 78 => "linear-orientation",
    LinearWeightSum = 79 => "linear-weight-sum",
    LinearWeight = 80 => "linear-weight",
    LinearGravity = 81 => "linear-gravity",
    LinearLayoutGravity = 82 => "linear-layout-gravity",
    LayoutAnimationCreateDuration = 83 => "layout-animation-create-duration",
    LayoutAnimationCreateTimingFunction = 84 => "layout-animation-create-timing-function",
    LayoutAnimationCreateDelay = 85 => "layout-animation-create-delay",
    LayoutAnimationCreateProperty = 86 => "layout-animation-create-property",
    LayoutAnimationDeleteDuration = 87 => "layout-animation-delete-duration",
    LayoutAnimationDeleteTimingFunction = 88 => "layout-animation-delete-timing-function",
    LayoutAnimationDeleteDelay = 89 => "layout-animation-delete-delay",
    LayoutAnimationDeleteProperty = 90 => "layout-animation-delete-property",
    LayoutAnimationUpdateDuration = 91 => "layout-animation-update-duration",
    LayoutAnimationUpdateTimingFunction = 92 => "layout-animation-update-timing-function",
    LayoutAnimationUpdateDelay = 93 => "layout-animation-update-delay",
    AdaptFontSize = 94 => "adapt-font-size",
    AspectRatio = 95 => "aspect-ratio",
    TextDecoration = 96 => "text-decoration",
    TextShadow = 97 => "text-shadow",
    BackgroundImage = 98 => "background-image",
    BackgroundPosition = 99 => "background-position",
    BackgroundOrigin = 100 => "background-origin",
    BackgroundRepeat = 101 => "background-repeat",
    BackgroundSize = 102 => "background-size",
    Border = 103 => "border",
    Visibility = 104 => "visibility",
    BorderRight = 105 => "border-right",
    BorderLeft = 106 => "border-left",
    BorderTop = 107 => "border-top",
    BorderBottom = 108 => "border-bottom",
    Transition = 109 => "transition",
    TransitionProperty = 110 => "transition-property",
    TransitionDuration = 111 => "transition-duration",
    TransitionDelay = 112 => "transition-delay",
    TransitionTimingFunction = 113 => "transition-timing-function",
    Content = 114 => "content",
    BorderLeftStyle = 115 => "border-left-style",
    BorderRightStyle = 116 => "border-right-style",
    BorderTopStyle = 117 => "border-top-style",
    BorderBottomStyle = 118 => "border-bottom-style",
    ImplicitAnimation = 119 => "implicit-animation",
    OverflowX = 120 => "overflow-x",
    OverflowY = 121 => "overflow-y",
    WordBreak = 122 => "word-break",
    BackgroundClip = 123 => "background-clip",
    Outline = 124 => "outline",
    OutlineColor = 125 => "outline-color",
    OutlineStyle = 126 => "outline-style",
    OutlineWidth = 127 => "outline-width",
    VerticalAlign = 128 => "vertical-align",
    CaretColor = 129 => "caret-color",
    Direction = 130 => "direction",
    RelativeId = 131 => "relative-id",
    RelativeAlignTop = 132 => "relative-align-top",
    RelativeAlignRight = 133 => "relative-align-right",
    RelativeAlignBottom = 134 => "relative-align-bottom",
    RelativeAlignLeft = 135 => "relative-align-left",
    RelativeTopOf = 136 => "relative-top-of",
    RelativeRightOf = 137 => "relative-right-of",
    RelativeBottomOf = 138 => "relative-bottom-of",
    RelativeLeftOf = 139 => "relative-left-of",
    RelativeLayoutOnce = 140 => "relative-layout-once",
    RelativeCenter = 141 => "relative-center",
    EnterTransitionName = 142 => "enter-transition-name",
    ExitTransitionName = 143 => "exit-transition-name",
    PauseTransitionName = 144 => "pause-transition-name",
    ResumeTransitionName = 145 => "resume-transition-name",
    FlexFlow = 146 => "flex-flow",
    ZIndex = 147 => "z-index",
    TextDecorationColor = 148 => "text-decoration-color",
    LinearCrossGravity = 149 => "linear-cross-gravity",
    MarginInlineStart = 150 => "margin-inline-start",
    MarginInlineEnd = 151 => "margin-inline-end",
    PaddingInlineStart = 152 => "padding-inline-start",
    PaddingInlineEnd = 153 => "padding-inline-end",
    BorderInlineStartColor = 154 => "border-inline-start-color",
    BorderInlineEndColor = 155 => "border-inline-end-color",
    BorderInlineStartWidth = 156 => "border-inline-start-width",
    BorderInlineEndWidth = 157 => "border-inline-end-width",
    BorderInlineStartStyle = 158 => "border-inline-start-style",
    BorderInlineEndStyle = 159 => "border-inline-end-style",
    BorderStartStartRadius = 160 => "border-start-start-radius",
    BorderEndStartRadius = 161 => "border-end-start-radius",
    BorderStartEndRadius = 162 => "border-start-end-radius",
    BorderEndEndRadius = 163 => "border-end-end-radius",
    RelativeAlignInlineStart = 164 => "relative-align-inline-start",
    RelativeAlignInlineEnd = 165 => "relative-align-inline-end",
    RelativeInlineStartOf = 166 => "relative-inline-start-of",
    RelativeInlineEndOf = 167 => "relative-inline-end-of",
    InsetInlineStart = 168 => "inset-inline-start",
    InsetInlineEnd = 169 => "inset-inline-end",
    MaskImage = 170 => "mask-image",
    GridTemplateColumns = 171 => "grid-template-columns",
    GridTemplateRows = 172 => "grid-template-rows",
    GridAutoColumns = 173 => "grid-auto-columns",
    GridAutoRows = 174 => "grid-auto-rows",
    GridColumnSpan = 175 => "grid-column-span",
    GridRowSpan = 176 => "grid-row-span",
    GridColumnStart = 177 => "grid-column-start",
    GridColumnEnd = 178 => "grid-column-end",
    GridRowStart = 179 => "grid-row-start",
    GridRowEnd = 180 => "grid-row-end",
    GridColumnGap = 181 => "grid-column-gap",
    GridRowGap = 182 => "grid-row-gap",
    JustifyItems = 183 => "justify-items",
    JustifySelf = 184 => "justify-self",
    GridAutoFlow = 185 => "grid-auto-flow",
    Filter = 186 => "filter",
    ListMainAxisGap = 187 => "list-main-axis-gap",
    ListCrossAxisGap = 188 => "list-cross-axis-gap",
    LinearDirection = 189 => "linear-direction",
    Perspective = 190 => "perspective",
    Cursor = 191 => "cursor",
    TextIndent = 192 => "text-indent",
    ClipPath = 193 => "clip-path",
    TextStroke = 194 => "text-stroke",
    TextStrokeWidth = 195 => "text-stroke-width",
    TextStrokeColor = 196 => "text-stroke-color",
    XAutoFontSize = 197 => "-x-auto-font-size",
    XAutoFontSizePresetSizes = 198 => "-x-auto-font-size-preset-sizes",
    Mask = 199 => "mask",
    MaskRepeat = 200 => "mask-repeat",
    MaskPosition = 201 => "mask-position",
    MaskClip = 202 => "mask-clip",
    MaskOrigin = 203 => "mask-origin",
    MaskSize = 204 => "mask-size",
    Gap = 205 => "gap",
    ColumnGap = 206 => "column-gap",
    RowGap = 207 => "row-gap",
    ImageRendering = 208 => "image-rendering",
    Hyphens = 209 => "hyphens",
    XAppRegion = 210 => "-x-app-region",
    XAnimationColorInterpolation = 211 => "-x-animation-color-interpolation",
    XHandleColor = 212 => "-x-handle-color",
    XHandleSize = 213 => "-x-handle-size",
    OffsetPath = 214 => "offset-path",
    OffsetDistance = 215 => "offset-distance",
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
