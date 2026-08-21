//! Behavior tests for `dom::visual`: stacking contexts, Appendix-E paint
//! order, transform matrices, clip chains, and hit testing.

#![allow(clippy::float_cmp)]

use crate::test_common::{self as common, Doc};
use crate::visual::{PaintItemKind, PaintOrder};
use crate::{FontBlob, NodeId, Point2D};

const AHEM: &[u8] = include_bytes!("../../../hughie/tests/fixtures/Ahem.ttf");

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

    fn element_order(&mut self) -> Vec<NodeId> {
        self.paint()
            .items()
            .iter()
            .filter(|item| item.kind == PaintItemKind::ElementBox)
            .map(|item| item.node)
            .collect()
    }

    fn hit(&mut self, x: f32, y: f32) -> Option<NodeId> {
        self.doc.dom.render();
        self.doc
            .dom
            .elements_from_point(Point2D::new(x, y))
            .first()
            .copied()
    }
}

const PAGE: &str = "page { display: flex; position: relative; width: 800px; height: 600px; }";

fn abs_box(extra: &str) -> String {
    format!(
        ".box {{ display: flex; position: absolute; left: 0; top: 0; width: 100px; height: 100px; }} {extra}"
    )
}

#[test]
fn in_flow_content_paints_in_tree_order() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .a, .b, .inner { display: flex; width: 100px; height: 100px; }",
    );
    let root = h.root();
    let a = h.el(root, "view.a");
    let inner = h.el(a, "view.inner");
    let b = h.el(root, "view.b");
    assert_eq!(h.element_order(), vec![root, a, inner, b]);
}

#[test]
fn z_index_orders_positioned_siblings() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(".low { z-index: 1; } .high { z-index: 2; }")
    ));
    let root = h.root();
    let high = h.el(root, "view.box.high");
    let low = h.el(root, "view.box.low");
    assert_eq!(h.element_order(), vec![root, low, high]);
    assert_eq!(h.hit(50.0, 50.0), Some(high));
}

#[test]
fn negative_z_index_paints_below_in_flow_content() {
    let mut h = Harness::new(&format!(
        "{PAGE} {} .flow {{ display: flex; width: 100px; height: 100px; }}",
        abs_box(".neg { z-index: -1; }")
    ));
    let root = h.root();
    let neg = h.el(root, "view.box.neg");
    let flow = h.el(root, "view.flow");
    assert_eq!(h.element_order(), vec![root, neg, flow]);
    assert_eq!(h.hit(50.0, 50.0), Some(flow));
}

#[test]
fn z_index_compares_only_within_the_same_stacking_context() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(".wrap { z-index: 1; } .inner { z-index: 9999; } .top { z-index: 2; }")
    ));
    let root = h.root();
    let wrap = h.el(root, "view.box.wrap");
    let inner = h.el(wrap, "view.box.inner");
    let top = h.el(root, "view.box.top");
    assert_eq!(h.element_order(), vec![root, wrap, inner, top]);
    assert_eq!(h.hit(50.0, 50.0), Some(top));
}

#[test]
fn pseudo_context_members_interleave_with_the_outer_context() {
    let mut h = Harness::new(&format!(
        "{PAGE} {} .d {{ display: flex; width: 50px; height: 50px; }}",
        abs_box(".neg { z-index: -1; }")
    ));
    let root = h.root();
    let wrapper = h.el(root, "view.box");
    let d = h.el(wrapper, "view.d");
    let neg = h.el(wrapper, "view.box.neg");
    assert_eq!(h.element_order(), vec![root, neg, wrapper, d]);
    assert_eq!(h.hit(25.0, 25.0), Some(d));
}

#[test]
fn order_modified_document_order_drives_member_ties() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .item { display: flex; position: relative; width: 100px; height: 100px; }
         .first { order: 2; }
         .second { order: 1; margin-right: -100px; }",
    );
    let root = h.root();
    let first = h.el(root, "view.item.first");
    let second = h.el(root, "view.item.second");
    assert_eq!(h.element_order(), vec![root, second, first]);
    assert_eq!(h.hit(50.0, 50.0), Some(first));
}

#[test]
fn order_is_inert_on_absolutely_positioned_children() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(".first { order: 2; } .second { order: 1; }")
    ));
    let root = h.root();
    let first = h.el(root, "view.box.first");
    let second = h.el(root, "view.box.second");
    assert_eq!(h.element_order(), vec![root, first, second]);
    assert_eq!(h.hit(50.0, 50.0), Some(second));
}

#[test]
fn opacity_context_is_atomic() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(".fade { opacity: 0.5; } .trapped { z-index: 5; } .over { z-index: 1; }")
    ));
    let root = h.root();
    let fade = h.el(root, "view.box.fade");
    let trapped = h.el(fade, "view.box.trapped");
    let over = h.el(root, "view.box.over");
    assert_eq!(h.element_order(), vec![root, fade, trapped, over]);
    assert_eq!(h.hit(50.0, 50.0), Some(over));
}

#[test]
fn static_flex_item_with_z_index_forms_a_context() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .item { display: flex; width: 100px; height: 100px; margin-right: -100px; z-index: 5; }
         .item2 { display: flex; width: 100px; height: 100px; position: relative; z-index: 1; }
         .trapped { display: flex; position: absolute; left: 0; top: 0;
                    width: 100px; height: 100px; z-index: 9; }",
    );
    let root = h.root();
    let item = h.el(root, "view.item");
    let trapped = h.el(item, "view.trapped");
    let item2 = h.el(root, "view.item2");
    assert_eq!(h.element_order(), vec![root, item2, item, trapped]);
    assert_eq!(h.hit(50.0, 50.0), Some(trapped));
}

#[test]
fn will_change_and_containment_create_atomic_contexts() {
    for trigger in [
        "will-change: transform;",
        "will-change: opacity;",
        "perspective: 100px;",
        "filter: grayscale(1);",
        "transform: translate(0px, 0px);",
    ] {
        let mut h = Harness::new(&format!(
            "{PAGE} {}",
            abs_box(&format!(
                ".t {{ {trigger} }} .trapped {{ z-index: 5; }} .over {{ z-index: 1; }}"
            ))
        ));
        let root = h.root();
        let t = h.el(root, "view.box.t");
        let trapped = h.el(t, "view.box.trapped");
        let over = h.el(root, "view.box.over");
        assert_eq!(
            h.element_order(),
            vec![root, t, trapped, over],
            "trigger `{trigger}` must make an atomic stacking context",
        );
    }
}

#[test]
fn contain_paint_creates_a_context_and_clips() {
    let mut h = Harness::new(&format!("{PAGE} {}", abs_box("")));
    let root = h.root();
    let contained = h.el(root, "view.box");
    h.doc.set_inline(contained, "contain: paint");
    let trapped = h.el(contained, "view.box.trapped");
    h.doc.set_inline(trapped, "z-index: 5; left: 150px");
    let over = h.el(root, "view.box.over");
    h.doc.set_inline(over, "z-index: 1");
    assert_eq!(h.element_order(), vec![root, contained, trapped, over]);
    assert_eq!(h.hit(50.0, 50.0), Some(over));
    assert_eq!(h.hit(170.0, 50.0), Some(root));
}

#[test]
fn fixed_position_forms_a_context_and_reanchors_to_transformed_ancestors() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .mover { display: flex; position: absolute; left: 100px; top: 100px;
                  width: 200px; height: 200px; transform: translate(50px, 0px); }
         .fixed { display: flex; position: fixed; left: 10px; top: 10px;
                  width: 50px; height: 50px; }",
    );
    let root = h.root();
    let mover = h.el(root, "view.mover");
    let fixed = h.el(mover, "view.fixed");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == fixed)
        .expect("fixed box paints");
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .expect("affine world matrix");
    assert_eq!((mapped.x, mapped.y), (160.0, 110.0));
    assert_eq!(h.hit(170.0, 120.0), Some(fixed));
}

#[test]
fn overflow_hidden_clips_paint_and_hits_of_in_flow_descendants() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .clipper { display: flex; overflow: hidden; width: 50px; height: 50px; }
         .big { display: flex; flex-shrink: 0; width: 200px; height: 200px; }",
    );
    let root = h.root();
    let clipper = h.el(root, "view.clipper");
    let big = h.el(clipper, "view.big");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == big)
        .expect("clipped content still paints");
    let clip = &paint.clips()[item.clip.expect("descendant carries the clip")];
    assert_eq!(clip.node, clipper);
    assert_eq!(h.hit(25.0, 25.0), Some(big));
    assert_eq!(h.hit(100.0, 25.0), Some(root));
}

#[test]
fn absolute_boxes_escape_clips_outside_their_containing_block_chain() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .w { display: flex; overflow: hidden; width: 50px; height: 50px; }
         .x { display: flex; position: absolute; left: 100px; top: 0;
              width: 50px; height: 50px; }
         .d { display: flex; flex-shrink: 0; width: 50px; height: 50px; }
         .g { display: flex; position: absolute; left: 0; top: 0;
              width: 25px; height: 25px; }",
    );
    let root = h.root();
    let clipper = h.el(root, "view.w");
    let escaper = h.el(clipper, "view.x");
    let static_child = h.el(escaper, "view.d");
    let abs_child = h.el(escaper, "view.g");
    assert_eq!(h.hit(140.0, 40.0), Some(static_child));
    assert_eq!(h.hit(110.0, 10.0), Some(abs_child));
    let paint = h.paint();
    for id in [escaper, static_child, abs_child] {
        let item = paint.items().iter().find(|item| item.node == id).unwrap();
        assert_eq!(item.clip, None, "escaping box must not carry the clip");
    }
}

#[test]
fn absolute_boxes_are_clipped_by_their_own_containing_block() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .r { display: flex; position: relative; overflow: hidden;
              width: 100px; height: 100px; }
         .w { display: flex; width: 100px; height: 100px; }
         .a { display: flex; position: absolute; left: 0; top: 150px;
              width: 50px; height: 50px; }",
    );
    let root = h.root();
    let clipping_block = h.el(root, "view.r");
    let wrapper = h.el(clipping_block, "view.w");
    let out_of_bounds = h.el(wrapper, "view.a");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == out_of_bounds)
        .unwrap();
    let clip = &paint.clips()[item.clip.expect("clipped by its containing block")];
    assert_eq!(clip.node, clipping_block);
    assert_eq!(h.hit(10.0, 160.0), Some(root));
}

#[test]
fn rotation_rotates_the_hit_region_about_the_transform_origin() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .rot { display: flex; position: absolute; left: 100px; top: 0;
                width: 100px; height: 100px; transform: rotate(90deg);
                transform-origin: 0 0; }",
    );
    let root = h.root();
    let rotated = h.el(root, "view.rot");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == rotated)
        .unwrap();
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(100.0, 0.0))
        .expect("rotation is invertible");
    assert!((mapped.x - 100.0).abs() < 1e-4 && (mapped.y - 100.0).abs() < 1e-4);
    assert_eq!(h.hit(50.0, 50.0), Some(rotated));
    assert_eq!(h.hit(150.0, 50.0), Some(root));
}

#[test]
fn percentage_translate_resolves_against_the_border_box() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .pct { display: flex; position: absolute; left: 0; top: 0;
                width: 200px; height: 100px; transform: translate(50%, 50%); }",
    );
    let root = h.root();
    let pct = h.el(root, "view.pct");
    assert_eq!(h.hit(110.0, 60.0), Some(pct));
    assert_eq!(h.hit(10.0, 10.0), Some(root));
}

#[test]
fn scale_zero_is_not_hittable() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(".flat { transform: scale(0); }")
    ));
    let root = h.root();
    let flat = h.el(root, "view.box.flat");
    assert_eq!(h.hit(50.0, 50.0), Some(root));
    let _ = flat;
}

#[test]
fn perspective_projects_children_about_the_parent_center() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .p { display: flex; position: absolute; left: 0; top: 0;
              width: 200px; height: 200px; perspective: 100px; }
         .c { display: flex; position: absolute; left: 75px; top: 75px;
              width: 50px; height: 50px; transform: translateZ(50px); }",
    );
    let root = h.root();
    let p = h.el(root, "view.p");
    let c = h.el(p, "view.c");
    let paint = h.paint();
    let item = paint.items().iter().find(|item| item.node == c).unwrap();
    let top_left = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    let bottom_right = item
        .transform
        .transform_point2d(Point2D::new(50.0, 50.0))
        .unwrap();
    assert!((top_left.x - 50.0).abs() < 1e-3 && (top_left.y - 50.0).abs() < 1e-3);
    assert!((bottom_right.x - 150.0).abs() < 1e-3 && (bottom_right.y - 150.0).abs() < 1e-3);
    assert_eq!(h.hit(140.0, 140.0), Some(c));
    let _ = p;
}

#[test]
fn pointer_events_none_falls_through_and_descendants_reenable() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}
         .ghost {{ pointer-events: none; }}
         .solid {{ pointer-events: auto; width: 50px; height: 50px; }}",
        abs_box("")
    ));
    let root = h.root();
    let under = h.el(root, "view.box");
    let ghost = h.el(root, "view.box.ghost");
    let solid = h.el(ghost, "view.solid");
    assert_eq!(h.hit(25.0, 25.0), Some(solid));
    assert_eq!(h.hit(75.0, 75.0), Some(under));
}

#[test]
fn visibility_hidden_skips_the_box_but_not_visible_descendants() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}
         .hidden {{ visibility: hidden; }}
         .shown {{ visibility: visible; width: 50px; height: 50px; }}",
        abs_box("")
    ));
    let root = h.root();
    let hidden = h.el(root, "view.box.hidden");
    let shown = h.el(hidden, "view.shown");
    let paint = h.paint();
    assert!(
        paint.items().iter().all(|item| item.node != hidden),
        "a hidden box paints nothing",
    );
    assert_eq!(h.hit(25.0, 25.0), Some(shown));
    assert_eq!(h.hit(75.0, 75.0), Some(root));
}

#[test]
fn border_radius_rounds_the_hit_region() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(".round { border-radius: 50px; }")
    ));
    let root = h.root();
    let round = h.el(root, "view.box.round");
    assert_eq!(h.hit(50.0, 50.0), Some(round));
    assert_eq!(h.hit(5.0, 5.0), Some(root));
}

#[test]
fn clip_border_radius_rounds_descendant_hit_regions() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .clipper { display: flex; overflow: hidden; border-radius: 50px;
                    width: 100px; height: 100px; }
         .fill { display: flex; flex-shrink: 0; width: 100px; height: 100px; }",
    );
    let root = h.root();
    let clipper = h.el(root, "view.clipper");
    let fill = h.el(clipper, "view.fill");
    assert_eq!(h.hit(50.0, 50.0), Some(fill));
    assert_eq!(h.hit(5.0, 5.0), Some(root));
    let _ = clipper;
}

#[test]
fn text_runs_paint_with_their_element_and_hit_as_the_element() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .label { display: flex; width: 200px; height: 50px;
                  font-family: Ahem; font-size: 20px; }",
    );
    h.doc.dom.register_fonts(FontBlob::from_static(AHEM));
    let root = h.root();
    let label = h.el(root, "view.label");
    let text = h.doc.dom.create_text_node("hello", ());
    h.doc.dom.append_child(label, text);
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == text)
        .expect("the text leaf paints as its own run");
    assert_eq!(item.kind, PaintItemKind::TextRun { element: label });
    assert!(item.size.width > 0.0 && item.size.height > 0.0);
    assert_eq!(h.hit(10.0, 10.0), Some(label));
    assert_eq!(
        h.doc.dom.elements_from_point(Point2D::new(10.0, 10.0)),
        vec![label, root]
    );
}

#[test]
fn hit_outside_all_content_is_none() {
    let mut h = Harness::new("page { display: flex; width: 100px; height: 100px; }");
    assert_eq!(h.hit(400.0, 400.0), None);
}

#[test]
fn a_bare_document_paints_only_its_permanent_root() {
    let mut doc: dom::Document<()> = dom::Document::new(common::device(800.0, 600.0), "page", ());
    let root = doc.document_element().id();
    let paint = doc.build_paint_order();
    let painted: Vec<_> = paint.items().iter().map(|item| item.node).collect();
    assert_eq!(painted, [root]);
}

#[test]
fn display_none_subtrees_neither_paint_nor_hit() {
    let mut h = Harness::new(&format!(
        "{PAGE} {} .gone {{ display: none; }}",
        abs_box("")
    ));
    let root = h.root();
    let gone = h.el(root, "view.box.gone");
    let child = h.el(gone, "view.box");
    let paint = h.paint();
    assert!(
        paint
            .items()
            .iter()
            .all(|item| item.node != gone && item.node != child)
    );
    assert_eq!(h.hit(50.0, 50.0), Some(root));
}

#[test]
fn sticky_position_forms_a_stacking_context() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(
            ".stick { position: sticky; left: 0; top: 0; } .trapped { z-index: 5; } .over { z-index: 1; }"
        )
    ));
    let root = h.root();
    let stick = h.el(root, "view.box.stick");
    let trapped = h.el(stick, "view.box.trapped");
    let over = h.el(root, "view.box.over");
    assert_eq!(h.element_order(), vec![root, stick, trapped, over]);
    assert_eq!(h.hit(50.0, 50.0), Some(over));
}

#[test]
fn fixed_position_escapes_static_clippers_to_the_viewport() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .clipper { display: flex; overflow: hidden; width: 50px; height: 50px; }
         .fix { display: flex; position: fixed; left: 100px; top: 100px;
                width: 50px; height: 50px; }",
    );
    let root = h.root();
    let clipper = h.el(root, "view.clipper");
    let fix = h.el(clipper, "view.fix");
    let paint = h.paint();
    let item = paint.items().iter().find(|item| item.node == fix).unwrap();
    assert_eq!(item.clip, None, "no fixed-CB ancestor: every clip escaped");
    assert_eq!(h.hit(110.0, 110.0), Some(fix));
    let _ = clipper;
}

#[test]
fn clip_chains_link_across_nested_clippers() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .outer { display: flex; overflow: hidden; width: 100px; height: 100px; }
         .inner { display: flex; overflow: hidden; flex-shrink: 0;
                  width: 200px; height: 50px; }
         .big { display: flex; flex-shrink: 0; width: 300px; height: 300px; }",
    );
    let root = h.root();
    let outer = h.el(root, "view.outer");
    let inner = h.el(outer, "view.inner");
    let big = h.el(inner, "view.big");
    let paint = h.paint();
    let item = paint.items().iter().find(|item| item.node == big).unwrap();
    let inner_clip = &paint.clips()[item.clip.expect("clipped by inner")];
    assert_eq!(inner_clip.node, inner);
    let outer_clip = &paint.clips()[inner_clip.parent.expect("chained to outer")];
    assert_eq!(outer_clip.node, outer);
    assert_eq!(outer_clip.parent, None);
    assert_eq!(h.hit(150.0, 25.0), Some(root));
    assert_eq!(h.hit(75.0, 25.0), Some(big));
    assert_eq!(h.hit(75.0, 75.0), Some(outer));
}

#[test]
fn a_transformed_clipper_carries_its_clip_along() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .mover { display: flex; position: absolute; left: 0; top: 0;
                  width: 100px; height: 100px; overflow: hidden;
                  transform: translate(100px, 0px); }
         .big { display: flex; flex-shrink: 0; width: 200px; height: 200px; }",
    );
    let root = h.root();
    let mover = h.el(root, "view.mover");
    let big = h.el(mover, "view.big");
    assert_eq!(h.hit(150.0, 50.0), Some(big));
    assert_eq!(h.hit(50.0, 50.0), Some(root));
    let _ = mover;
}

#[test]
fn rotate_x_flattens_about_the_default_center_origin() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .rx { display: flex; position: absolute; left: 0; top: 0;
               width: 100px; height: 100px; transform: rotateX(60deg); }",
    );
    let root = h.root();
    let rx = h.el(root, "view.rx");
    let paint = h.paint();
    let item = paint.items().iter().find(|item| item.node == rx).unwrap();
    let top = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    let bottom = item
        .transform
        .transform_point2d(Point2D::new(0.0, 100.0))
        .unwrap();
    assert!((top.x).abs() < 1e-3 && (top.y - 25.0).abs() < 1e-3);
    assert!((bottom.x).abs() < 1e-3 && (bottom.y - 75.0).abs() < 1e-3);
    assert_eq!(h.hit(50.0, 50.0), Some(rx));
    assert_eq!(h.hit(50.0, 10.0), Some(root));
}

#[test]
fn transform_origin_defaults_to_the_border_box_center() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .rot { display: flex; position: absolute; left: 100px; top: 0;
                width: 200px; height: 100px; transform: rotate(90deg); }",
    );
    let root = h.root();
    let rotated = h.el(root, "view.rot");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == rotated)
        .unwrap();
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!((mapped.x - 250.0).abs() < 1e-3 && (mapped.y + 50.0).abs() < 1e-3);
    assert_eq!(h.hit(200.0, 50.0), Some(rotated));
    assert_eq!(h.hit(120.0, 50.0), Some(root));
}

#[test]
fn a_clipping_pseudo_inside_an_escaping_pseudo_starts_a_fresh_chain() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .w { display: flex; overflow: hidden; width: 50px; height: 50px; }
         .x { display: flex; position: absolute; left: 100px; top: 0;
              width: 100px; height: 100px; overflow: hidden; }
         .y { display: flex; flex-shrink: 0; width: 200px; height: 200px; }",
    );
    let root = h.root();
    let wrapper = h.el(root, "view.w");
    let escaper = h.el(wrapper, "view.x");
    let filler = h.el(escaper, "view.y");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == filler)
        .unwrap();
    let clip = &paint.clips()[item.clip.expect("clipped by the escaping pseudo")];
    assert_eq!(clip.node, escaper);
    assert_eq!(
        clip.parent, None,
        "the escaped wrapper clip must not chain in"
    );
    assert_eq!(h.hit(150.0, 50.0), Some(filler));
    assert_eq!(h.hit(210.0, 50.0), Some(root));
}

#[test]
fn sticky_boxes_stay_in_the_normal_clip_flow() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .clipper { display: flex; overflow: hidden; width: 50px; height: 50px; }
         .stick { display: flex; position: sticky; left: 0; top: 0;
                  flex-shrink: 0; width: 200px; height: 200px; }",
    );
    let root = h.root();
    let clipper = h.el(root, "view.clipper");
    let stick = h.el(clipper, "view.stick");
    assert_eq!(h.hit(25.0, 25.0), Some(stick));
    assert_eq!(h.hit(100.0, 25.0), Some(root));
    let _ = clipper;
}

#[test]
fn a_hidden_context_root_still_structures_and_clips() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .fade { display: flex; position: absolute; left: 0; top: 0;
                 width: 100px; height: 100px; opacity: 0.5;
                 visibility: hidden; overflow: hidden; }
         .shown { display: flex; visibility: visible; flex-shrink: 0;
                  width: 200px; height: 200px; }",
    );
    let root = h.root();
    let fade = h.el(root, "view.fade");
    let shown = h.el(fade, "view.shown");
    let paint = h.paint();
    assert!(paint.items().iter().all(|item| item.node != fade));
    assert_eq!(h.hit(50.0, 50.0), Some(shown));
    assert_eq!(h.hit(150.0, 50.0), Some(root));
}

#[test]
fn pointer_events_none_on_a_context_root_lets_auto_descendants_hit() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}
         .ghost {{ opacity: 0.5; pointer-events: none; }}
         .solid {{ pointer-events: auto; width: 50px; height: 50px; }}",
        abs_box("")
    ));
    let root = h.root();
    let under = h.el(root, "view.box");
    let ghost = h.el(root, "view.box.ghost");
    let solid = h.el(ghost, "view.solid");
    assert_eq!(h.hit(25.0, 25.0), Some(solid));
    assert_eq!(h.hit(75.0, 75.0), Some(under));
}

#[test]
fn text_runs_are_clipped_by_their_element() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .clipper { display: flex; overflow: hidden; width: 60px; height: 20px;
                    font-family: Ahem; font-size: 20px; }",
    );
    h.doc.dom.register_fonts(FontBlob::from_static(AHEM));
    let root = h.root();
    let clipper = h.el(root, "view.clipper");
    let text = h.doc.dom.create_text_node("hellohello", ());
    h.doc.dom.append_child(clipper, text);
    let paint = h.paint();
    let item = paint.items().iter().find(|item| item.node == text).unwrap();
    let clip = &paint.clips()[item.clip.expect("text is clipped by its element")];
    assert_eq!(clip.node, clipper);
    assert!(
        item.size.width > 60.0,
        "the run itself is wider than the clip"
    );
    assert_eq!(h.hit(30.0, 10.0), Some(clipper));
    assert_eq!(h.hit(100.0, 10.0), Some(root));
}

#[test]
fn shared_edges_resolve_by_paint_order_and_trailing_edges_miss() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .cell { display: flex; width: 100px; height: 100px; }",
    );
    let root = h.root();
    let first = h.el(root, "view.cell");
    let second = h.el(root, "view.cell");
    assert_eq!(h.hit(100.0, 50.0), Some(second));
    assert_eq!(h.hit(200.0, 50.0), Some(root));
    assert_eq!(h.hit(400.0, 600.0), None);
    let _ = first;
}

#[test]
fn perspective_skips_non_direct_descendants() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .p { display: flex; position: absolute; left: 0; top: 0;
              width: 200px; height: 200px; perspective: 100px; }
         .mid { display: flex; position: absolute; left: 75px; top: 75px;
                width: 50px; height: 50px; }
         .deep { display: flex; position: absolute; left: 0; top: 0;
                 width: 50px; height: 50px; transform: translateZ(50px); }",
    );
    let root = h.root();
    let parent = h.el(root, "view.p");
    let mid = h.el(parent, "view.mid");
    let deep = h.el(mid, "view.deep");
    let paint = h.paint();
    let item = paint.items().iter().find(|item| item.node == deep).unwrap();
    let top_left = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    let bottom_right = item
        .transform
        .transform_point2d(Point2D::new(50.0, 50.0))
        .unwrap();
    assert!((top_left.x - 75.0).abs() < 1e-3 && (top_left.y - 75.0).abs() < 1e-3);
    assert!((bottom_right.x - 125.0).abs() < 1e-3 && (bottom_right.y - 125.0).abs() < 1e-3);
    assert_eq!(h.hit(140.0, 140.0), Some(parent));
}

#[test]
fn context_member_inside_a_pseudo_inside_a_negative_member_stays_atomic() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}
         .neg {{ z-index: -1; }}
         .opaq {{ opacity: 0.5; }}
         .trapped {{ z-index: 99; }}
         .flow {{ display: flex; width: 100px; height: 100px; }}",
        abs_box("")
    ));
    let root = h.root();
    let neg = h.el(root, "view.box.neg");
    let pseudo = h.el(neg, "view.box");
    let opaq = h.el(pseudo, "view.box.opaq");
    let trapped = h.el(opaq, "view.box.trapped");
    let flow = h.el(root, "view.flow");
    assert_eq!(
        h.element_order(),
        vec![root, neg, pseudo, opaq, trapped, flow]
    );
    assert_eq!(h.hit(50.0, 50.0), Some(flow));
}

#[test]
fn display_contents_paints_no_own_box() {
    let mut h = Harness::new(&format!(
        "{PAGE} {} .contents {{ display: contents; }}",
        abs_box("")
    ));
    let root = h.root();
    let contents = h.el(root, "view.contents");
    let paint = h.paint();
    assert!(
        paint.items().iter().all(|item| item.node != contents),
        "a display:contents element generates no box and paints nothing",
    );
    assert_eq!(h.hit(50.0, 50.0), Some(root));
}

#[test]
fn contents_children_paint_and_hit_in_the_outer_context() {
    let mut h = Harness::new(&format!(
        "{PAGE} {} .wrap {{ display: contents; }}",
        abs_box("")
    ));
    let root = h.root();
    let wrap = h.el(root, "view.wrap");
    let child = h.el(wrap, "view.box");
    let paint = h.paint();
    assert!(
        paint.items().iter().all(|item| item.node != wrap),
        "the contents element still paints no own box",
    );
    assert!(paint.items().iter().any(|item| item.node == child));
    assert_eq!(h.hit(50.0, 50.0), Some(child));
}

#[test]
fn contents_elements_never_form_stacking_contexts() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}
         .wrap {{ display: contents; opacity: 0.5; z-index: 10; }}
         .hi {{ z-index: 5; }} .lo {{ z-index: 1; }}",
        abs_box("")
    ));
    let root = h.root();
    let wrap = h.el(root, "view.wrap");
    let hi = h.el(wrap, "view.box.hi");
    let lo = h.el(root, "view.box.lo");
    assert_eq!(h.element_order(), vec![root, lo, hi]);
    assert_eq!(h.hit(50.0, 50.0), Some(hi));
    let _ = wrap;
}

#[test]
fn visibility_and_pointer_events_inherit_through_contents() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}
         .hidden-wrap {{ display: contents; visibility: hidden; }}
         .shown {{ visibility: visible; width: 50px; height: 50px; }}",
        abs_box("")
    ));
    let root = h.root();
    let wrap = h.el(root, "view.hidden-wrap");
    let ghost = h.el(wrap, "view.box");
    let shown = h.el(ghost, "view.shown");
    assert_eq!(h.hit(25.0, 25.0), Some(shown));
    assert_eq!(h.hit(75.0, 75.0), Some(root));

    let mut h2 = Harness::new(&format!(
        "{PAGE} {}
         .ghost-wrap {{ display: contents; pointer-events: none; }}
         .solid {{ pointer-events: auto; width: 50px; height: 50px; }}",
        abs_box("")
    ));
    let root2 = h2.root();
    let under = h2.el(root2, "view.box");
    let wrap2 = h2.el(root2, "view.ghost-wrap");
    let through = h2.el(wrap2, "view.box");
    let solid = h2.el(through, "view.solid");
    assert_eq!(h2.hit(25.0, 25.0), Some(solid));
    assert_eq!(h2.hit(75.0, 75.0), Some(under));
}

#[test]
fn text_in_contents_hits_the_contents_element() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px;
                font-family: Ahem; font-size: 20px; }
         .wrap { display: contents; }",
    );
    h.doc.dom.register_fonts(FontBlob::from_static(AHEM));
    let root = h.root();
    let wrap = h.el(root, "view.wrap");
    let text = h.doc.dom.create_text_node("hello", ());
    h.doc.dom.append_child(wrap, text);
    let paint = h.paint();
    let item = paint.items().iter().find(|item| item.node == text).unwrap();
    assert_eq!(item.kind, PaintItemKind::TextRun { element: wrap });
    assert_eq!(h.hit(10.0, 10.0), Some(wrap));
}

#[test]
fn transform_and_overflow_are_inert_on_contents_elements() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .wrap { display: contents; transform: translate(100px, 0px);
                 overflow: hidden; }
         .big { display: flex; width: 200px; height: 200px; }",
    );
    let root = h.root();
    let wrap = h.el(root, "view.wrap");
    let big = h.el(wrap, "view.big");
    let paint = h.paint();
    let item = paint.items().iter().find(|item| item.node == big).unwrap();
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert_eq!((mapped.x, mapped.y), (0.0, 0.0));
    assert_eq!(item.clip, None);
    assert_eq!(h.hit(150.0, 150.0), Some(big));
    let _ = wrap;
}

#[test]
fn fixed_child_of_contents_escapes_clips() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .clipper { display: flex; overflow: hidden; width: 50px; height: 50px; }
         .wrap { display: contents; }
         .fix { display: flex; position: fixed; left: 100px; top: 100px;
                width: 50px; height: 50px; }",
    );
    let root = h.root();
    let clipper = h.el(root, "view.clipper");
    let wrap = h.el(clipper, "view.wrap");
    let fix = h.el(wrap, "view.fix");
    let paint = h.paint();
    let item = paint.items().iter().find(|item| item.node == fix).unwrap();
    assert_eq!(item.clip, None);
    assert_eq!(h.hit(110.0, 110.0), Some(fix));
    let _ = clipper;
}

#[test]
fn perspective_applies_through_contents_wrappers() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .p { display: flex; position: absolute; left: 0; top: 0;
              width: 200px; height: 200px; perspective: 100px; }
         .wrap { display: contents; }
         .c { display: flex; position: absolute; left: 75px; top: 75px;
              width: 50px; height: 50px; transform: translateZ(50px); }",
    );
    let root = h.root();
    let parent = h.el(root, "view.p");
    let wrap = h.el(parent, "view.wrap");
    let c = h.el(wrap, "view.c");
    let paint = h.paint();
    let item = paint.items().iter().find(|item| item.node == c).unwrap();
    let top_left = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!((top_left.x - 50.0).abs() < 1e-3 && (top_left.y - 50.0).abs() < 1e-3);
    assert_eq!(h.hit(140.0, 140.0), Some(c));
    let _ = (parent, wrap);
}

#[test]
fn absolute_child_of_contents_is_clipped_by_its_containing_block() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .clipper { display: flex; position: relative; overflow: hidden;
                    width: 50px; height: 50px; }
         .wrap { display: contents; }
         .abs { display: flex; position: absolute; left: 100px; top: 100px;
                width: 50px; height: 50px; }",
    );
    let root = h.root();
    let clipper = h.el(root, "view.clipper");
    let wrap = h.el(clipper, "view.wrap");
    let abs = h.el(wrap, "view.abs");
    let paint = h.paint();
    let item = paint.items().iter().find(|item| item.node == abs).unwrap();
    let clip = &paint.clips()[item.clip.expect("clipped by the containing block")];
    assert_eq!(clip.node, clipper);
    assert_eq!(h.hit(110.0, 110.0), Some(root));
    let _ = wrap;
}

#[test]
fn contents_order_interleave_paints_and_survives_hidden_siblings() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 100px; }
         .wrap { display: contents; }
         .cell { display: flex; width: 100px; height: 100px; }
         .gone { display: none; }
         .o0 { order: 0; } .o1 { order: 1; } .o2 { order: 2; } .o3 { order: 3; }",
    );
    let root = h.root();
    let second = h.el(root, "view.cell.o2");
    let wrap = h.el(root, "view.wrap");
    let inner_first = h.el(wrap, "view.cell.o1");
    let hidden = h.el(wrap, "view.cell.gone");
    let inner_last = h.el(wrap, "view.cell.o3");
    let leading = h.el(root, "view.cell.o0");
    assert_eq!(
        h.element_order(),
        vec![root, leading, inner_first, second, inner_last]
    );
    let _ = hidden;
}

/// A removal drops its own node out of the retained frame's answers and
/// leaves the rest of the frame answering. The replacement built on the freed
/// node's storage is not in that frame at all, so it only appears once the
/// next render puts it there — it can never be mistaken for its predecessor,
/// because it carries a different id.
#[test]
fn a_removal_drops_only_its_own_node_from_the_retained_frame() {
    let mut h = Harness::new(&format!("{PAGE} {}", abs_box("")));
    let root = h.root();
    let removed = h.el(root, "view.box");
    h.doc.dom.render();
    assert_eq!(
        h.doc.dom.elements_from_point(Point2D::new(50.0, 50.0)),
        vec![removed, root]
    );

    h.doc.dom.drop_subtree(removed);
    let replacement = h.el(root, "view.box");
    assert_ne!(replacement, removed, "the removed id is retired");
    assert_eq!(
        h.doc.dom.elements_from_point(Point2D::new(50.0, 50.0)),
        vec![root],
        "the removed node is skipped while every other item still answers"
    );

    h.doc.dom.render();
    assert_eq!(
        h.doc.dom.elements_from_point(Point2D::new(50.0, 50.0)),
        vec![replacement, root]
    );
}

#[test]
fn hit_queries_read_the_rendered_frame_without_rebuilding_it() {
    let mut h = Harness::new(&format!("{PAGE} {}", abs_box("")));
    let root = h.root();
    let target = h.el(root, "view.box");
    h.doc.dom.render();
    assert_eq!(
        h.doc.dom.elements_from_point(Point2D::new(50.0, 50.0)),
        vec![target, root]
    );
    h.doc.dom.set_inline_style(target, "left: 200px");
    assert_eq!(
        h.doc.dom.elements_from_point(Point2D::new(50.0, 50.0)),
        vec![target, root]
    );
    h.doc.dom.render();
    assert_eq!(
        h.doc.dom.elements_from_point(Point2D::new(50.0, 50.0)),
        vec![root]
    );
    assert_eq!(
        h.doc.dom.elements_from_point(Point2D::new(250.0, 50.0)),
        vec![target, root]
    );
}

#[test]
fn a_document_that_never_rendered_answers_hit_queries_with_nothing() {
    let mut h = Harness::new(&format!("{PAGE} {}", abs_box("")));
    let root = h.root();
    let boxed = h.el(root, "view.box");
    h.doc.dom.layout();
    let _ = boxed;
    assert_eq!(
        h.doc.dom.elements_from_point(Point2D::new(50.0, 50.0)),
        Vec::new()
    );
}

#[test]
fn elements_from_points_answers_each_point_from_one_frame() {
    let mut h = Harness::new(&format!("{PAGE} {}", abs_box("")));
    let root = h.root();
    let target = h.el(root, "view.box");
    h.doc.dom.render();
    let points = [
        Point2D::new(50.0, 50.0),
        Point2D::new(250.0, 50.0),
        Point2D::new(10_000.0, 50.0),
    ];
    let batch = h.doc.dom.elements_from_points(&points);
    let singles: Vec<_> = points
        .iter()
        .map(|point| h.doc.dom.elements_from_point(*point))
        .collect();
    assert_eq!(batch, singles);
    assert_eq!(batch[0], vec![target, root]);
    assert_eq!(batch[1], vec![root]);
    assert_eq!(batch[2], Vec::new());
}

#[test]
fn offset_path_translates_the_anchor_along_the_path() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(
            r#".mover { offset-path: path("M 0 0 L 100 0"); offset-distance: 100%; offset-rotate: 0deg; }"#
        )
    ));
    let root = h.root();
    let mover = h.el(root, "view.box.mover");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == mover)
        .unwrap();
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!((mapped.x - 50.0).abs() < 1e-3 && (mapped.y - -50.0).abs() < 1e-3);
    assert_eq!(h.hit(100.0, 25.0), Some(mover));
    assert_eq!(h.hit(25.0, 75.0), Some(root));
}

#[test]
fn offset_rotate_auto_follows_the_path_direction() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(r#".mover { offset-path: path("M 0 0 L 0 100"); offset-distance: 50%; }"#)
    ));
    let root = h.root();
    let mover = h.el(root, "view.box.mover");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == mover)
        .unwrap();
    let corner = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!((corner.x - 50.0).abs() < 1e-3 && corner.y.abs() < 1e-3);
    let far = item
        .transform
        .transform_point2d(Point2D::new(100.0, 0.0))
        .unwrap();
    assert!((far.x - 50.0).abs() < 1e-3 && (far.y - 100.0).abs() < 1e-3);
    let _ = root;
}

#[test]
fn offset_rotate_fixed_angles_ignore_the_path_direction() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(
            r#".mover { offset-path: path("M 0 0 L 100 0"); offset-distance: 0; offset-rotate: 90deg; }"#
        )
    ));
    let root = h.root();
    let mover = h.el(root, "view.box.mover");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == mover)
        .unwrap();
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!(
        (mapped.x - 50.0).abs() < 1e-3 && (mapped.y - -50.0).abs() < 1e-3,
        "mapped = ({}, {})",
        mapped.x,
        mapped.y,
    );
    let _ = root;
}

#[test]
fn offset_rotate_reverse_is_not_in_the_lynx_grammar() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(
            r#".mover { offset-path: path("M 0 0 L 0 100"); offset-distance: 50%; offset-rotate: reverse; }"#
        )
    ));
    let root = h.root();
    let mover = h.el(root, "view.box.mover");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == mover)
        .unwrap();
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!(
        (mapped.x - 50.0).abs() < 1e-3 && mapped.y.abs() < 1e-3,
        "mapped = ({}, {})",
        mapped.x,
        mapped.y,
    );
    let _ = root;
}

#[test]
fn closed_paths_wrap_and_open_paths_clamp_the_offset_distance() {
    let mut h = Harness::new(&format!(
        r#"{PAGE} {}
         .wrap {{ offset-path: path("M 0 0 L 100 0 L 100 100 L 0 100 Z");
                  offset-distance: 550px; offset-rotate: 0deg; }}
         .clamp {{ offset-path: path("M 0 0 L 100 0"); offset-distance: 250px;
                   offset-rotate: 0deg; }}"#,
        abs_box("")
    ));
    let root = h.root();
    let wrapping = h.el(root, "view.box.wrap");
    let clamped = h.el(root, "view.box.clamp");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == wrapping)
        .unwrap();
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!((mapped.x - 50.0).abs() < 1e-3 && mapped.y.abs() < 1e-3);
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == clamped)
        .unwrap();
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!((mapped.x - 50.0).abs() < 1e-3 && (mapped.y - -50.0).abs() < 1e-3);
}

#[test]
fn circle_paths_start_rightmost_and_run_clockwise() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(".mover { offset-path: circle(50px); offset-distance: 25%; offset-rotate: 0deg; }")
    ));
    let root = h.root();
    let mover = h.el(root, "view.box.mover");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == mover)
        .unwrap();
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!(mapped.x.abs() < 0.5 && (mapped.y - 50.0).abs() < 0.5);
    let _ = root;
}

#[test]
fn inset_paths_start_at_the_top_edge_and_run_clockwise() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}
         .start {{ offset-path: inset(10px); offset-distance: 0; offset-rotate: 0deg; }}
         .quarter {{ offset-path: inset(10px); offset-distance: 25%; offset-rotate: 0deg; }}",
        abs_box("")
    ));
    let root = h.root();
    let start = h.el(root, "view.box.start");
    let quarter = h.el(root, "view.box.quarter");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == start)
        .unwrap();
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!((mapped.x - -40.0).abs() < 1e-3 && (mapped.y - -40.0).abs() < 1e-3);
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == quarter)
        .unwrap();
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!((mapped.x - 40.0).abs() < 1e-3 && (mapped.y - -40.0).abs() < 1e-3);
}

#[test]
fn svg_arc_commands_flatten_within_tolerance() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(
            r#".mover { offset-path: path("M 0 50 A 50 50 0 0 1 100 50"); offset-distance: 50%; offset-rotate: 0deg; }"#
        )
    ));
    let root = h.root();
    let mover = h.el(root, "view.box.mover");
    let paint = h.paint();
    let item = paint
        .items()
        .iter()
        .find(|item| item.node == mover)
        .unwrap();
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!(mapped.x.abs() < 0.5 && (mapped.y - -50.0).abs() < 0.5);
    let _ = root;
}

fn item_index(paint: &PaintOrder, node: NodeId) -> usize {
    paint
        .items()
        .iter()
        .position(|item| item.node == node && item.kind == PaintItemKind::ElementBox)
        .expect("node must have an element-box item")
}

#[test]
fn group_effect_contexts_get_render_layers() {
    for trigger in [
        "opacity: 0.5;",
        "filter: grayscale(1);",
        "clip-path: circle(40px);",
    ] {
        let mut h = Harness::new(&format!(
            "{PAGE} {}",
            abs_box(&format!(".fade {{ {trigger} }} .over {{ z-index: 1; }}"))
        ));
        let root = h.root();
        let fade = h.el(root, "view.box.fade");
        let inner = h.el(fade, "view.box");
        let over = h.el(root, "view.box.over");
        let paint = h.paint();

        let layers = paint.layers();
        assert_eq!(layers.len(), 1, "trigger `{trigger}` must form one layer");
        let layer = &layers[0];
        assert_eq!(layer.node, fade);
        assert_eq!(layer.parent, None);
        assert_eq!(layer.items.start, item_index(&paint, fade));
        assert!(layer.items.contains(&item_index(&paint, inner)));
        assert!(!layer.items.contains(&item_index(&paint, over)));
        assert_eq!(layer.items.end, layer.items.start + 2);
    }
}

#[test]
fn plain_stacking_contexts_get_no_layer() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(".ctx { z-index: 3; transform: translate(5px, 0px); }")
    ));
    let root = h.root();
    let ctx = h.el(root, "view.box.ctx");
    let _inner = h.el(ctx, "view.box");
    assert!(h.paint().layers().is_empty());
}

#[test]
fn nested_group_effects_nest_layers() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(".outer { filter: grayscale(1); } .inner { opacity: 0.5; }")
    ));
    let root = h.root();
    let outer = h.el(root, "view.box.outer");
    let inner = h.el(outer, "view.box.inner");
    let paint = h.paint();

    let layers = paint.layers();
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0].node, outer);
    assert_eq!(layers[0].parent, None);
    assert_eq!(layers[1].node, inner);
    assert_eq!(layers[1].parent, Some(0));
    assert!(layers[0].items.start <= layers[1].items.start);
    assert!(layers[1].items.end <= layers[0].items.end);
    assert_eq!(layers[1].items.start, item_index(&paint, inner));
}

#[test]
fn hidden_group_root_still_layers_visible_content() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(
            ".fade { opacity: 0.5; visibility: hidden; left: 20px; top: 10px;
                     border-radius: 8px; }
             .shown { visibility: visible; }"
        )
    ));
    let root = h.root();
    let fade = h.el(root, "view.box.fade");
    let shown = h.el(fade, "view.box.shown");
    let paint = h.paint();

    assert!(paint.items().iter().all(|item| item.node != fade));
    let layers = paint.layers();
    assert_eq!(layers.len(), 1);
    let layer = &layers[0];
    assert_eq!(layer.node, fade);
    assert_eq!(layer.items.clone().count(), 1);
    assert_eq!(layer.items.start, item_index(&paint, shown));
    assert_eq!(layer.size.width, 100.0);
    assert_eq!(layer.size.height, 100.0);
    assert_eq!(layer.radii.top_left.width, 8.0);
    assert_eq!(layer.radii.bottom_right.height, 8.0);
    let origin = layer
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert_eq!((origin.x, origin.y), (20.0, 10.0));
}

#[test]
fn empty_groups_are_dropped() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(".fade { opacity: 0.5; visibility: hidden; }")
    ));
    let root = h.root();
    let fade = h.el(root, "view.box.fade");
    let _hidden_child = h.el(fade, "view.box");
    assert!(h.paint().layers().is_empty());
}

#[test]
fn leaf_group_contexts_close_their_layer() {
    let mut h = Harness::new(&format!(
        "{PAGE} {}",
        abs_box(".fade { opacity: 0.5; } .fade > * { display: none; }")
    ));
    let root = h.root();
    let fade = h.el(root, "view.box.fade");
    let over = h.el(root, "view.box.over");
    let paint = h.paint();
    let layers = paint.layers();
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].items.clone().count(), 1);
    assert_eq!(layers[0].items.start, item_index(&paint, fade));
    assert!(!layers[0].items.contains(&item_index(&paint, over)));
}

// ---------------------------------------------------------------------------
// Buffer reuse
//
// The builder fills buffers reclaimed from the frame the painter last retired
// and a working set carried across frames, so the second build of a document
// runs against warm storage where the first ran against empty storage. These
// pin the property that makes that admissible: the storage a build starts from
// is not an input to what it produces.
// ---------------------------------------------------------------------------

/// Every field of a `PaintOrder` an observer could distinguish, with floats
/// compared by bit pattern so a path that turned `-0.0` into `0.0` would fail.
fn fingerprint(paint: &PaintOrder) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    for item in paint.items() {
        let _ = writeln!(
            out,
            "item {:?} {:?} clip={:?} hit={} size={:?} radii={:?} m={:?}",
            item.node,
            item.kind,
            item.clip,
            item.hit_testable,
            (item.size.width.to_bits(), item.size.height.to_bits()),
            radii_bits(&item.radii),
            item.transform.to_array().map(f32::to_bits),
        );
    }
    for clip in paint.clips() {
        let _ = writeln!(
            out,
            "clip parent={:?} rect={:?} radii={:?} m={:?}",
            clip.parent,
            (
                clip.rect.origin.x.to_bits(),
                clip.rect.origin.y.to_bits(),
                clip.rect.size.width.to_bits(),
                clip.rect.size.height.to_bits(),
            ),
            radii_bits(&clip.radii),
            clip.transform.to_array().map(f32::to_bits),
        );
    }
    for layer in paint.layers() {
        let _ = writeln!(
            out,
            "layer {:?} parent={:?} items={:?} size={:?} radii={:?} m={:?}",
            layer.node,
            layer.parent,
            layer.items,
            (layer.size.width.to_bits(), layer.size.height.to_bits()),
            radii_bits(&layer.radii),
            layer.transform.to_array().map(f32::to_bits),
        );
    }
    out
}

fn radii_bits(radii: &crate::visual::CornerRadii) -> [u32; 8] {
    [
        radii.top_left.width.to_bits(),
        radii.top_left.height.to_bits(),
        radii.top_right.width.to_bits(),
        radii.top_right.height.to_bits(),
        radii.bottom_right.width.to_bits(),
        radii.bottom_right.height.to_bits(),
        radii.bottom_left.width.to_bits(),
        radii.bottom_left.height.to_bits(),
    ]
}

/// Every shape the build's buffer discipline has to survive: nested stacking
/// contexts, a group effect, negative and positive stack levels, an escaping
/// absolute inside a scrolled clipper, nested pseudo-contexts, reordered
/// siblings, and a `display: none` sibling among them.
fn reuse_harness() -> (Harness, NodeId) {
    let mut h = Harness::new(&format!(
        "{PAGE}
         .box {{ display: flex; position: absolute; left: 4px; top: 6px;
                 width: 100px; height: 100px; }}
         .rel {{ position: relative; }}
         .fade {{ opacity: 0.5; }}
         .under {{ z-index: -1; }}
         .over {{ z-index: 3; }}
         .clip {{ overflow: scroll; width: 40px; height: 40px; }}
         .tall {{ display: flex; flex-shrink: 0; width: 200px; height: 400px; }}
         .flow {{ display: flex; width: 30px; height: 12px; }}
         .first {{ order: -1; }}
         .gone {{ display: none; }}"
    ));
    let root = h.root();
    let scroller = h.el(root, "view.box.rel.clip");
    h.el(scroller, "view.tall");
    let escaping = h.el(scroller, "view.box");
    h.el(escaping, "view.flow");
    let fade = h.el(root, "view.box.fade");
    h.el(fade, "view.flow");
    h.el(fade, "view.flow.first");
    h.el(fade, "view.flow.gone");
    let nested = h.el(fade, "view.box.rel");
    h.el(nested, "view.box.rel");
    h.el(root, "view.box.under");
    h.el(root, "view.box.over");
    (h, scroller)
}

#[test]
fn a_warm_build_produces_the_paint_order_a_cold_build_produces() {
    let (mut h, scroller) = reuse_harness();
    h.doc
        .dom
        .scroll_to(scroller, crate::Vector2D::new(0.0, 37.0));

    // The first build runs against empty buffers and an empty scratch. Every
    // later one runs against storage at the previous frame's high-water mark,
    // and against a pseudo-context pool that has already been handed out and
    // returned once.
    let cold = fingerprint(&h.paint());
    for round in 1..4 {
        assert_eq!(cold, fingerprint(&h.paint()), "build {round} diverged");
    }
    assert!(cold.contains("layer"), "the fixture must exercise a group");
    assert!(cold.contains("clip"), "the fixture must exercise a clip");
}

#[test]
fn a_warm_frame_encodes_the_scene_a_cold_frame_encodes() {
    let (mut h, scroller) = reuse_harness();
    h.doc
        .dom
        .scroll_to(scroller, crate::Vector2D::new(0.0, 37.0));
    h.doc.dom.render();
    let cold = h.doc.dom.scene().clone();

    // A repaint of an unchanged document is refused, so each round moves the
    // scroll offset and moves it back. The second render of each pair sees the
    // recycled buffers and must land on the same encoding as the first.
    for _ in 0..3 {
        h.doc
            .dom
            .scroll_to(scroller, crate::Vector2D::new(0.0, 38.0));
        h.doc.dom.render();
        h.doc
            .dom
            .scroll_to(scroller, crate::Vector2D::new(0.0, 37.0));
        h.doc.dom.render();
    }
    crate::paint::equivalence::assert_scenes_identical(&cold, &h.doc.dom.scene());
}

#[test]
fn the_builder_stops_allocating_once_the_page_shape_settles() {
    let (mut h, scroller) = reuse_harness();
    h.doc.dom.render();
    for offset in [1.0_f32, 0.0, 1.0, 0.0] {
        h.doc
            .dom
            .scroll_to(scroller, crate::Vector2D::new(0.0, offset));
        h.doc.dom.render();
    }
    let settled = h.doc.dom.paint_storage_capacities();

    for offset in [1.0_f32, 0.0, 1.0, 0.0] {
        h.doc
            .dom
            .scroll_to(scroller, crate::Vector2D::new(0.0, offset));
        h.doc.dom.render();
    }

    assert_eq!(
        settled,
        h.doc.dom.paint_storage_capacities(),
        "a settled page must reuse every buffer instead of growing a new one",
    );
    assert!(
        settled.iter().all(|&capacity| capacity > 0),
        "the fixture must exercise every buffer at least once, got {settled:?}",
    );
}

// ---------------------------------------------------------------------------
// Culling is invisible to hit testing
//
// The painter discards items that cannot put ink where the scene is looked at.
// That decision lives in the painter's scratch, never in the retained frame,
// which is what keeps a programmatic hit outside the viewport — and an event
// arriving between a scroll and its repaint — answering from the geometry the
// frame was built with.
// ---------------------------------------------------------------------------

#[test]
fn a_box_outside_the_viewport_still_answers_hit_queries() {
    let mut h = Harness::new(&format!(
        "{PAGE} .box {{ display: flex; position: absolute; width: 100px; height: 100px; }}"
    ));
    let root = h.root();
    let far = h.el(root, "view.box");
    h.doc.set_inline(far, "left: -500px; top: 20px");
    assert_eq!(h.hit(-450.0, 70.0), Some(far));
}

#[test]
fn culling_does_not_change_the_retained_frame() {
    let mut h = Harness::new(&format!(
        "{PAGE} .box {{ display: flex; position: absolute; width: 100px; height: 100px; }}"
    ));
    let root = h.root();
    for index in 0..6 {
        let box_id = h.el(root, "view.box");
        h.doc
            .set_inline(box_id, &format!("left: 4000px; top: {}px", index * 120));
    }
    let built = h.paint().items().len();
    h.doc.dom.render();
    let retained = h
        .doc
        .dom
        .painter
        .borrow()
        .frame()
        .expect("a rendered document retains its frame")
        .items()
        .len();
    assert_eq!(
        built, retained,
        "the frame carries every item, painted or not"
    );
}
