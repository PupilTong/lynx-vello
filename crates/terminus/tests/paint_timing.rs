//! Rendering timing: the document's commit timing
//! (`Document::set_commit_timing`, `Document::last_commit_timing`), Paint
//! Timing's contentful test on a committed frame
//! (`CommittedFrame::paints_content`, behind
//! `Document::set_content_detection`), and the presenting painter's
//! presentation records (`Painter::last_presented`, `Painter::first_paint`,
//! `Painter::first_contentful_paint`, behind
//! `Painter::set_presentation_timing`) over an offscreen target.

mod paint_common;

use paint_common::Doc;
use prime_radiant::TestImages;
use terminus::link::painter::{DrawTarget, GesturePolicy, Painter};
use terminus::link::{CommitTiming, FontBlob, ImageRole, NoImages, NodeId};

const AHEM: &[u8] = include_bytes!("../../seldon/tests/fixtures/Ahem.ttf");

const PAGE: &str = "page { display: flex; position: relative; width: 800px; height: 600px; }
    .box { display: flex; position: absolute; left: 10px; top: 10px;
           width: 200px; height: 100px; }
    .bg { background-color: teal; }
    .text { font-family: Ahem; font-size: 20px; color: black; }
    .clear { opacity: 0; }
    .far { left: 5000px; top: 5000px; }";

const ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="6">
  <rect width="12" height="6" fill="red"/>
</svg>"#;

/// A page styled by `css` after [`PAGE`], where a `text` element is a Lynx
/// text element and Ahem is the one face, with content detection on.
fn page(css: &str) -> Doc {
    let mut doc = undetected_page(css);
    doc.dom.set_content_detection(true);
    doc
}

/// [`page`] with content detection as a document starts: off.
fn undetected_page(css: &str) -> Doc {
    let mut doc = Doc::with_css(&format!("{PAGE} {css}"));
    doc.define_lynx_text("text");
    doc.dom.register_fonts(FontBlob::from_static(AHEM));
    doc
}

/// A `text` element under the root holding `content`.
fn text(doc: &mut Doc, class: &str, content: &str) -> NodeId {
    let root = doc.root;
    let holder = doc.el_tag(root, "text", &format!("box text {class}"));
    doc.text(holder, content);
    holder
}

fn paints_content(doc: &mut Doc) -> bool {
    doc.dom.commit().paints_content()
}

/// One 2×2 opaque bitmap at `source`.
fn bitmap(images: &TestImages, source: &str) {
    images.insert_rgba8(source, 2, 2, vec![255; 16]);
}

fn assert_ordered(timing: CommitTiming) {
    assert!(timing.style_start <= timing.style_end, "{timing:?}");
    assert!(timing.style_end <= timing.layout_start, "{timing:?}");
    assert!(timing.layout_start <= timing.layout_end, "{timing:?}");
    assert!(timing.layout_end <= timing.frame_build_start, "{timing:?}");
    assert!(
        timing.frame_build_start <= timing.frame_build_end,
        "{timing:?}"
    );
}

#[test]
fn commit_timing_is_off_by_default() {
    let mut doc = page("");
    doc.el(doc.root, "box bg");
    assert!(doc.dom.render());
    assert!(doc.dom.last_commit_timing().is_none());
}

#[test]
fn a_timed_render_records_its_phases_in_order() {
    let mut doc = page("");
    doc.dom.set_commit_timing(true);
    doc.el(doc.root, "box bg");
    assert!(doc.dom.render());
    let timing = doc
        .dom
        .last_commit_timing()
        .expect("a timed render that built a frame records its timing");
    assert_ordered(timing);
    let frame = doc.dom.committed_frame().expect("the render committed");
    assert_eq!(timing.commit_id, frame.commit_id());
}

#[test]
fn a_render_with_nothing_stale_keeps_the_last_timing() {
    let mut doc = page("");
    doc.dom.set_commit_timing(true);
    doc.el(doc.root, "box bg");
    assert!(doc.dom.render());
    let first = doc.dom.last_commit_timing().expect("timed");
    assert!(!doc.dom.render(), "nothing is stale");
    let second = doc.dom.last_commit_timing().expect("kept");
    assert_eq!(second.commit_id, first.commit_id);
    assert_eq!(second.style_start, first.style_start);
    assert_eq!(second.frame_build_end, first.frame_build_end);
}

#[test]
fn a_render_after_timing_is_turned_off_keeps_the_last_record() {
    let mut doc = page("");
    doc.dom.set_commit_timing(true);
    let node = doc.el(doc.root, "box bg");
    assert!(doc.dom.render());
    let timed = doc.dom.last_commit_timing().expect("timed");
    doc.dom.set_commit_timing(false);
    doc.dom.remove_class(node, "bg");
    assert!(doc.dom.render());
    let kept = doc.dom.last_commit_timing().expect("kept");
    assert_eq!(kept.commit_id, timed.commit_id);
    let frame = doc.dom.committed_frame().expect("committed");
    assert!(
        frame.commit_id() > kept.commit_id,
        "the untimed render published a later commit than the record names"
    );
}

#[test]
fn content_detection_is_off_by_default() {
    let mut doc = undetected_page("");
    text(&mut doc, "", "Hello");
    assert!(
        !paints_content(&mut doc),
        "a frame built with detection off is never contentful"
    );

    // Turned on, the next frame built is decided; turned off again, the
    // next is not.
    doc.dom.set_content_detection(true);
    text(&mut doc, "", "World");
    assert!(paints_content(&mut doc));
    doc.dom.set_content_detection(false);
    text(&mut doc, "", "Again");
    assert!(!paints_content(&mut doc));
}

#[test]
fn an_empty_document_paints_no_content() {
    let mut doc = page("");
    assert!(!paints_content(&mut doc));
}

#[test]
fn a_background_color_is_not_content() {
    let mut doc = page(".border { border: 4px solid black; box-shadow: 0 0 8px red; }");
    doc.el(doc.root, "box bg border");
    assert!(!paints_content(&mut doc));
}

#[test]
fn text_is_content() {
    let mut doc = page("");
    text(&mut doc, "", "Hello");
    assert!(paints_content(&mut doc));
}

#[test]
fn white_space_only_text_is_not_content() {
    let mut doc = page("");
    text(&mut doc, "", "   ");
    assert!(!paints_content(&mut doc));

    // Spaces that survive collapsing are shaped, and are still no ink.
    let mut doc = page(".pre { white-space: pre; }");
    text(&mut doc, "pre", "  \n  ");
    assert!(!paints_content(&mut doc));
    let mut doc = page("");
    text(&mut doc, "", "\u{a0}\u{a0}");
    assert!(!paints_content(&mut doc));
}

#[test]
fn text_at_zero_opacity_is_not_content() {
    let mut doc = page("");
    text(&mut doc, "clear", "Hello");
    assert!(!paints_content(&mut doc));
}

#[test]
fn text_nested_in_a_zero_opacity_group_is_not_content_and_text_after_it_is() {
    let mut doc = page(".half { opacity: 0.5; }");
    let root = doc.root;
    let clear = doc.el(root, "box clear");
    let half = doc.el(clear, "half");
    let holder = doc.el_tag(half, "text", "text");
    doc.text(holder, "Hidden");
    assert!(!paints_content(&mut doc));

    text(&mut doc, "", "Shown");
    assert!(paints_content(&mut doc));
}

#[test]
fn an_image_with_pixels_is_content() {
    let mut doc = page("");
    let images = TestImages::new();
    bitmap(&images, "app:///photo.png");
    let root = doc.root;
    let image = doc.el_tag(root, "img", "box");
    doc.dom
        .set_image_source(image, ImageRole::Source, Some("app:///photo.png"));
    prime_radiant::render_with_images(&mut doc.dom, &images);
    assert!(
        doc.dom
            .committed_frame()
            .expect("committed")
            .paints_content()
    );
}

#[test]
fn an_image_without_pixels_is_not_content() {
    let mut doc = page("");
    let root = doc.root;
    let image = doc.el_tag(root, "img", "box");
    doc.dom
        .set_image_source(image, ImageRole::Source, Some("app:///missing.png"));
    assert!(!paints_content(&mut doc));
}

#[test]
fn a_background_image_with_pixels_is_content() {
    let mut doc = page(".photo { background-image: url(app:///tile.png); }");
    let images = TestImages::new();
    bitmap(&images, "app:///tile.png");
    doc.el(doc.root, "box photo");
    prime_radiant::render_with_images(&mut doc.dom, &images);
    assert!(
        doc.dom
            .committed_frame()
            .expect("committed")
            .paints_content()
    );
}

#[test]
fn an_svg_image_is_content() {
    let mut doc = page("");
    let images = TestImages::new();
    images.insert_svg("app:///icon.svg", ICON);
    let root = doc.root;
    let image = doc.el_tag(root, "img", "box");
    doc.dom
        .set_image_source(image, ImageRole::Source, Some("app:///icon.svg"));
    prime_radiant::render_with_images(&mut doc.dom, &images);
    assert!(
        doc.dom
            .committed_frame()
            .expect("committed")
            .paints_content()
    );
}

#[test]
fn a_mask_image_is_not_content() {
    let mut doc = page(".masked { mask-image: url(app:///mask.png); }");
    let images = TestImages::new();
    bitmap(&images, "app:///mask.png");
    doc.el(doc.root, "box bg masked");
    prime_radiant::render_with_images(&mut doc.dom, &images);
    assert!(
        !doc.dom
            .committed_frame()
            .expect("committed")
            .paints_content()
    );
}

#[test]
fn an_image_outside_the_viewport_is_not_content() {
    let mut doc = page("");
    let images = TestImages::new();
    bitmap(&images, "app:///photo.png");
    let root = doc.root;
    let image = doc.el_tag(root, "img", "box far");
    doc.dom
        .set_image_source(image, ImageRole::Source, Some("app:///photo.png"));
    prime_radiant::render_with_images(&mut doc.dom, &images);
    assert!(
        !doc.dom
            .committed_frame()
            .expect("committed")
            .paints_content()
    );
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

/// Adopts the document's newest frame and draws it offscreen, answering the
/// frame's commit id.
fn present(painter: &mut Painter, doc: &mut Doc) -> u64 {
    let frame = doc.dom.commit();
    let commit = frame.commit_id();
    assert!(painter.adopt(Some(frame), Some(&NoImages)));
    let now = painter.now_seconds();
    assert!(
        painter
            .draw_offscreen(now, false)
            .expect("the draw succeeds"),
        "a newly adopted frame renders"
    );
    commit
}

#[test]
fn an_offscreen_draw_records_the_presented_frame_and_first_paint() {
    let mut painter = offscreen_painter();
    assert!(painter.last_presented().is_none());
    assert!(painter.first_paint().is_none());
    let mut doc = page("");
    doc.el(doc.root, "box bg");
    let commit = present(&mut painter, &mut doc);

    let presented = painter.last_presented().expect("the draw presented");
    assert_eq!(presented.commit_id, commit);
    assert!(presented.compose_start <= presented.compose_end);
    assert!(presented.compose_end <= presented.presented);
    let first = painter.first_paint().expect("the first presented frame");
    assert_eq!(first.commit_id, commit);
    assert!(
        painter.first_contentful_paint().is_none(),
        "a background colour is no contentful paint"
    );
}

#[test]
fn first_contentful_paint_waits_for_content_and_first_paint_holds() {
    let mut painter = offscreen_painter();
    let mut doc = page("");
    doc.el(doc.root, "box bg");
    let blank = present(&mut painter, &mut doc);
    assert!(painter.first_contentful_paint().is_none());

    text(&mut doc, "", "Hello");
    let contentful = present(&mut painter, &mut doc);
    assert!(contentful > blank);
    let fcp = painter
        .first_contentful_paint()
        .expect("the first frame with text is contentful");
    assert_eq!(fcp.commit_id, contentful);
    assert_eq!(painter.first_paint().expect("held").commit_id, blank);
    assert_eq!(
        painter.last_presented().expect("drawn").commit_id,
        contentful
    );

    // A third frame moves neither first mark.
    text(&mut doc, "", "World");
    let later = present(&mut painter, &mut doc);
    assert_eq!(painter.first_paint().expect("held").commit_id, blank);
    assert_eq!(
        painter.first_contentful_paint().expect("held").commit_id,
        contentful
    );
    assert_eq!(painter.last_presented().expect("drawn").commit_id, later);
}

#[test]
fn a_draw_of_the_frame_already_held_records_nothing_new() {
    let mut painter = offscreen_painter();
    let mut doc = page("");
    text(&mut doc, "", "Hello");
    present(&mut painter, &mut doc);
    let presented = painter.last_presented().expect("drawn");
    let now = painter.now_seconds();
    assert!(
        !painter.draw_offscreen(now, false).expect("no error"),
        "the target already holds the frame"
    );
    let again = painter.last_presented().expect("kept");
    assert_eq!(again.presented, presented.presented);
}

#[test]
fn without_content_detection_first_contentful_paint_never_comes() {
    let mut painter = offscreen_painter();
    let mut doc = undetected_page("");
    text(&mut doc, "", "Hello");
    let commit = present(&mut painter, &mut doc);
    assert_eq!(
        painter
            .first_paint()
            .expect("first paint needs no detection")
            .commit_id,
        commit
    );
    assert!(painter.first_contentful_paint().is_none());
}

#[test]
fn presentation_timing_off_records_nothing_and_on_again_records() {
    let mut painter = offscreen_painter();
    assert!(painter.presentation_timing(), "on by default");
    painter.set_presentation_timing(false);
    let mut doc = page("");
    text(&mut doc, "", "Hello");
    present(&mut painter, &mut doc);
    assert!(painter.last_presented().is_none());
    assert!(painter.first_paint().is_none());
    assert!(painter.first_contentful_paint().is_none());

    // The switch outlives the view it was set for.
    painter.forget_view();
    assert!(!painter.presentation_timing());

    painter.set_presentation_timing(true);
    text(&mut doc, "", "World");
    let commit = present(&mut painter, &mut doc);
    let presented = painter.last_presented().expect("recorded once on");
    assert_eq!(presented.commit_id, commit);
    assert_eq!(
        painter.first_paint().expect("the first recorded").commit_id,
        commit
    );
    assert_eq!(
        painter
            .first_contentful_paint()
            .expect("the first recorded with content")
            .commit_id,
        commit
    );
}

#[test]
fn forgetting_the_view_resets_the_presentation_records() {
    let mut painter = offscreen_painter();
    let mut doc = page("");
    text(&mut doc, "", "Hello");
    present(&mut painter, &mut doc);
    assert!(painter.first_contentful_paint().is_some());
    painter.forget_view();
    painter.forget_target();
    assert!(painter.last_presented().is_none());
    assert!(painter.first_paint().is_none());
    assert!(painter.first_contentful_paint().is_none());

    let mut next = page("");
    next.el(next.root, "box bg");
    let commit = present(&mut painter, &mut next);
    assert_eq!(
        painter.first_paint().expect("the new view's").commit_id,
        commit
    );
    assert!(painter.first_contentful_paint().is_none());
}
