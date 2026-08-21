//! Behavior tests for scrolling: how a scroll offset reaches the painted
//! frame, what escapes it, and how host input drives it end to end.

#![allow(clippy::float_cmp)]

use euclid::default::Vector2D;
use stylo::queries::values::PrefersColorScheme;

use crate::input::{InputEvent, PointerKind, PointerPhase};
use crate::scroll::ScrollAxes;
use crate::test_common::{Doc, device_with};
use crate::visual::{PaintItemKind, PaintOrder};
use crate::{NodeId, Point2D};

struct Harness {
    doc: Doc,
}

impl Harness {
    fn new(css: &str) -> Self {
        Self {
            doc: Doc::with_css(css),
        }
    }

    fn el(&mut self, parent: NodeId, spec: &str) -> NodeId {
        self.doc.el(parent, spec)
    }

    fn root(&self) -> NodeId {
        self.doc.root
    }

    fn paint(&mut self) -> PaintOrder {
        self.doc.dom.build_paint_order()
    }

    fn origin(&mut self, id: NodeId) -> Point2D<f32> {
        let frame = self.paint();
        let item = frame
            .items()
            .iter()
            .find(|item| item.node == id && item.kind == PaintItemKind::ElementBox)
            .expect("node paints an element box");
        item.transform
            .transform_point2d(Point2D::zero())
            .expect("a paintable item has a non-singular matrix")
    }

    fn hit(&mut self, x: f32, y: f32) -> Option<NodeId> {
        self.doc.dom.render();
        self.doc
            .dom
            .elements_from_point(Point2D::new(x, y))
            .first()
            .copied()
    }

    fn route(&mut self, event: InputEvent) -> Option<NodeId> {
        self.doc.dom.render();
        self.doc.dom.route_input(event)
    }

    fn scroll_to(&mut self, id: NodeId, x: f32, y: f32) {
        self.doc.dom.layout();
        self.doc.dom.scroll_to(id, Vector2D::new(x, y));
    }
}

const SCROLLER: &str = "page { display: flex; width: 800px; height: 600px; }
     .scroller { display: flex; flex-direction: column; overflow: scroll;
                 width: 100px; height: 100px; }
     .row { flex-shrink: 0; width: 100px; height: 100px; }";

#[test]
fn scrolling_moves_contents_and_leaves_the_scroller_itself_alone() {
    let mut h = Harness::new(SCROLLER);
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    let rows: Vec<NodeId> = (0..4).map(|_| h.el(scroller, "view.row")).collect();

    assert_eq!(h.origin(scroller), Point2D::new(0.0, 0.0));
    assert_eq!(h.origin(rows[2]), Point2D::new(0.0, 200.0));

    h.scroll_to(scroller, 0.0, 150.0);

    assert_eq!(
        h.origin(scroller),
        Point2D::new(0.0, 0.0),
        "the scroll container's own box does not move",
    );
    assert_eq!(h.origin(rows[2]), Point2D::new(0.0, 50.0));
}

#[test]
fn hit_testing_follows_the_scrolled_content_through_the_unmoved_clip() {
    let mut h = Harness::new(SCROLLER);
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    let rows: Vec<NodeId> = (0..4).map(|_| h.el(scroller, "view.row")).collect();

    assert_eq!(h.hit(50.0, 50.0), Some(rows[0]));
    assert_eq!(h.hit(50.0, 150.0), Some(root));

    h.scroll_to(scroller, 0.0, 150.0);

    assert_eq!(h.hit(50.0, 40.0), Some(rows[1]));
    assert_eq!(h.hit(50.0, 50.0), Some(rows[2]));
    assert_eq!(
        h.hit(50.0, 150.0),
        Some(root),
        "the clip stayed where the scroller is, not where its contents went",
    );
}

#[test]
fn a_positioned_box_scrolls_only_with_its_own_containing_block() {
    let css = format!(
        "{SCROLLER}
         page {{ position: relative; }}
         .pinned {{ display: flex; position: absolute; left: 0; top: 40px;
                    width: 20px; height: 20px; }}
         .anchored {{ position: relative; }}"
    );

    let mut h = Harness::new(&css);
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    h.el(scroller, "view.row");
    h.el(scroller, "view.row");
    let pinned = h.el(scroller, "view.pinned");

    assert_eq!(h.origin(pinned), Point2D::new(0.0, 40.0));
    h.scroll_to(scroller, 0.0, 30.0);
    assert_eq!(
        h.origin(pinned),
        Point2D::new(0.0, 40.0),
        "an absolute box anchored above the scroller does not scroll with it",
    );

    let mut h = Harness::new(&css);
    let root = h.root();
    let scroller = h.el(root, "view.scroller.anchored");
    h.el(scroller, "view.row");
    h.el(scroller, "view.row");
    let owned = h.el(scroller, "view.pinned");

    assert_eq!(h.origin(owned), Point2D::new(0.0, 40.0));
    h.scroll_to(scroller, 0.0, 30.0);
    assert_eq!(
        h.origin(owned),
        Point2D::new(0.0, 10.0),
        "once the scroller is the containing block, its absolute child scrolls",
    );
}

#[test]
fn a_wheel_over_a_pinned_box_does_not_scroll_what_it_is_pinned_above() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     width: 100px; height: 100px; }
         .row { flex-shrink: 0; width: 100px; height: 100px; }
         .pinned { display: flex; position: absolute; left: 0; top: 0;
                   width: 60px; height: 60px; }",
    );
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    let rows: Vec<NodeId> = (0..3).map(|_| h.el(scroller, "view.row")).collect();
    let pinned = h.el(scroller, "view.pinned");

    // Routing hits the pinned box, and the scroll chain a router would start
    // there follows the containing-block chain — page-anchored, so it finds
    // no scroller. That divergence from DOM ancestry is what keeps a wheel
    // over the pinned box from moving the content it covers.
    assert_eq!(
        h.route(InputEvent::wheel(Point2D::new(30.0, 30.0), (0.0, 80.0))),
        Some(pinned)
    );
    assert_eq!(
        h.doc
            .dom
            .nearest_user_scrollable(pinned, ScrollAxes { x: false, y: true }),
        None
    );
    assert_eq!(
        h.doc.dom.scroll_chain(pinned, Vector2D::new(0.0, 80.0)),
        None
    );
    assert_eq!(h.doc.dom.scroll_offset(scroller), Vector2D::zero());

    // Over the content the chain starts inside the scroller and consumes.
    assert_eq!(
        h.route(InputEvent::wheel(Point2D::new(80.0, 80.0), (0.0, 80.0))),
        Some(rows[0])
    );
    assert_eq!(
        h.doc.dom.scroll_chain(rows[0], Vector2D::new(0.0, 80.0)),
        Some((scroller, Vector2D::new(0.0, 80.0))),
    );
}

#[test]
fn a_fixed_box_never_scrolls_with_an_ancestor_scroller() {
    let mut h = Harness::new(&format!(
        "{SCROLLER}
         .fixed {{ display: flex; position: fixed; left: 10px; top: 60px;
                   width: 20px; height: 20px; }}"
    ));
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    h.el(scroller, "view.row");
    let fixed = h.el(scroller, "view.fixed");

    assert_eq!(h.origin(fixed), Point2D::new(10.0, 60.0));
    h.scroll_to(scroller, 0.0, 40.0);
    assert_eq!(h.origin(fixed), Point2D::new(10.0, 60.0));
}

#[test]
fn nested_scrollers_compose_their_offsets() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .outer { display: flex; flex-direction: column; overflow: scroll;
                  width: 200px; height: 200px; }
         .inner { display: flex; flex-direction: column; overflow: scroll;
                  flex-shrink: 0; width: 100px; height: 100px; }
         .row { flex-shrink: 0; width: 100px; height: 300px; }",
    );
    let root = h.root();
    let outer = h.el(root, "view.outer");
    let inner = h.el(outer, "view.inner");
    let row = h.el(inner, "view.row");
    h.el(outer, "view.row");

    h.scroll_to(outer, 0.0, 25.0);
    h.scroll_to(inner, 0.0, 70.0);

    assert_eq!(h.origin(inner), Point2D::new(0.0, -25.0));
    assert_eq!(h.origin(row), Point2D::new(0.0, -95.0));
}

#[test]
fn scroll_translations_snap_to_the_device_pixel_grid() {
    let mut doc = Doc::with_device(device_with(800.0, 600.0, 2.0, PrefersColorScheme::Light));
    doc.add_css(SCROLLER);
    let mut h = Harness { doc };
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    let row = h.el(scroller, "view.row");
    h.el(scroller, "view.row");

    h.scroll_to(scroller, 0.0, 10.5);
    assert_eq!(h.origin(row), Point2D::new(0.0, -10.5));

    h.scroll_to(scroller, 0.0, 10.25);
    assert_eq!(h.origin(row), Point2D::new(0.0, -10.5));
    assert_eq!(
        h.doc.dom.scroll_offset(scroller),
        Vector2D::new(0.0, 10.25),
        "the stored offset stays exact; only the painted translation snaps",
    );
}

#[test]
fn overflow_hidden_clips_without_answering_a_gesture() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .clipper { display: flex; flex-direction: column; overflow: hidden;
                    width: 100px; height: 100px; }
         .row { flex-shrink: 0; width: 100px; height: 100px; }",
    );
    let root = h.root();
    let clipper = h.el(root, "view.clipper");
    let rows: Vec<NodeId> = (0..3).map(|_| h.el(clipper, "view.row")).collect();

    // `hidden` is a scroll container that answers no user gesture: the chain
    // a router would start under the wheel finds no user-scrollable box.
    assert_eq!(
        h.route(InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 60.0))),
        Some(rows[0])
    );
    assert_eq!(
        h.doc.dom.nearest_user_scrollable(rows[0], ScrollAxes::BOTH),
        None
    );
    assert_eq!(
        h.doc.dom.scroll_chain(rows[0], Vector2D::new(0.0, 60.0)),
        None
    );
    assert_eq!(h.doc.dom.scroll_offset(clipper), Vector2D::zero());

    h.scroll_to(clipper, 0.0, 60.0);
    assert_eq!(h.origin(rows[1]), Point2D::new(0.0, 40.0));
}

#[test]
fn overflow_clip_is_not_a_scroll_container_at_all() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .clipper { display: flex; flex-direction: column; overflow: clip;
                    width: 100px; height: 100px; }
         .row { flex-shrink: 0; width: 100px; height: 100px; }",
    );
    let root = h.root();
    let clipper = h.el(root, "view.clipper");
    let rows: Vec<NodeId> = (0..3).map(|_| h.el(clipper, "view.row")).collect();
    h.doc.dom.layout();

    assert!(!h.doc.dom.is_scroll_container(clipper));
    assert_eq!(h.doc.dom.scroll_box(clipper), None);
    assert_eq!(
        h.doc.dom.scroll_to(clipper, Vector2D::new(0.0, 60.0)),
        Vector2D::zero(),
    );
    assert_eq!(h.origin(rows[1]), Point2D::new(0.0, 100.0));

    assert_eq!(h.hit(50.0, 50.0), Some(rows[0]));
    assert_eq!(h.hit(50.0, 150.0), Some(root));
}

#[test]
fn a_clip_box_does_not_leak_its_overflow_into_an_ancestor_scroller() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     width: 100px; height: 100px; }
         .clipper { display: flex; flex-direction: column; overflow: clip;
                    flex-shrink: 0; width: 100px; height: 100px; }
         .row { flex-shrink: 0; width: 100px; height: 300px; }",
    );
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    let clipper = h.el(scroller, "view.clipper");
    h.el(clipper, "view.row");
    h.doc.dom.layout();

    let scroll_box = h.doc.dom.scroll_box(scroller).expect("scroller scrolls");
    assert_eq!(scroll_box.max_offset(), Vector2D::zero());
}

#[test]
fn clip_on_one_axis_leaves_the_other_unbounded() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .strip { display: flex; overflow-x: clip; width: 100px; height: 100px; }
         .wide { flex-shrink: 0; width: 300px; height: 300px; }",
    );
    let root = h.root();
    let strip = h.el(root, "view.strip");
    let wide = h.el(strip, "view.wide");
    h.doc.dom.layout();

    assert!(!h.doc.dom.is_scroll_container(strip));
    assert_eq!(h.hit(50.0, 50.0), Some(wide));
    assert_eq!(h.hit(150.0, 50.0), Some(root));
    assert_eq!(h.hit(50.0, 250.0), Some(wide));
}

#[test]
fn a_routed_chain_scroll_drives_paint_and_hit_testing_end_to_end() {
    let mut h = Harness::new(SCROLLER);
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    let rows: Vec<NodeId> = (0..4).map(|_| h.el(scroller, "view.row")).collect();

    // The runtime's router turns a 70px touch drag into a 62px chain scroll
    // (its 8px slop subtracted); the document's half is routing, the chain,
    // the repaint, and hit testing through the scrolled frame.
    let target = h
        .route(InputEvent::pointer(
            Point2D::new(50.0, 90.0),
            7,
            PointerKind::Touch,
            PointerPhase::Down,
        ))
        .expect("the press routes");
    let latched = h
        .doc
        .dom
        .nearest_user_scrollable(target, ScrollAxes::BOTH)
        .expect("the press lands on scrollable content");
    assert_eq!(latched, scroller);
    assert_eq!(
        h.doc.dom.scroll_chain(latched, Vector2D::new(0.0, 62.0)),
        Some((scroller, Vector2D::new(0.0, 62.0))),
    );
    assert_eq!(h.origin(rows[1]), Point2D::new(0.0, 38.0));
    assert_eq!(h.hit(50.0, 40.0), Some(rows[1]));
}

#[test]
fn input_targets_through_the_scrolled_frame() {
    let mut h = Harness::new(SCROLLER);
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    let rows: Vec<NodeId> = (0..4).map(|_| h.el(scroller, "view.row")).collect();
    h.scroll_to(scroller, 0.0, 120.0);

    let target = h.route(InputEvent::pointer(
        Point2D::new(30.0, 10.0),
        1,
        PointerKind::Touch,
        PointerPhase::Down,
    ));

    assert_eq!(target, Some(rows[1]));
}
