//! Intersection Observer geometry (`dom::visual::intersection`) through a
//! real document: one target against one root, read off the last completed
//! layout and the live scroll offsets.
//!
//! The rulings these pin: transforms are included (unlike
//! `bounding_client_rect`), intersection is edge-inclusive through every
//! clip on the target's containing-block chain (Chromium's), and `rootMargin`
//! percentages take the root rect's height for top/bottom and its width for
//! left/right.

#![allow(clippy::float_cmp)]

mod common;

use common::{Doc, device};
use dom::{
    FontBlob, IntersectionGeometry, MarginLength, NodeId, Point2D, Rect, RootMargin, Vector2D,
};

const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");

/// `(x, y, width, height)`.
type Rect4 = (f32, f32, f32, f32);

/// Every box is a flex container, and a `view` a non-shrinking one, unless a
/// case says otherwise, so the sizes a case writes are the sizes it gets.
const BASE: &str = "page { display: flex; } view { display: flex; flex-shrink: 0; }";

fn page(css: &str) -> Doc {
    Doc::with_css(&format!("{BASE}\n{css}"))
}

fn sized_page(css: &str, width: f32, height: f32) -> Doc {
    let mut doc = Doc::with_device(device(width, height));
    doc.add_css(&format!("{BASE}\n{css}"));
    doc
}

fn tuple(rect: Rect<f32>) -> Rect4 {
    (
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
}

/// Against the viewport, with no margin.
fn geo(doc: &Doc, target: NodeId) -> IntersectionGeometry {
    doc.dom
        .intersection_geometry(target, None, &RootMargin::ZERO)
}

/// Against an element root, with no margin.
fn geo_in(doc: &Doc, target: NodeId, root: NodeId) -> IntersectionGeometry {
    doc.dom
        .intersection_geometry(target, Some(root), &RootMargin::ZERO)
}

fn client_rect(doc: &Doc, id: NodeId) -> Rect4 {
    tuple(
        doc.dom
            .bounding_client_rect(id)
            .expect("the box is rendered"),
    )
}

#[track_caller]
fn assert_close(actual: Rect<f32>, expected: Rect4) {
    let actual = tuple(actual);
    let close = |a: f32, b: f32| (a - b).abs() <= 1e-3;
    assert!(
        close(actual.0, expected.0)
            && close(actual.1, expected.1)
            && close(actual.2, expected.2)
            && close(actual.3, expected.3),
        "{actual:?} is not within 1e-3 of {expected:?}",
    );
}

/// A rendered target that is no descendant of its root: nothing of its own
/// is reported.
#[track_caller]
fn assert_not_a_descendant(geometry: IntersectionGeometry, root_bounds: Rect4) {
    assert!(geometry.rendered);
    assert!(!geometry.is_intersecting);
    assert_eq!(tuple(geometry.target_rect), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(tuple(geometry.intersection_rect), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(tuple(geometry.root_bounds), root_bounds);
    assert_eq!(geometry.intersection_ratio(), 0.0);
}

// --- Agreement with the untransformed geometry ----------------------------

/// With no transform anywhere, the target rect is `bounding_client_rect`:
/// the same chain, the same offsets, the same scroll offsets.
#[test]
fn an_untransformed_target_is_its_bounding_client_rect() {
    let mut doc = page(
        "page { width: 400px; height: 300px; }
         .parent { position: relative; left: 10px; top: 10px; width: 200px; height: 200px; }
         .child { position: relative; left: 30px; top: 20px; width: 50px; height: 40px; }
         .scroller { flex-direction: column; overflow: scroll; margin-left: 20px;
                     margin-top: 10px; width: 100px; height: 100px; }
         .tall { width: 80px; height: 400px; }",
    );
    let root = doc.root;
    let parent = doc.el(root, "view.parent");
    let child = doc.el(parent, "view.child");
    let scroller = doc.el(root, "view.scroller");
    let tall = doc.el(scroller, "view.tall");
    doc.flush();
    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 50.0));

    for id in [root, parent, child, scroller, tall] {
        let geometry = geo(&doc, id);
        assert!(geometry.rendered);
        assert_eq!(tuple(geometry.target_rect), client_rect(&doc, id));
        assert_eq!(tuple(geometry.root_bounds), (0.0, 0.0, 800.0, 600.0));
    }
    let child_geometry = geo(&doc, child);
    assert!(child_geometry.is_intersecting);
    assert_eq!(child_geometry.intersection_rect, child_geometry.target_rect);
    assert_eq!(child_geometry.intersection_ratio(), 1.0);

    // Scrolled by 50, the tall box shows the scrollport's 100px of its 400.
    let tall_geometry = geo(&doc, tall);
    assert_eq!(client_rect(&doc, tall), (220.0, -40.0, 80.0, 400.0));
    assert_eq!(
        tuple(tall_geometry.intersection_rect),
        (220.0, 10.0, 80.0, 100.0)
    );
    assert_eq!(tall_geometry.intersection_ratio(), 0.25);
}

// --- Transforms -------------------------------------------------------------

/// The transform ruling: a 100px target moved 500px right in a 300px
/// viewport is not intersecting, while its `bounding_client_rect` stays the
/// untransformed box.
#[test]
fn a_transform_moves_the_target_but_not_its_bounding_client_rect() {
    let mut doc = sized_page(
        "page { width: 300px; height: 300px; }
         .target { width: 100px; height: 100px; transform: translateX(500px); }",
        300.0,
        300.0,
    );
    let target = doc.el(doc.root, "view.target");
    doc.flush();
    let geometry = geo(&doc, target);
    assert!(geometry.rendered);
    assert!(!geometry.is_intersecting);
    assert_eq!(tuple(geometry.target_rect), (500.0, 0.0, 100.0, 100.0));
    assert_eq!(tuple(geometry.intersection_rect), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(client_rect(&doc, target), (0.0, 0.0, 100.0, 100.0));

    doc.set_inline(target, "transform: translateX(250px)");
    doc.flush();
    let geometry = geo(&doc, target);
    assert!(geometry.is_intersecting);
    assert_eq!(tuple(geometry.intersection_rect), (250.0, 0.0, 50.0, 100.0));
    assert_eq!(geometry.intersection_ratio(), 0.5);
}

#[test]
fn a_rotated_target_reports_its_bounding_box() {
    let mut doc = page(
        ".target { margin-left: 100px; margin-top: 100px; width: 100px; height: 100px;
                   transform: rotate(45deg); }",
    );
    let target = doc.el(doc.root, "view.target");
    doc.flush();
    let geometry = geo(&doc, target);
    assert_close(
        geometry.target_rect,
        (79.289_32, 79.289_32, 141.421_36, 141.421_36),
    );
    assert!(geometry.is_intersecting);
    assert_eq!(geometry.intersection_rect, geometry.target_rect);
    assert_eq!(geometry.intersection_ratio(), 1.0);
}

#[test]
fn a_scaled_ancestor_scales_its_descendants() {
    let mut doc = page(
        ".parent { width: 200px; height: 200px; transform: scale(2); transform-origin: 0 0; }
         .child { margin-left: 10px; margin-top: 10px; width: 50px; height: 50px; }",
    );
    let parent = doc.el(doc.root, "view.parent");
    let child = doc.el(parent, "view.child");
    doc.flush();
    assert_close(geo(&doc, child).target_rect, (20.0, 20.0, 100.0, 100.0));

    // About the centre instead, the child lands mostly off the top left.
    doc.set_inline(parent, "transform-origin: 50% 50%");
    doc.flush();
    let geometry = geo(&doc, child);
    assert_close(geometry.target_rect, (-80.0, -80.0, 100.0, 100.0));
    assert_close(geometry.intersection_rect, (0.0, 0.0, 20.0, 20.0));
    assert!((geometry.intersection_ratio() - 0.04).abs() < 1e-6);
}

/// The individual `translate`/`rotate`/`scale` are storage-only in the
/// fork's grammar, so a list stands in for them here; the walk folds both
/// through the painter's own matrix either way.
#[test]
fn a_transform_list_composes_about_its_origin() {
    assert!(
        !common::parses("translate", "10px 20px") && !common::parses("scale", "2"),
        "the individual transforms parse now: test them here through the cascade",
    );
    let mut doc = page(
        ".target { margin-left: 100px; margin-top: 100px; width: 50px; height: 50px;
                   transform: translate(10px, 20px) scale(2); transform-origin: 0 0; }",
    );
    let target = doc.el(doc.root, "view.target");
    doc.flush();
    assert_close(geo(&doc, target).target_rect, (110.0, 120.0, 100.0, 100.0));

    doc.set_inline(target, "transform-origin: 50% 50%");
    doc.flush();
    assert_close(geo(&doc, target).target_rect, (85.0, 95.0, 100.0, 100.0));
}

#[test]
fn the_document_elements_transform_moves_everything() {
    let mut doc = page(
        "page { width: 400px; height: 300px; transform: translateX(-50px); }
         .target { width: 100px; height: 100px; }",
    );
    let target = doc.el(doc.root, "view.target");
    doc.flush();
    let geometry = geo(&doc, target);
    assert_eq!(tuple(geometry.target_rect), (-50.0, 0.0, 100.0, 100.0));
    assert_eq!(tuple(geometry.intersection_rect), (0.0, 0.0, 50.0, 100.0));
    assert_eq!(geometry.intersection_ratio(), 0.5);
    assert_eq!(
        tuple(geo(&doc, doc.root).target_rect),
        (-50.0, 0.0, 400.0, 300.0)
    );
}

/// A parent's `perspective` projects its child's 3D transform, about the
/// parent's centre: `translateZ(d / 2)` doubles the child.
#[test]
fn a_perspective_parent_projects_its_childs_depth() {
    let mut doc = page(
        ".parent { width: 200px; height: 200px; perspective: 100px; }
         .child { margin-left: 50px; margin-top: 50px; width: 100px; height: 100px;
                  transform: translateZ(50px); }",
    );
    let parent = doc.el(doc.root, "view.parent");
    let child = doc.el(parent, "view.child");
    doc.flush();
    let geometry = geo(&doc, child);
    assert_close(geometry.target_rect, (0.0, 0.0, 200.0, 200.0));
    assert!(geometry.is_intersecting);

    // Past the eye, no corner has a projection: rendered, but nothing to
    // report and nothing intersecting.
    doc.set_inline(child, "transform: translateZ(150px)");
    doc.flush();
    let geometry = geo(&doc, child);
    assert!(geometry.rendered);
    assert!(!geometry.is_intersecting);
    assert_eq!(tuple(geometry.target_rect), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(tuple(geometry.root_bounds), (0.0, 0.0, 800.0, 600.0));
}

// --- Clips ------------------------------------------------------------------

/// The containing-block escape on the clip chain: a fixed box is not clipped
/// by a scroller that is not its containing block, and is once a transform
/// makes it one.
#[test]
fn a_fixed_box_escapes_an_untransformed_scrollers_clip_but_not_a_transformed_ones() {
    let mut doc = page(
        "page { position: relative; }
         .scroller { overflow: hidden; width: 100px; height: 100px; }
         .fixed { position: fixed; left: 0; top: 200px; width: 50px; height: 50px; }",
    );
    let scroller = doc.el(doc.root, "view.scroller");
    let fixed = doc.el(scroller, "view.fixed");
    doc.flush();
    let geometry = geo(&doc, fixed);
    assert_eq!(tuple(geometry.target_rect), (0.0, 200.0, 50.0, 50.0));
    assert!(geometry.is_intersecting);
    assert_eq!(geometry.intersection_ratio(), 1.0);

    doc.set_inline(scroller, "transform: translateX(0px)");
    doc.flush();
    let geometry = geo(&doc, fixed);
    assert_eq!(tuple(geometry.target_rect), (0.0, 200.0, 50.0, 50.0));
    assert!(!geometry.is_intersecting, "clipped by its containing block");
    assert_eq!(geometry.intersection_ratio(), 0.0);
}

/// An `overflow: hidden` scroller (a programmatic-only scroll container)
/// clips a 20px cell below a 110px spacer: out, on the edge, half, whole as
/// it scrolls by 10px steps. No render runs between them.
#[test]
fn a_scrollers_clip_cuts_its_content_as_it_scrolls() {
    let mut doc = page(
        ".scroller { flex-direction: column; overflow: hidden; width: 100px; height: 100px; }
         .spacer { width: 100px; height: 110px; }
         .cell { width: 60px; height: 20px; }",
    );
    let scroller = doc.el(doc.root, "view.scroller");
    doc.el(scroller, "view.spacer");
    let cell = doc.el(scroller, "view.cell");
    doc.flush();

    let geometry = geo(&doc, cell);
    assert!(!geometry.is_intersecting, "10px below the scrollport");
    assert_eq!(tuple(geometry.target_rect), (0.0, 110.0, 60.0, 20.0));
    assert_eq!(geometry.intersection_ratio(), 0.0);

    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 10.0));
    let geometry = geo(&doc, cell);
    assert!(geometry.is_intersecting, "touching the scrollport's edge");
    assert_eq!(tuple(geometry.intersection_rect), (0.0, 100.0, 60.0, 0.0));
    assert_eq!(geometry.intersection_ratio(), 0.0);

    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 20.0));
    let geometry = geo(&doc, cell);
    assert_eq!(tuple(geometry.intersection_rect), (0.0, 90.0, 60.0, 10.0));
    assert_eq!(geometry.intersection_ratio(), 0.5);

    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 30.0));
    let geometry = geo(&doc, cell);
    assert_eq!(geometry.intersection_rect, geometry.target_rect);
    assert_eq!(tuple(geometry.target_rect), (0.0, 80.0, 60.0, 20.0));
    assert_eq!(geometry.intersection_ratio(), 1.0);
}

/// `overflow-x: clip` with `overflow-y: visible` clips one axis: an infinite
/// strip, as the frame's clip node is.
#[test]
fn a_one_axis_clip_is_a_strip() {
    let mut doc = page(
        ".strip { overflow-x: clip; overflow-y: visible; width: 100px; height: 100px; }
         .target { position: relative; left: 50px; top: 150px; width: 100px; height: 100px; }",
    );
    let strip = doc.el(doc.root, "view.strip");
    let target = doc.el(strip, "view.target");
    doc.flush();
    let geometry = geo(&doc, target);
    assert_eq!(tuple(geometry.target_rect), (50.0, 150.0, 100.0, 100.0));
    assert_eq!(
        tuple(geometry.intersection_rect),
        (50.0, 150.0, 50.0, 100.0)
    );
    assert_eq!(geometry.intersection_ratio(), 0.5);
}

#[test]
fn paint_containment_clips() {
    let mut doc = page(
        ".contained { contain: paint; width: 100px; height: 100px; }
         .target { position: relative; top: 150px; width: 100px; height: 100px; }",
    );
    let contained = doc.el(doc.root, "view.contained");
    let target = doc.el(contained, "view.target");
    doc.flush();
    let geometry = geo(&doc, target);
    assert!(geometry.rendered);
    assert!(!geometry.is_intersecting);
    assert_eq!(tuple(geometry.target_rect), (0.0, 150.0, 100.0, 100.0));

    doc.set_inline(contained, "contain: none");
    doc.flush();
    assert!(geo(&doc, target).is_intersecting);
}

// --- Element roots ----------------------------------------------------------

/// A root that clips is its padding box; one that does not is its border
/// box, and its overflowing content still intersects it where it overlaps.
#[test]
fn an_element_root_is_its_padding_box_when_it_clips() {
    let mut doc = page(
        ".root { box-sizing: border-box; border: 10px solid black; overflow: hidden;
                 margin-left: 100px; width: 120px; height: 120px; }
         .target { position: relative; left: -10px; top: -10px; width: 50px; height: 50px; }",
    );
    let root = doc.el(doc.root, "view.root");
    let target = doc.el(root, "view.target");
    doc.flush();
    let bounds = doc
        .dom
        .root_geometry(Some(root), &RootMargin::ZERO)
        .expect("a rendered root")
        .bounds;
    assert_eq!(tuple(bounds), (110.0, 10.0, 100.0, 100.0));
    let geometry = geo_in(&doc, target, root);
    assert_eq!(tuple(geometry.target_rect), (100.0, 0.0, 50.0, 50.0));
    assert_eq!(tuple(geometry.intersection_rect), (110.0, 10.0, 40.0, 40.0));
    assert_eq!(tuple(geometry.root_bounds), (110.0, 10.0, 100.0, 100.0));

    doc.set_inline(root, "overflow: visible");
    doc.flush();
    let bounds = doc
        .dom
        .root_geometry(Some(root), &RootMargin::ZERO)
        .expect("a rendered root")
        .bounds;
    assert_eq!(tuple(bounds), (100.0, 0.0, 120.0, 120.0));
    let geometry = geo_in(&doc, target, root);
    assert_eq!(geometry.intersection_rect, geometry.target_rect);
    assert_eq!(geometry.intersection_ratio(), 1.0);
}

#[test]
fn a_target_must_descend_from_its_root_on_its_containing_block_chain() {
    let mut doc = page(
        "page { position: relative; }
         .box { width: 100px; height: 100px; }
         .scroller { overflow: hidden; width: 100px; height: 100px; }
         .fixed { position: fixed; left: 0; top: 0; width: 10px; height: 10px; }",
    );
    let sibling = doc.el(doc.root, "view.box");
    let other = doc.el(doc.root, "view.box");
    let scroller = doc.el(doc.root, "view.scroller");
    let fixed = doc.el(scroller, "view.fixed");
    doc.flush();
    assert_not_a_descendant(geo_in(&doc, sibling, other), (100.0, 0.0, 100.0, 100.0));
    assert_not_a_descendant(geo_in(&doc, other, other), (100.0, 0.0, 100.0, 100.0));
    // Inside the scroller in the DOM, but its containing block is the
    // viewport: the scroller is off its chain.
    assert_not_a_descendant(geo_in(&doc, fixed, scroller), (200.0, 0.0, 100.0, 100.0));
    assert!(geo(&doc, fixed).is_intersecting);
}

/// The root's own scroll offset moves its content against it; its bounds
/// stay put.
#[test]
fn a_scrolled_element_root_sees_its_content_move() {
    let mut doc = page(
        "page { flex-direction: column; }
         .above { width: 10px; height: 50px; }
         .root { flex-direction: column; overflow: scroll; width: 100px; height: 100px; }
         .spacer { width: 100px; height: 150px; }
         .target { width: 100px; height: 20px; }
         .tail { width: 100px; height: 200px; }",
    );
    doc.el(doc.root, "view.above");
    let root = doc.el(doc.root, "view.root");
    doc.el(root, "view.spacer");
    let target = doc.el(root, "view.target");
    doc.el(root, "view.tail");
    doc.flush();
    let geometry = geo_in(&doc, target, root);
    assert!(!geometry.is_intersecting);
    assert_eq!(tuple(geometry.target_rect), (0.0, 200.0, 100.0, 20.0));
    assert_eq!(tuple(geometry.root_bounds), (0.0, 50.0, 100.0, 100.0));

    doc.dom.scroll_to(root, Vector2D::new(0.0, 60.0));
    let geometry = geo_in(&doc, target, root);
    assert!(geometry.is_intersecting);
    assert_eq!(tuple(geometry.target_rect), (0.0, 140.0, 100.0, 20.0));
    assert_eq!(tuple(geometry.intersection_rect), (0.0, 140.0, 100.0, 10.0));
    assert_eq!(tuple(geometry.root_bounds), (0.0, 50.0, 100.0, 100.0));
    assert_eq!(geometry.intersection_ratio(), 0.5);
}

// --- Root margins -----------------------------------------------------------

/// The `rootMargin` ruling: top and bottom percentages take the root's
/// height, left and right its width.
#[test]
fn margin_percentages_take_the_roots_height_and_width() {
    let doc = page("");
    let margin = RootMargin {
        top: MarginLength::Px(10.0),
        right: MarginLength::Percent(20.0),
        bottom: MarginLength::Percent(40.0),
        left: MarginLength::Px(30.0),
    };
    let root = doc
        .dom
        .root_geometry(None, &margin)
        .expect("the implicit root always resolves");
    assert_eq!(root.root(), None);
    assert_eq!(tuple(root.bounds), (-30.0, -10.0, 990.0, 850.0));
}

#[test]
fn negative_margins_shrink_the_root() {
    let mut doc = page(
        ".corner { width: 100px; height: 100px; }
         .straddle { position: relative; left: 50px; top: 50px; width: 100px; height: 100px; }",
    );
    let corner = doc.el(doc.root, "view.corner");
    let straddle = doc.el(corner, "view.straddle");
    doc.flush();
    let margin = RootMargin {
        top: MarginLength::Px(-100.0),
        right: MarginLength::Px(-100.0),
        bottom: MarginLength::Px(-100.0),
        left: MarginLength::Px(-100.0),
    };
    let root = doc.dom.root_geometry(None, &margin).expect("implicit");
    assert_eq!(tuple(root.bounds), (100.0, 100.0, 600.0, 400.0));

    let geometry = doc.dom.intersection_geometry_in(corner, &root);
    assert!(
        geometry.is_intersecting,
        "touching the shrunk root's corner"
    );
    assert_eq!(tuple(geometry.intersection_rect), (100.0, 100.0, 0.0, 0.0));
    assert_eq!(geometry.intersection_ratio(), 0.0);

    let geometry = doc.dom.intersection_geometry_in(straddle, &root);
    assert_eq!(
        tuple(geometry.intersection_rect),
        (100.0, 100.0, 50.0, 50.0)
    );
    assert_eq!(geometry.intersection_ratio(), 0.25);

    // A margin that eats the rect leaves a zero-size root, not a negative
    // one.
    let eaten = RootMargin {
        top: MarginLength::Percent(-60.0),
        right: MarginLength::Px(0.0),
        bottom: MarginLength::Percent(-60.0),
        left: MarginLength::Px(0.0),
    };
    let root = doc.dom.root_geometry(None, &eaten).expect("implicit");
    assert_eq!(tuple(root.bounds), (0.0, 360.0, 800.0, 0.0));
}

#[test]
fn element_root_margins_resolve_against_the_element() {
    let mut doc = page(
        ".root { margin-left: 100px; margin-top: 100px; width: 200px; height: 100px; }
         .target { position: relative; left: 250px; width: 40px; height: 40px; }",
    );
    let root = doc.el(doc.root, "view.root");
    let target = doc.el(root, "view.target");
    doc.flush();
    let margin = RootMargin {
        top: MarginLength::Percent(10.0),
        right: MarginLength::Percent(50.0),
        bottom: MarginLength::Percent(10.0),
        left: MarginLength::Percent(50.0),
    };
    let resolved = doc
        .dom
        .root_geometry(Some(root), &margin)
        .expect("rendered");
    assert_eq!(resolved.root(), Some(root));
    assert_eq!(tuple(resolved.bounds), (0.0, 90.0, 400.0, 120.0));
    let geometry = doc.dom.intersection_geometry_in(target, &resolved);
    assert_eq!(tuple(geometry.target_rect), (350.0, 100.0, 40.0, 40.0));
    assert!(
        geometry.is_intersecting,
        "inside the margin, outside the box"
    );
    assert_eq!(geometry.intersection_ratio(), 1.0);
    assert!(!geo_in(&doc, target, root).is_intersecting);
}

/// A zero-area sentinel on the viewport's bottom edge intersects with ratio
/// 1; a pixel past it, it does not.
#[test]
fn a_zero_area_sentinel_on_the_edge_intersects() {
    let mut doc = page(
        "page { flex-direction: column; width: 800px; height: 600px; }
         .spacer { width: 10px; height: 600px; }
         .sentinel { width: 100px; height: 0; }",
    );
    let spacer = doc.el(doc.root, "view.spacer");
    let sentinel = doc.el(doc.root, "view.sentinel");
    doc.flush();
    let geometry = geo(&doc, sentinel);
    assert_eq!(tuple(geometry.target_rect), (0.0, 600.0, 100.0, 0.0));
    assert!(geometry.is_intersecting);
    assert_eq!(geometry.intersection_ratio(), 1.0);

    doc.set_inline(spacer, "height: 601px");
    doc.flush();
    let geometry = geo(&doc, sentinel);
    assert_eq!(tuple(geometry.target_rect), (0.0, 601.0, 100.0, 0.0));
    assert!(!geometry.is_intersecting);
    assert_eq!(geometry.intersection_ratio(), 0.0);
}

// --- Live offsets -----------------------------------------------------------

/// `tests/sticky.rs`'s fixture: stuck 10px below the scrollport's top at
/// offset 80, through a transformed scroller too.
#[test]
fn a_sticky_target_is_where_it_sticks() {
    let css = "page { width: 800px; height: 600px; }
        .scroller { flex-direction: column; align-items: flex-start;
                    overflow: scroll; width: 100px; height: 100px; }
        .spacer { width: 100px; height: 40px; }
        .sticky { position: sticky; top: 10px; width: 80px; height: 20px; }
        .tail { width: 100px; height: 500px; }";
    for (extra, expected) in [
        ("", (0.0, 10.0, 80.0, 20.0)),
        (
            ".scroller { transform: scale(2); transform-origin: 0 0; }",
            (0.0, 20.0, 160.0, 40.0),
        ),
    ] {
        let mut doc = page(&format!("{css}\n{extra}"));
        let scroller = doc.el(doc.root, "view.scroller");
        doc.el(scroller, "view.spacer");
        let sticky = doc.el(scroller, "view.sticky");
        doc.el(scroller, "view.tail");
        doc.flush();
        doc.dom.scroll_to(scroller, Vector2D::new(0.0, 80.0));
        let geometry = geo(&doc, sticky);
        assert_close(geometry.target_rect, expected);
        assert!(geometry.is_intersecting);
        assert_eq!(geometry.intersection_ratio(), 1.0, "{extra}");
        if extra.is_empty() {
            assert_eq!(tuple(geometry.target_rect), client_rect(&doc, sticky));
        }
    }
}

/// `tests/anchor_positioning.rs`'s default-scroll-shift fixture: the
/// anchored box follows its scrolled anchor, as `bounding_client_rect`
/// reports it.
#[test]
fn an_anchored_target_includes_the_default_scroll_shift() {
    let mut doc = page(
        "page { flex-direction: column; width: 800px; height: 600px; }
         view { flex-direction: column; }
         .cb { position: relative; width: 400px; height: 400px; }
         .scroller { overflow: scroll; width: 200px; height: 100px; }
         .filler { height: 500px; }
         .anchor { anchor-name: --a; width: 40px; height: 30px; margin-top: 50px; }
         .anchored { position: absolute; position-anchor: --a; position-area: bottom;
                     width: 10px; height: 10px; }",
    );
    let cb = doc.el(doc.root, "view.cb");
    let scroller = doc.el(cb, "view.scroller");
    let content = doc.el(scroller, "view");
    doc.el(content, "view.anchor");
    doc.el(content, "view.filler");
    let anchored = doc.el(cb, "view.anchored");
    doc.dom.render();
    assert_eq!(client_rect(&doc, anchored).1, 80.0);
    assert_eq!(
        tuple(geo(&doc, anchored).target_rect),
        client_rect(&doc, anchored)
    );

    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 20.0));
    doc.dom.render();
    assert_eq!(client_rect(&doc, anchored).1, 60.0);
    let geometry = geo(&doc, anchored);
    assert_eq!(tuple(geometry.target_rect), client_rect(&doc, anchored));
    assert!(geometry.is_intersecting);
}

/// `position-visibility` hides a box from paint and hit testing only: its
/// geometry is still the box's, as `bounding_client_rect`'s is.
#[test]
fn a_box_position_visibility_hides_is_still_geometric() {
    let mut doc = page(
        "page { flex-direction: column; position: relative; width: 200px; height: 300px; }
         .above { width: 100px; height: 100px; }
         .scroller { flex-direction: column; overflow: scroll; width: 100px; height: 100px; }
         .gap { width: 100px; height: 40px; }
         .anchor { width: 20px; height: 20px; anchor-name: --a; }
         .tail { width: 100px; height: 300px; }
         .anchored { position: absolute; position-anchor: --a; top: anchor(top); left: 120px;
                     width: 40px; height: 20px; position-visibility: anchors-visible; }",
    );
    doc.el(doc.root, "view.above");
    let scroller = doc.el(doc.root, "view.scroller");
    doc.el(scroller, "view.gap");
    doc.el(scroller, "view.anchor");
    doc.el(scroller, "view.tail");
    let anchored = doc.el(doc.root, "view.anchored");
    doc.dom.render();
    assert_eq!(client_rect(&doc, anchored), (120.0, 140.0, 40.0, 20.0));
    assert_eq!(
        doc.dom
            .elements_from_point(Point2D::new(130.0, 150.0))
            .first(),
        Some(&anchored)
    );

    // The anchor scrolled wholly out of the scroller's clip: the box hides.
    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 70.0));
    doc.dom.render();
    assert_eq!(client_rect(&doc, anchored), (120.0, 70.0, 40.0, 20.0));
    assert_ne!(
        doc.dom
            .elements_from_point(Point2D::new(130.0, 80.0))
            .first(),
        Some(&anchored),
        "hidden from hit testing",
    );
    let geometry = geo(&doc, anchored);
    assert_eq!(tuple(geometry.target_rect), (120.0, 70.0, 40.0, 20.0));
    assert!(geometry.is_intersecting);
    assert_eq!(geometry.intersection_ratio(), 1.0);
}

// --- The top layer ----------------------------------------------------------

/// `tests/top_layer.rs`'s membership fixture: the dialog is placed against
/// the viewport past a transformed, clipping ancestor, which is therefore
/// not on its chain — it neither moves, clips, nor roots it.
#[test]
fn a_top_layer_element_escapes_its_ancestors() {
    let mut doc = page(
        "page { width: 800px; height: 600px; }
         dialog { display: flex; }
         .t { transform: translate(1px, 1px); margin-left: 40px; overflow: hidden;
              width: 100px; height: 100px; }
         dialog { position: static; margin-left: 7px; width: 50px; height: 50px; }
         .abs { position: absolute; left: 3px; top: 4px; width: 5px; height: 5px; }",
    );
    let t = doc.el(doc.root, "view.t");
    let dialog = doc.el(t, "dialog");
    let abs = doc.el(dialog, "view.abs");
    doc.dom.add_to_top_layer(dialog, true);
    doc.flush();
    assert_eq!(client_rect(&doc, dialog), (7.0, 0.0, 50.0, 50.0));

    let geometry = geo(&doc, dialog);
    assert_eq!(tuple(geometry.target_rect), (7.0, 0.0, 50.0, 50.0));
    assert!(geometry.is_intersecting, "not clipped by `.t`");
    assert_eq!(geometry.intersection_ratio(), 1.0);
    assert_eq!(tuple(geo(&doc, abs).target_rect), (10.0, 4.0, 5.0, 5.0));

    assert_not_a_descendant(geo_in(&doc, dialog, t), (41.0, 1.0, 100.0, 100.0));
    assert_not_a_descendant(geo_in(&doc, abs, t), (41.0, 1.0, 100.0, 100.0));
    let in_dialog = geo_in(&doc, abs, dialog);
    assert!(
        in_dialog.is_intersecting,
        "the dialog is its containing block"
    );
    assert_eq!(tuple(in_dialog.root_bounds), (7.0, 0.0, 50.0, 50.0));
}

// --- Rendered or not --------------------------------------------------------

#[test]
fn a_target_with_no_box_is_not_rendered() {
    let mut doc = page(
        "page { width: 400px; height: 300px; }
         .gone { display: none; }
         .through { display: contents; }
         .cell { width: 40px; height: 40px; }
         .invisible { visibility: hidden; }",
    );
    let root = doc.root;
    let gone = doc.el(root, "view.gone");
    let under_gone = doc.el(gone, "view.cell");
    let through = doc.el(root, "view.through");
    let inside_through = doc.el(through, "view.cell");
    let removed = doc.el(root, "view.cell");
    let removed_child = doc.el(removed, "view.cell");
    let invisible = doc.el(root, "view.cell.invisible");
    let never_attached = doc.dom.create_element("view", ());
    doc.flush();

    for (id, case) in [
        (gone, "display: none"),
        (under_gone, "under display: none"),
        (through, "display: contents"),
        (never_attached, "never attached"),
    ] {
        assert_eq!(geo(&doc, id), IntersectionGeometry::UNRENDERED, "{case}");
    }
    assert!(geo(&doc, inside_through).rendered);
    let geometry = geo(&doc, invisible);
    assert!(geometry.rendered, "visibility hides paint, not the box");
    assert!(geometry.is_intersecting);
    assert!(geo(&doc, removed_child).rendered);

    doc.dom.remove_element(removed);
    assert_eq!(geo(&doc, removed), IntersectionGeometry::UNRENDERED);
    assert_eq!(geo(&doc, removed_child), IntersectionGeometry::UNRENDERED);
}

/// css-contain-2 §4.5: skipped contents are never intersecting — they have
/// no box. The box that skips them still does.
#[test]
fn skipped_contents_are_not_rendered() {
    let mut doc = page(
        ".skipper { content-visibility: hidden; width: 100px; height: 100px; }
         .cell { width: 40px; height: 40px; }",
    );
    let skipper = doc.el(doc.root, "view.skipper");
    let cell = doc.el(skipper, "view.cell");
    doc.flush();
    assert!(geo(&doc, skipper).is_intersecting);
    assert_eq!(geo(&doc, cell), IntersectionGeometry::UNRENDERED);

    doc.set_inline(skipper, "content-visibility: visible");
    doc.flush();
    assert!(geo(&doc, cell).is_intersecting);
}

/// `tests/content_visibility.rs`'s scroller of `content-visibility: auto`
/// rows: after a render, a row past the encode window skips its label,
/// which is then not rendered; the rows in view lay theirs out.
#[test]
fn an_off_screen_auto_rows_contents_are_not_rendered() {
    let mut doc = sized_page(
        "page { width: 200px; height: 100vh; align-items: flex-start; font-family: Ahem; }
         .scroller { flex-direction: column; overflow: hidden; width: 200px; height: 100vh;
                     align-items: flex-start; }
         .row { width: 200px; content-visibility: auto; contain-intrinsic-size: 200px 20px; }
         .label { display: -lynx-text; font-size: 20px; }",
        200.0,
        100.0,
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let scroller = doc.el(doc.root, "view.scroller");
    let mut rows = Vec::new();
    let mut labels = Vec::new();
    for _ in 0..20 {
        let row = doc.el(scroller, "view.row");
        let label = doc.el(row, "text.label");
        let run = doc.dom.create_text_node("x", ());
        doc.dom.append_child(label, run);
        rows.push(row);
        labels.push(label);
    }
    doc.dom.render();

    let first = geo(&doc, labels[0]);
    assert!(first.rendered && first.is_intersecting);
    assert_eq!(geo(&doc, labels[15]), IntersectionGeometry::UNRENDERED);
    let row = geo(&doc, rows[15]);
    assert!(row.rendered, "the row itself keeps its box");
    assert!(!row.is_intersecting, "clipped by the scroller");
    assert_eq!(tuple(row.target_rect), (0.0, 300.0, 200.0, 20.0));
}

#[test]
fn an_unrendered_root_resolves_to_nothing() {
    let mut doc = page(
        ".gone { display: none; }
         .cell { width: 40px; height: 40px; }",
    );
    let gone = doc.el(doc.root, "view.gone");
    let cell = doc.el(doc.root, "view.cell");
    doc.flush();
    assert_eq!(doc.dom.root_geometry(Some(gone), &RootMargin::ZERO), None);
    assert_eq!(geo_in(&doc, cell, gone), IntersectionGeometry::UNRENDERED);
}
