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
use dom::{FontBlob, FontFaceRequest, FontFaceSource, NodeId, Vector2D};
use flashbulb::headless;

const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");
const ROBOTO: &[u8] = include_bytes!("../../hughie/tests/fixtures/Roboto-Regular.ttf");

/// The four registered integer properties the Lynx UA sheet declares — two for
/// the truncation attributes, one for `tail-color-convert`, one flagging a
/// truncation marker (`crates/bobcat-core/src/main/tree/text.rs:121-124`).
/// Without the `@property` registration a custom property is an untyped token
/// stream and the layout never sees a limit at all.
const LIMIT_PROPERTIES: &str = r#"
@property --lynx-text-maxline { syntax: "<integer>"; inherits: false; initial-value: 0; }
@property --lynx-text-maxlength { syntax: "<integer>"; inherits: false; initial-value: -1; }
@property --lynx-tail-color-convert { syntax: "<integer>"; inherits: false; initial-value: 0; }
@property --lynx-inline-truncation { syntax: "<integer>"; inherits: false; initial-value: 0; }
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

fn is_black(color: [u8; 4]) -> bool {
    color[0] < 60 && color[1] < 60 && color[2] < 60
}

fn is_blue(color: [u8; 4]) -> bool {
    color[2] > 200 && color[0] < 60 && color[1] < 60
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
/// are all crate-private (`crates/dom/src/layout/mod.rs:303`, `:311`, `:319`),
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
/// A marker is content only where the UA sheet flagged it, which this replica's
/// CSS does not do (`collect_block`,
/// `crates/dom/src/layout/text_block.rs:142`), and the marker's own claim is
/// carried by `a_shown_truncation_subtree_is_painted_at_the_clamp`, so this
/// replica pins the re-clamp and not the marker.
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
    let run = doc.dom.create_text_node("b".repeat(30), ());
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

    doc.dom.set_text_node_data(run, "a".repeat(100));
    doc.flush();
    assert_eq!(
        ink(&doc, target),
        (100.0, 40.0),
        "the second long string clamps afresh rather than restoring the first",
    );
}

/// Replicates `text/tail-color-convert`
/// (`web-tests/dist/basic-element-text-tail-color-convert/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2734`) as the colour claim it is.
///
/// **Native semantics, not web-core's** — the user's ruling of 2026-09-15.
/// `tail-color-convert` is a boolean whose default is *false*, and false means
/// the truncation marker wears the colour of the inline run the cut landed in
/// (Android `TextRenderer.convertTailColor`, iOS
/// `LynxTextRenderer.m overrideTruncatedAttrIfNeed`, both of which rewrite the
/// foreground of an ellipsis span that is already built out of that run).
/// True hands the marker the establishing element's own colour instead, and
/// nothing else about it: the dots keep the cut run's font, so no geometry
/// moves between the two passes below. web-core inverts the default and its
/// `="false"` selects a different code path entirely; that is deliberately not
/// replicated.
///
/// The paragraph is one black glyph, then a red nested scope. A one-line
/// clamp at 100px leaves five 20px squares; `text-overflow: ellipsis` backs the
/// cut off by three units, so the kept prefix is the black square plus one red
/// one and the last three squares are the marker — painted red by default and
/// black once the block converts.
#[test]
fn the_truncation_marker_wears_the_cut_run_s_colour_until_the_block_converts() {
    fn marker_and_prefix(convert: bool) -> ([u8; 4], [u8; 4], [u8; 4]) {
        let mut doc = Doc::with_device(device(200.0, 100.0));
        doc.add_ua_css(LIMIT_PROPERTIES);
        doc.add_css(
            "page { display: flex; }
             .text { display: -lynx-text; width: 100px; color: #000000;
                     word-break: break-all; text-overflow: ellipsis;
                     font-family: Ahem; font-size: 20px; line-height: 20px;
                     --lynx-text-maxline: 1; }
             .convert { --lynx-tail-color-convert: 1; }
             .run { display: -lynx-text; color: #ff0000; }",
        );
        assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
        let root = doc.root;
        let text = doc.el(
            root,
            if convert {
                "view.text.convert"
            } else {
                "view.text"
            },
        );
        let lead = doc.dom.create_text_node("A", ());
        doc.dom.append_child(text, lead);
        let nested = doc.el(text, "view.run");
        let run = doc.dom.create_text_node("BBBBBBBBB", ());
        doc.dom.append_child(nested, run);

        let pixels = readback(
            "the_truncation_marker_wears_the_cut_run_s_colour_until_the_block_converts",
            &mut doc,
            200,
            100,
        );
        // One clamped line of five 20px squares: the black lead, one kept red
        // square, then the three-square marker.
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

/// Replicates `x-text/text-maxline-with-custom-truncation`
/// (`packages/web-platform/web-elements/tests/fixtures/x-text/text-maxline-with-custom-truncation.
/// html`, `packages/web-platform/web-elements/tests/web-elements.spec.ts:226`)
/// at the dom layer: a clamped block that overflows paints its
/// `inline-truncation` subtree's content at the end of the last visible line,
/// in place of the units a retreat frees for it, and paints no dots beside it.
///
/// This is the *wiring* half, and it is deliberately narrower than the tree
/// layer's replica of the same fixture
/// (`a_custom_truncation_s_content_replaces_the_clamp_marker_at_every_maxline`
/// in `crates/bobcat-core/src/main/tree/web_text_replication.rs`), which also
/// carries the `inline-truncation { display: none }` default the Lynx UA sheet
/// lifts only for a `text`'s own child. `crates/dom` names no Lynx tag, so the
/// marker here is a nested text scope carrying the registered
/// `--lynx-inline-truncation` that same sheet flags it with — the one fact the
/// paragraph walker keys on.
///
/// The algorithm underneath it is complete one layer down:
/// `custom_truncation_content_replaces_the_marker_at_the_clamp` in
/// `crates/hughie/tests/web_text_replication.rs` passes, and it fixes the
/// geometry asserted below — the cut retreats until the discarded tail is at
/// least as wide as the truncation content, but never by fewer than two units
/// (`removed >= 1 && freed >= needed`, `crates/hughie/src/text/block/truncate.rs:216`,
/// matching web-core's own `maxLineEndAt = end - 1` plus its fitting loop in
/// `XTextTruncation.ts`). Five Ahem em squares fill each 100px line and the
/// content is one square wide, so the minimum governs: two squares are given
/// up, the last line is three kept squares, the marker takes the fourth, and
/// the fifth stays empty.
///
/// Colour, not ink, is what separates the reference from what happens today: a
/// clamped line is 100px wide either way, so only the fifth square of the
/// second line being *red* distinguishes truncation content laid in at the cut
/// from black content that merely reaches the same place.
#[test]
fn a_shown_truncation_subtree_is_painted_at_the_clamp() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.add_ua_css(LIMIT_PROPERTIES);
    doc.add_css(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .text { display: -lynx-text; position: absolute; left: 0px; top: 0px;
                 width: 100px; word-break: break-all; color: #000000;
                 font-family: Ahem; font-size: 20px; line-height: 20px;
                 --lynx-text-maxline: 2; }
         .marker { display: -lynx-text; color: #ff0000;
                   --lynx-inline-truncation: 1; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "view.text");
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

    // Thirty squares break-all onto six 100px lines, clamped to two.
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

/// The box half of the same wiring: an atomic inline box written *inside* the
/// truncation content is laid out and placed like any other atom when the
/// marker is shown, and reports the Lynx `HideView` outcome when it is not.
///
/// Both blocks carry a second `inline-truncation` child as well. web-core
/// honours only the first (`XTextTruncation.ts` queries
/// `:scope > inline-truncation`), and a second one that leaked into the
/// content flow would widen the paragraph, so its absence from every measure
/// below is the assertion.
#[test]
fn a_truncation_atom_is_placed_at_the_clamp_and_hidden_without_one() {
    let mut doc = Doc::with_device(device(400.0, 200.0));
    doc.add_ua_css(LIMIT_PROPERTIES);
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { display: -lynx-text; width: 100px; word-break: break-all;
                 font-family: Ahem; font-size: 20px; line-height: 20px;
                 --lynx-text-maxline: 2; }
         .marker { display: -lynx-text; --lynx-inline-truncation: 1; }
         .icon { display: flex; width: 40px; height: 20px; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;

    let mut block = |content: &str| {
        let text = doc.el(root, "view.text");
        let run = doc.dom.create_text_node(content, ());
        doc.dom.append_child(text, run);
        let marker = doc.el(text, "view.marker");
        let icon = doc.el(marker, "view.icon");
        let spare = doc.el(text, "view.marker");
        let spare_run = doc.dom.create_text_node("HHHHH", ());
        doc.dom.append_child(spare, spare_run);
        (text, icon)
    };
    let (overflowing, placed) = block(&"H".repeat(30));
    let (fitting, unused) = block("HHHHH");
    doc.flush();

    // Six break-all lines clamped to two. The 40px icon needs two of the five
    // squares on the clamp line, which is also the two-unit minimum, so three
    // squares are kept and the icon takes the fourth and fifth.
    assert_eq!(ink(&doc, overflowing), (100.0, 40.0));
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

/// The `inline-truncation` element itself gets no *box*: it is a text scope,
/// so it has no paragraph and no layout box of its own, and the slot the
/// paragraph leaves it is empty whether or not its content is shown.
///
/// What paints is its *runs* — through the flattened paragraph, in the
/// marker's own colour — and, since the 2026-09-16 ruling, its own
/// `background-color` behind exactly those runs, as the inline box it is. A
/// background on the marker is therefore the probe for both halves: it must
/// cover the marker's fragment and nothing else — not the rest of the clamp
/// line, not the paragraph, which is what an element with a box would have
/// painted.
///
/// The marker's content is a space and an `H`, not a bare `H`: Ahem's glyphs
/// are solid em squares, so ink covering the whole fragment would hide the
/// very background this samples. The blank first unit is where the background
/// is read.
#[test]
fn a_shown_truncation_marker_paints_its_runs_and_a_background_behind_them_only() {
    fn readback_marker(content: &str) -> Vec<u8> {
        let mut doc = Doc::with_device(device(200.0, 100.0));
        doc.add_ua_css(LIMIT_PROPERTIES);
        doc.add_css(
            "page { display: flex; align-items: flex-start; }
             .text { display: -lynx-text; width: 100px; word-break: break-all;
                     color: #000000; font-family: Ahem; font-size: 20px;
                     line-height: 20px; --lynx-text-maxline: 2; }
             .marker { display: -lynx-text; --lynx-inline-truncation: 1;
                       color: #ff0000; background-color: #0000ff; }",
        );
        assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
        let root = doc.root;
        let text = doc.el(root, "view.text");
        let run = doc.dom.create_text_node(content, ());
        doc.dom.append_child(text, run);
        let marker = doc.el(text, "view.marker");
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
    // Three kept squares, then the marker's two units: a blank one carrying
    // the background and the marker's own glyph.
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

/// A nested `<text>` scope has no box of its own — the paragraph is flattened
/// and its slot hidden — so until the 2026-09-16 ruling a `background-color`
/// on it painted nothing at all. It now paints as css-backgrounds-3 says an
/// *inline box* does, which is what web-core gets by making a nested
/// `x-text`/`inline-text` `display: inline`
/// (`packages/web-platform/web-elements/src/elements/XText/x-text.css:52-67`
/// adds nothing to that but `background-clip: inherit`): one fragment per
/// line, spanning that scope's own glyphs horizontally and the font's content
/// area — ascent over descent — vertically.
///
/// Native Lynx fills the *line box* instead (Android `BackgroundColorSpan` /
/// `LynxTextBackgroundSpan`, iOS `NSBackgroundColorAttributeName`); this
/// engine follows web-core, and the `line-height: 40px` below is what tells
/// the two apart — the 10px of half-leading above and below the 20px content
/// area must stay unpainted.
///
/// Every test in this group paints the nested run's ink `transparent`. Ahem's
/// glyphs are solid em squares that cover the whole content area, which is
/// exactly the band the background fills, so visible ink would hide the
/// subject.
#[test]
fn a_nested_scope_s_background_paints_behind_its_own_fragments_only() {
    let mut doc = Doc::with_device(device(300.0, 60.0));
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { display: -lynx-text; width: 300px; color: #000000;
                 font-family: Ahem; font-size: 20px; line-height: 40px; }
         .tag { display: -lynx-text; color: transparent;
                background-color: #0000ff; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "view.text");
    let before = doc.dom.create_text_node("AAA", ());
    doc.dom.append_child(text, before);
    let tag = doc.el(text, "view.tag");
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

    // Nine em squares at 20px: the nested run is the fourth through sixth, so
    // its fragment is x 60..120.
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

    // The line box is 40px and the content area 20px, so the half-leading is
    // y 0..10 and y 30..40. A native-style line-box fill would paint it.
    for y in [2, 8, 32, 38] {
        assert!(
            is_white(pixel(&pixels, 300, 90, y)),
            "and the half-leading stays unpainted: the fragment is the content \
             area, not the line box (y={y})",
        );
    }
}

/// `box-decoration-break: slice`, approximated: a nested scope that wraps gets
/// one fragment per line, each covering only the part of the scope that landed
/// on that line.
#[test]
fn a_nested_scope_that_wraps_paints_one_fragment_per_line() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { display: -lynx-text; width: 100px; word-break: break-all;
                 color: #000000; font-family: Ahem; font-size: 20px;
                 line-height: 20px; }
         .tag { display: -lynx-text; color: transparent;
                background-color: #0000ff; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "view.text");
    let before = doc.dom.create_text_node("AAA", ());
    doc.dom.append_child(text, before);
    let tag = doc.el(text, "view.tag");
    let nested = doc.dom.create_text_node("BBBBBB", ());
    doc.dom.append_child(tag, nested);
    let pixels = readback(
        "a_nested_scope_that_wraps_paints_one_fragment_per_line",
        &mut doc,
        200,
        100,
    );

    // Five squares per 100px line: AAA and two of the six B squares on the
    // first, the remaining four on the second.
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

/// Paint order within a paragraph's inline backgrounds: outermost scope first,
/// so a nested scope's background covers the one it sits inside.
#[test]
fn an_inner_scope_s_background_covers_its_ancestor_s() {
    let mut doc = Doc::with_device(device(300.0, 40.0));
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { display: -lynx-text; width: 300px; color: #000000;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .outer { display: -lynx-text; color: transparent;
                  background-color: #0000ff; }
         .inner { display: -lynx-text; background-color: #ff0000; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "view.text");
    let before = doc.dom.create_text_node("AA", ());
    doc.dom.append_child(text, before);
    let outer = doc.el(text, "view.outer");
    let outer_run = doc.dom.create_text_node("BB", ());
    doc.dom.append_child(outer, outer_run);
    let inner = doc.el(outer, "view.inner");
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

/// A `display: contents` element generates no box, so it has no background
/// painting area either (css-display-3 3.3) — which matters here because the
/// compiled `wrapper` carrier between a `<text>` and its nested `<text>` is
/// exactly that element.
///
/// The wrapper holds two backgrounded scopes with a plain run between them, so
/// a pass that let a `display: contents` ancestor into the chain would betray
/// itself twice over: its fragment is the union of both scopes' runs, and the
/// gap between them is the one place neither scope paints. The scopes' own
/// backgrounds still paint, which is what keeps this from passing on a pass
/// that does nothing at all.
#[test]
fn a_wrapper_between_scopes_paints_no_background() {
    let mut doc = Doc::with_device(device(300.0, 40.0));
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { display: -lynx-text; width: 300px; color: transparent;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .wrapper { display: contents; background-color: #0000ff; }
         .tag { display: -lynx-text; background-color: #ff0000; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "view.text");
    let before = doc.dom.create_text_node("AA", ());
    doc.dom.append_child(text, before);
    let wrapper = doc.el(text, "view.wrapper");
    let first = doc.el(wrapper, "view.tag");
    let first_run = doc.dom.create_text_node("BB", ());
    doc.dom.append_child(first, first_run);
    let between = doc.dom.create_text_node("XX", ());
    doc.dom.append_child(wrapper, between);
    let second = doc.el(wrapper, "view.tag");
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

/// An atomic inline box inside a nested scope is part of that scope's
/// fragment: the background runs behind the atom as it does behind the glyphs
/// beside it.
///
/// The atom is unioned into the fragment on *both* axes. A browser keeps the
/// band at the inline box's own font metrics and lets a taller atom overflow
/// it; the union is the approximation this engine takes, because it is also
/// what lets a scope whose only content is an atom — which has no glyph run to
/// take metrics from — paint anything at all
/// (`crates/dom/src/paint/text.rs`'s `inline_background_fragments`).
#[test]
fn an_atom_inside_a_nested_scope_is_covered_by_its_background() {
    let mut doc = Doc::with_device(device(300.0, 40.0));
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { display: -lynx-text; width: 300px; color: transparent;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .tag { display: -lynx-text; background-color: #0000ff; }
         .icon { display: flex; width: 40px; height: 10px; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "view.text");
    let before = doc.dom.create_text_node("AA", ());
    doc.dom.append_child(text, before);
    let tag = doc.el(text, "view.tag");
    let nested = doc.dom.create_text_node("B", ());
    doc.dom.append_child(tag, nested);
    doc.el(tag, "view.icon");
    let pixels = readback(
        "an_atom_inside_a_nested_scope_is_covered_by_its_background",
        &mut doc,
        300,
        40,
    );

    // Two squares of plain run, then the scope: one square and a 40px atom.
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

/// A `background-image` layer on a nested scope fills the same fragment its
/// `background-color` would, gradient box and all — the fragment is handed to
/// the ordinary background painter, so every layer the property supports
/// resolves against it.
#[test]
fn a_gradient_background_image_on_a_nested_scope_fills_its_fragment() {
    let mut doc = Doc::with_device(device(300.0, 40.0));
    doc.add_css(
        "page { display: flex; align-items: flex-start; }
         .text { display: -lynx-text; width: 300px; color: #000000;
                 font-family: Ahem; font-size: 20px; line-height: 20px; }
         .tag { display: -lynx-text; color: transparent;
                background-image: linear-gradient(90deg, #ff0000, #0000ff); }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let text = doc.el(root, "view.text");
    let before = doc.dom.create_text_node("AAA", ());
    doc.dom.append_child(text, before);
    let tag = doc.el(text, "view.tag");
    let nested = doc.dom.create_text_node("BBB", ());
    doc.dom.append_child(tag, nested);
    let pixels = readback(
        "a_gradient_background_image_on_a_nested_scope_fills_its_fragment",
        &mut doc,
        300,
        40,
    );

    // The ramp spans the fragment, x 60..120, and nothing wider: a tile taken
    // from the block would have run the whole 300px.
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
/// on the sixth of eight equal children. This engine has no `scrollIntoView`
/// at any layer — `crates/dom/src/scroll/mod.rs` exposes `scroll_box`,
/// `scroll_offset`, `scroll_to(id, offset)` (`:200`), `scroll_by`,
/// `nearest_user_scrollable` and `scroll_chain`, and nothing else in the
/// workspace names the operation — so no test can call it.
///
/// What this test pins is therefore stated plainly, and it is **not**
/// alignment: it is the scroll-target *geometry* a text block exposes — its
/// border box, its container's `scroll_size` and `max_offset` — plus
/// `scroll_to`'s clamping of an offset against that maximum. The three
/// CSSOM-View block alignments below are computed *by this test*, from that
/// geometry, and then asserted to land where the arithmetic says; no engine
/// code participates in the alignment step, so a wrong or missing alignment
/// implementation could not fail this test. The alignment computation is an
/// open gap that no test in this replication carries.
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
/// Same substitution, and the same limit on what it proves:
/// `scrollIntoView({ inline: ... })` does not exist here, so what is pinned is
/// the text block's inline-axis scroll-target geometry and `scroll_to`'s
/// clamping on that axis — not alignment. The three inline alignments are
/// arithmetic this test performs on the block's own border box before calling
/// `scroll_to`, so no engine code computes them and none of these assertions
/// can fail on them. The fixture's own percentage sizing (`x-view`/`x-text
/// { width: 50%; height: 100% }` against a 400x100 port) is kept, so the 200px
/// slot is measured rather than declared.
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
/// The tile a gradient `color` fills from is decided per run
/// (`crates/dom/src/paint/text.rs:215-269`): the establishing element keeps its
/// padding box, a nested element gets the union of its own line fragments. It
/// used to be one paragraph-level decision taken from the establishing
/// element's own color, so a solid-colored block resolved no tile at all and
/// every nested run's gradient fell back to a solid fill.
#[test]
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
/// `@font-face { src: url(...) }`. `dom` fetches nothing itself, so the
/// fixture is carried by two tests: this one, the adapted replica, where the
/// extra face arrives through `register_fonts` and what is asserted is that
/// selecting it by family name overrides the default family; and
/// `a_font_face_declared_family_shapes_the_text_that_names_it` below, which
/// asserts the unadapted reference — the card's own `@font-face`, reported
/// over the loader seam and reaching shaping under its declared family.
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

/// Replicates `text/extra-font-family`
/// (`packages/web-platform/web-core-e2e/tests/reactlynx/basic-element-text-extra-font-family/index.
/// jsx`, `packages/web-platform/web-core-e2e/tests/reactlynx.spec.ts:2596`)
/// without the adaptation the test above makes: the family the card declares
/// with `@font-face { font-family: ...; src: url(...) }` must be the family
/// that shapes the text naming it.
///
/// This is what a browser gives web-core, and it is the whole point of the
/// fixture — a card ships a face with its bundle and uses it. The `src` here
/// is an absolute `file:` URL to a face that really exists in this repo, so
/// the assertion is answerable by any implementation that fetches it: five
/// Ahem em squares at 16px are 80px of advance on a 16px line box, a number no
/// other face in this test produces.
///
/// `dom` performs no IO (`AGENTS.md`), so the rule is not fetched here: it is
/// *reported*. `Document::take_font_face_requests`
/// (`crates/dom/src/layout/mod.rs`) reduces every `@font-face` rule the
/// cascade collected to a family plus its `src` components in author order,
/// once per rule; the embedder loads one of them and hands the bytes back
/// through `Document::register_font_face`, which files the face under the
/// declared family rather than under the name inside the font file
/// (`crates/hughie/src/text/context.rs`). This test stands in for the
/// embedder — it reads the `file:` URL it was handed off the disk — so what it
/// pins is the whole seam: the rule reaches the loader, and the loaded face
/// reaches shaping under the declared name. The engine's own embedder half
/// lives in `crates/bobcat-core` (the page's epilogue spawns one load per
/// request over `SourceRequest::Font`).
///
/// The bundle-side half of this path covers only the wire:
/// `font_face_with_a_src_url_survives_the_bundle` and
/// `font_face_descriptors_decode_in_authored_order_and_form` in
/// `crates/bobcat-source/tests/web_text_css_replication.rs` carry the
/// descriptors from a `.web.bundle` into the engine's stylesheet contract, and
/// hand them to exactly the rule this test reads back.
#[test]
fn a_font_face_declared_family_shapes_the_text_that_names_it() {
    const AHEM_URL: &str = concat!(
        "file://",
        env!("CARGO_MANIFEST_DIR"),
        "/../hughie/tests/fixtures/Ahem.ttf"
    );

    let mut doc = Doc::with_device(device(800.0, 600.0));
    doc.add_css(&format!(
        "@font-face {{ font-family: DeclaredAhem;
                       src: url(\"{AHEM_URL}\") format(\"truetype\"); }}
         page {{ display: flex; flex-direction: column; align-items: flex-start; }}
         .label {{ display: -lynx-text; font-size: 16px;
                   font-family: DeclaredAhem; }}"
    ));
    // Only the default face is handed over the embedder seam; the extra one is
    // the card's own, and reaches the document only through the rule above.
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(ROBOTO)), 1);
    assert!(doc.dom.set_default_font_family("Roboto"));

    // What is reported is the URL resolved against the document's base, so
    // the `..` this fixture path spells has already been normalized away.
    let resolved = AHEM_URL.replace("/dom/../hughie/", "/hughie/");
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

    // The embedder's half: fetch the source and hand the bytes back.
    let bytes = std::fs::read(
        resolved
            .strip_prefix("file://")
            .expect("the fixture URL is a file URL"),
    )
    .expect("the vendored Ahem fixture is on disk");
    assert_eq!(doc.dom.register_font_face("DeclaredAhem", bytes.into()), 1);

    let root = doc.root;
    let label = doc.el(root, "view.label");
    let run = doc.dom.create_text_node("EXTRA", ());
    doc.dom.append_child(label, run);
    doc.flush();

    assert_eq!(
        ink(&doc, label),
        (80.0, 16.0),
        "the declared face shapes the run that names its family, rather than \
         the run falling back to another face",
    );

    // A sheet mounted later is reported when it arrives, and only it.
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
/// 1. This test: the in-context descent (`crates/dom/src/visual/build.rs:902-965`), which does
///    reach a text block's non-text child. It has always passed.
/// 2. `a_boxed_child_paints_from_a_text_block_that_is_its_own_context`: the same child, reached
///    through the paint root and the stacking-context paths, where `build_stacking_context` used to
///    push the paragraph and return without descending. It now falls through to the same collection
///    walk (`crates/dom/src/visual/build.rs:563-569`).
/// 3. `an_atomic_inline_box_keeps_the_size_it_measured`: an in-flow atom's own committed box, which
///    `d19cbea2` (#227) closed by giving the paragraph a real inline-box commit entry point.
///
/// The child here is `position: absolute`, which is what separates (1) and (2)
/// from (3): the paragraph's own out-of-flow pass
/// (`crates/dom/src/layout/text_block.rs:569-584`) lays such a child out and
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
/// because they failed for their own reason. The child is the same
/// `position: absolute` one that
/// `a_boxed_child_paints_from_a_text_block_inside_its_parents_context` paints,
/// and it has a real box from the paragraph's out-of-flow pass
/// (`crates/dom/src/layout/text_block.rs:569-584`), so nothing but the
/// frame-builder path differs between that test and this one.
///
/// `build_stacking_context` — the path a text block takes both as the paint
/// root and as a real stacking context — pushed the paragraph and returned
/// without descending, and the text painter draws only
/// `PositionedLayoutItem::GlyphRun` (`crates/dom/src/paint/text.rs:403-420`),
/// so nobody emitted the child: `[255, 255, 255, 255]` where it should be.
/// It now pushes the paragraph and falls through to the collection walk
/// (`crates/dom/src/visual/build.rs:563-569`), the same order as the in-context
/// path. The glyphs stay unique because `collect_child`
/// (`crates/dom/src/visual/build.rs:829-871`) drops text nodes and the layout
/// slots `place_and_hide` (`crates/dom/src/layout/text_block.rs:607`) hid, so
/// an absorbed nested scope reaches no second record.
#[test]
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
/// (`crates/dom/src/layout/text_block.rs:540-548`), which fixed this case with
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
/// (`crates/dom/src/layout/text_block.rs:667`) while every reader of
/// `location` takes it as border-box relative: the frame builder adds it to
/// the element's own border-box offset
/// (`crates/dom/src/visual/build.rs:750-756`) and adds the content-box inset
/// only to the glyphs (`crates/dom/src/visual/build.rs:920-925`), and the
/// same function's own out-of-flow pass puts its children in that space by
/// adding the border back (`crates/dom/src/layout/text_block.rs:698-699`).
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
#[ignore = "GAP: an atom's paragraph-space origin is written into `location`, which every reader takes as border-box relative (crates/dom/src/layout/text_block.rs:667)"]
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
