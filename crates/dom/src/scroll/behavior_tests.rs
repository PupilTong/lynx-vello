//! Behavior tests for scrolling: how a scroll offset reaches the painted
//! frame, what escapes it, and how host input drives it end to end.

#![allow(clippy::float_cmp)]

use euclid::default::Vector2D;
use stylo::queries::values::PrefersColorScheme;

use crate::input::{DefaultAction, InputEvent, PointerKind, PointerPhase};
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
fn a_wheel_over_a_pinned_box_does_not_scroll_what_it_is_pinned_above() {
    // The other half of the containing-block rule. `pinned` is a DOM child of
    // the scroller but anchored on the page, so it does not move when the
    // scroller scrolls — and a wheel over it must not scroll the scroller
    // either, or content would slide behind a box that visibly stays put.
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
    h.doc.dom.layout();

    let over_pinned = h
        .doc
        .dom
        .handle_input(InputEvent::wheel(Point2D::new(30.0, 30.0), (0.0, 80.0)));
    assert_eq!(over_pinned.target, Some(pinned));
    assert_eq!(over_pinned.default_action, DefaultAction::None);
    assert_eq!(h.doc.dom.scroll_offset(scroller), Vector2D::zero());

    // Just outside the pinned box, over the scroller's own content, the same
    // wheel does scroll — so this is the containing-block chain at work, not a
    // dead input path.
    let over_content = h
        .doc
        .dom
        .handle_input(InputEvent::wheel(Point2D::new(80.0, 80.0), (0.0, 80.0)));
    assert_eq!(over_content.target, Some(rows[0]));
    assert_eq!(
        over_content.default_action,
        DefaultAction::Scroll {
            node: scroller,
            delta: Vector2D::new(0.0, 80.0),
        },
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

    let response = h
        .doc
        .dom
        .handle_input(InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 60.0)));
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
        h.doc.dom.handle_input(InputEvent::pointer(
            Point2D::new(50.0, y),
            7,
            PointerKind::Touch,
            phase,
        ))
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

    let response = h.doc.dom.handle_input(InputEvent::pointer(
        Point2D::new(30.0, 10.0),
        1,
        PointerKind::Touch,
        PointerPhase::Down,
    ));

    // Row 1 spans y=100..200 unscrolled, so 120 of scroll puts its y=30 under
    // the viewport's y=10.
    assert_eq!(response.target, Some(rows[1]));
    assert_eq!(response.local_position, Some(Point2D::new(30.0, 30.0)));
}

/// A frame's scroll node index for `id`.
fn scroll_index(frame: &dom::Frame, id: NodeId) -> usize {
    frame
        .scrolls()
        .iter()
        .position(|node| node.node == id)
        .expect("a scroll container gets a scroll node")
}

/// Where `id`'s element box lands when the frame is painted at `offsets`
/// rather than at the offsets it was built with.
fn corrected_origin(frame: &dom::Frame, id: NodeId, offsets: &[Vector2D<f32>]) -> Point2D<f32> {
    let item = frame
        .items()
        .iter()
        .find(|item| item.node == id && item.kind == PaintItemKind::ElementBox)
        .expect("node paints an element box");
    let baked = item
        .transform
        .transform_point2d(Point2D::zero())
        .expect("a paintable item has a non-singular matrix");
    baked + frame.scroll_correction(item.scroll, offsets)
}

#[test]
fn a_renderer_can_scroll_a_frame_the_document_has_not_caught_up_with() {
    // The property the paint thread rests on: correcting a frame by a scroll
    // delta puts content exactly where a frame the *document* built at that
    // offset would have put it. If these ever disagree, scrolling ahead would
    // drift from what the next published frame shows.
    let mut h = Harness::new(SCROLLER);
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    let rows: Vec<NodeId> = (0..3).map(|_| h.el(scroller, "view.row")).collect();
    h.doc.dom.layout();

    let unscrolled = h.doc.dom.frame();
    let index = scroll_index(&unscrolled, scroller);
    assert_eq!(unscrolled.scrolls()[index].baked_offset, Vector2D::zero());
    assert_eq!(
        unscrolled.scrolls()[index].max_offset,
        Vector2D::new(0.0, 200.0),
        "three 100px rows in a 100px scrollport overhang by 200px",
    );
    // Routing has to actually find this box, or paint-side scrolling would
    // silently do nothing at all.
    assert_eq!(
        unscrolled.scroll_target(Point2D::new(50.0, 50.0), &[]),
        Some(index),
    );

    let mut offsets = vec![Vector2D::zero(); unscrolled.scrolls().len()];
    offsets[index] = Vector2D::new(0.0, 40.0);
    let ahead = corrected_origin(&unscrolled, rows[1], &offsets);

    h.scroll_to(scroller, 0.0, 40.0);
    let baked = h.doc.dom.frame();
    let settled = corrected_origin(&baked, rows[1], &[]);

    assert_eq!(ahead, settled);
    assert_eq!(ahead, Point2D::new(0.0, 60.0));

    // The scroller's own box does not move either way.
    assert_eq!(
        corrected_origin(&unscrolled, scroller, &offsets),
        corrected_origin(&baked, scroller, &[]),
    );
}

#[test]
fn corrections_for_nested_scrollers_sum_along_the_chain() {
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
    h.doc.dom.layout();

    let frame = h.doc.dom.frame();
    let (outer_index, inner_index) = (scroll_index(&frame, outer), scroll_index(&frame, inner));
    assert_eq!(
        frame.scrolls()[inner_index].parent,
        Some(outer_index),
        "the inner scroller chains to the outer one",
    );
    let mut offsets = vec![Vector2D::zero(); frame.scrolls().len()];
    offsets[outer_index] = Vector2D::new(0.0, 25.0);
    offsets[inner_index] = Vector2D::new(0.0, 70.0);

    h.scroll_to(outer, 0.0, 25.0);
    h.scroll_to(inner, 0.0, 70.0);
    let baked = h.doc.dom.frame();

    assert_eq!(
        corrected_origin(&frame, row, &offsets),
        corrected_origin(&baked, row, &[]),
    );
    assert_eq!(
        corrected_origin(&frame, row, &offsets),
        Point2D::new(0.0, -95.0)
    );
}

#[test]
fn a_correction_maps_through_a_scaling_ancestor() {
    // `offset_to_viewport` is the scroll container's own world linear map, so
    // a scroller under `transform: scale(2)` moves twice as far on screen as
    // its offset. Baking and correcting have to agree about that too, or the
    // frame would jump the moment the document caught up.
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .zoom { display: flex; transform: scale(2); transform-origin: 0 0; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     width: 100px; height: 100px; }
         .row { flex-shrink: 0; width: 100px; height: 100px; }",
    );
    let root = h.root();
    let zoom = h.el(root, "view.zoom");
    let scroller = h.el(zoom, "view.scroller");
    let rows: Vec<NodeId> = (0..3).map(|_| h.el(scroller, "view.row")).collect();
    h.doc.dom.layout();

    let frame = h.doc.dom.frame();
    let index = scroll_index(&frame, scroller);
    assert_eq!(
        frame.scrolls()[index].to_viewport(Vector2D::new(1.0, 3.0)),
        Vector2D::new(2.0, 6.0),
        "the scroller's world linear map doubles an offset delta",
    );
    let mut offsets = vec![Vector2D::zero(); frame.scrolls().len()];
    offsets[index] = Vector2D::new(0.0, 30.0);

    h.scroll_to(scroller, 0.0, 30.0);
    let baked = h.doc.dom.frame();

    let ahead = corrected_origin(&frame, rows[1], &offsets);
    assert_eq!(ahead, corrected_origin(&baked, rows[1], &[]));
    // Row 1 sits 100 CSS px down, scrolled back 30, all doubled on screen.
    assert_eq!(ahead, Point2D::new(0.0, 140.0));
}

#[test]
fn a_scroll_container_own_clip_does_not_move_with_its_contents() {
    // The scrollport clips; it does not scroll. A clip tagged with its own
    // scroller would slide away as the content moved, revealing everything
    // the box is meant to hide.
    let mut h = Harness::new(SCROLLER);
    let root = h.root();
    let scroller = h.el(root, "view.scroller");
    let inner = h.el(scroller, "view.scroller");
    h.el(inner, "view.row");
    h.doc.dom.layout();

    let frame = h.doc.dom.frame();
    let outer_index = scroll_index(&frame, scroller);
    let outer_clip = frame
        .clips()
        .iter()
        .find(|clip| clip.node == scroller)
        .expect("a scroll container clips its scrollport");
    assert_ne!(outer_clip.scroll, Some(outer_index));

    // The nested scroller's clip *does* move with the outer one.
    let inner_clip = frame
        .clips()
        .iter()
        .find(|clip| clip.node == inner)
        .expect("the nested scroll container clips too");
    assert_eq!(inner_clip.scroll, Some(outer_index));
}

#[test]
fn scroll_routing_is_exact_under_a_rotated_ancestor() {
    // A bounding-box test would claim the whole axis-aligned extent of a
    // rotated scroller, including the four triangles outside it. Routing maps
    // the point back through the container's own inverse instead, so those
    // corners miss — which is what the box actually looks like on screen.
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .spin { display: flex; transform: rotate(45deg); transform-origin: 0 0; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     width: 100px; height: 100px; }
         .row { flex-shrink: 0; width: 100px; height: 100px; }",
    );
    let root = h.root();
    let spin = h.el(root, "view.spin");
    let scroller = h.el(spin, "view.scroller");
    for _ in 0..3 {
        h.el(scroller, "view.row");
    }
    h.doc.dom.layout();
    let frame = h.doc.dom.frame();
    let index = scroll_index(&frame, scroller);

    // Rotated 45° about the origin, the box is a diamond: its own centre
    // (70.7 down the vertical axis) is inside...
    assert_eq!(
        frame.scroll_target(Point2D::new(0.0, 70.0), &[]),
        Some(index)
    );
    // ...while the top-left of its axis-aligned extent is emphatically not.
    assert_eq!(frame.scroll_target(Point2D::new(-60.0, 5.0), &[]), None);
    assert_eq!(frame.scroll_target(Point2D::new(60.0, 5.0), &[]), None);
}

#[test]
fn routing_follows_a_scroller_its_own_ancestor_has_scrolled() {
    // The inner scroller's baked position is stale the moment the renderer
    // scrolls the outer one. Routing rebases the point past the parent
    // chain's correction, so a gesture lands on the box that is actually
    // under the finger rather than where the document last drew it.
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .outer { display: flex; flex-direction: column; overflow: scroll;
                  width: 200px; height: 200px; }
         .spacer { flex-shrink: 0; width: 200px; height: 100px; }
         .inner { display: flex; flex-direction: column; overflow: scroll;
                  flex-shrink: 0; width: 100px; height: 100px; }
         .row { flex-shrink: 0; width: 100px; height: 300px; }",
    );
    let root = h.root();
    let outer = h.el(root, "view.outer");
    h.el(outer, "view.spacer");
    let inner = h.el(outer, "view.inner");
    h.el(inner, "view.row");
    h.doc.dom.layout();

    let frame = h.doc.dom.frame();
    let (outer_index, inner_index) = (scroll_index(&frame, outer), scroll_index(&frame, inner));

    // Unscrolled, the inner box spans y=100..200 and y=50 is only the outer.
    assert_eq!(
        frame.scroll_target(Point2D::new(50.0, 150.0), &[]),
        Some(inner_index)
    );
    assert_eq!(
        frame.scroll_target(Point2D::new(50.0, 50.0), &[]),
        Some(outer_index)
    );

    // Scroll the outer by 100 on the renderer's side: the inner box has
    // ridden up to y=0..100, so y=50 now hits it.
    let mut offsets = vec![Vector2D::zero(); frame.scrolls().len()];
    offsets[outer_index] = Vector2D::new(0.0, 100.0);
    assert_eq!(
        frame.scroll_target(Point2D::new(50.0, 50.0), &offsets),
        Some(inner_index),
    );
    assert_eq!(
        frame.scroll_target(Point2D::new(50.0, 150.0), &offsets),
        Some(outer_index)
    );
}

/// The paint-side scroller, driven the way a renderer holding a frame drives
/// it: adopt a frame, feed it events, read the offsets it paints with.
mod frame_scroller {
    use super::{Harness, SCROLLER};
    use crate::input::{InputEvent, PointerKind, PointerPhase};
    use crate::scroll::frame_scroller::{FrameScroller, ScrollUpdate};
    use crate::visual::frame::Frame;
    use crate::{NodeId, Point2D, Vector2D};

    fn offset_of(scroller: &FrameScroller, frame: &Frame, node: NodeId) -> Vector2D<f32> {
        let index = frame
            .scrolls()
            .iter()
            .position(|entry| entry.node == node)
            .expect("the scroller has a scroll node");
        scroller.offsets_for_testing()[index]
    }

    fn scrolling_page() -> (Harness, NodeId) {
        let mut h = Harness::new(SCROLLER);
        let root = h.root();
        let scroller = h.el(root, "view.scroller");
        for _ in 0..3 {
            h.el(scroller, "view.row");
        }
        h.doc.dom.layout();
        (h, scroller)
    }

    #[test]
    fn a_wheel_scrolls_the_box_under_it_and_clamps_at_its_end() {
        let (mut h, scroller) = scrolling_page();
        let frame = h.doc.dom.frame();
        let mut state = FrameScroller::default();
        state.adopt(&frame, 0);

        let response = state.handle_input(
            &frame,
            &InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 60.0)),
        );
        assert_eq!(response.scrolled.len(), 1);
        assert_eq!(response.scrolled[0].node, scroller);
        assert_eq!(
            offset_of(&state, &frame, scroller),
            Vector2D::new(0.0, 60.0)
        );

        // Past the end of the scrolling area it clamps rather than running on.
        state.handle_input(
            &frame,
            &InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 1_000.0)),
        );
        assert_eq!(
            offset_of(&state, &frame, scroller),
            Vector2D::new(0.0, 200.0),
            "three 100px rows in a 100px scrollport overhang by 200px",
        );

        // A wheel that consumes nothing reports nothing, so the caller does
        // not repaint an identical scene.
        let none = state.handle_input(
            &frame,
            &InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 10.0)),
        );
        assert!(none.scrolled.is_empty());
    }

    #[test]
    fn a_wheel_outside_every_scroll_container_scrolls_nothing() {
        let (mut h, _) = scrolling_page();
        let frame = h.doc.dom.frame();
        let mut state = FrameScroller::default();
        state.adopt(&frame, 0);

        let response = state.handle_input(
            &frame,
            &InputEvent::wheel(Point2D::new(300.0, 300.0), (0.0, 60.0)),
        );
        assert!(response.scrolled.is_empty());
    }

    #[test]
    fn line_deltas_resolve_to_the_pixels_the_document_would_use() {
        // The recognizer and the units are shared with `Document::handle_input`
        // precisely so these cannot drift; assert the resolved distance rather
        // than trusting that.
        let (mut h, scroller) = scrolling_page();
        let frame = h.doc.dom.frame();
        let mut state = FrameScroller::default();
        state.adopt(&frame, 0);
        state.handle_input(
            &frame,
            &InputEvent::wheel_with_mode(
                Point2D::new(50.0, 50.0),
                (0.0, 2.0),
                crate::input::DeltaMode::Line,
            ),
        );
        let here = offset_of(&state, &frame, scroller);

        let mut document = scrolling_page().0;
        document.doc.dom.handle_input(InputEvent::wheel_with_mode(
            Point2D::new(50.0, 50.0),
            (0.0, 2.0),
            crate::input::DeltaMode::Line,
        ));
        assert_eq!(here, document.doc.dom.scroll_offset(scroller));
    }

    #[test]
    fn a_touch_drag_scrolls_after_spending_the_slop_and_stops_on_lift() {
        let (mut h, scroller) = scrolling_page();
        let frame = h.doc.dom.frame();
        let mut state = FrameScroller::default();
        state.adopt(&frame, 0);

        let down = state.handle_input(
            &frame,
            &InputEvent::pointer(
                Point2D::new(50.0, 90.0),
                1,
                PointerKind::Touch,
                PointerPhase::Down,
            ),
        );
        assert!(down.scrolled.is_empty(), "a press alone scrolls nothing");
        assert!(
            !down.owns_gesture,
            "a press that may yet be a tap must not claim the event",
        );

        // Inside the slop radius nothing moves and nothing is claimed.
        let inside = state.handle_input(
            &frame,
            &InputEvent::pointer(
                Point2D::new(50.0, 85.0),
                1,
                PointerKind::Touch,
                PointerPhase::Move,
            ),
        );
        assert!(inside.scrolled.is_empty());
        assert!(!inside.owns_gesture);

        // Past it the toll is spent once and the overshoot survives: 30px of
        // travel less the 8px threshold.
        let crossed = state.handle_input(
            &frame,
            &InputEvent::pointer(
                Point2D::new(50.0, 60.0),
                1,
                PointerKind::Touch,
                PointerPhase::Move,
            ),
        );
        assert_eq!(crossed.scrolled.len(), 1);
        assert!(
            crossed.owns_gesture,
            "a scrolling drag suppresses the default"
        );
        assert_eq!(
            offset_of(&state, &frame, scroller),
            Vector2D::new(0.0, 30.0 - crate::input::TOUCH_SLOP),
        );

        // After the lift the gesture is gone.
        state.handle_input(
            &frame,
            &InputEvent::pointer(
                Point2D::new(50.0, 60.0),
                1,
                PointerKind::Touch,
                PointerPhase::Up,
            ),
        );
        let stray = state.handle_input(
            &frame,
            &InputEvent::pointer(
                Point2D::new(50.0, 10.0),
                1,
                PointerKind::Touch,
                PointerPhase::Move,
            ),
        );
        assert!(stray.scrolled.is_empty());
        assert!(!stray.owns_gesture);
    }

    #[test]
    fn a_mouse_drag_does_not_scroll() {
        // Matching every browser, and `Document::handle_input`: a mouse
        // scrolls with its wheel.
        let (mut h, scroller) = scrolling_page();
        let frame = h.doc.dom.frame();
        let mut state = FrameScroller::default();
        state.adopt(&frame, 0);

        for phase in [PointerPhase::Down, PointerPhase::Move] {
            let response = state.handle_input(
                &frame,
                &InputEvent::pointer(Point2D::new(50.0, 20.0), 1, PointerKind::Mouse, phase),
            );
            assert!(response.scrolled.is_empty());
            assert!(!response.owns_gesture);
        }
        assert_eq!(offset_of(&state, &frame, scroller), Vector2D::zero());
    }

    #[test]
    fn routing_ignores_a_pointer_events_none_overlay() {
        // The payoff of routing by hit test rather than by scrollport
        // rectangle: a bounding-box probe cannot tell that the box under the
        // cursor is transparent to input, and would answer identically.
        let mut h = Harness::new(
            "page { display: flex; position: relative; width: 800px; height: 600px; }
             .scroller { display: flex; flex-direction: column; overflow: scroll;
                         width: 100px; height: 100px; }
             .row { flex-shrink: 0; width: 100px; height: 100px; }
             .veil { display: flex; position: absolute; left: 0; top: 0;
                     width: 100px; height: 100px; pointer-events: none; }",
        );
        let root = h.root();
        let scroller = h.el(root, "view.scroller");
        for _ in 0..3 {
            h.el(scroller, "view.row");
        }
        let veil = h.el(root, "view.veil");
        h.doc.dom.layout();

        let frame = h.doc.dom.frame();
        let target = frame.hit_test(Point2D::new(50.0, 50.0), &[]);
        assert!(target.is_some());
        assert_ne!(target, Some(veil), "the veil is seen through");

        let mut state = FrameScroller::default();
        state.adopt(&frame, 0);
        let response = state.handle_input(
            &frame,
            &InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 60.0)),
        );
        assert_eq!(response.scrolled.len(), 1, "the veil must not swallow it");
        assert_eq!(response.scrolled[0].node, scroller);
    }

    #[test]
    fn a_wheel_over_a_scroll_containers_own_padding_scrolls_that_container() {
        // Hit testing resolves the scroller's own box here, and that box is
        // tagged with the *enclosing* scroll node — a scrollport does not move
        // with what it clips. Routing considers the hit node itself first, or
        // a gesture aimed at a scroller's empty margin would scroll past it.
        let (mut h, scroller) = scrolling_page();
        let frame = h.doc.dom.frame();
        let mut state = FrameScroller::default();
        state.adopt(&frame, 0);

        state.handle_input(
            &frame,
            &InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 500.0)),
        );
        let response = state.handle_input(
            &frame,
            &InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, -40.0)),
        );
        assert_eq!(response.scrolled.len(), 1);
        assert_eq!(response.scrolled[0].node, scroller);
        assert_eq!(
            offset_of(&state, &frame, scroller),
            Vector2D::new(0.0, 160.0)
        );
    }

    #[test]
    fn an_unconfirmed_offset_survives_a_new_frame_and_a_confirmed_one_yields() {
        let (mut h, scroller) = scrolling_page();
        let frame = h.doc.dom.frame();
        let mut state = FrameScroller::default();
        state.adopt(&frame, 0);
        let response = state.handle_input(
            &frame,
            &InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 60.0)),
        );
        let seq = response.scrolled[0].seq;

        // A frame published before the document applied the update must not
        // snap the view back to where the document still thinks it is.
        let behind = h.doc.dom.frame();
        state.adopt(&behind, seq - 1);
        assert_eq!(
            offset_of(&state, &behind, scroller),
            Vector2D::new(0.0, 60.0)
        );

        // Once applied, the frame is authoritative again — which is what lets
        // a programmatic scroll, or a post-relayout clamp, reach the screen.
        h.doc.dom.scroll_to(scroller, Vector2D::new(0.0, 10.0));
        let settled = h.doc.dom.frame();
        state.adopt(&settled, seq);
        assert_eq!(
            offset_of(&state, &settled, scroller),
            Vector2D::new(0.0, 10.0)
        );
    }

    #[test]
    fn updates_carry_the_frames_removal_epoch() {
        // A `NodeId` carries no generation, so an update in flight while the
        // document removes a node could land on a stranger. The epoch travels
        // with it, and a pending offset keyed by a possibly-recycled node is
        // dropped rather than pinned.
        let (mut h, scroller) = scrolling_page();
        let frame = h.doc.dom.frame();
        let epoch = frame.node_removal_epoch();
        let mut state = FrameScroller::default();
        state.adopt(&frame, 0);

        let response = state.handle_input(
            &frame,
            &InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 60.0)),
        );
        let update: ScrollUpdate = response.scrolled[0];
        assert_eq!(update.epoch, epoch);
        assert_eq!(update.node, scroller);

        let spare = h.el(h.root(), "view.row");
        h.doc.dom.remove_subtree(spare);
        let later = h.doc.dom.frame();
        assert_ne!(later.node_removal_epoch(), epoch);
        state.adopt(&later, 0);
        assert_eq!(
            offset_of(&state, &later, scroller),
            Vector2D::zero(),
            "a possibly-recycled key must not keep steering the offset",
        );
    }

    #[test]
    fn a_page_at_its_baked_offsets_paints_with_no_corrections_at_all() {
        // Lynx's UA cascade makes almost every element a scroll container, so
        // the arena is close to one entry per box and the chains are long. A
        // page nobody has scrolled must not pay to walk them.
        let (mut h, _) = scrolling_page();
        let frame = h.doc.dom.frame();
        let mut state = FrameScroller::default();
        state.adopt(&frame, 0);
        assert!(!frame.scrolls().is_empty(), "there is an arena to skip");
        assert!(state.paint_offsets(&frame).is_empty());

        state.handle_input(
            &frame,
            &InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 60.0)),
        );
        assert!(!state.paint_offsets(&frame).is_empty());
    }
}
