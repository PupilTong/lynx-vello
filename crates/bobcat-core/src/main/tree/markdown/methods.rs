//! The markdown element's UI methods, each answering a data record
//! `element-papi.ts`'s `decodeMethodData` builds the method's object from.
//! See the module documentation of [`super`] for which are built and how
//! each follows web-core, native and the typings.

use rustc_hash::FxHashMap;
use serde_json::{Map, Value};
use terminus::link::{NodeId, Point2D, Vector2D};

use super::build::{IMAGE_TAG, Newlines, html_name};
use super::parse::{slice_utf16, utf16_len};
use super::rendered::{self, Paragraph, Step};
use super::{CONTENT_ATTRIBUTE, CONTENT_ID_ATTRIBUTE, attribute};
use crate::main::record::write_record_field;
use crate::main::tree::scroll_container::object;
use crate::main::tree::{InvalidParams, LynxDocument};

fn optional_object(params: &str) -> Result<Map<String, Value>, InvalidParams> {
    match serde_json::from_str(params) {
        Ok(Value::Object(params)) => Ok(params),
        Ok(Value::Null) => Ok(Map::new()),
        _ => Err(InvalidParams),
    }
}

fn optional_number(params: &Map<String, Value>, key: &str) -> Result<Option<f64>, InvalidParams> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_f64().map(Some).ok_or(InvalidParams),
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to zero first; a float-to-int cast saturates, past every string's length"
)]
fn to_offset(value: f64) -> usize {
    value.max(0.0).trunc() as usize
}

pub(super) fn get_content(
    document: &LynxDocument,
    element: NodeId,
    params: &str,
) -> Result<String, InvalidParams> {
    let params = optional_object(params)?;
    let start = optional_number(&params, "start")?;
    let end = optional_number(&params, "end")?;
    let content = attribute(document, element, CONTENT_ATTRIBUTE).unwrap_or_default();
    #[expect(
        clippy::cast_precision_loss,
        reason = "a string's UTF-16 length is far below 2^52"
    )]
    let length = utf16_len(&content) as f64;
    let start = start.unwrap_or(0.0).max(0.0);
    let end = (end.unwrap_or(length - 1.0) + 1.0).min(length);
    let slice = if end >= start {
        slice_utf16(&content, to_offset(start), to_offset(end)).0
    } else {
        ""
    };
    let mut record = String::new();
    write_record_field(&mut record, slice);
    Ok(record)
}

pub(super) fn get_images(document: &LynxDocument, root: NodeId) -> String {
    let mut images = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(node) = document.get(id) else {
            continue;
        };
        if node.tag_name() == Some(IMAGE_TAG)
            && let Some(src) = node.attribute("src").filter(|src| !src.is_empty())
        {
            images.push(src.into_owned());
        }
        stack.extend(node.child_ids().iter().rev().copied());
    }
    let mut record = String::new();
    write_record_field(&mut record, &images.len().to_string());
    for image in &images {
        write_record_field(&mut record, image);
    }
    record
}

pub(super) fn get_parse_result(
    document: &LynxDocument,
    element: NodeId,
    root: NodeId,
    newlines: &FxHashMap<NodeId, Newlines>,
    params: &str,
) -> Result<String, InvalidParams> {
    let params = object(params)?;
    let tags = params
        .get("tags")
        .and_then(Value::as_array)
        .filter(|tags| !tags.is_empty())
        .ok_or(InvalidParams)?;
    let mut names: Vec<&str> = Vec::new();
    for tag in tags.iter().filter_map(Value::as_str) {
        if !names.contains(&tag) {
            names.push(tag);
        }
    }
    let lowered: Vec<String> = names.iter().map(|name| name.to_ascii_lowercase()).collect();
    let mut found: Vec<(usize, Option<(usize, usize)>)> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut entered: Vec<bool> = Vec::new();
    let mut position = 0;
    rendered::walk(document, root, newlines, &mut |step| match step {
        Step::Enter(id) => {
            let name = document
                .get(id)
                .and_then(terminus::link::Node::tag_name)
                .and_then(html_name);
            let index = name.and_then(|name| lowered.iter().position(|tag| tag == name));
            if let Some(index) = index {
                open.push(found.len());
                found.push((index, None));
            }
            entered.push(index.is_some());
        }
        Step::Text(text) => {
            let end = position + utf16_len(text);
            for &index in &open {
                let range = &mut found[index].1;
                *range = Some((range.map_or(position, |(start, _)| start), end));
            }
            position = end;
        }
        Step::Leave => {
            if entered.pop() == Some(true) {
                open.pop();
            }
        }
    });
    let mut record = String::new();
    let id = attribute(document, element, CONTENT_ID_ATTRIBUTE).unwrap_or_default();
    write_record_field(&mut record, &id);
    write_record_field(&mut record, &names.len().to_string());
    for (index, name) in names.iter().enumerate() {
        let ranges: Vec<(usize, usize)> = found
            .iter()
            .filter(|(matched, _)| *matched == index)
            .map(|(_, range)| range.unwrap_or((0, 0)))
            .collect();
        write_record_field(&mut record, name);
        write_record_field(&mut record, &ranges.len().to_string());
        for (start, end) in ranges {
            write_record_field(&mut record, &start.to_string());
            write_record_field(&mut record, &end.to_string());
        }
    }
    Ok(record)
}

pub(super) fn get_text_bounding_rect(
    document: &LynxDocument,
    element: NodeId,
    root: NodeId,
    newlines: &FxHashMap<NodeId, Newlines>,
    params: &str,
) -> Result<Option<String>, InvalidParams> {
    let params = optional_object(params)?;
    if !params.contains_key("start") && !params.contains_key("end") {
        return Ok(None);
    }
    let start = optional_number(&params, "start")?.unwrap_or(0.0).max(0.0);
    let end = optional_number(&params, "end")?.unwrap_or(start).max(start);
    let (mut start, mut end) = (to_offset(start), to_offset(end));
    if params.get("indexType").and_then(Value::as_str) == Some("source") {
        let source: Vec<u16> = attribute(document, element, CONTENT_ATTRIBUTE)
            .unwrap_or_default()
            .encode_utf16()
            .collect();
        let text = rendered::text_units(document, root, newlines);
        let mapped_start = rendered::rendered_prefix(&source, &text, start);
        let mapped_end = rendered::rendered_prefix(&source, &text, end);
        (start, end) = (mapped_start, mapped_end.max(mapped_start));
    }
    let mut boxes = Vec::new();
    for paragraph in rendered::paragraphs(document, root, newlines) {
        let paragraph_end = paragraph.start + paragraph.len();
        let units = if start == end {
            if !(paragraph.start..=paragraph_end).contains(&start) {
                continue;
            }
            let unit = paragraph.unit_at(start - paragraph.start);
            (unit, unit)
        } else if (start < paragraph_end && end > paragraph.start)
            || (start <= paragraph.start && end >= paragraph_end)
        {
            let unit_start = if start <= paragraph.start {
                0
            } else {
                paragraph.unit_at(start - paragraph.start)
            };
            let unit_end = if end >= paragraph_end {
                paragraph.units()
            } else {
                paragraph.unit_at(end - paragraph.start)
            };
            (unit_start, unit_end)
        } else {
            continue;
        };
        let Some(origin) = content_origin(document, paragraph.element) else {
            continue;
        };
        let (Ok(unit_start), Ok(unit_end)) = (u32::try_from(units.0), u32::try_from(units.1))
        else {
            continue;
        };
        if let Some(found) = document.text_range_boxes(paragraph.element, unit_start, unit_end) {
            boxes.extend(found.into_iter().map(|rect| rect.translate(origin)));
        }
        if start == end {
            break;
        }
    }
    if boxes.is_empty() {
        return Ok(None);
    }
    let mut record = String::new();
    write_record_field(&mut record, &boxes.len().to_string());
    for rect in boxes {
        for value in [
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        ] {
            write_record_field(&mut record, &value.to_string());
        }
    }
    Ok(Some(record))
}

pub(super) fn get_char_index_by_point(
    document: &LynxDocument,
    element: NodeId,
    root: NodeId,
    newlines: &FxHashMap<NodeId, Newlines>,
    params: &str,
) -> Result<Option<String>, InvalidParams> {
    let params = object(params)?;
    let x = params
        .get("x")
        .and_then(Value::as_f64)
        .ok_or(InvalidParams)?;
    let y = params
        .get("y")
        .and_then(Value::as_f64)
        .ok_or(InvalidParams)?;
    let Some(host) = document.bounding_client_rect(element) else {
        return Ok(None);
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a CSS px coordinate, which layout keeps in f32"
    )]
    let point = Point2D::new(host.origin.x + x as f32, host.origin.y + y as f32);
    let Some(offset) = rendered::paragraphs(document, root, newlines)
        .iter()
        .find_map(|paragraph| char_at(document, paragraph, point))
    else {
        return Ok(None);
    };
    let index = if params.get("indexType").and_then(Value::as_str) == Some("source") {
        let source: Vec<u16> = attribute(document, element, CONTENT_ATTRIBUTE)
            .unwrap_or_default()
            .encode_utf16()
            .collect();
        let text = rendered::text_units(document, root, newlines);
        let Some(index) = rendered::source_offset(&source, &text, offset) else {
            return Ok(None);
        };
        index
    } else {
        offset
    };
    let mut record = String::new();
    write_record_field(&mut record, &index.to_string());
    Ok(Some(record))
}

fn char_at(document: &LynxDocument, paragraph: &Paragraph, point: Point2D<f32>) -> Option<usize> {
    let rect = document.bounding_client_rect(paragraph.element)?;
    if !rect.contains(point) {
        return None;
    }
    let origin = content_origin(document, paragraph.element)?;
    let unit = document.text_index_at_point(paragraph.element, point - origin)?;
    Some(paragraph.start + paragraph.offset_of(unit))
}

pub(super) fn content_origin(document: &LynxDocument, element: NodeId) -> Option<Vector2D<f32>> {
    let rect = document.bounding_client_rect(element)?;
    let layout = document.rounded_layout(element)?;
    Some(Vector2D::new(
        rect.origin.x + layout.border.left + layout.padding.left,
        rect.origin.y + layout.border.top + layout.padding.top,
    ))
}
