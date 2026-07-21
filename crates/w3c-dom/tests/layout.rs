//! Behavior tests for the `w3c_dom::layout` integration: the neutron-star
//! host protocol implemented over `Document<T>` + stylo computed styles.
//!
//! These are **wiring** tests — each dispatch arm (flex/grid/linear/relative/
//! leaf), value translation (percentages, `calc()`, box model), the
//! out-of-flow positioned pass (including the W3C `position: fixed`
//! containing-block rule), device-pixel rounding, and the cache/invalidation
//! contract. Algorithm-level conformance lives in `neutron-star`'s own
//! suites.

// Exact-geometry oracles: every expected value is exactly representable and
// produced by the same arithmetic, so strict float equality is the point.
#![allow(clippy::float_cmp)]

mod common;

use common::{Doc, device_with};
use stylo::queries::values::PrefersColorScheme;
use w3c_dom::NodeId;
use w3c_dom::layout::{Layout, LeafMeasureInput, LeafMetrics, MeasureLeaf, Size};

/// [`Doc`] plus layout helpers (results are read straight off the nodes).
struct Harness {
    doc: Doc,
}

impl Harness {
    fn new(css: &str) -> Self {
        Self {
            doc: Doc::with_css(css),
        }
    }

    /// Run the style-then-layout pipeline.
    fn layout(&mut self) {
        self.doc.engine.layout_document(&mut self.doc.dom);
    }

    fn layout_of(&self, id: NodeId) -> Layout {
        self.doc.dom.get(id).expect("node id is live").layout()
    }

    /// `(x, y, width, height)` of the node's rounded border box, relative to
    /// its parent's border box.
    fn rect(&self, id: NodeId) -> (f32, f32, f32, f32) {
        let layout = self.layout_of(id);
        (
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        )
    }
}

#[test]
fn flex_row_distributes_free_space_and_positions_children() {
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 50px; }
         .a { flex-grow: 1; }
         .b { flex-grow: 3; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, ".a");
    let b = h.doc.el(root, ".b");
    h.layout();

    assert_eq!(h.rect(root), (0.0, 0.0, 200.0, 50.0));
    // align-items: normal behaves as stretch on the cross axis.
    assert_eq!(h.rect(a), (0.0, 0.0, 50.0, 50.0));
    assert_eq!(h.rect(b), (50.0, 0.0, 150.0, 50.0));
}

#[test]
fn flex_gap_margin_padding_and_percentages_resolve() {
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 100px; gap: 10px; padding: 10px;
                box-sizing: border-box; }
         view { width: 25%; height: 50%; margin-left: 10px; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, "view");
    let b = h.doc.el(root, "view");
    h.layout();

    // The border box stays 200×100 (border-box sizing); children resolve
    // percentages against the 180×80 content box.
    assert_eq!(h.rect(root).2, 200.0);
    let (_, _, width, height) = h.rect(a);
    assert_eq!((width, height), (45.0, 40.0)); // 25% of 180, 50% of 80
    assert_eq!(h.rect(a).0, 20.0); // padding 10 + margin-left 10
    assert_eq!(h.rect(b).0, 85.0); // 20 + 45 + gap 10 + margin-left 10
}

#[test]
fn content_box_sizing_is_the_default_and_padding_grows_the_border_box() {
    // The W3C initial `box-sizing: content-box` — Lynx's border-box default
    // is the embedder's cascade-level (UA sheet) policy, not this crate's.
    let mut h = Harness::new("page { display: flex; width: 200px; height: 100px; padding: 10px; }");
    h.layout();
    assert_eq!(h.rect(h.doc.root).2, 220.0);
}

#[test]
fn flex_order_reorders_layout_and_paint_indices() {
    let mut h = Harness::new(
        "page { display: flex; width: 100px; height: 10px; }
         view { width: 40px; }
         .late { order: 2; }
         .early { order: 1; }",
    );
    let root = h.doc.root;
    let late = h.doc.el(root, ".late");
    let early = h.doc.el(root, ".early");
    h.layout();

    assert_eq!(h.rect(early).0, 0.0);
    assert_eq!(h.rect(late).0, 40.0);
    assert_eq!(h.layout_of(early).order, 0);
    assert_eq!(h.layout_of(late).order, 1);
}

#[test]
fn rtl_direction_flips_the_flex_row_axis() {
    let mut h = Harness::new(
        "page { display: flex; width: 100px; height: 10px; direction: rtl; }
         view { width: 30px; }",
    );
    let root = h.doc.root;
    let first = h.doc.el(root, "view");
    h.layout();

    assert_eq!(h.rect(first).0, 70.0);
}

#[test]
fn calc_widths_resolve_during_layout() {
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 40px; }
         view { width: calc(50% - 10px); height: calc(10px + 25%); }",
    );
    let root = h.doc.root;
    let child = h.doc.el(root, "view");
    h.layout();

    let (_, _, width, height) = h.rect(child);
    assert_eq!(width, 90.0); // 50% of 200 - 10
    assert_eq!(height, 20.0); // 10 + 25% of 40
}

#[test]
fn min_max_clamps_and_aspect_ratio_apply() {
    let mut h = Harness::new(
        "page { display: flex; width: 400px; height: 300px; align-items: flex-start; }
         .clamped { width: 300px; max-width: 100px; min-height: 30px; }
         .ratio { width: 100px; aspect-ratio: 2; }",
    );
    let root = h.doc.root;
    let clamped = h.doc.el(root, ".clamped");
    let ratio = h.doc.el(root, ".ratio");
    h.layout();

    let (_, _, width, height) = h.rect(clamped);
    assert_eq!((width, height), (100.0, 30.0));
    let (_, _, width, height) = h.rect(ratio);
    assert_eq!((width, height), (100.0, 50.0));
}

#[test]
fn border_box_sizing_and_borders_reach_layout() {
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 100px; }
         view { box-sizing: border-box; width: 100px; height: 60px;
                padding: 10px; border: 5px solid black; }",
    );
    let root = h.doc.root;
    let child = h.doc.el(root, "view");
    h.layout();

    let layout = h.layout_of(child);
    assert_eq!(layout.size.width, 100.0); // border-box width stays 100
    assert_eq!(layout.border.left, 5.0);
    assert_eq!(layout.padding.top, 10.0);
}

#[test]
fn linear_column_stacks_and_distributes_weights() {
    let mut h = Harness::new(
        "page { display: linear; width: 100px; height: 100px; }
         view { width: 40px; }
         .w1 { linear-weight: 1; }
         .w3 { linear-weight: 3; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, ".w1");
    let b = h.doc.el(root, ".w3");
    h.layout();

    // Definite 100px main size distributed 1:3.
    assert_eq!(h.rect(a), (0.0, 0.0, 40.0, 25.0));
    assert_eq!(h.rect(b), (0.0, 25.0, 40.0, 75.0));
}

#[test]
fn linear_row_direction_comes_from_linear_direction() {
    let mut h = Harness::new(
        "page { display: linear; linear-direction: row; width: 100px; height: 20px; }
         view { width: 30px; height: 10px; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, "view");
    let b = h.doc.el(root, "view");
    h.layout();

    assert_eq!(h.rect(a).0, 0.0);
    assert_eq!(h.rect(b).0, 30.0);
    assert_eq!(h.rect(b).1, 0.0);
}

#[test]
fn relative_container_solves_id_constraints() {
    let mut h = Harness::new(
        "page { display: relative; width: 200px; height: 100px; }
         .a { relative-id: 1; width: 50px; height: 20px; }
         .right { width: 30px; height: 10px; relative-align-right: parent; }
         .below { width: 40px; height: 10px; relative-bottom-of: 1; relative-align-left: 1; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, ".a");
    let right = h.doc.el(root, ".right");
    let below = h.doc.el(root, ".below");
    h.layout();

    assert_eq!(h.rect(a), (0.0, 0.0, 50.0, 20.0));
    // Right margin edge aligned with the parent's right content edge.
    assert_eq!(h.rect(right).0, 170.0);
    // `relative-bottom-of: 1` places the box below item 1; left edges align.
    assert_eq!(h.rect(below), (0.0, 20.0, 40.0, 10.0));
}

#[test]
fn grid_places_items_into_fixed_tracks() {
    let mut h = Harness::new(
        "page { display: grid; width: 100px; height: 60px;
                grid-template-columns: 30px 70px; grid-template-rows: 20px 40px; }
         .spans { grid-column: span 2; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, "view");
    let b = h.doc.el(root, "view");
    let spans = h.doc.el(root, ".spans");
    h.layout();

    assert_eq!(h.rect(a), (0.0, 0.0, 30.0, 20.0));
    assert_eq!(h.rect(b), (30.0, 0.0, 70.0, 20.0));
    assert_eq!(h.rect(spans), (0.0, 20.0, 100.0, 40.0));
}

#[test]
fn grid_fr_tracks_and_repeat_expand() {
    let mut h = Harness::new(
        "page { display: grid; width: 120px; height: 30px;
                grid-template-columns: repeat(2, 1fr) 40px; grid-template-rows: 30px; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, "view");
    let b = h.doc.el(root, "view");
    let c = h.doc.el(root, "view");
    h.layout();

    assert_eq!(h.rect(a), (0.0, 0.0, 40.0, 30.0));
    assert_eq!(h.rect(b), (40.0, 0.0, 40.0, 30.0));
    assert_eq!(h.rect(c), (80.0, 0.0, 40.0, 30.0));
}

#[test]
fn display_none_zeroes_the_subtree_and_layout_recovers_after_invalidation() {
    let mut h = Harness::new(
        "page { display: flex; width: 100px; height: 40px; }
         view { flex-grow: 1; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, "view");
    let b = h.doc.el(root, "view");
    let grandchild = h.doc.el(a, "view");
    h.layout();
    assert_eq!(h.rect(a).2, 50.0);

    h.doc.set_inline(a, "display: none");
    h.doc.dom.invalidate_layout(a);
    h.layout();

    assert_eq!(h.rect(a), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(h.rect(grandchild), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(h.rect(b).2, 100.0);

    h.doc.set_inline(a, "");
    h.doc.dom.invalidate_layout(a);
    h.layout();
    assert_eq!(h.rect(a).2, 50.0);
    assert_eq!(h.rect(b).2, 50.0);
}

#[test]
fn absolute_child_resolves_against_its_positioned_parent() {
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 100px; }
         .parent { display: flex; position: relative; width: 100px; height: 80px;
                   border: 4px solid black; box-sizing: border-box; }
         .abs { position: absolute; left: 10px; bottom: 6px; width: 20px; height: 10px; }",
    );
    let root = h.doc.root;
    let parent = h.doc.el(root, ".parent");
    let abs = h.doc.el(parent, ".abs");
    h.layout();

    // Insets resolve against the parent's padding box (100-8 × 80-8),
    // locations are border-box-relative: border + inset.
    assert_eq!(h.rect(abs), (14.0, 60.0, 20.0, 10.0));
}

#[test]
fn absolute_child_with_auto_insets_uses_its_static_position() {
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 100px; justify-content: center;
                align-items: center; }
         .abs { position: absolute; width: 20px; height: 10px; }",
    );
    let root = h.doc.root;
    let abs = h.doc.el(root, ".abs");
    h.layout();

    // Flexbox §4.1: static position as the sole flex item — centered.
    assert_eq!(h.rect(abs), (90.0, 45.0, 20.0, 10.0));
}

#[test]
fn fixed_anchors_to_the_viewport_unless_an_ancestor_establishes_the_cb() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .host { width: 300px; height: 200px; margin-left: 100px; margin-top: 50px;
                 display: flex; }
         .plain {}
         .transformed { transform: translateX(0px); }
         .fixed { position: fixed; left: 10px; top: 20px; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let plain = h.doc.el(root, ".host.plain");
    let fixed_to_viewport = h.doc.el(plain, ".fixed");
    let transformed = h.doc.el(root, ".host.transformed");
    let fixed_to_ancestor = h.doc.el(transformed, ".fixed");
    h.layout();

    // Viewport-anchored: stored parent-relative, so the parent's own offset
    // (x = 100, y = 50) is subtracted from the viewport position (10, 20).
    assert_eq!(h.rect(fixed_to_viewport), (-90.0, -30.0, 30.0, 40.0));
    // A transformed ancestor is the containing block per the W3C rule the
    // repository standards policy mandates (not Lynx's escape-to-root).
    assert_eq!(h.rect(fixed_to_ancestor), (10.0, 20.0, 30.0, 40.0));
}

#[test]
fn fixed_stays_viewport_anchored_when_its_parent_answers_from_cache() {
    // The formatting parent's LayoutInput does not change when only an
    // *ancestor* offset moves, so the parent answers from its measurement
    // cache and its algorithm (which records static positions) never
    // re-runs. The positioned pass must still re-anchor the fixed child to
    // the viewport from current ancestor geometry.
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .spacer { width: 100px; height: 10px; }
         .host { display: flex; width: 200px; height: 100px; }
         .fixed { position: fixed; left: 10px; top: 20px; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let spacer = h.doc.el(root, ".spacer");
    let host = h.doc.el(root, ".host");
    let fixed = h.doc.el(host, ".fixed");
    h.layout();
    // Viewport (10, 20) expressed relative to the host at x = 100.
    assert_eq!(h.rect(fixed), (-90.0, 20.0, 30.0, 40.0));

    // Move the ancestor: only the spacer (and its ancestors) are
    // invalidated; the host's own cache stays warm.
    h.doc.set_inline(spacer, "width: 150px");
    h.doc.dom.invalidate_layout(spacer);
    h.layout();

    // The host moved to x = 150; the fixed child must still sit at viewport
    // (10, 20), i.e. a *different* parent-relative location.
    assert_eq!(h.rect(host).0, 150.0);
    assert_eq!(h.rect(fixed), (-140.0, 20.0, 30.0, 40.0));
}

#[test]
fn hoisted_children_paint_with_effective_order_zero() {
    // The engine's paint-key rule gives out-of-flow children effective
    // `order` 0 — their authored `order` must not reorder them.
    let mut h = Harness::new(
        "page { display: flex; width: 300px; height: 50px; }
         .fixed { position: fixed; order: 5; left: 0; top: 0; width: 10px; height: 10px; }
         .plain { width: 30px; }
         .early { order: 1; width: 30px; }",
    );
    let root = h.doc.root;
    let fixed = h.doc.el(root, ".fixed");
    let plain = h.doc.el(root, ".plain");
    let early = h.doc.el(root, ".early");
    h.layout();

    // Merged paint keys: fixed (0, doc 0), plain (0, doc 1), early (1, doc 2).
    assert_eq!(h.layout_of(fixed).order, 0);
    assert_eq!(h.layout_of(plain).order, 1);
    assert_eq!(h.layout_of(early).order, 2);
}

#[test]
fn offset_path_establishes_the_fixed_containing_block() {
    // Motion Path: a non-none `offset-path` has the usual `transform`
    // effects, containing-block creation for fixed descendants included.
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .mover { display: flex; width: 300px; height: 200px; margin-left: 100px;
                  offset-path: path(\"M 0 0 H 100\"); }
         .fixed { position: fixed; left: 10px; top: 20px; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let mover = h.doc.el(root, ".mover");
    let fixed = h.doc.el(mover, ".fixed");
    h.layout();

    assert_eq!(h.rect(fixed), (10.0, 20.0, 30.0, 40.0));
}

#[test]
fn root_filter_is_exempt_from_fixed_containing_block_creation() {
    // Filter Effects §5 exempts the document root element: a filtered root
    // does not capture fixed descendants (they stay viewport-anchored),
    // while the same filter on a non-root ancestor does.
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; border: 10px solid black;
                filter: grayscale(1); }
         .filtered { display: flex; width: 300px; height: 200px; margin-left: 100px;
                     filter: grayscale(1); }
         .fixed { position: fixed; left: 0; top: 0; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let root_fixed = h.doc.el(root, ".fixed");
    let filtered = h.doc.el(root, ".filtered");
    let captured_fixed = h.doc.el(filtered, ".fixed");
    h.layout();

    // Exempt: anchored to the viewport origin, not the root's padding box
    // (which starts at (10, 10) inside the border).
    assert_eq!(h.rect(root_fixed), (0.0, 0.0, 30.0, 40.0));
    // Non-root filter captures: anchored to the filtered box's padding box.
    assert_eq!(h.rect(captured_fixed), (0.0, 0.0, 30.0, 40.0));
    assert_eq!(h.rect(filtered).0, 110.0); // border 10 + margin 100
}

#[test]
fn fixed_inside_nested_hoisted_subtrees_completes_in_preorder() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .outer { position: fixed; left: 100px; top: 100px; width: 200px; height: 200px;
                  display: flex; transform: translateX(0px); }
         .inner { position: fixed; left: 10px; top: 5px; width: 20px; height: 20px; }",
    );
    let root = h.doc.root;
    let outer = h.doc.el(root, ".outer");
    let inner = h.doc.el(outer, ".inner");
    h.layout();

    assert_eq!(h.rect(outer).0, 100.0);
    // The outer fixed box is transformed, so it is the inner one's CB.
    assert_eq!(h.rect(inner), (10.0, 5.0, 20.0, 20.0));
}

#[test]
fn hoisted_nodes_relayout_across_passes() {
    // The hoisted-queue dedupe flag must reset between passes, or the second
    // pass would silently skip the positioned pass for the same node.
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .fixed { position: fixed; left: 10px; top: 20px; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let fixed = h.doc.el(root, ".fixed");
    h.layout();
    assert_eq!(h.rect(fixed), (10.0, 20.0, 30.0, 40.0));

    h.doc.set_inline(fixed, "left: 50px");
    h.doc.dom.invalidate_layout(fixed);
    h.layout();
    assert_eq!(h.rect(fixed), (50.0, 20.0, 30.0, 40.0));
}

#[test]
fn flow_containers_fall_back_to_leaves_and_zero_their_children() {
    // No display set: the (blockified) flow fallback. Its own box styles
    // still apply; children do not participate in layout.
    let mut h = Harness::new(
        "page { display: flex; width: 100px; height: 100px; align-items: flex-start; }
         .flow { width: 40px; height: 30px; }
         .child { width: 999px; height: 999px; }",
    );
    let root = h.doc.root;
    let flow = h.doc.el(root, ".flow");
    let child = h.doc.el(flow, ".child");
    h.layout();

    assert_eq!(h.rect(flow), (0.0, 0.0, 40.0, 30.0));
    assert_eq!(h.rect(child), (0.0, 0.0, 0.0, 0.0));
}

/// A payload whose [`MeasureLeaf`] hook gives every content-bearing leaf a
/// fixed measurement — the embedder-measurement stand-in for these tests
/// (real embedders plug a text engine in here).
#[derive(Debug)]
struct FixedMeasure(f32, f32);

impl w3c_dom::ExternalState for FixedMeasure {}

impl MeasureLeaf for FixedMeasure {
    fn measure_leaf(&self, node: &w3c_dom::Node<Self>, _input: LeafMeasureInput) -> LeafMetrics {
        if node.text().is_some() {
            LeafMetrics::new(Size::new(self.0, self.1))
        } else {
            LeafMetrics::default()
        }
    }
}

#[test]
fn leaves_measure_through_the_payload_hook() {
    // Element-backed character data (Lynx's `<raw-text>` shape): the node is
    // an element leaf whose content size comes from the payload's hook.
    let mut engine = w3c_dom::StyleEngine::new(common::device(800.0, 600.0));
    engine.add_stylesheet_str(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start; }",
        w3c_dom::StylesheetOrigin::Author,
    );
    let mut dom = engine.new_document();
    let root = dom.create_element("page", FixedMeasure(42.0, 17.0));
    dom.append_child(root);
    let text = dom.create_element("text", FixedMeasure(42.0, 17.0));
    dom.append(root, text);
    dom.set_text(text, Some("hello".into()));
    engine.layout_document(&mut dom);

    let layout = dom.get(text).unwrap().layout();
    assert_eq!(
        (
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height
        ),
        (0.0, 0.0, 42.0, 17.0)
    );
}

#[test]
fn text_nodes_lay_out_as_anonymous_leaf_boxes() {
    // A real text node (no computed style): box properties take their
    // initial values — the anonymous box CSS wraps a text run in — and the
    // content size comes from the payload's measure hook.
    let mut engine = w3c_dom::StyleEngine::new(common::device(800.0, 600.0));
    engine.add_stylesheet_str(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start; }
         .sibling { width: 50px; height: 10px; }",
        w3c_dom::StylesheetOrigin::Author,
    );
    let mut dom = engine.new_document();
    let root = dom.create_element("page", FixedMeasure(30.0, 12.0));
    dom.append_child(root);
    let sibling = dom.create_element("view", FixedMeasure(30.0, 12.0));
    dom.add_class(sibling, "sibling");
    dom.append(root, sibling);
    let text = dom.create_text_node("hello", FixedMeasure(30.0, 12.0));
    dom.append(root, text);
    engine.layout_document(&mut dom);

    let rect = |id: NodeId| {
        let layout = dom.get(id).unwrap().layout();
        (
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        )
    };
    assert_eq!(rect(sibling), (0.0, 0.0, 50.0, 10.0));
    // The text item follows its sibling with zero margins/padding (initial
    // values), sized purely by the hook's measurement.
    assert_eq!(rect(text), (50.0, 0.0, 30.0, 12.0));
}

#[test]
fn rounding_snaps_to_the_device_pixel_grid() {
    let mut h = Harness {
        doc: Doc::with_device(device_with(800.0, 600.0, 2.0, PrefersColorScheme::Light)),
    };
    h.doc.add_css(
        "page { display: flex; width: 100px; height: 10px; }
         view { width: 20.25px; height: 10px; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, "view");
    let b = h.doc.el(root, "view");
    h.layout();

    // On a DPR-2 grid quarter pixels snap to halves...
    assert_eq!(h.layout_of(a).size.width, 20.5);
    // ...while the unrounded truth is preserved for relayout.
    let a_node = h.doc.dom.get(a).unwrap();
    assert_eq!(a_node.unrounded_layout().size.width, 20.25);
    // Adjacent edges share the snapped boundary (no cumulative drift).
    assert_eq!(h.layout_of(b).location.x, 20.5);
}

#[test]
fn layout_state_dies_with_its_node() {
    let mut h = Harness::new(
        "page { display: flex; width: 100px; height: 10px; }
         view { flex-grow: 1; }",
    );
    let root = h.doc.root;
    let old = h.doc.el(root, "view");
    h.layout();
    assert_eq!(h.rect(old).2, 100.0);

    h.doc.dom.remove_subtree(old);
    // The layout state was dropped with the node: the slab slot is vacant
    // (raw ids carry no generation — the runtime layer owns id lifetime).
    assert!(h.doc.dom.get(old).is_none());

    // Removal changed the parent's child list: invalidate the old parent.
    h.doc.dom.invalidate_layout(root);
    let new = h.doc.el(root, "view");
    h.layout();

    // The reused slot starts from freshly-constructed layout state.
    assert_eq!(h.rect(new).2, 100.0);
}

#[test]
fn viewport_percentages_resolve_against_the_engine_viewport() {
    let mut h = Harness::new("page { display: flex; width: 50%; height: 25%; }");
    h.layout();
    // Doc's device is 800×600.
    assert_eq!(h.rect(h.doc.root), (0.0, 0.0, 400.0, 150.0));
}

#[test]
fn layout_document_flushes_pending_styles_itself() {
    // The style → layout phase barrier is enforced by construction:
    // layout_document runs the restyle traversal first, so no explicit
    // flush call is needed between mutation and layout.
    let mut h = Harness::new("page { display: flex; width: 200px; height: 50px; }");
    let root = h.doc.root;
    let child = h.doc.el(root, "view");
    h.doc.set_inline(child, "width: 60px");
    h.layout(); // no doc.flush() anywhere

    assert_eq!(h.rect(child).2, 60.0);
}
