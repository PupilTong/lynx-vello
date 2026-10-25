//! What a memory monitor reads of a document and of the frame it commits:
//! the element arena's footprint (`Document::memory_footprint`) and the
//! committed frame's encoded scene bytes (`CommittedFrame::encoded_bytes`,
//! `CommittedFrame::fragment_count`).

mod paint_common;

use paint_common::Doc;

const PAGE: &str = "page { display: flex; position: relative; width: 800px; height: 600px; }
    .box { display: flex; width: 200px; height: 100px; }
    .bg { background-color: teal; }";

#[test]
fn the_footprint_counts_live_elements_and_not_text() {
    let mut doc = Doc::with_css(PAGE);
    let start = doc.dom.memory_footprint();
    assert!(start.live_elements >= 1, "the page root is an element");
    assert!(start.node_stride_bytes > 0);
    assert_eq!(
        start.element_bytes(),
        start.live_elements * start.node_stride_bytes
    );

    let root = doc.root;
    let first = doc.el(root, "box");
    doc.el(root, "box");
    doc.el(first, "box");
    doc.text(first, "not an element");
    let grown = doc.dom.memory_footprint();
    assert_eq!(grown.live_elements, start.live_elements + 3);
    assert_eq!(grown.node_stride_bytes, start.node_stride_bytes);
    assert!(grown.slot_bound >= grown.live_elements);

    // A removal frees nothing; a drop does.
    let last = doc.el(root, "box");
    doc.dom.remove_element(last);
    assert_eq!(
        doc.dom.memory_footprint().live_elements,
        start.live_elements + 4
    );
    doc.dom.drop_element(last);
    let dropped = doc.dom.memory_footprint();
    assert_eq!(dropped.live_elements, start.live_elements + 3);
    assert_eq!(
        dropped.slot_bound,
        grown.slot_bound + 1,
        "the slot stays in the bound"
    );
}

#[test]
fn a_frame_that_paints_a_box_has_encoded_bytes() {
    let mut doc = Doc::with_css(PAGE);
    doc.el(doc.root, "box bg");
    let frame = doc.dom.commit();
    assert!(frame.fragment_count() >= 1);
    assert!(frame.encoded_bytes() > 0, "the box's fill is encoded");
}

#[test]
fn a_frame_that_paints_nothing_has_no_encoded_bytes() {
    let mut doc = Doc::with_css(PAGE);
    doc.el(doc.root, "box");
    let frame = doc.dom.commit();
    assert_eq!(frame.encoded_bytes(), 0);
}
