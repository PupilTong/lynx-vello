//! The `text` tag's attribute-to-CSS limits and defaults: what color a run wears,
//! what may generate a box inside one, and whether a pointer can select it
//! (`text-selection`, as `user-select`); and the `text` component, which
//! answers three UI methods.
//!
//! The other half of Lynx text — how a run reaches the engine and where it
//! lays out — is [`super::raw_text`], which owns the `raw-text` generated-content rule and
//! the rules that dissolve a carrier into the `text` it is written inside.
//!
//! # UI methods
//!
//! The Lynx docs give `text` three (`lynxjs.org/api/elements/built-in/text`,
//! the typings in `lynx/js_libraries/types/types/common/element/text.d.ts:220-258`,
//! `@since 2.18`); web-core's `XText` has none of them. All three are built:
//!
//! - `getTextBoundingRect({start, end})` ([`text_bounding_rect`]), over
//!   [`terminus::link::Document::text_range_boxes`], with iOS's per-line answer
//!   (`lynx/platform/darwin/ios/lynx/ui/text/LynxUIText.m:790-807`, `LynxTextView.m:205-235`).
//! - `getSelectedText()` ([`selected_text`]): the selected text of the document's selection when it
//!   lies in this element, `""` otherwise (`UIText.java:417-427`, `LynxUIText.m:809-812`).
//! - `setTextSelection({startX, startY, endX, endY, ...})` ([`set_text_selection`]): selects the
//!   text between two points of the element and answers its boxes and the two handle positions
//!   (`docs/editable-text-design.md`, "Read-only text selection").
//!
//! # `text-selection`
//!
//! A UA rule, as web-core's `x-text.css:147-181` writes it: every `text`, `inline-text` and
//! `inline-truncation` is `user-select: none`, and a `text` whose `text-selection` is present and
//! not an ASCII case-insensitive `"false"` is `user-select: auto` with the text elements inside it,
//! the parent's used value, which css-ui-4 resolves to `text` under an ordinary page
//! ([`UA_RULES`]). `terminus` reads the used value off the computed style
//! ([`terminus::link::Document::is_user_selectable`]), so a pointer selects that text without
//! focusing it (`crate::main::runtime`'s editing module), and an author's own `user-select` on any
//! element does the same. Native parses the attribute as a boolean (`UIText.java:256`,
//! `LynxUIText.m:719`); web-core's selector is `[text-selection]`, presence alone, so there
//! `text-selection="false"` selects too, and the boolean reading is kept.
//!
//! # Events
//!
//! `text` and `inline-text` fire Lynx's `selectionchange` (`bindselectionchange`)
//! with the detail `{start, end, direction}`, web-core's
//! (`lynx-stack/packages/web-platform/web-elements/src/elements/XText/XTextSelectionEvents.ts`,
//! on both `XText` and `InlineText`; `inline-truncation` has none), which
//! native Android (`AndroidText.java:482-490`) and iOS
//! (`LynxTextView.m:319-328`) share:
//!
//! - `start` and `end` are the smaller and the larger of the document selection's two boundary
//!   points, as offsets into the element's whole text content — every text node of its subtree, in
//!   tree order, in UTF-16 units — and `direction` is `"forward"` when the anchor is at or before
//!   the focus, `"backward"` otherwise.
//! - A collapsed selection, one with either end outside the element, and no selection at all are
//!   the empty record `{start: -1, end: -1, direction: "forward"}`.
//! - It is derived from the W3C `selectionchange`: the runtime fires that one at the document node
//!   and then calls [`report_selection_changes`], which queues this one at each `text` and
//!   `inline-text` whose record changed, for the same entry to deliver after the W3C one. web-core
//!   listens for the document's `selectionchange` in the same way.
//! - Only while something listens for `selectionchange` on the element
//!   ([`terminus::link::Document::would_reach_listener`]), as web-core attaches its document
//!   listener only while the event is bound.
//! - A record equal to the last one sent to the element is not sent again, and the empty record is
//!   sent only to an element whose last record was not empty: web-core's signature check.
//! - Not bubbling, composed ([`ComponentEvents::queue_detailed`]).
//!
//! The document's selection lies in a focused editing host or in a
//! selectable element ([`terminus::link::Document::selection`]), and
//! [`text_selection_in`] maps its two boundary points into an element's
//! text content the way web-core's `range.selectNodeContents(root);
//! range.setEnd(node, offset); range.toString().length` does:
//!
//! - In an editing host — a `text` with `contenteditable`, or an element inside one — the points
//!   are offsets into the host's value, its direct text children. So a `text` around a host that is
//!   a nested `inline-text` reports offsets that count the text before the host. There is one
//!   difference from web-core: an `inline-text` *inside* the host never holds a boundary, because
//!   the host's value is its direct text children alone, so it reports the empty record; a
//!   browser's selection can enter that `inline-text`, and web-core fires offsets at it.
//! - In a selectable element — a `text-selection` `text` selected by a pointer or by
//!   `setTextSelection` — the points are source units of its paragraph, every text and generated
//!   run counted and an atomic inline box counting one. The element's record counts its text
//!   content, every run but no atom ([`terminus::link::Document::text_content_offset`], web-core's
//!   `range.toString().length`; native counts the placeholder character,
//!   `AndroidText.java:482-490`). A `text` or `inline-text` inside it reports when both points lie
//!   within its own source range ([`terminus::link::Document::inline_source_range`]), as offsets
//!   into its own content, and the empty record otherwise.
//!
//! The component holds no state and has no lifecycle callback; the last
//! record sent to each element is the runtime's, in a table it passes to
//! [`report_selection_changes`] ([`SelectionReports`]). Its definition, and
//! the two behaviourless ones beside it for `inline-text` and
//! `inline-truncation`, carry one fact besides those: the three tags are
//! Lynx text elements ([`terminus::link::CustomElement::is_lynx_text`]), which is what
//! lays each out as a Lynx paragraph or a run of one. `:defined` already
//! matches every element in `terminus`, and the `raw-text` rules are selector rules
//! on the tag name, which a definition does not touch.

pub(super) const UA_RULES: &str = r#"
@property --lynx-text-maxline { syntax: "<integer>"; inherits: false; initial-value: -1; }
@property --lynx-text-maxlength { syntax: "<integer>"; inherits: false; initial-value: -1; }
@property --lynx-tail-color-convert { syntax: "<integer>"; inherits: false; initial-value: 0; }
@property --lynx-inline-truncation { syntax: "<integer>"; inherits: false; initial-value: 0; }
text { color: initial; }
text {
  --lynx-text-maxline: attr(text-maxline number, -1);
  --lynx-text-maxlength: attr(text-maxlength number, -1);
}
text[tail-color-convert="true"] { --lynx-tail-color-convert: 1; }
text[text] { content: attr(text); }
text[text-overflow="ellipsis"] { text-overflow: ellipsis; }
text[text-overflow="clip"] { text-overflow: clip; }
inline-image, :not(text) > inline-truncation { display: none; }
text > :where(:not(text, inline-text, inline-truncation)) { display: none; }
text > wrapper { display: contents; }
text > view, text > image { display: flex; }
text > inline-truncation { --lynx-inline-truncation: 1; }
text > text, text > wrapper > text { color: inherit; }
text > image, text > wrapper > image, inline-text > image, inline-text > wrapper > image, text > inline-truncation > image, text > inline-truncation > wrapper > image { padding: 0 !important; }
text, inline-text, inline-truncation { user-select: none; }
text[text-selection]:not([text-selection="false" i]), text[text-selection]:not([text-selection="false" i]) :is(text, inline-text, inline-truncation) { user-select: auto; }
"#;

pub(super) const CSS_INHERITANCE_RULES: &str = "\
view > text, view > wrapper, view > wrapper > text { color: inherit; text-decoration: inherit; }
";

use std::ops::Range;

use rustc_hash::FxHashMap;
use serde_json::{Map, Value};
use smallvec::SmallVec;
use terminus::link::editing::{Command, Direction, DocumentSelection, Granularity};
use terminus::link::{CustomElement, MethodCall, MethodError, MethodOutcome, NodeId};

use super::scroll_container::object;
use super::{ComponentDetail, ComponentEvents, InvalidParams, LynxDocument, SELECTION_CHANGE};
use crate::main::record::write_record_field;

pub(super) const TEXT_TAG: &str = "text";

pub(crate) fn is_text(document: &LynxDocument, node: NodeId) -> bool {
    document
        .get(node)
        .and_then(terminus::link::Node::tag_name)
        .is_some_and(|tag| tag == TEXT_TAG)
}

pub(super) fn define(document: &mut LynxDocument) {
    document.define(TEXT_TAG, Box::new(Text));
    document.define(INLINE_TEXT_TAG, Box::new(TextRun));
    document.define(INLINE_TRUNCATION_TAG, Box::new(TextRun));
}

const INLINE_TEXT_TAG: &str = "inline-text";

const INLINE_TRUNCATION_TAG: &str = "inline-truncation";

struct TextRun;

impl CustomElement<()> for TextRun {
    fn is_lynx_text(&self) -> bool {
        true
    }
}

struct Text;

impl CustomElement<()> for Text {
    fn is_lynx_text(&self) -> bool {
        true
    }

    fn invoke(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        match call.name {
            "getTextBoundingRect" => match text_bounding_rect(document, element, call.params) {
                Ok(Some(fields)) => MethodOutcome::Data(fields),
                Ok(None) => MethodOutcome::Failed(MethodError::Operation),
                Err(InvalidParams) => MethodOutcome::Failed(MethodError::InvalidParams),
            },
            "getSelectedText" => MethodOutcome::Data(selected_text(document, element)),
            "setTextSelection" => set_text_selection(document, element, call.params),
            _ => MethodOutcome::NotFound,
        }
    }
}

#[derive(Default)]
pub(crate) struct SelectionReports(FxHashMap<NodeId, (Range<u32>, bool)>);

fn reports_selection(node: &terminus::link::Node<()>) -> bool {
    matches!(node.tag_name(), Some(TEXT_TAG | INLINE_TEXT_TAG))
}

pub(crate) fn report_selection_changes(
    document: &LynxDocument,
    reports: &mut SelectionReports,
    events: &ComponentEvents,
) {
    let selection = document.selection();
    let mut candidates: SmallVec<[NodeId; 4]> = SmallVec::new();
    match selection {
        Some(selection) if document.focused() == Some(selection.host) => {
            let mut current = Some(selection.host);
            while let Some(id) = current {
                let Some(node) = document.get(id) else {
                    break;
                };
                if reports_selection(node) {
                    candidates.push(id);
                }
                current = node.parent_id();
            }
        }
        Some(selection) => {
            let mut stack: SmallVec<[NodeId; 16]> = SmallVec::new();
            stack.push(selection.host);
            while let Some(id) = stack.pop() {
                let Some(node) = document.get(id) else {
                    continue;
                };
                if reports_selection(node) {
                    candidates.push(id);
                }
                stack.extend(node.child_ids().iter().rev().copied());
            }
        }
        None => {}
    }
    for &element in reports.0.keys() {
        if !candidates.contains(&element) {
            candidates.push(element);
        }
    }
    for element in candidates {
        if document.get(element).is_none()
            || !document.would_reach_listener(element, SELECTION_CHANGE, false, true)
        {
            reports.0.remove(&element);
            continue;
        }
        let record =
            selection.and_then(|selection| text_selection_in(document, element, selection));
        match (reports.0.get(&element), record) {
            (None, None) => {}
            (Some(last), Some(record)) if *last == record => {}
            (_, Some((range, forward))) => {
                reports.0.insert(element, (range.clone(), forward));
                events.queue_detailed(
                    element,
                    SELECTION_CHANGE,
                    ComponentDetail::TextSelection {
                        range: Some(range),
                        forward,
                    },
                );
            }
            (Some(_), None) => {
                reports.0.remove(&element);
                events.queue_detailed(
                    element,
                    SELECTION_CHANGE,
                    ComponentDetail::TextSelection {
                        range: None,
                        forward: true,
                    },
                );
            }
        }
    }
}

fn text_selection_in(
    document: &LynxDocument,
    element: NodeId,
    selection: DocumentSelection,
) -> Option<(Range<u32>, bool)> {
    if selection.anchor == selection.focus {
        return None;
    }
    let (anchor, focus) = if document.focused() == Some(selection.host) {
        let mut current = Some(selection.host);
        while current != Some(element) {
            current = document
                .get(current?)
                .and_then(terminus::link::Node::parent_id);
        }
        (
            text_offset(document, element, selection.host, selection.anchor)?,
            text_offset(document, element, selection.host, selection.focus)?,
        )
    } else {
        selectable_offsets(document, element, selection)?
    };
    Some((
        anchor.min(focus)..anchor.max(focus),
        selection.anchor <= selection.focus,
    ))
}

fn selectable_offsets(
    document: &LynxDocument,
    element: NodeId,
    selection: DocumentSelection,
) -> Option<(u32, u32)> {
    let root = selection.host;
    let content = |unit| document.text_content_offset(root, unit);
    if element == root {
        return Some((content(selection.anchor)?, content(selection.focus)?));
    }
    let mut current = document.get(element)?.parent_id();
    while current != Some(root) {
        current = document.get(current?)?.parent_id();
    }
    let range = document.inline_source_range(element)?;
    let start = u32::try_from(range.start).ok()?;
    let end = u32::try_from(range.end).ok()?;
    let inside = |unit: u32| (start..=end).contains(&unit);
    if !inside(selection.anchor) || !inside(selection.focus) {
        return None;
    }
    let base = content(start)?;
    Some((
        content(selection.anchor)?.saturating_sub(base),
        content(selection.focus)?.saturating_sub(base),
    ))
}

fn text_offset(document: &LynxDocument, root: NodeId, host: NodeId, unit: u32) -> Option<u32> {
    let mut remaining = unit;
    let mut count: u32 = 0;
    let mut stack: SmallVec<[NodeId; 16]> = SmallVec::new();
    stack.push(root);
    while let Some(id) = stack.pop() {
        let Some(node) = document.get(id) else {
            continue;
        };
        if let Some(text) = node.text() {
            let length =
                u32::try_from(text.chars().map(char::len_utf16).sum::<usize>()).unwrap_or(u32::MAX);
            if node.parent_id() == Some(host) {
                if remaining <= length {
                    return Some(count.saturating_add(remaining));
                }
                remaining -= length;
            }
            count = count.saturating_add(length);
            continue;
        }
        stack.extend(node.child_ids().iter().rev().copied());
    }
    None
}

fn text_bounding_rect(
    document: &LynxDocument,
    text: NodeId,
    params: &str,
) -> Result<Option<String>, InvalidParams> {
    let params = object(params)?;
    let start = range_offset(&params, "start")?;
    let end = range_offset(&params, "end")?;
    if start > end {
        return Err(InvalidParams);
    }
    let Some(boxes) = viewport_boxes(document, text, start, end) else {
        return Ok(None);
    };
    let mut record = String::new();
    write_boxes(&mut record, &boxes);
    Ok(Some(record))
}

fn viewport_boxes(
    document: &LynxDocument,
    text: NodeId,
    start: u32,
    end: u32,
) -> Option<Vec<terminus::link::Rect<f32>>> {
    let element_box = document.bounding_client_rect(text)?;
    let layout = document.rounded_layout(text)?;
    let boxes = document
        .text_range_boxes(text, start, end)
        .filter(|boxes| !boxes.is_empty())?;
    let origin = terminus::link::Vector2D::new(
        element_box.origin.x + layout.border.left + layout.padding.left,
        element_box.origin.y + layout.border.top + layout.padding.top,
    );
    Some(
        boxes
            .into_iter()
            .map(|rect| rect.translate(origin))
            .collect(),
    )
}

fn write_boxes(record: &mut String, boxes: &[terminus::link::Rect<f32>]) {
    write_record_field(record, &boxes.len().to_string());
    for rect in boxes {
        for value in [
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        ] {
            write_record_field(record, &value.to_string());
        }
    }
}

fn selected_text(document: &LynxDocument, text: NodeId) -> String {
    let mut record = String::new();
    write_record_field(
        &mut record,
        &document.selected_text(text).unwrap_or_default(),
    );
    record
}

const HANDLE_RADIUS: f32 = 50.0;

fn set_text_selection(document: &mut LynxDocument, text: NodeId, params: &str) -> MethodOutcome {
    let Ok([start_x, start_y, end_x, end_y]) = selection_points(params) else {
        return MethodOutcome::Failed(MethodError::InvalidParams);
    };
    if [start_x, start_y, end_x, end_y]
        .iter()
        .any(|&value| value < 0.0)
    {
        if document.selection_host() == Some(text) {
            document.clear_selection();
        }
        return MethodOutcome::Done;
    }
    if !document.begin_selection(text) {
        return MethodOutcome::Failed(MethodError::Operation);
    }
    document.apply_editing(Command::SelectFromPoint {
        point: terminus::link::Point2D::new(start_x, start_y),
        extend: false,
    });
    document.apply_editing(Command::SelectFromPoint {
        point: terminus::link::Point2D::new(end_x, end_y),
        extend: true,
    });
    for direction in [Direction::Forward, Direction::Backward] {
        if document
            .selection()
            .is_none_or(|selection| selection.anchor != selection.focus)
        {
            break;
        }
        document.apply_editing(Command::Move {
            granularity: Granularity::Character,
            direction,
            extend: true,
        });
    }
    let Some(selection) = document
        .selection()
        .filter(|selection| selection.host == text)
    else {
        return MethodOutcome::Failed(MethodError::Operation);
    };
    let start = selection.anchor.min(selection.focus);
    let end = selection.anchor.max(selection.focus);
    let Some(boxes) = viewport_boxes(document, text, start, end) else {
        return MethodOutcome::Failed(MethodError::Operation);
    };
    let (Some(first), Some(last)) = (boxes.first(), boxes.last()) else {
        return MethodOutcome::Failed(MethodError::Operation);
    };
    let handles = [
        first.min_x(),
        first.max_y(),
        HANDLE_RADIUS,
        last.max_x(),
        last.max_y(),
        HANDLE_RADIUS,
    ];
    let mut record = String::new();
    write_boxes(&mut record, &boxes);
    for value in handles {
        write_record_field(&mut record, &value.to_string());
    }
    MethodOutcome::Data(record)
}

fn selection_points(params: &str) -> Result<[f32; 4], InvalidParams> {
    let params = object(params)?;
    let mut points = [0.0; 4];
    for (point, key) in points.iter_mut().zip(["startX", "startY", "endX", "endY"]) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a CSS px coordinate; f32 is the engine's geometry"
        )]
        {
            *point = params
                .get(key)
                .and_then(Value::as_f64)
                .ok_or(InvalidParams)? as f32;
        }
    }
    Ok(points)
}

fn range_offset(params: &Map<String, Value>, key: &str) -> Result<u32, InvalidParams> {
    let offset = params
        .get(key)
        .and_then(Value::as_f64)
        .ok_or(InvalidParams)?
        .trunc();
    if offset < 0.0 {
        return Err(InvalidParams);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a non-negative integer; one past u32::MAX saturates and is past every paragraph"
    )]
    Ok(offset as u32)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use terminus::link::stylo::color::AbsoluteColor;
    use terminus::link::stylo::values::computed::{ColorPropertyValue, Display};

    use super::super::LynxDocument;
    use super::super::test_support::{child, display, document, element_under, style_of};

    const MAX_LINES: &str = "text-maxline";
    const MAX_CHARS: &str = "text-maxlength";
    const TAIL_COLOR_CONVERT: &str = "tail-color-convert";
    const TEXT_OVERFLOW: &str = "text-overflow";

    fn set_limit(
        document: &mut LynxDocument,
        element: terminus::link::NodeId,
        name: &str,
        value: Option<&str>,
    ) {
        if let Some(value) = value {
            document.set_attribute(element, name, value);
        } else {
            document.remove_attribute(element, name);
        }
    }

    fn append_run(document: &mut LynxDocument, parent: terminus::link::NodeId, content: &str) {
        let raw = element_under(document, parent, "raw-text", "");
        document.set_attribute(raw, "text", content);
    }

    fn paragraph(content: &str) -> (LynxDocument, terminus::link::NodeId) {
        const AHEM: &[u8] = include_bytes!("../../../../seldon/tests/fixtures/Ahem.ttf");
        let mut document = document();
        assert_eq!(
            document.register_fonts(terminus::link::FontBlob::from_static(AHEM)),
            1
        );
        let text = child(
            &mut document,
            "text",
            "width:100px;font-family:Ahem;font-size:20px;line-height:21px",
        );
        append_run(&mut document, text, content);
        (document, text)
    }

    fn assert_height(document: &mut LynxDocument, text: terminus::link::NodeId, height: f32) {
        document.layout();
        assert_eq!(
            document.text_block_size(text).expect("paragraph").height,
            height
        );
        assert_eq!(
            document.rounded_layout(text).expect("text box").size.height,
            height
        );
    }

    #[test]
    fn text_maxline_updates_and_removal_resize_text_and_following_content() {
        for content in ["abc def", "abc\ndef"] {
            let (mut document, text) = paragraph(content);
            let sibling = child(&mut document, "view", "width:10px;height:10px");
            assert_height(&mut document, text, 42.0);

            for (value, height) in [
                (Some("1"), 21.0),
                (Some("2"), 42.0),
                (Some("1"), 21.0),
                (None, 42.0),
            ] {
                set_limit(&mut document, text, MAX_LINES, value);
                assert_height(&mut document, text, height);
                assert_eq!(
                    document
                        .rounded_layout(sibling)
                        .expect("following box")
                        .location
                        .y,
                    height,
                    "content after the paragraph follows its new height"
                );
            }
        }
    }

    #[test]
    fn text_limits_require_integers_and_reset_invalid_values() {
        let (mut document, text) = paragraph("abc def");
        for (value, height) in [
            ("1", 21.0),
            ("0", 42.0),
            ("1.0", 42.0),
            ("-1", 42.0),
            ("1e0", 42.0),
            ("", 42.0),
            ("  +1px", 42.0),
            ("garbage", 42.0),
            ("1e+", 42.0),
            ("1.5", 42.0),
            ("calc(1)", 42.0),
            ("1 trailing", 42.0),
            ("\u{feff}1", 42.0),
            ("Infinity", 42.0),
            ("1", 21.0),
            ("4294967296", 42.0),
        ] {
            set_limit(&mut document, text, MAX_LINES, Some(value));
            assert_height(&mut document, text, height);
        }
        for (value, width) in [("1.9", 60.0), ("0", 0.0), ("-1", 60.0)] {
            set_limit(&mut document, text, MAX_CHARS, Some(value));
            document.layout();
            assert_eq!(
                document.text_block_size(text).expect("paragraph").width,
                width
            );
        }
    }

    #[test]
    fn text_limits_belong_to_the_whole_paragraph_across_nested_runs() {
        let (mut document, text) = paragraph("abc ");
        let nested = element_under(&mut document, text, "text", "");
        append_run(&mut document, nested, "def");
        set_limit(&mut document, nested, MAX_CHARS, Some("0"));
        set_limit(&mut document, nested, MAX_LINES, Some("1"));
        assert_height(&mut document, text, 42.0);

        set_limit(&mut document, text, MAX_LINES, Some("1"));
        assert_height(&mut document, text, 21.0);
        set_limit(&mut document, text, MAX_CHARS, Some("2"));
        document.layout();
        assert_eq!(
            document.text_block_size(text).expect("paragraph").width,
            40.0
        );

        set_limit(&mut document, text, MAX_LINES, None);
        assert_height(&mut document, text, 21.0);
        set_limit(&mut document, text, MAX_CHARS, None);
        assert_height(&mut document, text, 42.0);
    }

    #[test]
    fn registered_text_limits_do_not_inherit_from_the_parent() {
        let (mut document, text) = paragraph("abc def");
        let page = document.document_element().id();
        set_limit(&mut document, page, MAX_LINES, Some("1"));
        set_limit(&mut document, page, MAX_CHARS, Some("0"));
        assert_height(&mut document, text, 42.0);
        assert_eq!(document.text_block_size(text).unwrap().width, 60.0);
        assert_eq!(
            custom_value(&document, page, "lynx-text-maxline", "-1"),
            "-1",
            "`page` is not a `text`, so the attribute is not read",
        );

        document.set_inline_style_property(page, "--lynx-text-maxline", "1");
        document.set_inline_style_property(page, "--lynx-text-maxlength", "0");
        assert_height(&mut document, text, 42.0);
        assert_eq!(
            document.text_block_size(text).unwrap().width,
            60.0,
            "and a declared limit is non-inheriting, so it stops at the page",
        );
    }

    #[test]
    fn text_attribute_rules_are_scoped_to_text() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        for name in [TEXT_OVERFLOW, "ellipsize-mode", "text-maxline-x"] {
            set_limit(&mut document, text, name, Some("1"));
        }
        let view = child(&mut document, "view", "");
        for name in [MAX_LINES, MAX_CHARS, TAIL_COLOR_CONVERT] {
            set_limit(&mut document, view, name, Some("1"));
        }
        document.layout();

        for (property, initial) in [
            ("lynx-text-maxline", "-1"),
            ("lynx-text-maxlength", "-1"),
            ("lynx-tail-color-convert", "0"),
        ] {
            assert_eq!(
                custom_value(&document, text, property, initial),
                initial,
                "an unrelated attribute changes no value: {property}",
            );
            assert_eq!(
                custom_value(&document, view, property, initial),
                initial,
                "a paragraph limit means nothing on a tag that is not a \
                 `text`: {property}",
            );
        }
    }

    #[test]
    fn inline_css_can_override_a_text_limit() {
        for (attribute, property, value, width, height) in [
            (MAX_LINES, "--lynx-text-maxline", "2", 60.0, 42.0),
            (MAX_LINES, "--lynx-text-maxline", "calc(2 - 1)", 60.0, 21.0),
            (MAX_LINES, "--lynx-text-maxline", "1.5", 60.0, 42.0),
            (MAX_LINES, "--lynx-text-maxline", "1px", 60.0, 42.0),
            (MAX_CHARS, "--lynx-text-maxlength", "2", 40.0, 21.0),
            (
                MAX_CHARS,
                "--lynx-text-maxlength",
                "calc(1 + 1)",
                40.0,
                21.0,
            ),
            (MAX_CHARS, "--lynx-text-maxlength", "1.5", 60.0, 42.0),
            (MAX_CHARS, "--lynx-text-maxlength", "1px", 60.0, 42.0),
        ] {
            let (mut document, text) = paragraph("abc def");
            set_limit(&mut document, text, attribute, Some("1"));
            document.set_inline_style_property(text, property, value);
            assert_height(&mut document, text, height);
            assert_eq!(document.text_block_size(text).unwrap().width, width);
            assert_eq!(
                document.get(text).unwrap().attribute(attribute).as_deref(),
                Some("1")
            );
        }
    }

    #[test]
    fn author_css_overrides_text_attributes_and_removing_it_restores_the_attribute() {
        for (attribute, property, width, height) in [
            (MAX_LINES, "--lynx-text-maxline", 60.0, 42.0),
            (MAX_CHARS, "--lynx-text-maxlength", 40.0, 21.0),
        ] {
            let (mut document, text) = paragraph("abc def");
            set_limit(&mut document, text, attribute, Some("1"));
            document.add_class(text, "override");
            document.add_stylesheet(
                &format!("@layer limits {{ .override {{ {property}: 2; }} }}"),
                terminus::link::StylesheetOrigin::Author,
            );
            for value in [Some("1"), Some("3"), None, Some("1")] {
                set_limit(&mut document, text, attribute, value);
                assert_height(&mut document, text, height);
                assert_eq!(document.text_block_size(text).unwrap().width, width);
            }
            document.remove_class(text, "override");
            assert_height(&mut document, text, 21.0);
            assert_eq!(
                document.text_block_size(text).unwrap().width,
                if attribute == MAX_LINES { 60.0 } else { 20.0 }
            );
        }
    }

    #[test]
    fn invalid_limits_preserve_attribute_selector_values() {
        for (attribute, first, invalid) in [(MAX_LINES, "1", "1.0"), (MAX_CHARS, "2", "2.9")] {
            let (mut document, text) = paragraph("abc def");
            document.add_stylesheet(
                &format!("[{attribute}=\"{invalid}\"] {{ padding-top: 7px; }}"),
                terminus::link::StylesheetOrigin::Author,
            );
            set_limit(&mut document, text, attribute, Some(first));
            document.layout();
            assert_eq!(document.rounded_layout(text).unwrap().size.height, 21.0);
            set_limit(&mut document, text, attribute, Some(invalid));
            document.layout();
            assert_eq!(
                document.rounded_layout(text).unwrap().size.height,
                42.0 + 7.0
            );
            assert_eq!(
                document.get(text).unwrap().attribute(attribute).as_deref(),
                Some(invalid)
            );
        }
    }

    fn custom_value(
        document: &LynxDocument,
        element: terminus::link::NodeId,
        name: &str,
        initial: &str,
    ) -> String {
        use terminus::link::stylo::custom_properties::Name;

        style_of(document, element)
            .custom_properties()
            .non_inherited
            .get(&Name::from(name))
            .map_or_else(|| initial.to_owned(), |value| value.to_variable_value().css)
    }

    fn verbatim_paragraph(content: &str) -> (LynxDocument, terminus::link::NodeId) {
        const AHEM: &[u8] = include_bytes!("../../../../seldon/tests/fixtures/Ahem.ttf");
        let mut document = document();
        assert_eq!(
            document.register_fonts(terminus::link::FontBlob::from_static(AHEM)),
            1
        );
        let text = child(
            &mut document,
            "text",
            "width:400px;font-family:Ahem;font-size:20px;line-height:21px",
        );
        append_run(&mut document, text, content);
        document.layout();
        (document, text)
    }

    fn ink(document: &LynxDocument, text: terminus::link::NodeId) -> (f32, f32) {
        let size = document.text_block_size(text).expect("paragraph");
        (size.width, size.height)
    }

    #[test]
    fn a_text_keeps_its_white_space_verbatim() {
        let (document, text) = verbatim_paragraph("  a   b  ");
        assert_eq!(ink(&document, text), (7.0 * 20.0, 21.0));

        let (document, text) = verbatim_paragraph("ab\n  cd");
        assert_eq!(
            ink(&document, text),
            (4.0 * 20.0, 2.0 * 21.0),
            "the raw-text carrier's newline breaks the line; its indent stays"
        );
    }

    #[test]
    fn a_nested_run_is_kept_verbatim_whatever_its_white_space() {
        let (mut document, text) = verbatim_paragraph("a");
        let nested = element_under(&mut document, text, "text", "white-space: normal");
        append_run(&mut document, nested, "  b");
        let inline = element_under(&mut document, text, "inline-text", "white-space: normal");
        append_run(&mut document, inline, "  c");
        document.layout();
        assert_eq!(ink(&document, text), (7.0 * 20.0, 21.0));
    }

    #[test]
    fn a_text_attribute_s_newline_breaks_the_line() {
        let (mut document, text) = verbatim_paragraph("");
        document.set_attribute(text, "text", "ab\ncd");
        document.layout();
        assert_eq!(ink(&document, text), (2.0 * 20.0, 2.0 * 21.0));
    }

    fn computed(
        document: &LynxDocument,
        element: terminus::link::NodeId,
        property: &str,
    ) -> String {
        use terminus::link::stylo::properties::PropertyId;

        let id = PropertyId::parse_enabled_for_all_content(property)
            .unwrap_or_else(|()| panic!("unknown property `{property}`"));
        let longhand = id
            .as_shorthand()
            .err()
            .unwrap_or_else(|| panic!("`{property}` is a shorthand"));
        style_of(document, element).computed_value_to_string(longhand)
    }

    #[test]
    fn only_nowrap_stops_a_text_from_wrapping() {
        const CONTENT: &str = "aaaaaaaaaaaaaaa bbbbbbbbbb\ncc";
        for (keyword, wrap_mode, collapse, expected) in [
            ("normal", "wrap", "collapse", (15.0 * 20.0, 3.0 * 21.0)),
            ("pre", "nowrap", "preserve", (15.0 * 20.0, 3.0 * 21.0)),
            ("pre-wrap", "wrap", "preserve", (15.0 * 20.0, 3.0 * 21.0)),
            (
                "pre-line",
                "wrap",
                "preserve-breaks",
                (15.0 * 20.0, 3.0 * 21.0),
            ),
            (
                "break-spaces",
                "wrap",
                "break-spaces",
                (15.0 * 20.0, 3.0 * 21.0),
            ),
            ("nowrap", "nowrap", "collapse", (26.0 * 20.0, 21.0)),
        ] {
            let (mut document, text) = verbatim_paragraph(CONTENT);
            document.set_inline_style_property(text, "white-space", keyword);
            document.layout();
            assert_eq!(
                computed(&document, text, "text-wrap-mode"),
                wrap_mode,
                "{keyword}"
            );
            assert_eq!(
                computed(&document, text, "white-space-collapse"),
                collapse,
                "{keyword}"
            );
            assert_eq!(ink(&document, text), expected, "{keyword}");
        }
    }

    #[test]
    fn switching_nowrap_to_pre_shows_every_line_again() {
        let (mut document, text) = verbatim_paragraph("ab\ncdef");
        for (keyword, expected) in [
            ("nowrap", (2.0 * 20.0, 21.0)),
            ("pre", (4.0 * 20.0, 2.0 * 21.0)),
            ("nowrap", (2.0 * 20.0, 21.0)),
        ] {
            document.set_inline_style_property(text, "white-space", keyword);
            document.layout();
            assert_eq!(ink(&document, text), expected, "{keyword}");
        }
    }

    #[test]
    fn a_newline_that_ends_a_text_adds_an_empty_line() {
        let (document, text) = verbatim_paragraph("ab\n");
        assert_eq!(ink(&document, text), (2.0 * 20.0, 2.0 * 21.0));

        let (document, text) = verbatim_paragraph("aaaaaaaaaaaaaaa bbbbbbbbbb\n");
        assert_eq!(ink(&document, text), (15.0 * 20.0, 3.0 * 21.0));

        let (mut document, text) = verbatim_paragraph("");
        document.set_attribute(text, "text", "ab\n");
        document.layout();
        assert_eq!(ink(&document, text), (2.0 * 20.0, 2.0 * 21.0));

        let (mut document, text) = verbatim_paragraph("ab\n");
        document.set_inline_style_property(text, "white-space", "nowrap");
        document.layout();
        assert_eq!(ink(&document, text), (2.0 * 20.0, 21.0));
    }

    #[test]
    fn tail_color_convert_selects_only_the_literal_true() {
        let (mut document, text) = paragraph("abc def");
        for (value, expected) in [
            (Some("true"), "1"),
            (Some("false"), "0"),
            (Some("TRUE"), "0"),
            (Some("1"), "0"),
            (Some(""), "0"),
            (Some("true"), "1"),
            (None, "0"),
        ] {
            set_limit(&mut document, text, TAIL_COLOR_CONVERT, value);
            document.layout();
            assert_eq!(
                custom_value(&document, text, "lynx-tail-color-convert", "0"),
                expected,
                "tail-color-convert={value:?}",
            );
            assert_eq!(
                document
                    .get(text)
                    .unwrap()
                    .attribute(TAIL_COLOR_CONVERT)
                    .as_deref(),
                value,
                "and the attribute itself is never rewritten",
            );
        }
    }

    #[test]
    fn text_overflow_selects_the_existing_ellipsis_path() {
        let (mut document, text) = paragraph("abc def");
        set_limit(&mut document, text, MAX_CHARS, Some("1"));
        for (overflow, width) in [("clip", 20.0), ("ellipsis", 80.0), ("clip", 20.0)] {
            document.set_inline_style_property(text, "text-overflow", overflow);
            document.layout();
            assert_eq!(
                document.text_block_size(text).expect("paragraph").width,
                width
            );
        }
    }

    fn paragraph_width(document: &mut LynxDocument, text: terminus::link::NodeId) -> f32 {
        document.layout();
        document.text_block_size(text).expect("paragraph").width
    }

    #[test]
    fn text_overflow_attribute_selects_the_ellipsis_path_through_the_ua_sheet() {
        let (mut document, text) = paragraph("abc def");
        set_limit(&mut document, text, MAX_CHARS, Some("1"));
        for (value, width) in [
            (Some("ellipsis"), 80.0),
            (Some("clip"), 20.0),
            (Some("ellipsis"), 80.0),
            (None, 20.0),
            (Some("ELLIPSIS"), 20.0),
            (Some("garbage"), 20.0),
            (Some(""), 20.0),
            (Some("ellipsis"), 80.0),
        ] {
            set_limit(&mut document, text, TEXT_OVERFLOW, value);
            assert_eq!(
                paragraph_width(&mut document, text),
                width,
                "text-overflow={value:?}",
            );
            assert_eq!(
                document
                    .get(text)
                    .unwrap()
                    .attribute(TEXT_OVERFLOW)
                    .as_deref(),
                value,
                "and the attribute itself is never rewritten",
            );
        }
    }

    #[test]
    fn author_css_outranks_the_text_overflow_attribute() {
        let (mut document, text) = paragraph("abc def");
        set_limit(&mut document, text, MAX_CHARS, Some("1"));
        set_limit(&mut document, text, TEXT_OVERFLOW, Some("ellipsis"));
        document.add_class(text, "override");
        document.add_stylesheet(
            "@layer limits { .override { text-overflow: clip; } }",
            terminus::link::StylesheetOrigin::Author,
        );
        assert_eq!(paragraph_width(&mut document, text), 20.0);
        assert_eq!(
            document
                .get(text)
                .unwrap()
                .attribute(TEXT_OVERFLOW)
                .as_deref(),
            Some("ellipsis"),
            "the author declaration wins the cascade without touching the attribute",
        );

        document.remove_class(text, "override");
        assert_eq!(paragraph_width(&mut document, text), 80.0);
    }

    #[test]
    fn a_text_does_not_wear_an_ancestor_s_color() {
        let mut document = document();
        let view = child(&mut document, "view", "color: rgb(1, 2, 3)");
        let text = element_under(&mut document, view, "text", "");
        let nested = element_under(&mut document, view, "view", "");
        document.layout();

        assert_eq!(
            style_of(&document, nested).get_color().solid_color(),
            style_of(&document, view).get_color().solid_color(),
            "a view still inherits color: only text is reset"
        );
        assert_eq!(
            style_of(&document, text).get_color().solid_color(),
            AbsoluteColor::BLACK,
            "`color: initial` stops the cascade at the text root"
        );
    }

    #[test]
    fn a_nested_text_wears_its_parent_text_s_color() {
        let mut document = document();
        let text = child(&mut document, "text", "color: rgb(1, 2, 3)");
        let direct = element_under(&mut document, text, "text", "");
        let wrapper = element_under(&mut document, text, "wrapper", "");
        let through_wrapper = element_under(&mut document, wrapper, "text", "");
        document.layout();

        let parent = style_of(&document, text).get_color().solid_color();
        assert_ne!(parent, AbsoluteColor::BLACK);
        for nested in [direct, through_wrapper] {
            assert_eq!(
                style_of(&document, nested).get_color().solid_color(),
                parent
            );
        }
    }

    #[test]
    fn the_color_reset_and_its_opt_in_carry_a_text_gradient() {
        const GRADIENT: &str = "color: linear-gradient(90deg, rgb(1, 2, 3), rgb(4, 5, 6))";

        let mut document = document();
        let painted = child(&mut document, "text", GRADIENT);
        let nested = element_under(&mut document, painted, "text", "");
        let view = child(&mut document, "view", GRADIENT);
        let under_view = element_under(&mut document, view, "text", "");
        document.layout();

        for gradient in [painted, nested] {
            assert!(
                matches!(
                    style_of(&document, gradient).get_color(),
                    ColorPropertyValue::Gradient(_)
                ),
                "`inherit` hands a nested run the gradient, not the black it collapses to"
            );
        }
        assert!(
            matches!(
                style_of(&document, under_view).get_color(),
                ColorPropertyValue::Color(_)
            ),
            "`color: initial` stops an ancestor's gradient at the text root"
        );
    }

    #[test]
    fn an_inline_truncation_is_content_only_as_a_text_s_own_child() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        let marker = element_under(&mut document, text, "inline-truncation", "");
        let second = element_under(&mut document, text, "inline-truncation", "");
        let wrapper = element_under(&mut document, text, "wrapper", "");
        let through_wrapper = element_under(&mut document, wrapper, "inline-truncation", "");
        let outside = child(&mut document, "inline-truncation", "");
        document.layout();

        for (label, scope) in [("first", marker), ("second", second)] {
            assert_eq!(
                display(&document, scope),
                Display::Flex,
                "a text's own inline-truncation child keeps the initial display: {label}",
            );
            assert_eq!(
                document.paragraph_of(scope),
                Some(text),
                "and is a text scope of the paragraph around it: {label}",
            );
            assert_eq!(
                custom_value(&document, scope, "lynx-inline-truncation", "0"),
                "1",
                "and says so in computed style, which is how the layout host \
                 finds it without naming the tag: {label}",
            );
        }
        for (label, elsewhere) in [("under a wrapper", through_wrapper), ("outside", outside)] {
            assert_eq!(
                display(&document, elsewhere),
                Display::None,
                "and nowhere else: {label}",
            );
        }
    }

    #[test]
    fn a_text_renders_text_and_the_tags_that_are_content() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        let boxes = ["view", "image"].map(|tag| (tag, element_under(&mut document, text, tag, "")));
        let nested = element_under(&mut document, text, "text", "");
        let nested_inline = element_under(&mut document, text, "inline-text", "");
        let wrapper = element_under(&mut document, text, "wrapper", "");
        let through_wrapper = element_under(&mut document, wrapper, "view", "");
        let foreign = element_under(&mut document, text, "x-foreign", "");
        let outside = child(&mut document, "x-foreign", "");
        document.layout();

        for (tag, content) in boxes {
            assert_eq!(display(&document, content), Display::Flex, "{tag}");
        }
        for (tag, scope) in [("text", nested), ("inline-text", nested_inline)] {
            assert_eq!(
                display(&document, scope),
                Display::Flex,
                "a nested text scope keeps the initial display: {tag}"
            );
            assert_eq!(
                document.paragraph_of(scope),
                Some(text),
                "and is part of the paragraph, not a box in it: {tag}"
            );
        }
        assert_eq!(display(&document, wrapper), Display::Contents);
        assert_eq!(display(&document, through_wrapper), Display::Linear);
        assert_eq!(
            display(&document, foreign),
            Display::None,
            "anything else written inside a text generates no box"
        );
        assert_eq!(
            display(&document, outside),
            Display::Flex,
            "and the suppression reaches no further than a text's own children"
        );
    }

    #[test]
    fn an_author_display_hides_a_text_scope_only_with_none() {
        for (author, computed) in [
            ("display: none", Display::None),
            ("display: flex", Display::Flex),
            ("display: grid", Display::Grid),
            ("display: linear", Display::Linear),
        ] {
            let mut document = document();
            let text = child(&mut document, "text", author);
            let run = document.create_text_node("ab", ());
            document.append_child(text, run);
            let host = child(&mut document, "text", "");
            let scopes = [
                (
                    "nested text",
                    element_under(&mut document, host, "text", author),
                ),
                (
                    "inline-text",
                    element_under(&mut document, host, "inline-text", author),
                ),
                (
                    "inline-truncation",
                    element_under(&mut document, host, "inline-truncation", author),
                ),
            ];
            for (_, scope) in scopes {
                let run = document.create_text_node("ab", ());
                document.append_child(scope, run);
            }
            document.layout();

            assert_eq!(display(&document, text), computed, "text, {author}");
            let hidden = computed == Display::None;
            assert_eq!(
                document.text_block_size(text).is_none(),
                hidden,
                "a text establishes its paragraph unless it is hidden: {author}"
            );
            for (label, scope) in scopes {
                assert_eq!(display(&document, scope), computed, "{label}, {author}");
                assert_eq!(
                    document.paragraph_of(scope),
                    (!hidden).then_some(host),
                    "a scope is part of its host's paragraph unless it is hidden: \
                     {label}, {author}"
                );
            }
        }
    }

    #[test]
    fn a_text_with_display_contents_is_undefined_and_only_does_not_crash() {
        let mut document = document();
        let view = child(&mut document, "view", "");
        let text = element_under(&mut document, view, "text", "display: contents");
        let run = document.create_text_node("hello", ());
        document.append_child(text, run);
        document.layout();
        document.render();

        assert_eq!(display(&document, text), Display::Contents);
        assert_eq!(
            document.bounding_client_rect(text),
            None,
            "the text element has no box"
        );
        assert_eq!(document.text_block_size(text), None);
    }

    fn invoke(
        document: &mut LynxDocument,
        text: terminus::link::NodeId,
        name: &str,
        params: &str,
    ) -> terminus::link::MethodOutcome {
        document.invoke_element_method(text, terminus::link::MethodCall { name, params })
    }

    fn boxes_record(boxes: &[[f32; 4]]) -> terminus::link::MethodOutcome {
        let mut record = String::new();
        crate::main::record::write_record_field(&mut record, &boxes.len().to_string());
        for values in boxes {
            for value in values {
                crate::main::record::write_record_field(&mut record, &value.to_string());
            }
        }
        terminus::link::MethodOutcome::Data(record)
    }

    #[test]
    fn get_text_bounding_rect_answers_one_box_per_line_in_viewport_coordinates() {
        const AHEM: &[u8] = include_bytes!("../../../../seldon/tests/fixtures/Ahem.ttf");
        let mut document = document();
        assert_eq!(
            document.register_fonts(terminus::link::FontBlob::from_static(AHEM)),
            1
        );
        let text = child(
            &mut document,
            "text",
            "position:absolute;left:20px;top:30px;padding:5px;border:2px solid;width:100px;\
             font-family:Ahem;font-size:20px;line-height:21px",
        );
        append_run(&mut document, text, "abc def");
        document.layout();
        assert_eq!(
            invoke(
                &mut document,
                text,
                "getTextBoundingRect",
                r#"{"start":1,"end":6}"#
            ),
            boxes_record(&[[47.0, 37.0, 40.0, 21.0], [27.0, 58.0, 40.0, 21.0]]),
        );
        assert_eq!(
            invoke(
                &mut document,
                text,
                "getTextBoundingRect",
                r#"{"start":0,"end":3}"#
            ),
            boxes_record(&[[27.0, 37.0, 60.0, 21.0]]),
        );
        assert_eq!(
            invoke(
                &mut document,
                text,
                "getTextBoundingRect",
                r#"{"start":5,"end":5}"#
            ),
            boxes_record(&[[27.0, 37.0, 0.0, 21.0]]),
            "an empty range is the natives' zero box at the content box's origin"
        );
        assert_eq!(
            invoke(
                &mut document,
                text,
                "getTextBoundingRect",
                r#"{"start":1.9,"end":2.5}"#
            ),
            boxes_record(&[[47.0, 37.0, 20.0, 21.0]]),
            "offsets truncate toward zero"
        );
        assert_eq!(
            invoke(
                &mut document,
                text,
                "getTextBoundingRect",
                r#"{"start":-0.5,"end":1,"x":0}"#
            ),
            boxes_record(&[[27.0, 37.0, 20.0, 21.0]]),
            "-0.5 truncates to 0, which is not negative; other keys are ignored"
        );
    }

    #[test]
    fn get_text_bounding_rect_refuses_bad_params_and_finds_nothing_with_code_8() {
        let (mut document, text) = paragraph("abc def");
        let nested = element_under(&mut document, text, "text", "");
        append_run(&mut document, nested, "gh");
        let view = child(&mut document, "view", "width:10px;height:10px");
        document.layout();
        let invalid =
            terminus::link::MethodOutcome::Failed(terminus::link::MethodError::InvalidParams);
        let not_found =
            terminus::link::MethodOutcome::Failed(terminus::link::MethodError::Operation);
        for params in [
            "{}",
            "null",
            "[]",
            "3",
            r#"{"start":1}"#,
            r#"{"end":1}"#,
            r#"{"start":"1","end":"2"}"#,
            r#"{"start":-1,"end":2}"#,
            r#"{"start":0,"end":-1}"#,
            r#"{"start":3,"end":1}"#,
            "not json",
        ] {
            assert_eq!(
                invoke(&mut document, text, "getTextBoundingRect", params),
                invalid,
                "{params}"
            );
        }
        for params in [
            r#"{"start":0,"end":10}"#,
            r#"{"start":1e12,"end":1e12}"#,
            r#"{"start":3,"end":4}"#,
        ] {
            assert_eq!(
                invoke(&mut document, text, "getTextBoundingRect", params),
                not_found,
                "{params}"
            );
        }
        assert!(matches!(
            invoke(
                &mut document,
                text,
                "getTextBoundingRect",
                r#"{"start":7,"end":9}"#
            ),
            terminus::link::MethodOutcome::Data(_)
        ));
        assert_eq!(
            invoke(
                &mut document,
                nested,
                "getTextBoundingRect",
                r#"{"start":0,"end":1}"#
            ),
            not_found,
            "a nested text is a run of its ancestor's paragraph and establishes none"
        );
        assert_eq!(
            invoke(&mut document, text, "boundingClientRect", "{}"),
            terminus::link::MethodOutcome::NotFound,
            "the base set answers the rest"
        );
        assert_eq!(
            invoke(
                &mut document,
                view,
                "getTextBoundingRect",
                r#"{"start":0,"end":1}"#
            ),
            terminus::link::MethodOutcome::NotFound,
            "only `text` has the method"
        );
    }

    fn append_text(document: &mut LynxDocument, parent: terminus::link::NodeId, content: &str) {
        let node = document.create_text_node(content, ());
        document.append_child(parent, node);
    }

    fn select(
        document: &mut LynxDocument,
        host: terminus::link::NodeId,
        anchor: usize,
        focus: usize,
    ) {
        if document.focused() != Some(host) {
            assert!(document.focus(host));
        }
        assert!(
            document
                .apply_editing(terminus::link::editing::Command::SetSelection {
                    range: anchor..focus,
                })
                .applied
        );
    }

    fn record_of(
        document: &LynxDocument,
        element: terminus::link::NodeId,
    ) -> Option<(std::ops::Range<u32>, bool)> {
        super::text_selection_in(document, element, document.selection()?)
    }

    #[test]
    fn a_text_s_selection_counts_its_whole_text_content() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        document.set_attribute(text, "contenteditable", "");
        append_text(&mut document, text, "before");
        let inline = element_under(&mut document, text, "inline-text", "");
        append_text(&mut document, inline, "inline");
        append_text(&mut document, text, "after");
        document.layout();

        select(&mut document, text, 1, 4);
        assert_eq!(record_of(&document, text), Some((1..4, true)));
        assert_eq!(record_of(&document, inline), None);

        select(&mut document, text, 8, 7);
        assert_eq!(record_of(&document, text), Some((13..14, false)));
        assert_eq!(record_of(&document, inline), None);

        select(&mut document, text, 3, 3);
        assert_eq!(record_of(&document, text), None, "collapsed");

        let mut other = super::super::test_support::document();
        let plain = child(&mut other, "text", "");
        other.set_attribute(plain, "contenteditable", "");
        append_text(&mut other, plain, "abc");
        other.layout();
        select(&mut other, plain, 0, 2);
        assert_eq!(record_of(&other, plain), Some((0..2, true)));
    }

    #[test]
    fn an_inline_text_host_s_selection_counts_from_its_text() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        append_text(&mut document, text, "ab");
        let inline = element_under(&mut document, text, "inline-text", "");
        document.set_attribute(inline, "contenteditable", "");
        append_text(&mut document, inline, "cdef");
        document.layout();

        select(&mut document, inline, 1, 3);
        assert_eq!(record_of(&document, inline), Some((1..3, true)));
        assert_eq!(record_of(&document, text), Some((3..5, true)));
        let view = child(&mut document, "view", "");
        assert_eq!(record_of(&document, view), None, "outside the element");
    }

    #[test]
    fn selection_reports_are_deduped_and_the_empty_record_follows_a_selection() {
        use terminus::link::event::{ListenerKey, ListenerKinds};

        use super::super::test_support::with_component_events;
        use super::super::{ComponentDetail, ComponentEvent, PageConfig};
        use super::{SELECTION_CHANGE, SelectionReports, report_selection_changes};

        let (mut document, events) = with_component_events(PageConfig::default());
        let text = child(&mut document, "text", "");
        document.set_attribute(text, "contenteditable", "");
        append_text(&mut document, text, "abc");
        document.layout();
        let mut reports = SelectionReports::default();
        let report = |document: &LynxDocument, reports: &mut SelectionReports| {
            report_selection_changes(document, reports, &events);
            events.take()
        };
        let queued = |range, forward| {
            vec![ComponentEvent::Detailed {
                node: text,
                name: SELECTION_CHANGE,
                detail: ComponentDetail::TextSelection { range, forward },
            }]
        };

        select(&mut document, text, 0, 2);
        assert_eq!(report(&document, &mut reports), [], "nothing listens");

        let realm = ListenerKey(1);
        document.add_listener(text, SELECTION_CHANGE, ListenerKinds::PATH_BUBBLE, realm);
        assert_eq!(report(&document, &mut reports), queued(Some(0..2), true));
        assert_eq!(report(&document, &mut reports), [], "the same record");
        select(&mut document, text, 2, 0);
        assert_eq!(report(&document, &mut reports), queued(Some(0..2), false));

        assert!(document.blur());
        assert_eq!(report(&document, &mut reports), queued(None, true));
        assert_eq!(report(&document, &mut reports), [], "empty after empty");

        select(&mut document, text, 1, 1);
        assert_eq!(report(&document, &mut reports), []);
        select(&mut document, text, 1, 3);
        assert_eq!(report(&document, &mut reports), queued(Some(1..3), true));

        document.remove_listener(text, SELECTION_CHANGE, realm);
        assert!(document.blur());
        assert_eq!(report(&document, &mut reports), []);
        document.add_listener(text, SELECTION_CHANGE, ListenerKinds::PATH_BUBBLE, realm);
        select(&mut document, text, 1, 3);
        assert_eq!(report(&document, &mut reports), queued(Some(1..3), true));
    }

    fn text_record(text: &str) -> terminus::link::MethodOutcome {
        let mut record = String::new();
        crate::main::record::write_record_field(&mut record, text);
        terminus::link::MethodOutcome::Data(record)
    }

    fn select_in(
        document: &mut LynxDocument,
        root: terminus::link::NodeId,
        anchor: usize,
        focus: usize,
    ) {
        assert!(document.begin_selection(root));
        assert!(
            document
                .apply_editing(terminus::link::editing::Command::SetSelection {
                    range: anchor..focus,
                })
                .applied
        );
    }

    fn selectable_paragraph(
        mut document: LynxDocument,
    ) -> (LynxDocument, terminus::link::NodeId, terminus::link::NodeId) {
        const AHEM: &[u8] = include_bytes!("../../../../seldon/tests/fixtures/Ahem.ttf");
        assert_eq!(
            document.register_fonts(terminus::link::FontBlob::from_static(AHEM)),
            1
        );
        let text = child(
            &mut document,
            "text",
            "width:400px;font-family:Ahem;font-size:20px;line-height:20px",
        );
        document.set_attribute(text, "text-selection", "true");
        append_run(&mut document, text, "ab");
        let inline = element_under(&mut document, text, "inline-text", "");
        append_run(&mut document, inline, "cd");
        element_under(&mut document, text, "view", "width:20px;height:20px");
        append_run(&mut document, text, "ef");
        document.layout();
        (document, text, inline)
    }

    #[test]
    fn text_selection_maps_onto_user_select() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        let inline = element_under(&mut document, text, "inline-text", "");
        let run = element_under(&mut document, text, "raw-text", "");
        document.set_attribute(run, "text", "abc");
        let view = child(&mut document, "view", "");
        for (value, expected) in [
            (Some("true"), "auto"),
            (Some("false"), "none"),
            (Some(""), "auto"),
            (Some("FALSE"), "none"),
            (Some("1"), "auto"),
            (None, "none"),
        ] {
            set_limit(&mut document, text, "text-selection", value);
            set_limit(&mut document, view, "text-selection", value);
            document.layout();
            assert_eq!(
                computed(&document, text, "user-select"),
                expected,
                "{value:?}"
            );
            assert_eq!(
                computed(&document, inline, "user-select"),
                expected,
                "{value:?} on the inline-text inside"
            );
            assert_eq!(
                computed(&document, view, "user-select"),
                "auto",
                "{value:?} on a view"
            );
            assert_eq!(
                document.is_user_selectable(run),
                expected == "auto",
                "{value:?}: the used value"
            );
            assert!(
                document.is_user_selectable(view),
                "{value:?}: a view's auto"
            );
        }
    }

    #[test]
    fn a_selectable_text_s_record_counts_its_text_content_and_no_atom() {
        let (mut document, text, inline) = selectable_paragraph(document());

        select_in(&mut document, text, 1, 6);
        assert_eq!(record_of(&document, text), Some((1..5, true)));
        assert_eq!(record_of(&document, inline), None, "unit 1 is outside cd");

        select_in(&mut document, text, 2, 3);
        assert_eq!(record_of(&document, text), Some((2..3, true)));
        assert_eq!(record_of(&document, inline), Some((0..1, true)));

        select_in(&mut document, text, 4, 2);
        assert_eq!(record_of(&document, text), Some((2..4, false)));
        assert_eq!(
            record_of(&document, inline),
            Some((0..2, false)),
            "the end of the inline-text's range is in it"
        );

        select_in(&mut document, text, 3, 5);
        assert_eq!(record_of(&document, text), Some((3..4, true)));
        assert_eq!(record_of(&document, inline), None);

        select_in(&mut document, text, 3, 3);
        assert_eq!(record_of(&document, text), None, "collapsed");

        let view = child(&mut document, "view", "");
        select_in(&mut document, text, 0, 2);
        assert_eq!(record_of(&document, view), None, "outside the text");
    }

    #[test]
    fn a_selectable_text_reports_to_an_inline_text_inside_it() {
        use terminus::link::event::{ListenerKey, ListenerKinds};

        use super::super::test_support::with_component_events;
        use super::super::{ComponentDetail, ComponentEvent, PageConfig};
        use super::{SELECTION_CHANGE, SelectionReports, report_selection_changes};

        let (document, events) = with_component_events(PageConfig::default());
        let (mut document, text, inline) = selectable_paragraph(document);
        document.add_listener(
            inline,
            SELECTION_CHANGE,
            ListenerKinds::PATH_BUBBLE,
            ListenerKey(1),
        );
        let mut reports = SelectionReports::default();
        let mut report = |document: &LynxDocument| {
            report_selection_changes(document, &mut reports, &events);
            events.take()
        };
        let queued = |range, forward| {
            vec![ComponentEvent::Detailed {
                node: inline,
                name: SELECTION_CHANGE,
                detail: ComponentDetail::TextSelection { range, forward },
            }]
        };

        select_in(&mut document, text, 2, 4);
        assert_eq!(report(&document), queued(Some(0..2), true));
        assert_eq!(report(&document), [], "the same record");
        select_in(&mut document, text, 0, 4);
        assert_eq!(report(&document), queued(None, true), "unit 0 is outside");
        select_in(&mut document, text, 3, 2);
        assert_eq!(report(&document), queued(Some(0..1), false));
        assert!(document.clear_selection());
        assert_eq!(report(&document), queued(None, true));
    }

    #[test]
    fn get_selected_text_answers_the_selection_in_this_text() {
        let (mut document, text, _) = selectable_paragraph(document());
        let other = child(&mut document, "text", "");
        document.set_attribute(other, "text-selection", "");
        append_run(&mut document, other, "xyz");
        let nested = element_under(&mut document, text, "text", "");
        append_run(&mut document, nested, "gh");
        document.layout();

        assert_eq!(
            invoke(&mut document, text, "getSelectedText", "{}"),
            text_record("")
        );
        select_in(&mut document, text, 1, 6);
        assert_eq!(
            invoke(&mut document, text, "getSelectedText", "null"),
            text_record("bcd\u{fffc}e"),
            "no params are read"
        );
        assert_eq!(
            invoke(&mut document, other, "getSelectedText", "{}"),
            text_record("")
        );
        assert_eq!(
            invoke(&mut document, nested, "getSelectedText", "{}"),
            text_record("")
        );
        select_in(&mut document, text, 2, 2);
        assert_eq!(
            invoke(&mut document, text, "getSelectedText", "{}"),
            text_record("")
        );
        select_in(&mut document, other, 2, 0);
        assert_eq!(
            invoke(&mut document, other, "getSelectedText", "{}"),
            text_record("xy")
        );
        assert_eq!(
            invoke(&mut document, text, "getSelectedText", "{}"),
            text_record("")
        );
    }

    fn selection_record(
        boxes: &[[f32; 4]],
        handles: [[f32; 2]; 2],
    ) -> terminus::link::MethodOutcome {
        let terminus::link::MethodOutcome::Data(mut record) = boxes_record(boxes) else {
            unreachable!("boxes_record answers data");
        };
        for [x, y] in handles {
            for value in [x, y, 50.0] {
                crate::main::record::write_record_field(&mut record, &value.to_string());
            }
        }
        terminus::link::MethodOutcome::Data(record)
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one scenario over the selection, its boxes and both handles"
    )]
    fn set_text_selection_selects_between_two_points_and_answers_boxes_and_handles() {
        const AHEM: &[u8] = include_bytes!("../../../../seldon/tests/fixtures/Ahem.ttf");
        let mut document = document();
        assert_eq!(
            document.register_fonts(terminus::link::FontBlob::from_static(AHEM)),
            1
        );
        let text = child(
            &mut document,
            "text",
            "position:absolute;left:20px;top:30px;padding:5px;border:2px solid;width:200px;\
             font-family:Ahem;font-size:20px;line-height:21px",
        );
        document.set_attribute(text, "text-selection", "true");
        append_run(&mut document, text, "abc def");
        let nested = element_under(&mut document, text, "text", "");
        document.set_attribute(nested, "text-selection", "true");
        append_run(&mut document, nested, "gh");
        let plain = child(&mut document, "text", "");
        append_run(&mut document, plain, "plain");
        document.layout();
        let call = |document: &mut LynxDocument, element, [sx, sy, ex, ey]: [f32; 4]| {
            invoke(
                document,
                element,
                "setTextSelection",
                &format!(r#"{{"startX":{sx},"startY":{sy},"endX":{ex},"endY":{ey}}}"#),
            )
        };
        let selected = |document: &LynxDocument| {
            document
                .selection()
                .map(|selection| (selection.host, selection.anchor, selection.focus))
        };

        assert_eq!(
            call(&mut document, text, [32.0, 17.0, 92.0, 17.0]),
            selection_record(&[[47.0, 37.0, 60.0, 21.0]], [[47.0, 58.0], [107.0, 58.0]]),
        );
        assert_eq!(selected(&document), Some((text, 1, 4)));
        assert_eq!(
            invoke(&mut document, text, "getSelectedText", "{}"),
            text_record("bc ")
        );

        assert_eq!(
            call(&mut document, text, [32.0, 17.0, 32.0, 17.0]),
            selection_record(&[[47.0, 37.0, 20.0, 21.0]], [[47.0, 58.0], [67.0, 58.0]]),
        );
        assert_eq!(selected(&document), Some((text, 1, 2)));
        assert_eq!(
            call(&mut document, text, [207.0, 17.0, 207.0, 17.0]),
            selection_record(&[[187.0, 37.0, 20.0, 21.0]], [[187.0, 58.0], [207.0, 58.0]]),
        );
        assert_eq!(selected(&document), Some((text, 9, 8)));

        assert_eq!(
            call(&mut document, text, [-1.0, 0.0, 10.0, 10.0]),
            terminus::link::MethodOutcome::Done
        );
        assert_eq!(selected(&document), None);
        select_in(&mut document, text, 0, 2);
        assert_eq!(
            call(&mut document, plain, [0.0, 0.0, 0.0, -0.5]),
            terminus::link::MethodOutcome::Done
        );
        assert_eq!(selected(&document), Some((text, 0, 2)));

        let invalid =
            terminus::link::MethodOutcome::Failed(terminus::link::MethodError::InvalidParams);
        for params in [
            "{}",
            "null",
            "[]",
            r#"{"startX":1,"startY":1,"endX":1}"#,
            r#"{"startX":"1","startY":1,"endX":1,"endY":1}"#,
            "not json",
        ] {
            assert_eq!(
                invoke(&mut document, text, "setTextSelection", params),
                invalid,
                "{params}"
            );
        }
        assert!(
            matches!(
                invoke(
                    &mut document,
                    text,
                    "setTextSelection",
                    r#"{"startX":10,"startY":10,"endX":30,"endY":10,"showStartHandle":false,"showEndHandle":"x"}"#
                ),
                terminus::link::MethodOutcome::Data(_)
            ),
            "the handle flags are not read"
        );
        let operation =
            terminus::link::MethodOutcome::Failed(terminus::link::MethodError::Operation);
        assert_eq!(
            call(&mut document, nested, [0.0, 0.0, 10.0, 10.0]),
            operation,
            "a nested text establishes no paragraph"
        );
        assert_eq!(
            call(&mut document, plain, [0.0, 0.0, 10.0, 10.0]),
            operation,
            "a text without text-selection cannot hold the selection"
        );
    }
}
