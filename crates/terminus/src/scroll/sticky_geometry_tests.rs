//! Sticky geometry must be observable before the first paint, and immediately
//! after a scroll, while the durable normal-flow layout stays unchanged.

#![allow(clippy::float_cmp)]

use crate::test_common::Doc;
use crate::{NodeId, Vector2D};

const CSS: &str = "
    page { display:flex; width:800px; height:600px; }
    view { display:flex; flex-direction:column; flex-shrink:0; }
    .scroll { overflow:scroll; width:120px; height:100px; }
    .sticky { position:sticky; top:10px; width:40px; height:20px; }
    .lead { height:60px; }
    .tail { height:400px; }
";

fn top(doc: &Doc, id: NodeId) -> f32 {
    doc.dom
        .bounding_client_rect(id)
        .expect("laid-out box")
        .origin
        .y
}

#[test]
fn sticky_client_rect_samples_live_scroll_without_paint_or_relayout() {
    let mut doc = Doc::with_css(CSS);
    let scroll = doc.el(doc.root, "view.scroll");
    doc.el(scroll, "view.lead");
    let sticky = doc.el(scroll, "view.sticky");
    let tail = doc.el(scroll, "view.tail");
    doc.dom.layout();
    assert!(doc.dom.committed_frame().is_none());
    assert_eq!(top(&doc, sticky), 60.0);
    doc.dom.scroll_to(scroll, Vector2D::new(0.0, 30.0));
    assert_eq!(top(&doc, sticky), 30.0);
    doc.dom.scroll_to(scroll, Vector2D::new(0.0, 80.0));
    assert_eq!(top(&doc, sticky), 10.0);
    assert_eq!(top(&doc, tail), 0.0);
    assert_eq!(doc.dom.rounded_layout(sticky).unwrap().location.y, 60.0);
    assert_eq!(doc.dom.rounded_layout(tail).unwrap().location.y, 80.0);

    doc.set_inline(sticky, "top:30px");
    assert_eq!(top(&doc, sticky), 10.0, "the query does not flush style");
    doc.dom.layout();
    assert_eq!(top(&doc, sticky), 30.0);
}

#[test]
fn sticky_client_rect_stops_at_its_container_and_carries_positioned_descendants() {
    let mut doc = Doc::with_css(&format!(
        "{CSS}
        .group {{ height:160px; }}
        .sticky {{ top:0; }}
        .absolute {{ position:absolute; top:4px; width:5px; height:5px; }}
        .fixed {{ position:fixed; top:9px; width:5px; height:5px; }}"
    ));
    let scroll = doc.el(doc.root, "view.scroll");
    let group = doc.el(scroll, "view.group");
    let sticky = doc.el(group, "view.sticky");
    let absolute = doc.el(sticky, "view.absolute");
    let fixed = doc.el(sticky, "view.fixed");
    doc.el(scroll, "view.tail");
    doc.dom.layout();
    doc.dom.scroll_to(scroll, Vector2D::new(0.0, 200.0));
    assert_eq!(top(&doc, sticky), -60.0);
    assert_eq!(top(&doc, absolute), -56.0);
    assert_eq!(
        top(&doc, fixed),
        9.0,
        "viewport-fixed box escapes sticky displacement"
    );
}

#[test]
fn nested_sticky_client_rect_uses_ancestor_displacement_once() {
    let mut doc = Doc::with_css(&format!(
        "{CSS}
        .outer {{ position:sticky; top:0; height:160px; }}
        .sticky {{ top:20px; }}"
    ));
    let scroll = doc.el(doc.root, "view.scroll");
    let outer = doc.el(scroll, "view.outer");
    let inner = doc.el(outer, "view.sticky");
    doc.el(scroll, "view.tail");
    doc.dom.layout();
    doc.dom.scroll_to(scroll, Vector2D::new(0.0, 100.0));
    assert_eq!(top(&doc, outer), 0.0);
    assert_eq!(top(&doc, inner), 20.0);
}

#[test]
fn distributed_auto_margin_does_not_shorten_sticky_travel() {
    let mut doc = Doc::with_css(&format!(
        "{CSS}
        .group {{ height:400px; }}
        .sticky {{ top:0; margin-bottom:auto; }}
        .following {{ height:100px; }}"
    ));
    let scroll = doc.el(doc.root, "view.scroll");
    let group = doc.el(scroll, "view.group");
    let sticky = doc.el(group, "view.sticky");
    doc.el(group, "view.following");
    doc.el(scroll, "view.tail");
    doc.dom.layout();
    assert_eq!(doc.dom.rounded_layout(sticky).unwrap().margin.bottom, 280.0);
    doc.dom.scroll_to(scroll, Vector2D::new(0.0, 350.0));
    assert_eq!(top(&doc, sticky), 0.0);
    doc.dom.scroll_to(scroll, Vector2D::new(0.0, 400.0));
    assert_eq!(top(&doc, sticky), -20.0);
}
