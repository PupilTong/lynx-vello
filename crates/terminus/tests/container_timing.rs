//! W3C Container Timing as the document and the presenting painter build it:
//! roots marked on the document (`Document::set_container_timing`,
//! `Document::set_container_timing_ignored`, `Document::stop_container_timing`),
//! the paints the walk attributes to them, and the reports the painter makes
//! after a present (`Painter::take_container_paints`) over an offscreen
//! target.

mod paint_common;

use paint_common::Doc;
use terminus::link::painter::{DrawTarget, GesturePolicy, Painter};
use terminus::link::{ContainerPaint, FontBlob, NoImages, NodeId, ShadowRootMode};

const AHEM: &[u8] = include_bytes!("../../seldon/tests/fixtures/Ahem.ttf");

const PAGE: &str = "page { display: flex; position: relative; width: 800px; height: 600px; }
    .abs { display: flex; position: absolute; }
    .area { left: 0px; top: 0px; width: 400px; height: 300px; }
    .clip { overflow: hidden; }
    .bg { background-color: teal; }
    .text { font-family: Ahem; font-size: 20px; color: black; }";

/// A page where a `text` element is a Lynx text element and Ahem the one
/// face.
fn page() -> Doc {
    let mut doc = Doc::with_css(PAGE);
    doc.define_lynx_text("text");
    doc.dom.register_fonts(FontBlob::from_static(AHEM));
    doc
}

/// An absolutely placed `view` under `parent` with `class`, its border box
/// at (`left`, `top`) and `width`×`height`.
fn area(doc: &mut Doc, parent: NodeId, left: f32, top: f32, size: (f32, f32)) -> NodeId {
    let node = doc.el(parent, "abs");
    doc.dom.set_inline_style(
        node,
        &format!(
            "left: {left}px; top: {top}px; width: {}px; height: {}px",
            size.0, size.1
        ),
    );
    node
}

/// A `text` element under `parent` at (`left`, `top`) holding `content`:
/// one Ahem line, 20 px per character.
fn text_at(doc: &mut Doc, parent: NodeId, left: f32, top: f32, content: &str) -> NodeId {
    let holder = doc.el_tag(parent, "text", "abs text");
    doc.dom
        .set_inline_style(holder, &format!("left: {left}px; top: {top}px"));
    doc.text(holder, content);
    holder
}

const POLICY: GesturePolicy = GesturePolicy {
    tap_slop: 50.0,
    long_press_move_slop: 8.0,
    drag_slop: 8.0,
    long_press_seconds: 0.5,
    tap_event: "tap",
    long_press_event: "longpress",
};

fn offscreen_painter() -> Painter {
    offscreen_painter_at(1.0)
}

fn offscreen_painter_at(device_pixel_ratio: f32) -> Painter {
    pollster::block_on(Painter::new(
        DrawTarget::Offscreen,
        800.0,
        600.0,
        device_pixel_ratio,
        POLICY,
    ))
    .expect("a GPU adapter is mandatory for the render tests")
}

/// Commits the document, presents the frame offscreen, and answers what the
/// painter reported for it.
fn present(painter: &mut Painter, doc: &mut Doc) -> Vec<ContainerPaint> {
    let frame = doc.dom.commit();
    assert!(painter.adopt(Some(frame), Some(&NoImages)));
    let now = painter.now_seconds();
    assert!(
        painter
            .draw_offscreen(now, false)
            .expect("the draw succeeds"),
        "a newly adopted frame renders"
    );
    painter.take_container_paints()
}

/// A `text` element under `parent`, a node of a shadow tree, styled inline
/// as [`text_at`]'s are by the page's sheet, which does not reach into a
/// shadow tree.
fn shadow_text_at(doc: &mut Doc, parent: NodeId, left: f32, top: f32, content: &str) -> NodeId {
    let holder = doc.dom.create_element("text", ());
    doc.dom.set_inline_style(
        holder,
        &format!(
            "display: flex; position: absolute; left: {left}px; top: {top}px; \
             font-family: Ahem; font-size: 20px; color: black"
        ),
    );
    doc.dom.append_child(parent, holder);
    doc.text(holder, content);
    holder
}

/// `(x, y, width, height)` of a report's painted bounds.
fn bounds(paint: &ContainerPaint) -> (f32, f32, f32, f32) {
    (
        paint.painted.origin.x,
        paint.painted.origin.y,
        paint.painted.size.width,
        paint.painted.size.height,
    )
}

/// The one report `paints` holds.
fn only(paints: &[ContainerPaint]) -> ContainerPaint {
    let [paint] = paints[..] else {
        panic!("one report: {paints:?}");
    };
    paint
}

#[test]
fn a_page_with_no_root_reports_nothing() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    text_at(&mut doc, root, 10.0, 10.0, "Hello");
    assert!(present(&mut painter, &mut doc).is_empty());
}

#[test]
fn a_root_reports_its_child_s_paint_in_css_px() {
    // At a device pixel ratio of 2: the report is in CSS px all the same.
    let mut painter = offscreen_painter_at(2.0);
    let mut doc = page();
    doc.dom.set_device_pixel_ratio(2.0);
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    let child = text_at(&mut doc, hero, 10.0, 10.0, "Hi");

    let paint = only(&present(&mut painter, &mut doc));
    assert_eq!(paint.root, hero);
    assert_eq!(paint.last_painted, child);
    assert_eq!(bounds(&paint), (10.0, 10.0, 40.0, 20.0));
    assert!(paint.size > 0.0);
    assert!((paint.size - 800.0).abs() < 0.5, "{paint:?}");
    assert_eq!(paint.first_render, paint.latest);
}

#[test]
fn growth_reports_the_union_s_bounds_and_its_true_area() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    text_at(&mut doc, hero, 10.0, 10.0, "Hi");
    let first = only(&present(&mut painter, &mut doc));

    // 60×20 at (30, 20), overlapping the first 40×20 by 20×10.
    let second = text_at(&mut doc, hero, 30.0, 20.0, "Hey");
    let grown = only(&present(&mut painter, &mut doc));
    assert_eq!(grown.last_painted, second);
    assert_eq!(bounds(&grown), (10.0, 10.0, 80.0, 30.0));
    // The union's area, 800 + 1200 − 200, not its bounding box's 2400.
    assert!((grown.size - 1800.0).abs() < 0.5, "{grown:?}");
    assert_eq!(grown.first_render, first.first_render);
    assert!(grown.latest > first.latest);
}

#[test]
fn a_frame_that_paints_nothing_new_under_a_root_reports_nothing() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    let child = text_at(&mut doc, hero, 10.0, 10.0, "Hi");
    assert_eq!(present(&mut painter, &mut doc).len(), 1);

    // A new commit whose one change is a paint outside the root: the
    // child paints the same area again.
    text_at(&mut doc, root, 500.0, 500.0, "Out");
    assert!(present(&mut painter, &mut doc).is_empty());
    // Painted again in a new frame at the same place.
    doc.dom
        .set_inline_style(child, "left: 10px; top: 10px; color: red");
    assert!(present(&mut painter, &mut doc).is_empty());
}

#[test]
fn a_nested_root_reports_its_own_paints_and_its_ancestor_root_both() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    let outer = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(outer, true);
    let direct = text_at(&mut doc, outer, 10.0, 10.0, "Out");
    let inner = area(&mut doc, outer, 0.0, 100.0, (400.0, 100.0));
    doc.dom.set_container_timing(inner, true);
    let nested = text_at(&mut doc, inner, 10.0, 10.0, "In");

    let paints = present(&mut painter, &mut doc);
    assert_eq!(paints.len(), 2, "{paints:?}");
    let of = |root: NodeId| {
        only(
            &paints
                .iter()
                .copied()
                .filter(|paint| paint.root == root)
                .collect::<Vec<_>>(),
        )
    };
    let inner_paint = of(inner);
    assert_eq!(inner_paint.last_painted, nested);
    assert_eq!(bounds(&inner_paint), (10.0, 110.0, 40.0, 20.0));
    // The draft passes a nested root's paints on to the root around it.
    let outer_paint = of(outer);
    assert_eq!(outer_paint.last_painted, direct, "the larger new area");
    assert_eq!(bounds(&outer_paint), (10.0, 10.0, 60.0, 120.0));
    assert!((outer_paint.size - 2000.0).abs() < 0.5, "{outer_paint:?}");
}

#[test]
fn an_ignored_subtree_counts_toward_no_root_above_it() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    let aside = area(&mut doc, hero, 0.0, 100.0, (400.0, 100.0));
    doc.dom.set_container_timing_ignored(aside, true);
    text_at(&mut doc, aside, 0.0, 0.0, "Aside");
    assert!(present(&mut painter, &mut doc).is_empty());

    // A root that is also ignored keeps its paints to itself.
    doc.dom.set_container_timing(aside, true);
    text_at(&mut doc, aside, 0.0, 50.0, "Own");
    assert_eq!(only(&present(&mut painter, &mut doc)).root, aside);
}

#[test]
fn a_shadow_tree_paints_inside_its_host_s_root_retargeted_to_the_host() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    let host = area(&mut doc, hero, 0.0, 0.0, (400.0, 300.0));
    let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);
    shadow_text_at(&mut doc, shadow, 20.0, 30.0, "Card");

    let paint = only(&present(&mut painter, &mut doc));
    assert_eq!(paint.root, hero);
    assert_eq!(paint.last_painted, host, "retargeted into the root's tree");
    assert_eq!(bounds(&paint), (20.0, 30.0, 80.0, 20.0));
}

#[test]
fn stopping_or_forgetting_the_view_ends_the_recording() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    text_at(&mut doc, hero, 0.0, 0.0, "One");
    let first = only(&present(&mut painter, &mut doc));

    // A report the embedder has not taken goes with the view, and the same
    // paints are new area for the view shown next.
    let second = text_at(&mut doc, hero, 0.0, 100.0, "Two");
    let frame = doc.dom.commit();
    assert!(painter.adopt(Some(frame), Some(&NoImages)));
    let now = painter.now_seconds();
    assert!(painter.draw_offscreen(now, false).expect("drawn"));
    painter.forget_view();
    painter.forget_target();
    assert!(painter.take_container_paints().is_empty());
    doc.dom.set_inline_style(second, "left: 0px; top: 120px");
    let paint = only(&present(&mut painter, &mut doc));
    assert!(paint.first_render > first.first_render);
    assert_eq!(bounds(&paint), (0.0, 0.0, 60.0, 140.0));

    // Stopped: a later paint under the root reports nothing.
    doc.dom.stop_container_timing();
    text_at(&mut doc, hero, 0.0, 200.0, "Three");
    assert!(present(&mut painter, &mut doc).is_empty());
}
