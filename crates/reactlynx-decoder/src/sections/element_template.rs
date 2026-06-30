//! `NEW_ELEMENT_TEMPLATE` section decoder.

use crate::{
    error::{DecodeError, Result},
    model::{
        AttributeBinding, AttributeBindingType, CompileOptions, ElementBuiltInAttribute,
        ElementBuiltInTag, ElementNode, ElementSection, ElementTag, ElementTemplates, EventBinding,
        EventType, ParsedStyleEntry, PiperEventBinding, TemplateBundle, style::decode_css_value,
    },
    reader::Reader,
    value::{Value, decode_value},
};

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    bundle.element_templates = decode_templates(reader, &bundle.compile_options)?;
    Ok(())
}

fn decode_templates<'a>(
    reader: &mut Reader<'a>,
    options: &CompileOptions<'a>,
) -> Result<ElementTemplates<'a>> {
    let route_count = reader.compact_u32()? as usize;
    let mut routes = Vec::new();
    routes
        .try_reserve(route_count)
        .map_err(|_| DecodeError::Malformed("element template router is too large"))?;
    for _ in 0..route_count {
        let key = reader.lstr()?;
        let start_offset = reader.compact_u32()? as usize;
        routes.push((key, start_offset));
    }

    let descriptor_offset = reader.pos();
    let mut templates = Vec::new();
    templates
        .try_reserve(routes.len())
        .map_err(|_| DecodeError::Malformed("element template map is too large"))?;
    for (key, start_offset) in routes {
        let body_pos = descriptor_offset
            .checked_add(start_offset)
            .ok_or(DecodeError::Malformed("element template offset overflow"))?;
        reader.seek(body_pos)?;
        let element_count = reader.compact_u32()? as usize;
        let mut roots = Vec::new();
        roots
            .try_reserve(element_count)
            .map_err(|_| DecodeError::Malformed("element template root list is too large"))?;
        for _ in 0..element_count {
            roots.push(decode_element(reader, options)?);
        }
        templates.push((key, roots));
    }

    Ok(ElementTemplates { templates })
}

fn decode_element<'a>(
    reader: &mut Reader<'a>,
    options: &CompileOptions<'a>,
) -> Result<ElementNode<'a>> {
    let field_end = reader.pos().saturating_add(4);
    let children_section_offset = reader.u32()? as usize;
    let children_section_pos = field_end
        .checked_add(children_section_offset)
        .ok_or(DecodeError::Malformed("children section offset overflow"))?;

    let mut section = read_section(reader)?;
    if section == ElementSection::ConstructionInfo {
        decode_construction_info(reader)?;
        section = read_section(reader)?;
    }

    let tag = match section {
        ElementSection::TagEnum => ElementTag::Builtin(ElementBuiltInTag::try_from(reader.u8()?)?),
        ElementSection::TagStr => ElementTag::Custom(reader.lstr()?),
        _ => return Err(DecodeError::Malformed("element tag section missing")),
    };

    let mut node = ElementNode {
        tag,
        attributes_array: Vec::new(),
        slot_index: None,
        builtin_attributes: Vec::new(),
        id_selector: None,
        inline_styles: Vec::new(),
        classes: Vec::new(),
        events: Vec::new(),
        piper_events: Vec::new(),
        attributes: Vec::new(),
        dataset: Vec::new(),
        parsed_style_key: None,
        parsed_styles: None,
        children: Vec::new(),
    };

    loop {
        let raw_section = reader.u8()?;
        let section = match ElementSection::try_from(raw_section) {
            Ok(section) => section,
            Err(DecodeError::BadElementTag(_)) => {
                // C++ forward-compat skip:
                // core/template_bundle/template_codec/binary_decoder/element_binary_reader.cc:212
                reader.seek(children_section_pos)?;
                continue;
            }
            Err(err) => return Err(err),
        };

        match section {
            ElementSection::ConstructionInfo => decode_construction_info(reader)?,
            ElementSection::TagEnum | ElementSection::TagStr => {
                return Err(DecodeError::Malformed("duplicate element tag section"));
            }
            ElementSection::BuiltinAttribute => {
                node.builtin_attributes = decode_builtin_attributes(reader)?;
            }
            ElementSection::IdSelector => node.id_selector = Some(reader.lstr()?),
            ElementSection::Children => {
                node.children = decode_children(reader, options)?;
                break;
            }
            ElementSection::Class => node.classes = decode_string_list(reader)?,
            ElementSection::Styles => node.inline_styles = decode_value_map(reader)?,
            ElementSection::Attributes => node.attributes = decode_named_value_map(reader)?,
            ElementSection::Events => node.events = decode_events(reader)?,
            ElementSection::DataSet => node.dataset = decode_named_value_map(reader)?,
            ElementSection::ParsedStyles => {
                node.parsed_styles = Some(decode_parsed_style_entry(reader, options)?);
            }
            ElementSection::ParsedStylesKey => node.parsed_style_key = Some(reader.lstr()?),
            ElementSection::PiperEvents => node.piper_events = decode_piper_events(reader)?,
            ElementSection::AttributeArray => {
                node.attributes_array = decode_attribute_array(reader)?;
            }
            ElementSection::SlotIndex => node.slot_index = Some(reader.compact_u32()?),
        }
    }

    Ok(node)
}

fn read_section(reader: &mut Reader<'_>) -> Result<ElementSection> {
    ElementSection::try_from(reader.u8()?)
}

fn decode_construction_info(reader: &mut Reader<'_>) -> Result<()> {
    let count = reader.compact_u32()? as usize;
    for _ in 0..count {
        let _key = reader.compact_u32()?;
        let _value = decode_value(reader)?;
    }
    Ok(())
}

fn decode_attribute_array<'a>(reader: &mut Reader<'a>) -> Result<Vec<AttributeBinding<'a>>> {
    let count = reader.compact_u32()? as usize;
    let mut out = Vec::new();
    out.try_reserve(count)
        .map_err(|_| DecodeError::Malformed("attribute array is too large"))?;
    for _ in 0..count {
        let raw_kind = reader.compact_u32()?;
        let kind_u8 = u8::try_from(raw_kind)
            .map_err(|_| DecodeError::Malformed("attribute binding type too large"))?;
        let kind = AttributeBindingType::try_from(kind_u8)?;
        let key = reader.lstr()?;
        let binding = match kind {
            AttributeBindingType::Static => AttributeBinding::Static {
                key,
                value: decode_value(reader)?,
            },
            AttributeBindingType::Dynamic => AttributeBinding::Dynamic {
                key,
                attr_slot_index: reader.compact_u32()?,
            },
            AttributeBindingType::Spread => AttributeBinding::Spread {
                attr_slot_index: reader.compact_u32()?,
            },
        };
        out.push(binding);
    }
    Ok(out)
}

fn decode_builtin_attributes<'a>(
    reader: &mut Reader<'a>,
) -> Result<Vec<(ElementBuiltInAttribute, Value<'a>)>> {
    let count = reader.compact_u32()? as usize;
    let mut out = Vec::new();
    out.try_reserve(count)
        .map_err(|_| DecodeError::Malformed("built-in attributes are too large"))?;
    for _ in 0..count {
        let key = ElementBuiltInAttribute::try_from(reader.compact_u32()?)?;
        let value = decode_value(reader)?;
        out.push((key, value));
    }
    Ok(out)
}

fn decode_value_map<'a>(reader: &mut Reader<'a>) -> Result<Vec<(u32, Value<'a>)>> {
    let count = reader.compact_u32()? as usize;
    let mut out = Vec::new();
    out.try_reserve(count)
        .map_err(|_| DecodeError::Malformed("value map is too large"))?;
    for _ in 0..count {
        out.push((reader.compact_u32()?, decode_value(reader)?));
    }
    Ok(out)
}

fn decode_named_value_map<'a>(reader: &mut Reader<'a>) -> Result<Vec<(&'a str, Value<'a>)>> {
    let count = reader.compact_u32()? as usize;
    let mut out = Vec::new();
    out.try_reserve(count)
        .map_err(|_| DecodeError::Malformed("named value map is too large"))?;
    for _ in 0..count {
        out.push((reader.lstr()?, decode_value(reader)?));
    }
    Ok(out)
}

fn decode_string_list<'a>(reader: &mut Reader<'a>) -> Result<Vec<&'a str>> {
    let count = reader.compact_u32()? as usize;
    let mut out = Vec::new();
    out.try_reserve(count)
        .map_err(|_| DecodeError::Malformed("string list is too large"))?;
    for _ in 0..count {
        out.push(reader.lstr()?);
    }
    Ok(out)
}

fn decode_events<'a>(reader: &mut Reader<'a>) -> Result<Vec<EventBinding<'a>>> {
    let count = reader.compact_u32()? as usize;
    let mut out = Vec::new();
    out.try_reserve(count)
        .map_err(|_| DecodeError::Malformed("event list is too large"))?;
    for _ in 0..count {
        let kind = EventType::try_from(reader.u8()?)?;
        let name = reader.lstr()?;
        let value = reader.lstr()?;
        out.push(EventBinding { kind, name, value });
    }
    Ok(out)
}

fn decode_piper_events<'a>(reader: &mut Reader<'a>) -> Result<Vec<PiperEventBinding<'a>>> {
    let count = reader.compact_u32()? as usize;
    let mut out = Vec::new();
    out.try_reserve(count)
        .map_err(|_| DecodeError::Malformed("piper event list is too large"))?;
    for _ in 0..count {
        let kind = EventType::try_from(reader.u8()?)?;
        let name = reader.lstr()?;
        let value = decode_value(reader)?;
        out.push(PiperEventBinding { kind, name, value });
    }
    Ok(out)
}

fn decode_parsed_style_entry<'a>(
    reader: &mut Reader<'a>,
    options: &CompileOptions<'a>,
) -> Result<ParsedStyleEntry<'a>> {
    let style_count = reader.compact_u32()? as usize;
    let mut attributes = Vec::new();
    attributes
        .try_reserve(style_count)
        .map_err(|_| DecodeError::Malformed("parsed style block is too large"))?;
    for _ in 0..style_count {
        let property_id = reader.compact_u32()?;
        let value = decode_css_value(
            reader,
            options.enable_css_parser,
            options.enable_css_variable,
            options.target_sdk,
        )?;
        attributes.push((property_id, value));
    }

    let variable_count = reader.compact_u32()? as usize;
    let mut variables = Vec::new();
    variables
        .try_reserve(variable_count)
        .map_err(|_| DecodeError::Malformed("parsed style variables are too large"))?;
    for _ in 0..variable_count {
        variables.push((reader.lstr()?, reader.lstr()?));
    }

    Ok(ParsedStyleEntry {
        attributes,
        variables,
    })
}

fn decode_children<'a>(
    reader: &mut Reader<'a>,
    options: &CompileOptions<'a>,
) -> Result<Vec<ElementNode<'a>>> {
    let count = reader.compact_u32()? as usize;
    let mut children = Vec::new();
    children
        .try_reserve(count)
        .map_err(|_| DecodeError::Malformed("element child list is too large"))?;
    for _ in 0..count {
        children.push(decode_element(reader, options)?);
    }
    Ok(children)
}
