//! Behavior tests for `w3c_dom::visual`: stacking contexts, Appendix-E paint
//! order, transform matrices, clip chains, and hit testing.

#![allow(clippy::float_cmp)]

mod common;

use common::Doc;
use w3c_dom::NodeId;
use w3c_dom::visual::{PaintItemKind, PaintOrder, Point2D};

const AHEM: &[u8] = include_bytes!("../../neutron-star/tests/fixtures/Ahem.ttf");

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

    /// Node ids of the element-box items, in paint order.
    fn element_order(&mut self) -> Vec<NodeId> {
        self.paint()
            .items()
            .iter()
            .filter(|item| item.kind == PaintItemKind::ElementBox)
            .map(|item| item.node)
            .collect()
    }

    fn hit(&mut self, x: f32, y: f32) -> Option<NodeId> {
        self.doc.dom.hit_test(Point2D::new(x, y))
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
    // The negative context paints above the root's own background but below
    // the root's in-flow content.
    assert_eq!(h.element_order(), vec![root, neg, flow]);
    assert_eq!(h.hit(50.0, 50.0), Some(flow));
}

#[test]
fn z_index_compares_only_within_the_same_stacking_context() {
    // The anti-Lynx scenario: an inner z-index: 9999 cannot escape its
    // z-index: 1 parent context to beat a z-index: 2 outer sibling.
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
    // A positioned z-index: auto wrapper is not atomic: its negative-z
    // descendant escapes below the wrapper itself.
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
    // Two relative-positioned flex items overlap via a negative margin;
    // `order` reorders their painting (order-modified document order), not
    // raw DOM order.
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .item { display: flex; position: relative; width: 100px; height: 100px; }
         .first { order: 2; }
         .second { order: 1; margin-right: -100px; }",
    );
    let root = h.root();
    let first = h.el(root, "view.item.first");
    let second = h.el(root, "view.item.second");
    // order: 1 paints before order: 2 despite DOM order.
    assert_eq!(h.element_order(), vec![root, second, first]);
    assert_eq!(h.hit(50.0, 50.0), Some(first));
}

#[test]
fn order_is_inert_on_absolutely_positioned_children() {
    // css-display-3 §3: "Absolutely-positioned children of a flex/grid
    // container are treated as having order: 0 for the purpose of
    // determining their painting order relative to flex/grid items." (The
    // old css-flexbox-1 §4.1 "participates in the reordering step" sentence
    // was superseded by this.) The layout host's effective-order-0 rule for
    // hoisted boxes implements exactly that: DOM order decides.
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
    // A z-index: 5 child is trapped inside its opacity-context parent and
    // loses to a z-index: 1 sibling of that parent.
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
    // css-flexbox-1 §4.3: z-index applies to flex items even when static.
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
    // item (z:5) is a real context: trapped (z:9) stays inside it, and item
    // paints above item2 (z:1).
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
    // Atomic (trapped stays inside) and clipping (trapped's box at
    // x 150..250 is cut off by the contain: paint padding box).
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
    // The fixed box's containing block is the transformed ancestor, so its
    // world matrix carries the ancestor's translate: local (0,0) lands at
    // (100 + 50 + 10, 100 + 10).
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
    // Inside big's border box, outside the clip: falls through to the root.
    assert_eq!(h.hit(100.0, 25.0), Some(root));
}

#[test]
fn absolute_boxes_escape_clips_outside_their_containing_block_chain() {
    // E { relative } > W { overflow: hidden, static } > X { absolute } with
    // children D (static) and G (absolute): X, D, and G all escape W's clip
    // because their containing-block chains bypass it.
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
    // r { relative, overflow: hidden } is the containing block AND the
    // clipper: the out-of-bounds absolute child is clipped away.
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
    // rotate(90deg) about the box's own top-left: local (100, 0) lands at
    // (100, 100); the box now occupies x 0..100, y 0..100. This pins the
    // T(−origin)·M·T(origin)·T(offset) composition order — the inverted
    // order would orbit the box to x 100..200 unrotated or worse.
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(100.0, 0.0))
        .expect("rotation is invertible");
    assert!((mapped.x - 100.0).abs() < 1e-4 && (mapped.y - 100.0).abs() < 1e-4);
    assert_eq!(h.hit(50.0, 50.0), Some(rotated));
    // The untransformed position no longer hits.
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
    // 50% of 200x100 = (100, 50).
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
    // depth 100, z 50 ⇒ scale 2 about p's center (100, 100): the 50px box at
    // (75, 75) projects to (50, 50)..(150, 150).
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
    // Outside the unprojected 75..125 box, inside the projection.
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
    // The ghost subtree inherits pointer-events: none except where a
    // descendant re-enables auto.
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
    // The corner outside the circle falls through.
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
    h.doc.dom.register_fonts(AHEM);
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
}

#[test]
fn hit_outside_all_content_is_none() {
    let mut h = Harness::new("page { display: flex; width: 100px; height: 100px; }");
    assert_eq!(h.hit(400.0, 400.0), None);
}

#[test]
fn empty_document_paints_nothing() {
    let mut doc: w3c_dom::Document<()> = w3c_dom::Document::new(common::device(800.0, 600.0));
    let paint = doc.paint_order();
    assert!(paint.items().is_empty());
    assert_eq!(paint.hit_test(Point2D::new(10.0, 10.0)), None);
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
    // Sticky always establishes a context: trapped (z: 5) cannot escape it
    // to beat the z-index: 1 sibling.
    assert_eq!(h.element_order(), vec![root, stick, trapped, over]);
    assert_eq!(h.hit(50.0, 50.0), Some(over));
}

#[test]
fn fixed_position_escapes_static_clippers_to_the_viewport() {
    // The fixed clip context is distinct from the absolute one: with no
    // fixed-CB ancestor, a fixed box escapes every static clipper.
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
    // Inside inner's rect but outside outer's: the chain walk must reject.
    assert_eq!(h.hit(150.0, 25.0), Some(root));
    assert_eq!(h.hit(75.0, 25.0), Some(big));
    // Below inner's 50px band but inside outer: outer itself.
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
    // The clip rect lives in the mover's transformed space.
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
    // Flattened rotateX(60°) about the default 50% 50% origin compresses y
    // by cos 60° = 0.5 about y = 50: the box covers y 25..75.
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
    // rotate(90deg) about the default center (100, 50) then offset (100, 0):
    // local (x, y) → (250 − y, x − 50). The box lands on x 150..250,
    // y −50..150.
    let mapped = item
        .transform
        .transform_point2d(Point2D::new(0.0, 0.0))
        .unwrap();
    assert!((mapped.x - 250.0).abs() < 1e-3 && (mapped.y + 50.0).abs() < 1e-3);
    assert_eq!(h.hit(200.0, 50.0), Some(rotated));
    // Inside the untransformed span, outside the rotated strip.
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
    // Sticky does not escape its ancestor clip the way absolute does.
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
    // The hidden context root's clip still applies to its content.
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
    h.doc.dom.register_fonts(AHEM);
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
    // Glyphs beyond the clip do not hit.
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
    // x = 100 is first's trailing edge (excluded) and second's leading edge.
    assert_eq!(h.hit(100.0, 50.0), Some(second));
    // x = 200 is second's trailing edge with nothing beyond: the root.
    assert_eq!(h.hit(200.0, 50.0), Some(root));
    // The root's own trailing edges miss too.
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
    // deep's DOM parent is mid, not the perspective element, so it gets NO
    // perspective: translateZ flattens to identity and the box stays at
    // (75, 75)..(125, 125) — no ×2 projection about (100, 100).
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
    // Outside the unprojected box: the perspective element itself.
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
    // Everything inside the z:-1 context stays below the root's in-flow
    // content — z:99 cannot escape the atomic negative member.
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

// --- display:contents dissolution ---

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
    // Probe-verified Chrome behavior: SC triggers are inert on a boxless
    // element, so a z-index: 5 grandchild escapes into the root context and
    // beats a z-index: 1 sibling — the opposite of an opacity-wrapper trap.
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
    // The box child inherits hidden through the contents wrapper; its own
    // visible descendant re-reveals.
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
    h.doc.dom.register_fonts(AHEM);
    let root = h.root();
    let wrap = h.el(root, "view.wrap");
    let text = h.doc.dom.create_text_node("hello", ());
    h.doc.dom.append_child(wrap, text);
    let paint = h.paint();
    let item = paint.items().iter().find(|item| item.node == text).unwrap();
    // The singular text-hit rule: a text run targets its DOM parent element
    // even when that parent is boxless (matches Chrome's elementFromPoint).
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
    // No transform (box stays at the container origin), no clip.
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
    // Same projection as the direct-child perspective test: the dissolved
    // grandchild is a box-tree child of the perspective element.
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
    // The capture side of dissolved clip threading: the clipping relative
    // ancestor IS the abs grandchild's containing block, so its clip applies
    // even though the dissolved contents level sits in between.
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
    // The abs box lies fully outside the 50x50 clip: hits fall through.
    assert_eq!(h.hit(110.0, 110.0), Some(root));
    let _ = wrap;
}

#[test]
fn contents_order_interleave_paints_and_survives_hidden_siblings() {
    // The paint-order side of the order interleave, with a display:none
    // child inside the wrapper (hidden children consume dissolved indices
    // but are excluded from ranks — the sort must not be perturbed).
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
