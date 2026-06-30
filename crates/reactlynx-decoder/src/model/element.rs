//! Element-template model and wire enums.

use crate::{error::DecodeError, value::Value};

/// Ordered map of template id to root element nodes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ElementTemplates<'a> {
    /// Template entries in router order.
    pub templates: Vec<(&'a str, Vec<ElementNode<'a>>)>,
}

/// A decoded fiber element node.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementNode<'a> {
    /// Element tag.
    pub tag: ElementTag<'a>,
    /// Attribute descriptor array.
    pub attributes_array: Vec<AttributeBinding<'a>>,
    /// Slot placeholder index.
    pub slot_index: Option<u32>,
    /// Built-in attributes whose ids are in the 1000+ range.
    pub builtin_attributes: Vec<(ElementBuiltInAttribute, Value<'a>)>,
    /// Dedicated id selector.
    pub id_selector: Option<&'a str>,
    /// Dedicated inline style attributes.
    pub inline_styles: Vec<(u32, Value<'a>)>,
    /// Dedicated class list.
    pub classes: Vec<&'a str>,
    /// JavaScript event bindings.
    pub events: Vec<EventBinding<'a>>,
    /// Lepus/static event bindings.
    pub piper_events: Vec<PiperEventBinding<'a>>,
    /// Generic attributes.
    pub attributes: Vec<(&'a str, Value<'a>)>,
    /// Dataset attributes.
    pub dataset: Vec<(&'a str, Value<'a>)>,
    /// Key into the shared parsed-styles section.
    pub parsed_style_key: Option<&'a str>,
    /// Inline parsed styles.
    pub parsed_styles: Option<ParsedStyleEntry<'a>>,
    /// Child element nodes.
    pub children: Vec<ElementNode<'a>>,
}

/// Element tag representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementTag<'a> {
    /// Built-in tag enum.
    Builtin(ElementBuiltInTag),
    /// Custom tag string.
    Custom(&'a str),
}

/// Attribute array binding.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeBinding<'a> {
    /// Static key/value.
    Static { key: &'a str, value: Value<'a> },
    /// Dynamic slot binding.
    Dynamic { key: &'a str, attr_slot_index: u32 },
    /// Spread slot binding.
    Spread { attr_slot_index: u32 },
}

/// JavaScript event binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventBinding<'a> {
    /// Event type.
    pub kind: EventType,
    /// Event name.
    pub name: &'a str,
    /// Handler value.
    pub value: &'a str,
}

/// Lepus/static event binding.
#[derive(Debug, Clone, PartialEq)]
pub struct PiperEventBinding<'a> {
    /// Event type.
    pub kind: EventType,
    /// Event name.
    pub name: &'a str,
    /// Event payload.
    pub value: Value<'a>,
}

/// Parsed inline style block.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedStyleEntry<'a> {
    /// CSS property id to parsed value pairs.
    pub attributes: Vec<(u32, crate::model::style::CssValue<'a>)>,
    /// CSS variable name/value pairs.
    pub variables: Vec<(&'a str, &'a str)>,
}

/// Element section tags.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementSection {
    /// Construction info, currently skipped.
    ConstructionInfo = 0,
    /// Built-in tag enum.
    TagEnum = 1,
    /// Custom tag string.
    TagStr = 2,
    /// Built-in attributes.
    BuiltinAttribute = 3,
    /// Dedicated id selector.
    IdSelector = 4,
    /// Children terminator section.
    Children = 5,
    /// Dedicated class list.
    Class = 6,
    /// Dedicated inline styles.
    Styles = 7,
    /// Generic attributes.
    Attributes = 8,
    /// JavaScript events.
    Events = 9,
    /// Dataset.
    DataSet = 10,
    /// Inline parsed styles.
    ParsedStyles = 11,
    /// Shared parsed style key.
    ParsedStylesKey = 12,
    /// Lepus/static events.
    PiperEvents = 13,
    /// Attribute descriptor array.
    AttributeArray = 14,
    /// Slot element index.
    SlotIndex = 15,
}

impl TryFrom<u8> for ElementSection {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::ConstructionInfo),
            1 => Ok(Self::TagEnum),
            2 => Ok(Self::TagStr),
            3 => Ok(Self::BuiltinAttribute),
            4 => Ok(Self::IdSelector),
            5 => Ok(Self::Children),
            6 => Ok(Self::Class),
            7 => Ok(Self::Styles),
            8 => Ok(Self::Attributes),
            9 => Ok(Self::Events),
            10 => Ok(Self::DataSet),
            11 => Ok(Self::ParsedStyles),
            12 => Ok(Self::ParsedStylesKey),
            13 => Ok(Self::PiperEvents),
            14 => Ok(Self::AttributeArray),
            15 => Ok(Self::SlotIndex),
            other => Err(DecodeError::BadElementTag(other)),
        }
    }
}

/// Built-in element tags.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementBuiltInTag {
    /// `view`
    View = 0,
    /// `text`
    Text = 1,
    /// `raw-text`
    RawText = 2,
    /// `image`
    Image = 3,
    /// `scroll-view`
    ScrollView = 4,
    /// `list`
    List = 5,
    /// `component`
    Component = 6,
    /// `page`
    Page = 7,
    /// `none`
    None = 8,
    /// `wrapper`
    Wrapper = 9,
    /// Other built-in.
    Other = 10,
    /// `x-text`
    XText = 11,
    /// `x-scroll-view`
    XScrollView = 12,
    /// Empty sentinel.
    Empty = 13,
    /// `inline-text`
    InlineText = 14,
    /// `x-inline-text`
    XInlineText = 15,
    /// `x-nested-scroll-view`
    XNestedScrollView = 16,
    /// `inline-image`
    InlineImage = 17,
    /// `slot`
    Slot = 18,
}

impl TryFrom<u8> for ElementBuiltInTag {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::View),
            1 => Ok(Self::Text),
            2 => Ok(Self::RawText),
            3 => Ok(Self::Image),
            4 => Ok(Self::ScrollView),
            5 => Ok(Self::List),
            6 => Ok(Self::Component),
            7 => Ok(Self::Page),
            8 => Ok(Self::None),
            9 => Ok(Self::Wrapper),
            10 => Ok(Self::Other),
            11 => Ok(Self::XText),
            12 => Ok(Self::XScrollView),
            13 => Ok(Self::Empty),
            14 => Ok(Self::InlineText),
            15 => Ok(Self::XInlineText),
            16 => Ok(Self::XNestedScrollView),
            17 => Ok(Self::InlineImage),
            18 => Ok(Self::Slot),
            _ => Err(DecodeError::Malformed("unknown built-in element tag")),
        }
    }
}

/// Attribute binding kind.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeBindingType {
    /// Static key/value.
    Static = 0,
    /// Dynamic slot binding.
    Dynamic = 1,
    /// Spread slot binding.
    Spread = 2,
}

impl TryFrom<u8> for AttributeBindingType {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Static),
            1 => Ok(Self::Dynamic),
            2 => Ok(Self::Spread),
            _ => Err(DecodeError::Malformed("unknown attribute binding type")),
        }
    }
}

/// Event type enum.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    /// Bind event.
    BindEvent = 0,
    /// Catch event.
    CatchEvent = 1,
    /// Capture bind.
    CaptureBind = 2,
    /// Capture catch.
    CaptureCatch = 3,
    /// Global bind.
    GlobalBind = 4,
}

impl TryFrom<u8> for EventType {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::BindEvent),
            1 => Ok(Self::CatchEvent),
            2 => Ok(Self::CaptureBind),
            3 => Ok(Self::CaptureCatch),
            4 => Ok(Self::GlobalBind),
            _ => Err(DecodeError::Malformed("unknown event type")),
        }
    }
}

/// Built-in attribute enum. Values start at 1000, so this cannot be `repr(u8)`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementBuiltInAttribute {
    /// Component id.
    ComponentId = 1000,
    /// Component name.
    ComponentName = 1001,
    /// Component path.
    ComponentPath = 1002,
    /// CSS id.
    CssId = 1003,
    /// Node index.
    NodeIndex = 1004,
    /// Dirty id.
    DirtyId = 1005,
    /// Config.
    Config = 1006,
    /// Template part flag.
    IsTemplatePart = 1007,
}

impl TryFrom<u32> for ElementBuiltInAttribute {
    type Error = DecodeError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1000 => Ok(Self::ComponentId),
            1001 => Ok(Self::ComponentName),
            1002 => Ok(Self::ComponentPath),
            1003 => Ok(Self::CssId),
            1004 => Ok(Self::NodeIndex),
            1005 => Ok(Self::DirtyId),
            1006 => Ok(Self::Config),
            1007 => Ok(Self::IsTemplatePart),
            _ => Err(DecodeError::Malformed("unknown built-in attribute")),
        }
    }
}
