//! Behavior tests for the `w3c_dom::layout` integration: the neutron-star
//! host protocol implemented over `Document<T>` + stylo computed styles.

#![allow(clippy::float_cmp)]

mod common;

use common::{Doc, device_with};
use stylo::device::Device;
use stylo::queries::values::PrefersColorScheme;
use w3c_dom::NodeId;
use w3c_dom::layout::Layout;

const AHEM: &[u8] = include_bytes!("../../neutron-star/tests/fixtures/Ahem.ttf");

/// [`Doc`] plus helpers for document-owned layout results.
struct Harness {
    doc: Doc,
}

#[derive(Debug, PartialEq)]
struct LayoutSnapshot {
    order: u32,
    geometry: [f32; 18],
}

impl LayoutSnapshot {
    fn of(layout: &Layout) -> Self {
        Self {
            order: layout.order,
            geometry: [
                layout.location.x,
                layout.location.y,
                layout.size.width,
                layout.size.height,
                layout.content_size.width,
                layout.content_size.height,
                layout.border.left,
                layout.border.right,
                layout.border.top,
                layout.border.bottom,
                layout.padding.left,
                layout.padding.right,
                layout.padding.top,
                layout.padding.bottom,
                layout.margin.left,
                layout.margin.right,
                layout.margin.top,
                layout.margin.bottom,
            ],
        }
    }
}

fn dom_rect(dom: &w3c_dom::Document<()>, id: NodeId) -> (f32, f32, f32, f32) {
    let layout = dom.rounded_layout(id).expect("node id is live");
    (
        layout.location.x,
        layout.location.y,
        layout.size.width,
        layout.size.height,
    )
}

impl Harness {
    fn new(css: &str) -> Self {
        Self {
            doc: Doc::with_css(css),
        }
    }

    fn with_device(css: &str, device: Device) -> Self {
        let mut doc = Doc::with_device(device);
        doc.add_css(css);
        Self { doc }
    }

    fn layout(&mut self) {
        self.doc.dom.layout();
    }

    fn layout_of(&self, id: NodeId) -> &Layout {
        self.doc.dom.rounded_layout(id).expect("node id is live")
    }

    fn layouts_of(&self, ids: &[NodeId]) -> Vec<LayoutSnapshot> {
        ids.iter()
            .map(|&id| LayoutSnapshot::of(self.layout_of(id)))
            .collect()
    }

    fn rect(&self, id: NodeId) -> (f32, f32, f32, f32) {
        let layout = self.layout_of(id);
        (
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        )
    }

    fn node_cache_empty(&self, id: NodeId) -> bool {
        self.doc
            .dom
            .layout_cache_is_empty(id)
            .expect("node id is live")
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

    assert_eq!(h.rect(root).2, 200.0);
    let (_, _, width, height) = h.rect(a);
    assert_eq!((width, height), (45.0, 40.0));
    assert_eq!(h.rect(a).0, 20.0);
    assert_eq!(h.rect(b).0, 85.0);
}

#[test]
fn content_box_sizing_is_the_default_and_padding_grows_the_border_box() {
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
    assert_eq!(width, 90.0);
    assert_eq!(height, 20.0);
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
    assert_eq!(layout.size.width, 100.0);
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
    assert_eq!(h.rect(right).0, 170.0);
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

    assert_eq!(h.rect(fixed_to_viewport), (-90.0, -30.0, 30.0, 40.0));
    assert_eq!(h.rect(fixed_to_ancestor), (10.0, 20.0, 30.0, 40.0));
}

#[test]
fn fixed_stays_viewport_anchored_when_its_parent_answers_from_cache() {
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
    assert_eq!(h.rect(fixed), (-90.0, 20.0, 30.0, 40.0));

    h.doc.set_inline(spacer, "width: 150px");
    h.doc.dom.invalidate_layout(spacer);
    h.layout();

    assert_eq!(h.rect(host).0, 150.0);
    assert_eq!(h.rect(fixed), (-140.0, 20.0, 30.0, 40.0));
}

#[test]
fn hoisted_children_paint_with_effective_order_zero() {
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

    assert_eq!(h.layout_of(fixed).order, 0);
    assert_eq!(h.layout_of(plain).order, 1);
    assert_eq!(h.layout_of(early).order, 2);
}

#[test]
fn offset_path_establishes_the_fixed_containing_block() {
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

    assert_eq!(h.rect(fixed), (10.0, 20.0, 30.0, 40.0));
}

#[test]
fn will_change_position_establishes_the_absolute_containing_block() {
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

    assert_eq!(h.rect(abs), (10.0, 20.0, 30.0, 40.0));
}

#[test]
fn root_will_change_filter_is_exempt_from_fixed_containing_block_creation() {
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

    assert_eq!(h.rect(root_fixed), (0.0, 0.0, 30.0, 40.0));
    assert_eq!(h.rect(captured_fixed), (0.0, 0.0, 30.0, 40.0));
    assert_eq!(h.rect(host).0, 110.0);
}

#[test]
fn root_filter_is_exempt_from_fixed_containing_block_creation() {
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

    assert_eq!(h.rect(root_fixed), (0.0, 0.0, 30.0, 40.0));
    assert_eq!(h.rect(captured_fixed), (0.0, 0.0, 30.0, 40.0));
    assert_eq!(h.rect(filtered).0, 110.0);
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
    assert_eq!(h.rect(inner), (10.0, 5.0, 20.0, 20.0));
}

#[test]
fn hoisted_nodes_relayout_across_passes() {
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

#[test]
fn text_nodes_use_parley_with_their_parents_inherited_style() {
    let mut dom = w3c_dom::Document::new(common::device(800.0, 600.0));
    dom.add_stylesheet(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start;
                font-family: Ahem; font-size: 16px; }
         .sibling { width: 50px; height: 10px; }",
        w3c_dom::StylesheetOrigin::Author,
    );
    assert_eq!(dom.register_fonts(AHEM), 1);
    let root = dom.create_element("page", ());
    dom.append_document_element(root);
    let sibling = dom.create_element("view", ());
    dom.add_class(sibling, "sibling");
    dom.append_child(root, sibling);
    let text = dom.create_text_node("hello", ());
    dom.append_child(root, text);
    dom.layout();

    assert_eq!(dom_rect(&dom, sibling), (0.0, 0.0, 50.0, 10.0));
    assert_eq!(dom_rect(&dom, text), (50.0, 0.0, 80.0, 16.0));

    dom.set_text_node_data(text, "hi");
    dom.layout();
    assert_eq!(dom_rect(&dom, text), (50.0, 0.0, 32.0, 16.0));
}

#[test]
fn inherited_text_style_change_remeasures_text_child() {
    let mut dom = w3c_dom::Document::new(common::device(800.0, 600.0));
    dom.add_stylesheet(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start;
                font-family: Ahem; font-size: 16px; }",
        w3c_dom::StylesheetOrigin::Author,
    );
    assert_eq!(dom.register_fonts(AHEM), 1);
    let root = dom.create_element("page", ());
    dom.append_document_element(root);
    let text = dom.create_text_node("hello", ());
    dom.append_child(root, text);

    dom.layout();
    assert_eq!(dom_rect(&dom, text), (0.0, 0.0, 80.0, 16.0));

    dom.set_inline_style(root, "font-size: 32px");
    dom.layout();

    assert_eq!(dom_rect(&dom, text), (0.0, 0.0, 160.0, 32.0));
}

#[test]
fn inherited_text_style_change_remeasures_nested_text_child() {
    let mut dom = w3c_dom::Document::new(common::device(800.0, 600.0));
    dom.add_stylesheet(
        "page, view { display: flex; width: 200px; height: 100px;
                      align-items: flex-start; }
         page { font-family: Ahem; font-size: 16px; }",
        w3c_dom::StylesheetOrigin::Author,
    );
    assert_eq!(dom.register_fonts(AHEM), 1);
    let root = dom.create_element("page", ());
    dom.append_document_element(root);
    let parent = dom.create_element("view", ());
    dom.append_child(root, parent);
    let text = dom.create_text_node("hello", ());
    dom.append_child(parent, text);

    dom.layout();
    assert_eq!(dom_rect(&dom, text), (0.0, 0.0, 80.0, 16.0));

    dom.set_inline_style(root, "font-size: 32px");
    dom.layout();

    assert_eq!(dom_rect(&dom, text), (0.0, 0.0, 160.0, 32.0));
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

    assert_eq!(h.layout_of(a).size.width, 20.5);
    assert_eq!(
        h.doc
            .dom
            .unrounded_layout(a)
            .expect("node id is live")
            .size
            .width,
        20.25
    );
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
    assert!(h.doc.dom.get(old).is_none());

    h.doc.dom.invalidate_layout(root);
    let new = h.doc.el(root, "view");
    h.layout();

    assert_eq!(h.rect(new).2, 100.0);
}

#[test]
fn viewport_percentages_resolve_against_the_engine_viewport() {
    let mut h = Harness::new("page { display: flex; width: 50%; height: 25%; }");
    h.layout();
    assert_eq!(h.rect(h.doc.root), (0.0, 0.0, 400.0, 150.0));
}

#[test]
fn layout_flushes_pending_styles_itself() {
    let mut h = Harness::new("page { display: flex; width: 200px; height: 50px; }");
    let root = h.doc.root;
    let child = h.doc.el(root, "view");
    h.doc.set_inline(child, "width: 60px");
    h.layout();

    assert_eq!(h.rect(child).2, 60.0);
}

#[test]
fn style_width_change_relayouts_without_manual_invalidation() {
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
    h.layout();

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
            .any(|entry| entry.node_id == child && entry.damage.needs_relayout()),
        "the standalone flush reports RELAYOUT damage for the width change",
    );
    drop(summary);

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

    h.doc.set_inline(old_child, "width: 30px; height: 20px");
    h.doc.flush();

    assert_eq!(h.doc.dom.remove_subtree(old_boundary).len(), 2);
    h.doc.dom.invalidate_layout(parent);

    let first_reused = h.doc.dom.create_element("view", ());
    let second_reused = h.doc.dom.create_element("view", ());
    let reused_boundary = if first_reused == old_boundary {
        first_reused
    } else {
        assert_eq!(second_reused, old_boundary, "the freed slot is reused");
        second_reused
    };
    let reused_child = h.doc.dom.create_element("view", ());
    h.doc.dom.append_child(reused_boundary, reused_child);
    h.doc.dom.append_child(hidden, reused_boundary);
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
    let mut dom = w3c_dom::Document::new(common::device(800.0, 600.0));
    dom.add_stylesheet(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start;
                font-family: Ahem; font-size: 16px; }",
        w3c_dom::StylesheetOrigin::Author,
    );
    assert_eq!(dom.register_fonts(AHEM), 1);
    let root = dom.create_element("page", ());
    dom.append_document_element(root);
    let text = dom.create_text_node("hello", ());
    dom.append_child(root, text);

    dom.layout();
    assert_eq!(dom_rect(&dom, text), (0.0, 0.0, 80.0, 16.0));
    assert_eq!(dom.layout_cache_is_empty(text), Some(false));
    assert_eq!(dom.layout_cache_is_empty(root), Some(false));

    dom.set_inline_style(root, "color: rgb(0, 0, 255)");
    let summary = dom.flush_styles();
    assert_eq!(
        summary
            .damage
            .iter()
            .filter(|entry| entry.damage.needs_relayout())
            .count(),
        0,
        "a color-only change produces no relayout damage",
    );
    assert!(
        !dom.layout_cache_is_empty(text)
            .expect("text node remains live"),
        "the Parley leaf keeps its measurement cache after the flush",
    );
    assert!(
        !dom.layout_cache_is_empty(root)
            .expect("root node remains live"),
        "the leaf's ancestor keeps its measurement cache after the flush",
    );

    dom.layout();
    assert_eq!(dom_rect(&dom, text), (0.0, 0.0, 80.0, 16.0));
}

#[test]
fn contain_strict_boundary_keeps_ancestor_caches_and_relayouts_interior() {
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
fn warm_cache_append_invalidates_the_new_parent_spine() {
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 40px; align-items: flex-start; }
         view { width: 40px; height: 20px; }",
    );
    let root = h.doc.root;
    let first = h.doc.el(root, "view");
    h.layout();
    assert!(!h.node_cache_empty(root));

    let appended = h.doc.dom.create_element("view", ());
    h.doc.dom.append_child(root, appended);
    assert!(h.node_cache_empty(root), "the new parent spine is cleared");
    assert!(
        h.node_cache_empty(appended),
        "the newly attached subtree starts cold"
    );

    h.layout();
    assert_eq!(h.rect(first).0, 0.0);
    assert_eq!(h.rect(appended).0, 40.0);
}

#[test]
fn warm_cache_detach_invalidates_the_old_parent_spine() {
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 40px; align-items: flex-start; }
         view { width: 40px; height: 20px; }",
    );
    let root = h.doc.root;
    let first = h.doc.el(root, "view");
    let second = h.doc.el(root, "view");
    h.layout();
    assert_eq!(h.rect(second).0, 40.0);

    h.doc.dom.detach(first);
    assert!(h.node_cache_empty(root), "the old parent spine is cleared");
    assert!(h.node_cache_empty(first), "the detached subtree is cold");

    h.layout();
    assert_eq!(h.rect(second).0, 0.0);
}

#[test]
fn warm_cache_same_parent_reorder_invalidates_layout_order() {
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 40px; align-items: flex-start; }
         .a { width: 30px; height: 20px; }
         .b { width: 40px; height: 20px; }
         .c { width: 50px; height: 20px; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, ".a");
    let b = h.doc.el(root, ".b");
    let c = h.doc.el(root, ".c");
    h.layout();
    assert_eq!((h.rect(a).0, h.rect(b).0, h.rect(c).0), (0.0, 30.0, 70.0));

    h.doc.dom.insert_before(root, c, Some(a));
    assert!(h.node_cache_empty(root), "the reordered parent is cleared");
    assert!(h.node_cache_empty(c), "the moved subtree is cold");

    h.layout();
    assert_eq!((h.rect(c).0, h.rect(a).0, h.rect(b).0), (0.0, 50.0, 80.0));
}

#[test]
fn warm_cache_cross_parent_move_invalidates_both_parent_spines() {
    let mut h = Harness::new(
        "page { display: flex; width: 240px; height: 50px; align-items: flex-start; }
         .parent { display: flex; width: 100px; height: 40px; }
         .item { width: 30px; height: 20px; }",
    );
    let root = h.doc.root;
    let old_parent = h.doc.el(root, ".parent");
    let new_parent = h.doc.el(root, ".parent");
    let moved = h.doc.el(old_parent, ".item");
    let old_tail = h.doc.el(old_parent, ".item");
    let new_head = h.doc.el(new_parent, ".item");
    h.layout();
    assert_eq!(h.rect(old_tail).0, 30.0);
    assert_eq!(h.rect(new_head).0, 0.0);

    h.doc.dom.append_child(new_parent, moved);
    assert!(h.node_cache_empty(old_parent), "the old spine is cleared");
    assert!(h.node_cache_empty(new_parent), "the new spine is cleared");
    assert!(h.node_cache_empty(root), "the shared ancestor is cleared");
    assert!(h.node_cache_empty(moved), "the moved subtree is cold");

    h.layout();
    assert_eq!(h.rect(old_tail).0, 0.0);
    assert_eq!(h.rect(new_head).0, 0.0);
    assert_eq!(h.rect(moved).0, 30.0);
}

#[test]
fn contained_interior_relayouts_automatically() {
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
    h.layout();

    assert_eq!(h.rect(inner).2, 50.0);
    assert_eq!(h.rect(outer).2, 80.0);
}

#[test]
fn a_damaged_boundary_still_clears_its_ancestors() {
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
    h.layout();

    assert_eq!(h.rect(inner).2, 50.0, "the boundary interior updates");
    assert_eq!(h.rect(outer).2, 80.0, "the contained outer size holds");
    assert_eq!(h.rect(sib).2, 70.0, "the clears-to-root sibling updates");
}

#[test]
fn nested_boundaries_relayout_deepest_first() {
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
    assert_eq!(h.rect(b2).2, 180.0);
    assert_eq!(h.rect(dinner).2, 180.0);
    assert_eq!(h.rect(dinner).3, 20.0);

    h.doc
        .set_inline(m, "padding-left: 30px; padding-right: 30px");
    h.doc.set_inline(dinner, "height: 40px");
    h.layout();

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
    h.layout();

    assert_eq!(
        h.rect(boundary).2,
        90.0,
        "the boundary's own size change applied"
    );
    assert_eq!(h.rect(inner).2, 50.0, "the interior change applied");
}

#[test]
fn two_damaged_nodes_under_one_boundary_keep_root_warm() {
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

#[test]
fn content_visibility_hidden_skips_descendant_layout_and_measurement() {
    let mut dom = w3c_dom::Document::new(common::device(800.0, 600.0));
    dom.add_stylesheet(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start;
                font-family: Ahem; font-size: 16px; }
         .container { display: flex; width: 60px; height: 80px; align-items: flex-start; }
         .fixed { position: fixed; left: 10px; top: 20px; width: 30px; height: 40px; }",
        w3c_dom::StylesheetOrigin::Author,
    );
    assert_eq!(dom.register_fonts(AHEM), 1);
    let root = dom.create_element("page", ());
    dom.append_document_element(root);
    let container = dom.create_element("view", ());
    dom.add_class(container, "container");
    dom.set_inline_style(container, "content-visibility: hidden");
    dom.append_child(root, container);
    let text = dom.create_text_node("hi", ());
    dom.append_child(container, text);
    let fixed = dom.create_element("view", ());
    dom.add_class(fixed, "fixed");
    dom.append_child(container, fixed);

    dom.layout();

    let rect = |dom: &w3c_dom::Document<()>, id: NodeId| {
        let l = dom.rounded_layout(id).expect("live");
        (l.location.x, l.location.y, l.size.width, l.size.height)
    };
    assert_eq!(rect(&dom, container), (0.0, 0.0, 60.0, 80.0));
    assert_eq!(rect(&dom, text), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(rect(&dom, fixed), (0.0, 0.0, 0.0, 0.0));
    assert!(
        dom.layout_cache_is_empty(text)
            .expect("text node remains live"),
        "a skipped text leaf never populates its measurement cache",
    );
    dom.set_inline_style(container, "");
    dom.layout();
    assert_eq!(
        rect(&dom, text),
        (0.0, 0.0, 32.0, 16.0),
        "revealing the container lays its text back out",
    );
    assert!(
        !dom.layout_cache_is_empty(text)
            .expect("text node remains live"),
        "the revealed text leaf now has a Parley measurement",
    );
}

#[test]
fn content_visibility_auto_establishes_the_fixed_containing_block() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .host { display: flex; width: 300px; height: 200px; margin-left: 100px;
                 margin-top: 50px; }
         .cv { content-visibility: auto; }
         .fixed { position: fixed; left: 10px; top: 20px; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let plain = h.doc.el(root, ".host");
    let plain_fixed = h.doc.el(plain, ".fixed");
    let cv = h.doc.el(root, ".host.cv");
    let cv_fixed = h.doc.el(cv, ".fixed");
    h.layout();

    assert_eq!(h.rect(plain_fixed), (-90.0, -30.0, 30.0, 40.0));
    assert_eq!(h.rect(cv_fixed), (10.0, 20.0, 30.0, 40.0));
}

#[test]
fn contained_boundary_relayout_refreshes_scrollable_content_size() {
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
            .rounded_layout(id)
            .expect("live")
            .content_size
            .height
    };
    assert_eq!(h.rect(scroll), (0.0, 0.0, 80.0, 80.0));
    assert_eq!(content_height(&h, scroll), 80.0);

    h.doc.set_inline(child, "height: 120px");
    h.layout();

    assert_eq!(h.rect(scroll), (0.0, 0.0, 80.0, 80.0));
    assert_eq!(content_height(&h, scroll), 120.0);
    assert_eq!(
        h.doc.dom.unrounded_layout(child).expect("live").size.height,
        120.0,
        "the boundary interior actually re-laid-out",
    );
}

#[test]
fn layout_contained_visible_boundary_excludes_descendant_scrollable_overflow() {
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

    assert_eq!(
        h.doc.dom.unrounded_layout(child).expect("live").size.height,
        120.0,
        "the descendant is laid out (only its overflow is ink-only)",
    );
    assert_eq!(
        h.doc
            .dom
            .rounded_layout(scroll)
            .expect("live")
            .content_size
            .height,
        80.0,
        "layout containment + overflow:visible excludes descendant overflow",
    );
}

#[test]
fn boundary_scrollable_overflow_is_consistent_across_incremental_and_cold_layout() {
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
            .rounded_layout(id)
            .expect("live")
            .content_size
            .height
    };
    assert_eq!(content_h(&h, boundary), 60.0);
    assert_eq!(content_h(&h, root), 80.0);

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

    assert!(
        !h.node_cache_empty(root),
        "the root cache is warm after the first pass",
    );
    assert_eq!(h.rect(hidden), (0.0, 0.0, 40.0, 30.0));

    h.doc.dom.invalidate_layout(child);
    assert!(
        !h.node_cache_empty(root),
        "the root cache stays warm past a skipped container",
    );

    h.layout();
    assert_eq!(h.rect(hidden), (0.0, 0.0, 40.0, 30.0));

    h.doc.set_inline(hidden, "content-visibility: visible");
    h.layout();
    assert_eq!(h.rect(child), (0.0, 0.0, 20.0, 20.0));
}

#[test]
fn idle_frames_are_skipped_and_stay_idempotent() {
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 100px; }
         .a { flex-grow: 1; height: 20px; }
         .fixed { position: fixed; left: 5px; top: 7px; width: 10px; height: 12px; }",
    );
    let root = h.doc.root;
    let a = h.doc.el(root, ".a");
    let fixed = h.doc.el(root, ".fixed");
    let ids = [root, a, fixed];
    h.layout();
    let snapshot = h.layouts_of(&ids);

    h.layout();
    h.layout();
    assert_eq!(h.layouts_of(&ids), snapshot, "idle passes are idempotent");

    h.doc.set_inline(a, "flex-grow: 0; width: 40px");
    h.layout();
    assert_eq!(
        h.rect(a).2,
        40.0,
        "a mutation after idle frames still relayouts"
    );
}

#[test]
fn incremental_boundary_relayout_matches_a_full_relayout() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .spacer { width: 137px; height: 40px; }
         .boundary { display: flex; contain: strict; width: 200px; height: 100px;
                     align-items: flex-start; }
         .a { width: 30px; height: 20px; }
         .b { width: 40px; height: 25px; }",
    );
    let root = h.doc.root;
    let spacer = h.doc.el(root, ".spacer");
    let boundary = h.doc.el(root, ".boundary");
    let a = h.doc.el(boundary, ".a");
    let b = h.doc.el(boundary, ".b");
    let ids = [root, spacer, boundary, a, b];
    h.layout();

    h.doc.set_inline(a, "width: 55px");
    h.layout();
    let incremental = h.layouts_of(&ids);
    assert_eq!(h.rect(a).2, 55.0, "the interior actually changed");
    assert_eq!(h.rect(b).0, 55.0, "the sibling shifted with it");

    h.doc.dom.invalidate_layout_all();
    h.layout();
    assert_eq!(incremental, h.layouts_of(&ids), "incremental == full");
}

#[test]
fn incremental_relayout_matches_full_under_fractional_device_pixels() {
    let mut h = Harness::with_device(
        "page { display: flex; width: 400px; height: 300px; }
         .spacer { width: 37.5px; height: 20.5px; }
         .boundary { display: flex; contain: strict; width: 121.5px; height: 80px;
                     align-items: flex-start; }
         .a { width: 20.5px; height: 15.5px; }
         .b { width: 30.5px; height: 18.5px; }",
        device_with(400.0, 300.0, 2.0, PrefersColorScheme::Light),
    );
    let root = h.doc.root;
    let spacer = h.doc.el(root, ".spacer");
    let boundary = h.doc.el(root, ".boundary");
    let a = h.doc.el(boundary, ".a");
    let b = h.doc.el(boundary, ".b");
    let ids = [root, spacer, boundary, a, b];
    h.layout();

    h.doc.set_inline(a, "width: 44.5px");
    h.layout();
    let incremental = h.layouts_of(&ids);

    h.doc.dom.invalidate_layout_all();
    h.layout();
    assert_eq!(
        incremental,
        h.layouts_of(&ids),
        "fractional incremental rounding must equal a full re-round"
    );
}

#[test]
fn nested_parked_boundaries_incremental_matches_full() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .outer { display: flex; contain: strict; width: 300px; height: 200px;
                  align-items: flex-start; }
         .inner { display: flex; contain: strict; width: 120px; height: 80px;
                  align-items: flex-start; }
         .x { width: 30px; height: 20px; }
         .sib { width: 40px; height: 25px; }",
    );
    let root = h.doc.root;
    let outer = h.doc.el(root, ".outer");
    let inner = h.doc.el(outer, ".inner");
    let x = h.doc.el(inner, ".x");
    let sib = h.doc.el(outer, ".sib");
    let ids = [root, outer, inner, x, sib];
    h.layout();

    h.doc.set_inline(x, "width: 55px");
    h.doc.set_inline(sib, "width: 60px");
    h.layout();
    let incremental = h.layouts_of(&ids);
    assert_eq!(h.rect(x).2, 55.0, "the inner interior changed");
    assert_eq!(h.rect(sib).2, 60.0, "the outer sibling changed");

    h.doc.dom.invalidate_layout_all();
    h.layout();
    assert_eq!(
        incremental,
        h.layouts_of(&ids),
        "nested incremental == full"
    );
}

#[test]
fn incremental_relayout_reanchors_a_hoisted_node_inside_a_boundary() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .boundary { display: flex; contain: strict; width: 400px; height: 200px;
                     align-items: flex-start; }
         .filler { width: 20px; height: 30px; }
         .cb { display: flex; width: 120px; height: 100px; transform: translateX(0px);
               align-items: flex-start; }
         .mid { display: flex; width: 60px; height: 60px; }
         .fixed { position: fixed; left: 8px; top: 6px; width: 12px; height: 10px; }",
    );
    let root = h.doc.root;
    let boundary = h.doc.el(root, ".boundary");
    let filler = h.doc.el(boundary, ".filler");
    let cb = h.doc.el(boundary, ".cb");
    let mid = h.doc.el(cb, ".mid");
    let fixed = h.doc.el(mid, ".fixed");
    let ids = [root, boundary, filler, cb, mid, fixed];
    h.layout();

    h.doc.set_inline(filler, "width: 50px");
    h.layout();
    let incremental = h.layouts_of(&ids);

    h.doc.dom.invalidate_layout_all();
    h.layout();
    assert_eq!(
        incremental,
        h.layouts_of(&ids),
        "hoisted re-anchor incremental == full"
    );
}
