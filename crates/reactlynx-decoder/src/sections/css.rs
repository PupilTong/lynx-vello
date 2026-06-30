//! `CSS` section decoder.

use crate::{
    error::{DecodeError, Result},
    model::{
        CssDescriptor, CssFragment, CssFragmentBody, CssFragmentTokens, CssRule, CssRuleType,
        CssSelectorTuple, FontFaceEntry, TemplateBundle,
        style::{decode_css_keyframes_token, decode_css_parse_token, decode_font_face_token},
    },
    reader::Reader,
    value::decode_value,
};

const CSS_BINARY_FONT_FACE_TYPE: u8 = 0x01;

#[derive(Debug, Clone, Copy)]
struct FragmentRange {
    id: i32,
    start: usize,
    end: usize,
}

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // C++ DecodeCSSRoute stores the post-route offset as css_section_range_.start.
    // core/template_bundle/template_codec/binary_decoder/lynx_binary_base_css_reader.cc:85
    let ranges = decode_route(reader)?;
    let base = reader.pos();
    let mut fragments = Vec::new();
    fragments
        .try_reserve(ranges.len())
        .map_err(|_| DecodeError::Malformed("CSS fragments too large"))?;
    for range in ranges {
        let start = base
            .checked_add(range.start)
            .ok_or(DecodeError::Malformed("CSS fragment start overflow"))?;
        let end = base
            .checked_add(range.end)
            .ok_or(DecodeError::Malformed("CSS fragment end overflow"))?;
        if start > end || end > reader.len() {
            return Err(DecodeError::Malformed("CSS fragment range out of bounds"));
        }
        let mut fragment_reader = reader.sub(start, end)?;
        let fragment = decode_fragment(&mut fragment_reader, range.id, &bundle.compile_options)?;
        fragments.push(fragment);
    }
    bundle.css = CssDescriptor { fragments };
    Ok(())
}

fn decode_route(reader: &mut Reader<'_>) -> Result<Vec<FragmentRange>> {
    let count = reader.compact_u32()? as usize;
    let mut ranges = Vec::new();
    ranges
        .try_reserve(count)
        .map_err(|_| DecodeError::Malformed("CSS route too large"))?;
    for _ in 0..count {
        ranges.push(FragmentRange {
            id: reader.compact_i32()?,
            start: reader.compact_u32()? as usize,
            end: reader.compact_u32()? as usize,
        });
    }
    Ok(ranges)
}

fn decode_fragment<'a>(
    reader: &mut Reader<'a>,
    route_id: i32,
    options: &crate::model::CompileOptions<'_>,
) -> Result<CssFragment<'a>> {
    // C++ DecodeCSSFragment starts with id and dependents.
    // core/template_bundle/template_codec/binary_decoder/lynx_binary_base_css_reader.cc:106
    let id = reader.compact_u32()?;
    if route_id >= 0 && id != route_id.cast_unsigned() {
        return Err(DecodeError::Malformed(
            "CSS fragment id does not match route",
        ));
    }
    let dependent_count = reader.compact_u32()? as usize;
    let mut dependent_ids = Vec::new();
    dependent_ids
        .try_reserve(dependent_count)
        .map_err(|_| DecodeError::Malformed("CSS dependents too large"))?;
    for _ in 0..dependent_count {
        dependent_ids.push(reader.compact_i32()?);
    }

    let body = if options.enable_css_rule {
        CssFragmentBody::Rules(decode_rules(reader, options)?)
    } else {
        CssFragmentBody::Tokens(decode_token_body(reader, options)?)
    };

    Ok(CssFragment {
        id,
        dependent_ids,
        body,
    })
}

fn decode_token_body<'a>(
    reader: &mut Reader<'a>,
    options: &crate::model::CompileOptions<'_>,
) -> Result<CssFragmentTokens<'a>> {
    let selectors = if options.enable_css_selector {
        decode_selector_tuples(reader, options)?
    } else {
        Vec::new()
    };

    let packed = reader.compact_u32()?;
    let token_count = (packed & 0xFFFF) as usize;
    let keyframes_count = (packed >> 16) as usize;
    let mut tokens = Vec::new();
    tokens
        .try_reserve(token_count)
        .map_err(|_| DecodeError::Malformed("CSS tokens too large"))?;
    for _ in 0..token_count {
        let key = reader.lstr()?;
        let token = decode_css_parse_token(reader, options)?;
        tokens.push((key, token));
    }

    let mut keyframes = Vec::new();
    keyframes
        .try_reserve(keyframes_count)
        .map_err(|_| DecodeError::Malformed("CSS keyframes too large"))?;
    for _ in 0..keyframes_count {
        let name = reader.lstr()?;
        let token = decode_css_keyframes_token(reader, options)?;
        keyframes.push((name, token));
    }

    let mut font_faces = Vec::new();
    while reader.remaining() >= 5 {
        let block_type = reader.u8()?;
        let typed_size = reader.compact_u32()? as usize;
        if block_type == CSS_BINARY_FONT_FACE_TYPE {
            font_faces
                .try_reserve(typed_size)
                .map_err(|_| DecodeError::Malformed("font-face entries too large"))?;
            for _ in 0..typed_size {
                // C++ lynx_binary_base_css_reader.cc:749 gates the token-count
                // prefix on FEATURE_CSS_FONT_FACE_EXTENSION (target_sdk >= 2.7).
                let tokens =
                    decode_font_face_list(reader, options.css_font_face_extension_enabled())?;
                let family = font_family(&tokens);
                font_faces.push(FontFaceEntry { family, tokens });
            }
        }
    }
    if !reader.is_at_end() {
        return Err(DecodeError::Malformed(
            "trailing CSS typed block is truncated",
        ));
    }

    Ok(CssFragmentTokens {
        selectors,
        tokens,
        keyframes,
        font_faces,
    })
}

fn decode_selector_tuples<'a>(
    reader: &mut Reader<'a>,
    options: &crate::model::CompileOptions<'_>,
) -> Result<Vec<CssSelectorTuple<'a>>> {
    let count = reader.compact_u32()? as usize;
    let mut tuples = Vec::new();
    tuples
        .try_reserve(count)
        .map_err(|_| DecodeError::Malformed("CSS selector tuples too large"))?;
    for _ in 0..count {
        let flattened_size = reader.compact_u32()? as usize;
        if flattened_size == 0 {
            continue;
        }
        let mut selectors = Vec::new();
        selectors
            .try_reserve(flattened_size)
            .map_err(|_| DecodeError::Malformed("CSS selectors too large"))?;
        for _ in 0..flattened_size {
            selectors.push(decode_value(reader)?);
        }
        let token = decode_css_parse_token(reader, options)?;
        tuples.push(CssSelectorTuple { selectors, token });
    }
    Ok(tuples)
}

fn decode_rules<'a>(
    reader: &mut Reader<'a>,
    options: &crate::model::CompileOptions<'_>,
) -> Result<Vec<CssRule<'a>>> {
    let count = reader.compact_u32()? as usize;
    let mut rules = Vec::new();
    rules
        .try_reserve(count)
        .map_err(|_| DecodeError::Malformed("CSS rules too large"))?;
    for _ in 0..count {
        rules.push(decode_sized_rule(reader, options)?);
    }
    Ok(rules)
}

fn decode_sized_rule<'a>(
    reader: &mut Reader<'a>,
    options: &crate::model::CompileOptions<'_>,
) -> Result<CssRule<'a>> {
    // C++ DecodeCSSRules always seeks to Offset()+payload_size after each rule.
    // core/template_bundle/template_codec/binary_decoder/lynx_binary_base_css_reader.cc:229
    let raw_type = reader.u8()?;
    let payload_size = reader.u32()? as usize;
    let payload_start = reader.pos();
    let next = payload_start
        .checked_add(payload_size)
        .ok_or(DecodeError::Malformed("CSS rule payload overflow"))?;
    if next > reader.len() {
        return Err(DecodeError::UnexpectedEof {
            at: payload_start,
            need: payload_size,
            have: reader.remaining(),
        });
    }

    let rule = match CssRuleType::try_from(raw_type) {
        Ok(CssRuleType::Style) => decode_style_rule(reader, options)?,
        Ok(CssRuleType::Media) => {
            let condition = decode_value(reader)?;
            let children = decode_nested_rules(reader, options)?;
            CssRule::Media {
                condition,
                children,
            }
        }
        Ok(CssRuleType::Supports) => {
            let condition = decode_value(reader)?;
            let children = decode_nested_rules(reader, options)?;
            CssRule::Supports {
                condition,
                children,
            }
        }
        Ok(CssRuleType::Keyframes) => CssRule::Keyframes {
            name: reader.lstr()?,
            token: decode_css_keyframes_token(reader, options)?,
        },
        Ok(CssRuleType::FontFace) => CssRule::FontFace(decode_value(reader)?),
        Ok(CssRuleType::LayerBlock) => decode_layer_rule(reader, options, true)?,
        Ok(CssRuleType::LayerStatement) => decode_layer_rule(reader, options, false)?,
        Ok(_) | Err(_) => CssRule::Skipped {
            rule_type: raw_type,
        },
    };

    if reader.pos() > next {
        return Err(DecodeError::Malformed("CSS rule over-read payload"));
    }
    reader.seek(next)?;
    Ok(rule)
}

fn decode_nested_rules<'a>(
    reader: &mut Reader<'a>,
    options: &crate::model::CompileOptions<'_>,
) -> Result<Vec<CssRule<'a>>> {
    let count = reader.compact_u32()? as usize;
    let mut children = Vec::new();
    children
        .try_reserve(count)
        .map_err(|_| DecodeError::Malformed("CSS child rules too large"))?;
    for _ in 0..count {
        children.push(decode_sized_rule(reader, options)?);
    }
    Ok(children)
}

fn decode_style_rule<'a>(
    reader: &mut Reader<'a>,
    options: &crate::model::CompileOptions<'_>,
) -> Result<CssRule<'a>> {
    let position = reader.compact_u32()?;
    let flattened_size = reader.compact_u32()? as usize;
    let mut selectors = Vec::new();
    selectors
        .try_reserve(flattened_size)
        .map_err(|_| DecodeError::Malformed("CSS style selectors too large"))?;
    for _ in 0..flattened_size {
        selectors.push(decode_value(reader)?);
    }
    let token = decode_css_parse_token(reader, options)?;
    Ok(CssRule::Style {
        position,
        selectors,
        token,
    })
}

fn decode_layer_rule<'a>(
    reader: &mut Reader<'a>,
    options: &crate::model::CompileOptions<'_>,
    is_block: bool,
) -> Result<CssRule<'a>> {
    let segment_count = reader.compact_u32()? as usize;
    let mut segments = Vec::new();
    segments
        .try_reserve(segment_count)
        .map_err(|_| DecodeError::Malformed("CSS layer segments too large"))?;
    for _ in 0..segment_count {
        segments.push(reader.lstr()?);
    }
    let position = reader.compact_u32()?;
    let children = if is_block {
        decode_nested_rules(reader, options)?
    } else {
        Vec::new()
    };
    Ok(CssRule::Layer {
        segments,
        position,
        children,
        is_block,
    })
}

pub(crate) fn decode_font_face_list<'a>(
    reader: &mut Reader<'a>,
    has_token_count: bool,
) -> Result<Vec<crate::model::style::FontFaceToken<'a>>> {
    let token_count = if has_token_count {
        reader.compact_u32()? as usize
    } else {
        1
    };
    let mut tokens = Vec::new();
    tokens
        .try_reserve(token_count)
        .map_err(|_| DecodeError::Malformed("font-face token list too large"))?;
    for _ in 0..token_count {
        tokens.push(decode_font_face_token(reader)?);
    }
    Ok(tokens)
}

pub(crate) fn font_family<'a>(tokens: &[crate::model::style::FontFaceToken<'a>]) -> &'a str {
    tokens
        .first()
        .and_then(|token| token.iter().find(|(key, _)| *key == "font-family"))
        .map_or("", |(_, value)| *value)
}
