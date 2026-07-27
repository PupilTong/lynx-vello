//! Structural scene tests: `Document` → `PaintOrder` → `vello::Scene`,
//! asserting on the scene's encoding (draw counts, layer balance) without
//! touching a GPU.

mod common;

use common::Doc;
use pulsar::vello::kurbo::Size;
use pulsar::{ImageStore, PaintOptions, Painter};
use w3c_dom::visual::PaintOrder;

const AHEM: &[u8] = include_bytes!("../../neutron-star/tests/fixtures/Ahem.ttf");

const PAGE: &str = "page { display: flex; position: relative; width: 800px; height: 600px; }
    .box { display: flex; position: absolute; left: 10px; top: 10px;
           width: 100px; height: 100px; }";

struct Harness {
    doc: Doc,
    painter: Painter,
    images: ImageStore,
}

impl Harness {
    fn new(css: &str) -> Self {
        Self {
            doc: Doc::with_css(&format!("{PAGE} {css}")),
            painter: Painter::new(),
            images: ImageStore::new(),
        }
    }

    fn frame(&mut self) -> PaintOrder {
        self.doc.dom.paint_order()
    }

    /// Paints and returns `(draw ops, clip/layer pairs, open layers)`.
    fn stats(&mut self) -> (usize, u32, u32) {
        let frame = self.doc.dom.paint_order();
        let options = PaintOptions {
            scale: 1.0,
            viewport: Size::new(800.0, 600.0),
        };
        let scene = self
            .painter
            .paint(&self.doc.dom, &frame, &self.images, &options);
        let encoding = scene.encoding();
        (
            encoding.draw_tags.len(),
            encoding.n_clips,
            encoding.n_open_clips,
        )
    }
}

#[test]
fn painting_is_layer_balanced_and_nonempty() {
    let mut h = Harness::new(".bg { background-color: rebeccapurple; }");
    let root = h.doc.root;
    h.doc.el(root, "box bg");
    let (draws, _, open) = h.stats();
    assert!(draws > 0, "a background must encode at least one draw");
    assert_eq!(open, 0, "every pushed layer must be popped");
}

#[test]
fn opacity_groups_add_layers() {
    let mut h = Harness::new(".bg { background-color: rebeccapurple; } .fade { opacity: 0.5; }");
    let root = h.doc.root;
    h.doc.el(root, "box bg");
    let (_, clips_plain, _) = h.stats();

    let mut h2 = Harness::new(".bg { background-color: rebeccapurple; } .fade { opacity: 0.5; }");
    let root2 = h2.doc.root;
    h2.doc.el(root2, "box bg fade");
    let (_, clips_faded, open) = h2.stats();
    assert_eq!(open, 0);
    assert!(
        clips_faded > clips_plain,
        "an opacity group must push an effect layer ({clips_faded} vs {clips_plain})"
    );
}

#[test]
fn overflow_clips_push_clip_layers() {
    let mut h = Harness::new(".clip { overflow: hidden; } .bg { background-color: teal; }");
    let root = h.doc.root;
    let clip = h.doc.el(root, "box clip");
    h.doc.el(clip, "box bg");
    let (_, clips, open) = h.stats();
    assert_eq!(open, 0);
    assert!(clips > 0, "overflow: hidden must clip its content");
}

#[test]
fn singular_transforms_paint_nothing() {
    let css = ".bg { background-color: teal; } .gone { transform: scale(0, 1); }";
    let mut h = Harness::new(css);
    let root = h.doc.root;
    h.doc.el(root, "box bg gone");
    let (draws_gone, _, open) = h.stats();
    assert_eq!(open, 0);

    let mut h2 = Harness::new(css);
    let root2 = h2.doc.root;
    h2.doc.el(root2, "box bg");
    let (draws_there, _, _) = h2.stats();
    assert!(
        draws_gone < draws_there,
        "a singular transform must not render ({draws_gone} vs {draws_there})"
    );
}

#[test]
fn text_runs_encode_glyphs() {
    let mut h = Harness::new(
        ".text { display: flex; width: 200px; height: 50px;
                 font-family: Ahem; font-size: 20px; color: black; }",
    );
    h.doc.dom.register_fonts(AHEM);
    let root = h.doc.root;
    let holder = h.doc.el(root, "box text");
    h.doc.text(holder, "Hello");
    let (draws, _, open) = h.stats();
    assert_eq!(open, 0);
    assert!(draws > 0, "text must encode glyph draws");
}

#[test]
fn frames_can_be_repainted_with_a_reused_painter() {
    let mut h = Harness::new(".bg { background-color: teal; opacity: 0.7; overflow: hidden; }");
    let root = h.doc.root;
    let outer = h.doc.el(root, "box bg");
    h.doc.el(outer, "box bg");
    let first = h.stats();
    let second = h.stats();
    assert_eq!(first, second, "repainting the same frame must be stable");
}

#[test]
fn hidden_group_roots_still_composite_children() {
    // The layer arena survives into painting: an invisible opacity root with
    // a visible child still pushes the group.
    let mut h = Harness::new(
        ".ghost { opacity: 0.5; visibility: hidden; }
         .shown { visibility: visible; background-color: teal; }",
    );
    let root = h.doc.root;
    let ghost = h.doc.el(root, "box ghost");
    h.doc.el(ghost, "box shown");
    let frame = h.frame();
    assert_eq!(frame.layers().len(), 1);
    let (draws, clips, open) = h.stats();
    assert_eq!(open, 0);
    assert!(draws > 0);
    assert!(clips > 0);
}

#[test]
fn background_clip_text_sandwiches_the_layer() {
    // With glyphs present, `background-clip: text` wraps the background in
    // an isolate + SrcIn sandwich (two extra layers, balanced); without
    // text children, the clip region is empty and the background vanishes.
    let css = ".text { display: flex; width: 200px; height: 50px;
                        font-family: Ahem; font-size: 20px; color: black;
                        background-color: rebeccapurple; background-clip: text; }";
    let mut with_text = Harness::new(css);
    with_text.doc.dom.register_fonts(AHEM);
    let root = with_text.doc.root;
    let holder = with_text.doc.el(root, "box text");
    with_text.doc.text(holder, "Hello");
    let (draws_text, clips_text, open) = with_text.stats();
    assert_eq!(open, 0);
    assert!(draws_text > 0);
    assert!(
        clips_text >= 2,
        "the text-clip sandwich must push layers ({clips_text})"
    );

    let mut empty = Harness::new(css);
    empty.doc.dom.register_fonts(AHEM);
    let root2 = empty.doc.root;
    empty.doc.el(root2, "box text");
    let (_, clips_empty, open2) = empty.stats();
    assert_eq!(open2, 0);
    assert!(
        clips_empty < clips_text,
        "an empty text clip paints no sandwich ({clips_empty} vs {clips_text})"
    );
}

#[test]
fn outlines_paint_outside_the_border_box() {
    let plain = ".bg { background-color: teal; }";
    let mut h = Harness::new(plain);
    let root = h.doc.root;
    h.doc.el(root, "box bg");
    let (draws_plain, _, _) = h.stats();

    let outlined = ".bg { background-color: teal; outline: 3px solid red; }";
    let mut h2 = Harness::new(outlined);
    let root2 = h2.doc.root;
    h2.doc.el(root2, "box bg");
    let (draws_outlined, _, open) = h2.stats();
    assert_eq!(open, 0);
    assert!(
        draws_outlined > draws_plain,
        "an outline must add draws ({draws_outlined} vs {draws_plain})"
    );

    // outline-style: none paints nothing even with a width.
    let none = ".bg { background-color: teal; outline: 3px red; }";
    let mut h3 = Harness::new(none);
    let root3 = h3.doc.root;
    h3.doc.el(root3, "box bg");
    let (draws_none, _, _) = h3.stats();
    assert_eq!(
        draws_none, draws_plain,
        "width without style must not paint"
    );
}

#[test]
fn transparent_border_sides_reject_the_uniform_fast_path() {
    // `border: 10px solid transparent` + one red side: the red side must
    // NOT fill the whole ring (the transparent sides own ring area that
    // stays unpainted), so painting must take the per-side path — visible
    // as its miter-quad clip layers.
    let uniform = ".bg { border: 10px solid red; }";
    let mut h = Harness::new(uniform);
    let root = h.doc.root;
    h.doc.el(root, "box bg");
    let (_, clips_uniform, open) = h.stats();
    assert_eq!(open, 0);

    let mixed = ".bg { border: 10px solid transparent; border-top-color: red; }";
    let mut h2 = Harness::new(mixed);
    let root2 = h2.doc.root;
    h2.doc.el(root2, "box bg");
    let (_, clips_mixed, open2) = h2.stats();
    assert_eq!(open2, 0);
    assert!(
        clips_mixed > clips_uniform,
        "a transparent positive-width side must force the per-side path \
         ({clips_mixed} vs {clips_uniform})"
    );
}

#[test]
#[should_panic(expected = "visually stale PaintOrder")]
fn painting_a_frame_built_before_a_style_mutation_panics() {
    // Reviewer scenario: change `display`/style after building the frame,
    // even with a completed re-layout — the old frame's geometry mixed
    // with live styles is incoherent (a display:none element would paint
    // at its former size).
    let mut h = Harness::new(".bg { background-color: teal; }");
    let root = h.doc.root;
    let el = h.doc.el(root, "box bg");
    let frame = h.doc.dom.paint_order();
    h.doc.dom.set_inline_style(el, "display: none");
    h.doc.dom.layout();
    let options = PaintOptions {
        scale: 1.0,
        viewport: Size::new(800.0, 600.0),
    };
    let _ = h.painter.paint(&h.doc.dom, &frame, &h.images, &options);
}

#[test]
fn text_decorations_propagate_through_nested_boxes() {
    // css-text-decor-3 §2: text-decoration-line is not inherited — it
    // propagates from ancestor decorating boxes to in-flow descendant
    // text. The text's immediate parent here has no decoration of its own.
    let css = ".u { text-decoration-line: underline; }
        .inner { display: flex; }
        .text { display: flex; width: 200px; height: 50px;
                font-family: Ahem; font-size: 20px; color: black; }";
    let mut plain = Harness::new(css);
    plain.doc.dom.register_fonts(AHEM);
    let root = plain.doc.root;
    let outer = plain.doc.el(root, "box text");
    let inner = plain.doc.el(outer, "inner");
    plain.doc.text(inner, "Hi");
    let (draws_plain, _, _) = plain.stats();

    let mut decorated = Harness::new(css);
    decorated.doc.dom.register_fonts(AHEM);
    let root2 = decorated.doc.root;
    let outer2 = decorated.doc.el(root2, "box text u");
    let inner2 = decorated.doc.el(outer2, "inner");
    decorated.doc.text(inner2, "Hi");
    let (draws_decorated, _, open) = decorated.stats();
    assert_eq!(open, 0);
    assert!(
        draws_decorated > draws_plain,
        "an ancestor underline must reach nested text \
         ({draws_decorated} vs {draws_plain})"
    );

    // But not INTO an absolutely positioned descendant (spec exception).
    let abs_css = ".u { text-decoration-line: underline; }
        .inner { display: flex; position: absolute; left: 0; top: 0;
                 width: 200px; height: 50px; }
        .text { display: flex; width: 200px; height: 50px;
                font-family: Ahem; font-size: 20px; color: black; }";
    let mut escaped = Harness::new(abs_css);
    escaped.doc.dom.register_fonts(AHEM);
    let root3 = escaped.doc.root;
    let outer3 = escaped.doc.el(root3, "box text u");
    let inner3 = escaped.doc.el(outer3, "inner");
    escaped.doc.text(inner3, "Hi");
    let (draws_escaped, _, _) = escaped.stats();
    assert_eq!(
        draws_escaped, draws_plain,
        "decorations must not propagate into out-of-flow boxes"
    );
}
