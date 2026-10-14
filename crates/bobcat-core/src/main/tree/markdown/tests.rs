#![allow(clippy::float_cmp)] // Ahem and explicit line heights have exact metrics.

use std::fmt::Write as _;

use dom::event::ElementEventKind;
use dom::stylo::properties::PropertyId;
use dom::{MethodCall, MethodError, MethodOutcome, NodeId, Point2D};

use super::super::test_support::{child, style_of, with_component_events};
use super::super::{ComponentDetail, ComponentEvent, ComponentEvents, LynxDocument, PageConfig};
use super::{MARKDOWN_TAG, X_MARKDOWN_TAG, markdown_root};

const AHEM: &[u8] = include_bytes!("../../../../../hughie/tests/fixtures/Ahem.ttf");

/// Ahem at 10px on a 300px-wide element: every glyph a 10px square, every
/// line `1.4 × 10 = 14px` tall.
const AHEM_HOST: &str = "font-family: Ahem; font-size: 10px; width: 300px";

/// web-core's `basic.html` fixture content.
const BASIC: &str = "# Title\n\nthis is **bold** and *italic*.\n\n- item 1\n- item 2\n\ninline code: `code`.\n\n```\nblock code\n```";

/// A document that shapes Ahem, with an `x-markdown` under the page.
fn markdown(style: &str) -> (LynxDocument, ComponentEvents, NodeId) {
    markdown_tag(X_MARKDOWN_TAG, style)
}

fn markdown_tag(tag: &str, style: &str) -> (LynxDocument, ComponentEvents, NodeId) {
    let (mut document, events) = with_component_events(PageConfig::default());
    document.register_fonts(dom::FontBlob::from_static(AHEM));
    let element = child(&mut document, tag, style);
    (document, events, element)
}

/// `markdown(AHEM_HOST)` rendering `content`, laid out, with the events of
/// the render taken.
fn rendered(content: &str) -> (LynxDocument, ComponentEvents, NodeId) {
    let (mut document, events, element) = markdown(AHEM_HOST);
    document.set_attribute(element, "content", content);
    document.layout();
    events.take();
    (document, events, element)
}

fn root(document: &LynxDocument, element: NodeId) -> NodeId {
    markdown_root(document, element).expect("every markdown element has its root")
}

/// The shadow tree under `#markdown-root` as an outline: one line per node,
/// indented by depth, `tag.class` for an element and the text quoted.
fn outline(document: &LynxDocument, element: NodeId) -> String {
    let mut out = String::new();
    let mut stack: Vec<(NodeId, usize)> = document
        .get(root(document, element))
        .unwrap()
        .child_ids()
        .iter()
        .rev()
        .map(|id| (*id, 0))
        .collect();
    while let Some((id, depth)) = stack.pop() {
        let node = document.get(id).unwrap();
        out.push_str(&"  ".repeat(depth));
        if node.is_text_node() {
            writeln!(out, "{:?}", node.text().unwrap()).unwrap();
        } else {
            out.push_str(node.tag_name().unwrap());
            for class in node.classes() {
                out.push('.');
                out.push_str(class);
            }
            out.push('\n');
            stack.extend(
                node.child_ids()
                    .iter()
                    .rev()
                    .map(|child| (*child, depth + 1)),
            );
        }
    }
    out
}

/// Every element named `tag` under `element`'s root, in tree order.
fn all(document: &LynxDocument, element: NodeId, tag: &str) -> Vec<NodeId> {
    let mut found = Vec::new();
    let mut stack = vec![root(document, element)];
    while let Some(id) = stack.pop() {
        let node = document.get(id).unwrap();
        if node.tag_name() == Some(tag) {
            found.push(id);
        }
        stack.extend(node.child_ids().iter().rev().copied());
    }
    found
}

fn first(document: &LynxDocument, element: NodeId, tag: &str) -> NodeId {
    all(document, element, tag)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("a `{tag}` in the shadow tree"))
}

/// One computed longhand, as a string.
fn value(document: &LynxDocument, element: NodeId, property: &str) -> String {
    let id = PropertyId::parse_enabled_for_all_content(property)
        .unwrap_or_else(|()| panic!("unknown property `{property}`"));
    let longhand = id
        .as_shorthand()
        .err()
        .unwrap_or_else(|| panic!("`{property}` is a shorthand"));
    style_of(document, element).computed_value_to_string(longhand)
}

fn rect(document: &LynxDocument, element: NodeId) -> (f32, f32, f32, f32) {
    let rect = document
        .bounding_client_rect(element)
        .expect("a laid-out element");
    (
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
}

fn invoke(document: &mut LynxDocument, element: NodeId, name: &str, params: &str) -> MethodOutcome {
    document.invoke_element_method(element, MethodCall { name, params })
}

/// A record answer split into its fields (`<units>:<text>` each).
fn fields(outcome: &MethodOutcome) -> Vec<String> {
    let MethodOutcome::Data(record) = outcome else {
        panic!("a data answer, not {outcome:?}");
    };
    let mut rest = record.as_str();
    let mut fields = Vec::new();
    while !rest.is_empty() {
        let (count, tail) = rest.split_once(':').expect("a field length");
        let units: usize = count.parse().expect("a number");
        let mut taken = 0;
        let mut bytes = 0;
        for character in tail.chars() {
            if taken == units {
                break;
            }
            taken += character.len_utf16();
            bytes += character.len_utf8();
        }
        fields.push(tail[..bytes].to_owned());
        rest = &tail[bytes..];
    }
    fields
}

/// `getParseResult({tags})`'s answer as `tag:[start,end]...` per tag, after
/// the id.
fn parse_result(document: &mut LynxDocument, element: NodeId, tags: &[&str]) -> Vec<String> {
    let params = serde_json::json!({ "tags": tags }).to_string();
    let fields = fields(&invoke(document, element, "getParseResult", &params));
    let mut out = vec![format!("id={}", fields[0])];
    let mut at = 2;
    for _ in 0..fields[1].parse::<usize>().unwrap() {
        let name = &fields[at];
        let count: usize = fields[at + 1].parse().unwrap();
        let ranges: Vec<String> = (0..count)
            .map(|index| {
                format!(
                    "[{},{}]",
                    fields[at + 2 + index * 2],
                    fields[at + 3 + index * 2]
                )
            })
            .collect();
        out.push(format!("{name}:{}", ranges.join("")));
        at += 2 + count * 2;
    }
    out
}

/// The rendered text the offsets count in, with markdown-it's line breaks
/// (the component's test-only `renderedText` method).
fn rendered_text(document: &mut LynxDocument, element: NodeId) -> String {
    let answer = invoke(document, element, super::RENDERED_TEXT_METHOD, "{}");
    fields(&answer).remove(0)
}

#[test]
fn the_basic_fixture_builds_its_shadow_tree_and_lays_out_its_paragraphs() {
    let (document, _events, element) = rendered(BASIC);
    assert_eq!(
        outline(&document, element),
        "\
h1
  \"Title\"
p
  \"this is \"
  strong
    \"bold\"
  \" and \"
  em
    \"italic\"
  \".\"
ul
  li
    md-marker
      \"\u{2022}\"
    md-line
      \"item 1\"
  li
    md-marker
      \"\u{2022}\"
    md-line
      \"item 2\"
p
  \"inline code: \"
  code
    \"code\"
  \".\"
pre
  code
    \"block code\\n\"
"
    );
    for tag in ["h1", "p", "md-line", "md-marker", "code"] {
        for paragraph in all(&document, element, tag) {
            if document
                .get(paragraph)
                .unwrap()
                .parent()
                .unwrap()
                .tag_name()
                == Some("p")
            {
                // Inline code is a run of its paragraph.
                assert_eq!(
                    document.paragraph_of(paragraph),
                    document.get(paragraph).unwrap().parent_id()
                );
                continue;
            }
            assert_eq!(document.paragraph_of(paragraph), Some(paragraph), "{tag}");
            assert!(
                document.text_block_size(paragraph).unwrap().height > 0.0,
                "{tag}"
            );
        }
    }
    // The heading: 2em of 10px, 1.4 line height, after its 0.8em margin.
    let heading = first(&document, element, "h1");
    assert_eq!(rect(&document, heading), (0.0, 16.0, 300.0, 28.0));
    assert_eq!(value(&document, heading, "font-weight"), "700");
    // Bold and italic are runs with HTML's weight and style.
    let bold = first(&document, element, "strong");
    assert_eq!(value(&document, bold, "font-weight"), "700");
    let italic = first(&document, element, "em");
    assert_eq!(value(&document, italic, "font-style"), "italic");
    // A list item's text starts 1.5em + 40px in, its marker before it.
    let items = all(&document, element, "md-line");
    let markers = all(&document, element, "md-marker");
    let (item_x, item_y, _, _) = rect(&document, items[0]);
    let (marker_x, marker_y, marker_width, _) = rect(&document, markers[0]);
    assert_eq!(item_x, 55.0);
    assert_eq!((marker_x + marker_width, marker_y), (item_x, item_y));
    // The code block keeps its line and scrolls sideways.
    let pre = first(&document, element, "pre");
    assert_eq!(value(&document, pre, "overflow-x"), "scroll");
    assert_eq!(value(&document, pre, "overflow-y"), "hidden");
}

#[test]
fn both_tags_render_the_same_tree() {
    let (mut document, _events, element) = markdown_tag(MARKDOWN_TAG, AHEM_HOST);
    document.set_attribute(element, "content", BASIC);
    document.layout();
    let (reference, _, x_element) = rendered(BASIC);
    assert_eq!(outline(&document, element), outline(&reference, x_element));
    for tag in [MARKDOWN_TAG, X_MARKDOWN_TAG] {
        let (mut document, _, element) = markdown_tag(tag, "");
        document.layout();
        assert_eq!(value(&document, element, "display"), "flex", "{tag}");
        assert_eq!(
            value(&document, element, "flex-direction"),
            "column",
            "{tag}"
        );
        assert_eq!(
            value(&document, element, "box-sizing"),
            "border-box",
            "{tag}"
        );
    }
}

/// web-core's `style.html` fixture: `markdown-style` reaches the link and
/// the inline code, and a new value replaces the old one.
#[test]
fn markdown_style_styles_the_link_and_inline_code_and_is_replaced_on_change() {
    let (mut document, _events, element) = markdown(AHEM_HOST);
    document.set_attribute(
        element,
        "content",
        "this is [link](https://example.com).\n\ninline code: `code`.",
    );
    document.set_attribute(
        element,
        "markdown-style",
        r#"{"normalText":{"fontSize":20},"link":{"color":"ff0000","lineHeight":24},"inlineCode":{"color":"0000ff","backgroundColor":"eeeeee"}}"#,
    );
    document.layout();
    let link = first(&document, element, "a");
    let code = first(&document, element, "code");
    let root = root(&document, element);
    assert_eq!(value(&document, link, "color"), "rgb(255, 0, 0)");
    assert_eq!(value(&document, link, "line-height"), "24px");
    assert_eq!(value(&document, code, "color"), "rgb(0, 0, 255)");
    assert_eq!(
        value(&document, code, "background-color"),
        "rgb(238, 238, 238)"
    );
    assert_eq!(value(&document, root, "font-size"), "20px");

    document.set_attribute(element, "markdown-style", r#"{"link":{"color":"00ff00"}}"#);
    document.layout();
    assert_eq!(value(&document, link, "color"), "rgb(0, 255, 0)");
    assert_eq!(value(&document, link, "line-height"), "14px");
    assert_eq!(
        value(&document, code, "background-color"),
        "rgba(0, 0, 0, 0)"
    );
    assert_eq!(value(&document, root, "font-size"), "10px");

    // Without it, the template's link colour.
    document.remove_attribute(element, "markdown-style");
    document.layout();
    assert_eq!(value(&document, link, "color"), "rgb(3, 102, 214)");
    assert_eq!(value(&document, link, "text-decoration-line"), "underline");
}

/// The paragraphs are not `text`, so they wear the host's colour, as
/// web-core's `.markdown-body { color: inherit }` does.
#[test]
fn the_rendered_text_inherits_the_hosts_color() {
    let (mut document, _events, element) = markdown("color: rgb(1, 2, 3)");
    document.set_attribute(element, "content", "# heading\n\nplain *em*\n\n- item");
    document.layout();
    for tag in ["h1", "p", "em", "md-line", "md-marker"] {
        assert_eq!(
            value(&document, first(&document, element, tag), "color"),
            "rgb(1, 2, 3)",
            "{tag}"
        );
    }
}

/// web-core's `html-tags.html` fixture: a kept `span` and `p` carry only
/// their classes, and the refused `div` is text.
#[test]
fn raw_html_keeps_span_and_p_with_their_classes_only() {
    let (mut document, _events, element) = markdown(AHEM_HOST);
    document.set_attribute(
        element,
        "content",
        "### Hello <span class=\"mark-red other\" style=\"color: red\" onclick=\"noop()\">World</span>\n\n<p class=\"mark-help\" style=\"font-size: 12px\">Paragraph</p>\n\n<div class=\"not-allowed\">Not allowed</div>",
    );
    document.set_attribute(
        element,
        "markdown-style",
        r#"{".mark-red":{"color":"ff0000"},".mark-help":{"color":"00aa00"}}"#,
    );
    document.layout();
    let span = first(&document, element, "span");
    let attributes: Vec<(String, String)> = document
        .get(span)
        .unwrap()
        .attributes()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    assert_eq!(
        attributes,
        [("class".to_owned(), "mark-red other".to_owned())]
    );
    assert_eq!(value(&document, span, "color"), "rgb(255, 0, 0)");
    let paragraphs = all(&document, element, "p");
    let help = paragraphs
        .iter()
        .copied()
        .find(|p| document.get(*p).unwrap().has_class("mark-help"))
        .expect("the kept p");
    assert_eq!(value(&document, help, "color"), "rgb(0, 170, 0)");
    assert_eq!(
        all(&document, element, "div"),
        [root(&document, element)],
        "no `div` but the root"
    );
    assert!(
        outline(&document, element)
            .contains("\"<div class=\\\"not-allowed\\\">Not allowed</div>\"")
    );
}

/// web-core's `table.html` fixture: the cells stand in two columns and two
/// rows of a grid.
#[test]
fn a_table_places_its_cells_in_columns() {
    let (document, _events, element) = rendered("\n| A | B |\n| - | :-: |\n| 1 | 2 |\n");
    let headers = all(&document, element, "th");
    let cells = all(&document, element, "td");
    let (a, b, one, two) = (
        rect(&document, headers[0]),
        rect(&document, headers[1]),
        rect(&document, cells[0]),
        rect(&document, cells[1]),
    );
    // 2px spacing around and between the cells, 1px cell padding, one Ahem
    // glyph each.
    assert_eq!(a, (2.0, 2.0, 12.0, 16.0));
    assert_eq!(b, (16.0, 2.0, 12.0, 16.0));
    assert_eq!(one, (2.0, 20.0, 12.0, 16.0));
    assert_eq!(two, (16.0, 20.0, 12.0, 16.0));
    assert_eq!(value(&document, headers[0], "font-weight"), "700");
    assert_eq!(value(&document, headers[0], "text-align"), "center");
    assert_eq!(value(&document, cells[1], "text-align"), "center");
    assert_eq!(value(&document, cells[0], "text-align"), "start");
}

/// web-core's `image.html` fixture: the image is the engine's `image`,
/// sized from its bitmap.
#[test]
fn an_image_is_an_auto_sized_image_element() {
    let (document, _events, element) =
        rendered("![alt text](/tests/fixtures/resources/firefox-logo.png)");
    let image = first(&document, element, "image");
    let node = document.get(image).unwrap();
    assert_eq!(
        node.attribute("src"),
        Some("/tests/fixtures/resources/firefox-logo.png")
    );
    assert_eq!(node.attribute("auto-size"), Some(""));
    assert_eq!(node.attribute("alt"), Some("alt text"));
    assert_eq!(node.parent().unwrap().tag_name(), Some("p"));
}

/// web-core's `inlineview.html` fixture: the light child named by the slot
/// sits in the line of text.
#[test]
fn an_inline_view_shows_its_slotted_child_in_the_line() {
    let (mut document, _events, element) = markdown(AHEM_HOST);
    let badge = document.create_element("view", ());
    document.set_attribute(badge, "slot", "badge");
    document.set_inline_style(badge, "width: 10px; height: 10px");
    document.append_child(element, badge);
    document.set_attribute(
        element,
        "content",
        "inline view here ![](inlineview://badge) next",
    );
    document.layout();
    let slot = first(&document, element, "slot");
    assert_eq!(document.get(slot).unwrap().attribute("name"), Some("badge"));
    let container = document.get(slot).unwrap().parent().unwrap();
    assert!(container.has_class("md-inline-view"));
    assert_eq!(container.parent().unwrap().tag_name(), Some("p"));
    assert_eq!(document.assigned_nodes(slot), [badge]);
    // "inline view here " is 17 glyphs; the badge sits in the first 14px
    // line, its bottom on the baseline.
    let (x, y, width, height) = rect(&document, badge);
    assert_eq!((x, width, height), (170.0, 10.0, 10.0));
    assert!((0.0..=4.0).contains(&y), "{y}");
    assert_eq!(rect(&document, first(&document, element, "p")).3, 14.0);
}

/// web-core's `incremental.html` fixture: appended content keeps the
/// elements the earlier content built.
#[test]
fn appended_content_keeps_the_elements_of_unchanged_blocks() {
    let (mut document, _events, element) = markdown(AHEM_HOST);
    document.set_attribute(element, "content", "Line 1\n");
    let line = first(&document, element, "p");
    document.set_attribute(element, "content", "Line 1\n\nLine 2\n");
    let paragraphs = all(&document, element, "p");
    assert_eq!(paragraphs.len(), 2);
    assert_eq!(
        paragraphs[0], line,
        "the first paragraph is the same element"
    );
    // A change inside the first block rebuilds it and what follows.
    document.set_attribute(element, "content", "Line one\n\nLine 2\n");
    let rebuilt = all(&document, element, "p");
    assert_ne!(rebuilt[0], line);
    assert!(
        document.get(line).is_none(),
        "the replaced paragraph is freed"
    );
    // Empty content empties the root.
    document.set_attribute(element, "content", "");
    assert_eq!(outline(&document, element), "");
}

/// `content-range` renders a UTF-16 slice of `content`; an invalid value
/// keeps the range in force, and removing it renders everything.
#[test]
fn content_range_renders_a_slice() {
    let (mut document, _events, element) = markdown(AHEM_HOST);
    document.set_attribute(element, "content", "first\n\nsecond");
    document.set_attribute(element, "content-range", "[7, 13]");
    assert_eq!(outline(&document, element), "p\n  \"second\"\n");
    document.set_attribute(element, "content-range", "nonsense");
    assert_eq!(outline(&document, element), "p\n  \"second\"\n");
    document.set_attribute(element, "content-range", "0,5");
    assert_eq!(outline(&document, element), "p\n  \"first\"\n");
    document.remove_attribute(element, "content-range");
    assert_eq!(
        outline(&document, element),
        "p\n  \"first\"\np\n  \"second\"\n"
    );
}

/// `parseEnd` follows every render of non-empty content, bubbling, with the
/// `content-id` when there is one.
#[test]
fn parse_end_is_queued_per_render_with_the_content_id() {
    let (mut document, events, element) = markdown("");
    document.set_attribute(element, "content", "one");
    document.set_attribute(element, "content-id", "doc-1");
    document.set_attribute(element, "content", "two");
    document.set_attribute(element, "content", "");
    assert_eq!(
        events.take(),
        [
            ComponentEvent::Detailed {
                node: element,
                name: "parseEnd",
                bubbles: true,
                detail: ComponentDetail::Id { id: None },
            },
            ComponentEvent::Detailed {
                node: element,
                name: "parseEnd",
                bubbles: true,
                detail: ComponentDetail::Id {
                    id: Some("doc-1".to_owned()),
                },
            },
        ]
    );
}

/// The rendered text and `getParseResult`'s ranges, against what markdown-it
/// 15 and a browser's `textContent` give for the same content (computed
/// under Node with jsdom, web-core's pipeline).
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one table of contents against their web-core textContent"
)]
fn get_parse_result_counts_offsets_in_web_cores_text_content() {
    let (mut document, _events, element) = rendered(BASIC);
    assert_eq!(
        rendered_text(&mut document, element),
        "Title\nthis is bold and italic.\n\nitem 1\nitem 2\n\ninline code: code.\nblock code\n\n"
    );
    assert_eq!(
        parse_result(
            &mut document,
            element,
            &[
                "p", "h1", "li", "ul", "strong", "em", "code", "pre", "a", "img"
            ]
        ),
        [
            "id=",
            "p:[6,30][47,65]",
            "h1:[0,5]",
            "li:[32,38][39,45]",
            "ul:[31,46]",
            "strong:[14,18]",
            "em:[23,29]",
            "code:[60,64][66,77]",
            "pre:[66,77]",
            "a:",
            "img:",
        ]
    );

    let (mut document, _events, element) = rendered("\n| A | B |\n| - | - |\n| 1 | 2 |\n");
    assert_eq!(
        rendered_text(&mut document, element),
        "\n\n\nA\nB\n\n\n\n\n1\n2\n\n\n\n"
    );
    assert_eq!(
        parse_result(&mut document, element, &["table", "th", "td", "tr"]),
        [
            "id=",
            "table:[0,17]",
            "th:[3,4][5,6]",
            "td:[11,12][13,14]",
            "tr:[2,7][10,15]",
        ]
    );

    let (mut document, _events, element) =
        rendered("- a\n\n- b\n  - c\n\n> q1\n>\n> q2\n\n---\n\nx  \ny\nz");
    assert_eq!(
        rendered_text(&mut document, element),
        "\n\na\n\n\nb\n\nc\n\n\n\n\nq1\nq2\n\n\nx\ny\nz\n"
    );
    assert_eq!(
        parse_result(
            &mut document,
            element,
            &["p", "li", "ul", "blockquote", "br", "hr"]
        ),
        [
            "id=",
            "p:[2,3][6,7][15,17][18,20][23,28]",
            "li:[1,4][5,12][9,10]",
            "ul:[0,13][8,11]",
            "blockquote:[14,21]",
            "br:[0,0]",
            "hr:[0,0]",
        ]
    );

    let (mut document, _events, element) = rendered("1. one\n2. two\n\n3) x\n\n7. seven");
    assert_eq!(
        rendered_text(&mut document, element),
        "\none\ntwo\n\n\nx\n\n\nseven\n\n"
    );
    assert_eq!(
        parse_result(&mut document, element, &["ol", "li"]),
        [
            "id=",
            "ol:[0,9][10,13][14,21]",
            "li:[1,4][5,8][11,12][15,20]"
        ]
    );

    let (mut document, _events, element) = rendered("- a\n  # h\n- b");
    assert_eq!(rendered_text(&mut document, element), "\na\nh\n\nb\n\n");
    assert_eq!(
        parse_result(&mut document, element, &["H1", "li", "ul"]),
        ["id=", "H1:[3,4]", "li:[1,5][6,7]", "ul:[0,8]"]
    );

    // Raw HTML blocks, through web-core's raw-HTML policy and DOMPurify: a
    // kept block's last line break is a text node after its elements, a
    // refused block's is part of its text, and the last block of the
    // content has none.
    let (mut document, _events, element) = rendered(
        "### Hello <span class=\"mark-red other\" style=\"color: red\" onclick=\"noop()\">World</span>\n\n<p class=\"mark-help\" style=\"font-size: 12px\">Paragraph</p>\n\n<div class=\"not-allowed\">Not allowed</div>",
    );
    assert_eq!(
        rendered_text(&mut document, element),
        "Hello World\nParagraph\n<div class=\"not-allowed\">Not allowed</div>"
    );
    assert_eq!(
        parse_result(&mut document, element, &["span", "p", "h3"]),
        ["id=", "span:[6,11]", "p:[12,21]", "h3:[0,11]"]
    );

    let (mut document, _events, element) = rendered(
        "<p class=\"a\">one</p>\n<p>two</p>\n\nafter\n\n<div class=\"no\">x</div>\n\n<p>end</p>",
    );
    assert_eq!(
        rendered_text(&mut document, element),
        "one\ntwo\nafter\n<div class=\"no\">x</div>\nend"
    );
    assert_eq!(
        parse_result(&mut document, element, &["p"]),
        ["id=", "p:[0,3][4,7][8,13][38,41]"]
    );
    // The line break between the two raw paragraphs lays out nothing: the
    // second sits under the first's 0.75em margin, as with none.
    let paragraphs = all(&document, element, "p");
    assert_eq!(rect(&document, paragraphs[1]).1, 22.0);

    let (mut document, _events, element) = rendered("- <p>in</p>\n\n> <p>q</p>\n> tail");
    assert_eq!(
        rendered_text(&mut document, element),
        "\n\nin\n\n\n\nq\ntail\n"
    );
    assert_eq!(
        parse_result(&mut document, element, &["p", "li", "ul", "blockquote"]),
        [
            "id=",
            "p:[2,4][8,9]",
            "li:[1,5]",
            "ul:[0,6]",
            "blockquote:[7,14]"
        ]
    );

    let (mut document, _events, element) =
        rendered("this is [link](https://example.com).\n\n![img](/a.png)");
    document.set_attribute(element, "content-id", "case-1");
    assert_eq!(
        parse_result(&mut document, element, &["p", "a", "img", "md-line"]),
        [
            "id=case-1",
            "p:[0,13][0,0]",
            "a:[8,12]",
            "img:[0,0]",
            "md-line:"
        ]
    );
}

#[test]
fn get_parse_result_refuses_params_without_tags() {
    let (mut document, _events, element) = rendered("text");
    let invalid = MethodOutcome::Failed(MethodError::InvalidParams);
    for params in ["{}", r#"{"tags":[]}"#, r#"{"tags":"p"}"#, "null", "[]"] {
        assert_eq!(
            invoke(&mut document, element, "getParseResult", params),
            invalid,
            "{params}"
        );
    }
    // A name that is not a string is skipped, as native skips it.
    assert_eq!(
        parse_result_raw(&mut document, element, r#"{"tags":[1,"p"]}"#),
        ["", "1", "p", "1", "0", "4"]
    );
}

fn parse_result_raw(document: &mut LynxDocument, element: NodeId, params: &str) -> Vec<String> {
    fields(&invoke(document, element, "getParseResult", params))
}

#[test]
fn get_content_slices_with_an_inclusive_end() {
    let (mut document, _events, element) = rendered("hello *world*");
    let content = |document: &mut LynxDocument, params: &str| {
        fields(&invoke(document, element, "getContent", params))
    };
    assert_eq!(content(&mut document, "{}"), ["hello *world*"]);
    assert_eq!(content(&mut document, "null"), ["hello *world*"]);
    assert_eq!(content(&mut document, r#"{"start":0,"end":4}"#), ["hello"]);
    assert_eq!(content(&mut document, r#"{"start":6}"#), ["*world*"]);
    assert_eq!(content(&mut document, r#"{"end":0}"#), ["h"]);
    assert_eq!(content(&mut document, r#"{"start":-3,"end":1}"#), ["he"]);
    assert_eq!(content(&mut document, r#"{"start":5,"end":2}"#), [""]);
    assert_eq!(
        content(&mut document, r#"{"start":2,"end":99}"#),
        ["llo *world*"]
    );
    assert_eq!(
        invoke(&mut document, element, "getContent", r#"{"start":"1"}"#),
        MethodOutcome::Failed(MethodError::InvalidParams)
    );
}

#[test]
fn get_images_lists_every_image_source_in_tree_order() {
    let (mut document, _events, element) =
        rendered("![a](x.png) and ![b]()\n\n- ![c](y.png)\n\n![](inlineview://v)");
    assert_eq!(
        fields(&invoke(&mut document, element, "getImages", "{}")),
        ["2", "x.png", "y.png"]
    );
}

/// The `{start, end}` range of rendered text, on one line and across
/// paragraphs, in viewport px; and the character under a point.
#[test]
fn get_text_bounding_rect_and_get_char_index_by_point_read_the_rendered_text() {
    let (mut document, _events, element) = markdown(AHEM_HOST);
    document.set_attribute(
        element,
        "content",
        "this is [link](https://a.com) end\n\nnext",
    );
    document.layout();
    let boxes = |document: &mut LynxDocument, params: &str| {
        let fields = fields(&invoke(document, element, "getTextBoundingRect", params));
        fields
            .iter()
            .skip(1)
            .map(|field| field.parse::<f32>().unwrap())
            .collect::<Vec<_>>()
    };
    // "link" is rendered 8..12: the first paragraph's line, 80px in.
    assert_eq!(
        boxes(&mut document, r#"{"start":8,"end":12}"#),
        [80.0, 0.0, 40.0, 14.0]
    );
    // The same characters by their source offsets, `[link]` at 9..13.
    assert_eq!(
        boxes(
            &mut document,
            r#"{"start":9,"end":13,"indexType":"source"}"#
        ),
        [80.0, 0.0, 40.0, 14.0]
    );
    // From " end" across the "\n" between the paragraphs into "next": one
    // box in each paragraph. The second starts after the first's 0.75em
    // bottom margin, at 21.5px, which the rounded layout puts at 22.
    assert_eq!(
        boxes(&mut document, r#"{"start":12,"end":19}"#),
        [120.0, 0.0, 40.0, 14.0, 0.0, 22.0, 20.0, 14.0]
    );
    let operation = MethodOutcome::Failed(MethodError::Operation);
    // No range is the selection, and there is none.
    assert_eq!(
        invoke(&mut document, element, "getTextBoundingRect", "{}"),
        operation
    );
    assert_eq!(
        invoke(
            &mut document,
            element,
            "getTextBoundingRect",
            r#"{"start":"a"}"#
        ),
        MethodOutcome::Failed(MethodError::InvalidParams)
    );

    let index = |document: &mut LynxDocument, params: &str| {
        fields(&invoke(document, element, "getCharIndexByPoint", params))
    };
    assert_eq!(index(&mut document, r#"{"x":85,"y":5}"#), ["8"]);
    assert_eq!(
        index(&mut document, r#"{"x":85,"y":5,"indexType":"source"}"#),
        ["9"]
    );
    // "next" is rendered 17..21, on the second paragraph's line.
    assert_eq!(index(&mut document, r#"{"x":15,"y":25}"#), ["18"]);
    assert_eq!(
        invoke(
            &mut document,
            element,
            "getCharIndexByPoint",
            r#"{"x":295,"y":5}"#
        ),
        operation,
        "past the end of the line"
    );
    assert_eq!(
        invoke(&mut document, element, "getCharIndexByPoint", r#"{"x":1}"#),
        MethodOutcome::Failed(MethodError::InvalidParams)
    );
    // The unbuilt methods are the kind's unknown names.
    for name in [
        "pauseAnimation",
        "resumeAnimation",
        "getSelectedText",
        "setTextSelection",
    ] {
        assert_eq!(
            invoke(&mut document, element, name, "{}"),
            MethodOutcome::NotFound,
            "{name}"
        );
    }
}

/// Taps the topmost element the committed frame shows at `(x, y)`, as the
/// runtime delivers a `tap` to the component whose shadow content it hit.
fn tap(document: &mut LynxDocument, x: f32, y: f32) {
    let hit = document
        .commit()
        .hit(Point2D::new(x, y), &|_| None, None)
        .expect("something is under the point")
        .node;
    if document.shadow_root_of(hit).is_some() {
        document.dispatch_element_event(hit, ElementEventKind::Tap { x, y }, true, false);
    }
}

#[test]
fn a_tap_on_a_link_or_an_image_queues_web_cores_events() {
    let (mut document, events, element) = markdown(AHEM_HOST);
    document.set_attribute(element, "content-id", "case-1");
    document.set_attribute(
        element,
        "content",
        "this is [**li**nk](https://example.com).\n\n![img](/a.png)",
    );
    document.set_attribute(
        element,
        "markdown-style",
        r#"{"image":{"width":20,"height":20}}"#,
    );
    document.layout();
    events.take();

    // A glyph of the link, inside its `strong` and outside it.
    for x in [85.0, 105.0] {
        tap(&mut document, x, 5.0);
        assert_eq!(
            events.take(),
            [ComponentEvent::Detailed {
                node: element,
                name: "link",
                bubbles: true,
                detail: ComponentDetail::UrlContent {
                    url: "https://example.com".to_owned(),
                    content: "link".to_owned(),
                    content_id: Some("case-1".to_owned()),
                },
            }],
            "x = {x}"
        );
    }
    // Plain text, and the paragraph's box past its text.
    for x in [15.0, 250.0] {
        tap(&mut document, x, 5.0);
        assert_eq!(events.take(), [], "x = {x}");
    }
    let image = first(&document, element, "image");
    let (x, y, _, _) = rect(&document, image);
    tap(&mut document, x + 5.0, y + 5.0);
    assert_eq!(
        events.take(),
        [ComponentEvent::Detailed {
            node: element,
            name: "imageTap",
            bubbles: true,
            detail: ComponentDetail::Url {
                url: "/a.png".to_owned(),
                content_id: Some("case-1".to_owned()),
            },
        }]
    );
}

/// A code block keeps its line breaks and does not wrap, and scrolls
/// sideways in its `pre`; runs of spaces collapse, `preserve-breaks` being
/// the one preserving mode the engine has.
#[test]
fn a_code_block_keeps_its_lines_without_wrapping() {
    let (mut document, _events, element) = markdown(AHEM_HOST);
    document.set_attribute(
        element,
        "markdown-style",
        r#"{"codeBlock":{"fontFamily":"Ahem"}}"#,
    );
    document.set_attribute(
        element,
        "content",
        "```\nabc\n  d\nlong line of code that is wider than the element\n```",
    );
    document.layout();
    let code = first(&document, element, "code");
    let size = document.text_block_size(code).expect("the code paragraph");
    // Three lines, the longest 48 glyphs on one line.
    assert_eq!(size.height, 42.0);
    assert_eq!(size.width, 480.0);
    let pre = first(&document, element, "pre");
    assert_eq!(
        rect(&document, pre).2,
        300.0,
        "the pre stays the element's width"
    );
    // `  d`'s two spaces collapse away at the start of its line.
    let boxes = document
        .text_range_boxes(code, 4, 7)
        .expect("the second line");
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].size.width, 10.0);
}

/// The definition's entry for a host goes with the host's free — the
/// runtime frees a collected element with `drop_element` — and not with a
/// removal, after which the element can come back.
#[test]
fn a_freed_host_drops_its_state() {
    let (mut document, _events, element) = rendered("one");
    let other = child(&mut document, X_MARKDOWN_TAG, "");
    let count = |document: &mut LynxDocument| {
        fields(&invoke(document, other, super::STATE_COUNT_METHOD, "{}")).remove(0)
    };
    assert_eq!(count(&mut document), "2");
    document.remove_element(element);
    assert_eq!(
        count(&mut document),
        "2",
        "a removed element keeps its state"
    );
    document.drop_element(element);
    assert!(document.get(element).is_none());
    assert_eq!(count(&mut document), "1");
}
