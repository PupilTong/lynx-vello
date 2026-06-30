//! Style and CSS models shared by section decoders.

use crate::{
    error::{DecodeError, Result},
    model::CompileOptions,
    reader::Reader,
    value::{Value, decode_value},
    version::{V_2_14, V_3_9, Version},
};

/// Decoded CSS descriptor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CssDescriptor<'a> {
    /// CSS fragments keyed by their fragment id on wire.
    pub fragments: Vec<CssFragment<'a>>,
}

/// A decoded CSS fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct CssFragment<'a> {
    /// Fragment id.
    pub id: u32,
    /// Dependent CSS fragment ids.
    pub dependent_ids: Vec<i32>,
    /// Fragment body selected by compile options.
    pub body: CssFragmentBody<'a>,
}

/// Body variants for a CSS fragment.
#[derive(Debug, Clone, PartialEq)]
pub enum CssFragmentBody<'a> {
    /// Rule-list form used when `enable_css_rule` is set.
    Rules(Vec<CssRule<'a>>),
    /// Legacy selector/token form.
    Tokens(CssFragmentTokens<'a>),
}

/// Legacy selector/token CSS fragment body.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CssFragmentTokens<'a> {
    /// Selector tuples present when `enable_css_selector` is set.
    pub selectors: Vec<CssSelectorTuple<'a>>,
    /// Named parse tokens.
    pub tokens: Vec<(&'a str, CssParseToken<'a>)>,
    /// Keyframe tokens.
    pub keyframes: Vec<(&'a str, CssKeyframesToken<'a>)>,
    /// Legacy font-face blocks.
    pub font_faces: Vec<FontFaceEntry<'a>>,
}

/// One flattened selector tuple plus its declarations.
#[derive(Debug, Clone, PartialEq)]
pub struct CssSelectorTuple<'a> {
    /// Opaque selector nodes encoded as lepus values.
    pub selectors: Vec<Value<'a>>,
    /// Declarations for this selector tuple.
    pub token: CssParseToken<'a>,
}

/// A decoded rule-list CSS rule.
#[derive(Debug, Clone, PartialEq)]
pub enum CssRule<'a> {
    /// A style rule.
    Style {
        /// Parser document-order position.
        position: u32,
        /// Opaque selector nodes encoded as lepus values.
        selectors: Vec<Value<'a>>,
        /// Rule declarations.
        token: CssParseToken<'a>,
    },
    /// A media rule.
    Media {
        /// Opaque media condition encoded as a lepus value.
        condition: Value<'a>,
        /// Nested rules.
        children: Vec<CssRule<'a>>,
    },
    /// A supports rule.
    Supports {
        /// Opaque supports condition encoded as a lepus value.
        condition: Value<'a>,
        /// Nested rules.
        children: Vec<CssRule<'a>>,
    },
    /// A keyframes rule.
    Keyframes {
        /// Animation name.
        name: &'a str,
        /// Keyframes body.
        token: CssKeyframesToken<'a>,
    },
    /// A font-face rule in the rule-list format.
    FontFace(Value<'a>),
    /// A layer statement or block.
    Layer {
        /// Dot-separated layer-name segments.
        segments: Vec<&'a str>,
        /// Parser document-order position.
        position: u32,
        /// Nested rules, empty for statement form.
        children: Vec<CssRule<'a>>,
        /// Whether this was a block rule instead of a statement.
        is_block: bool,
    },
    /// A known or unknown rule type intentionally skipped by payload size.
    Skipped {
        /// Raw rule type byte.
        rule_type: u8,
    },
}

/// CSS parse token: declarations and optional selector metadata.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CssParseToken<'a> {
    /// Normal declarations.
    pub attributes: CssAttributes<'a>,
    /// Important declarations (`target_sdk >= 3.9`).
    pub important_attributes: CssAttributes<'a>,
    /// CSS style variables.
    pub style_variables: Vec<(&'a str, &'a str)>,
    /// Legacy sheets when selectors are disabled.
    pub sheets: Vec<CssSheet<'a>>,
}

/// CSS declaration map.
pub type CssAttributes<'a> = Vec<(u32, CssValue<'a>)>;

/// Legacy CSS sheet record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssSheet<'a> {
    /// Encoded type, ignored by the C++ reader and recomputed at runtime.
    pub type_id: u32,
    /// Sheet name.
    pub name: &'a str,
    /// Selector text.
    pub selector: &'a str,
}

/// Keyframes token.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CssKeyframesToken<'a> {
    /// Keyframe declaration frames.
    pub frames: Vec<CssKeyframe<'a>>,
    /// Custom property declarations gated by target SDK and compile options.
    pub custom_properties: Vec<CssKeyframeCustomProperties<'a>>,
}

/// A single keyframe selector and its declarations.
#[derive(Debug, Clone, PartialEq)]
pub struct CssKeyframe<'a> {
    /// Parsed numeric key when CSS parser is enabled.
    pub key: CssKeyframeKey<'a>,
    /// Frame declarations.
    pub attributes: CssAttributes<'a>,
}

/// Keyframe selector representation.
#[derive(Debug, Clone, PartialEq)]
pub enum CssKeyframeKey<'a> {
    /// Parsed floating-point key.
    Parsed(f64),
    /// Raw textual key.
    Text(&'a str),
}

/// Keyframe custom property content for a selector.
#[derive(Debug, Clone, PartialEq)]
pub struct CssKeyframeCustomProperties<'a> {
    /// Keyframe selector.
    pub key: CssKeyframeKey<'a>,
    /// Custom properties.
    pub properties: Vec<(&'a str, CssValue<'a>)>,
}

/// One font-face token.
pub type FontFaceToken<'a> = Vec<(&'a str, &'a str)>;

/// A decoded font-face entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFaceEntry<'a> {
    /// Family map key derived by the engine from the first token.
    pub family: &'a str,
    /// Token list.
    pub tokens: Vec<FontFaceToken<'a>>,
}

/// Decoded style-object descriptor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StyleObjects<'a> {
    /// Simple style objects.
    pub objects: Vec<CssAttributes<'a>>,
    /// Style-object keyframes.
    pub keyframes: Vec<(&'a str, CssKeyframesToken<'a>)>,
    /// Style-object font faces.
    pub font_faces: Vec<FontFaceEntry<'a>>,
}

/// Decoded parsed-styles descriptor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedStyles<'a> {
    /// Keyed parsed style blocks.
    pub entries: Vec<(&'a str, ParsedStyleBlock<'a>)>,
}

/// A parsed inline-style block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedStyleBlock<'a> {
    /// Parsed style declarations.
    pub attributes: CssAttributes<'a>,
    /// CSS variables.
    pub variables: Vec<(&'a str, &'a str)>,
}

/// A decoded CSS value.
#[derive(Debug, Clone, PartialEq)]
pub struct CssValue<'a> {
    /// Parser pattern. If CSS parser is disabled, this is `String`.
    pub pattern: CssValuePattern,
    /// Raw lepus value.
    pub value: Value<'a>,
    /// CSS variable value type.
    pub value_type: CssValueType,
    /// CSS variable default value trailer.
    pub default_value: Option<&'a str>,
    /// CSS variable multi-default trailer (`target_sdk >= 2.14`).
    pub default_value_map: Option<Value<'a>>,
}

/// CSS rule type enum.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssRuleType {
    /// Unknown rule.
    Unknown = 0,
    /// Charset.
    Charset = 1,
    /// Style rule.
    Style = 2,
    /// Import.
    Import = 3,
    /// Media.
    Media = 4,
    /// Font face.
    FontFace = 5,
    /// Font feature.
    FontFeature = 6,
    /// Property.
    Property = 7,
    /// Keyframes.
    Keyframes = 8,
    /// Layer block.
    LayerBlock = 9,
    /// Layer statement.
    LayerStatement = 10,
    /// Nested declarations.
    NestedDeclarations = 11,
    /// Function declarations.
    FunctionDeclarations = 12,
    /// Namespace.
    Namespace = 13,
    /// Container.
    Container = 14,
    /// Scope.
    Scope = 15,
    /// Supports.
    Supports = 16,
    /// Function.
    Function = 17,
    /// Mixin.
    Mixin = 18,
    /// Apply mixin.
    ApplyMixin = 19,
    /// Contents.
    Contents = 20,
    /// Position try.
    PositionTry = 21,
    /// Custom media.
    CustomMedia = 22,
}

impl TryFrom<u8> for CssRuleType {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Unknown),
            1 => Ok(Self::Charset),
            2 => Ok(Self::Style),
            3 => Ok(Self::Import),
            4 => Ok(Self::Media),
            5 => Ok(Self::FontFace),
            6 => Ok(Self::FontFeature),
            7 => Ok(Self::Property),
            8 => Ok(Self::Keyframes),
            9 => Ok(Self::LayerBlock),
            10 => Ok(Self::LayerStatement),
            11 => Ok(Self::NestedDeclarations),
            12 => Ok(Self::FunctionDeclarations),
            13 => Ok(Self::Namespace),
            14 => Ok(Self::Container),
            15 => Ok(Self::Scope),
            16 => Ok(Self::Supports),
            17 => Ok(Self::Function),
            18 => Ok(Self::Mixin),
            19 => Ok(Self::ApplyMixin),
            20 => Ok(Self::Contents),
            21 => Ok(Self::PositionTry),
            22 => Ok(Self::CustomMedia),
            _ => Err(DecodeError::Malformed("unknown CSS rule type")),
        }
    }
}

/// CSS value pattern enum.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssValuePattern {
    /// Empty.
    Empty = 0,
    /// String.
    String = 1,
    /// Number.
    Number = 2,
    /// Boolean.
    Boolean = 3,
    /// Enum.
    Enum = 4,
    /// px.
    Px = 5,
    /// rpx.
    Rpx = 6,
    /// em.
    Em = 7,
    /// rem.
    Rem = 8,
    /// vh.
    Vh = 9,
    /// vw.
    Vw = 10,
    /// percent.
    Percent = 11,
    /// calc.
    Calc = 12,
    /// env.
    Env = 13,
    /// array.
    Array = 14,
    /// map.
    Map = 15,
    /// ppx.
    Ppx = 16,
    /// intrinsic.
    Intrinsic = 17,
    /// sp.
    Sp = 18,
    /// fr.
    Fr = 19,
    /// count sentinel.
    Count = 20,
}

impl TryFrom<u8> for CssValuePattern {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Empty),
            1 => Ok(Self::String),
            2 => Ok(Self::Number),
            3 => Ok(Self::Boolean),
            4 => Ok(Self::Enum),
            5 => Ok(Self::Px),
            6 => Ok(Self::Rpx),
            7 => Ok(Self::Em),
            8 => Ok(Self::Rem),
            9 => Ok(Self::Vh),
            10 => Ok(Self::Vw),
            11 => Ok(Self::Percent),
            12 => Ok(Self::Calc),
            13 => Ok(Self::Env),
            14 => Ok(Self::Array),
            15 => Ok(Self::Map),
            16 => Ok(Self::Ppx),
            17 => Ok(Self::Intrinsic),
            18 => Ok(Self::Sp),
            19 => Ok(Self::Fr),
            20 => Ok(Self::Count),
            _ => Err(DecodeError::Malformed("unknown CSS value pattern")),
        }
    }
}

/// CSS variable value type.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssValueType {
    /// Plain default value.
    Default = 0,
    /// CSS variable value.
    Variable = 1,
}

impl TryFrom<u8> for CssValueType {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Default),
            1 => Ok(Self::Variable),
            _ => Err(DecodeError::Malformed("unknown CSS value type")),
        }
    }
}

/// Decode one `CSSValue`.
///
/// Reference: `core/template_bundle/template_codec/binary_decoder/lynx_binary_base_css_reader.cc:714`.
pub(crate) fn decode_css_value<'a>(
    reader: &mut Reader<'a>,
    enable_css_parser: bool,
    enable_css_variable: bool,
    target_sdk: Version,
) -> Result<CssValue<'a>> {
    let pattern = if enable_css_parser {
        let raw = reader.compact_u32()?;
        let raw_u8 =
            u8::try_from(raw).map_err(|_| DecodeError::Malformed("CSS value pattern too large"))?;
        CssValuePattern::try_from(raw_u8)?
    } else {
        CssValuePattern::String
    };
    let value = decode_value(reader)?;
    let (value_type, default_value, default_value_map) = if enable_css_variable {
        let raw = reader.compact_u32()?;
        let raw_u8 =
            u8::try_from(raw).map_err(|_| DecodeError::Malformed("CSS value type too large"))?;
        let value_type = CssValueType::try_from(raw_u8)?;
        let default_value = Some(reader.lstr()?);
        let default_value_map = if target_sdk.is_at_least(V_2_14) {
            Some(decode_value(reader)?)
        } else {
            None
        };
        (value_type, default_value, default_value_map)
    } else {
        (CssValueType::Default, None, None)
    };

    Ok(CssValue {
        pattern,
        value,
        value_type,
        default_value,
        default_value_map,
    })
}

/// Decode a `CSSAttributes` declaration map.
///
/// Reference: `core/template_bundle/template_codec/binary_decoder/lynx_binary_base_css_reader.cc:588`.
pub(crate) fn decode_css_attributes<'a>(
    reader: &mut Reader<'a>,
    enable_css_parser: bool,
    enable_css_variable: bool,
    target_sdk: Version,
) -> Result<CssAttributes<'a>> {
    let size = reader.compact_u32()? as usize;
    let mut attributes = Vec::new();
    attributes
        .try_reserve(size)
        .map_err(|_| DecodeError::Malformed("CSS attributes too large"))?;
    for _ in 0..size {
        let property_id = reader.compact_u32()?;
        let value = decode_css_value(reader, enable_css_parser, enable_css_variable, target_sdk)?;
        attributes.push((property_id, value));
    }
    Ok(attributes)
}

/// Decode a `CSSParseToken`.
///
/// Reference: `core/template_bundle/template_codec/binary_decoder/lynx_binary_base_css_reader.cc:505`.
pub(crate) fn decode_css_parse_token<'a>(
    reader: &mut Reader<'a>,
    options: &CompileOptions<'_>,
) -> Result<CssParseToken<'a>> {
    let attributes = decode_css_attributes(
        reader,
        options.css_parser_enabled(),
        options.css_variable_enabled(),
        options.target_sdk,
    )?;
    let important_attributes = if options.target_sdk.is_at_least(V_3_9) {
        decode_css_attributes(
            reader,
            options.css_parser_enabled(),
            options.css_variable_enabled(),
            options.target_sdk,
        )?
    } else {
        Vec::new()
    };
    let style_variables = if options.css_variable_enabled() {
        decode_css_style_variables(reader)?
    } else {
        Vec::new()
    };
    let sheets = if options.enable_css_selector {
        Vec::new()
    } else {
        decode_css_sheets(reader)?
    };

    Ok(CssParseToken {
        attributes,
        important_attributes,
        style_variables,
        sheets,
    })
}

/// Decode CSS style variables.
///
/// Reference: `core/template_bundle/template_codec/binary_decoder/lynx_binary_base_css_reader.cc:637`.
pub(crate) fn decode_css_style_variables<'a>(
    reader: &mut Reader<'a>,
) -> Result<Vec<(&'a str, &'a str)>> {
    let size = reader.compact_u32()? as usize;
    let mut variables = Vec::new();
    variables
        .try_reserve(size)
        .map_err(|_| DecodeError::Malformed("CSS variables too large"))?;
    for _ in 0..size {
        let key = reader.lstr()?;
        let value = reader.lstr()?;
        variables.push((key, value));
    }
    Ok(variables)
}

fn decode_css_sheets<'a>(reader: &mut Reader<'a>) -> Result<Vec<CssSheet<'a>>> {
    let size = reader.compact_u32()? as usize;
    let mut sheets = Vec::new();
    sheets
        .try_reserve(size)
        .map_err(|_| DecodeError::Malformed("CSS sheets too large"))?;
    for _ in 0..size {
        sheets.push(CssSheet {
            type_id: reader.compact_u32()?,
            name: reader.lstr()?,
            selector: reader.lstr()?,
        });
    }
    Ok(sheets)
}

/// Decode a keyframes token.
///
/// Reference: `core/template_bundle/template_codec/binary_decoder/lynx_binary_base_css_reader.cc:549`.
pub(crate) fn decode_css_keyframes_token<'a>(
    reader: &mut Reader<'a>,
    options: &CompileOptions<'_>,
) -> Result<CssKeyframesToken<'a>> {
    let frames = decode_css_keyframes(reader, options)?;
    let custom_properties = Vec::new();
    Ok(CssKeyframesToken {
        frames,
        custom_properties,
    })
}

fn decode_css_keyframes<'a>(
    reader: &mut Reader<'a>,
    options: &CompileOptions<'_>,
) -> Result<Vec<CssKeyframe<'a>>> {
    let size = reader.compact_u32()? as usize;
    let mut frames = Vec::new();
    frames
        .try_reserve(size)
        .map_err(|_| DecodeError::Malformed("CSS keyframes too large"))?;
    for _ in 0..size {
        let key = if options.css_parser_enabled() {
            CssKeyframeKey::Parsed(reader.compact_f64()?)
        } else {
            CssKeyframeKey::Text(reader.lstr()?)
        };
        let attributes = decode_css_attributes(
            reader,
            options.css_parser_enabled(),
            options.css_variable_enabled(),
            options.target_sdk,
        )?;
        frames.push(CssKeyframe { key, attributes });
    }
    Ok(frames)
}

/// Decode one legacy font-face token.
///
/// Reference: `core/template_bundle/template_codec/binary_decoder/lynx_binary_base_css_reader.cc:539`.
pub(crate) fn decode_font_face_token<'a>(reader: &mut Reader<'a>) -> Result<FontFaceToken<'a>> {
    let size = reader.compact_u32()? as usize;
    let mut token = Vec::new();
    token
        .try_reserve(size)
        .map_err(|_| DecodeError::Malformed("font-face token too large"))?;
    for _ in 0..size {
        token.push((reader.lstr()?, reader.lstr()?));
    }
    Ok(token)
}

/// Decode a parsed style block.
///
/// Reference: `core/template_bundle/template_codec/binary_decoder/element_binary_reader.cc:842`.
pub(crate) fn decode_parsed_style_block<'a>(
    reader: &mut Reader<'a>,
    options: &CompileOptions<'_>,
) -> Result<ParsedStyleBlock<'a>> {
    // C++ element_binary_reader.cc:849 calls the one-arg DecodeCSSValue overload,
    // which uses member-gated parser/variable flags rather than forcing true.
    let attributes = decode_css_attributes(
        reader,
        options.css_parser_enabled(),
        options.css_variable_enabled(),
        options.target_sdk,
    )?;
    let variables = decode_css_style_variables(reader)?;
    Ok(ParsedStyleBlock {
        attributes,
        variables,
    })
}
