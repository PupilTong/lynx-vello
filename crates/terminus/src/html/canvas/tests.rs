#![cfg_attr(coverage_nightly, coverage(off))]

use super::{
    CANVAS_TAG, CanvasFrames, Committed, ContextMode, InvalidState, context_mode,
    parse_non_negative_integer, transfer_control_to_offscreen, update_placeholders,
};
use crate::layout::natural_size;
use crate::render::canvas::Surface;
use crate::render::picture::PictureKey;
use crate::test_common::Doc;
use crate::tree::document::NodeId;

/// A document with HTML's elements defined and one canvas in it.
fn canvas() -> (Doc, NodeId) {
    let mut doc = Doc::new();
    doc.dom.define_html_elements();
    let canvas = doc.el(doc.root, CANVAS_TAG);
    (doc, canvas)
}

/// Commits a transparent `width` × `height` surface to `frames`.
fn commit(frames: &CanvasFrames, width: u32, height: u32) -> Committed {
    Surface::new(width, height)
        .expect("a side the device takes")
        .commit(frames)
}

fn natural(doc: &Doc, canvas: NodeId) -> crate::layout::NaturalSize {
    doc.dom.natural_size(canvas)
}

#[test]
fn width_and_height_parse_as_non_negative_integers() {
    assert_eq!(parse_non_negative_integer("640"), Some(640));
    assert_eq!(parse_non_negative_integer(" \t\n+12px"), Some(12));
    assert_eq!(parse_non_negative_integer("007.5"), Some(7));
    assert_eq!(parse_non_negative_integer("-0"), Some(0));
    assert_eq!(parse_non_negative_integer("-1"), None);
    assert_eq!(parse_non_negative_integer(""), None);
    assert_eq!(parse_non_negative_integer("+"), None);
    assert_eq!(parse_non_negative_integer("px"), None);
    assert_eq!(parse_non_negative_integer("99999999999"), None);
}

/// A new canvas is replaced, presents a picture of its own at 300 × 150,
/// and in mode none follows its attributes, falling back to the default
/// for one that does not parse.
#[test]
fn a_canvas_in_mode_none_takes_its_natural_size_from_its_attributes() {
    let (mut doc, canvas) = canvas();
    assert_eq!(context_mode(&doc.dom, canvas), Some(ContextMode::None));
    assert!(matches!(doc.dom.picture(canvas), Some(PictureKey::Id(_))));
    assert_eq!(natural(&doc, canvas), natural_size(300, 150));

    doc.set_attr(canvas, "width", "64");
    doc.set_attr(canvas, "height", "32");
    assert_eq!(natural(&doc, canvas), natural_size(64, 32));
    doc.set_attr(canvas, "width", "-4");
    assert_eq!(natural(&doc, canvas), natural_size(300, 32));
    doc.remove_attr(canvas, "height");
    assert_eq!(natural(&doc, canvas), natural_size(300, 150));
}

/// `transferControlToOffscreen()` succeeds once, with the attributes' size,
/// and then throws; a node that is no canvas throws too.
#[test]
fn transfer_control_to_offscreen_succeeds_once() {
    let (mut doc, canvas) = canvas();
    doc.set_attr(canvas, "width", "20");
    let placeholder =
        transfer_control_to_offscreen(&mut doc.dom, canvas).expect("a canvas in mode none");
    assert_eq!((placeholder.width, placeholder.height), (20, 150));
    assert_eq!(
        doc.dom.picture(canvas).and_then(PictureKey::id),
        Some(placeholder.frames.picture())
    );
    assert_eq!(
        context_mode(&doc.dom, canvas),
        Some(ContextMode::Placeholder)
    );
    assert_eq!(
        transfer_control_to_offscreen(&mut doc.dom, canvas).map(|_| ()),
        Err(InvalidState)
    );
    let div = doc.el(doc.root, "div");
    assert_eq!(
        transfer_control_to_offscreen(&mut doc.dom, div).map(|_| ()),
        Err(InvalidState)
    );
}

/// A placeholder ignores its attributes; its natural size follows a bitmap
/// of a new size once the document applies it, and one resize is owed
/// however many commits precede it.
#[test]
fn a_placeholders_natural_size_follows_its_committed_bitmaps() {
    let (mut doc, canvas) = canvas();
    let placeholder = transfer_control_to_offscreen(&mut doc.dom, canvas).expect("mode none");
    let frames = placeholder.frames;
    assert!(
        frames.describe::<()>().is_none(),
        "nothing committed draws nothing"
    );

    doc.set_attr(canvas, "width", "10");
    assert_eq!(natural(&doc, canvas), natural_size(300, 150), "do nothing");

    let first = commit(&frames, 300, 150);
    assert!(first.wake, "the first commit wakes the host");
    assert!(!first.resize, "the size it was transferred at");

    let resized = commit(&frames, 40, 30);
    assert!(!resized.wake, "the first commit's wake is still owed");
    assert!(resized.resize);
    assert!(!commit(&frames, 50, 30).resize, "already owed");
    assert_eq!(
        natural(&doc, canvas),
        natural_size(300, 150),
        "not applied yet"
    );

    update_placeholders(&mut doc.dom);
    assert_eq!(
        natural(&doc, canvas),
        natural_size(50, 30),
        "the newest size"
    );
    let shown = frames.describe::<()>().expect("committed");
    assert_eq!((shown.descriptor.width, shown.descriptor.height), (50, 30));
    assert_eq!(shown.generation, 3, "one generation per commit");
    let again = commit(&frames, 60, 30);
    assert!(again.resize, "owed again once applied");
    assert!(again.wake, "described since the last wake");
}

/// Once the element is freed its commits go nowhere and owe nothing.
#[test]
fn a_freed_canvas_takes_no_commits() {
    let (mut doc, canvas) = canvas();
    let frames = transfer_control_to_offscreen(&mut doc.dom, canvas)
        .expect("mode none")
        .frames;
    doc.dom.drop_element(canvas);
    assert!(!frames.is_live());
    assert_eq!(commit(&frames, 4, 4), Committed::default());
    assert!(frames.describe::<()>().is_none());
    update_placeholders(&mut doc.dom);
}
