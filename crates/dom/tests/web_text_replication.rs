//! Replicas of `lynx-stack`'s web-platform text tests that land on `dom`:
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
//! screenshot tests over Lynx elements; here there is no Lynx element
//! vocabulary at all (`crates/dom` must not contain any), so a `<text>` is
//! spelled as the `display: -lynx-text` box the Lynx UA sheet gives it and a
//! `text-maxline="n"` attribute as the `--lynx-text-maxline` custom property
//! that same sheet reflects it into. And every metric assertion shapes the
//! vendored Ahem face, whose glyphs are solid em squares: an advance is
//! exactly glyph-count x font-size and a glyph's ink is pixel-assertable,
//! which a full-page golden could never be.

#![allow(clippy::float_cmp, reason = "Ahem geometry is exact at these sizes")]

mod common;

use common::{Doc, device};
use dom::vello::peniko::Color;
use dom::{FontBlob, NodeId, Vector2D};
use flashbulb::headless;

const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");
const ROBOTO: &[u8] = include_bytes!("../../hughie/tests/fixtures/Roboto-Regular.ttf");

/// The two registered integer properties the Lynx UA sheet declares for the
/// truncation attributes (`crates/bobcat-core/src/main/tree/text.rs:91-92`).
/// Without the `@property` registration a custom property is an untyped token
/// stream and the layout never sees a limit at all.
const LIMIT_PROPERTIES: &str = r#"
@property --lynx-text-maxline { syntax: "<integer>"; inherits: false; initial-value: 0; }
@property --lynx-text-maxlength { syntax: "<integer>"; inherits: false; initial-value: -1; }
"#;

/// The paragraph ink `holder` establishes, as `(width, height)`.
fn ink(doc: &Doc, holder: NodeId) -> (f32, f32) {
    let size = doc
        .dom
        .text_block_size(holder)
        .expect("the holder establishes a paragraph");
    (size.width, size.height)
}

/// A node's border box as `(x, y, width, height)`.
fn rect(doc: &Doc, id: NodeId) -> (f32, f32, f32, f32) {
    let layout = doc.dom.rounded_layout(id).expect("node id is live");
    (
        layout.location.x,
        layout.location.y,
        layout.size.width,
        layout.size.height,
    )
}

/// One RGBA pixel out of a readback buffer.
fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * width + x) * 4) as usize;
    pixels[index..index + 4].try_into().unwrap()
}

/// Lays the document out, renders it, and reads the framebuffer back.
fn readback(test: &str, doc: &mut Doc, width: u32, height: u32) -> Vec<u8> {
    let mut gpu = headless(test);
    doc.dom.layout();
    doc.dom.render();
    let scene = doc.dom.scene(&dom::NoImages);
    gpu.render(&scene, width, height, Color::WHITE)
        .expect("headless render")
}

fn is_red(color: [u8; 4]) -> bool {
    color[0] > 200 && color[1] < 60 && color[2] < 60
}

fn is_white(color: [u8; 4]) -> bool {
    color[0] > 240 && color[1] > 240 && color[2] > 240
}

/// Replicates `x-text/text-not-resize-detect-new-line`
/// (`packages/web-platform/web-elements/tests/fixtures/x-text/text-not-resize-detect-new-line.
/// html`, `packages/web-platform/web-elements/tests/web-elements.spec.ts:378`): moving
/// a clamped paragraph must not re-run line detection, while a change to the
/// line limit alone must.
///
/// The web implementation observes its inner box with a `ResizeObserver` and
/// skips exactly one first callback, so that `left: 200px` — which changes the
/// element's origin and nothing about its content box — cannot be mistaken for
/// a resize that re-drives truncation. Here relayout is keyed on the inline
/// available size and the content instead of on an observer, so the equivalent
/// claim is that the paragraph the move produces is identical to the one
/// before it, and only the `text-maxline` 3 -> 2 edit in the same task changes
/// the line set.
///
/// This is the frame-level half of the replica, and on its own it cannot fail
/// on the property it is named for: re-breaking is idempotent here, so the ink
/// below is the same number whether or not the move re-shaped and re-broke the
/// paragraph. What it does pin is the fixture's own final frame — a two-line
/// clamp at an origin 200px to the right — and the third stage, where a limit
/// edit alone must change the line set.
///
/// The claim itself is asserted where the counters live:
/// `moving_a_clamped_paragraph_does_not_reshape_or_rebreak_it` in
/// `crates/dom/src/layout/mod.rs`'s `mod tests` asserts `text_block_rebuilds`
/// and `break_count` are unchanged across the same move.
/// `Document::text_block`, `text_block_rebuilds` and `text_block_is_probe_dirty`
/// are all crate-private (`crates/dom/src/layout/mod.rs:237`, `:245`, `:253`),
/// so no integration test can see them.
///
/// The line count is therefore read here as ink height over an explicit
/// `line-height`.
#[test]
fn moving_a_clamped_paragraph_does_not_rebreak_its_lines() {
    let mut doc = Doc::with_device(device(800.0, 600.0));
    doc.add_ua_css(LIMIT_PROPERTIES);
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .wrap { display: flex; width: 100px; }
         .target { display: -lynx-text; width: 100%; flex-shrink: 0;
                   font-family: Ahem; font-size: 20px; line-height: 20px;
                   --lynx-text-maxline: 3; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let wrap = doc.el(root, "view.wrap");
    let target = doc.el(wrap, "view.target");
    // Six five-character words: each one fills the 100px line exactly, so the
    // natural paragraph is six lines and every clamp below is unambiguous.
    let run = doc
        .dom
        .create_text_node("aaaaa bbbbb ccccc ddddd eeeee fffff", ());
    doc.dom.append_child(target, run);
    doc.flush();

    assert_eq!(ink(&doc, target), (100.0, 60.0), "clamped to three lines");
    assert_eq!(rect(&doc, target), (0.0, 0.0, 100.0, 60.0));

    // The move. Same content box, same available inline size, new origin.
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

    // The limit edit that rides the same task in the fixture.
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

/// Replicates `x-text/text-maxline-with-custom-truncation-innertext-change`
/// (`packages/web-platform/web-elements/tests/fixtures/x-text/
/// text-maxline-with-custom-truncation-innertext-change.html`, `packages/web-platform/web-elements/
/// tests/web-elements.spec.ts:327`): rewriting a truncated text node's data re-clamps against the
/// new string, with no trace of the old one.
///
/// The web implementation cuts the node's data in place and saves the original
/// to restore later, so the hazard the fixture exists to catch is a stale
/// saved string surviving a host mutation. Here truncation is a layout-time
/// cut recomputed from the source items every pass, so the third stage below —
/// long, then short, then long again — is what proves the state is not sticky:
/// the short string must produce a genuinely unclamped one-line paragraph and
/// the second long string must clamp afresh.
///
/// Adaptation: the fixture's `<inline-truncation>` marker content is dropped.
/// A custom truncation element is not reachable from this crate's tree at all
/// (`crates/dom/src/layout/text_block.rs:276` flattens a paragraph's children
/// without one), so the replica pins the re-clamp and not the marker.
#[test]
fn rewriting_a_clamped_text_node_reclamps_against_the_new_string() {
    let mut doc = Doc::with_device(device(800.0, 600.0));
    doc.add_ua_css(LIMIT_PROPERTIES);
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .target { display: -lynx-text; width: 100px; flex-shrink: 0;
                   word-break: break-all;
                   font-family: Ahem; font-size: 20px; line-height: 20px;
                   --lynx-text-maxline: 2; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let target = doc.el(root, "view.target");
    let run = doc.dom.create_text_node(&"b".repeat(30), ());
    doc.dom.append_child(target, run);
    doc.flush();

    // Thirty characters break-all onto six 100px lines, clamped to two.
    assert_eq!(ink(&doc, target), (100.0, 40.0));

    doc.dom.set_text_node_data(run, "bb");
    doc.flush();
    assert_eq!(
        ink(&doc, target),
        (40.0, 20.0),
        "the short replacement is not clamped, and no cut of the old string \
         survives into it",
    );

    doc.dom.set_text_node_data(run, &"a".repeat(100));
    doc.flush();
    assert_eq!(
        ink(&doc, target),
        (100.0, 40.0),
        "the second long string clamps afresh rather than restoring the first",
    );
}

/// Replicates `layout/percentage-cyclic-text`
/// (`packages/web-platform/web-elements/tests/fixtures/layout/percentage-cyclic-text.html`,
/// `packages/web-platform/web-elements/tests/web-elements.spec.ts:46`): a
/// percentage width on a text block whose containing block is sized
/// `fit-content` from that same text.
///
/// This case has no reference answer to copy. Its source test is
/// `test.skip(true)` and no snapshot directory was ever committed for it, and
/// the fixture's own comment records that the web result diverges from the
/// Lynx SDK — which is why it is disabled. So `AGENTS.md`'s bucket 1 applies
/// and the W3C answer is the one to pin: css-sizing-3 5.2.1, a percentage
/// resolved against an indefinite basis behaves as `auto`. The outer box's
/// `fit-content` width is therefore the text's own max-content width — twenty
/// Ahem digits at 10px, so 200px — and not a cycle through the 50%.
#[test]
fn a_percentage_width_against_an_indefinite_basis_behaves_as_auto() {
    let mut doc = Doc::with_device(device(800.0, 600.0));
    doc.add_css(
        "page { display: flex; align-items: flex-start;
                font-family: Ahem; font-size: 10px; line-height: 10px; }
         .middle { display: flex; flex-direction: column; flex: none;
                   height: 150px; width: fit-content; }
         .inner { display: -lynx-text; height: 100px; width: 50%;
                  overflow: hidden; overflow-wrap: normal; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let middle = doc.el(root, "view.middle");
    let inner = doc.el(middle, "view.inner");
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

/// Replicates `scroll-view/scroll-into-view-text`
/// (`packages/web-platform/web-elements/tests/fixtures/scroll-view/scroll-into-view-text.html`,
/// `packages/web-platform/web-elements/tests/web-elements.spec.ts:785`): a
/// text block is a scroll target with the same border-box geometry as a view.
///
/// The original calls `scrollIntoView({ block: 'start' | 'center' | 'end' })`
/// on the sixth of eight equal children. There is no `scrollIntoView` in this
/// engine — the scroll module exposes only `scroll_to(id, offset)`
/// (`crates/dom/src/scroll/mod.rs:200`) — so what is replicated is the half
/// the case really pins about text: the block reports the same scrollable
/// geometry a view in its place would, and each of the three CSSOM-View
/// alignments computed from that geometry lands the scrollport somewhere
/// different. The alignment computation itself is unimplemented and is
/// reported as such.
///
/// The fixture's sizing is kept verbatim (`x-view`/`x-text { width: 100%;
/// height: 50% }` against a 100x400 port), so the 200px slot is measured out
/// of the percentage rather than declared: a text block that resolved
/// percentages differently from a view would move the sixth slot and fail
/// every offset below.
#[test]
fn a_text_block_is_a_block_axis_scroll_target_like_a_view() {
    let mut doc = Doc::with_device(device(400.0, 600.0));
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .port { display: flex; flex-direction: column; flex: none;
                 width: 100px; height: 400px; overflow-y: scroll; }
         .cell { display: flex; flex: none; width: 100%; height: 50%; }
         .textcell { display: -lynx-text; flex: none; width: 100%; height: 50%;
                     font-family: Ahem; font-size: 20px; }
         .scope { display: -lynx-text; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let port = doc.el(root, "view.port");
    let cells: Vec<NodeId> = (0..8)
        .map(|index| {
            if index == 5 {
                let text = doc.el(port, "view.textcell");
                // `<x-text><x-text>hello lynx</x-text></x-text>`: an outer
                // block whose only content is one nested inline run.
                let scope = doc.el(text, "view.scope");
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

    // The three CSSOM-View block alignments, computed from the text block's
    // own border box against the 400px scrollport.
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

/// Replicates `scroll-view/scroll-into-view-text-x`
/// (`packages/web-platform/web-elements/tests/fixtures/scroll-view/scroll-into-view-text-x.html`,
/// `packages/web-platform/web-elements/tests/web-elements.spec.ts:808`): the
/// inline-axis half of the case above — a text block is a horizontal scroll
/// target with a view's geometry.
///
/// Same substitution: `scrollIntoView({ inline: ... })` does not exist here,
/// so the three inline alignments are computed from the text block's border
/// box and each one's landing offset is asserted. The fixture's own
/// percentage sizing (`x-view`/`x-text { width: 50%; height: 100% }` against
/// a 400x100 port) is kept, so the 200px slot is measured rather than
/// declared.
#[test]
fn a_text_block_is_an_inline_axis_scroll_target_like_a_view() {
    let mut doc = Doc::with_device(device(600.0, 400.0));
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .port { display: flex; flex-direction: row; flex: none;
                 width: 400px; height: 100px; overflow-x: scroll; }
         .cell { display: flex; flex: none; width: 50%; height: 100%; }
         .textcell { display: -lynx-text; flex: none; width: 50%; height: 100%;
                     font-family: Ahem; font-size: 20px; }
         .scope { display: -lynx-text; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let port = doc.el(root, "view.port");
    let cells: Vec<NodeId> = (0..8)
        .map(|index| {
            if index == 5 {
                let text = doc.el(port, "view.textcell");
                let scope = doc.el(text, "view.scope");
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

/// Replicates `text/text-with-linear-gradient`
/// (`packages/web-platform/web-tests/dist/basic-element-text-text-with-linear-gradient/index.web.
/// json`, `packages/web-platform/web-core-e2e/tests/reactlynx.spec.ts:2623`): Lynx's
/// gradient-valued `color` paints glyph ink with the ramp rather than a solid
/// fill.
///
/// The original is an 80px `<text>` reading "sub text"; Ahem stands in so the
/// ramp can be sampled at exact pixels — a glyph's ink is the whole em square,
/// and `linear-gradient(green, yellow)` with no angle runs top to bottom
/// across the establishing element's padding box, so the top and bottom of one
/// glyph are two different points on the ramp.
#[test]
fn gradient_valued_color_paints_glyph_ink_with_the_ramp() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.add_css(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .text { display: -lynx-text; position: absolute; left: 0px; top: 0px;
                 width: 200px; height: 80px;
                 font-family: Ahem; font-size: 80px; line-height: 80px;
                 color: linear-gradient(green, yellow); }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "view.text");
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
    // The negative control that makes this about glyph ink rather than the
    // box: two 80px em squares stop at x = 160, so x = 180 is inside the
    // block's own 200x80 border box and off the glyphs. A ramp painted as a
    // block fill — the failure the fixture's golden exists to catch — would
    // colour it.
    let off_ink = pixel(&pixels, 200, 180, 40);
    assert!(
        is_white(off_ink),
        "the ramp fills the glyphs, not the block's box ({off_ink:?})",
    );
}

/// Replicates `text/linear-gradient-color`
/// (`packages/web-platform/web-tests/dist/basic-element-text-linear-gradient-color/index.web.json`,
/// `packages/web-platform/web-core-e2e/tests/reactlynx.spec.ts:2765`): a
/// gradient `color` on the block, on one nested inline run, and inherited by
/// an unstyled nested run from a gradient-colored block.
///
/// The fixture builds three paragraphs. This test carries the first and the
/// third, in the fixture's own order and at the fixture's own offsets; the
/// middle one is the sibling test below. The spec file carries a
/// `TODO: fix this issue` note that the inheritance in the third paragraph
/// behaves differently on Android; web-core is the reference (`AGENTS.md`),
/// and under it the nested run inherits.
///
/// The first and third paragraphs hold today and are this test; the middle one
/// does not and is `a_gradient_color_on_a_nested_run_fills_only_that_run`
/// below. The case is split rather than ignored whole so that the two ramps
/// that do paint stay in CI.
///
/// The shared CSS and the `greenish`/`yellowish` predicates are duplicated
/// across the two halves rather than hoisted, so each half reads as the
/// fixture paragraph it replicates.
#[test]
fn a_gradient_color_fills_a_block_and_the_run_that_inherits_it() {
    let mut doc = Doc::with_device(device(240.0, 180.0));
    doc.add_css(
        "page { display: flex; position: relative; width: 240px; height: 180px; }
         .text { display: -lynx-text; position: absolute; left: 0px;
                 width: 240px; height: 40px;
                 font-family: Ahem; font-size: 40px; line-height: 40px; }
         .grad { color: linear-gradient(green, yellow); }
         .scope { display: -lynx-text; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;

    // 1. The gradient on the block itself.
    let block = doc.el(root, "view.text.grad");
    doc.set_inline(block, "top: 0px");
    let run = doc.dom.create_text_node("AA", ());
    doc.dom.append_child(block, run);

    // 3. A gradient block with an unstyled nested run, which inherits it.
    let inheriting = doc.el(root, "view.text.grad");
    doc.set_inline(inheriting, "top: 120px");
    let plain_scope = doc.el(inheriting, "view.scope");
    let plain_run = doc.dom.create_text_node("AA", ());
    doc.dom.append_child(plain_scope, plain_run);

    let pixels = readback(
        "a_gradient_color_fills_a_block_and_the_run_that_inherits_it",
        &mut doc,
        240,
        180,
    );

    // Every em square is 40px wide, so glyph n spans x in [40n, 40n + 40).
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

    // Both rows pinned to glyph coverage rather than to the box: each row is
    // two 40px em squares stopping at x = 80, so x = 200 is inside the 240px
    // block and off the glyphs. A ramp painted as a block fill colours it.
    for (row, y) in [("block", 20), ("inherited", 140)] {
        let off_ink = pixel(&pixels, 240, 200, y);
        assert!(
            is_white(off_ink),
            "the {row} ramp fills the glyphs, not the block's box ({off_ink:?})",
        );
    }
}

/// The middle paragraph of `text/linear-gradient-color`, split out of
/// `a_gradient_color_fills_a_block_and_the_run_that_inherits_it` because it
/// does not hold: a solid-colored block with one gradient-colored nested run
/// must paint each in its own fill.
///
/// This is the reason per-run painting exists, and the reason a golden of the
/// whole page would not catch its absence — such a golden passes just as
/// happily if every glyph wears the establishing element's style.
///
/// The tile a gradient `color` fills from is decided once per paragraph, from
/// the *establishing element's* own color
/// (`crates/dom/src/paint/walker.rs:816-817`), so a paragraph whose block is
/// solid-colored resolves no tile at all and every nested run's gradient falls
/// back to a solid fill (`crates/dom/src/paint/text.rs:71-76`). Measured: the
/// nested run paints `[0, 0, 0, 255]` at both ends of the ramp.
#[test]
#[ignore = "GAP: a gradient `color` on a nested run is ignored unless the establishing element's own color is also a gradient (crates/dom/src/paint/walker.rs:816-817)"]
fn a_gradient_color_on_a_nested_run_fills_only_that_run() {
    let mut doc = Doc::with_device(device(240.0, 180.0));
    doc.add_css(
        "page { display: flex; position: relative; width: 240px; height: 180px; }
         .text { display: -lynx-text; position: absolute; left: 0px;
                 width: 240px; height: 40px;
                 font-family: Ahem; font-size: 40px; line-height: 40px; }
         .grad { color: linear-gradient(green, yellow); }
         .solid { color: #000000; }
         .scope { display: -lynx-text; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;

    // 2. A solid block with one gradient-colored nested run.
    let mixed = doc.el(root, "view.text.solid");
    doc.set_inline(mixed, "top: 60px");
    let solid_run = doc.dom.create_text_node("A", ());
    doc.dom.append_child(mixed, solid_run);
    let gradient_scope = doc.el(mixed, "view.scope.grad");
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

    // Off the two 40px em squares but inside the 240px block.
    let off_ink = pixel(&pixels, 240, 200, 80);
    assert!(
        is_white(off_ink),
        "the ramp fills the glyphs, not the block's box ({off_ink:?})",
    );
}

/// Replicates `text/extra-font-family`
/// (`packages/web-platform/web-core-e2e/tests/reactlynx/basic-element-text-extra-font-family/index.
/// jsx`, `packages/web-platform/web-core-e2e/tests/reactlynx.spec.ts:2596`): a
/// family beyond the default one is selected by name and actually shapes the
/// text that asks for it.
///
/// The original supplies that family through a card-authored
/// `@font-face { src: url(...) }`. Nothing in this engine consumes an
/// `@font-face` rule: the rule parses and enters the cascade
/// (`crates/dom/src/style/engine.rs:413-423`) but there is no `src: url()`
/// fetch anywhere, and faces reach shaping only as embedder-supplied blobs
/// (`crates/dom/src/layout/mod.rs:177`). Ruling R4 of this replication forbids
/// writing the shaping assertion against `@font-face` itself — the descriptor
/// grammar is already pinned by `crates/dom/tests/at_rules.rs` — so this is
/// the adapted replica: the extra face arrives through `register_fonts`
/// instead of a URL, and what is asserted is the half the fixture is really
/// about, that selecting it by family name overrides the default family.
#[test]
fn an_extra_registered_family_is_selected_over_the_default_one() {
    let mut doc = Doc::with_device(device(800.0, 600.0));
    doc.add_css(
        "page { display: flex; flex-direction: column; align-items: flex-start; }
         .label { display: -lynx-text; font-size: 16px; }
         .extra { font-family: Ahem; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(ROBOTO)), 1);
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    assert!(doc.dom.set_default_font_family("Roboto"));
    let root = doc.root;
    let default_family = doc.el(root, "view.label");
    let default_run = doc.dom.create_text_node("EXTRA", ());
    doc.dom.append_child(default_family, default_run);
    let extra_family = doc.el(root, "view.label.extra");
    let extra_run = doc.dom.create_text_node("EXTRA", ());
    doc.dom.append_child(extra_family, extra_run);
    doc.flush();

    // Five Ahem em squares at 16px, exactly.
    assert_eq!(
        ink(&doc, extra_family),
        (80.0, 16.0),
        "the extra family shapes the run that names it",
    );
    // The other half of the selection: the run that names no family is shaped
    // by the registered default, with Roboto's own advances and line metrics
    // for the same five characters. Asserting the number rather than merely
    // "not 80" is what catches a regression that swapped which face answers
    // the default family — a bare inequality is satisfied by any second face.
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

/// Replicates the atomic-inline-box half of
/// `x-text/text-not-resize-detect-new-line`
/// (`packages/web-platform/web-elements/tests/fixtures/x-text/text-not-resize-detect-new-line.
/// html`, `packages/web-platform/web-elements/tests/web-elements.spec.ts:378`), whose
/// fixture nests a whole element inside its `<x-text>`: a non-text child of a
/// text block paints, whatever kind of paint root or stacking context the
/// block is.
///
/// In web-core such a child is an ordinary element inside the shadow host and
/// the browser paints it like any other box; nothing about the enclosing
/// element being a stacking context changes that.
///
/// Two independent defects stopped that here, so the case is carried by three
/// tests rather than one — a single ignore standing for both would not go
/// green when either was fixed, and would never cover the path that already
/// works:
///
/// 1. This test: the in-context descent (`crates/dom/src/visual/build.rs:915-960`), which does
///    reach a text block's non-text child. It passes and stays in CI.
/// 2. `a_boxed_child_paints_from_a_text_block_that_is_its_own_context`: the same child, reached
///    through the paint root and the stacking-context paths, where
///    `crates/dom/src/visual/build.rs:563-570` pushes the paragraph and returns without descending.
/// 3. `an_atomic_inline_box_keeps_the_size_it_measured`: an in-flow atom's own committed box, which
///    `d19cbea2` (#227) closed by giving the paragraph a real inline-box commit entry point.
///
/// The child here is `position: absolute`, which is what separates (1) and (2)
/// from (3): the paragraph's own out-of-flow pass
/// (`crates/dom/src/layout/text_block.rs:465-480`) lays such a child out and
/// gives it a real box, so the only thing left to vary is which frame-builder
/// path the enclosing block takes.
#[test]
fn a_boxed_child_paints_from_a_text_block_inside_its_parents_context() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.add_css(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .text { display: -lynx-text; position: relative;
                 width: 200px; height: 40px;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .child { position: absolute; left: 0px; top: 0px;
                  width: 40px; height: 20px; background-color: #ff0000; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "view.text");
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

/// The paint-root and stacking-context halves of the case above, split out
/// because they do not hold. The child is the same `position: absolute` one
/// that `a_boxed_child_paints_from_a_text_block_inside_its_parents_context`
/// paints, and it has a real box from the paragraph's out-of-flow pass
/// (`crates/dom/src/layout/text_block.rs:465-480`), so nothing but the
/// frame-builder path differs between that test and this one.
///
/// `crates/dom/src/visual/build.rs:563-570` pushes the paragraph and returns
/// without descending — the path a text block takes both as the paint root and
/// as a real stacking context — and the text painter draws only
/// `PositionedLayoutItem::GlyphRun`
/// (`crates/dom/src/paint/text.rs:303-320`), so nobody emits the child.
/// Measured: `[255, 255, 255, 255]` where the child should be.
#[test]
#[ignore = "GAP: a text block that is the paint root or a stacking context never descends past its paragraph (crates/dom/src/visual/build.rs:563-570)"]
fn a_boxed_child_paints_from_a_text_block_that_is_its_own_context() {
    // The text block is the paint root itself.
    let mut as_root = Doc::with_device(device(200.0, 100.0));
    as_root.add_css(
        "page { display: -lynx-text; position: relative;
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

    // The text block is a nested, real stacking context.
    let mut as_context = Doc::with_device(device(200.0, 100.0));
    as_context.add_css(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .text { display: -lynx-text; position: relative; z-index: 1;
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
    let text = as_context.el(root, "view.text");
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

/// The in-flow-atom half of the same case, split out because its cause is a
/// different one: an atomic inline box keeps the size it was measured at.
///
/// In web-core the `x-view` inside an `x-text` is an inline-block with its own
/// declared 40x20 box, and every consumer of that box — paint, hit testing,
/// the scroll area — reads the same number the line was broken against.
///
/// The assertion is on the committed box rather than on pixels, so the frame
/// builder — the subject of the two tests above — takes no part in it.
///
/// This held only from `d19cbea2` (#227) on. Before it the paragraph measured
/// every atom with a measure-goal input, which writes no layout, and
/// `place_and_hide` copied that unwritten `unrounded.size` into the placed
/// layout, so the atom ended the pass 0x0. #227 needed committed geometry for
/// atomic children restored after a content replacement and gave the
/// paragraph a commit-goal path through `compute_inline_box_layout`
/// (`crates/dom/src/layout/text_block.rs:436-444`), which fixed this case with
/// it.
#[test]
fn an_atomic_inline_box_keeps_the_size_it_measured() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.add_css(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .text { display: -lynx-text; position: relative;
                 width: 200px; height: 40px;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .atom { display: flex; width: 40px; height: 20px;
                 background-color: #ff0000; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "view.text");
    let atom = doc.el(text, "view.atom");
    doc.flush();

    let (_, _, width, height) = rect(&doc, atom);
    assert_eq!(
        (width, height),
        (40.0, 20.0),
        "the atom keeps the 40x20 box the line was broken against",
    );
}

/// Replicates the padded-`<x-text>`-with-a-child half of
/// `scroll-view/scroll-into-view-text`
/// (`packages/web-platform/web-elements/tests/fixtures/scroll-view/scroll-into-view-text.html`,
/// `packages/web-platform/web-elements/tests/web-elements.spec.ts:785`), whose
/// text elements nest another element inside them: an atomic inline box sits
/// in the same coordinate system as the glyphs around it, inside the block's
/// border and padding.
///
/// In web-core the paragraph and everything in it lay out inside the content
/// box, so padding moves the two together. Here the paragraph writes an atom's
/// paragraph-space origin straight into `location`
/// (`crates/dom/src/layout/text_block.rs:446`) while every reader of
/// `location` takes it as border-box relative: the frame builder adds it to
/// the element's own border-box offset
/// (`crates/dom/src/visual/build.rs:750-756`) and adds the content-box inset
/// only to the glyphs (`crates/dom/src/visual/build.rs:920-925`), and the
/// same function's own out-of-flow pass puts its children in that space by
/// adding the border back (`crates/dom/src/layout/text_block.rs:474-475`).
/// Glyphs and inline boxes therefore end up in two different coordinate
/// systems, the atom short by the border plus padding.
///
/// The assertion is on `location` rather than on pixels so that it answers
/// only for the origin: the atom's size is a separate claim, carried by
/// `an_atomic_inline_box_keeps_the_size_it_measured`, and whether the frame
/// builder descends to the atom at all is a third one, carried by
/// `a_boxed_child_paints_from_a_text_block_that_is_its_own_context`. Measured:
/// with the 10px padding below, the atom reports `location` `(0, 0)`.
#[test]
#[ignore = "GAP: an atom's paragraph-space origin is written into `location`, which every reader takes as border-box relative (crates/dom/src/layout/text_block.rs:446)"]
fn an_atomic_inline_box_sits_inside_the_blocks_border_and_padding() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.add_css(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .text { display: -lynx-text; position: absolute; left: 20px; top: 10px;
                 width: 160px; height: 80px; box-sizing: border-box;
                 padding: 10px;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .atom { display: flex; width: 40px; height: 20px;
                 background-color: #ff0000; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "view.text");
    let atom = doc.el(text, "view.atom");
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
}
