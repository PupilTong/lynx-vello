//! Sticky geometry must agree in the document and a retained frame whose
//! consumer scrolls independently, including after the normal box is culled.

#![allow(clippy::float_cmp)]

mod common;

use common::Doc;
use dom::vello::Scene;
use dom::vello::peniko::Color;
use dom::{CommittedFrame, NodeId, Point2D};
use euclid::default::Vector2D;

const CSS: &str = "page { display: flex; width: 800px; height: 600px; }
    .scroller { display: flex; flex-direction: column; align-items: flex-start;
                overflow: scroll; width: 100px; height: 100px; }
    .spacer { flex-shrink: 0; width: 100px; height: 40px; }
    .sticky { display: flex; flex-direction: column; flex-shrink: 0;
              position: sticky; top: 10px; width: 80px; height: 20px;
              background: red; }
    .tail { flex-shrink: 0; width: 100px; height: 500px; background: blue; }
    .group { display: flex; flex-direction: column; align-items: flex-start;
             flex-shrink: 0; width: 100px; height: 160px; }";

fn page(extra: &str) -> (Doc, NodeId, NodeId) {
    let mut doc = Doc::with_css(&format!("{CSS}\n{extra}"));
    let scroller = doc.el(doc.root, "view.scroller");
    doc.el(scroller, "view.spacer");
    let sticky = doc.el(scroller, "view.sticky");
    doc.el(scroller, "view.tail");
    (doc, scroller, sticky)
}

fn hit(frame: &CommittedFrame, scroller: NodeId, x: f32, y: f32, offset: f32) -> Option<NodeId> {
    frame
        .hit(
            Point2D::new(x, y),
            &|slot| (slot.node == scroller).then_some(Vector2D::new(0.0, offset)),
            None,
        )
        .map(|target| target.node)
}

#[test]
fn sticky_preserves_flow_and_tracks_live_offsets_without_a_commit() {
    let (mut doc, scroller, sticky) = page("");
    let frame = doc.dom.commit();
    assert_eq!(doc.dom.rounded_layout(sticky).unwrap().location.y, 40.0);
    assert_eq!(hit(&frame, scroller, 20.0, 45.0, 0.0), Some(sticky));
    assert_ne!(hit(&frame, scroller, 20.0, 15.0, 0.0), Some(sticky));
    for offset in [30.0, 50.0, 80.0] {
        assert_eq!(hit(&frame, scroller, 20.0, 15.0, offset), Some(sticky));
        assert_ne!(hit(&frame, scroller, 20.0, 35.0, offset), Some(sticky));
    }
    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 80.0));
    assert!(!doc.dom.render(), "an in-window scroll only recomposes");
    assert_eq!(doc.dom.commit().commit_id(), frame.commit_id());
    assert_eq!(
        doc.dom.elements_from_point(Point2D::new(20.0, 15.0))[0],
        sticky
    );
    assert_eq!(doc.dom.rounded_layout(sticky).unwrap().location.y, 40.0);
}

#[test]
fn sticky_stops_at_its_containing_block_end() {
    let mut doc = Doc::with_css(CSS);
    let scroller = doc.el(doc.root, "view.scroller");
    let group = doc.el(scroller, "view.group");
    doc.el(group, "view.spacer");
    let sticky = doc.el(group, "view.sticky");
    doc.el(scroller, "view.tail");
    doc.dom.commit();
    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 135.0));
    let frame = doc.dom.commit();
    assert_eq!(hit(&frame, scroller, 20.0, 6.0, 135.0), Some(sticky));
    assert_ne!(hit(&frame, scroller, 20.0, 26.0, 135.0), Some(sticky));
    assert_ne!(hit(&frame, scroller, 20.0, 5.0, 161.0), Some(sticky));
}

#[test]
fn auto_insets_leave_the_axis_in_normal_flow() {
    let (mut doc, scroller, sticky) = page(".sticky { top: auto; }");
    let frame = doc.dom.commit();
    assert_eq!(hit(&frame, scroller, 20.0, 15.0, 30.0), Some(sticky));
    assert_ne!(hit(&frame, scroller, 20.0, 15.0, 80.0), Some(sticky));
}

#[test]
fn percentage_insets_use_the_scrollport() {
    let mut doc = Doc::with_css(&format!("{CSS} .sticky {{ top: 20%; }}"));
    let scroller = doc.el(doc.root, "view.scroller");
    let group = doc.el(scroller, "view.group");
    doc.el(group, "view.spacer");
    let sticky = doc.el(group, "view.sticky");
    doc.el(scroller, "view.tail");
    let frame = doc.dom.commit();
    assert_ne!(hit(&frame, scroller, 20.0, 19.0, 70.0), Some(sticky));
    assert_eq!(hit(&frame, scroller, 20.0, 21.0, 70.0), Some(sticky));
}

#[test]
fn fractional_sticky_insets_remain_subpixel() {
    let (mut doc, scroller, sticky) = page(".sticky { top: 0.25px; }");
    let frame = doc.dom.commit();
    assert_ne!(hit(&frame, scroller, 20.0, 0.1, 80.0), Some(sticky));
    assert_eq!(hit(&frame, scroller, 20.0, 0.3, 80.0), Some(sticky));
    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 80.0));
    assert_eq!(doc.dom.bounding_client_rect(sticky).unwrap().origin.y, 0.25);
}

#[test]
fn bottom_inset_pulls_up_without_changing_normal_position() {
    let (mut doc, scroller, sticky) =
        page(".spacer { height: 150px; } .sticky { top: auto; bottom: 10px; }");
    let frame = doc.dom.commit();
    assert_eq!(doc.dom.rounded_layout(sticky).unwrap().location.y, 150.0);
    assert_eq!(hit(&frame, scroller, 20.0, 75.0, 0.0), Some(sticky));
    assert_ne!(hit(&frame, scroller, 20.0, 95.0, 0.0), Some(sticky));
    assert_eq!(hit(&frame, scroller, 20.0, 55.0, 100.0), Some(sticky));
}

#[test]
fn oversized_sticky_reduces_the_end_inset() {
    let (mut doc, scroller, sticky) = page(".sticky { height: 150px; bottom: 10px; }");
    let frame = doc.dom.commit();
    assert_eq!(hit(&frame, scroller, 20.0, 15.0, 80.0), Some(sticky));
    assert_eq!(hit(&frame, scroller, 20.0, 95.0, 80.0), Some(sticky));
    assert_ne!(hit(&frame, scroller, 20.0, 5.0, 80.0), Some(sticky));
}

#[test]
fn hidden_is_a_scrollport_but_clip_is_not() {
    for overflow in ["hidden", "clip"] {
        let mut doc = Doc::with_css(&format!("{CSS} .group {{ overflow: {overflow}; }}"));
        let outer = doc.el(doc.root, "view.scroller");
        let group = doc.el(outer, "view.group");
        let sticky = doc.el(group, "view.sticky");
        doc.el(group, "view.tail");
        doc.el(outer, "view.tail");
        let frame = doc.dom.commit();
        let found = hit(&frame, outer, 20.0, 15.0, 80.0);
        if overflow == "clip" {
            assert_eq!(found, Some(sticky));
        } else {
            assert_ne!(found, Some(sticky));
            assert_eq!(
                frame
                    .hit(
                        Point2D::new(20.0, 15.0),
                        &|slot| { (slot.node == group).then_some(Vector2D::new(0.0, 80.0)) },
                        None
                    )
                    .map(|target| target.node),
                Some(sticky),
                "hidden supports programmatic sticky scrolling",
            );
        }
    }
}

#[test]
fn display_contents_does_not_replace_the_sticky_containing_block() {
    let mut doc = Doc::with_css(&format!("{CSS} .wrapper {{ display: contents; }}"));
    let scroller = doc.el(doc.root, "view.scroller");
    let group = doc.el(scroller, "view.group");
    let wrapper = doc.el(group, "view.wrapper");
    let sticky = doc.el(wrapper, "view.sticky");
    doc.el(scroller, "view.tail");
    let frame = doc.dom.commit();
    assert_eq!(hit(&frame, scroller, 20.0, 15.0, 80.0), Some(sticky));
}

#[test]
fn horizontal_sticky_uses_left_and_right_insets() {
    for (inset, offset, expected_x) in [("left: 10px", 100.0, 10.0), ("right: 10px", 0.0, 10.0)] {
        let (mut doc, scroller, sticky) = page(&format!(
            ".scroller {{ flex-direction: row; }} .sticky {{ top: auto; {inset}; }}"
        ));
        let frame = doc.dom.commit();
        let offset_of =
            |slot: &dom::ScrollSlot| (slot.node == scroller).then_some(Vector2D::new(offset, 0.0));
        assert_eq!(
            frame
                .hit(Point2D::new(expected_x + 1.0, 5.0), &offset_of, None)
                .map(|target| target.node),
            Some(sticky),
            "{inset}",
        );
    }
}

#[test]
fn fixed_descendant_escapes_sticky_motion_but_absolute_descendant_follows() {
    let (mut doc, scroller, sticky) = page(
        ".child { position: absolute; top: 3px; left: 3px; width: 10px; height: 10px; }
         .fixed { position: fixed; top: 70px; left: 50px; width: 10px; height: 10px; }",
    );
    let child = doc.el(sticky, "view.child");
    let fixed = doc.el(sticky, "view.fixed");
    let frame = doc.dom.commit();
    assert_eq!(hit(&frame, scroller, 5.0, 15.0, 80.0), Some(child));
    for offset in [0.0, 80.0] {
        assert_eq!(hit(&frame, scroller, 55.0, 75.0, offset), Some(fixed));
    }
}

#[test]
fn nested_sticky_offsets_do_not_double_count_parent_motion() {
    let (mut doc, scroller, outer) = page(
        ".sticky { height: 70px; }
         .inner { position: sticky; top: 15px; width: 30px; height: 20px; flex-shrink: 0; }",
    );
    let inner = doc.el(outer, "view.inner");
    let frame = doc.dom.commit();
    assert_eq!(hit(&frame, scroller, 10.0, 16.0, 80.0), Some(inner));
    assert_eq!(hit(&frame, scroller, 10.0, 12.0, 80.0), Some(outer));
    assert_eq!(hit(&frame, scroller, 10.0, 36.0, 80.0), Some(outer));
}

#[test]
fn a_transformed_scroller_maps_sticky_motion_to_viewport_coordinates() {
    let (mut doc, scroller, sticky) =
        page(".scroller { transform: scale(2); transform-origin: 0 0; }");
    let frame = doc.dom.commit();
    assert_ne!(hit(&frame, scroller, 20.0, 19.0, 80.0), Some(sticky));
    assert_eq!(hit(&frame, scroller, 20.0, 21.0, 80.0), Some(sticky));
    assert_eq!(hit(&frame, scroller, 20.0, 59.0, 80.0), Some(sticky));
    assert_ne!(hit(&frame, scroller, 20.0, 61.0, 80.0), Some(sticky));
}

#[test]
fn a_sticky_scroller_moves_its_clip_with_its_box() {
    let (mut doc, scroller, sticky) = page(
        ".sticky { overflow: hidden; }
         .child { flex-shrink: 0; width: 30px; height: 60px; }",
    );
    let child = doc.el(sticky, "view.child");
    let frame = doc.dom.commit();
    assert_eq!(hit(&frame, scroller, 10.0, 15.0, 80.0), Some(child));
    assert_ne!(hit(&frame, scroller, 10.0, 35.0, 80.0), Some(child));
}

#[test]
fn an_unscrollable_document_uses_the_viewport_as_sticky_reference() {
    let (mut doc, _, sticky) = page(".scroller { overflow: visible; }");
    // No scrollport ancestor; bottom: 90vh leaves a viewport view rectangle
    // ending at y=60, pulling the 20px box at y=150 up to y=40.
    doc.add_css(".spacer { height: 150px; } .sticky { top: auto; bottom: 90vh; }");
    let frame = doc.dom.commit();
    assert_eq!(
        frame
            .hit(Point2D::new(20.0, 45.0), &|_| None, None)
            .map(|target| target.node),
        Some(sticky),
    );
}

#[test]
fn a_grid_item_stops_at_its_grid_area_end() {
    let mut doc = Doc::with_css(&format!(
        "{CSS} .grid {{ display: grid; grid-template-rows: 120px 140px;
                       width: 100px; height: 260px; flex-shrink: 0; align-items: start; }}
         .sticky {{ margin-top: 40px; }}"
    ));
    let scroller = doc.el(doc.root, "view.scroller");
    let grid = doc.el(scroller, "view.grid");
    let sticky = doc.el(grid, "view.sticky");
    doc.el(scroller, "view.tail");
    let frame = doc.dom.commit();
    assert_eq!(doc.dom.rounded_layout(sticky).unwrap().location.y, 40.0);
    assert_eq!(hit(&frame, scroller, 20.0, 6.0, 95.0), Some(sticky));
    assert_ne!(hit(&frame, scroller, 20.0, 26.0, 95.0), Some(sticky));
}

#[test]
fn containing_block_clamp_reduces_margin_to_the_margin_edge_distance() {
    let mut doc = Doc::with_css(&format!(
        "{CSS} .group {{ height: 170px; }} .sticky {{ margin-bottom: 100px; }}"
    ));
    let scroller = doc.el(doc.root, "view.scroller");
    let group = doc.el(scroller, "view.group");
    doc.el(group, "view.spacer");
    let sticky = doc.el(group, "view.sticky");
    doc.el(scroller, "view.tail");
    doc.dom.commit();
    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 135.0));
    let frame = doc.dom.commit();
    assert_eq!(hit(&frame, scroller, 20.0, 6.0, 135.0), Some(sticky));
    assert_ne!(hit(&frame, scroller, 20.0, 26.0, 135.0), Some(sticky));
}

#[test]
fn sticky_insets_start_at_the_scrollport_inside_the_border() {
    let (mut doc, scroller, sticky) = page(".scroller { border: 5px solid black; padding: 10px; }");
    let frame = doc.dom.commit();
    assert_ne!(hit(&frame, scroller, 20.0, 14.0, 80.0), Some(sticky));
    assert_eq!(hit(&frame, scroller, 20.0, 16.0, 80.0), Some(sticky));
}

#[test]
fn transformed_sticky_is_the_fixed_descendants_containing_block() {
    let (mut doc, scroller, sticky) = page(
        ".sticky { transform: translateX(5px); }
         .fixed { position: fixed; top: 3px; left: 3px; width: 10px; height: 10px; }",
    );
    let fixed = doc.el(sticky, "view.fixed");
    let frame = doc.dom.commit();
    assert_eq!(hit(&frame, scroller, 10.0, 15.0, 80.0), Some(fixed));
}

#[test]
fn sticky_scroll_container_preserves_content_visibility_culling() {
    let mut doc = Doc::with_css(
        "page { display: flex; width: 800px; height: 600px; }
         .scroller { display: flex; flex-direction: column; position: sticky;
                     top: 10px; width: 100px; height: 100px; overflow: scroll; }
         .row { display: flex; flex-shrink: 0; width: 100px;
                content-visibility: auto; contain-intrinsic-size: 100px 100px; }
         .child { width: 100px; height: 100px; }",
    );
    let scroller = doc.el(doc.root, "view.scroller");
    let children: Vec<_> = (0..20)
        .map(|_| {
            let row = doc.el(scroller, "view.row");
            doc.el(row, "view.child")
        })
        .collect();
    doc.dom.render();
    assert_eq!(
        doc.dom.rounded_layout(children[0]).unwrap().size.height,
        100.0
    );
    assert_eq!(
        doc.dom.rounded_layout(children[19]).unwrap().size.height,
        0.0,
        "sticky movement must not reveal all offscreen descendants"
    );
}

#[test]
fn sticky_survives_an_initial_target_refill_and_a_retained_snap_step() {
    let (mut doc, scroller, sticky) = page(
        ".scroller { scroll-snap-type: y mandatory; }
         .snap-page { display: flex; flex-shrink: 0; width: 100px; height: 100px;
                      scroll-snap-align: start; background: blue; }",
    );
    let target = doc.el(scroller, "view.snap-page");
    doc.el(scroller, "view.snap-page");
    let original = doc.dom.commit();
    assert_eq!(doc.dom.scroll_offset(scroller), Vector2D::zero());
    assert!(
        !original.covers_scroll_offset(scroller, Vector2D::new(0.0, 560.0)),
        "the initial target must require a newly centered encode window"
    );

    doc.dom
        .set_inline_style(target, "scroll-initial-target: nearest");
    let frame = doc.dom.commit();
    let slot = &frame.scroll_slots()[frame.slot_of(scroller).unwrap() as usize];
    assert_eq!(slot.offset, Vector2D::new(0.0, 560.0));
    assert_eq!(doc.dom.scroll_offset(scroller), slot.offset);
    assert!(frame.covers_scroll_offset(scroller, slot.offset));
    let (_, snap_y) = frame.snap_axes(slot);
    assert_eq!(
        snap_y
            .unwrap()
            .points
            .iter()
            .map(|point| point.min)
            .collect::<Vec<_>>(),
        [560.0, 660.0],
        "the rebuilt frame retains its snap-point table beside sticky constraints"
    );

    let mut gpu = flashbulb::headless("sticky_initial_target_and_snap");
    for snapped in [false, true] {
        if snapped {
            doc.dom
                .scroll_chain_directed(scroller, Vector2D::new(0.0, 1.0));
            assert_eq!(doc.dom.scroll_offset(scroller), Vector2D::new(0.0, 660.0));
            assert_eq!(
                doc.dom.commit().commit_id(),
                frame.commit_id(),
                "an in-window snap samples the existing sticky constraints"
            );
        }
        // First use only the published offset; then override it with the
        // live snap result while retaining the same committed frame.
        let offsets = |slot: &dom::ScrollSlot| snapped.then(|| doc.dom.scroll_offset(slot.node));
        assert_eq!(
            frame
                .hit(Point2D::new(20.0, 15.0), &offsets, None)
                .map(|target| target.node),
            Some(sticky)
        );
        let mut scene = Scene::new();
        frame.compose_into(&mut scene, &[], &[], &offsets, None);
        let pixels = gpu
            .render(&scene, &[], 100, 100, Color::WHITE)
            .expect("render sticky after initial targeting and snapping");
        let at = (15 * 100 + 20) * 4;
        assert_eq!(&pixels[at..at + 4], &[255, 0, 0, 255]);
    }
}

#[test]
fn sticky_pixels_survive_scroll_refills_and_match_retained_frame_hits() {
    let (mut doc, scroller, sticky) = page("");
    let mut gpu = flashbulb::headless("sticky_pixels_survive_scroll_refills");
    for offset in [0.0, 80.0, 400.0] {
        doc.dom.commit();
        doc.dom.scroll_to(scroller, Vector2D::new(0.0, offset));
        let frame = doc.dom.commit();
        let y: u16 = if offset == 0.0 { 45 } else { 15 };
        let mut scene = Scene::new();
        frame.compose_into(
            &mut scene,
            &[],
            &[],
            &|slot| Some(doc.dom.scroll_offset(slot.node)),
            None,
        );
        let pixels = gpu
            .render(&scene, &[], 100, 100, Color::WHITE)
            .expect("render sticky");
        let at = (usize::from(y) * 100 + 20) * 4;
        assert_eq!(&pixels[at..at + 4], &[255, 0, 0, 255], "scrollTop={offset}");
        assert_eq!(
            hit(&frame, scroller, 20.0, f32::from(y), offset),
            Some(sticky)
        );
    }
    // The compositor can move the retained frame without changing document state.
    let (mut doc, scroller, sticky) = page("");
    let frame = doc.dom.commit();
    let mut scene = Scene::new();
    frame.compose_into(
        &mut scene,
        &[],
        &[],
        &|slot| (slot.node == scroller).then_some(Vector2D::new(0.0, 80.0)),
        None,
    );
    let pixels = gpu
        .render(&scene, &[], 100, 100, Color::WHITE)
        .expect("compose sticky");
    let at = (15 * 100 + 20) * 4;
    assert_eq!(&pixels[at..at + 4], &[255, 0, 0, 255]);
    assert_eq!(hit(&frame, scroller, 20.0, 15.0, 80.0), Some(sticky));
    assert_eq!(doc.dom.scroll_offset(scroller), Vector2D::zero());
}

#[test]
fn sticky_pixels_survive_enclosing_effects_and_far_refills() {
    let mut gpu = flashbulb::headless("sticky_pixels_survive_enclosing_effects");
    for effect in ["opacity: .5", "filter: blur(1px)"] {
        let (mut doc, scroller, _) = page(&format!(".scroller {{ {effect}; }}"));
        gpu.forget_filters();
        for (generation, offset) in (0_u64..).zip([0.0, 80.0, 400.0]) {
            doc.dom.commit();
            doc.dom.scroll_to(scroller, Vector2D::new(0.0, offset));
            let frame = doc.dom.commit();
            let offsets = |slot: &dom::ScrollSlot| Some(doc.dom.scroll_offset(slot.node));
            let filtered = gpu
                .prepare_filters(&frame, &[], &offsets, generation, None)
                .expect("prepare sticky effects")
                .to_vec();
            let mut scene = Scene::new();
            frame.compose_into(&mut scene, &[], &filtered, &offsets, None);
            let pixels = gpu
                .render(&scene, &[], 100, 100, Color::WHITE)
                .expect("render sticky effects");
            let y = if offset == 0.0 { 45 } else { 15 };
            let at = (y * 100 + 20) * 4;
            let color = &pixels[at..at + 4];
            assert!(color[0] >= 253, "{effect} at {offset}: {color:?}");
            if effect.starts_with("opacity") {
                assert!(
                    (126..=129).contains(&color[1]) && (126..=129).contains(&color[2]),
                    "{effect} at {offset}: {color:?}"
                );
            } else {
                assert!(
                    color[1] <= 2 && color[2] <= 2,
                    "{effect} at {offset}: {color:?}"
                );
            }
        }
    }
}

#[test]
fn nested_sticky_motion_refreshes_a_filter_with_the_same_scroll_chain() {
    let (mut doc, scroller, sticky) = page(
        ".sticky { height: 70px; filter: blur(1px); background: transparent; }
         .inner { position: sticky; top: 15px; flex-shrink: 0;
                  width: 40px; height: 20px; background: red; }",
    );
    let inner = doc.el(sticky, "view.inner");
    let frame = doc.dom.commit();
    assert_eq!(frame.filter_groups().len(), 1);
    let mut gpu = flashbulb::headless("nested_sticky_filter_refresh");
    for (generation, offset, y) in [(0, 0.0, 45_u16), (1, 80.0, 20)] {
        let offsets =
            |slot: &dom::ScrollSlot| (slot.node == scroller).then_some(Vector2D::new(0.0, offset));
        let filtered = gpu
            .prepare_filters(&frame, &[], &offsets, generation, None)
            .expect("prepare nested sticky filter")
            .to_vec();
        let mut scene = Scene::new();
        frame.compose_into(&mut scene, &[], &filtered, &offsets, None);
        let pixels = gpu
            .render(&scene, &[], 100, 100, Color::WHITE)
            .expect("render nested sticky filter");
        let at = (usize::from(y) * 100 + 20) * 4;
        assert_eq!(&pixels[at..at + 4], &[255, 0, 0, 255]);
        assert_eq!(
            hit(&frame, scroller, 20.0, f32::from(y), offset),
            Some(inner)
        );
        if offset > 0.0 {
            let above = (11 * 100 + 20) * 4;
            assert!(
                pixels[above + 1] >= 250,
                "a cached filter must not leave the child at its parent's y=10"
            );
        }
    }
}
