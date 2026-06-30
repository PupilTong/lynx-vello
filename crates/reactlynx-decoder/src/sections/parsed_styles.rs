//! `PARSED_STYLES` section decoder.

use crate::{
    error::{DecodeError, Result},
    model::{ParsedStyles, TemplateBundle, style::decode_parsed_style_block},
    reader::Reader,
};

const FIBER_ARCH: u8 = 1;

#[derive(Debug, Clone, Copy)]
struct StyleRange<'a> {
    key: &'a str,
    start: usize,
}

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // C++ gates PARSED_STYLES on arch_option_ == FIBER_ARCH.
    // core/template_bundle/template_codec/binary_decoder/lynx_binary_base_template_reader_impl.cc:488
    if bundle.compile_options.arch_option != FIBER_ARCH {
        return Err(DecodeError::Malformed(
            "PARSED_STYLES requires fiber arch_option",
        ));
    }

    // ElementBinaryReader::DecodeStringKeyRouter leaves descriptor_offset at
    // the byte immediately after the router.
    // core/template_bundle/template_codec/binary_decoder/element_binary_reader.cc:861
    let ranges = decode_string_key_router(reader)?;
    let base = reader.pos();
    let mut entries = Vec::new();
    entries
        .try_reserve(ranges.len())
        .map_err(|_| DecodeError::Malformed("parsed styles too large"))?;
    for range in ranges {
        let start = base
            .checked_add(range.start)
            .ok_or(DecodeError::Malformed("parsed styles start overflow"))?;
        if start > reader.len() {
            return Err(DecodeError::Malformed("parsed styles range out of bounds"));
        }
        let mut style_reader = reader.sub(start, reader.len())?;
        let block = decode_parsed_style_block(&mut style_reader, &bundle.compile_options)?;
        entries.push((range.key, block));
    }
    bundle.parsed_styles = Some(ParsedStyles { entries });
    Ok(())
}

fn decode_string_key_router<'a>(reader: &mut Reader<'a>) -> Result<Vec<StyleRange<'a>>> {
    let count = reader.compact_u32()? as usize;
    let mut ranges = Vec::new();
    ranges
        .try_reserve(count)
        .map_err(|_| DecodeError::Malformed("parsed styles router too large"))?;
    for _ in 0..count {
        ranges.push(StyleRange {
            key: reader.lstr()?,
            start: reader.compact_u32()? as usize,
        });
    }
    Ok(ranges)
}
