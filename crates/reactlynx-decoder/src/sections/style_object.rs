//! `STYLE_OBJECT` section decoder.

use crate::{
    error::{DecodeError, Result},
    model::{
        CssAttributes, FontFaceEntry, StyleObjects, TemplateBundle,
        style::{decode_css_attributes, decode_css_keyframes_token},
    },
    reader::Reader,
    sections::css::{decode_font_face_list, font_family},
};

const SECTION_COUNT: u32 = 3;

#[derive(Debug, Clone, Copy)]
struct StyleObjectRange {
    start: usize,
    end: usize,
}

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // TemplateBinaryReader::DecodeStyleObjects gates this section on
    // enable_simple_styling before decoding.
    // core/template_bundle/template_codec/binary_decoder/template_binary_reader.cc:121
    if !bundle.compile_options.enable_simple_styling {
        return Ok(());
    }
    let section_count = reader.compact_u32()?;
    if section_count != SECTION_COUNT {
        return Err(DecodeError::Malformed(
            "STYLE_OBJECT section_count must be 3",
        ));
    }

    let objects = decode_objects_subsection(reader, bundle)?;
    let keyframes = decode_keyframes_subsection(reader, bundle)?;
    let font_faces = decode_font_faces_subsection(reader)?;
    bundle.style_objects = Some(StyleObjects {
        objects,
        keyframes,
        font_faces,
    });
    Ok(())
}

fn decode_route(reader: &mut Reader<'_>) -> Result<(Vec<StyleObjectRange>, usize)> {
    // C++ DecodeStyleObjectRoute records style_objects_section_range_.start
    // immediately after the route.
    // core/template_bundle/template_codec/binary_decoder/lynx_binary_base_css_reader.cc:797
    let count = reader.compact_u32()? as usize;
    let mut ranges = Vec::new();
    ranges
        .try_reserve(count)
        .map_err(|_| DecodeError::Malformed("style-object route too large"))?;
    for _ in 0..count {
        ranges.push(StyleObjectRange {
            start: reader.compact_u32()? as usize,
            end: reader.compact_u32()? as usize,
        });
    }
    Ok((ranges, reader.pos()))
}

fn seek_after_subsection(
    reader: &mut Reader<'_>,
    base: usize,
    ranges: &[StyleObjectRange],
) -> Result<()> {
    let end = ranges
        .iter()
        .map(|range| range.end)
        .max()
        .unwrap_or_default();
    reader.seek(base.checked_add(end).ok_or(DecodeError::Malformed(
        "style-object subsection end overflow",
    ))?)
}

fn sub_reader<'a>(reader: &Reader<'a>, base: usize, range: StyleObjectRange) -> Result<Reader<'a>> {
    let start = base
        .checked_add(range.start)
        .ok_or(DecodeError::Malformed("style-object start overflow"))?;
    let end = base
        .checked_add(range.end)
        .ok_or(DecodeError::Malformed("style-object end overflow"))?;
    if start > end || end > reader.len() {
        return Err(DecodeError::Malformed("style-object range out of bounds"));
    }
    reader.sub(start, end)
}

fn decode_objects_subsection<'a>(
    reader: &mut Reader<'a>,
    bundle: &TemplateBundle<'a>,
) -> Result<Vec<CssAttributes<'a>>> {
    let (ranges, base) = decode_route(reader)?;
    let mut objects = Vec::new();
    objects
        .try_reserve(ranges.len())
        .map_err(|_| DecodeError::Malformed("style objects too large"))?;
    for &range in &ranges {
        let mut object_reader = sub_reader(reader, base, range)?;
        objects.push(decode_css_attributes(
            &mut object_reader,
            true,
            false,
            bundle.compile_options.target_sdk,
        )?);
    }
    seek_after_subsection(reader, base, &ranges)?;
    Ok(objects)
}

fn decode_keyframes_subsection<'a>(
    reader: &mut Reader<'a>,
    bundle: &TemplateBundle<'a>,
) -> Result<Vec<(&'a str, crate::model::CssKeyframesToken<'a>)>> {
    let (ranges, base) = decode_route(reader)?;
    let mut keyframes = Vec::new();
    keyframes
        .try_reserve(ranges.len())
        .map_err(|_| DecodeError::Malformed("style-object keyframes too large"))?;
    for &range in &ranges {
        let mut keyframe_reader = sub_reader(reader, base, range)?;
        let name = keyframe_reader.lstr()?;
        let token = decode_css_keyframes_token(&mut keyframe_reader, &bundle.compile_options)?;
        keyframes.push((name, token));
    }
    seek_after_subsection(reader, base, &ranges)?;
    Ok(keyframes)
}

fn decode_font_faces_subsection<'a>(reader: &mut Reader<'a>) -> Result<Vec<FontFaceEntry<'a>>> {
    let (ranges, base) = decode_route(reader)?;
    let mut font_faces = Vec::new();
    font_faces
        .try_reserve(ranges.len())
        .map_err(|_| DecodeError::Malformed("style-object font faces too large"))?;
    for &range in &ranges {
        let mut font_face_reader = sub_reader(reader, base, range)?;
        let _name = font_face_reader.lstr()?;
        let tokens = decode_font_face_list(&mut font_face_reader, true)?;
        let family = font_family(&tokens);
        font_faces.push(FontFaceEntry { family, tokens });
    }
    seek_after_subsection(reader, base, &ranges)?;
    Ok(font_faces)
}
