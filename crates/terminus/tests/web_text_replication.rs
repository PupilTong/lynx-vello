//! Replicas of `lynx-stack`'s web-platform text tests that land on `terminus`:
//! text re-layout when a run's content or position changes, percentage sizing
//! against a text block, the scrollable-target geometry a text block exposes,
//! and the paint of gradient-valued `color` and of atomic inline boxes.
//!
//! The reference is **web-core**, not native Lynx (`AGENTS.md`'s compatibility
//! target): where the browser-hosted implementation and the Android/iOS
//! engines disagree, what a `.web.bundle` does in a browser today is the
//! behavior these tests pin.
//!
//! Two adaptations run through the whole file. The originals are Playwright
//! screenshot tests over Lynx elements; `crates/terminus` has no Lynx element
//! vocabulary at all (it must not contain any), so a `<text>` is an element
//! each test defines as a Lynx text element (`common::define_lynx_text`), as
//! `crates/bobcat-core` defines the Lynx text tags, and a `text-maxline="n"`
//! attribute is spelled as the `--lynx-text-maxline` custom property the Lynx
//! UA sheet reflects it into. And every metric assertion shapes the
//! vendored Ahem face, whose glyphs are solid em squares: an advance is
//! exactly glyph-count x font-size and a glyph's ink is pixel-assertable,
//! which a full-page golden could never be.

#![allow(clippy::float_cmp, reason = "Ahem geometry is exact at these sizes")]

mod common;

use common::{Doc, device};
use prime_radiant::headless;
use terminus::link::peniko::Color;
use terminus::link::{FontBlob, FontFaceRequest, FontFaceSource, NodeId, Vector2D};

const AHEM: &[u8] = include_bytes!("../../seldon/tests/fixtures/Ahem.ttf");
const ROBOTO: &[u8] = include_bytes!("../../seldon/tests/fixtures/Roboto-Regular.ttf");

const LIMIT_PROPERTIES: &str = r#"
@property --lynx-text-maxline { syntax: "<integer>"; inherits: false; initial-value: 0; }
@property --lynx-text-maxlength { syntax: "<integer>"; inherits: false; initial-value: -1; }
@property --lynx-tail-color-convert { syntax: "<integer>"; inherits: false; initial-value: 0; }
@property --lynx-inline-truncation { syntax: "<integer>"; inherits: false; initial-value: 0; }
"#;

fn ink(doc: &Doc, holder: NodeId) -> (f32, f32) {
    let size = doc
        .dom
        .text_block_size(holder)
        .expect("the holder establishes a paragraph");
    (size.width, size.height)
}

fn rect(doc: &Doc, id: NodeId) -> (f32, f32, f32, f32) {
    let layout = doc.dom.rounded_layout(id).expect("node id is live");
    (
        layout.location.x,
        layout.location.y,
        layout.size.width,
        layout.size.height,
    )
}

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * width + x) * 4) as usize;
    pixels[index..index + 4].try_into().unwrap()
}

fn readback(test: &str, doc: &mut Doc, width: u32, height: u32) -> Vec<u8> {
    let mut gpu = headless(test);
    doc.dom.layout();
    doc.dom.render();
    let scene = doc.dom.scene(&terminus::link::NoImages);
    gpu.render(&scene, width, height, Color::WHITE)
        .expect("headless render")
}

fn is_red(color: [u8; 4]) -> bool {
    color[0] > 200 && color[1] < 60 && color[2] < 60
}

fn is_white(color: [u8; 4]) -> bool {
    color[0] > 240 && color[1] > 240 && color[2] > 240
}

fn is_black(color: [u8; 4]) -> bool {
    color[0] < 60 && color[1] < 60 && color[2] < 60
}

fn is_blue(color: [u8; 4]) -> bool {
    color[2] > 200 && color[0] < 60 && color[1] < 60
}

#[test]
fn moving_a_clamped_paragraph_does_not_rebreak_its_lines() {
    let mut doc = Doc::with_device(device(800.0, 600.0));
    doc.define_lynx_text("text");
    doc.add_ua_css(LIMIT_PROPERTIES);
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .wrap { display: flex; width: 100px; }
         .target { width: 100%; flex-shrink: 0;
                   font-family: Ahem; font-size: 20px; line-height: 20px;
                   --lynx-text-maxline: 3; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let wrap = doc.el(root, "view.wrap");
    let target = doc.el(wrap, "text.target");
    let run = doc
        .dom
        .create_text_node("aaaaa bbbbb ccccc ddddd eeeee fffff", ());
    doc.dom.append_child(target, run);
    doc.flush();

    assert_eq!(ink(&doc, target), (100.0, 60.0), "clamped to three lines");
    assert_eq!(rect(&doc, target), (0.0, 0.0, 100.0, 60.0));

    doc.set_inline(target, "position: relative; left: 200px");
    doc.flush();
    assert_eq!(
        ink(&doc, target),
        (100.0, 60.0),
        "a positional change must not re-break the paragraph",
    );
    assert_eq!(
        rect(&doc, target),
        (200.0, 0.0, 100.0, 60.0),
        "the block moved and kept its size",
    );

    doc.set_inline(
        target,
        "position: relative; left: 200px; --lynx-text-maxline: 2",
    );
    doc.flush();
    assert_eq!(
        ink(&doc, target),
        (100.0, 40.0),
        "the limit change alone re-breaks to two lines",
    );
    assert_eq!(rect(&doc, target), (200.0, 0.0, 100.0, 40.0));
}

#[test]
fn rewriting_a_clamped_text_node_reclamps_against_the_new_string() {
    let mut doc = Doc::with_device(device(800.0, 600.0));
    doc.define_lynx_text("text");
    doc.add_ua_css(LIMIT_PROPERTIES);
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .target { width: 100px; flex-shrink: 0;
                   word-break: break-all;
                   font-family: Ahem; font-size: 20px; line-height: 20px;
                   --lynx-text-maxline: 2; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let target = doc.el(root, "text.target");
    let run = doc.dom.create_text_node("b".repeat(30), ());
    doc.dom.append_child(target, run);
    doc.flush();

    assert_eq!(ink(&doc, target), (100.0, 40.0));

    doc.dom.set_text_node_data(run, "bb");
    doc.flush();
    assert_eq!(
        ink(&doc, target),
        (40.0, 20.0),
        "the short replacement is not clamped, and no cut of the old string \
         survives into it",
    );

    doc.dom.set_text_node_data(run, "a".repeat(100));
    doc.flush();
    assert_eq!(
        ink(&doc, target),
        (100.0, 40.0),
        "the second long string clamps afresh rather than restoring the first",
    );
}

#[test]
fn the_truncation_marker_wears_the_cut_run_s_colour_until_the_block_converts() {
    fn marker_and_prefix(convert: bool) -> ([u8; 4], [u8; 4], [u8; 4]) {
        let mut doc = Doc::with_device(device(200.0, 100.0));
        doc.define_lynx_text("text");
        doc.add_ua_css(LIMIT_PROPERTIES);
        doc.add_css(
            "page { display: flex; }
             .text { width: 100px; color: #000000;
                     word-break: break-all; text-overflow: ellipsis;
                     font-family: Ahem; font-size: 20px; line-height: 20px;
                     --lynx-text-maxline: 1; }
             .convert { --lynx-tail-color-convert: 1; }
             .run { color: #ff0000; }",
        );
        assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
        let root = doc.root;
        let text = doc.el(
            root,
            if convert {
                "text.text.convert"
            } else {
                "text.text"
            },
        );
        let lead = doc.dom.create_text_node("A", ());
        doc.dom.append_child(text, lead);
        let nested = doc.el(text, "text.run");
        let run = doc.dom.create_text_node("BBBBBBBBB", ());
        doc.dom.append_child(nested, run);

        let pixels = readback(
            "the_truncation_marker_wears_the_cut_run_s_colour_until_the_block_converts",
            &mut doc,
            200,
            100,
        );
        assert_eq!(ink(&doc, text), (100.0, 20.0));
        (
            pixel(&pixels, 200, 10, 10),
            pixel(&pixels, 200, 30, 10),
            pixel(&pixels, 200, 70, 10),
        )
    }

    let (lead, kept, marker) = marker_and_prefix(false);
    assert!(is_black(lead), "the block's own run stays black ({lead:?})");
    assert!(is_red(kept), "the kept nested square stays red ({kept:?})");
    assert!(
        is_red(marker),
        "and by default the marker takes that run's colour too ({marker:?})",
    );

    let (lead, kept, marker) = marker_and_prefix(true);
    assert!(is_black(lead), "conversion moves no other run ({lead:?})");
    assert!(is_red(kept), "including the run at the cut ({kept:?})");
    assert!(
        is_black(marker),
        "only the marker's fill becomes the block's own colour ({marker:?})",
    );
}

#[test]
fn a_shown_truncation_subtree_is_painted_at_the_clamp() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.define_lynx_text("text");
    doc.define_lynx_text("inline-truncation");
    doc.add_ua_css(LIMIT_PROPERTIES);
    doc.add_css(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .text { position: absolute; left: 0px; top: 0px;
                 width: 100px; word-break: break-all; color: #000000;
                 font-family: Ahem; font-size: 20px; line-height: 20px;
                 --lynx-text-maxline: 2; }
         .marker { color: #ff0000;
                   --lynx-inline-truncation: 1; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "text.text");
    let run = doc.dom.create_text_node("H".repeat(30), ());
    doc.dom.append_child(text, run);
    let marker = doc.el(text, "inline-truncation.marker");
    let marker_run = doc.dom.create_text_node("H", ());
    doc.dom.append_child(marker, marker_run);

    let pixels = readback(
        "a_shown_truncation_subtree_is_painted_at_the_clamp",
        &mut doc,
        200,
        100,
    );

    assert_eq!(ink(&doc, text), (100.0, 40.0));

    let marker_square = pixel(&pixels, 200, 70, 30);
    assert!(
        is_red(marker_square),
        "the truncation content takes the fourth square, the first the \
         two-unit minimum retreat freed ({marker_square:?})",
    );
    let freed_square = pixel(&pixels, 200, 90, 30);
    assert!(
        is_white(freed_square),
        "the second freed square stays empty: the retreat is bounded below by \
         two units, not by the content's one-square width ({freed_square:?})",
    );
    let kept_square = pixel(&pixels, 200, 50, 30);
    assert!(
        kept_square[0] < 60 && kept_square[1] < 60 && kept_square[2] < 60,
        "and the third square is still kept text ({kept_square:?})",
    );
    let first_line_end = pixel(&pixels, 200, 90, 10);
    assert!(
        first_line_end[0] < 60 && first_line_end[1] < 60 && first_line_end[2] < 60,
        "and the line above the cut is untouched ({first_line_end:?})",
    );
    assert!(
        is_white(pixel(&pixels, 200, 90, 50)),
        "nothing paints past the clamp",
    );
}

#[test]
fn a_truncation_atom_is_placed_at_the_clamp_and_hidden_without_one() {
    let mut doc = Doc::with_device(device(400.0, 200.0));
    doc.define_lynx_text("text");
    doc.add_ua_css(LIMIT_PROPERTIES);
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { width: 100px; word-break: break-all;
                 font-family: Ahem; font-size: 20px; line-height: 20px;
                 --lynx-text-maxline: 2; }
         .marker { --lynx-inline-truncation: 1; }
         .icon { display: flex; width: 40px; height: 20px; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;

    let mut block = |content: &str| {
        let text = doc.el(root, "text.text");
        let run = doc.dom.create_text_node(content, ());
        doc.dom.append_child(text, run);
        let marker = doc.el(text, "text.marker");
        let icon = doc.el(marker, "view.icon");
        let spare = doc.el(text, "text.marker");
        let spare_run = doc.dom.create_text_node("HHHHH", ());
        doc.dom.append_child(spare, spare_run);
        (text, icon)
    };
    let (overflowing, placed) = block(&"H".repeat(30));
    let (fitting, unused) = block("HHHHH");
    doc.flush();

    assert_eq!(ink(&doc, overflowing), (100.0, 44.0));
    assert_eq!(
        rect(&doc, placed).0,
        60.0,
        "the icon starts where the retreat left off",
    );
    assert_eq!(
        (rect(&doc, placed).2, rect(&doc, placed).3),
        (40.0, 20.0),
        "and keeps the box its own layout produced",
    );

    assert_eq!(
        ink(&doc, fitting),
        (100.0, 20.0),
        "one line, and neither truncation child is in it",
    );
    assert_eq!(
        rect(&doc, unused),
        (0.0, 0.0, 0.0, 0.0),
        "an atom the paragraph never showed generates no box",
    );
}

#[test]
fn a_shown_truncation_marker_paints_its_runs_and_a_background_behind_them_only() {
    fn readback_marker(content: &str) -> Vec<u8> {
        let mut doc = Doc::with_device(device(200.0, 100.0));
        doc.define_lynx_text("text");
        doc.add_ua_css(LIMIT_PROPERTIES);
        doc.add_css(
            "page { display: flex; align-items: flex-start; }
             .text { width: 100px; word-break: break-all;
                     color: #000000; font-family: Ahem; font-size: 20px;
                     line-height: 20px; --lynx-text-maxline: 2; }
             .marker { --lynx-inline-truncation: 1;
                       color: #ff0000; background-color: #0000ff; }",
        );
        assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
        let root = doc.root;
        let text = doc.el(root, "text.text");
        let run = doc.dom.create_text_node(content, ());
        doc.dom.append_child(text, run);
        let marker = doc.el(text, "text.marker");
        let marker_run = doc.dom.create_text_node(" H", ());
        doc.dom.append_child(marker, marker_run);
        readback(
            "a_shown_truncation_marker_paints_its_runs_and_a_background_behind_them_only",
            &mut doc,
            200,
            100,
        )
    }

    let shown = readback_marker(&"H".repeat(30));
    assert!(
        is_red(pixel(&shown, 200, 90, 30)),
        "the marker's own run paints, in the marker's colour",
    );
    assert!(
        is_blue(pixel(&shown, 200, 70, 22)) && is_blue(pixel(&shown, 200, 70, 38)),
        "and its background paints behind its own fragment, over the fragment's          whole content area",
    );
    assert!(
        (0..100).all(|y| (0..60).all(|x| !is_blue(pixel(&shown, 200, x, y)))),
        "and nowhere left of the cut: the marker has no box, so its background          is the inline fragment's and not the clamp line's",
    );
    assert!(
        (0..100).all(|y| (100..200).all(|x| !is_blue(pixel(&shown, 200, x, y)))),
        "and nowhere past the paragraph either",
    );
    assert!(
        (0..20).all(|y| (0..200).all(|x| !is_blue(pixel(&shown, 200, x, y)))),
        "and not on the line above the clamp, which holds none of its runs",
    );

    let hidden = readback_marker("HHHHH");
    assert!(
        (0..100).all(|y| (0..200).all(|x| {
            let color = pixel(&hidden, 200, x, y);
            !is_blue(color) && !is_red(color)
        })),
        "and with nothing to clamp neither its runs nor a background of its own \
         reach the frame",
    );
}

#[test]
fn a_nested_scope_s_background_paints_behind_its_own_fragments_only() {
    let mut doc = Doc::with_device(device(300.0, 60.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { width: 300px; color: #000000;
                 font-family: Ahem; font-size: 20px; line-height: 40px; }
         .tag { color: transparent;
                background-color: #0000ff; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "text.text");
    let before = doc.dom.create_text_node("AAA", ());
    doc.dom.append_child(text, before);
    let tag = doc.el(text, "text.tag");
    let nested = doc.dom.create_text_node("BBB", ());
    doc.dom.append_child(tag, nested);
    let after = doc.dom.create_text_node("AAA", ());
    doc.dom.append_child(text, after);
    let pixels = readback(
        "a_nested_scope_s_background_paints_behind_its_own_fragments_only",
        &mut doc,
        300,
        60,
    );

    for x in [62, 90, 118] {
        assert!(
            is_blue(pixel(&pixels, 300, x, 20)),
            "the nested scope's background covers its own run (x={x})",
        );
    }
    for x in [10, 50, 130, 170] {
        assert!(
            is_black(pixel(&pixels, 300, x, 20)),
            "and never reaches the runs around it, which keep their own ink \
             over the block's background (x={x})",
        );
    }

    for y in [2, 8, 32, 38] {
        assert!(
            is_white(pixel(&pixels, 300, 90, y)),
            "and the half-leading stays unpainted: the fragment is the content \
             area, not the line box (y={y})",
        );
    }
}

#[test]
fn a_nested_scope_that_wraps_paints_one_fragment_per_line() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { width: 100px; word-break: break-all;
                 color: #000000; font-family: Ahem; font-size: 20px;
                 line-height: 20px; }
         .tag { color: transparent;
                background-color: #0000ff; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "text.text");
    let before = doc.dom.create_text_node("AAA", ());
    doc.dom.append_child(text, before);
    let tag = doc.el(text, "text.tag");
    let nested = doc.dom.create_text_node("BBBBBB", ());
    doc.dom.append_child(tag, nested);
    let pixels = readback(
        "a_nested_scope_that_wraps_paints_one_fragment_per_line",
        &mut doc,
        200,
        100,
    );

    assert!(
        is_blue(pixel(&pixels, 200, 62, 10)) && is_blue(pixel(&pixels, 200, 98, 10)),
        "the first fragment covers the part of the scope on the first line",
    );
    assert!(
        is_black(pixel(&pixels, 200, 50, 10)),
        "and stops where the scope starts",
    );
    assert!(
        is_blue(pixel(&pixels, 200, 2, 30)) && is_blue(pixel(&pixels, 200, 78, 30)),
        "the second fragment starts at the line's own origin",
    );
    assert!(
        is_white(pixel(&pixels, 200, 82, 30)) && is_white(pixel(&pixels, 200, 120, 10)),
        "and neither fragment runs to the block's width",
    );
    assert!(
        (40..100).all(|y| (0..200).all(|x| !is_blue(pixel(&pixels, 200, x, y)))),
        "and no third fragment exists: the scope ends on the second line",
    );
}

#[test]
fn an_inner_scope_s_background_covers_its_ancestor_s() {
    let mut doc = Doc::with_device(device(300.0, 40.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { width: 300px; color: #000000;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .outer { color: transparent;
                  background-color: #0000ff; }
         .inner { background-color: #ff0000; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "text.text");
    let before = doc.dom.create_text_node("AA", ());
    doc.dom.append_child(text, before);
    let outer = doc.el(text, "text.outer");
    let outer_run = doc.dom.create_text_node("BB", ());
    doc.dom.append_child(outer, outer_run);
    let inner = doc.el(outer, "text.inner");
    let inner_run = doc.dom.create_text_node("CC", ());
    doc.dom.append_child(inner, inner_run);
    let pixels = readback(
        "an_inner_scope_s_background_covers_its_ancestor_s",
        &mut doc,
        300,
        40,
    );

    assert!(
        is_blue(pixel(&pixels, 300, 42, 10)) && is_blue(pixel(&pixels, 300, 78, 10)),
        "the outer scope paints behind the run that is only its own",
    );
    assert!(
        is_red(pixel(&pixels, 300, 82, 10)) && is_red(pixel(&pixels, 300, 118, 10)),
        "and the inner scope's background covers it where the two overlap",
    );
    assert!(
        is_black(pixel(&pixels, 300, 10, 10)),
        "and neither reaches the run outside them both",
    );
}

#[test]
fn a_wrapper_between_scopes_paints_no_background() {
    let mut doc = Doc::with_device(device(300.0, 40.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { width: 300px; color: transparent;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .wrapper { display: contents; background-color: #0000ff; }
         .tag { background-color: #ff0000; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "text.text");
    let before = doc.dom.create_text_node("AA", ());
    doc.dom.append_child(text, before);
    let wrapper = doc.el(text, "view.wrapper");
    let first = doc.el(wrapper, "text.tag");
    let first_run = doc.dom.create_text_node("BB", ());
    doc.dom.append_child(first, first_run);
    let between = doc.dom.create_text_node("XX", ());
    doc.dom.append_child(wrapper, between);
    let second = doc.el(wrapper, "text.tag");
    let second_run = doc.dom.create_text_node("CC", ());
    doc.dom.append_child(second, second_run);
    let pixels = readback(
        "a_wrapper_between_scopes_paints_no_background",
        &mut doc,
        300,
        40,
    );

    assert!(
        is_red(pixel(&pixels, 300, 42, 10)) && is_red(pixel(&pixels, 300, 158, 10)),
        "both scopes under the wrapper paint their own background",
    );
    assert!(
        is_white(pixel(&pixels, 300, 100, 10)),
        "the run between them carries no background of its own",
    );
    assert!(
        (0..40).all(|y| (0..300).all(|x| !is_blue(pixel(&pixels, 300, x, y)))),
        "and the `display: contents` wrapper around all three paints nothing",
    );
}

#[test]
fn an_atom_inside_a_nested_scope_is_covered_by_its_background() {
    let mut doc = Doc::with_device(device(300.0, 40.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { width: 300px; color: transparent;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .tag { background-color: #0000ff; }
         .icon { display: flex; width: 40px; height: 10px; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "text.text");
    let before = doc.dom.create_text_node("AA", ());
    doc.dom.append_child(text, before);
    let tag = doc.el(text, "text.tag");
    let nested = doc.dom.create_text_node("B", ());
    doc.dom.append_child(tag, nested);
    doc.el(tag, "view.icon");
    let pixels = readback(
        "an_atom_inside_a_nested_scope_is_covered_by_its_background",
        &mut doc,
        300,
        40,
    );

    assert!(
        is_blue(pixel(&pixels, 300, 42, 10)),
        "the scope's background covers its own glyph",
    );
    assert!(
        is_blue(pixel(&pixels, 300, 70, 10)) && is_blue(pixel(&pixels, 300, 98, 10)),
        "and reaches across the atom beside it",
    );
    assert!(
        is_white(pixel(&pixels, 300, 10, 10)) && is_white(pixel(&pixels, 300, 110, 10)),
        "and stops at the scope's two ends",
    );
}

#[test]
fn a_gradient_background_image_on_a_nested_scope_fills_its_fragment() {
    let mut doc = Doc::with_device(device(300.0, 40.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { width: 300px; color: #000000;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .tag { color: transparent;
                background-image: linear-gradient(90deg, #ff0000, #0000ff); }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "text.text");
    let before = doc.dom.create_text_node("AAA", ());
    doc.dom.append_child(text, before);
    let tag = doc.el(text, "text.tag");
    let nested = doc.dom.create_text_node("BBB", ());
    doc.dom.append_child(tag, nested);
    let pixels = readback(
        "a_gradient_background_image_on_a_nested_scope_fills_its_fragment",
        &mut doc,
        300,
        40,
    );

    let left = pixel(&pixels, 300, 62, 10);
    let right = pixel(&pixels, 300, 118, 10);
    assert!(
        left[0] > 200 && left[2] < 60,
        "the ramp starts at its first stop inside the fragment ({left:?})",
    );
    assert!(
        right[2] > 200 && right[0] < 60,
        "and reaches its last one by the fragment's far edge ({right:?})",
    );
    assert!(
        is_white(pixel(&pixels, 300, 130, 10)),
        "and paints nothing past the fragment",
    );
}

#[test]
fn a_percentage_width_against_an_indefinite_basis_behaves_as_auto() {
    let mut doc = Doc::with_device(device(800.0, 600.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; align-items: flex-start;
                font-family: Ahem; font-size: 10px; line-height: 10px; }
         .middle { display: flex; flex-direction: column; flex: none;
                   height: 150px; width: fit-content; }
         .inner { height: 100px; width: 50%;
                  overflow: hidden; overflow-wrap: normal; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let middle = doc.el(root, "view.middle");
    let inner = doc.el(middle, "text.inner");
    let run = doc.dom.create_text_node("12345678901234567890", ());
    doc.dom.append_child(inner, run);
    doc.flush();

    assert_eq!(
        rect(&doc, middle),
        (0.0, 0.0, 200.0, 150.0),
        "the fit-content basis is the text's max-content width, with the \
         cyclic 50% treated as auto",
    );
    assert_eq!(
        rect(&doc, inner),
        (0.0, 0.0, 100.0, 100.0),
        "once the basis is definite the percentage resolves against it",
    );
}

#[test]
fn a_text_block_is_a_block_axis_scroll_target_like_a_view() {
    let mut doc = Doc::with_device(device(400.0, 600.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .port { display: flex; flex-direction: column; flex: none;
                 width: 100px; height: 400px; overflow-y: scroll; }
         .cell { display: flex; flex: none; width: 100%; height: 50%; }
         .textcell { flex: none; width: 100%; height: 50%;
                     font-family: Ahem; font-size: 20px; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let port = doc.el(root, "view.port");
    let cells: Vec<NodeId> = (0..8)
        .map(|index| {
            if index == 5 {
                let text = doc.el(port, "text.textcell");
                let scope = doc.el(text, "text.scope");
                let run = doc.dom.create_text_node("hello lynx", ());
                doc.dom.append_child(scope, run);
                text
            } else {
                doc.el(port, "view.cell")
            }
        })
        .collect();
    doc.flush();

    let text = cells[5];
    assert_eq!(
        rect(&doc, text),
        (0.0, 1000.0, 100.0, 200.0),
        "the text block occupies the sixth 200px slot, exactly as a view does",
    );
    assert_eq!(
        rect(&doc, cells[4]).3,
        rect(&doc, text).3,
        "and it is the same height as the view above it",
    );

    let scroll_box = doc
        .dom
        .scroll_box(port)
        .expect("the port is a scroll container");
    assert_eq!(scroll_box.scroll_size.height, 1600.0);
    assert_eq!(scroll_box.max_offset().y, 1200.0);

    let (_, top, _, height) = rect(&doc, text);
    for (alignment, offset, landed) in [
        ("start", top, 1000.0),
        ("center", top + height / 2.0 - 400.0 / 2.0, 900.0),
        ("end", top + height - 400.0, 800.0),
    ] {
        doc.dom.scroll_to(port, Vector2D::new(0.0, offset));
        assert_eq!(
            doc.dom.scroll_offset(port).y,
            landed,
            "block:{alignment} lands the scrollport at {landed}",
        );
    }
}

#[test]
fn a_text_block_is_an_inline_axis_scroll_target_like_a_view() {
    let mut doc = Doc::with_device(device(600.0, 400.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .port { display: flex; flex-direction: row; flex: none;
                 width: 400px; height: 100px; overflow-x: scroll; }
         .cell { display: flex; flex: none; width: 50%; height: 100%; }
         .textcell { flex: none; width: 50%; height: 100%;
                     font-family: Ahem; font-size: 20px; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let port = doc.el(root, "view.port");
    let cells: Vec<NodeId> = (0..8)
        .map(|index| {
            if index == 5 {
                let text = doc.el(port, "text.textcell");
                let scope = doc.el(text, "text.scope");
                let run = doc.dom.create_text_node("hello lynx", ());
                doc.dom.append_child(scope, run);
                text
            } else {
                doc.el(port, "view.cell")
            }
        })
        .collect();
    doc.flush();

    let text = cells[5];
    assert_eq!(
        rect(&doc, text),
        (1000.0, 0.0, 200.0, 100.0),
        "the text block occupies the sixth 200px slot on the inline axis",
    );

    let scroll_box = doc
        .dom
        .scroll_box(port)
        .expect("the port is a scroll container");
    assert_eq!(scroll_box.scroll_size.width, 1600.0);
    assert_eq!(scroll_box.max_offset().x, 1200.0);

    let (left, _, width, _) = rect(&doc, text);
    for (alignment, offset, landed) in [
        ("start", left, 1000.0),
        ("center", left + width / 2.0 - 400.0 / 2.0, 900.0),
        ("end", left + width - 400.0, 800.0),
    ] {
        doc.dom.scroll_to(port, Vector2D::new(offset, 0.0));
        assert_eq!(
            doc.dom.scroll_offset(port).x,
            landed,
            "inline:{alignment} lands the scrollport at {landed}",
        );
    }
}

#[test]
fn gradient_valued_color_paints_glyph_ink_with_the_ramp() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .text { position: absolute; left: 0px; top: 0px;
                 width: 200px; height: 80px;
                 font-family: Ahem; font-size: 80px; line-height: 80px;
                 color: linear-gradient(green, yellow); }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "text.text");
    let run = doc.dom.create_text_node("HH", ());
    doc.dom.append_child(text, run);

    let pixels = readback(
        "gradient_valued_color_paints_glyph_ink_with_the_ramp",
        &mut doc,
        200,
        100,
    );

    let top = pixel(&pixels, 200, 40, 4);
    let bottom = pixel(&pixels, 200, 40, 76);
    assert!(
        top[0] < 80 && top[1] > 100,
        "the top of the em square sits at the green end ({top:?})"
    );
    assert!(
        bottom[0] > 200 && bottom[1] > 200 && bottom[2] < 80,
        "the bottom sits at the yellow end ({bottom:?})"
    );
    assert!(
        bottom[0] > top[0] + 100,
        "red must climb down the ramp ({top:?} then {bottom:?})"
    );
    assert!(
        is_white(pixel(&pixels, 200, 40, 90)),
        "nothing paints below the block",
    );
    let off_ink = pixel(&pixels, 200, 180, 40);
    assert!(
        is_white(off_ink),
        "the ramp fills the glyphs, not the block's box ({off_ink:?})",
    );
}

#[test]
fn a_gradient_color_fills_a_block_and_the_run_that_inherits_it() {
    let mut doc = Doc::with_device(device(240.0, 180.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; position: relative; width: 240px; height: 180px; }
         .text { position: absolute; left: 0px;
                 width: 240px; height: 40px;
                 font-family: Ahem; font-size: 40px; line-height: 40px; }
         .grad { color: linear-gradient(green, yellow); }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;

    let block = doc.el(root, "text.text.grad");
    doc.set_inline(block, "top: 0px");
    let run = doc.dom.create_text_node("AA", ());
    doc.dom.append_child(block, run);

    let inheriting = doc.el(root, "text.text.grad");
    doc.set_inline(inheriting, "top: 120px");
    let plain_scope = doc.el(inheriting, "text.scope");
    let plain_run = doc.dom.create_text_node("AA", ());
    doc.dom.append_child(plain_scope, plain_run);

    let pixels = readback(
        "a_gradient_color_fills_a_block_and_the_run_that_inherits_it",
        &mut doc,
        240,
        180,
    );

    let greenish = |color: [u8; 4]| color[0] < 80 && color[1] > 100;
    let yellowish = |color: [u8; 4]| color[0] > 200 && color[1] > 200 && color[2] < 80;

    let first = pixel(&pixels, 240, 20, 4);
    assert!(greenish(first), "block gradient starts green ({first:?})");
    let first_bottom = pixel(&pixels, 240, 20, 36);
    assert!(
        yellowish(first_bottom),
        "block gradient ends yellow ({first_bottom:?})"
    );

    let inherited_top = pixel(&pixels, 240, 20, 124);
    let inherited_bottom = pixel(&pixels, 240, 20, 156);
    assert!(
        greenish(inherited_top) && yellowish(inherited_bottom),
        "an unstyled nested run inherits the block's gradient \
         ({inherited_top:?} then {inherited_bottom:?})"
    );

    for (row, y) in [("block", 20), ("inherited", 140)] {
        let off_ink = pixel(&pixels, 240, 200, y);
        assert!(
            is_white(off_ink),
            "the {row} ramp fills the glyphs, not the block's box ({off_ink:?})",
        );
    }
}

#[test]
fn a_gradient_color_on_a_nested_run_fills_only_that_run() {
    let mut doc = Doc::with_device(device(240.0, 180.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; position: relative; width: 240px; height: 180px; }
         .text { position: absolute; left: 0px;
                 width: 240px; height: 40px;
                 font-family: Ahem; font-size: 40px; line-height: 40px; }
         .grad { color: linear-gradient(green, yellow); }
         .solid { color: #000000; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;

    let mixed = doc.el(root, "text.text.solid");
    doc.set_inline(mixed, "top: 60px");
    let solid_run = doc.dom.create_text_node("A", ());
    doc.dom.append_child(mixed, solid_run);
    let gradient_scope = doc.el(mixed, "text.scope.grad");
    let gradient_run = doc.dom.create_text_node("B", ());
    doc.dom.append_child(gradient_scope, gradient_run);

    let pixels = readback(
        "a_gradient_color_on_a_nested_run_fills_only_that_run",
        &mut doc,
        240,
        180,
    );

    let greenish = |color: [u8; 4]| color[0] < 80 && color[1] > 100;
    let yellowish = |color: [u8; 4]| color[0] > 200 && color[1] > 200 && color[2] < 80;

    let solid_glyph = pixel(&pixels, 240, 20, 64);
    assert!(
        solid_glyph[0] < 60 && solid_glyph[1] < 60 && solid_glyph[2] < 60,
        "the block's own run keeps its solid black ({solid_glyph:?})"
    );
    let nested_top = pixel(&pixels, 240, 60, 64);
    let nested_bottom = pixel(&pixels, 240, 60, 96);
    assert!(
        greenish(nested_top) && yellowish(nested_bottom),
        "the nested run paints its own ramp ({nested_top:?} then {nested_bottom:?})"
    );

    let off_ink = pixel(&pixels, 240, 200, 80);
    assert!(
        is_white(off_ink),
        "the ramp fills the glyphs, not the block's box ({off_ink:?})",
    );
}

#[test]
fn an_extra_registered_family_is_selected_over_the_default_one() {
    let mut doc = Doc::with_device(device(800.0, 600.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; flex-direction: column; align-items: flex-start; }
         .label { font-size: 16px; }
         .extra { font-family: Ahem; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(ROBOTO)), 1);
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    assert!(doc.dom.set_default_font_family("Roboto"));
    let root = doc.root;
    let default_family = doc.el(root, "text.label");
    let default_run = doc.dom.create_text_node("EXTRA", ());
    doc.dom.append_child(default_family, default_run);
    let extra_family = doc.el(root, "text.label.extra");
    let extra_run = doc.dom.create_text_node("EXTRA", ());
    doc.dom.append_child(extra_family, extra_run);
    doc.flush();

    assert_eq!(
        ink(&doc, extra_family),
        (80.0, 16.0),
        "the extra family shapes the run that names it",
    );
    let (default_width, default_line) = ink(&doc, default_family);
    assert!(
        (default_width - 48.960_938).abs() < 0.01,
        "the default family shapes with Roboto's own advances ({default_width})",
    );
    assert!(
        (default_line - 21.101_563).abs() < 0.01,
        "and with Roboto's own line metrics, not Ahem's 16px em box \
         ({default_line})",
    );
}

#[test]
fn a_font_face_declared_family_shapes_the_text_that_names_it() {
    const AHEM_URL: &str = concat!(
        "file://",
        env!("CARGO_MANIFEST_DIR"),
        "/../seldon/tests/fixtures/Ahem.ttf"
    );

    let mut doc = Doc::with_device(device(800.0, 600.0));
    doc.define_lynx_text("text");
    doc.add_css(&format!(
        "@font-face {{ font-family: DeclaredAhem;
                       src: url(\"{AHEM_URL}\") format(\"truetype\"); }}
         page {{ display: flex; flex-direction: column; align-items: flex-start; }}
         .label {{ font-size: 16px;
                   font-family: DeclaredAhem; }}"
    ));
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(ROBOTO)), 1);
    assert!(doc.dom.set_default_font_family("Roboto"));

    let resolved = AHEM_URL.replace("/terminus/../seldon/", "/seldon/");
    let requests = doc.dom.take_font_face_requests();
    assert_eq!(
        requests,
        vec![FontFaceRequest {
            family: "DeclaredAhem".to_owned(),
            sources: vec![FontFaceSource::Url(resolved.clone())],
        }],
        "the declared family and its one source, with the format hint dropped",
    );
    assert!(
        doc.dom.take_font_face_requests().is_empty(),
        "a rule is reported once, not again on the next drain",
    );

    let bytes = std::fs::read(
        resolved
            .strip_prefix("file://")
            .expect("the fixture URL is a file URL"),
    )
    .expect("the vendored Ahem fixture is on disk");
    assert_eq!(doc.dom.register_font_face("DeclaredAhem", bytes.into()), 1);

    let root = doc.root;
    let label = doc.el(root, "text.label");
    let run = doc.dom.create_text_node("EXTRA", ());
    doc.dom.append_child(label, run);
    doc.flush();

    assert_eq!(
        ink(&doc, label),
        (80.0, 16.0),
        "the declared face shapes the run that names its family, rather than \
         the run falling back to another face",
    );

    doc.add_css(
        "@font-face { font-family: LateFace; src: local(\"Helvetica\"); }
         @font-face { font-family: Nameless; src: url(\"about:blank\"); }",
    );
    assert_eq!(
        doc.dom.take_font_face_requests(),
        vec![
            FontFaceRequest {
                family: "LateFace".to_owned(),
                sources: vec![FontFaceSource::Local("Helvetica".to_owned())],
            },
            FontFaceRequest {
                family: "Nameless".to_owned(),
                sources: vec![FontFaceSource::Url("about:blank".to_owned())],
            },
        ],
        "only the rules the new sheet added",
    );
}

#[test]
fn a_boxed_child_paints_from_a_text_block_inside_its_parents_context() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .text { position: relative;
                 width: 200px; height: 40px;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .child { position: absolute; left: 0px; top: 0px;
                  width: 40px; height: 20px; background-color: #ff0000; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "text.text");
    doc.el(text, "view.child");
    let pixels = readback(
        "a_boxed_child_paints_from_a_text_block_inside_its_parents_context",
        &mut doc,
        200,
        100,
    );
    let ink = pixel(&pixels, 200, 20, 10);
    assert!(
        is_red(ink),
        "a text block's own child paints from the in-context path ({ink:?})"
    );
    let beside = pixel(&pixels, 200, 100, 10);
    assert!(
        is_white(beside),
        "and only over its own 40x20 box ({beside:?})",
    );
}

#[test]
fn a_boxed_child_paints_from_a_text_block_that_is_its_own_context() {
    let mut as_root = Doc::with_device(device(200.0, 100.0));
    as_root.define_lynx_text("page");
    as_root.add_css(
        "page { position: relative;
                width: 200px; height: 100px;
                font-family: Ahem; font-size: 20px; line-height: 20px; }
         .child { position: absolute; left: 0px; top: 0px;
                  width: 40px; height: 20px; background-color: #ff0000; }",
    );
    assert_eq!(as_root.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = as_root.root;
    as_root.el(root, "view.child");
    let pixels = readback(
        "a_boxed_child_paints_from_a_text_block_that_is_its_own_context.root",
        &mut as_root,
        200,
        100,
    );
    let ink = pixel(&pixels, 200, 20, 10);
    assert!(
        is_red(ink),
        "a child of the paint root's own paragraph must paint ({ink:?})"
    );

    let mut as_context = Doc::with_device(device(200.0, 100.0));
    as_context.define_lynx_text("text");
    as_context.add_css(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .text { position: relative; z-index: 1;
                 width: 200px; height: 40px;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .child { position: absolute; left: 0px; top: 0px;
                  width: 40px; height: 20px; background-color: #ff0000; }",
    );
    assert_eq!(
        as_context.dom.register_fonts(FontBlob::from_static(AHEM)),
        1
    );
    let root = as_context.root;
    let text = as_context.el(root, "text.text");
    as_context.el(text, "view.child");
    let pixels = readback(
        "a_boxed_child_paints_from_a_text_block_that_is_its_own_context.context",
        &mut as_context,
        200,
        100,
    );
    let ink = pixel(&pixels, 200, 20, 10);
    assert!(
        is_red(ink),
        "a child of a stacking-context paragraph must paint ({ink:?})"
    );
}

#[test]
fn an_atomic_inline_box_keeps_the_size_it_measured() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .text { position: relative;
                 width: 200px; height: 40px;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .atom { display: flex; width: 40px; height: 20px;
                 background-color: #ff0000; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "text.text");
    let atom = doc.el(text, "view.atom");
    doc.flush();

    let (_, _, width, height) = rect(&doc, atom);
    assert_eq!(
        (width, height),
        (40.0, 20.0),
        "the atom keeps the 40x20 box the line was broken against",
    );
}

#[test]
fn an_atomic_inline_box_sits_inside_the_blocks_border_and_padding() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .text { position: absolute; left: 20px; top: 10px;
                 width: 160px; height: 80px; box-sizing: border-box;
                 padding: 10px;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .bordered { border: 4px solid #0000ff; padding-left: 6px; padding-top: 2px; }
         .carrier { display: contents; }
         .atom { display: flex; width: 40px; height: 20px;
                 background-color: #ff0000; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "text.text");
    let atom = doc.el(text, "view.atom");
    let bordered = doc.el(root, "text.text.bordered");
    let inside_border = doc.el(bordered, "view.atom");
    let wrapping = doc.el(root, "text.text");
    let carrier = doc.el(wrapping, "view.carrier");
    let under_carrier = doc.el(carrier, "view.atom");
    doc.flush();

    assert_eq!(
        rect(&doc, text),
        (20.0, 10.0, 160.0, 80.0),
        "the block itself is where the fixture puts it",
    );
    let (x, y, _, _) = rect(&doc, atom);
    assert_eq!(
        (x, y),
        (10.0, 10.0),
        "the atom is the first thing on the first line, so it starts at the \
         content-box origin — the same origin the glyphs beside it get",
    );
    let (x, y, _, _) = rect(&doc, inside_border);
    assert_eq!(
        (x, y),
        (10.0, 6.0),
        "and the content-box origin is border plus padding, not padding alone: \
         4 + 6 across and 4 + 2 down",
    );
    assert_eq!(
        rect(&doc, carrier),
        (0.0, 0.0, 0.0, 0.0),
        "a carrier keeps an empty layout rather than a hidden one, so the walk \
         below it still descends",
    );
    let (x, y, _, _) = rect(&doc, under_carrier);
    assert_eq!(
        (x, y),
        (10.0, 10.0),
        "and the atom under it lands exactly where an unwrapped one does: the \
         carrier contributes no origin of its own",
    );
}

#[test]
fn an_atoms_margins_reach_the_line_and_step_its_border_box_in() {
    let mut doc = Doc::with_device(device(400.0, 400.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; flex-direction: column; width: 400px; height: 400px; }
         .text { width: 400px;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .atom { display: flex; width: 40px; height: 60px; }
         .left { margin-left: 10px; }
         .back { margin-left: -10px; }
         .up { margin-top: 8px; }
         .down { margin-bottom: 8px; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let mut paragraph = |classes: &str| {
        let text = doc.el(root, "text.text");
        let run = doc.dom.create_text_node("AB", ());
        doc.dom.append_child(text, run);
        let atom = doc.el(text, &format!("view.atom{classes}"));
        (text, atom)
    };
    let (plain, plain_atom) = paragraph("");
    let (left, left_atom) = paragraph(".left");
    let (back, back_atom) = paragraph(".back");
    let (up, up_atom) = paragraph(".up");
    let (down, down_atom) = paragraph(".down");
    doc.flush();

    assert_eq!(
        ink(&doc, plain),
        (80.0, 64.0),
        "the control: two em squares, then a 40x60 box that is the whole of the \
         line's height above the baseline, over the run's 4px descent",
    );
    assert_eq!(rect(&doc, plain_atom), (40.0, 0.0, 40.0, 60.0));

    assert_eq!(
        ink(&doc, left),
        (90.0, 64.0),
        "`margin-left` is advance the line grows by ahead of the box",
    );
    assert_eq!(
        rect(&doc, left_atom),
        (50.0, 0.0, 40.0, 60.0),
        "and the border box starts one margin past the margin box's origin, \
         keeping the size its own layout produced and its place on the baseline",
    );

    assert_eq!(
        ink(&doc, back),
        (70.0, 64.0),
        "a negative margin shrinks the advance rather than being clamped away",
    );
    assert_eq!(
        rect(&doc, back_atom),
        (30.0, 0.0, 40.0, 60.0),
        "and pulls the border box back over the run before it",
    );

    assert_eq!(
        ink(&doc, up),
        (80.0, 72.0),
        "`margin-top` is above-baseline contribution: the line grows by it",
    );
    assert_eq!(
        rect(&doc, up_atom),
        (40.0, 8.0, 40.0, 60.0),
        "and the border box moves down inside the margin box by that margin",
    );

    assert_eq!(
        ink(&doc, down),
        (80.0, 72.0),
        "`margin-bottom` is above the baseline too, because it is the bottom \
         margin edge that sits on it, so the line grows by the same 8",
    );
    assert_eq!(
        rect(&doc, down_atom),
        (40.0, 0.0, 40.0, 60.0),
        "while the border box, which the margin hangs below, does not move",
    );
}

#[test]
fn an_out_of_flow_child_of_a_text_resolves_against_the_blocks_padding_box() {
    let mut doc = Doc::with_device(device(200.0, 200.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; position: relative; width: 200px; height: 200px; }
         .text { position: relative;
                 width: 160px; height: 100px; box-sizing: border-box;
                 border: 5px solid #0000ff; padding: 10px;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .out { position: absolute; left: 0; top: 0; width: 50%; height: 50%; }
         .static-position { position: absolute; width: 20px; height: 20px; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "text.text");
    let out = doc.el(text, "view.out");
    let anchored = doc.el(text, "view.static-position");
    doc.flush();

    assert_eq!(
        rect(&doc, out),
        (5.0, 5.0, 75.0, 45.0),
        "the containing block is the 150x90 padding box, and `left: 0` counts \
         from the padding edge — so the box lands one border in",
    );
    assert_eq!(
        rect(&doc, anchored),
        (15.0, 15.0, 20.0, 20.0),
        "and a box with no insets falls back to the static position, which is \
         the content-box origin: border plus padding, counted once",
    );
}

#[test]
fn a_relative_atom_stays_in_the_line_and_only_absolute_leaves_it() {
    let mut doc = Doc::with_device(device(400.0, 400.0));
    doc.define_lynx_text("text");
    doc.add_css(
        "page { display: flex; flex-direction: column; width: 400px; height: 400px; }
         .text { position: relative; width: 400px;
                 font-family: Ahem; font-size: 20px; line-height: 30px; }
         .atom { display: flex; width: 40px; height: 20px; }
         .relative { position: relative; }
         .inset { position: relative; left: 7px; top: 5px; }
         .sticky { position: sticky; }
         .absolute { position: absolute; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let mut paragraph = |classes: &str| {
        let text = doc.el(root, "text.text");
        let run = doc.dom.create_text_node("AB", ());
        doc.dom.append_child(text, run);
        let first = doc.el(text, &format!("view.atom{classes}"));
        let second = doc.el(text, "view.atom");
        (text, first, second)
    };
    let (plain, plain_first, plain_second) = paragraph("");
    let (relative, relative_first, relative_second) = paragraph(".relative");
    let (inset, inset_first, inset_second) = paragraph(".inset");
    let (sticky, sticky_first, sticky_second) = paragraph(".sticky");
    let (absolute, absolute_first, absolute_second) = paragraph(".absolute");
    doc.flush();

    let control = ink(&doc, plain);
    assert_eq!(control, (120.0, 30.0), "two em squares and two 40px boxes");
    assert_eq!(rect(&doc, plain_first), (40.0, 1.0, 40.0, 20.0));
    assert_eq!(rect(&doc, plain_second), (80.0, 1.0, 40.0, 20.0));

    for (label, text, first, second) in [
        ("relative", relative, relative_first, relative_second),
        ("sticky", sticky, sticky_first, sticky_second),
    ] {
        assert_eq!(ink(&doc, text), control, "{label} is still in the line");
        assert_eq!(rect(&doc, first), rect(&doc, plain_first), "{label} atom");
        assert_eq!(
            rect(&doc, second),
            rect(&doc, plain_second),
            "{label}: the atom after it is where the static control puts it",
        );
    }

    assert_eq!(
        ink(&doc, inset),
        control,
        "an inset changes no break and no advance, so the paragraph is the \
         same one",
    );
    assert_eq!(
        rect(&doc, inset_first),
        rect(&doc, plain_first),
        "and it does not move the box either: `left: 7px; top: 5px` on an \
         in-flow atom is ignored, so it sits exactly where the static control \
         puts it",
    );
    assert_eq!(
        rect(&doc, inset_second),
        rect(&doc, plain_second),
        "while the atom after it does not move at all",
    );

    assert_eq!(
        ink(&doc, absolute),
        (80.0, 30.0),
        "an absolutely positioned child never entered the paragraph: the line \
         advances by the run and the one remaining atom alone",
    );
    assert_eq!(
        rect(&doc, absolute_first),
        (0.0, 0.0, 40.0, 20.0),
        "it is laid out by the block's own out-of-flow pass instead, at the \
         static position its containing block's content origin gives it",
    );
    assert_eq!(
        rect(&doc, absolute_second),
        (40.0, 1.0, 40.0, 20.0),
        "and the in-flow atom beside it closes the gap the escape left",
    );
}
