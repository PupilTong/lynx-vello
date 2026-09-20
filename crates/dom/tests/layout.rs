//! Behavior tests for the `dom::layout` integration: the hughie
//! host protocol implemented over `Document<T>` + stylo computed styles.

#![allow(clippy::float_cmp)]

mod common;

use common::{Doc, device_with};
use dom::layout::Layout;
use dom::{FontBlob, NodeId};
use stylo::queries::values::PrefersColorScheme;

const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");

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

/// The measured ink of the paragraph `holder` establishes.
///
/// What the text tests were really asserting: a text node has no box of its
/// own any more, and the element's box may be stretched by its parent.
fn text_ink(dom: &dom::Document<()>, holder: dom::NodeId) -> (f32, f32) {
    let size = dom
        .text_block_size(holder)
        .expect("holder establishes a block");
    (size.width, size.height)
}

fn dom_rect(dom: &dom::Document<()>, id: NodeId) -> (f32, f32, f32, f32) {
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

    fn with_device(css: &str, device: dom::Device) -> Self {
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

    fn force_full_layout_through_viewport_change(&mut self) {
        let viewport = self.doc.dom.viewport_size();
        self.doc
            .dom
            .set_viewport(viewport.width + 1.0, viewport.height);
        self.layout();
        self.doc.dom.set_viewport(viewport.width, viewport.height);
        self.layout();
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
    h.layout();

    assert_eq!(h.rect(a), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(h.rect(grandchild), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(h.rect(b).2, 100.0);

    h.doc.set_inline(a, "");
    h.layout();
    assert_eq!(h.rect(a).2, 50.0);
    assert_eq!(h.rect(b).2, 50.0);
}

#[test]
fn display_contents_lifts_children_into_the_containers_formatting_context() {
    let mut h = Harness::new(
        "page { display: flex; width: 300px; height: 40px; }
         view { flex-grow: 1; }
         .wrapper { display: contents; }",
    );
    let root = h.doc.root;
    let first = h.doc.el(root, "view");
    let wrapper = h.doc.el(root, ".wrapper");
    let lifted = h.doc.el(wrapper, "view");
    let nested_wrapper = h.doc.el(wrapper, ".wrapper");
    let nested = h.doc.el(nested_wrapper, "view");
    h.layout();

    assert_eq!(h.rect(first), (0.0, 0.0, 100.0, 40.0));
    assert_eq!(h.rect(lifted), (100.0, 0.0, 100.0, 40.0));
    assert_eq!(h.rect(nested), (200.0, 0.0, 100.0, 40.0));

    assert_eq!(h.rect(wrapper), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(h.rect(nested_wrapper), (0.0, 0.0, 0.0, 0.0));
}

#[test]
fn display_contents_works_across_every_container_algorithm() {
    let mut h = Harness::new(
        "page { display: flex; width: 300px; height: 300px; }
         .wrapper { display: contents; }
         .grid { display: grid; grid-template-columns: 40px 60px; width: 100px; height: 20px; }
         .linear { display: linear; linear-direction: row; linear-weight-sum: 2;
                   width: 100px; height: 20px; }
         .weighted { linear-weight: 1; }
         .relative { display: relative; width: 100px; height: 20px; }
         .anchor { relative-id: 1; width: 30px; height: 20px; }
         .after { relative-right-of: 1; width: 20px; height: 20px; }",
    );
    let root = h.doc.root;

    let grid = h.doc.el(root, ".grid");
    let grid_wrapper = h.doc.el(grid, ".wrapper");
    let first_cell = h.doc.el(grid_wrapper, "view");
    let second_cell = h.doc.el(grid_wrapper, "view");

    let linear = h.doc.el(root, ".linear");
    let linear_wrapper = h.doc.el(linear, ".wrapper");
    let first_weighted = h.doc.el(linear_wrapper, ".weighted");
    let second_weighted = h.doc.el(linear_wrapper, ".weighted");

    let relative = h.doc.el(root, ".relative");
    let relative_wrapper = h.doc.el(relative, ".wrapper");
    let anchor = h.doc.el(relative_wrapper, ".anchor");
    let after = h.doc.el(relative_wrapper, ".after");

    h.layout();

    assert_eq!(h.rect(first_cell).0, 0.0);
    assert_eq!(h.rect(first_cell).2, 40.0);
    assert_eq!(h.rect(second_cell).0, 40.0);
    assert_eq!(h.rect(second_cell).2, 60.0);

    assert_eq!(h.rect(first_weighted).0, 0.0);
    assert_eq!(h.rect(first_weighted).2, 50.0);
    assert_eq!(h.rect(second_weighted).0, 50.0);

    assert_eq!(h.rect(anchor).0, 0.0);
    assert_eq!(h.rect(after).0, 30.0);
}

#[test]
fn display_contents_inherits_to_its_children_without_boxing_them() {
    let mut dom = dom::Document::new(common::device(800.0, 600.0), "page", ());
    dom.add_stylesheet(
        "page { display: -lynx-text; width: 200px; height: 100px;
                font-family: Ahem; font-size: 16px; }
         .wrapper { display: contents; font-size: 8px; }",
        dom::StylesheetOrigin::Author,
    );
    assert_eq!(dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = dom.document_element().id();
    let wrapper = dom.create_element("view", ());
    dom.add_class(wrapper, "wrapper");
    dom.append_child(root, wrapper);
    let text = dom.create_text_node("hello", ());
    dom.append_child(wrapper, text);
    dom.layout();

    // The wrapper generates no box, but its style still reaches the run
    // inside it: the paragraph measures at 8px, not the page's 16px.
    let _ = text;
    assert_eq!(text_ink(&dom, root), (40.0, 8.0));
    assert_eq!(dom_rect(&dom, wrapper), (0.0, 0.0, 0.0, 0.0));
}

#[test]
fn display_contents_cannot_contain_position_or_paint_its_descendants() {
    let mut h = Harness::new(
        "page { display: flex; width: 300px; height: 100px; }
         .wrapper { display: contents; position: fixed; left: 90px; top: 80px;
                    transform: translateX(5px); contain: strict;
                    content-visibility: hidden; }
         .abs { position: absolute; left: 10px; top: 20px; width: 30px; height: 40px; }
         .plain { width: 30px; }",
    );
    let root = h.doc.root;
    let plain = h.doc.el(root, ".plain");
    let wrapper = h.doc.el(root, ".wrapper");
    let absolute = h.doc.el(wrapper, ".abs");
    h.layout();

    assert_eq!(h.rect(wrapper), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(h.rect(absolute), (10.0, 20.0, 30.0, 40.0));
    assert_eq!(h.layout_of(plain).order, 0);
    assert_eq!(h.layout_of(absolute).order, 1);
}

#[test]
fn mutations_below_display_contents_relayout_the_box_parent() {
    for display in ["flex", "linear"] {
        let mut h = Harness::new(&format!(
            "page {{ display: {display}; linear-direction: row; width: 300px; height: 40px; }}
         .wrapper {{ display: contents; }}
         view {{ width: 20px; height: 10px; flex-shrink: 0; }}",
        ));
        let root = h.doc.root;
        let wrapper = h.doc.el(root, ".wrapper");
        let nested = h.doc.el(wrapper, ".wrapper");
        let child = h.doc.el(nested, "view");
        let sibling = h.doc.el(root, "view");
        h.layout();
        assert_eq!(h.rect(sibling).0, 20.0);

        h.doc.set_inline(child, "width: 80px");
        h.layout();
        assert_eq!(h.rect(child).2, 80.0);
        assert_eq!(h.rect(sibling).0, 80.0);

        h.doc.dom.remove_element(child);
        let replacement = h.doc.el(nested, "view");
        h.layout();
        assert_eq!(h.rect(replacement).2, 20.0);
        assert_eq!(h.rect(sibling).0, 20.0);

        h.doc.dom.remove_element(replacement);
        h.layout();
        assert_eq!(h.rect(sibling).0, 0.0);

        h.doc.dom.remove_element(wrapper);
        let new_wrapper = h.doc.el(root, ".wrapper");
        let child = h.doc.el(new_wrapper, "view");
        h.layout();
        assert_eq!(h.rect(child).2, 20.0);
    }
}

#[test]
fn display_contents_flip_relayouts_the_container_and_clears_the_stale_box() {
    let mut h = Harness::new(
        "page { display: flex; width: 300px; height: 40px; }
         view { flex-grow: 1; }
         .wrapper { display: flex; }",
    );
    let root = h.doc.root;
    let wrapper = h.doc.el(root, ".wrapper");
    let lifted = h.doc.el(wrapper, "view");
    let sibling = h.doc.el(root, "view");
    h.layout();
    assert_eq!(h.rect(wrapper).2, 150.0);
    assert_eq!(h.rect(lifted).2, 150.0);

    h.doc.set_inline(wrapper, "display: contents");
    h.layout();

    assert_eq!(h.rect(wrapper), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(h.rect(lifted), (0.0, 0.0, 150.0, 40.0));
    assert_eq!(h.rect(sibling), (150.0, 0.0, 150.0, 40.0));

    h.doc.set_inline(wrapper, "");
    h.layout();
    assert_eq!(h.rect(wrapper).2, 150.0);
    assert_eq!(h.rect(lifted).2, 150.0);
}

#[test]
#[should_panic(expected = "Bobcat does not support Stylo computed display")]
fn display_contents_root_fixup_panics_as_an_unsupported_display() {
    let mut h = Harness::new(
        "page { display: contents; width: 120px; height: 30px; }
         view { width: 20px; height: 10px; }",
    );
    let root = h.doc.root;
    let _child = h.doc.el(root, "view");
    h.layout();
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
fn fixed_descendants_of_default_flex_are_laid_out() {
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
    assert_eq!(h.rect(fixed), (10.0, 20.0, 30.0, 40.0));
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
fn backdrop_filter_establishes_the_fixed_containing_block() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .host { display: flex; width: 300px; height: 200px; margin-left: 100px;
                 backdrop-filter: blur(4px); }
         .fixed { position: fixed; left: 10px; top: 20px; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let host = h.doc.el(root, ".host");
    let fixed = h.doc.el(host, ".fixed");
    h.layout();

    // Relative to `.host`, which itself starts at x = 100. An uncontained
    // fixed box would resolve against the viewport and land at x = -90 here.
    assert_eq!(h.rect(host).0, 100.0);
    assert_eq!(h.rect(fixed), (10.0, 20.0, 30.0, 40.0));
}

#[test]
fn backdrop_filter_establishes_the_absolute_containing_block() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; }
         .host { display: flex; width: 300px; height: 200px; margin-left: 100px;
                 backdrop-filter: blur(4px); }
         .abs { position: absolute; left: 10px; top: 20px; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let host = h.doc.el(root, ".host");
    let abs = h.doc.el(host, ".abs");
    h.layout();

    assert_eq!(h.rect(abs), (10.0, 20.0, 30.0, 40.0));
}

#[test]
fn root_backdrop_filter_is_exempt_from_fixed_containing_block_creation() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 600px; border: 10px solid black;
                backdrop-filter: blur(4px); }
         .host { display: flex; width: 300px; height: 200px; margin-left: 100px;
                 backdrop-filter: blur(4px); }
         .fixed { position: fixed; left: 0; top: 0; width: 30px; height: 40px; }",
    );
    let root = h.doc.root;
    let root_fixed = h.doc.el(root, ".fixed");
    let host = h.doc.el(root, ".host");
    let captured_fixed = h.doc.el(host, ".fixed");
    h.layout();

    // filter-effects-2 §2.1 exempts a document root element: the root's fixed
    // child resolves against the viewport, so the root's own 10px border does
    // not shift it. A non-root `backdrop-filter` element does capture its own.
    assert_eq!(h.rect(root_fixed), (0.0, 0.0, 30.0, 40.0));
    assert_eq!(h.rect(captured_fixed), (0.0, 0.0, 30.0, 40.0));
    assert_eq!(h.rect(host).0, 110.0);
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
    h.layout();
    assert_eq!(h.rect(fixed), (50.0, 20.0, 30.0, 40.0));
}

#[test]
fn omitted_display_uses_flex_and_lays_out_children() {
    let mut h = Harness::new(
        "page { display: flex; width: 100px; height: 100px; align-items: flex-start; }
         .default { width: 40px; height: 30px; align-items: flex-start; }
         .child { flex: none; width: 20px; height: 10px; }",
    );
    let root = h.doc.root;
    let default = h.doc.el(root, ".default");
    let child = h.doc.el(default, ".child");
    h.layout();

    assert_eq!(h.rect(default), (0.0, 0.0, 40.0, 30.0));
    assert_eq!(h.rect(child), (0.0, 0.0, 20.0, 10.0));
}

#[test]
fn text_nodes_use_parley_with_their_parents_inherited_style() {
    let mut dom = dom::Document::new(common::device(800.0, 600.0), "page", ());
    dom.add_stylesheet(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start;
                font-family: Ahem; font-size: 16px; }
         .sibling { width: 50px; height: 10px; }
         .label { display: -lynx-text; }",
        dom::StylesheetOrigin::Author,
    );
    assert_eq!(dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = dom.document_element().id();
    let sibling = dom.create_element("view", ());
    dom.add_class(sibling, "sibling");
    dom.append_child(root, sibling);
    let label = dom.create_element("view", ());
    dom.add_class(label, "label");
    dom.append_child(root, label);
    let text = dom.create_text_node("hello", ());
    dom.append_child(label, text);
    dom.layout();

    assert_eq!(dom_rect(&dom, sibling), (0.0, 0.0, 50.0, 10.0));
    assert_eq!(dom_rect(&dom, label), (50.0, 0.0, 80.0, 16.0));
    assert_eq!(text_ink(&dom, label), (80.0, 16.0));

    dom.set_text_node_data(text, "hi");
    dom.layout();
    assert_eq!(text_ink(&dom, label), (32.0, 16.0));
}

#[test]
fn embedder_default_family_shapes_text_without_an_authored_font_family() {
    let mut dom = dom::Document::new(common::device(800.0, 600.0), "page", ());
    dom.add_stylesheet(
        "page { display: -lynx-text; width: 200px; height: 100px;
                font-size: 16px; }",
        dom::StylesheetOrigin::Author,
    );
    assert_eq!(dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = dom.document_element().id();
    let text = dom.create_text_node("hello", ());
    dom.append_child(root, text);

    dom.layout();
    assert!(dom.set_default_font_family("Ahem"));
    dom.layout();

    let _ = text;
    assert_eq!(text_ink(&dom, root), (80.0, 16.0));
}

#[test]
fn inherited_text_style_change_remeasures_text_child() {
    let mut dom = dom::Document::new(common::device(800.0, 600.0), "page", ());
    dom.add_stylesheet(
        "page { display: -lynx-text; width: 200px; height: 100px;
                font-family: Ahem; font-size: 16px; }",
        dom::StylesheetOrigin::Author,
    );
    assert_eq!(dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = dom.document_element().id();
    let text = dom.create_text_node("hello", ());
    dom.append_child(root, text);

    dom.layout();
    let _ = text;
    assert_eq!(text_ink(&dom, root), (80.0, 16.0));

    dom.set_inline_style(root, "font-size: 32px");
    dom.layout();

    assert_eq!(text_ink(&dom, root), (160.0, 32.0));
}

#[test]
fn inherited_text_style_change_remeasures_nested_text_child() {
    let mut dom = dom::Document::new(common::device(800.0, 600.0), "page", ());
    dom.add_stylesheet(
        "page { display: flex; width: 200px; height: 100px;
                font-family: Ahem; font-size: 16px; }
         view { display: -lynx-text; width: 200px; height: 100px; }",
        dom::StylesheetOrigin::Author,
    );
    assert_eq!(dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = dom.document_element().id();
    let parent = dom.create_element("view", ());
    dom.append_child(root, parent);
    let text = dom.create_text_node("hello", ());
    dom.append_child(parent, text);

    dom.layout();
    let _ = text;
    assert_eq!(text_ink(&dom, parent), (80.0, 16.0));

    dom.set_inline_style(root, "font-size: 32px");
    dom.layout();

    assert_eq!(text_ink(&dom, parent), (160.0, 32.0));
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

    h.doc.dom.drop_subtree(old);
    assert!(h.doc.dom.get(old).is_none());

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

/// A parked relayout root names a containment boundary that may be gone by
/// the time the next pass runs. Its id can never come back attached to a
/// different node, so the pruning is no longer an aliasing defense — but a
/// replacement built on the freed node's storage must still not inherit the
/// parked pass, and must obey the skipped-contents subtree it is placed in.
#[test]
fn a_removed_boundary_does_not_replay_onto_the_node_that_replaces_it() {
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

    h.doc.dom.set_natural_size(
        old_child,
        dom::layout::NaturalSize::from_size(dom::layout::Size::new(30.0, 20.0)),
    );

    assert_eq!(h.doc.dom.drop_subtree(old_boundary).len(), 2);

    // Both freed slots go back on the arena free list, so these two land on
    // the storage the removed boundary and its child were using.
    let replacement_boundary = h.doc.dom.create_element("view", ());
    let replacement_child = h.doc.dom.create_element("view", ());
    assert!(
        [replacement_boundary, replacement_child]
            .iter()
            .all(|&id| id != old_boundary && id != old_child),
        "the removed ids are retired, so the replacements get fresh ones",
    );
    h.doc
        .dom
        .append_child(replacement_boundary, replacement_child);
    h.doc.dom.append_child(hidden, replacement_boundary);
    h.doc.set_inline(
        replacement_boundary,
        "display: flex; contain: strict; width: 80px; height: 40px",
    );
    h.doc
        .set_inline(replacement_child, "width: 10px; height: 10px");

    h.layout();
    assert_eq!(
        h.rect(replacement_child),
        (0.0, 0.0, 0.0, 0.0),
        "a stale parked root must not lay out a replacement node under skipped contents",
    );
    assert!(
        h.doc.dom.get(old_boundary).is_none() && h.doc.dom.get(old_child).is_none(),
        "and the removed ids still name nothing",
    );
}

#[test]
fn color_only_change_preserves_text_geometry() {
    let mut dom = dom::Document::new(common::device(800.0, 600.0), "page", ());
    dom.add_stylesheet(
        "page { display: -lynx-text; width: 200px; height: 100px;
                font-family: Ahem; font-size: 16px; }",
        dom::StylesheetOrigin::Author,
    );
    assert_eq!(dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = dom.document_element().id();
    let text = dom.create_text_node("hello", ());
    dom.append_child(root, text);

    dom.layout();
    let _ = text;
    assert_eq!(text_ink(&dom, root), (80.0, 16.0));

    dom.set_inline_style(root, "color: rgb(0, 0, 255)");
    dom.layout();
    assert_eq!(text_ink(&dom, root), (80.0, 16.0));
    assert_eq!(
        dom.get(root)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        common::rgb(0, 0, 255),
    );
}

#[test]
fn contain_strict_boundary_relayouts_interior_without_changing_outer_size() {
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
    h.layout();
    assert_eq!(h.rect(inner).2, 50.0, "the boundary interior re-lays-out");
    assert_eq!(
        h.rect(outer).2,
        80.0,
        "the contained box keeps its outer size"
    );
}

#[test]
fn appending_to_a_laid_out_parent_updates_sibling_positions() {
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 40px; align-items: flex-start; }
         view { width: 40px; height: 20px; }",
    );
    let root = h.doc.root;
    let first = h.doc.el(root, "view");
    h.layout();

    let appended = h.doc.dom.create_element("view", ());
    h.doc.dom.append_child(root, appended);
    h.layout();
    assert_eq!(h.rect(first).0, 0.0);
    assert_eq!(h.rect(appended).0, 40.0);
}

#[test]
fn detaching_from_a_laid_out_parent_updates_remaining_positions() {
    let mut h = Harness::new(
        "page { display: flex; width: 200px; height: 40px; align-items: flex-start; }
         view { width: 40px; height: 20px; }",
    );
    let root = h.doc.root;
    let first = h.doc.el(root, "view");
    let second = h.doc.el(root, "view");
    h.layout();
    assert_eq!(h.rect(second).0, 40.0);

    h.doc.dom.remove_element(first);
    h.layout();
    assert_eq!(h.rect(second).0, 0.0);
}

#[test]
fn same_parent_reorder_updates_layout_order() {
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
    h.layout();
    assert_eq!((h.rect(c).0, h.rect(a).0, h.rect(b).0), (0.0, 50.0, 80.0));
}

#[test]
fn cross_parent_move_updates_both_formatting_contexts() {
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
fn two_damaged_nodes_under_one_boundary_both_relayout() {
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
    h.layout();
    assert_eq!(h.rect(a).2, 30.0, "the first interior change applied");
    assert_eq!(h.rect(b).3, 30.0, "the second interior change applied");
    assert_eq!(h.rect(outer).2, 80.0, "the contained outer size holds");
}

#[test]
fn content_visibility_hidden_skips_descendant_layout_and_measurement() {
    let mut dom = dom::Document::new(common::device(800.0, 600.0), "page", ());
    dom.add_stylesheet(
        "page { display: flex; width: 200px; height: 100px; align-items: flex-start;
                font-family: Ahem; font-size: 16px; }
         .container { display: -lynx-text; width: 60px; height: 80px; }
         .fixed { position: fixed; left: 10px; top: 20px; width: 30px; height: 40px; }",
        dom::StylesheetOrigin::Author,
    );
    assert_eq!(dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = dom.document_element().id();
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

    let rect = |dom: &dom::Document<()>, id: NodeId| {
        let l = dom.rounded_layout(id).expect("live");
        (l.location.x, l.location.y, l.size.width, l.size.height)
    };
    assert_eq!(rect(&dom, container), (0.0, 0.0, 60.0, 80.0));
    assert_eq!(rect(&dom, fixed), (0.0, 0.0, 0.0, 0.0));
    assert!(
        dom.text_block_size(container).is_none(),
        "a skipped container lays out no paragraph at all"
    );
    dom.set_inline_style(container, "");
    dom.layout();
    let _ = text;
    assert_eq!(
        dom.text_block_size(container).map(|s| (s.width, s.height)),
        Some((32.0, 16.0)),
        "revealing the container lays its text back out",
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
    // A render, not a bare layout: `content-visibility: auto` relevance is
    // determined by the rendering update, and an `auto` box no rendering
    // update has reached skips its contents — so the fixed descendant this
    // test is about only exists once the box is known to be on screen.
    assert!(h.doc.dom.render(), "the first render commits a frame");

    assert_eq!(h.rect(plain_fixed), (-90.0, -30.0, 30.0, 40.0));
    assert_eq!(h.rect(cv_fixed), (10.0, 20.0, 30.0, 40.0));

    // The same fixed containing block while the box is skipping: its own
    // geometry never depended on its contents being laid out.
    h.doc.dom.set_viewport(800.0, 40.0);
    assert!(h.doc.dom.render(), "the resize commits another frame");
    assert_eq!(
        h.rect(cv_fixed),
        (0.0, 0.0, 0.0, 0.0),
        "a skipped box lays out no descendant, hoisted or not",
    );
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
        h.doc.dom.rounded_layout(child).expect("live").size.height,
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
        h.doc.dom.rounded_layout(child).expect("live").size.height,
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

    h.force_full_layout_through_viewport_change();
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
fn mutation_inside_a_skipped_container_is_deferred_until_reveal() {
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

    assert_eq!(h.rect(hidden), (0.0, 0.0, 40.0, 30.0));

    h.doc.set_inline(child, "width: 30px; height: 20px");
    h.layout();
    assert_eq!(h.rect(hidden), (0.0, 0.0, 40.0, 30.0));
    assert_eq!(h.rect(child), (0.0, 0.0, 0.0, 0.0));

    h.doc.set_inline(hidden, "content-visibility: visible");
    h.layout();
    assert_eq!(h.rect(child), (0.0, 0.0, 30.0, 20.0));
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

    h.force_full_layout_through_viewport_change();
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

    h.force_full_layout_through_viewport_change();
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

    h.force_full_layout_through_viewport_change();
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

    h.force_full_layout_through_viewport_change();
    assert_eq!(
        incremental,
        h.layouts_of(&ids),
        "hoisted re-anchor incremental == full"
    );
}

#[test]
fn contents_children_compete_in_the_container_order_sort() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 100px; }
         .wrap { display: contents; }
         .cell { display: flex; width: 100px; height: 100px; }
         .o0 { order: 0; } .o1 { order: 1; } .o2 { order: 2; } .o3 { order: 3; }",
    );
    let root = h.doc.root;
    let second = h.doc.el(root, "view.cell.o2");
    let wrap = h.doc.el(root, "view.wrap");
    let inner_first = h.doc.el(wrap, "view.cell.o1");
    let inner_last = h.doc.el(wrap, "view.cell.o3");
    let leading = h.doc.el(root, "view.cell.o0");
    h.layout();
    assert_eq!(h.rect(leading).0, 0.0);
    assert_eq!(h.rect(inner_first).0, 100.0);
    assert_eq!(h.rect(second).0, 200.0);
    assert_eq!(h.rect(inner_last).0, 300.0);
}

#[test]
fn text_children_of_contents_measure_as_container_items() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 100px;
                font-family: Ahem; font-size: 20px; }
         .wrap { display: contents; }
         .label { display: -lynx-text; }
         .cell { display: flex; width: 100px; height: 100px; }",
    );
    h.doc.dom.register_fonts(FontBlob::from_static(AHEM));
    let root = h.doc.root;
    let wrap = h.doc.el(root, "view.wrap");
    // Text needs a block to live in, so what `display: contents` flattens into
    // the container's item list is that block — which is the subject here.
    let label = h.doc.el(wrap, "view.label");
    let text = h.doc.dom.create_text_node("hi", ());
    h.doc.dom.append_child(label, text);
    let after = h.doc.el(root, "view.cell");
    h.layout();
    let label_rect = dom_rect(&h.doc.dom, label);
    assert_eq!((label_rect.0, label_rect.2), (0.0, 40.0));
    assert_eq!(h.rect(after).0, 40.0);
}

#[test]
fn absolute_child_of_contents_anchors_to_the_box_ancestor() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .wrap { display: contents; position: relative; }
         .abs { display: flex; position: absolute; left: 10px; top: 20px;
                width: 50px; height: 50px; }",
    );
    let root = h.doc.root;
    let wrap = h.doc.el(root, "view.wrap");
    let abs = h.doc.el(wrap, "view.abs");
    h.layout();
    assert_eq!(h.rect(abs), (10.0, 20.0, 50.0, 50.0));
    assert_eq!(h.rect(wrap), (0.0, 0.0, 0.0, 0.0));
}

#[test]
fn a_parked_boundary_that_flips_to_contents_is_dropped_gracefully() {
    let mut h = Harness::new(
        "page { display: flex; width: 800px; height: 100px; }
         .boundary { display: flex; contain: strict; width: 100px; height: 100px; }
         .inner { display: flex; width: 50px; height: 50px; }
         .cell { display: flex; width: 100px; height: 100px; }",
    );
    let root = h.doc.root;
    let boundary = h.doc.el(root, "view.boundary");
    h.doc.set_inline(boundary, "contain: strict");
    let inner = h.doc.el(boundary, "view.inner");
    let text = h.doc.dom.create_text_node("a", ());
    h.doc.dom.append_child(inner, text);
    let after = h.doc.el(root, "view.cell");
    h.layout();
    assert_eq!(h.rect(after).0, 100.0);

    h.doc.dom.set_text_node_data(text, "bb");
    h.doc
        .set_inline(boundary, "contain: strict; display: contents");
    h.layout();
    assert_eq!(h.rect(inner), (0.0, 0.0, 50.0, 50.0));
    assert_eq!(h.rect(after).0, 50.0);
}

#[test]
fn hoisted_boxes_rank_in_the_dissolved_sibling_space() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 100px; }
         .wrap { display: contents; }
         .cell { display: flex; width: 100px; height: 100px; }
         .fixed { display: flex; position: fixed; left: 0; top: 0;
                  width: 50px; height: 50px; }",
    );
    let root = h.doc.root;
    let wrap = h.doc.el(root, "view.wrap");
    let sibling = h.doc.el(wrap, "view.cell");
    let fixed = h.doc.el(wrap, "view.fixed");
    h.layout();
    assert_eq!(h.layout_of(sibling).order, 0);
    assert_eq!(h.layout_of(fixed).order, 1);
}

#[test]
fn hoisted_rank_counts_negative_order_siblings_after_the_target() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 100px; }
         .wrap { display: contents; }
         .cell { display: flex; width: 100px; height: 100px; }
         .neg { order: -1; }
         .fixed { display: flex; position: fixed; left: 0; top: 0;
                  width: 50px; height: 50px; }",
    );
    let root = h.doc.root;
    let cell = h.doc.el(root, "view.cell");
    let wrap = h.doc.el(root, "view.wrap");
    let fixed = h.doc.el(wrap, "view.fixed");
    let neg = h.doc.el(wrap, "view.cell.neg");
    h.layout();
    assert_eq!(h.layout_of(neg).order, 0);
    assert_eq!(h.layout_of(cell).order, 1);
    assert_eq!(h.layout_of(fixed).order, 2);
}

#[test]
fn hoisted_rank_resolves_through_nested_contents_levels() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 800px; height: 100px; }
         .wrap { display: contents; }
         .cell { display: flex; width: 100px; height: 100px; }
         .fixed { display: flex; position: fixed; left: 0; top: 0;
                  width: 50px; height: 50px; }",
    );
    let root = h.doc.root;
    let sibling = h.doc.el(root, "view.cell");
    let outer = h.doc.el(root, "view.wrap");
    let inner = h.doc.el(outer, "view.wrap");
    let fixed = h.doc.el(inner, "view.fixed");
    h.layout();
    assert_eq!(h.layout_of(sibling).order, 0);
    assert_eq!(h.layout_of(fixed).order, 1);
}

/// `bounding_client_rect` as a tuple, for the same reason `dom_rect` exists.
fn client_rect(dom: &dom::Document<()>, id: NodeId) -> Option<(f32, f32, f32, f32)> {
    dom.bounding_client_rect(id).map(|rect| {
        (
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        )
    })
}

#[test]
fn bounding_client_rect_telescopes_nested_offset_boxes() {
    let mut h = Harness::new(
        "page { display: flex; width: 400px; height: 300px; }
         .parent { display: flex; position: relative; left: 10px; top: 10px;
                   width: 200px; height: 200px; }
         .child { display: flex; position: relative; left: 30px; top: 20px;
                  width: 50px; height: 40px; }",
    );
    let root = h.doc.root;
    let parent = h.doc.el(root, "view.parent");
    let child = h.doc.el(parent, "view.child");
    h.layout();

    assert_eq!(
        client_rect(&h.doc.dom, parent),
        Some((10.0, 10.0, 200.0, 200.0))
    );
    // Box-parent-relative locations sum into viewport coordinates.
    assert_eq!(h.rect(child), (30.0, 20.0, 50.0, 40.0));
    assert_eq!(
        client_rect(&h.doc.dom, child),
        Some((40.0, 30.0, 50.0, 40.0))
    );
}

/// The no-transform ruling: the rect is the untransformed border box, as
/// native's own engine-side conversion produces it.
#[test]
fn a_transform_never_moves_the_bounding_client_rect() {
    let mut h = Harness::new(
        "page { display: flex; width: 400px; height: 300px; }
         .parent { display: flex; width: 200px; height: 200px; }
         .child { display: flex; width: 50px; height: 40px; }",
    );
    let root = h.doc.root;
    let parent = h.doc.el(root, "view.parent");
    let child = h.doc.el(parent, "view.child");
    h.layout();
    let before = client_rect(&h.doc.dom, child);
    assert_eq!(before, Some((0.0, 0.0, 50.0, 40.0)));

    h.doc.set_inline(child, "transform: rotate(45deg)");
    h.layout();
    assert_eq!(client_rect(&h.doc.dom, child), before);

    // Nor does one on the ancestor, which also turns it into a containing
    // block for everything positioned under it.
    h.doc.set_inline(parent, "transform: rotate(45deg)");
    h.layout();
    assert_eq!(client_rect(&h.doc.dom, child), before);
}

#[test]
fn bounding_client_rect_subtracts_ancestor_scroll_offsets_but_not_its_own() {
    let mut h = Harness::new(
        "page { display: flex; width: 400px; height: 300px; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     margin-left: 20px; margin-top: 10px;
                     width: 100px; height: 100px; }
         .tall { display: flex; flex-shrink: 0; width: 80px; height: 400px; }",
    );
    let root = h.doc.root;
    let scroller = h.doc.el(root, "view.scroller");
    let child = h.doc.el(scroller, "view.tall");
    h.layout();
    assert_eq!(
        client_rect(&h.doc.dom, scroller),
        Some((20.0, 10.0, 100.0, 100.0))
    );
    assert_eq!(
        client_rect(&h.doc.dom, child),
        Some((20.0, 10.0, 80.0, 400.0))
    );

    assert_eq!(
        h.doc.dom.scroll_to(scroller, dom::Vector2D::new(0.0, 50.0)),
        dom::Vector2D::new(0.0, 50.0)
    );
    assert_eq!(
        client_rect(&h.doc.dom, child),
        Some((20.0, -40.0, 80.0, 400.0)),
        "the child moves by exactly the scroll offset",
    );
    assert_eq!(
        client_rect(&h.doc.dom, scroller),
        Some((20.0, 10.0, 100.0, 100.0)),
        "scrolling a container never moves the container",
    );

    // Scrolled entirely out of the scrollport, the box is still a box.
    h.doc
        .dom
        .scroll_to(scroller, dom::Vector2D::new(0.0, 300.0));
    assert_eq!(
        client_rect(&h.doc.dom, child),
        Some((20.0, -290.0, 80.0, 400.0))
    );
}

/// The containing-block escape, on the scroll chain: an out-of-flow box does
/// not move with scrollers between it and its containing block, and does move
/// with the containing block's own.
#[test]
fn an_out_of_flow_box_follows_only_its_containing_blocks_scroll() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 400px; height: 300px; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     width: 100px; height: 100px; }
         .anchored { position: relative; }
         .tall { display: flex; flex-shrink: 0; width: 80px; height: 400px; }
         .abs { display: flex; position: absolute; left: 5px; top: 5px;
                width: 20px; height: 20px; }",
    );
    let root = h.doc.root;
    let scroller = h.doc.el(root, "view.scroller");
    let escaping = h.doc.el(scroller, "view.abs");
    h.doc.el(scroller, "view.tall");
    let anchored_scroller = h.doc.el(root, "view.scroller.anchored");
    let anchored = h.doc.el(anchored_scroller, "view.abs");
    h.doc.el(anchored_scroller, "view.tall");
    h.layout();

    assert_eq!(
        client_rect(&h.doc.dom, escaping),
        Some((5.0, 5.0, 20.0, 20.0))
    );
    let anchored_before = client_rect(&h.doc.dom, anchored);
    assert_eq!(anchored_before, Some((105.0, 5.0, 20.0, 20.0)));

    h.doc.dom.scroll_to(scroller, dom::Vector2D::new(0.0, 50.0));
    h.doc
        .dom
        .scroll_to(anchored_scroller, dom::Vector2D::new(0.0, 50.0));

    assert_eq!(
        client_rect(&h.doc.dom, escaping),
        Some((5.0, 5.0, 20.0, 20.0)),
        "the page is its containing block, so the scroller between does not move it",
    );
    assert_eq!(
        client_rect(&h.doc.dom, anchored),
        Some((105.0, -45.0, 20.0, 20.0)),
        "its own containing block scrolled, so it scrolled",
    );
}

#[test]
fn a_fixed_box_inside_a_scroller_never_moves() {
    let mut h = Harness::new(
        "page { display: flex; position: relative; width: 400px; height: 300px; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     margin-left: 20px; width: 100px; height: 100px; }
         .tall { display: flex; flex-shrink: 0; width: 80px; height: 400px; }
         .fixed { display: flex; position: fixed; left: 7px; top: 9px;
                  width: 30px; height: 30px; }",
    );
    let root = h.doc.root;
    let scroller = h.doc.el(root, "view.scroller");
    let fixed = h.doc.el(scroller, "view.fixed");
    h.doc.el(scroller, "view.tall");
    h.layout();
    assert_eq!(client_rect(&h.doc.dom, fixed), Some((7.0, 9.0, 30.0, 30.0)));

    h.doc.dom.scroll_to(scroller, dom::Vector2D::new(0.0, 60.0));
    assert_eq!(
        client_rect(&h.doc.dom, fixed),
        Some((7.0, 9.0, 30.0, 30.0)),
        "a viewport-anchored box is on no scroller's chain",
    );
}

#[test]
fn a_box_less_or_detached_element_has_no_bounding_client_rect() {
    let mut h = Harness::new(
        "page { display: flex; width: 400px; height: 300px; }
         .gone { display: none; }
         .through { display: contents; }
         .cell { display: flex; width: 40px; height: 40px; }",
    );
    let root = h.doc.root;
    let gone = h.doc.el(root, "view.gone");
    let through = h.doc.el(root, "view.through");
    let inside_through = h.doc.el(through, "view.cell");
    let removed = h.doc.el(root, "view.cell");
    let removed_child = h.doc.el(removed, "view.cell");
    let never_attached = h.doc.dom.create_element("view", ());
    h.layout();

    assert_eq!(client_rect(&h.doc.dom, gone), None, "display: none");
    assert_eq!(client_rect(&h.doc.dom, through), None, "display: contents");
    assert_eq!(
        client_rect(&h.doc.dom, inside_through),
        Some((0.0, 0.0, 40.0, 40.0)),
        "a contents box contributes no offset, but its children still have one",
    );
    assert_eq!(client_rect(&h.doc.dom, never_attached), None);

    assert_eq!(
        client_rect(&h.doc.dom, removed_child),
        Some((40.0, 0.0, 40.0, 40.0))
    );

    h.doc.dom.remove_element(removed);
    assert_eq!(
        client_rect(&h.doc.dom, removed),
        None,
        "a detached subtree answers nothing, as a disconnected element does on the web",
    );
    assert_eq!(
        client_rect(&h.doc.dom, removed_child),
        None,
        "including a box inside it, whose walk runs off the top of that subtree",
    );
}

#[test]
fn a_hidden_but_laid_out_box_reports_its_real_rect() {
    let mut h = Harness::new(
        "page { display: flex; width: 400px; height: 300px; }
         .cell { display: flex; width: 40px; height: 40px; }
         .invisible { visibility: hidden; }",
    );
    let root = h.doc.root;
    h.doc.el(root, "view.cell");
    let hidden = h.doc.el(root, "view.cell.invisible");
    h.layout();

    assert_eq!(
        client_rect(&h.doc.dom, hidden),
        Some((40.0, 0.0, 40.0, 40.0)),
        "the rect comes from layout, not from the paint order",
    );
}

#[test]
fn bounding_client_rect_is_the_border_box_of_a_padded_text_element() {
    let mut h = Harness::new(
        "page { display: flex; align-items: flex-start; width: 400px; height: 300px;
                font-family: Ahem; font-size: 20px; }
         .label { display: -lynx-text; padding: 10px; margin-left: 30px; }",
    );
    assert_eq!(h.doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = h.doc.root;
    let label = h.doc.el(root, "text.label");
    let run = h.doc.dom.create_text_node("hi", ());
    h.doc.dom.append_child(label, run);
    h.layout();

    let (ink_width, ink_height) = text_ink(&h.doc.dom, label);
    assert_eq!((ink_width, ink_height), (40.0, 20.0));
    assert_eq!(
        client_rect(&h.doc.dom, label),
        Some((30.0, 0.0, 60.0, 40.0)),
        "the border box, not the content box the paragraph sits in",
    );
}

/// The no-flush contract: the answer is the last completed pass, whatever the
/// tree has done since.
#[test]
fn bounding_client_rect_never_runs_a_pending_layout() {
    let mut h = Harness::new(
        "page { display: flex; width: 400px; height: 300px; }
         .cell { display: flex; width: 40px; height: 40px; }",
    );
    let root = h.doc.root;
    let cell = h.doc.el(root, "view.cell");
    h.layout();
    assert_eq!(client_rect(&h.doc.dom, cell), Some((0.0, 0.0, 40.0, 40.0)));

    h.doc.set_inline(cell, "margin-left: 25px");
    assert_eq!(
        client_rect(&h.doc.dom, cell),
        Some((0.0, 0.0, 40.0, 40.0)),
        "a mutation the pipeline has not seen changes nothing",
    );

    h.layout();
    assert_eq!(client_rect(&h.doc.dom, cell), Some((25.0, 0.0, 40.0, 40.0)));
}

// --- CSS Grid Level 3 `display: grid-lanes` -------------------------------

/// The waterfall shape the Lynx `<list list-type="waterfall">` component will
/// be lowered onto: fixed `minmax(0, 1fr)` lanes, a gutter in both axes, and
/// `flow-tolerance: 0` so every item lands in the strictly shortest lane.
const LANES: &str = "page { display: flex; align-items: flex-start;
                            width: 400px; height: 400px; }
     .lanes { display: grid-lanes; width: 200px; gap: 10px; flow-tolerance: 0;
              grid-template-columns: repeat(2, minmax(0, 1fr)); }";

#[test]
fn grid_lanes_stacks_items_into_the_shortest_lane() {
    let mut h = Harness::new(LANES);
    let root = h.doc.root;
    let lanes = h.doc.el(root, "view.lanes");
    let items = [30.0, 50.0, 20.0, 60.0, 40.0, 10.0]
        .into_iter()
        .map(|height| {
            let item = h.doc.el(lanes, "view");
            h.doc.set_inline(item, &format!("height: {height}px"));
            item
        })
        .collect::<Vec<_>>();
    h.layout();

    for (index, expected) in [
        (0.0, 0.0, 95.0, 30.0),
        (105.0, 0.0, 95.0, 50.0),
        (0.0, 40.0, 95.0, 20.0),
        (105.0, 60.0, 95.0, 60.0),
        (0.0, 70.0, 95.0, 40.0),
        (0.0, 120.0, 95.0, 10.0),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(h.rect(items[index]), expected, "item {index}");
    }
    assert_eq!(h.rect(lanes), (0.0, 0.0, 200.0, 130.0));
}

/// The same two-lane template written the way the `<list>` UA rule will write
/// it, so `var()` substitution has to reach the track list before expansion.
#[test]
fn grid_lanes_reads_a_var_substituted_track_list() {
    let mut h = Harness::new(
        "page { display: flex; align-items: flex-start; width: 400px; height: 400px; }
         .lanes { display: grid-lanes; width: 200px; gap: 10px; flow-tolerance: 0;
                  --n: 2;
                  grid-template-columns: repeat(var(--n), minmax(0, 1fr)); }",
    );
    let root = h.doc.root;
    let lanes = h.doc.el(root, "view.lanes");
    let items = [30.0, 50.0, 20.0]
        .into_iter()
        .map(|height| {
            let item = h.doc.el(lanes, "view");
            h.doc.set_inline(item, &format!("height: {height}px"));
            item
        })
        .collect::<Vec<_>>();
    h.layout();

    assert_eq!(
        h.doc.value(lanes, "grid-template-columns"),
        "repeat(2, minmax(0px, 1fr))",
        "the substituted count reaches the computed track list",
    );
    assert_eq!(h.rect(items[0]), (0.0, 0.0, 95.0, 30.0));
    assert_eq!(h.rect(items[1]), (105.0, 0.0, 95.0, 50.0));
    assert_eq!(h.rect(items[2]), (0.0, 40.0, 95.0, 20.0));
}

/// A full-span item spans every lane, so it starts below the longest one and
/// leaves all of them level behind it.
#[test]
fn grid_lanes_full_span_item_levels_every_lane() {
    let mut h = Harness::new(LANES);
    let root = h.doc.root;
    let lanes = h.doc.el(root, "view.lanes");
    let first = h.doc.el(lanes, "view");
    h.doc.set_inline(first, "height: 30px");
    let second = h.doc.el(lanes, "view");
    h.doc.set_inline(second, "height: 50px");
    let full = h.doc.el(lanes, "view");
    h.doc.set_inline(full, "grid-column: 1 / -1; height: 20px");
    let fourth = h.doc.el(lanes, "view");
    h.doc.set_inline(fourth, "height: 15px");
    let fifth = h.doc.el(lanes, "view");
    h.doc.set_inline(fifth, "height: 25px");
    h.layout();

    assert_eq!(h.rect(first), (0.0, 0.0, 95.0, 30.0));
    assert_eq!(h.rect(second), (105.0, 0.0, 95.0, 50.0));
    assert_eq!(h.rect(full), (0.0, 60.0, 200.0, 20.0));
    assert_eq!(h.rect(fourth), (0.0, 90.0, 95.0, 15.0));
    assert_eq!(h.rect(fifth), (105.0, 90.0, 95.0, 25.0));
    assert_eq!(h.rect(lanes), (0.0, 0.0, 200.0, 115.0));
}

/// The waterfall as it will actually ship: inside a scroll container whose
/// scrollable extent is the stacking range the lanes pass produced.
#[test]
fn grid_lanes_scroll_extent_is_the_stacking_range_beyond_the_scrollport() {
    let mut h = Harness::new(
        "page { display: flex; align-items: flex-start; width: 400px; height: 400px; }
         .lanes { display: grid-lanes; width: 200px; height: 100px; overflow-y: scroll;
                  gap: 10px; flow-tolerance: 0;
                  grid-template-columns: repeat(2, minmax(0, 1fr)); }",
    );
    let root = h.doc.root;
    let lanes = h.doc.el(root, "view.lanes");
    for height in [30.0, 50.0, 20.0, 60.0, 40.0, 10.0] {
        let item = h.doc.el(lanes, "view");
        h.doc.set_inline(item, &format!("height: {height}px"));
    }
    h.layout();

    let scroll_box = h
        .doc
        .dom
        .scroll_box(lanes)
        .expect("the lanes container scrolls");
    assert_eq!(scroll_box.scrollport.height, 100.0);
    assert_eq!(scroll_box.scroll_size.height, 130.0, "the stacking range");
    assert_eq!(scroll_box.max_offset().y, 30.0);
}

/// `grid-template-rows` alone puts the tracks on the block axis, so the items
/// stack rightwards and the scrollable extent is horizontal.
#[test]
fn row_grid_lanes_stack_along_the_inline_axis_and_scroll_there() {
    let mut h = Harness::new(
        "page { display: flex; align-items: flex-start; width: 400px; height: 400px; }
         .lanes { display: grid-lanes; width: 100px; height: 200px; overflow-x: scroll;
                  gap: 10px; flow-tolerance: 0;
                  grid-template-rows: repeat(2, minmax(0, 1fr)); }",
    );
    let root = h.doc.root;
    let lanes = h.doc.el(root, "view.lanes");
    let items = [30.0, 50.0, 20.0, 60.0, 40.0, 10.0]
        .into_iter()
        .map(|width| {
            let item = h.doc.el(lanes, "view");
            h.doc.set_inline(item, &format!("width: {width}px"));
            item
        })
        .collect::<Vec<_>>();
    h.layout();

    for (index, expected) in [
        (0.0, 0.0, 30.0, 95.0),
        (0.0, 105.0, 50.0, 95.0),
        (40.0, 0.0, 20.0, 95.0),
        (60.0, 105.0, 60.0, 95.0),
        (70.0, 0.0, 40.0, 95.0),
        (120.0, 0.0, 10.0, 95.0),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(h.rect(items[index]), expected, "item {index}");
    }
    let scroll_box = h
        .doc
        .dom
        .scroll_box(lanes)
        .expect("the lanes container scrolls");
    assert_eq!(scroll_box.scrollport.width, 100.0);
    assert_eq!(scroll_box.scroll_size.width, 130.0);
    assert_eq!(scroll_box.max_offset().x, 30.0);
}

/// A `display: contents` wrapper generates no box, so the items it holds are
/// placed exactly where they would be as direct children.
#[test]
fn grid_lanes_flattens_a_display_contents_wrapper() {
    let mut h = Harness::new(&format!("{LANES} .wrap {{ display: contents; }}"));
    let root = h.doc.root;
    let lanes = h.doc.el(root, "view.lanes");
    let first = h.doc.el(lanes, "view");
    h.doc.set_inline(first, "height: 30px");
    let wrap = h.doc.el(lanes, "view.wrap");
    let wrapped = [50.0, 20.0]
        .into_iter()
        .map(|height| {
            let item = h.doc.el(wrap, "view");
            h.doc.set_inline(item, &format!("height: {height}px"));
            item
        })
        .collect::<Vec<_>>();
    let last = h.doc.el(lanes, "view");
    h.doc.set_inline(last, "height: 60px");
    h.layout();

    assert_eq!(h.rect(first), (0.0, 0.0, 95.0, 30.0));
    assert_eq!(h.rect(wrapped[0]), (105.0, 0.0, 95.0, 50.0));
    assert_eq!(h.rect(wrapped[1]), (0.0, 40.0, 95.0, 20.0));
    assert_eq!(h.rect(last), (105.0, 60.0, 95.0, 60.0));
    assert_eq!(h.rect(lanes), (0.0, 0.0, 200.0, 120.0));
}

/// css-grid-3 §4.2: `flow-tolerance: normal` is `1em` of the container's own
/// font, so the same three items land in different lanes under two font sizes.
#[test]
fn normal_flow_tolerance_resolves_against_the_containers_font_size() {
    for (font_size, third) in [(16.0_f32, (100.0, 20.0)), (40.0_f32, (0.0, 50.0))] {
        let mut h = Harness::new(
            "page { display: flex; align-items: flex-start; width: 400px; height: 400px; }
             .lanes { display: grid-lanes; width: 200px; gap: 0px;
                      grid-template-columns: repeat(2, minmax(0, 1fr)); }",
        );
        let root = h.doc.root;
        let lanes = h.doc.el(root, "view.lanes");
        h.doc
            .set_inline(lanes, &format!("font-size: {font_size}px"));
        let items = [50.0, 20.0, 20.0]
            .into_iter()
            .map(|height| {
                let item = h.doc.el(lanes, "view");
                h.doc.set_inline(item, &format!("height: {height}px"));
                item
            })
            .collect::<Vec<_>>();
        h.layout();

        assert_eq!(
            h.doc.value(lanes, "flow-tolerance"),
            "normal",
            "the tolerance keyword survives to computed-value time",
        );
        assert_eq!(h.rect(items[0]), (0.0, 0.0, 100.0, 50.0), "{font_size}px");
        assert_eq!(h.rect(items[1]), (100.0, 0.0, 100.0, 20.0), "{font_size}px");
        assert_eq!(
            h.rect(items[2]),
            (third.0, third.1, 100.0, 20.0),
            "the third item's lane is decided by a {font_size}px tolerance",
        );
    }
}

/// Neither a non-generated box nor an out-of-flow one is a grid item, so
/// neither occupies a lane or moves the items after it.
#[test]
fn grid_lanes_skips_hidden_and_out_of_flow_children() {
    let mut h = Harness::new(&format!(
        "{LANES} .gone {{ display: none; }}
         .badge {{ position: absolute; left: 0; top: 0; }}"
    ));
    let root = h.doc.root;
    let lanes = h.doc.el(root, "view.lanes");
    let first = h.doc.el(lanes, "view");
    h.doc.set_inline(first, "height: 20px");
    let gone = h.doc.el(lanes, "view.gone");
    h.doc.set_inline(gone, "height: 100px");
    let badge = h.doc.el(lanes, "view.badge");
    h.doc.set_inline(badge, "width: 12px; height: 100px");
    let second = h.doc.el(lanes, "view");
    h.doc.set_inline(second, "height: 20px");
    h.layout();

    assert_eq!(h.rect(first), (0.0, 0.0, 95.0, 20.0));
    assert_eq!(h.rect(second), (105.0, 0.0, 95.0, 20.0));
    assert_eq!(h.rect(gone), (0.0, 0.0, 0.0, 0.0), "a hidden box is zeroed");
    assert_eq!(
        h.rect(lanes),
        (0.0, 0.0, 200.0, 20.0),
        "neither child contributes to the stacking range",
    );
    assert_eq!(h.rect(badge).3, 100.0, "the out-of-flow box is still sized");
}

// ---------------------------------------------------------------------------
// Replaced content with an intrinsic aspect ratio, inside each container
//
// The seam: `Document::set_natural_size` makes a node replaced, and the node's
// natural size then reaches layout twice — as the leaf's own measurement, and
// as `CoreStyle::natural_size`, which the container above reads to settle
// css-grid-1 §6.2 `normal` self-alignment before it measures anything. These
// assert the laid-out size, not any element's behaviour, because every element
// that carries decoded pixels arrives here the same way.
// ---------------------------------------------------------------------------

/// Attaches a 40x20 natural size to a fresh child, which is what a decoded
/// bitmap does and what makes the node replaced.
fn replaced_child(h: &mut Harness, parent: NodeId, spec: &str) -> NodeId {
    let node = h.doc.el(parent, spec);
    h.doc.dom.set_natural_size(
        node,
        dom::layout::NaturalSize::from_size(dom::layout::Size::new(40.0, 20.0)),
    );
    node
}

/// css-align-3 §6.2.4: a flex item's `normal` cross alignment is `stretch`,
/// with no exception for replaced content, and css-flexbox-1 §9.8 makes that
/// stretched cross size definite for the §9.2 flex-base measurement. The item
/// fills the 100 of height and its ratio carries the width to 200.
#[test]
fn a_replaced_item_stretches_and_transfers_inside_a_flex_row() {
    let mut h = Harness::new("page { display: flex; width: 300px; height: 100px; }");
    let root = h.doc.root;
    let item = replaced_child(&mut h, root, "image");
    h.layout();

    assert_eq!(h.rect(item), (0.0, 0.0, 200.0, 100.0));
}

/// css-grid-1 §6.2: `normal` does not stretch a replaced box with a natural
/// size — it is sized by the block-level rules for replaced elements, at
/// 40x20. An explicit `stretch` does stretch it, in both axes, which §6.2's
/// own note says will distort the ratio.
#[test]
fn a_replaced_grid_item_keeps_its_natural_size_until_it_is_told_to_stretch() {
    for (class, expected) in [
        ("image", (0.0, 0.0, 40.0, 20.0)),
        ("image.fill", (0.0, 0.0, 200.0, 200.0)),
    ] {
        let mut h = Harness::new(
            "page { display: grid; width: 400px; height: 400px;
                    grid-template-columns: 200px; grid-template-rows: 200px; }
             .fill { justify-self: stretch; align-self: stretch; }",
        );
        let root = h.doc.root;
        let item = replaced_child(&mut h, root, class);
        h.layout();

        assert_eq!(h.rect(item), expected, "{class}");
    }
}

/// css-grid-1 §11.5 with css-sizing-4 §5: a replaced item stretched across a
/// definite column contributes the ratio-transferred height to the `auto` row,
/// so the row is 100 rather than the item's natural 20.
#[test]
fn a_stretched_replaced_grid_item_transfers_into_an_auto_row() {
    // The grid is nested so that its own block size stays indefinite: a root
    // box fills the viewport, and css-grid-1 §11.8 would then stretch the
    // `auto` row to that instead of leaving it at the contribution.
    let mut h = Harness::new(
        "page { display: flex; flex-direction: column; align-items: flex-start;
                width: 400px; height: 400px; }
         .grid { display: grid; grid-template-columns: 200px;
                 grid-template-rows: auto; }
         .fill-inline { justify-self: stretch; }",
    );
    let root = h.doc.root;
    let grid = h.doc.el(root, "view.grid");
    let item = replaced_child(&mut h, grid, "image.fill-inline");
    h.layout();

    assert_eq!(h.rect(item), (0.0, 0.0, 200.0, 100.0));
    assert_eq!(h.rect(grid).3, 100.0, "the auto row takes the transfer");
}

/// css-grid-3 §6.2 sends grid-axis alignment straight through regular Grid, so
/// a lanes item told to fill takes its lane's width and its stacking height
/// from the ratio — and the next item stacks behind it at that height.
#[test]
fn replaced_lanes_items_take_their_lane_width_and_stack_by_the_transferred_height() {
    let mut h = Harness::new(
        "page { display: grid-lanes; width: 200px; flow-tolerance: 0;
                grid-template-columns: repeat(2, 100px); }
         image { justify-self: stretch; }",
    );
    let root = h.doc.root;
    let first = replaced_child(&mut h, root, "image");
    let second = replaced_child(&mut h, root, "image");
    let third = replaced_child(&mut h, root, "image");
    h.layout();

    assert_eq!(h.rect(first), (0.0, 0.0, 100.0, 50.0));
    assert_eq!(h.rect(second), (100.0, 0.0, 100.0, 50.0));
    assert_eq!(h.rect(third), (0.0, 50.0, 100.0, 50.0));
    assert_eq!(h.rect(root).3, 100.0);
}

/// The same lanes without the fill: §6.2's `normal` leaves each replaced item
/// at 40x20 inside its 100-wide lane, and the stacking range follows the
/// smaller items.
#[test]
fn normal_alignment_leaves_replaced_lanes_items_at_their_natural_size() {
    let mut h = Harness::new(
        "page { display: grid-lanes; width: 200px; flow-tolerance: 0;
                grid-template-columns: repeat(2, 100px); }",
    );
    let root = h.doc.root;
    let first = replaced_child(&mut h, root, "image");
    let second = replaced_child(&mut h, root, "image");
    let third = replaced_child(&mut h, root, "image");
    h.layout();

    assert_eq!(h.rect(first), (0.0, 0.0, 40.0, 20.0));
    assert_eq!(h.rect(second), (100.0, 0.0, 40.0, 20.0));
    assert_eq!(h.rect(third), (0.0, 20.0, 40.0, 20.0));
    assert_eq!(h.rect(root).3, 40.0);
}
