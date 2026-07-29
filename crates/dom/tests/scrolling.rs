//! Behavior tests for scrolling: how a scroll offset reaches the painted
//! frame, what escapes it, and how host input drives it end to end.

#![allow(clippy::float_cmp)]

mod common;

use common::{Doc, device_with};
use dom::NodeId;
use dom::input::{DefaultAction, InputEvent, PointerKind, PointerPhase};
use dom::visual::{PaintItemKind, PaintOrder, Point2D};
use euclid::default::Vector2D;
use stylo::queries::values::PrefersColorScheme;

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
        self.doc.dom.paint_order()
    }

    /// The viewport-space border-box origin of a node's element item.
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
        self.doc.dom.hit_test(Point2D::new(x, y))
    }

    /// Scroll offsets clamp against the committed layout, so a tree that has
    /// been mutated since the last pass needs one before it can scroll.
    fn scroll_to(&mut self, id: NodeId, x: f32, y: f32) {
        self.doc.dom.layout();
        self.doc.dom.scroll_to(id, Vector2D::new(x, y));
    }
}

/// A 100×100 scroller at the page origin, stacking 100px rows into a column.
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
    // Row 1 is laid out at y=100..200 but clipped away by the 100px scrollport.
    assert_eq!(h.hit(50.0, 150.0), Some(root));

    h.scroll_to(scroller, 0.0, 150.0);

    // Row 1 now spans y=-50..50, and a box's hit region is half-open at its
    // trailing edge, so 40 lands on it and 50 already belongs to row 2.
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
    // Both absolute boxes are DOM children of the scroller. The scroller is
    // static, so `inside` is anchored on the *page* and must not scroll; give
    // the scroller `position: relative` and the same box becomes its own and
    // does.
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
    // At a 2× ratio, a half-CSS-pixel offset is a whole device pixel and
    // survives; a quarter is not and rounds away, so scrolled content keeps
    // the pixel alignment layout rounding gave unscrolled content.
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

    let frame = h.paint();
    let response = h.doc.dom.handle_input(
        &frame,
        InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 60.0)),
    );
    assert_eq!(response.default_action, DefaultAction::None);
    assert_eq!(h.doc.dom.scroll_offset(clipper), Vector2D::zero());

    // The same box still scrolls when something asks it to directly.
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
    // Unlike `hidden`, not even a programmatic scroll moves it.
    assert_eq!(
        h.doc.dom.scroll_to(clipper, Vector2D::new(0.0, 60.0)),
        Vector2D::zero(),
    );
    assert_eq!(h.origin(rows[1]), Point2D::new(0.0, 100.0));

    // It still clips: row 1 starts exactly at the 100px bottom edge.
    assert_eq!(h.hit(50.0, 50.0), Some(rows[0]));
    assert_eq!(h.hit(50.0, 150.0), Some(root));
}

#[test]
fn a_clip_box_does_not_leak_its_overflow_into_an_ancestor_scroller() {
    // The scroller's own child is 100px tall and clips 300px of content. Its
    // scrolling area must stop at the child's border box, not reach through it.
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
    // `clip` + `visible` is the one pair the style adjuster leaves mixed: it
    // reconciles axes that disagree about being *scrollable*, and neither of
    // these is. So this box clips horizontally and overflows vertically.
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
    // Inside the 100px width: hit. Past it: clipped away.
    assert_eq!(h.hit(50.0, 50.0), Some(wide));
    assert_eq!(h.hit(150.0, 50.0), Some(root));
    // Below the 100px height: still there, because that axis never clipped.
    assert_eq!(h.hit(50.0, 250.0), Some(wide));
}

#[test]
fn a_host_gesture_drives_paint_and_hit_testing_end_to_end() {
    let mut h = Harness::new(SCROLLER);
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    let rows: Vec<NodeId> = (0..4).map(|_| h.el(scroller, "view.row")).collect();

    let drag = |h: &mut Harness, y: f32, phase| {
        let frame = h.paint();
        h.doc.dom.handle_input(
            &frame,
            InputEvent::pointer(Point2D::new(50.0, y), 7, PointerKind::Touch, phase),
        )
    };

    drag(&mut h, 90.0, PointerPhase::Down);
    let moved = drag(&mut h, 20.0, PointerPhase::Move);
    drag(&mut h, 20.0, PointerPhase::Up);

    // 70px of travel, less the 8px slop toll.
    assert_eq!(
        moved.default_action,
        DefaultAction::Scroll {
            node: scroller,
            delta: Vector2D::new(0.0, 62.0),
        },
    );
    assert_eq!(h.origin(rows[1]), Point2D::new(0.0, 38.0));
    assert_eq!(h.hit(50.0, 40.0), Some(rows[1]));
}

#[test]
fn the_response_reports_where_the_event_landed_inside_its_target() {
    let mut h = Harness::new(SCROLLER);
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    let rows: Vec<NodeId> = (0..4).map(|_| h.el(scroller, "view.row")).collect();
    h.scroll_to(scroller, 0.0, 120.0);

    let frame = h.paint();
    let response = h.doc.dom.handle_input(
        &frame,
        InputEvent::pointer(
            Point2D::new(30.0, 10.0),
            1,
            PointerKind::Touch,
            PointerPhase::Down,
        ),
    );

    // Row 1 spans y=100..200 unscrolled, so 120 of scroll puts its y=30 under
    // the viewport's y=10.
    assert_eq!(response.target, Some(rows[1]));
    let local = response.local.expect("a hit reports its local point");
    assert_eq!(local.position, Point2D::new(30.0, 30.0));
    assert_eq!(frame.items()[local.item].node, rows[1]);
}
