//! The markdown element's UI methods, each answering a data record
//! `element-papi.ts`'s `decodeMethodData` builds the method's object from.
//! See the module documentation of [`super`] for which are built and how
//! each follows web-core, native and the typings.

use dom::{NodeId, Point2D, Vector2D};
use rustc_hash::FxHashMap;
use serde_json::{Map, Value};

use super::build::{IMAGE_TAG, Newlines, html_name};
use super::parse::{slice_utf16, utf16_len};
use super::rendered::{self, Paragraph, Step};
use super::{CONTENT_ATTRIBUTE, CONTENT_ID_ATTRIBUTE, attribute};
use crate::main::record::write_record_field;
use crate::main::tree::scroll_container::object;
use crate::main::tree::{InvalidParams, LynxDocument};

/// `params` as a JSON object, with JSON `null` — what the realm sends for
/// params it cannot serialize — read as no params, as web-core's
/// `params?.start` reads an absent object. Anything else is code 4.
fn optional_object(params: &str) -> Result<Map<String, Value>, InvalidParams> {
    match serde_json::from_str(params) {
        Ok(Value::Object(params)) => Ok(params),
        Ok(Value::Null) => Ok(Map::new()),
        _ => Err(InvalidParams),
    }
}

/// `params[key]` as web-core's `params?.key ?? fallback` reads it: absent or
/// `null` is `None`, a number is itself, anything else is code 4.
fn optional_number(params: &Map<String, Value>, key: &str) -> Result<Option<f64>, InvalidParams> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_f64().map(Some).ok_or(InvalidParams),
    }
}

/// A non-negative offset as a `usize`, truncated, saturating past the end.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to zero first; a float-to-int cast saturates, past every string's length"
)]
fn to_offset(value: f64) -> usize {
    value.max(0.0).trunc() as usize
}

/// `getContent({start?, end?})`: web-core's `getContent` (`XMarkdown.ts:240-246`),
/// the `content` attribute from `start` to `end` *inclusive*, in UTF-16 code
/// units. `start` defaults to 0 and is clamped to it; `end` defaults to the
/// last unit; an `end` before `start` is the empty string. A `start` or `end`
/// that is present and not a number is code 4.
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
    let length = utf16_len(content) as f64;
    let start = start.unwrap_or(0.0).max(0.0);
    let end = (end.unwrap_or(length - 1.0) + 1.0).min(length);
    let slice = if end >= start {
        slice_utf16(content, to_offset(start), to_offset(end)).0
    } else {
        ""
    };
    let mut record = String::new();
    write_record_field(&mut record, slice);
    Ok(record)
}

/// `getImages()`: the `src` of every image in the shadow root, in tree
/// order, an empty one left out (web-core's `getImages`, `XMarkdown.ts:229-238`).
/// The record is the count and then each URL; `element-papi.ts` answers
/// `{images}`, the typings' shape, where web-core answers the bare array.
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
            images.push(src.to_owned());
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

/// `getParseResult({tags})`: for each tag name, every element of that name
/// in the shadow root, in tree order, with the range of the rendered text it
/// holds — web-core's `getParseResult` (`XMarkdown.ts:390-424`), whose
/// `calcOffset` takes the first and last text node inside the element in the
/// root's `textContent`. An element holding no text answers `{0, 0}`, which
/// is web-core's `-1` clamped to the text.
///
/// - `tags` must be a non-empty array, else code 4 (native, `LynxUIMarkdown.m:281-291`; web-core
///   throws on a missing one). An item that is not a string is skipped, as native skips it. A name
///   is matched as an HTML tag name, ASCII case-insensitively; `img` names the engine's images, and
///   the engine's own line and marker elements match no name.
/// - The answer is `{id, result}`, native's and the typings' shape, where web-core answers the bare
///   `result` map; `id` is the `content-id` attribute, `""` without one, as native's.
///
/// The record is `id`, the number of distinct tags, and per tag its name,
/// its number of ranges and then `start`, `end` per range.
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
    // Per element found, in tree order: which lowered name it matched, and
    // its first and last text offsets.
    let mut found: Vec<(usize, Option<(usize, usize)>)> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut entered: Vec<bool> = Vec::new();
    let mut position = 0;
    rendered::walk(document, root, newlines, &mut |step| match step {
        Step::Enter(id) => {
            let name = document
                .get(id)
                .and_then(dom::Node::tag_name)
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
    write_record_field(&mut record, id);
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

/// `getTextBoundingRect({start?, end?, indexType?})`: the boxes the rendered
/// text `[start, end)` occupies, one per line per paragraph it spans, in
/// viewport CSS px — web-core's `getTextBoundingRect` (`XMarkdown.ts:282-331`)
/// with the per-line `boxes` native and the `text` component answer
/// ([`super::super::text`]), in the record that component writes.
///
/// - Offsets are rendered-text offsets, the ones `getParseResult` answers in. `start` is clamped to
///   0 and defaults to it, `end` defaults to `start` and is clamped to it (web-core). A `start` or
///   `end` that is present and not a number is code 4.
/// - `indexType: "source"` reads them as offsets into the `content` attribute instead, mapped to
///   rendered offsets by web-core's greedy prefix walk (`mapSourceRangeToCharRange`,
///   `XMarkdown.ts:103-121`).
/// - The range is split across the paragraphs it spans, each part asked of
///   [`dom::Document::text_range_boxes`] in that paragraph's source units, and each box moved into
///   viewport space as the `text` component moves its own. A range covering a whole paragraph
///   includes its images and inline views; a collapsed one is one zero-width box at the start of
///   the paragraph holding it, the `text` component's quirk.
/// - With neither `start` nor `end`, web-core measures the selection; there is no selection model
///   here, so there is nothing to measure. That, and a range that shows no content, is `Ok(None)`:
///   code 8, the `text` component's answer when it finds nothing.
pub(super) fn get_text_bounding_rect(
    document: &LynxDocument,
    element: NodeId,
    root: NodeId,
    newlines: &FxHashMap<NodeId, Newlines>,
    params: &str,
) -> Result<Option<String>, InvalidParams> {
    let params = optional_object(params)?;
    if !params.contains_key("start") && !params.contains_key("end") {
        // No selection model: nothing is selected.
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
            // A boundary outside the paragraph takes in everything up to its
            // edge, its atoms included, as a DOM range from an earlier or to
            // a later text node does.
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

/// `getCharIndexByPoint({x, y, indexType?})`: the rendered-text offset of the
/// character under the point, native's method (`LynxUIMarkdown.m:194-219`;
/// web-core has none).
///
/// - `x` and `y` must be numbers, else code 4. They are relative to the element's border box, as
///   native's are before it subtracts the border and padding to reach its text view's space; the
///   point is that origin plus `(x, y)` in viewport space.
/// - The paragraph whose box holds the point answers the source unit under it
///   ([`dom::Document::text_index_at_point`]), which is turned into a rendered-text offset — an
///   image's or inline view's the offset it sits at — or, under `indexType: "source"`, into the
///   offset into `content` web-core's greedy prefix walk matched that character at.
/// - `Ok(None)`, code 8, when the point hits no character (native answers 1, "can not find char
///   index"), or the element has no box.
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

/// The rendered offset of the character `paragraph` shows under `point`, in
/// viewport space.
fn char_at(document: &LynxDocument, paragraph: &Paragraph, point: Point2D<f32>) -> Option<usize> {
    let rect = document.bounding_client_rect(paragraph.element)?;
    if !rect.contains(point) {
        return None;
    }
    let origin = content_origin(document, paragraph.element)?;
    let unit = document.text_index_at_point(paragraph.element, point - origin)?;
    Some(paragraph.start + paragraph.offset_of(unit))
}

/// Where `element`'s content box starts in viewport space: its
/// `boundingClientRect` origin plus its border and padding, the offset the
/// `text` component adds to a paragraph's own coordinates.
pub(super) fn content_origin(document: &LynxDocument, element: NodeId) -> Option<Vector2D<f32>> {
    let rect = document.bounding_client_rect(element)?;
    let layout = document.rounded_layout(element)?;
    Some(Vector2D::new(
        rect.origin.x + layout.border.left + layout.padding.left,
        rect.origin.y + layout.border.top + layout.padding.top,
    ))
}
