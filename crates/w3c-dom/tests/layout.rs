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

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
        self.doc
            .dom
            .get(id)
            .expect("node id is live")
            .layout()
            .clone()
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

    /// Whether `id`'s measurement cache is currently empty — the observable for
    /// boundary-stopped invalidation (a cleared spine vs. a surviving cache).
    fn node_cache_empty(&self, id: NodeId) -> bool {
        self.doc
            .dom
            .get(id)
            .expect("node id is live")
            .layout_cache_is_empty()
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
fn fixed_descendants_of_the_leaf_fallback_stay_zeroed() {
    // A flow container falls back to leaf layout and zeroes its children;
    // the positioned pass must not revive a hoisted descendant inside that
    // zeroed subtree.
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; align-items: flex-start; }
         .flow { width: 40px; height: 30px; }
         .fixed { position: fixed; left: 10px; top: 20px; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let flow = h.doc.el(root, ".flow");
    let fixed = h.doc.el(flow, ".fixed");
    h.layout();

    assert_eq!(h.rect(flow), (0.0, 0.0, 40.0, 30.0));
    assert_eq!(h.rect(fixed), (0.0, 0.0, 0.0, 0.0));
}

#[test]
fn will_change_contain_establishes_the_fixed_containing_block() {
    // Will Change §2: naming `contain` must reproduce the containing block
    // a non-initial `contain` (layout/paint) would create.
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .host { display: flex; width: 300px; height: 200px; margin-left: 100px;
                 will-change: contain; }
         .fixed { position: fixed; left: 10px; top: 20px; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let host = h.doc.el(root, ".host");
    let fixed = h.doc.el(host, ".fixed");
    h.layout();

    // Captured by the host's padding box, not the viewport (which would be
    // parent-relative x = -90).
    assert_eq!(h.rect(fixed), (10.0, 20.0, 30.0, 40.0));
}

#[test]
fn will_change_position_establishes_the_absolute_containing_block() {
    // Will Change §2: naming `position` must reproduce the containing block
    // a non-initial `position` would create — for *absolute* descendants
    // (a positioned ancestor does not capture `fixed`).
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .host { display: flex; width: 300px; height: 200px; margin-left: 100px;
                 will-change: position; }
         .abs { position: absolute; left: 10px; top: 20px; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let host = h.doc.el(root, ".host");
    let abs = h.doc.el(host, ".abs");
    h.layout();

    // Resolved against the host's padding box, not the viewport (which
    // would be parent-relative x = -90).
    assert_eq!(h.rect(abs), (10.0, 20.0, 30.0, 40.0));
}

#[test]
fn root_will_change_filter_is_exempt_from_fixed_containing_block_creation() {
    // Will Change §2 reproduces the named property's behavior — including
    // Filter Effects §5's document-root exemption: `will-change: filter` on
    // the root must not capture fixed descendants (the WPT will-change
    // fixed-CB suite pins this), while the same declaration on a non-root
    // ancestor must.
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; border: 10px solid black;
                will-change: filter; }
         .host { display: flex; width: 300px; height: 200px; margin-left: 100px;
                 will-change: filter; }
         .fixed { position: fixed; left: 0; top: 0; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let root_fixed = h.doc.el(root, ".fixed");
    let host = h.doc.el(root, ".host");
    let captured_fixed = h.doc.el(host, ".fixed");
    h.layout();

    // Exempt: anchored to the viewport origin, not the root's padding box
    // (which starts at (10, 10) inside the border).
    assert_eq!(h.rect(root_fixed), (0.0, 0.0, 30.0, 40.0));
    // Non-root `will-change: filter` captures normally.
    assert_eq!(h.rect(captured_fixed), (0.0, 0.0, 30.0, 40.0));
    assert_eq!(h.rect(host).0, 110.0); // border 10 + margin 100
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

// --- damage → layout wiring ---------------------------------------------------
//
// `layout_document` consumes its own flush's restyle damage into layout
// invalidation, so a plain style change re-lays-out with no explicit
// `invalidate_layout` call, and a `contain: strict` boundary stops the
// invalidation walk so its ancestors keep their caches.

/// A [`MeasureLeaf`] payload that reports a fixed size for text-bearing leaves
/// and tallies how often the engine measured one — the "did layout do work?"
/// probe for the incremental-relayout tests. The tally is shared across every
/// node in the document.
#[derive(Debug)]
struct CountingMeasure {
    width: f32,
    height: f32,
    measures: Arc<AtomicUsize>,
}

impl CountingMeasure {
    fn new(width: f32, height: f32, measures: &Arc<AtomicUsize>) -> Self {
        Self {
            width,
            height,
            measures: Arc::clone(measures),
        }
    }
}

impl w3c_dom::ExternalState for CountingMeasure {}

impl MeasureLeaf for CountingMeasure {
    fn measure_leaf(&self, node: &w3c_dom::Node<Self>, _input: LeafMeasureInput) -> LeafMetrics {
        if node.text().is_some() {
            self.measures.fetch_add(1, Ordering::Relaxed);
            LeafMetrics::new(Size::new(self.width, self.height))
        } else {
            LeafMetrics::default()
        }
    }
}

#[test]
fn style_width_change_relayouts_without_manual_invalidation() {
    // A width change is RELAYOUT damage; layout_document consumes it and
    // re-lays-out with NO explicit invalidate_layout call.
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 50px; }
         view { flex-grow: 1; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, "view");
    let b = h.doc.el(root, "view");
    h.layout();
    assert_eq!(h.rect(a).2, 100.0);
    assert_eq!(h.rect(b).2, 100.0);

    h.doc.set_inline(a, "flex-grow: 0; width: 40px");
    h.layout(); // no h.doc.dom.invalidate_layout anywhere

    assert_eq!(h.rect(a).2, 40.0);
    assert_eq!(h.rect(b).2, 160.0);
}

#[test]
fn standalone_style_flush_preserves_relayout_for_next_layout() {
    let mut h = Harness::new("page { display: flex; width: 200px; height: 50px; }");
    let root = h.doc.root;
    let child = h.doc.el(root, "view");
    h.doc.set_inline(child, "width: 40px");
    h.layout();
    assert_eq!(h.rect(child).2, 40.0);

    h.doc.set_inline(child, "width: 60px");
    let summary = h.doc.flush();
    assert!(
        summary
            .damage
            .iter()
            .any(|&(id, damage)| id == child && damage.needs_relayout()),
        "the standalone flush reports RELAYOUT damage for the width change",
    );
    drop(summary);

    // The following layout's internal flush is a no-op. The standalone flush
    // must nevertheless have preserved its relayout effect internally rather
    // than leaving the warm 40px layout cache valid forever.
    h.layout();
    assert_eq!(h.rect(child).2, 60.0);
}

#[test]
fn removed_boundary_is_not_replayed_after_its_node_id_is_reused() {
    let mut h = Harness::new("page { display: flex; width: 300px; height: 100px; }");
    let root = h.doc.root;
    let outer = h.doc.el(root, "view");
    let parent = h.doc.el(outer, "view");
    let old_boundary = h.doc.el(parent, "view");
    let old_child = h.doc.el(old_boundary, "view");
    let hidden = h.doc.el(root, "view");
    h.doc.set_inline(
        outer,
        "display: flex; contain: strict; width: 120px; height: 60px",
    );
    h.doc
        .set_inline(parent, "display: flex; width: 100%; height: 100%");
    h.doc.set_inline(
        old_boundary,
        "display: flex; contain: strict; width: 60px; height: 40px",
    );
    h.doc.set_inline(old_child, "width: 20px; height: 20px");
    h.doc.set_inline(
        hidden,
        "display: flex; content-visibility: hidden; width: 100px; height: 50px",
    );
    h.layout();

    // The standalone flush parks `old_boundary` for an in-place relayout.
    h.doc.set_inline(old_child, "width: 30px; height: 20px");
    h.doc.flush();

    // Remove the parked boundary before the next layout. Invalidating its old
    // parent is the documented structural-mutation half of the layout seam;
    // the surrounding boundary keeps the document root warm.
    assert_eq!(h.doc.dom.remove_subtree(old_boundary).len(), 2);
    h.doc.dom.invalidate_layout(parent);

    // `Slab` reuses the two freed slots. Put a new containment boundary in the
    // old boundary's slot, but under skipped contents where the root pass must
    // never lay it or its child out.
    let first_reused = h.doc.dom.create_node("view", ());
    let second_reused = h.doc.dom.create_node("view", ());
    let reused_boundary = if first_reused == old_boundary {
        first_reused
    } else {
        assert_eq!(second_reused, old_boundary, "the freed slot is reused");
        second_reused
    };
    let reused_child = h.doc.dom.create_node("view", ());
    h.doc.dom.append(reused_boundary, reused_child);
    h.doc.dom.append(hidden, reused_boundary);
    h.doc.set_inline(
        reused_boundary,
        "display: flex; contain: strict; width: 80px; height: 40px",
    );
    h.doc.set_inline(reused_child, "width: 10px; height: 10px");
    h.doc.dom.invalidate_layout(reused_boundary);

    h.layout();
    assert_eq!(
        h.rect(reused_child),
        (0.0, 0.0, 0.0, 0.0),
        "a stale parked root must not lay out a replacement node under skipped contents",
    );
}

#[test]
fn color_only_change_relayouts_nothing() {
    // A paint-only (REPAINT) change produces no relayout damage, so
    // layout_document invalidates nothing and the second pass answers entirely
    // from cache — the leaf is never re-measured.
    let measures = Arc::new(AtomicUsize::new(0));
    let mut engine = w3c_dom::StyleEngine::new(common::device(800.0, 600.0));
    engine.add_stylesheet_str(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start; }",
        w3c_dom::StylesheetOrigin::Author,
    );
    let mut dom = engine.new_document();
    let root = dom.create_element("page", CountingMeasure::new(0.0, 0.0, &measures));
    dom.append_child(root);
    let text = dom.create_element("text", CountingMeasure::new(30.0, 12.0, &measures));
    dom.set_text(text, Some("hello".into()));
    dom.append(root, text);

    engine.layout_document(&mut dom);
    let after_first = measures.load(Ordering::Relaxed);
    assert!(after_first >= 1, "the initial pass measures the text leaf");

    dom.set_inline_style(text, "color: rgb(0, 0, 255)");
    engine.layout_document(&mut dom);
    assert_eq!(
        measures.load(Ordering::Relaxed),
        after_first,
        "a color-only change re-measures nothing",
    );
    // No relayout damage means no invalidation: the whole measurement-cache
    // spine survives, so the second pass answered entirely from cache.
    assert!(
        !dom.get(text).unwrap().layout_cache_is_empty(),
        "the leaf keeps its measurement cache across a paint-only change",
    );
    assert!(
        !dom.get(root).unwrap().layout_cache_is_empty(),
        "the leaf's ancestor keeps its measurement cache too",
    );
}

#[test]
fn contain_strict_boundary_keeps_ancestor_caches_and_relayouts_interior() {
    // The `contain: strict` box is a relayout boundary: an interior mutation
    // clears the dirty node and the boundary, but leaves the boundary's
    // ancestor (the root) cache warm — and the interior still re-lays-out (the
    // boundary is re-run in place).
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start; }
         .outer { display: flex; contain: strict; width: 80px; height: 80px; }
         .inner { width: 30px; height: 30px; }",
    );
    let root = h.doc.root;
    let outer = h.doc.el(root, ".outer");
    let inner = h.doc.el(outer, ".inner");
    h.layout();
    assert_eq!(h.rect(outer).2, 80.0);
    assert_eq!(h.rect(inner).2, 30.0);

    // Harvesting the standalone flush's damage performs boundary-stopped
    // invalidation even though its summary is discarded.
    h.doc.set_inline(inner, "width: 50px; height: 30px");
    h.doc.flush();
    assert!(h.node_cache_empty(inner), "the dirty node is cleared");
    assert!(h.node_cache_empty(outer), "the boundary itself is cleared");
    assert!(
        !h.node_cache_empty(root),
        "the boundary's ancestor keeps its cache",
    );

    h.layout();
    assert_eq!(h.rect(inner).2, 50.0, "the boundary interior re-lays-out");
    assert_eq!(
        h.rect(outer).2,
        80.0,
        "the contained box keeps its outer size"
    );
}

#[test]
fn uncontained_interior_change_clears_the_ancestor_caches() {
    // The control for the boundary test: with no containment the walk runs to
    // the document root, so the ancestor's cache is cleared.
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start; }
         .outer { display: flex; width: 80px; height: 80px; }
         .inner { width: 30px; height: 30px; }",
    );
    let root = h.doc.root;
    let outer = h.doc.el(root, ".outer");
    let inner = h.doc.el(outer, ".inner");
    h.layout();

    h.doc.set_inline(inner, "width: 50px; height: 30px");
    h.doc.flush();
    assert!(h.node_cache_empty(outer), "the container is cleared");
    assert!(
        h.node_cache_empty(root),
        "the ancestor is cleared — no boundary stops the walk",
    );
}

#[test]
fn contained_interior_relayouts_automatically() {
    // The automatic path end to end: the flush's RELAYOUT damage on `inner`
    // drives a boundary-stopped invalidate and the boundary re-root, with no
    // manual invalidate_layout call.
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start; }
         .outer { display: flex; contain: strict; width: 80px; height: 80px; }
         .inner { width: 30px; height: 30px; }",
    );
    let root = h.doc.root;
    let outer = h.doc.el(root, ".outer");
    let inner = h.doc.el(outer, ".inner");
    h.layout();
    assert_eq!(h.rect(inner).2, 30.0);

    h.doc.set_inline(inner, "width: 50px; height: 30px");
    h.layout(); // automatic: no manual invalidate

    assert_eq!(h.rect(inner).2, 50.0);
    assert_eq!(h.rect(outer).2, 80.0);
}

#[test]
fn a_damaged_boundary_still_clears_its_ancestors() {
    // Decision: the boundary test applies to *ancestors*, never to the damaged
    // node itself. A `contain: strict` node whose own style changes can still
    // resize (size containment isolates it from its contents, not from its own
    // box), so its ancestors must be cleared.
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start; }
         .boundary { display: flex; contain: strict; width: 60px; height: 60px; }
         .inner { width: 20px; height: 20px; }",
    );
    let root = h.doc.root;
    let boundary = h.doc.el(root, ".boundary");
    h.doc.el(boundary, ".inner");
    h.layout();
    assert_eq!(h.rect(boundary).2, 60.0);

    h.doc.set_inline(boundary, "width: 90px; height: 60px");
    h.doc.flush();
    assert!(
        h.node_cache_empty(boundary),
        "the damaged boundary is cleared"
    );
    assert!(
        h.node_cache_empty(root),
        "its ancestor is cleared: the boundary's own size can change",
    );

    h.layout();
    assert_eq!(h.rect(boundary).2, 90.0, "its own size change takes effect");
}

#[test]
fn display_flip_relayouts_the_parent_automatically() {
    // A child's display flip changes box generation; the parent re-collects its
    // children on the next layout_document, with no manual invalidate.
    let mut h = Harness::new(
        "page { display: flex; width: 100px; height: 40px; }
         view { flex-grow: 1; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, "view");
    let b = h.doc.el(root, "view");
    h.layout();
    assert_eq!(h.rect(a).2, 50.0);
    assert_eq!(h.rect(b).2, 50.0);

    h.doc.set_inline(a, "display: none");
    h.layout();
    assert_eq!(h.rect(a).2, 0.0);
    assert_eq!(h.rect(b).2, 100.0);

    h.doc.set_inline(a, "");
    h.layout();
    assert_eq!(h.rect(a).2, 50.0);
    assert_eq!(h.rect(b).2, 50.0);
}

#[test]
fn boundary_reroot_and_root_pass_coexist_in_one_flush() {
    // One flush carries both a contained-interior change (parked as a boundary
    // re-root) and a clears-to-root change (a sibling). The boundary re-run and
    // the root pass must both land: the boundary interior updates while its
    // fixed outer size holds, and the sibling re-lays-out.
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start; }
         .outer { display: flex; contain: strict; width: 80px; height: 80px; }
         .inner { width: 30px; height: 30px; }
         .sib { width: 40px; height: 40px; }",
    );
    let root = h.doc.root;
    let outer = h.doc.el(root, ".outer");
    let inner = h.doc.el(outer, ".inner");
    let sib = h.doc.el(root, ".sib");
    h.layout();
    assert_eq!(h.rect(inner).2, 30.0);
    assert_eq!(h.rect(sib), (80.0, 0.0, 40.0, 40.0));

    h.doc.set_inline(inner, "width: 50px; height: 30px");
    h.doc.set_inline(sib, "width: 70px; height: 40px");
    h.layout(); // one automatic pass drives both

    assert_eq!(h.rect(inner).2, 50.0, "the boundary interior updates");
    assert_eq!(h.rect(outer).2, 80.0, "the contained outer size holds");
    assert_eq!(h.rect(sib).2, 70.0, "the clears-to-root sibling updates");
}

#[test]
fn nested_boundaries_relayout_deepest_first() {
    // page > B1(contain:strict) > M(flex) > B2(contain:strict, flex-grow) >
    // Dinner. One flush changes M's horizontal padding (resizing B2's flex-imposed
    // width) AND Dinner's height. M's damage parks B1; Dinner's parks B2 — in that
    // outer-first push order. Re-running the parked boundaries in push order lets
    // B2's stale committed input (the OLD imposed width) overwrite Dinner *after*
    // B1 already re-laid it at the NEW width, and the root pass cannot repair it
    // (B1's ancestors stay warm). Re-running deepest-first (B2 then B1) lets the
    // outer boundary have the final say.
    let mut h = Harness::new(
        "page { display: flex; width: 400px; height: 300px; align-items: flex-start; }
         .b1 { display: flex; contain: strict; width: 200px; height: 200px; }
         .m { display: flex; flex-grow: 1; padding-left: 10px; padding-right: 10px; }
         .b2 { display: flex; flex-direction: column; contain: strict; flex-grow: 1;
               align-items: stretch; }
         .dinner { height: 20px; }",
    );
    let root = h.doc.root;
    let b1 = h.doc.el(root, ".b1");
    let m = h.doc.el(b1, ".m");
    let b2 = h.doc.el(m, ".b2");
    let dinner = h.doc.el(b2, ".dinner");
    h.layout();
    // Initial: M content width = 200 - 2*10 = 180, so B2 and Dinner are 180 wide.
    assert_eq!(h.rect(b2).2, 180.0);
    assert_eq!(h.rect(dinner).2, 180.0);
    assert_eq!(h.rect(dinner).3, 20.0);

    // One flush: grow M's horizontal padding (B2's imposed width 180 → 140) and
    // change Dinner's height (the interior mutation that parks B2).
    h.doc
        .set_inline(m, "padding-left: 30px; padding-right: 30px");
    h.doc.set_inline(dinner, "height: 40px");
    h.layout(); // automatic: both parked, re-run deepest-first

    // Dinner ends at the NEW imposed width (140), not B2's stale-replay 180.
    assert_eq!(
        h.rect(dinner).2,
        140.0,
        "Dinner tracks B2's new parent-imposed width, not the stale inner replay"
    );
    assert_eq!(h.rect(dinner).3, 40.0, "Dinner's own height change applied");
    assert_eq!(
        h.rect(b2).2,
        140.0,
        "B2's outer width is the new imposed size"
    );
}

#[test]
fn boundary_own_and_interior_change_in_one_flush() {
    // A boundary whose OWN size changes AND an interior descendant change, in one
    // flush. The boundary's own damage clears to the root (its outer size can
    // change) — which clears the boundary's own cache first — so the interior
    // walk finds no committed input to park and also clears to the root; the root
    // pass then re-lays-out everything. Pins that this (currently-correct)
    // interleaving survives the already-parked stop: the boundary is never parked
    // here, so case 3 (never-laid-out) still applies and clears upward.
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start; }
         .boundary { display: flex; contain: strict; width: 60px; height: 60px; }
         .inner { width: 20px; height: 20px; }",
    );
    let root = h.doc.root;
    let boundary = h.doc.el(root, ".boundary");
    let inner = h.doc.el(boundary, ".inner");
    h.layout();
    assert_eq!(h.rect(boundary).2, 60.0);
    assert_eq!(h.rect(inner).2, 20.0);

    h.doc.set_inline(boundary, "width: 90px; height: 60px");
    h.doc.set_inline(inner, "width: 50px; height: 20px");
    h.layout(); // automatic: both changes ride one flush

    assert_eq!(
        h.rect(boundary).2,
        90.0,
        "the boundary's own size change applied"
    );
    assert_eq!(h.rect(inner).2, 50.0, "the interior change applied");
}

#[test]
fn two_damaged_nodes_under_one_boundary_keep_root_warm() {
    // Two interior nodes under one boundary, both damaged in one flush. The first
    // walk parks the boundary and clears its cache; the second must recognize the
    // boundary as ALREADY PARKED and stop, rather than seeing its now-empty
    // committed slot and clearing on to the root. Pins FINDING 2: the ancestor
    // (root) cache stays warm after BOTH invalidations.
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start; }
         .outer { display: flex; contain: strict; width: 80px; height: 80px; }
         .a { width: 20px; height: 20px; }
         .b { width: 20px; height: 20px; }",
    );
    let root = h.doc.root;
    let outer = h.doc.el(root, ".outer");
    let a = h.doc.el(outer, ".a");
    let b = h.doc.el(outer, ".b");
    h.layout();
    assert_eq!(h.rect(a).2, 20.0);
    assert_eq!(h.rect(b).3, 20.0);

    // The standalone flush consumes both damage entries. The second
    // invalidation must stop at the boundary parked by the first rather than
    // clearing through it to the root.
    h.doc.set_inline(a, "width: 30px; height: 20px");
    h.doc.set_inline(b, "width: 20px; height: 30px");
    h.doc.flush();
    assert!(h.node_cache_empty(a), "the first damaged node is cleared");
    assert!(h.node_cache_empty(b), "the second damaged node is cleared");
    assert!(h.node_cache_empty(outer), "the boundary is cleared");
    assert!(
        !h.node_cache_empty(root),
        "the boundary's ancestor stays warm after both invalidations",
    );

    h.layout();
    assert_eq!(h.rect(a).2, 30.0, "the first interior change applied");
    assert_eq!(h.rect(b).3, 30.0, "the second interior change applied");
    assert_eq!(h.rect(outer).2, 80.0, "the contained outer size holds");
}

// --- content-visibility skipping + implied containment ------------------------
//
// `content-visibility: hidden` skips laying out its contents (P1-1); the
// containment it (and `auto`) implies establishes the fixed/absolute containing
// block (P1-2); and a `contain: strict` boundary re-run refreshes its stored
// scrollable overflow (P1-3).

#[test]
fn content_visibility_hidden_skips_descendant_layout_and_measurement() {
    // `content-visibility: hidden` sizes the container from its own styles and
    // skips laying out its contents: descendants get zero geometry, their
    // `MeasureLeaf` hook is never called, and a `position: fixed` descendant
    // generates no positioned box. Revealing the container restores layout.
    let measures = Arc::new(AtomicUsize::new(0));
    let mut engine = w3c_dom::StyleEngine::new(common::device(800.0, 600.0));
    engine.add_stylesheet_str(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start; }
         .container { display: flex; width: 60px; height: 80px; align-items: flex-start; }
         .fixed { position: fixed; left: 10px; top: 20px; width: 30px; height: 40px; }",
        w3c_dom::StylesheetOrigin::Author,
    );
    let mut dom = engine.new_document();
    let root = dom.create_element("page", CountingMeasure::new(0.0, 0.0, &measures));
    dom.append_child(root);
    let container = dom.create_element("view", CountingMeasure::new(0.0, 0.0, &measures));
    dom.add_class(container, "container");
    dom.set_inline_style(container, "content-visibility: hidden");
    dom.append(root, container);
    let text = dom.create_element("text", CountingMeasure::new(50.0, 70.0, &measures));
    dom.set_text(text, Some("hi".into()));
    dom.append(container, text);
    let fixed = dom.create_element("view", CountingMeasure::new(0.0, 0.0, &measures));
    dom.add_class(fixed, "fixed");
    dom.append(container, fixed);

    engine.layout_document(&mut dom);

    // A borrow-free reader (takes `dom` by reference) so it can be used on both
    // sides of the reveal mutation below.
    let rect = |dom: &w3c_dom::Document<CountingMeasure>, id: NodeId| {
        let l = dom.get(id).expect("live").layout();
        (l.location.x, l.location.y, l.size.width, l.size.height)
    };
    // The container still generates its own box, sized purely from its styles.
    assert_eq!(rect(&dom, container), (0.0, 0.0, 60.0, 80.0));
    // Its contents are skipped: zeroed, and the text leaf is never measured.
    assert_eq!(rect(&dom, text), (0.0, 0.0, 0.0, 0.0));
    // A `position: fixed` descendant inside the skipped subtree produces no
    // positioned box (the positioned pass prunes at the skip root).
    assert_eq!(rect(&dom, fixed), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(
        measures.load(Ordering::Relaxed),
        0,
        "a skipped text leaf is never measured",
    );

    // Reveal the container: its contents lay out again (transition cleanliness).
    dom.set_inline_style(container, "");
    engine.layout_document(&mut dom);
    assert_eq!(
        rect(&dom, text),
        (0.0, 0.0, 50.0, 70.0),
        "revealing the container lays its text back out",
    );
    assert!(
        measures.load(Ordering::Relaxed) >= 1,
        "the revealed text leaf is now measured",
    );
}

#[test]
fn content_visibility_auto_establishes_the_fixed_containing_block() {
    // css-contain-2 / CSS Position: `content-visibility: auto` implies layout +
    // paint containment, which establishes the containing block for fixed (and
    // absolute) descendants — a fixed child resolves against the host, not the
    // viewport. `auto` does not skip contents in v1, so the child is a normal
    // in-flow-captured absolute box.
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .host { display: flex; width: 300px; height: 200px; margin-left: 100px;
                 margin-top: 50px; }
         .cv { content-visibility: auto; }
         .fixed { position: fixed; left: 10px; top: 20px; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    // The plain host is the first flex item (origin (100, 50)); the cv host is
    // the second (origin (500, 50)). A captured fixed child is host-relative, so
    // its rect is position-independent — but a viewport-anchored one is not.
    let plain = h.doc.el(root, ".host");
    let plain_fixed = h.doc.el(plain, ".fixed");
    let cv = h.doc.el(root, ".host.cv");
    let cv_fixed = h.doc.el(cv, ".fixed");
    h.layout();

    // Control (no containment): the fixed child anchors to the viewport, stored
    // parent-relative — viewport (10, 20) minus the plain host origin (100, 50).
    assert_eq!(h.rect(plain_fixed), (-90.0, -30.0, 30.0, 40.0));
    // The content-visibility:auto host captures it: resolved against the host's
    // padding box, so it sits at its own inset (10, 20) relative to the host,
    // wherever the host is — the reviewer's (100,50)+(10,20)→(110,70) shape,
    // expressed host-relative. (Pre-fix it hoisted to the viewport at
    // (10 - 500, 20 - 50) = (-490, -30).)
    assert_eq!(h.rect(cv_fixed), (10.0, 20.0, 30.0, 40.0));
}

#[test]
fn contained_boundary_relayout_refreshes_scrollable_content_size() {
    // A `contain: strict` boundary that is ALSO a scroll container
    // (`overflow: hidden`) has a fixed outer size (the relayout-boundary theorem)
    // but a `content_size` (scrollable overflow) that tracks its interior: as a
    // scroll container it keeps its full interior union as its own scroll range
    // (css-overflow-3 §3.3), even though that no longer leaks to the root. A child
    // growing past the boundary must refresh the STORED content_size even though
    // the boundary is re-run in place via `compute_boundary_relayout`, which
    // deliberately does not restore the boundary's own `Layout`. The merge lands
    // before the rounding pass snaps it.
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 200px; align-items: flex-start; }
         .scroll { display: flex; flex-direction: column; contain: strict; overflow: hidden;
                   width: 80px; height: 80px; align-items: flex-start; }
         .child { width: 40px; height: 30px; flex-shrink: 0; }",
    );
    let root = h.doc.root;
    let scroll = h.doc.el(root, ".scroll");
    let child = h.doc.el(scroll, ".child");
    h.layout();

    let content_height = |harness: &Harness, id: NodeId| {
        harness
            .doc
            .dom
            .get(id)
            .expect("live")
            .layout()
            .content_size
            .height
    };
    // The 30px child fits inside the 80px box: content_size equals the border box.
    assert_eq!(h.rect(scroll), (0.0, 0.0, 80.0, 80.0));
    assert_eq!(content_height(&h, scroll), 80.0);

    // Grow the child past the boundary via the automatic damage path (no manual
    // invalidate): the boundary is parked and re-run in place.
    h.doc.set_inline(child, "height: 120px");
    h.layout();

    // The outer size still holds (the theorem)...
    assert_eq!(h.rect(scroll), (0.0, 0.0, 80.0, 80.0));
    // ...while the stored content_size now reflects the 120px interior — the
    // merged value the rounding pass snapped (reviewer repro: 30→120, no longer
    // stale at 80).
    assert_eq!(content_height(&h, scroll), 120.0);
    assert_eq!(
        h.doc
            .dom
            .get(child)
            .expect("live")
            .unrounded_layout()
            .size
            .height,
        120.0,
        "the boundary interior actually re-laid-out",
    );
}

#[test]
fn layout_contained_visible_boundary_excludes_descendant_scrollable_overflow() {
    // css-contain-2 §3.3 companion to the test above: the same shape with
    // `overflow: visible` on the contained boundary. Layout containment makes the
    // 120px child's overflow *ink* overflow (item 3), so the boundary's scrollable
    // overflow equals its own 80px border box — it does NOT include the child.
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 200px; align-items: flex-start; }
         .scroll { display: flex; flex-direction: column; contain: strict; overflow: visible;
                   width: 80px; height: 80px; align-items: flex-start; }
         .child { width: 40px; height: 120px; flex-shrink: 0; }",
    );
    let root = h.doc.root;
    let scroll = h.doc.el(root, ".scroll");
    let child = h.doc.el(scroll, ".child");
    h.layout();

    // The child is still laid out and overflows the box...
    assert_eq!(
        h.doc
            .dom
            .get(child)
            .expect("live")
            .unrounded_layout()
            .size
            .height,
        120.0,
        "the descendant is laid out (only its overflow is ink-only)",
    );
    // ...but the boundary's scrollable overflow is just its border box (§3.3).
    assert_eq!(
        h.doc
            .dom
            .get(scroll)
            .expect("live")
            .layout()
            .content_size
            .height,
        80.0,
        "layout containment + overflow:visible excludes descendant overflow",
    );
}

#[test]
fn boundary_scrollable_overflow_is_consistent_across_incremental_and_cold_layout() {
    // Reviewer's repro: an 80px root contains a scroll-container boundary
    // (`contain: strict; overflow: hidden`, 60x60) whose child grows 30 -> 120.
    // The boundary traps its interior, so it contributes only its 60px border box
    // to the root: boundary.content_size == 120 (its own scroll range) but
    // root.content_size == 80. This must hold BOTH via the incremental damage
    // path (parked boundary re-run) AND after a cold `invalidate_layout_all` — the
    // cold path previously leaked the boundary's 120 up to the root.
    let mut h = Harness::new(
        "page { display: flex; flex-direction: column; width: 80px; height: 80px;
                align-items: flex-start; }
         .boundary { display: flex; flex-direction: column; contain: strict; overflow: hidden;
                     width: 60px; height: 60px; align-items: flex-start; }
         .child { width: 40px; height: 30px; flex-shrink: 0; }",
    );
    let root = h.doc.root;
    let boundary = h.doc.el(root, ".boundary");
    let child = h.doc.el(boundary, ".child");
    h.layout();

    let content_h = |harness: &Harness, id: NodeId| {
        harness
            .doc
            .dom
            .get(id)
            .expect("live")
            .layout()
            .content_size
            .height
    };
    // Initial: the 30px child fits the 60px boundary; the root sees the border box.
    assert_eq!(content_h(&h, boundary), 60.0);
    assert_eq!(content_h(&h, root), 80.0);

    // Grow the child past the boundary via the automatic damage path (parked
    // boundary re-run, root left warm).
    h.doc.set_inline(child, "height: 120px");
    h.layout();
    assert_eq!(
        content_h(&h, boundary),
        120.0,
        "incremental: the boundary tracks its interior scroll range",
    );
    assert_eq!(
        content_h(&h, root),
        80.0,
        "incremental: the boundary is trapped, so the root stays at its border box",
    );

    // A cold full relayout must agree by construction (this is where the leak was:
    // the root previously accumulated the boundary's 120 instead of its border box).
    h.doc.dom.invalidate_layout_all();
    h.layout();
    assert_eq!(
        content_h(&h, boundary),
        120.0,
        "cold: the boundary tracks its interior scroll range",
    );
    assert_eq!(
        content_h(&h, root),
        80.0,
        "cold: the boundary is trapped, so the root matches the incremental path",
    );
}

#[test]
fn mutation_inside_a_skipped_container_keeps_ancestor_caches_warm() {
    // A `content-visibility: hidden` container lays out none of its contents and
    // bypasses the measurement cache, so it never records a committed input. An
    // interior mutation must NOT empty the warm ancestor caches: boundary case 0
    // stops the invalidation walk at the skipped container immediately (it folds
    // LAYOUT|SIZE, so without case 0 it would fall through the "never laid out"
    // case and clear every ancestor cache up to the root).
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 200px; }
         .hidden { display: flex; content-visibility: hidden;
                   contain-intrinsic-size: 40px 30px; width: 40px; height: 30px; }
         .child { width: 20px; height: 20px; }",
    );
    let root = h.doc.root;
    let hidden = h.doc.el(root, ".hidden");
    let child = h.doc.el(hidden, ".child");
    h.layout();

    // Warm: the root holds its committed layout; the hidden box is sized from
    // its own styles and its child is not laid out.
    assert!(
        !h.node_cache_empty(root),
        "the root cache is warm after the first pass",
    );
    assert_eq!(h.rect(hidden), (0.0, 0.0, 40.0, 30.0));

    // Invalidate a descendant of the skipped container. The walk must stop at the
    // skipped box (case 0) and leave the root cache warm.
    h.doc.dom.invalidate_layout(child);
    assert!(
        !h.node_cache_empty(root),
        "the root cache stays warm past a skipped container",
    );

    // A subsequent pass is still correct (the root answers from its warm cache).
    h.layout();
    assert_eq!(h.rect(hidden), (0.0, 0.0, 40.0, 30.0));

    // Flipping hidden -> visible produces correct fresh layout: the flip's own
    // RELAYOUT damage on the container drives the real invalidation, so the child
    // is now laid out inside the (visible) container.
    h.doc.set_inline(hidden, "content-visibility: visible");
    h.layout();
    assert_eq!(h.rect(child), (0.0, 0.0, 20.0, 20.0));
}
