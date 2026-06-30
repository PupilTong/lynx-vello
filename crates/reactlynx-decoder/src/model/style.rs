//! Style and CSS models shared by section decoders.

use crate::{
    error::{DecodeError, Result},
    reader::Reader,
    value::{Value, decode_value},
    version::{V_2_14, Version},
};

/// Raw-captured CSS descriptor for this narrowed run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CssDescriptor<'a> {
    /// Raw CSS section body, when present.
    pub raw: Option<&'a [u8]>,
}

/// Raw-captured style-object descriptor for this narrowed run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyleObjects<'a> {
    /// Raw `STYLE_OBJECT` section body.
    pub raw: &'a [u8],
}

/// Raw-captured parsed-styles descriptor for this narrowed run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedStyles<'a> {
    /// Raw `PARSED_STYLES` section body.
    pub raw: &'a [u8],
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
