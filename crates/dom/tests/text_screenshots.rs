//! Golden screenshots of DOM's private text painter.
//!
//! Each case exists to be *looked at* as much as to be diffed: glyph
//! rasterization quality, synthesis, and how decorations sit against real
//! letterforms are all things a numeric assertion cannot judge but a reviewer
//! can see at a glance. `tests/gpu_pixels.rs` covers the complementary half —
//! Ahem's solid em squares make individual pixels assertable, at the cost of
//! showing nothing about rendering quality.
//!
//! Every fixture renders **vendored Roboto**, never a host font; see
//! `support/screenshot.rs` for why that is not optional. Refresh with:
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p dom --test text_screenshots`.

#[path = "support/html.rs"]
mod html;
mod paint_common;
#[path = "support/screenshot.rs"]
mod screenshot;

use flashbulb::Image;

fn assert_text_golden(case: &str, actual: &Image) {
    screenshot::assert_golden(&["text", case], actual);
}

#[test]
fn text_specimen() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 420px; height: 300px; padding: 16px 20px; gap: 2px; box-sizing: border-box; background-color: white; font-family: Roboto; color: #111827">
  <div style="display: flex; font-size: 11px" class="text-block">Sphinx of black quartz, judge my vow — 11px</div>
  <div style="display: flex; font-size: 13px" class="text-block">Sphinx of black quartz, judge my vow — 13px</div>
  <div style="display: flex; font-size: 16px" class="text-block">Sphinx of black quartz — 16px</div>
  <div style="display: flex; font-size: 22px" class="text-block">Sphinx of black — 22px</div>
  <div style="display: flex; font-size: 32px" class="text-block">Hamburgefonstiv</div>
  <div style="display: flex; font-size: 44px" class="text-block">Hamburg 0123</div>
  <div style="display: flex; font-size: 20px; font-weight: 700" class="text-block">Synthetic bold 700</div>
  <div style="display: flex; font-size: 20px; font-style: italic" class="text-block">Synthetic oblique</div>
  <div style="display: flex; font-size: 20px; color: #dc2626" class="text-block">Colored fill (#dc2626)</div>
</div>
"#;
    let actual = screenshot::capture("text_specimen", FRAGMENT, 420.0, 300.0);
    assert_text_golden("specimen", &actual);
}

#[test]
fn text_decorations() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 460px; height: 430px; padding: 16px 20px; gap: 6px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 26px; color: #111827">
  <div style="display: flex; text-decoration: underline" class="text-block">Underline solid</div>
  <div style="display: flex; text-decoration: underline double" class="text-block">Underline double</div>
  <div style="display: flex; text-decoration: underline dotted" class="text-block">Underline dotted</div>
  <div style="display: flex; text-decoration: underline dashed" class="text-block">Underline dashed</div>
  <div style="display: flex; text-decoration: underline wavy" class="text-block">Underline wavy</div>
  <div style="display: flex; text-decoration: line-through" class="text-block">Line-through solid</div>
  <div style="display: flex; text-decoration: line-through double" class="text-block">Line-through double</div>
  <div style="display: flex; text-decoration: underline line-through" class="text-block">Both lines at once</div>
  <div style="display: flex; text-decoration: underline #2563eb wavy" class="text-block">Decoration color wins</div>
  <div style="display: flex; text-decoration: underline #16a34a">
    <span style="display: flex; color: #dc2626" class="text-block">Propagated from the parent</span>
  </div>
</div>
"#;
    let actual = screenshot::capture("text_decorations", FRAGMENT, 460.0, 430.0);
    assert_text_golden("decorations", &actual);
}

#[test]
fn text_shadow_and_stroke() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 460px; height: 330px; padding: 16px 20px; gap: 12px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 28px; color: #111827">
  <div style="display: flex; text-shadow: 4px 4px 0px #93c5fd" class="text-block">Offset shadow, no blur</div>
  <div style="display: flex; text-shadow: 4px 4px 6px #9ca3af" class="text-block">Blur radius is ignored</div>
  <div style="display: flex; color: white; text-shadow: 2px 2px 0px #111827" class="text-block">White fill, dark shadow</div>
  <div style="display: flex; text-stroke: 1px #2563eb" class="text-block">Thin stroke over fill</div>
  <div style="display: flex; color: #fde68a; text-stroke: 3px #b45309" class="text-block">Heavy stroke over fill</div>
  <div style="display: flex; text-decoration: underline; text-shadow: 4px 4px 0px #fca5a5" class="text-block">Shadow takes the underline</div>
</div>
"#;
    let actual = screenshot::capture("text_shadow_and_stroke", FRAGMENT, 460.0, 330.0);
    assert_text_golden("shadow-and-stroke", &actual);
}

#[test]
fn text_paragraph() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 420px; height: 420px; padding: 16px 20px; gap: 10px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 15px; color: #1f2937">
  <div style="display: flex; width: 380px; line-height: 1.5" class="text-block">Typography is the craft of arranging type to make written language legible, readable and appealing when displayed.</div>
  <div style="display: flex; width: 380px; line-height: 2.2; background-color: #f3f4f6" class="text-block">Loose line-height at 2.2 spreads these wrapped lines apart, and the band of background behind them shows where each line box sits.</div>
  <div style="display: flex; width: 380px; letter-spacing: 2px" class="text-block">Letter-spacing of two pixels</div>
  <div style="display: flex; width: 380px; text-align: center; background-color: #f3f4f6" class="text-block">Centered text, wrapped over enough lines that the last one has room left over to be centered in.</div>
  <div style="display: flex; width: 380px; text-align: right; background-color: #f3f4f6" class="text-block">Right-aligned text, wrapped over enough lines that the last one has room left over to sit against the right edge.</div>
  <div style="display: flex; width: 380px; text-indent: 28px; background-color: #f3f4f6" class="text-block">Indented first line, then the rest of the paragraph wraps back to the left edge of the box.</div>
</div>
"#;
    let actual = screenshot::capture("text_paragraph", FRAGMENT, 420.0, 420.0);
    assert_text_golden("paragraph", &actual);
}

#[test]
fn text_color_gradient() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 440px; height: 400px; padding: 16px 20px; gap: 12px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 32px; font-weight: 700">
  <div style="display: flex; color: linear-gradient(90deg, #f43f5e, #6366f1, #14b8a6)" class="text-block">Linear ramp</div>
  <div style="display: flex; color: radial-gradient(circle, #f59e0b, #7c3aed)" class="text-block">Radial ramp</div>
  <div style="display: flex; color: conic-gradient(#ef4444, #22c55e, #3b82f6, #ef4444)" class="text-block">Conic ramp</div>
  <div style="display: flex; padding-left: 40px; color: linear-gradient(90deg, #f43f5e, #14b8a6)" class="text-block">Padded</div>
  <div style="display: flex; width: 320px; font-size: 22px; color: linear-gradient(180deg, #f43f5e, #14b8a6)" class="text-block">This wraps onto three lines, so a vertical ramp has to walk rose to teal down them.</div>
  <div style="display: flex; font-size: 26px; text-decoration: underline; color: linear-gradient(90deg, #f59e0b, #ef4444)" class="text-block">Underline stays solid</div>
</div>
"#;
    let actual = screenshot::capture("text_color_gradient", FRAGMENT, 440.0, 400.0);
    assert_text_golden("color-gradient", &actual);
}

#[test]
fn text_color_gradient_over_background() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 460px; height: 400px; padding: 16px 20px; gap: 14px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 30px; font-weight: 700">
  <div style="display: flex; background-color: #111827; color: linear-gradient(90deg, #fde68a, #f43f5e)" class="text-block">Solid backdrop</div>
  <div style="display: flex; border: 8px solid #0f766e; padding: 4px; background-color: #111827; color: linear-gradient(90deg, #fde68a, #f43f5e)" class="text-block">Border moves it</div>
  <div style="display: flex; background-image: linear-gradient(90deg, #1e3a8a, #111827); color: linear-gradient(270deg, #fde68a, #f43f5e)" class="text-block">Two ramps, opposed</div>
  <div style="display: flex; padding: 2px 10px; border-radius: 16px; background-color: #4c1d95; color: linear-gradient(90deg, #fde68a, #5eead4)" class="text-block">Rounded backdrop</div>
  <div style="display: flex; background-clip: text; background-color: #111827; color: linear-gradient(90deg, #fbbf24, #a855f7)" class="text-block">Clip plus gradient</div>
</div>
"#;
    let actual = screenshot::capture(
        "text_color_gradient_over_background",
        FRAGMENT,
        460.0,
        400.0,
    );
    assert_text_golden("color-gradient-over-background", &actual);
}

#[test]
fn text_background_clip() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 440px; height: 260px; padding: 16px 20px; gap: 14px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 40px; font-weight: 700">
  <div style="display: flex; color: transparent; background-clip: text; background-image: linear-gradient(90deg, #f43f5e, #6366f1, #14b8a6)" class="text-block">Gradient ink</div>
  <div style="display: flex; color: transparent; background-clip: text; background-color: #7c3aed" class="text-block">Solid ink</div>
  <div style="display: flex; color: transparent; text-decoration: underline #111827; background-clip: text; background-image: linear-gradient(90deg, #f59e0b, #ef4444)" class="text-block">Bare underline</div>
</div>
"#;
    let actual = screenshot::capture("text_background_clip", FRAGMENT, 440.0, 260.0);
    assert_text_golden("background-clip", &actual);
}

/// The same paragraph through generated and literal runs must draw identical
/// pixels, including the element's color/decoration and a paint-only update.
#[test]
fn generated_text_matches_literal_runs() {
    let mut generated = html::parse(
        r#"<div class="text-block" style="width: 240px; height: 80px; font-family: Roboto; font-size: 24px"><span class="generated" style="display: contents">suppressed</span>after</div>"#,
        240.0,
        80.0,
    );
    let carrier = generated
        .dom
        .query_selector(generated.root, ".generated")
        .unwrap()
        .unwrap();
    // The fragment helper imports only class/style attributes.
    generated.dom.set_attribute(carrier, "text", "Before ");
    generated
        .dom
        .register_fonts(dom::FontBlob::from_static(screenshot::ROBOTO));
    generated.dom.add_stylesheet(".generated { content: attr(text); color: red; text-decoration: underline; font-size: 30px; }", dom::StylesheetOrigin::Author);
    for color in ["red", "blue"] {
        generated.dom.add_stylesheet(
            &format!(".generated {{ color: {color}; }}"),
            dom::StylesheetOrigin::Author,
        );
        let actual = screenshot::capture_prebuilt_document(
            "generated-text",
            &mut generated.dom,
            &dom::NoImages,
        );
        let expected = screenshot::capture(
            "literal-reference",
            &format!(
                r#"<div class="text-block" style="width: 240px; height: 80px; font-family: Roboto; font-size: 24px; color: black"><span style="display: contents; color: {color}; text-decoration: underline; font-size: 30px">Before </span>after</div>"#
            ),
            240.0,
            80.0,
        );
        assert_eq!(
            actual, expected,
            "generated text must use its element style ({color})"
        );
    }
}

/// The flow layout an engine sheet writes, which an author sheet cannot:
/// `.flow` a block container, `.inline` an inline box in its inline
/// formatting context. A UA sheet stands in for the engine's own here.
const FLOW_SHEET: &str = ".flow { display: block; } .inline { display: inline; }";

/// Inline boxes in a block container's inline formatting context, with
/// `box-decoration-break: slice`: a padded, bordered, rounded box wrapped
/// over two lines shows its start border, padding and corners on the first
/// line only and its end ones on the last; a gradient background runs across
/// the wrap as one strip; a nested box paints over its parent's background;
/// an empty box with a border and padding still takes up its line; vertical
/// padding paints past the line box without moving it.
#[test]
fn text_inline_boxes() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 440px; height: 260px; padding: 16px 20px; gap: 18px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 20px; color: #111827">
  <div class="flow" style="width: 300px; line-height: 1.8">Text before <span class="inline" style="background-color: #fde68a; border: 2px solid #b45309; border-radius: 8px; padding: 2px 10px">a padded box that wraps onto the next line</span> and after.</div>
  <div class="flow" style="width: 330px; line-height: 1.8">Nested <span class="inline" style="background-color: #bfdbfe; padding: 0 6px">outer <span class="inline" style="background-color: #1d4ed8; color: white; border-radius: 4px; padding: 0 4px">inner</span> box</span>, an empty <span class="inline" style="border-left: 3px solid #dc2626; padding-right: 12px; background-color: #fecaca"></span> one, and <span class="inline" style="background-image: linear-gradient(90deg, #f43f5e, #6366f1); color: white; padding: 0 8px">a gradient that runs across the wrap</span>.</div>
</div>
"#;
    let mut doc = html::parse(FRAGMENT, 440.0, 260.0);
    doc.dom
        .register_fonts(dom::FontBlob::from_static(screenshot::ROBOTO));
    doc.dom
        .add_stylesheet(FLOW_SHEET, dom::StylesheetOrigin::UserAgent);
    let actual =
        screenshot::capture_prebuilt_document("text_inline_boxes", &mut doc.dom, &dom::NoImages);
    assert_text_golden("inline-boxes", &actual);
}

/// A nested inline box sliced across a wrap, inside a box that is sliced
/// too: each box shows its start border, padding and corners on its first
/// fragment only and its end ones on its last, its middle fragments are
/// open on both sides, and the inner box's fragments paint over the outer
/// box's on every line.
#[test]
fn text_nested_inline_box_across_a_wrap() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 300px; height: 200px; padding: 16px 20px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 20px; color: #111827">
  <div class="flow" style="width: 220px; line-height: 1.8">Start <span class="inline" style="background-color: #bfdbfe; border: 2px solid #1d4ed8; border-radius: 6px; padding: 0 8px">outer <span class="inline" style="background-color: #1d4ed8; color: white; border: 2px solid #f59e0b; border-radius: 4px; padding: 0 6px">inner text that wraps over lines</span> tail</span> end.</div>
</div>
"#;
    let mut doc = html::parse(FRAGMENT, 300.0, 200.0);
    doc.dom
        .register_fonts(dom::FontBlob::from_static(screenshot::ROBOTO));
    doc.dom
        .add_stylesheet(FLOW_SHEET, dom::StylesheetOrigin::UserAgent);
    let actual = screenshot::capture_prebuilt_document(
        "text_nested_inline_box_across_a_wrap",
        &mut doc.dom,
        &dom::NoImages,
    );
    assert_text_golden("nested-inline-box-wrap", &actual);
}

/// One focused editing host in a `width` × `height` page, Roboto at 24px
/// with no host font behind it, edited by `edit` before the capture.
///
/// The document's text context is adopted without system fonts, so a
/// character Roboto lacks — the emoji below — renders as Roboto's own
/// `.notdef` box on every machine rather than as whichever emoji face the
/// host has: the golden is about where the selection and the caret fall
/// around the surrogate pair, not about the emoji's picture.
fn editing_golden(
    case: &str,
    value: &str,
    host_width: f32,
    edit: impl FnOnce(&mut dom::Document<()>),
) {
    use dom::StylesheetOrigin;

    let (width, height) = (360.0, 120.0);
    let mut dom = dom::Document::new(paint_common::device(width, height), "div", ());
    dom.adopt_text_context(dom::TextContext::without_system_fonts());
    assert_eq!(
        dom.register_fonts(dom::FontBlob::from_static(screenshot::ROBOTO)),
        1
    );
    // The embedder's UA rule for an editing host (bobcat-core's sheet).
    dom.add_stylesheet(
        "[contenteditable]:not([contenteditable=\"false\" i]) \
         { display: -lynx-text !important; white-space: pre-wrap !important; }",
        StylesheetOrigin::UserAgent,
    );
    let root = dom.document_element().id();
    dom.set_inline_style(
        root,
        "display: flex; align-items: flex-start; width: 360px; height: 120px; padding: 16px; \
         box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 24px; \
         color: #111827",
    );
    let host = dom.create_element("div", ());
    dom.set_attribute(host, "contenteditable", "");
    dom.set_inline_style(
        host,
        &format!(
            "width: {host_width}px; padding: 4px 6px; background-color: #f3f4f6; line-height: 32px"
        ),
    );
    dom.append_child(root, host);
    let text = dom.create_text_node(value, ());
    dom.append_child(host, text);
    dom.layout();
    assert!(dom.focus(host));
    edit(&mut dom);
    let actual = screenshot::capture_prebuilt_document(case, &mut dom, &dom::NoImages);
    assert_text_golden(case, &actual);
}

#[test]
fn editing_caret_at_end() {
    editing_golden("editing-caret-at-end", "Caret at the end", 300.0, |dom| {
        use dom::editing::{Command, Direction, Granularity};
        assert!(
            dom.apply_editing(Command::Move {
                granularity: Granularity::Document,
                direction: Direction::Forward,
                extend: false,
            })
            .applied
        );
    });
}

#[test]
fn editing_selection_across_two_lines() {
    editing_golden(
        "editing-selection-two-lines",
        "A selection that runs over a wrapped line",
        240.0,
        |dom| {
            use dom::editing::Command;
            assert!(
                dom.apply_editing(Command::SetSelection { range: 2..28 })
                    .applied
            );
        },
    );
}

#[test]
fn editing_composition_underline() {
    editing_golden("editing-composition", "Typing ", 300.0, |dom| {
        use dom::editing::{Command, Direction, Granularity};
        dom.apply_editing(Command::Move {
            granularity: Granularity::Document,
            direction: Direction::Forward,
            extend: false,
        });
        assert!(
            dom.apply_editing(Command::SetComposition {
                text: "here".to_owned(),
                selection: 4..4,
            })
            .applied
        );
    });
}

#[test]
fn editing_emoji_boundary() {
    editing_golden("editing-emoji-boundary", "A😀B", 300.0, |dom| {
        use dom::editing::Command;
        // Exactly the surrogate pair's two units: the highlight covers its
        // one box and neither neighbour.
        assert!(
            dom.apply_editing(Command::SetSelection { range: 1..3 })
                .applied
        );
    });
}
