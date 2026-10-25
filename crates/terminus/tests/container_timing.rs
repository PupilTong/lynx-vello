//! W3C Container Timing as the document and the presenting painter build it:
//! roots marked on the document (`Document::set_container_timing`,
//! `Document::set_container_timing_ignored`, `Document::stop_container_timing`),
//! the paints the walk attributes to them, and the reports the painter makes
//! after a present (`Painter::take_container_paints`) over an offscreen
//! target.

mod paint_common;

use paint_common::Doc;
use prime_radiant::TestImages;
use terminus::link::painter::{DrawTarget, GesturePolicy, Painter};
use terminus::link::{ContainerPaint, FontBlob, ImageRole, NoImages, NodeId, ShadowRootMode};

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
    pollster::block_on(Painter::new(
        DrawTarget::Offscreen,
        800.0,
        600.0,
        1.0,
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

#[test]
fn a_page_with_no_root_reports_nothing() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    text_at(&mut doc, root, 10.0, 10.0, "Hello");
    assert_eq!(doc.dom.container_timing_roots(), 0);
    let frame = doc.dom.commit();
    assert!(!frame.paints_under_containers());
    assert!(present(&mut painter, &mut doc).is_empty());
}

#[test]
fn a_root_reports_its_child_s_paint_once_and_then_each_growth() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    let first = text_at(&mut doc, hero, 10.0, 10.0, "Hi");

    let paints = present(&mut painter, &mut doc);
    let [paint] = paints[..] else {
        panic!("one report: {paints:?}");
    };
    assert_eq!(paint.root, hero);
    assert_eq!(paint.last_painted, first);
    assert_eq!(bounds(&paint), (10.0, 10.0, 40.0, 20.0));
    assert!((paint.size - 800.0).abs() < 0.5, "{paint:?}");
    assert_eq!(paint.first_render, paint.latest);
    assert!((paint.device_pixel_ratio - 1.0).abs() < f32::EPSILON);

    // A paint outside the union grows it.
    let second = text_at(&mut doc, hero, 100.0, 200.0, "Hey");
    let paints = present(&mut painter, &mut doc);
    let [grown] = paints[..] else {
        panic!("one report: {paints:?}");
    };
    assert_eq!(grown.root, hero);
    assert_eq!(grown.last_painted, second);
    assert_eq!(bounds(&grown), (10.0, 10.0, 150.0, 210.0));
    assert!((grown.size - 2000.0).abs() < 0.5, "{grown:?}");
    assert_eq!(grown.first_render, paint.first_render);
    assert!(grown.latest > paint.latest);
    assert!(grown.commit_id > paint.commit_id);

    // A frame that paints nothing new under the root reports nothing.
    let elsewhere = doc.el(root, "abs bg");
    doc.dom.set_inline_style(
        elsewhere,
        "left: 500px; top: 500px; width: 10px; height: 10px",
    );
    assert!(present(&mut painter, &mut doc).is_empty());
}

#[test]
fn a_background_colour_and_white_space_are_not_content() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    let filled = area(&mut doc, hero, 10.0, 10.0, (50.0, 50.0));
    doc.dom.add_class(filled, "bg");
    text_at(&mut doc, hero, 100.0, 100.0, "   ");
    assert!(present(&mut painter, &mut doc).is_empty());
}

#[test]
fn loose_text_paints_under_its_container_s_root() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    let holder = area(&mut doc, hero, 10.0, 10.0, (200.0, 100.0));
    doc.dom.add_class(holder, "text");
    doc.text(holder, "Loose");
    let paints = present(&mut painter, &mut doc);
    let [paint] = paints[..] else {
        panic!("one report: {paints:?}");
    };
    assert_eq!(paint.root, hero);
    assert_eq!(paint.last_painted, holder, "the anonymous box's container");
}

#[test]
fn an_image_with_pixels_paints_under_its_root_and_one_without_does_not() {
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    let image = doc.el_tag(hero, "img", "abs");
    doc.dom
        .set_inline_style(image, "left: 10px; top: 10px; width: 50px; height: 50px");
    doc.dom
        .set_image_source(image, ImageRole::Source, Some("app:///photo.png"));
    let images = TestImages::new();
    prime_radiant::render_with_images(&mut doc.dom, &images);
    assert!(
        !doc.dom
            .committed_frame()
            .expect("committed")
            .paints_under_containers(),
        "no pixels, no paint"
    );
    images.insert_rgba8("app:///photo.png", 2, 2, vec![255; 16]);
    doc.dom
        .set_inline_style(image, "left: 20px; top: 10px; width: 50px; height: 50px");
    prime_radiant::render_with_images(&mut doc.dom, &images);
    assert!(
        doc.dom
            .committed_frame()
            .expect("committed")
            .paints_under_containers()
    );
}

#[test]
fn a_paint_is_cut_by_its_clip_and_the_viewport() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.add_class(hero, "clip");
    doc.dom.set_container_timing(hero, true);
    // 100 px wide, its right half past the clip.
    let wide = text_at(&mut doc, hero, 350.0, 10.0, "Clip!");
    doc.dom
        .set_inline_style(wide, "left: 350px; top: 10px; width: 100px; height: 20px");
    let paints = present(&mut painter, &mut doc);
    let [paint] = paints[..] else {
        panic!("one report: {paints:?}");
    };
    assert_eq!(bounds(&paint), (350.0, 10.0, 50.0, 20.0));

    // Past the viewport's bottom edge.
    let low = area(&mut doc, root, 0.0, 590.0, (800.0, 100.0));
    doc.dom.set_container_timing(low, true);
    text_at(&mut doc, low, 0.0, 0.0, "Low");
    let paints = present(&mut painter, &mut doc);
    let [paint] = paints[..] else {
        panic!("one report: {paints:?}");
    };
    assert_eq!(paint.root, low);
    assert_eq!(bounds(&paint), (0.0, 590.0, 60.0, 10.0));
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
    let of = |root: NodeId| -> ContainerPaint {
        let found: Vec<_> = paints.iter().filter(|paint| paint.root == root).collect();
        let [paint] = found[..] else {
            panic!("one report for {root:?}: {paints:?}");
        };
        *paint
    };
    assert_eq!(paints.len(), 2, "{paints:?}");
    let inner_paint = of(inner);
    assert_eq!(inner_paint.last_painted, nested);
    assert_eq!(bounds(&inner_paint), (10.0, 110.0, 40.0, 20.0));
    // The draft passes a nested root's paints on to the root around it.
    let outer_paint = of(outer);
    assert_eq!(outer_paint.last_painted, direct, "the larger new area");
    assert_eq!(bounds(&outer_paint), (10.0, 10.0, 60.0, 120.0));
    assert!((outer_paint.size - 2000.0).abs() < 0.5, "{outer_paint:?}");

    // A nested root that is also ignored keeps its paints to itself.
    doc.dom.set_container_timing_ignored(inner, true);
    text_at(&mut doc, inner, 200.0, 10.0, "Only");
    let paints = present(&mut painter, &mut doc);
    let [paint] = paints[..] else {
        panic!("one report: {paints:?}");
    };
    assert_eq!(paint.root, inner);
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

    let counted = text_at(&mut doc, hero, 0.0, 0.0, "Main");
    let paints = present(&mut painter, &mut doc);
    let [paint] = paints[..] else {
        panic!("one report: {paints:?}");
    };
    assert_eq!(paint.last_painted, counted);
    assert_eq!(bounds(&paint), (0.0, 0.0, 80.0, 20.0));
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

    let paints = present(&mut painter, &mut doc);
    let [paint] = paints[..] else {
        panic!("one report: {paints:?}");
    };
    assert_eq!(paint.root, hero);
    assert_eq!(paint.last_painted, host, "retargeted into the root's tree");
    assert_eq!(bounds(&paint), (20.0, 30.0, 80.0, 20.0));

    // A root inside the shadow tree is a root too, and its paints are its
    // own tree's.
    let card = doc.dom.create_element("view", ());
    doc.dom.set_inline_style(
        card,
        "display: flex; position: absolute; left: 0px; top: 100px; width: 400px; height: 100px",
    );
    doc.dom.set_container_timing(card, true);
    doc.dom.append_child(shadow, card);
    let line = shadow_text_at(&mut doc, card, 0.0, 0.0, "Line");
    let paints = present(&mut painter, &mut doc);
    let card_paint = paints
        .iter()
        .find(|paint| paint.root == card)
        .unwrap_or_else(|| panic!("the shadow tree's root reports: {paints:?}"));
    assert_eq!(card_paint.last_painted, line);
    let hero_paint = paints
        .iter()
        .find(|paint| paint.root == hero)
        .unwrap_or_else(|| panic!("the host's root reports the growth: {paints:?}"));
    assert_eq!(hero_paint.last_painted, host);
}

#[test]
fn stopping_ends_every_root_and_a_root_marked_again_starts_afresh() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    text_at(&mut doc, hero, 0.0, 0.0, "One");
    let first = present(&mut painter, &mut doc);
    assert_eq!(first.len(), 1);

    doc.dom.stop_container_timing();
    assert_eq!(doc.dom.container_timing_roots(), 0);
    text_at(&mut doc, hero, 0.0, 100.0, "Two");
    assert!(present(&mut painter, &mut doc).is_empty());

    // Marked again: a new record, with its own first render and the whole
    // subtree's paints as new area.
    doc.dom.set_container_timing(hero, true);
    text_at(&mut doc, hero, 0.0, 200.0, "Three");
    let paints = present(&mut painter, &mut doc);
    let [paint] = paints[..] else {
        panic!("one report: {paints:?}");
    };
    assert!(paint.first_render > first[0].first_render);
    assert_eq!(bounds(&paint), (0.0, 0.0, 100.0, 220.0));
}

#[test]
fn a_freed_root_is_no_longer_counted() {
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    assert_eq!(doc.dom.container_timing_roots(), 1);
    doc.dom.drop_subtree(hero);
    assert_eq!(doc.dom.container_timing_roots(), 0);
}

#[test]
fn forgetting_the_view_clears_the_regions_and_the_reports() {
    let mut painter = offscreen_painter();
    let mut doc = page();
    let root = doc.root;
    let hero = area(&mut doc, root, 0.0, 0.0, (400.0, 300.0));
    doc.dom.set_container_timing(hero, true);
    text_at(&mut doc, hero, 0.0, 0.0, "One");
    let first = present(&mut painter, &mut doc);
    assert_eq!(first.len(), 1);

    // A report the embedder has not taken goes with the view.
    let second = text_at(&mut doc, hero, 0.0, 100.0, "Two");
    let frame = doc.dom.commit();
    assert!(painter.adopt(Some(frame), Some(&NoImages)));
    let now = painter.now_seconds();
    assert!(painter.draw_offscreen(now, false).expect("drawn"));
    painter.forget_view();
    painter.forget_target();
    assert!(painter.take_container_paints().is_empty());

    // The same paints are new area for the view shown next.
    doc.dom.set_inline_style(second, "left: 0px; top: 120px");
    let paints = present(&mut painter, &mut doc);
    let [paint] = paints[..] else {
        panic!("one report: {paints:?}");
    };
    assert!(paint.first_render > first[0].first_render);
    assert_eq!(bounds(&paint), (0.0, 0.0, 60.0, 140.0));
}
