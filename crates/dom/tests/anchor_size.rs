//! css-anchor-position-1 `anchor-name` (§2.1) and `anchor-size()` (§5.1),
//! resolved through a real document.
//!
//! The grammar lives in the vendored Stylo fork (`lynx` feature): the computed
//! value keeps the function, and `hughie` resolves it during layout through
//! `dom`'s target-anchor lookup (§2.3, `crates/dom/src/layout/anchors.rs`).
//! These tests pin `anchor-size()` itself and the lookup cases its first
//! users rely on; the rest of the module (and the rest of §2.3) is
//! `anchor_positioning.rs`'s. The WPT cases are ported from
//! `css/css-anchor-position/anchor-size-*.html`; each names its file and says
//! what changed.

#![allow(clippy::float_cmp)]

mod common;

use common::Doc;
use dom::NodeId;
use dom::layout::{NaturalSize, Size};

/// A document laid out from `css`, with helpers for rounded geometry.
struct Page {
    doc: Doc,
}

impl Page {
    fn new(css: &str) -> Self {
        Self {
            doc: Doc::with_css(css),
        }
    }

    fn el(&mut self, parent: NodeId, spec: &str, inline: &str) -> NodeId {
        let id = self.doc.el(parent, spec);
        if !inline.is_empty() {
            self.doc.set_inline(id, inline);
        }
        id
    }

    fn layout(&mut self) {
        self.doc.dom.layout();
    }

    /// `(x, y, width, height)` of the rounded border box, in the box parent.
    fn rect(&self, id: NodeId) -> (f32, f32, f32, f32) {
        let layout = self.doc.dom.rounded_layout(id).expect("node is laid out");
        (
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        )
    }

    fn size(&self, id: NodeId) -> (f32, f32) {
        let (_, _, width, height) = self.rect(id);
        (width, height)
    }

    /// `(left, right, top, bottom)` used margins.
    fn margin(&self, id: NodeId) -> (f32, f32, f32, f32) {
        let margin = self.doc.dom.rounded_layout(id).expect("laid out").margin;
        (margin.left, margin.right, margin.top, margin.bottom)
    }

    fn scroll_range(&self, id: NodeId) -> f32 {
        self.doc
            .dom
            .scroll_box(id)
            .expect("a scroll container")
            .max_offset()
            .y
    }
}

// ---------------------------------------------------------------------------
// Grammar: what reaches computed style.

#[test]
fn anchor_size_parses_in_every_property_the_fork_admits() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(
        el,
        "anchor-name: --a, --b; width: anchor-size(--a width); \
         height: anchor-size(--a height, 0px); min-width: anchor-size(--a); \
         max-height: calc(100% - anchor-size(--t height, 0px)); \
         top: anchor-size(--a height); margin-left: anchor-size(--a block, 4px)",
    );
    doc.flush();
    assert_eq!(doc.value(el, "anchor-name"), "--a, --b");
    assert_eq!(doc.value(el, "width"), "anchor-size(--a width)");
    assert_eq!(doc.value(el, "height"), "anchor-size(--a height, 0px)");
    assert_eq!(doc.value(el, "min-width"), "anchor-size(--a)");
    assert_eq!(
        doc.value(el, "max-height"),
        "calc(100% - anchor-size(--t height, 0px))"
    );
    assert_eq!(doc.value(el, "top"), "anchor-size(--a height)");
    assert_eq!(doc.value(el, "margin-left"), "anchor-size(--a block, 4px)");
}

#[test]
fn anchor_parses_in_an_inset() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, "top: 3px; top: anchor(--a top)");
    doc.flush();
    assert_eq!(doc.value(el, "top"), "anchor(--a top)");
}

// ---------------------------------------------------------------------------
// web-platform-tests ports.

/// The shared page of `anchor-size-001.html`, `anchor-size-minmax-001.html`
/// and `anchor-size-replaced-001.html`: a positioned 10px-tall container
/// whose first child is the 5×7 (24 tall in the replaced test) anchor
/// `--a1`. Adaptation: the container is a column flexbox rather than a block,
/// the one block-level layout this engine has, which places the in-flow
/// anchor and every static position at the container's origin just as block
/// flow does.
fn wpt_page(anchor_height: f32) -> (Page, NodeId) {
    let mut page = Page::new(&format!(
        "page {{ display: flex; flex-direction: column; width: 800px; height: 600px; }}
         .container {{ display: flex; flex-direction: column; position: relative;
                       height: 10px; }}
         .anchor1 {{ anchor-name: --a1; width: 5px; height: {anchor_height}px;
                     flex-shrink: 0; }}
         .target {{ display: flex; position: absolute; }}"
    ));
    let root = page.doc.root;
    let container = page.el(root, ".container", "");
    page.el(container, ".anchor1", "");
    (page, container)
}

/// What one `anchor-size-001.html` target checks.
#[derive(Clone, Copy)]
enum Expect {
    Width(f32),
    Height(f32),
    OffsetX(f32),
    OffsetY(f32),
    MarginLeft(f32),
    MarginTop(f32),
    MarginRight(f32),
    MarginBottom(f32),
}

/// wpt `css/css-anchor-position/anchor-size-001.html`, every case: each
/// keyword on each axis in a size, an inset and a margin, and the implicit
/// keyword. Adaptation: `data-offset-*` reads the box's position in the
/// container, which is its offset parent there.
#[test]
fn wpt_anchor_size_001() {
    let (mut page, container) = wpt_page(7.0);
    let cases = [
        ("width: anchor-size(--a1 width)", Expect::Width(5.0)),
        ("height: anchor-size(--a1 height)", Expect::Height(7.0)),
        ("width: anchor-size(--a1 inline)", Expect::Width(5.0)),
        ("height: anchor-size(--a1 block)", Expect::Height(7.0)),
        ("width: anchor-size(--a1 self-inline)", Expect::Width(5.0)),
        ("height: anchor-size(--a1 self-block)", Expect::Height(7.0)),
        ("height: anchor-size(--a1 width)", Expect::Height(5.0)),
        ("width: anchor-size(--a1 height)", Expect::Width(7.0)),
        ("height: anchor-size(--a1 inline)", Expect::Height(5.0)),
        ("width: anchor-size(--a1 block)", Expect::Width(7.0)),
        ("height: anchor-size(--a1 self-inline)", Expect::Height(5.0)),
        ("width: anchor-size(--a1 self-block)", Expect::Width(7.0)),
        ("left: anchor-size(--a1 width)", Expect::OffsetX(5.0)),
        ("top: anchor-size(--a1 height)", Expect::OffsetY(7.0)),
        ("left: anchor-size(--a1 height)", Expect::OffsetX(7.0)),
        // The WPT source leaves this one's parenthesis open, which the CSS
        // tokenizer closes at the end of the declaration.
        ("top: anchor-size(--a1 width", Expect::OffsetY(5.0)),
        (
            "margin-left: anchor-size(--a1 width)",
            Expect::MarginLeft(5.0),
        ),
        (
            "margin-top: anchor-size(--a1 height)",
            Expect::MarginTop(7.0),
        ),
        (
            "margin-right: anchor-size(--a1 height)",
            Expect::MarginRight(7.0),
        ),
        (
            "margin-bottom: anchor-size(--a1 width",
            Expect::MarginBottom(5.0),
        ),
        ("width: anchor-size(--a1)", Expect::Width(5.0)),
        ("height: anchor-size(--a1)", Expect::Height(7.0)),
        ("left: anchor-size(--a1)", Expect::OffsetX(5.0)),
        ("top: anchor-size(--a1)", Expect::OffsetY(7.0)),
        ("margin-left: anchor-size(--a1)", Expect::MarginLeft(5.0)),
        ("margin-right: anchor-size(--a1)", Expect::MarginRight(5.0)),
        ("margin-top: anchor-size(--a1)", Expect::MarginTop(7.0)),
        (
            "margin-bottom: anchor-size(--a1)",
            Expect::MarginBottom(7.0),
        ),
    ];
    let targets: Vec<NodeId> = cases
        .iter()
        .map(|(inline, _)| page.el(container, ".target", inline))
        .collect();
    page.layout();
    for (&target, &(inline, expect)) in targets.iter().zip(&cases) {
        let (x, y, width, height) = page.rect(target);
        let (left, right, top, bottom) = page.margin(target);
        let (actual, expected) = match expect {
            Expect::Width(value) => (width, value),
            Expect::Height(value) => (height, value),
            Expect::OffsetX(value) => (x, value),
            Expect::OffsetY(value) => (y, value),
            Expect::MarginLeft(value) => (left, value),
            Expect::MarginTop(value) => (top, value),
            Expect::MarginRight(value) => (right, value),
            Expect::MarginBottom(value) => (bottom, value),
        };
        assert_eq!(actual, expected, "`{inline}`");
    }
}

/// wpt `css/css-anchor-position/anchor-size-minmax-001.html`, every case:
/// min/max sizes on boxes whose own size is `auto`, sized from content.
#[test]
fn wpt_anchor_size_minmax_001() {
    let (mut page, container) = wpt_page(7.0);
    let min_width = page.el(container, ".target", "min-width: anchor-size(--a1 width)");
    let min_height = page.el(container, ".target", "min-height: anchor-size(--a1 width)");
    let max_width = page.el(container, ".target", "max-width: anchor-size(--a1 width)");
    page.el(max_width, "view", "width: 100px; flex-shrink: 0");
    let max_height = page.el(container, ".target", "max-height: anchor-size(--a1 width)");
    page.el(max_height, "view", "height: 100px; flex-shrink: 0");
    page.layout();
    assert_eq!(page.size(min_width).0, 5.0);
    assert_eq!(page.size(min_height).1, 5.0);
    assert_eq!(page.size(max_width).0, 5.0);
    assert_eq!(page.size(max_height).1, 5.0);
}

/// wpt `css/css-anchor-position/anchor-size-function-chain.html`: five
/// absolutely positioned boxes all named `--box`, each after the first sized
/// from the one before it — the last acceptable `--box` in tree order, since
/// a later absolutely positioned one is not laid out yet.
#[test]
fn wpt_anchor_size_function_chain() {
    let mut page = Page::new(
        "page { display: flex; width: 800px; height: 600px; }
         .containing-block { display: flex; border: 1px solid black; position: relative;
                             width: 500px; height: 200px; }
         .box { display: flex; position: absolute; anchor-name: --box; }
         #box1 { width: 50px; height: 60px; }
         #box2, #box3, #box4, #box5 {
           width: calc(anchor-size(--box width) + 10px);
           height: calc(anchor-size(--box height) + 20px);
         }
         #box2 { left: 60px; } #box3 { left: 130px; }
         #box4 { left: 210px; } #box5 { left: 300px; }",
    );
    let root = page.doc.root;
    let block = page.el(root, ".containing-block", "");
    let boxes: Vec<NodeId> = (1..=5)
        .map(|index| page.el(block, &format!("#box{index}.box"), ""))
        .collect();
    page.layout();
    let sizes: Vec<(f32, f32)> = boxes.iter().map(|&id| page.size(id)).collect();
    assert_eq!(
        sizes,
        [
            (50.0, 60.0),
            (60.0, 80.0),
            (70.0, 100.0),
            (80.0, 120.0),
            (90.0, 140.0)
        ]
    );
}

/// wpt `css/css-anchor-position/anchor-size-replaced-001.html`, every case:
/// a 16×16 replaced box against a 5×24 anchor. Adaptation: the image is an
/// element given the natural size `green-16x16.png` decodes to.
#[test]
fn wpt_anchor_size_replaced_001() {
    let (mut page, container) = wpt_page(24.0);
    let cases = [
        ("width: anchor-size(--a1 width)", (5.0, 5.0)),
        ("height: anchor-size(--a1 width)", (5.0, 5.0)),
        ("min-width: anchor-size(--a1 width)", (16.0, 16.0)),
        ("min-height: anchor-size(--a1 width)", (16.0, 16.0)),
        ("min-width: anchor-size(--a1 height)", (24.0, 24.0)),
        ("min-height: anchor-size(--a1 height)", (24.0, 24.0)),
        ("max-width: anchor-size(--a1 width)", (5.0, 5.0)),
        ("max-height: anchor-size(--a1 width)", (5.0, 5.0)),
        ("max-width: anchor-size(--a1 height)", (16.0, 16.0)),
        ("max-height: anchor-size(--a1 height)", (16.0, 16.0)),
        (
            "width: anchor-size(--a1 width); aspect-ratio: 0.5",
            (5.0, 10.0),
        ),
        (
            "height: anchor-size(--a1 width); aspect-ratio: 2",
            (10.0, 5.0),
        ),
    ];
    let images: Vec<NodeId> = cases
        .iter()
        .map(|(inline, _)| {
            let image = page.el(container, "img.target", inline);
            page.doc
                .dom
                .set_natural_size(image, NaturalSize::from_size(Size::new(16.0, 16.0)));
            image
        })
        .collect();
    page.layout();
    for (&image, &(inline, expected)) in images.iter().zip(&cases) {
        assert_eq!(page.size(image), expected, "`{inline}`");
    }
}

// ---------------------------------------------------------------------------
// Resolution and fallback.

fn anchored_page() -> (Page, NodeId) {
    let mut page = Page::new(
        "page { display: flex; width: 800px; height: 600px; }
         .cb { display: flex; flex-direction: column; position: relative;
               width: 300px; height: 200px; }
         .anchor { anchor-name: --a; width: 40px; height: 30px; flex-shrink: 0; }
         .abs { display: flex; position: absolute; left: 0; top: 0; }",
    );
    let root = page.doc.root;
    let cb = page.el(root, ".cb", "");
    (page, cb)
}

#[test]
fn an_absolute_box_reads_its_anchor_in_sizes_insets_and_margins() {
    let (mut page, cb) = anchored_page();
    page.el(cb, ".anchor", "");
    let sized = page.el(
        cb,
        ".abs",
        "width: anchor-size(--a width); height: anchor-size(--a height, 10px)",
    );
    let inset = page.el(
        cb,
        ".abs",
        "top: anchor-size(--a height); width: 1px; height: 1px",
    );
    let margin = page.el(
        cb,
        ".abs",
        "margin-top: anchor-size(--a height); width: 1px; height: 1px",
    );
    let calc = page.el(
        cb,
        ".abs",
        "width: 1px; height: calc(100% - anchor-size(--a height, 0px))",
    );
    page.layout();
    assert_eq!(page.rect(sized), (0.0, 0.0, 40.0, 30.0));
    assert_eq!(page.rect(inset), (0.0, 30.0, 1.0, 1.0));
    assert_eq!(page.rect(margin), (0.0, 30.0, 1.0, 1.0));
    assert_eq!(page.margin(margin).2, 30.0);
    assert_eq!(page.rect(calc), (0.0, 0.0, 1.0, 170.0));
}

#[test]
fn without_a_target_the_fallback_applies_and_without_one_the_initial_value() {
    let (mut page, cb) = anchored_page();
    let fallback = page.el(
        cb,
        ".abs",
        "width: anchor-size(--missing width, 12px); height: anchor-size(--missing height, 10px)",
    );
    // Invalid at computed-value time: `auto`, so the box is its content's
    // size — here a 7×9 child.
    let initial = page.el(cb, ".abs", "width: anchor-size(--missing width)");
    page.el(initial, "view", "width: 7px; height: 9px; flex-shrink: 0");
    let inset = page.el(
        cb,
        ".abs",
        "top: anchor-size(--missing height); bottom: 5px; width: 1px; height: 1px",
    );
    let calc = page.el(
        cb,
        ".abs",
        "width: 1px; height: calc(100% - anchor-size(--missing height, 50px))",
    );
    let calc_invalid = page.el(
        cb,
        ".abs",
        "width: 1px; height: calc(100% - anchor-size(--missing height))",
    );
    page.el(calc_invalid, "view", "height: 3px; flex-shrink: 0");
    page.layout();
    assert_eq!(page.size(fallback), (12.0, 10.0));
    assert_eq!(page.size(initial), (7.0, 9.0));
    // `top: auto` with `bottom: 5px` puts the box 5px above the bottom.
    assert_eq!(page.rect(inset).1, 200.0 - 5.0 - 1.0);
    assert_eq!(page.size(calc), (1.0, 150.0));
    assert_eq!(page.size(calc_invalid), (1.0, 3.0));
}

#[test]
fn an_in_flow_box_takes_the_fallback_or_the_initial_value() {
    let (mut page, cb) = anchored_page();
    page.el(cb, ".anchor", "");
    // Relatively positioned and in flow: §5.1.1 resolves nothing for it even
    // with an anchor right before it.
    let fallback = page.el(
        cb,
        "view",
        "position: relative; flex-shrink: 0; width: anchor-size(--a width, 11px); \
         height: anchor-size(--a height, 13px); top: anchor-size(--a height, 2px)",
    );
    let initial = page.el(
        cb,
        "view",
        "position: relative; flex-shrink: 0; width: anchor-size(--a width); height: 4px; \
         top: anchor-size(--a height); margin-left: anchor-size(--a width)",
    );
    page.layout();
    // Below the 30px anchor, nudged down by the 2px fallback.
    assert_eq!(page.rect(fallback), (0.0, 32.0, 11.0, 13.0));
    // `width: auto` stretches across the column; `top: auto` and a `0` margin
    // leave the box where the column put it.
    assert_eq!(page.rect(initial), (0.0, 43.0, 300.0, 4.0));
}

#[test]
fn a_box_placed_by_the_positioned_pass_resolves_through_the_containing_block_chain() {
    // The parent is not positioned, so the absolute box escapes to the
    // positioned `.cb` and is placed by the rounding tail's positioned pass.
    // Its containing block is `.cb`, whose in-flow child is the anchor, so
    // §2.3 accepts it. The `fixed` box's containing block is the viewport;
    // the anchor's chain reaches the root element, which the initial
    // containing block (here: the viewport) contains, in flow.
    let (mut page, cb) = anchored_page();
    page.el(cb, ".anchor", "");
    let parent = page.el(
        cb,
        "view",
        "display: flex; width: 50px; height: 50px; flex-shrink: 0",
    );
    let escaped = page.el(
        parent,
        ".abs",
        "width: anchor-size(--a width, 3px); height: anchor-size(--a height, 4px)",
    );
    let fixed = page.el(
        cb,
        ".abs",
        "position: fixed; width: anchor-size(--a width, 5px); height: 6px",
    );
    page.layout();
    assert_eq!(page.size(escaped), (40.0, 30.0));
    assert_eq!(page.size(fixed), (40.0, 6.0));
}

#[test]
fn a_fixed_box_whose_parent_contains_it_resolves_like_an_absolute_one() {
    // A `transform` makes the parent the containing block of a `fixed` child,
    // so the parent's own absolute pass lays the box out; a `fixed` box is
    // absolutely positioned, and §5.1.1 resolves for it.
    let (mut page, cb) = anchored_page();
    page.doc.set_inline(cb, "transform: translateX(0px)");
    page.el(cb, ".anchor", "");
    let fixed = page.el(
        cb,
        ".abs",
        "position: fixed; width: anchor-size(--a width, 5px); height: 6px",
    );
    page.layout();
    assert_eq!(page.size(fixed), (40.0, 6.0));
}

// ---------------------------------------------------------------------------
// Which element is the target (§2.3).

#[test]
fn an_in_flow_anchor_after_the_query_is_acceptable() {
    let (mut page, cb) = anchored_page();
    let query = page.el(
        cb,
        ".abs",
        "width: anchor-size(--a width, 1px); height: 1px",
    );
    page.el(cb, ".anchor", "");
    page.layout();
    assert_eq!(page.size(query).0, 40.0);
}

#[test]
fn an_absolute_anchor_after_the_query_is_not() {
    let (mut page, cb) = anchored_page();
    let query = page.el(
        cb,
        ".abs",
        "width: anchor-size(--a width, 1px); height: 1px",
    );
    page.el(cb, ".anchor.abs", "");
    page.layout();
    assert_eq!(page.size(query).0, 1.0);
}

#[test]
fn the_last_acceptable_anchor_in_tree_order_wins() {
    let (mut page, cb) = anchored_page();
    page.el(cb, ".anchor", "");
    let query = page.el(cb, ".abs", "width: anchor-size(--a width); height: 1px");
    // In flow, so acceptable though it follows the query; last in tree order.
    page.el(cb, ".anchor", "width: 60px");
    // Absolutely positioned and after the query: not laid out yet.
    page.el(cb, ".anchor.abs", "width: 90px");
    page.layout();
    assert_eq!(page.size(query).0, 60.0);
}

#[test]
fn an_anchor_through_a_contents_wrapper_is_a_box_child_of_the_containing_block() {
    let (mut page, cb) = anchored_page();
    let wrapper = page.el(cb, "view", "display: contents");
    page.el(wrapper, ".anchor", "");
    let query = page.el(
        cb,
        ".abs",
        "width: anchor-size(--a width, 1px); height: 1px",
    );
    page.layout();
    assert_eq!(page.size(query).0, 40.0);
}

#[test]
fn an_ancestor_is_never_a_target_of_a_box_its_parent_contains() {
    // §2.3 wants an anchor inside the query box's containing block; the
    // containing block of an absolute box laid out by its parent is that
    // parent, so the parent and everything above it are outside.
    let (mut page, cb) = anchored_page();
    page.doc.set_inline(cb, "anchor-name: --a");
    let query = page.el(
        cb,
        ".abs",
        "width: anchor-size(--a width, 1px); height: 1px",
    );
    page.layout();
    assert_eq!(page.size(query).0, 1.0);
}

#[test]
fn an_anchor_inside_a_sibling_subtree_is_acceptable() {
    // §2.3's containing-block-chain clause: the anchor's containing block is
    // generated by the sibling, which is in flow in the query box's
    // containing block.
    let (mut page, cb) = anchored_page();
    let sibling = page.el(cb, "view", "display: flex; flex-shrink: 0");
    page.el(sibling, ".anchor", "");
    let query = page.el(
        cb,
        ".abs",
        "width: anchor-size(--a width, 1px); height: 1px",
    );
    page.layout();
    assert_eq!(page.size(query).0, 40.0);
}

#[test]
fn a_hidden_anchor_is_not_a_target() {
    let (mut page, cb) = anchored_page();
    page.el(cb, ".anchor", "width: 20px");
    page.el(cb, ".anchor", "display: none");
    let query = page.el(
        cb,
        ".abs",
        "width: anchor-size(--a width, 1px); height: 1px",
    );
    page.layout();
    assert_eq!(page.size(query).0, 20.0);
}

// ---------------------------------------------------------------------------
// The `<scroll-coordinator>` geometry in plain CSS (design §2).

const COORDINATOR: &str = "
    page { display: flex; width: 400px; height: 600px; }
    .coordinator { display: flex; flex-direction: column; position: relative;
                   width: 400px; height: 600px; overflow-y: scroll; }
    .toolbar { position: sticky; top: 0; flex: 0 0 auto; height: 200px;
               anchor-name: --t; }
    .header { position: absolute; top: 0; left: 0; width: 100%; height: 400px;
              anchor-name: --h; }
    .slot { position: absolute; left: 0; width: 100%;
            top: anchor-size(--h height, 0px);
            height: calc(100% - anchor-size(--t height, 0px)); }";

#[test]
fn the_coordinator_slot_sits_under_the_header_and_fills_below_the_toolbar() {
    let mut page = Page::new(COORDINATOR);
    let root = page.doc.root;
    let coordinator = page.el(root, ".coordinator", "");
    let toolbar = page.el(coordinator, ".toolbar", "");
    let header = page.el(coordinator, ".header", "");
    let slot = page.el(coordinator, ".slot", "");
    page.layout();
    assert_eq!(page.rect(toolbar), (0.0, 0.0, 400.0, 200.0));
    assert_eq!(page.rect(header), (0.0, 0.0, 400.0, 400.0));
    assert_eq!(page.rect(slot), (0.0, 400.0, 400.0, 400.0));
    // Header height minus toolbar height.
    assert_eq!(page.scroll_range(coordinator), 200.0);
}

#[test]
fn without_a_toolbar_the_slot_fills_the_scrollport() {
    let mut page = Page::new(COORDINATOR);
    let root = page.doc.root;
    let coordinator = page.el(root, ".coordinator", "");
    page.el(coordinator, ".header", "");
    let slot = page.el(coordinator, ".slot", "");
    page.layout();
    assert_eq!(page.rect(slot), (0.0, 400.0, 400.0, 600.0));
    assert_eq!(page.scroll_range(coordinator), 400.0);
}

#[test]
fn a_slot_before_its_header_falls_back_to_the_top() {
    let mut page = Page::new(COORDINATOR);
    let root = page.doc.root;
    let coordinator = page.el(root, ".coordinator", "");
    page.el(coordinator, ".toolbar", "");
    let slot = page.el(coordinator, ".slot", "");
    page.el(coordinator, ".header", "");
    page.layout();
    assert_eq!(page.rect(slot), (0.0, 0.0, 400.0, 400.0));
}

// ---------------------------------------------------------------------------
// Invalidation: a moved anchor lays the query box out again.

#[test]
fn a_resized_header_moves_the_slot() {
    let mut page = Page::new(COORDINATOR);
    let root = page.doc.root;
    let coordinator = page.el(root, ".coordinator", "");
    let toolbar = page.el(coordinator, ".toolbar", "");
    let header = page.el(coordinator, ".header", "");
    let slot = page.el(coordinator, ".slot", "");
    page.layout();
    assert_eq!(page.rect(slot).1, 400.0);

    page.doc.set_inline(header, "height: 250px");
    page.layout();
    assert_eq!(page.rect(slot), (0.0, 250.0, 400.0, 400.0));
    assert_eq!(page.scroll_range(coordinator), 50.0);

    page.doc.set_inline(toolbar, "height: 100px");
    page.layout();
    assert_eq!(page.rect(slot), (0.0, 250.0, 400.0, 500.0));
}

#[test]
fn a_header_that_grows_with_its_content_moves_the_slot() {
    // The header's height is its content's, so the change starts below it:
    // the header relays out under its own committed input, and only its new
    // size reaches the coordinator.
    let mut page = Page::new(COORDINATOR);
    let root = page.doc.root;
    let coordinator = page.el(root, ".coordinator", "");
    page.el(coordinator, ".toolbar", "");
    let header = page.el(coordinator, ".header", "display: flex; height: auto");
    let content = page.el(header, "view", "height: 300px; flex-shrink: 0");
    let slot = page.el(coordinator, ".slot", "");
    page.layout();
    assert_eq!(page.rect(slot).1, 300.0);

    page.doc
        .set_inline(content, "height: 350px; flex-shrink: 0");
    page.layout();
    assert_eq!(page.rect(slot).1, 350.0);
}

#[test]
fn renaming_the_anchor_drops_the_slot_to_its_fallback() {
    let mut page = Page::new(COORDINATOR);
    let root = page.doc.root;
    let coordinator = page.el(root, ".coordinator", "");
    page.el(coordinator, ".toolbar", "");
    let header = page.el(coordinator, ".header", "");
    let slot = page.el(coordinator, ".slot", "");
    page.layout();
    assert_eq!(page.rect(slot).1, 400.0);

    page.doc.set_inline(header, "anchor-name: --other");
    page.layout();
    assert_eq!(page.rect(slot).1, 0.0);
}
