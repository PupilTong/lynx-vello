//! css-anchor-position-1 through a real document: the `dom` host's target
//! anchor lookup (§2.3), default anchors (§2.4), anchor relevance (§2.5),
//! position options cascaded at the style harvest (§6), the last successful
//! position option (§6.5.1), the scrollable containing block
//! (css-position-4) and the settle loop (`crates/dom/src/layout/anchors.rs`).
//! `hughie`'s own rules are pinned against a mock host in
//! `crates/hughie/tests/anchor_positioning.rs`; `anchor-size()` has
//! `anchor_size.rs`.
//!
//! Most cases are web-platform-tests ports from
//! `css/css-anchor-position/`; each names its file and what changed. The
//! usual adaptation: this engine has no block layout, so a block container
//! is a column flexbox (`.col`), which stacks its children at the start
//! edge as block flow does but collapses no margins; expected numbers are
//! restated where a collapsed margin moved them.
//!
//! **Not ported, and why:** top-layer, popover and dialog cases (no top
//! layer), pseudo-element cases (`::before` anchors and implicit anchors),
//! `writing-mode` and `vertical-*` cases (only `direction` exists),
//! multicol and inline-fragmentation cases (no multicol, no inline boxes
//! with anchor names), `transform`-on-anchor cases (the layout box is the
//! anchor box, §28), CSSOM and Typed OM cases, `anchor-scroll-*` and
//! `position-visibility-*` (the painter's, V3), and parse/computed-value
//! cases (the fork's `lynx_anchor_positioning.rs` has them).

#![allow(clippy::float_cmp)]

mod common;

use common::Doc;
use dom::{NodeId, ShadowRootMode};

/// A border box: `(x, y, width, height)`.
type Rect4 = (f32, f32, f32, f32);

/// One `anchor-position-borders-001.html` case: containing block classes,
/// nested containing block classes, anchor classes, target classes, and the
/// expected offset and size.
type BorderCase = (
    &'static str,
    Option<&'static str>,
    &'static str,
    &'static str,
    Rect4,
);

/// One `position-anchor-match-parent-rendering.html` case: the parent chain
/// as `(classes, inline style)`, the probe's inline style, and whether it
/// anchors.
type MatchParentCase = (Vec<(&'static str, &'static str)>, &'static str, bool);

/// A document laid out from `css`.
struct Page {
    doc: Doc,
}

/// The block-flow stand-in every port shares, plus `.cb` for a positioned
/// containing block.
const BASE: &str = "
    page { display: flex; flex-direction: column; width: 800px; height: 600px; }
    view { display: flex; flex-direction: column; flex-shrink: 0; }
    .cb { position: relative; }";

impl Page {
    fn new(css: &str) -> Self {
        Self {
            doc: Doc::with_css(&format!("{BASE}\n{css}")),
        }
    }

    fn root(&self) -> NodeId {
        self.doc.root
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

    /// One rendering update: layout, the §6.5.1.1 recording, paint.
    fn render(&mut self) {
        self.doc.dom.render();
    }

    /// `(x, y, width, height)` of the border box in viewport coordinates.
    fn abs(&self, id: NodeId) -> Rect4 {
        let rect = self
            .doc
            .dom
            .bounding_client_rect(id)
            .expect("node is laid out");
        (
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        )
    }

    /// `(x, y, width, height)` of `id`'s border box relative to the padding
    /// box of `frame` — the WPT `data-offset-*` of a box whose offset parent
    /// is `frame`.
    fn offset(&self, id: NodeId, frame: NodeId) -> Rect4 {
        let (x, y, width, height) = self.abs(id);
        let (fx, fy, ..) = self.abs(frame);
        let border = self.doc.dom.rounded_layout(frame).expect("laid out").border;
        (x - fx - border.left, y - fy - border.top, width, height)
    }

    fn computed(&self, id: NodeId, property: &str) -> String {
        self.doc
            .dom
            .computed_style_text(id, property, false)
            .expect("a computed value")
    }
}

// ---------------------------------------------------------------------------
// anchor() and the target lookup.

/// wpt `anchor-position-001.html`: `anchor()` on all four insets, against
/// two anchors. Adaptation: the anchors' top margins do not collapse
/// through the container, so `--a1` sits 100px lower than in block flow and
/// the expected `y` is 200, not 100; the width and height are the file's.
#[test]
fn wpt_anchor_position_001() {
    let mut page = Page::new(
        "#a1 { anchor-name: --a1; margin-left: 100px; margin-top: 100px;
               width: 100px; height: 100px; }
         #a2 { anchor-name: --a2; margin-left: 500px; margin-top: 100px;
               width: 100px; height: 100px; }
         #target { position: absolute; left: anchor(--a1 right);
                   top: anchor(--a1 bottom); right: anchor(--a2 left);
                   bottom: anchor(--a2 top); }",
    );
    let root = page.root();
    let container = page.el(root, "view.cb", "");
    page.el(container, "view#a1", "");
    page.el(container, "view#a2", "");
    let target = page.el(container, "view#target", "");
    page.layout();
    assert_eq!(page.offset(target, container), (200.0, 200.0, 300.0, 100.0));
}

/// wpt `anchor-position-002.html`: anchors whose own containing blocks
/// differ from the query box's are acceptable through the containing-block
/// chain — an in-flow wrapper, and a `transform` containing block holding
/// an absolutely positioned and a fixed wrapper.
#[test]
fn wpt_anchor_position_002() {
    let mut page = Page::new(
        "#container { transform: translate(0px, 0px); }
         #anchor1 { anchor-name: --a1; width: 5px; height: 7px; }
         #anchor2 { anchor-name: --a2; width: 9px; height: 11px; }
         #anchor3 { anchor-name: --a3; width: 13px; height: 15px; }
         .target { position: absolute; }",
    );
    let root = page.root();
    let container = page.el(root, "view.cb#container", "");
    let wrapper = page.el(container, "view", "");
    page.el(wrapper, "view#anchor1", "");
    let first = page.el(container, "view.target", "left: anchor(--a1 right)");
    let outer = page.el(container, "view", "");
    let transformed = page.el(outer, "view", "transform: translate(0px, 0px)");
    let positioned = page.el(transformed, "view", "position: absolute; left: 10px");
    page.el(positioned, "view#anchor2", "");
    let second = page.el(container, "view.target", "left: anchor(--a2 right)");
    let outer = page.el(container, "view", "");
    let transformed = page.el(outer, "view", "transform: translate(0px, 0px)");
    let fixed = page.el(transformed, "view", "position: fixed; left: 20px");
    page.el(fixed, "view#anchor3", "");
    let third = page.el(container, "view.target", "left: anchor(--a3 right)");
    page.layout();
    assert_eq!(page.offset(first, container).0, 5.0);
    assert_eq!(page.offset(second, container).0, 19.0);
    assert_eq!(page.offset(third, container).0, 33.0);
}

/// wpt `anchor-position-borders-001.html`, every case: the containing
/// block's margins, borders and padding, a nested containing block's, the
/// anchor's and the query box's own. `anchor()` answers in the containing
/// block's padding-box coordinates.
#[test]
#[allow(clippy::too_many_lines, reason = "the file's eleven cases, one table")]
fn wpt_anchor_position_borders_001() {
    let mut page = Page::new(
        ".cb { border-bottom: 2px solid gray; }
         .not-positioned-cb { transform: translate(0px, 0px); }
         .margins { margin: 5px 6px 7px 8px; }
         .borders { border-width: 5px 6px 7px 8px; border-style: solid; }
         .paddings { padding: 5px 6px 7px 8px; }
         .spacer { height: 9px; }
         .anchor1 { anchor-name: --a1; margin-left: 50px; width: 31px; height: 31px; }
         .target { position: absolute; left: anchor(--a1 left); right: anchor(--a1 right);
                   top: anchor(--a1 top); bottom: anchor(--a1 bottom); }",
    );
    let root = page.root();
    // (containing block classes, nested containing block classes, anchor
    // classes, target classes, expected offset and size). The anchor sits
    // at `margin-left: 50px`; its `.margins` case moves it by the top
    // margin only (the left margin is the anchor's own 50px, overridden).
    let cases: [BorderCase; 11] = [
        (
            "view.cb.margins",
            None,
            "view.anchor1",
            "view.target",
            (50.0, 9.0, 31.0, 31.0),
        ),
        (
            "view.cb.borders",
            None,
            "view.anchor1",
            "view.target",
            (50.0, 9.0, 31.0, 31.0),
        ),
        (
            "view.cb.paddings",
            None,
            "view.anchor1",
            "view.target",
            (58.0, 14.0, 31.0, 31.0),
        ),
        (
            "view.cb",
            Some("view.not-positioned-cb.margins"),
            "view.anchor1",
            "view.target",
            (58.0, 14.0, 31.0, 31.0),
        ),
        (
            "view.cb",
            Some("view.not-positioned-cb.borders"),
            "view.anchor1",
            "view.target",
            (58.0, 14.0, 31.0, 31.0),
        ),
        (
            "view.cb",
            Some("view.not-positioned-cb.paddings"),
            "view.anchor1",
            "view.target",
            (58.0, 14.0, 31.0, 31.0),
        ),
        (
            "view.cb",
            None,
            "view.anchor1.margins",
            "view.target",
            (50.0, 14.0, 31.0, 31.0),
        ),
        (
            "view.cb",
            None,
            "view.anchor1.borders",
            "view.target",
            (50.0, 9.0, 45.0, 43.0),
        ),
        (
            "view.cb",
            None,
            "view.anchor1.paddings",
            "view.target",
            (50.0, 9.0, 45.0, 43.0),
        ),
        (
            "view.cb",
            None,
            "view.anchor1",
            "view.target.margins",
            (58.0, 14.0, 17.0, 19.0),
        ),
        (
            "view.cb",
            None,
            "view.anchor1",
            "view.target.borders",
            (50.0, 9.0, 31.0, 31.0),
        ),
    ];
    let mut built = Vec::new();
    for (cb, nested, anchor, target, expected) in cases {
        let cb = page.el(root, cb, "");
        page.el(cb, "view.spacer", "");
        let parent = nested.map_or(cb, |nested| page.el(cb, nested, ""));
        // `.margins` on the anchor would reset its 50px left margin.
        let inline = if anchor.contains("margins") {
            "margin-left: 50px"
        } else {
            ""
        };
        page.el(parent, anchor, inline);
        let target = page.el(cb, target, "");
        built.push((cb, target, expected));
    }
    page.layout();
    for (index, (cb, target, expected)) in built.into_iter().enumerate() {
        assert_eq!(page.offset(target, cb), expected, "case {index}");
    }
}

/// wpt `anchor-position-circular.html`: two boxes anchored to each other.
/// The first reads the second, which is absolutely positioned after it and
/// so not acceptable; the second reads the first. Neither loops.
#[test]
fn wpt_anchor_position_circular() {
    let mut page = Page::new(
        "view.box { width: 100px; height: 100px; }
         #anchored1 { position: absolute; position-anchor: --a1; left: anchor(left);
                      top: anchor(bottom); anchor-name: --a2; }
         #anchored2 { position: absolute; position-anchor: --a2; left: anchor(left);
                      top: anchor(bottom); anchor-name: --a1; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
    let first = page.el(cb, "view.box#anchored1", "");
    let second = page.el(cb, "view.box#anchored2", "");
    page.layout();
    assert_eq!(page.offset(first, cb), (0.0, 0.0, 100.0, 100.0));
    assert_eq!(page.offset(second, cb), (0.0, 100.0, 100.0, 100.0));
}

/// wpt `mixed-dependency-chain.html`: ten absolutely positioned boxes all
/// named `--box`, each placed after the one before it by `position-area`
/// or by `anchor()` — the last acceptable `--box` is always the previous
/// sibling.
#[test]
fn wpt_mixed_dependency_chain() {
    let mut page = Page::new(
        ".containing-block { border: 1px solid black; width: 700px; height: 500px; }
         .box { width: 50px; height: 50px; position: absolute; anchor-name: --box; }
         #box1 { top: 0px; left: 0px; }
         .uses-anchor-function { position-anchor: --box; left: calc(anchor(right) + 10px); }
         .uses-position-area { position-anchor: --box; position-area: right center;
                               margin-left: 10px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb.containing-block", "");
    let kinds = [
        "view.box#box1",
        "view.box.uses-position-area",
        "view.box.uses-position-area",
        "view.box.uses-anchor-function",
        "view.box.uses-anchor-function",
        "view.box.uses-position-area",
        "view.box.uses-anchor-function",
        "view.box.uses-position-area",
        "view.box.uses-anchor-function",
        "view.box.uses-position-area",
    ];
    let boxes: Vec<NodeId> = kinds.iter().map(|kind| page.el(cb, kind, "")).collect();
    page.layout();
    for (index, id) in boxes.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let expected = 60.0 * index as f32;
        assert_eq!(page.offset(id, cb).0, expected, "box {}", index + 1);
    }
}

// ---------------------------------------------------------------------------
// position-anchor and position-area.

/// `position-anchor` names the default anchor; `position-area` places the
/// box in the 3×3 grid around it (wpt `position-area-basic.html`'s
/// `top center` and `bottom right` cells, on a 100×100 anchor at 100,100).
#[test]
fn position_area_places_against_the_default_anchor() {
    let mut page = Page::new(
        "#anchor { anchor-name: --a; width: 100px; height: 100px;
                   margin-left: 100px; margin-top: 100px; }
         .anchored { position: absolute; position-anchor: --a; width: 20px; height: 20px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
    page.el(cb, "view#anchor", "");
    let above = page.el(cb, "view.anchored", "position-area: top center");
    let below_right = page.el(cb, "view.anchored", "position-area: bottom right");
    let no_anchor = page.el(
        cb,
        "view.anchored",
        "position-anchor: --missing; position-area: bottom right",
    );
    page.layout();
    // Centered over the anchor, ending at its top edge.
    assert_eq!(page.offset(above, cb), (140.0, 80.0, 20.0, 20.0));
    // The bottom-right cell starts at the anchor's corner.
    assert_eq!(page.offset(below_right, cb), (200.0, 200.0, 20.0, 20.0));
    // No default anchor: `position-area` does nothing.
    assert_eq!(page.offset(no_anchor, cb), (0.0, 0.0, 20.0, 20.0));
}

/// wpt `anchored-c-v-hidden.html`: an anchored box that skips its own
/// contents is still laid out against its anchor. Adaptation: pixel
/// reftest → geometry, in a positioned container.
#[test]
fn wpt_anchored_c_v_hidden() {
    let mut page = Page::new(
        "#anchor { anchor-name: --a; width: 100px; height: 100px; }
         #anchored { position: absolute; position-anchor: --a; position-area: center;
                     content-visibility: hidden; width: 100px; height: 100px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
    page.el(cb, "view#anchor", "");
    let anchored = page.el(cb, "view#anchored", "");
    page.el(anchored, "view", "height: 20px");
    page.layout();
    assert_eq!(page.offset(anchored, cb), (0.0, 0.0, 100.0, 100.0));
}

// ---------------------------------------------------------------------------
// anchor-scope.

/// wpt `anchor-scope-basic.html`, the cases with one query box: a scoped
/// name is visible only inside the scope, from both sides. Each case is the
/// file's template inside the 100×100 `main`; anchors are 10px tall, so
/// the expected `top` is 10 × the anchor's position in the column.
#[test]
fn wpt_anchor_scope_basic() {
    let css = ".scope-all { anchor-scope: all; }
         .scope-a { anchor-scope: --a; }
         .scope-ab { anchor-scope: --a, --b; }
         .anchor-a { anchor-name: --a; }
         .anchor-b { anchor-name: --b; }
         .anchor-a, .anchor-b { height: 10px; }
         .anchored-a { position-anchor: --a; }
         .anchored-b { position-anchor: --b; }
         .anchored-a, .anchored-b { position: absolute; top: anchor(bottom); left: anchor(left);
                                    width: 5px; height: 5px; }
         main { position: relative; width: 100px; height: 100px; }";

    // "Can anchor to a name both defined and scoped by the same element":
    // the anchor is the query box's parent here, which is not acceptable
    // for a box whose containing block is `main`, so `main` holds the scope.
    {
        let mut page = Page::new(css);
        let root = page.root();
        let main = page.el(
            root,
            "view",
            "position: relative; width: 100px; height: 100px",
        );
        let scoped = page.el(main, "view.scope-a", "");
        page.el(scoped, "view.anchor-a", "");
        let anchored = page.el(scoped, "view.anchored-a", "");
        page.layout();
        assert_eq!(page.offset(anchored, main).1, 10.0, "inclusive subtree");
    }
    // "Sibling can not anchor into anchor-scope, even when anchor-name
    // present".
    {
        let mut page = Page::new(css);
        let root = page.root();
        let main = page.el(
            root,
            "view",
            "position: relative; width: 100px; height: 100px",
        );
        for _ in 0..3 {
            page.el(main, "view.anchor-a", "");
        }
        page.el(main, "view.scope-a.anchor-a", "");
        let anchored = page.el(main, "view.anchored-a", "");
        page.layout();
        assert_eq!(page.offset(anchored, main).1, 30.0, "sibling scope");
    }
    // "anchor-scope:all on common ancestor" and "--a on common ancestor".
    for scope in ["view.scope-all", "view.scope-a"] {
        let mut page = Page::new(css);
        let root = page.root();
        let main = page.el(
            root,
            "view",
            "position: relative; width: 100px; height: 100px",
        );
        let scoped = page.el(main, scope, "");
        for _ in 0..4 {
            page.el(scoped, "view.anchor-a", "");
        }
        let anchored = page.el(scoped, "view.anchored-a", "");
        page.layout();
        assert_eq!(
            page.offset(anchored, main).1,
            40.0,
            "{scope} on common ancestor"
        );
    }
    // "anchor-scope:all on sibling", "scopes multiple names", "--a,--b".
    for scope in ["view.scope-all", "view.scope-ab"] {
        let mut page = Page::new(css);
        let root = page.root();
        let main = page.el(
            root,
            "view",
            "position: relative; width: 100px; height: 100px",
        );
        page.el(main, "view.anchor-b", "");
        page.el(main, "view.anchor-a", "");
        let scoped = page.el(main, scope, "");
        page.el(scoped, "view.anchor-b", "");
        page.el(scoped, "view.anchor-a", "");
        let a = page.el(main, "view.anchored-a", "");
        let b = page.el(main, "view.anchored-b", "");
        page.layout();
        assert_eq!(page.offset(a, main).1, 20.0, "{scope}: --a");
        assert_eq!(page.offset(b, main).1, 10.0, "{scope}: --b");
    }
}

// ---------------------------------------------------------------------------
// Tree-scoped names.

/// wpt `anchor-name-shadow-higher-tree.html`: a positioned box inside a
/// shadow tree matches a name declared by the host's (document) styles.
#[test]
fn wpt_anchor_name_shadow_higher_tree() {
    let mut page = Page::new(
        "#host { anchor-name: --higher-anchor; height: 100px; width: 200px; margin: 50px; }",
    );
    let root = page.root();
    let host = page.el(root, "view#host", "");
    let shadow = page.doc.dom.attach_shadow(host, ShadowRootMode::Open);
    page.doc.dom.add_shadow_stylesheet(
        shadow,
        "view { display: flex; }
         #anchored { height: 50px; width: 50px; position: absolute;
                     top: anchor(--higher-anchor top, 37px);
                     left: anchor(--higher-anchor right, 37px); }",
    );
    let anchored = page.doc.dom.create_element("view", ());
    page.doc.dom.set_id_attribute(anchored, Some("anchored"));
    page.doc.dom.append_child(shadow, anchored);
    page.layout();
    let host_rect = page.abs(host);
    assert_eq!(page.abs(anchored).0, host_rect.0 + host_rect.2);
}

/// wpt `anchor-name-shadow-lower-tree.html`: a name declared by a shadow
/// tree's styles is invisible to a reference from the document tree.
#[test]
fn wpt_anchor_name_shadow_lower_tree() {
    let mut page = Page::new(
        "#host { width: 200px; height: 100px; margin: 50px; }
         #anchored { position: absolute; left: anchor(--lower-anchor right, 37px);
                     top: anchor(--lower-anchor top, 37px); width: 50px; height: 50px; }",
    );
    let root = page.root();
    let host = page.el(root, "view#host", "");
    let shadow = page.doc.dom.attach_shadow(host, ShadowRootMode::Open);
    page.doc.dom.add_shadow_stylesheet(
        shadow,
        "view { display: flex; }
         #inner-anchor { anchor-name: --lower-anchor; width: 100px; height: 50px; }",
    );
    let inner = page.doc.dom.create_element("view", ());
    page.doc.dom.set_id_attribute(inner, Some("inner-anchor"));
    page.doc.dom.append_child(shadow, inner);
    let anchored = page.el(root, "view#anchored", "");
    page.layout();
    assert_eq!(page.abs(anchored).0, 37.0);
}

/// wpt `anchor-name-in-shadow.html`, first case: a name a shadow tree's
/// styles declare does not leak out of it.
///
/// Its second case ("`anchor()` in shadow tree should not match host
/// anchor-name") expects a reference from a shadow tree *not* to match a
/// name the host's document-tree styles declare, which contradicts the
/// Editor's Draft's loosely matched names and
/// `anchor-name-shadow-higher-tree.html` (above); this engine follows the
/// ED, so that case is not ported.
#[test]
fn wpt_anchor_name_in_shadow() {
    let mut page = Page::new(
        "#anchor { anchor-name: --anchor; }
         #filler { height: 100px; }
         #anchored { position: absolute; top: anchor(--anchor top); }",
    );
    let root = page.root();
    let host = page.el(root, "view#host", "");
    page.el(root, "view#filler", "");
    page.el(root, "view#anchor", "");
    let anchored = page.el(root, "view#anchored", "");
    let shadow = page.doc.dom.attach_shadow(host, ShadowRootMode::Open);
    page.doc
        .dom
        .add_shadow_stylesheet(shadow, "view { display: flex; anchor-name: --anchor; }");
    let inner = page.doc.dom.create_element("view", ());
    page.doc.dom.append_child(shadow, inner);
    page.layout();
    assert_eq!(page.abs(anchored).1, 100.0);
}

// ---------------------------------------------------------------------------
// Position options (§6) and the last successful one (§6.5.1).

/// wpt `last-successful-basic.html`: the option chosen at a rendering
/// update is where the next determination starts, and it is kept while
/// nothing fits.
#[test]
fn wpt_last_successful_basic() {
    let mut page = Page::new(
        "#container { width: 400px; height: 400px; }
         #anchor { position: relative; top: 100px; left: 100px; width: 100px;
                   height: 100px; anchor-name: --a; }
         #anchored { position-anchor: --a; position-try-fallbacks: flip-block;
                     position: absolute; width: 100px; height: 200px;
                     position-area: top center; }",
    );
    let root = page.root();
    let container = page.el(root, "view.cb#container", "");
    let anchor = page.el(container, "view#anchor", "");
    let anchored = page.el(container, "view#anchored", "");
    page.render();
    assert_eq!(
        page.offset(anchored, container).1,
        200.0,
        "starts with flip-block"
    );

    page.doc.set_inline(anchor, "top: 150px");
    page.render();
    // flip-block's region is 150px tall below the anchor; the 200px box
    // overflows it and is shifted back inside the containing block.
    assert_eq!(
        page.offset(anchored, container).1,
        200.0,
        "no fit: keep flip-block"
    );

    page.doc.set_inline(anchor, "top: 200px");
    page.render();
    assert_eq!(
        page.offset(anchored, container).1,
        0.0,
        "the base fits again"
    );
}

/// wpt `position-try-fallbacks-no-fit-after-fit.html`: without a rendering
/// update in between, a second layout starts from the base style again.
#[test]
fn wpt_position_try_fallbacks_no_fit_after_fit() {
    let mut page = Page::new(
        "#container { width: 400px; height: 300px; }
         #anchor { anchor-name: --a; position: absolute; bottom: 0px; left: 0px;
                   width: 20px; height: 20px; }
         #anchored { position: absolute; position-anchor: --a; top: anchor(outside);
                     left: 0px; position-try-fallbacks: flip-block;
                     width: 390px; height: 100px; }",
    );
    let root = page.root();
    let container = page.el(root, "view.cb#container", "");
    page.el(container, "view#anchor", "");
    let anchored = page.el(container, "view#anchored", "");
    page.layout();
    assert_eq!(page.offset(anchored, container).1, 180.0, "flip-block fits");

    page.doc.set_inline(container, "width: 380px");
    page.layout();
    assert_eq!(
        page.offset(anchored, container).1,
        300.0,
        "nothing fits: base"
    );
    page.render();
    page.render();
    assert_eq!(
        page.offset(anchored, container).1,
        300.0,
        "base after rendering"
    );
}

/// wpt `last-successful-change-try-rule.html`: a changed `@position-try`
/// rule is a fallback-sensitive change, so the next rendering update
/// forgets the last successful option. Adaptation: the rule is changed by
/// a later sheet declaring the same name, not through CSSOM.
#[test]
fn wpt_last_successful_change_try_rule() {
    let mut page = Page::new(
        "#container { width: 400px; height: 400px; }
         #anchor { position: relative; top: 100px; left: 100px; width: 100px;
                   height: 100px; anchor-name: --a; }
         #anchored { position-anchor: --a; position-try-fallbacks: --try;
                     position: absolute; width: 100px; height: 200px;
                     position-area: top center; }
         @position-try --try { position-area: bottom center; }",
    );
    let root = page.root();
    let container = page.el(root, "view.cb#container", "");
    let anchor = page.el(container, "view#anchor", "");
    let anchored = page.el(container, "view#anchored", "");
    page.render();
    assert_eq!(
        page.offset(anchored, container).1,
        200.0,
        "starts with --try"
    );

    page.doc.set_inline(anchor, "top: 150px");
    page.render();
    assert_eq!(
        page.offset(anchored, container).1,
        200.0,
        "no fit: keep --try"
    );

    page.doc
        .add_css("@position-try --try { position-area: bottom; }");
    page.render();
    page.render();
    assert_eq!(
        page.offset(anchored, container).1,
        0.0,
        "invalidated by the rule"
    );
}

/// §6.5: the chosen option's accepted properties are what computed-value
/// readback reports; the others stay the base style's.
#[test]
fn readback_reports_the_chosen_options_accepted_properties() {
    let mut page = Page::new(
        "#container { width: 400px; height: 300px; }
         #anchor { anchor-name: --a; position: absolute; bottom: 0px; left: 0px;
                   width: 20px; height: 20px; }
         #anchored { position: absolute; position-anchor: --a; top: anchor(outside);
                     left: 0px; position-try-fallbacks: --up; width: 390px; height: 100px;
                     opacity: 0.5; }
         @position-try --up { top: auto; bottom: anchor(outside); margin-left: 3px; }",
    );
    let root = page.root();
    let container = page.el(root, "view.cb#container", "");
    page.el(container, "view#anchor", "");
    let anchored = page.el(container, "view#anchored", "");
    page.layout();
    assert_eq!(page.offset(anchored, container).1, 180.0);
    assert_eq!(page.computed(anchored, "top"), "auto");
    assert_eq!(page.computed(anchored, "bottom"), "anchor(outside)");
    assert_eq!(page.computed(anchored, "margin-left"), "3px");
    assert_eq!(page.computed(anchored, "opacity"), "0.5");
}

// ---------------------------------------------------------------------------
// The scrollable containing block (css-position-4).

/// wpt `scrollable-containing-block-validity.html`: only a box with a
/// default anchor is laid out against its scroll container's scrollable
/// overflow area; a `position-anchor` naming nothing, and a `position-area`
/// with no anchor, keep the padding box. Adaptation: this engine's
/// scrolling area (`ScrollBox::scroll_size`) ends at the content's far
/// edge rather than past the scroller's end padding, so the scrollable
/// containing block is 190px, not the file's 200px.
#[test]
fn wpt_scrollable_containing_block_validity() {
    let mut page = Page::new(
        ".scroller { overflow: hidden; width: 80px; height: 80px; margin: 10px;
                     border: 3px solid black; padding: 10px; }
         .filler { min-width: 180px; min-height: 180px; }
         .anchor { anchor-name: --a; }
         .target { position: absolute; top: 0px; left: 0px; right: 0px; bottom: 0px;
                   justify-self: stretch; align-self: stretch; }",
    );
    let root = page.root();
    let mut targets = Vec::new();
    for (inline, expected) in [
        ("", (100.0, 100.0)),
        ("position-anchor: --a", (190.0, 190.0)),
        ("position-anchor: --b", (100.0, 100.0)),
        ("position-area: top", (100.0, 100.0)),
    ] {
        let scroller = page.el(root, "view.cb.scroller", "");
        let filler = page.el(scroller, "view.filler", "");
        page.el(filler, "view.anchor", "");
        targets.push((page.el(scroller, "view.target", inline), expected, inline));
    }
    page.layout();
    for (target, expected, inline) in targets {
        let (.., width, height) = page.abs(target);
        assert_eq!((width, height), expected, "{inline:?}");
    }
}

// ---------------------------------------------------------------------------
// anchor() sides, through the host.

/// wpt `anchor-inside-outside.html`: `inside`/`outside` on each physical
/// inset and each inline-axis logical one. Not ported: the four
/// `inset-block-*` cases — those longhands are not author-facing in the
/// fork's `lynx` build (without `writing-mode` the block axis is always
/// vertical), so the declarations are dropped.
#[test]
fn wpt_anchor_inside_outside() {
    let mut page = Page::new(
        "#cb { width: 400px; height: 400px; border: 1px solid black; }
         #anchor { position: absolute; top: 250px; left: 150px; width: 50px; height: 50px;
                   anchor-name: --a; }
         .target { position: absolute; position-anchor: --a; width: 10px; height: 10px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb#cb", "");
    page.el(cb, "view#anchor", "");
    let cases: [(&str, Option<f32>, Option<f32>); 12] = [
        ("left:anchor(inside)", Some(150.0), None),
        ("left:anchor(outside)", Some(200.0), None),
        ("right:anchor(inside)", Some(190.0), None),
        ("right:anchor(outside)", Some(140.0), None),
        ("top:anchor(inside)", None, Some(250.0)),
        ("top:anchor(outside)", None, Some(300.0)),
        ("bottom:anchor(inside)", None, Some(290.0)),
        ("bottom:anchor(outside)", None, Some(240.0)),
        ("inset-inline-start:anchor(inside)", Some(150.0), None),
        ("inset-inline-start:anchor(outside)", Some(200.0), None),
        ("inset-inline-end:anchor(inside)", Some(190.0), None),
        ("inset-inline-end:anchor(outside)", Some(140.0), None),
    ];
    let targets: Vec<_> = cases
        .iter()
        .map(|&(inline, x, y)| (page.el(cb, "view.target", inline), inline, x, y))
        .collect();
    page.layout();
    for (target, inline, x, y) in targets {
        let (left, top, ..) = page.offset(target, cb);
        if let Some(x) = x {
            assert_eq!(left, x, "{inline}");
        }
        if let Some(y) = y {
            assert_eq!(top, y, "{inline}");
        }
    }
}

// ---------------------------------------------------------------------------
// Position options through the fork's cascade.

/// wpt `try-tactic-basic.html`, every case: a named `@position-try` rule
/// under each try tactic, cascaded at the style harvest.
#[test]
fn wpt_try_tactic_basic() {
    let cases: [(&str, Rect4); 30] = [
        ("", (10.0, 20.0, 30.0, 40.0)),
        ("flip-block", (10.0, 340.0, 30.0, 40.0)),
        ("flip-y", (10.0, 340.0, 30.0, 40.0)),
        ("flip-inline", (360.0, 20.0, 30.0, 40.0)),
        ("flip-x", (360.0, 20.0, 30.0, 40.0)),
        ("flip-block flip-inline", (360.0, 340.0, 30.0, 40.0)),
        ("flip-inline flip-block", (360.0, 340.0, 30.0, 40.0)),
        ("flip-y flip-x", (360.0, 340.0, 30.0, 40.0)),
        ("flip-x flip-y", (360.0, 340.0, 30.0, 40.0)),
        ("flip-start", (20.0, 10.0, 40.0, 30.0)),
        (
            "flip-block flip-start flip-inline",
            (20.0, 10.0, 40.0, 30.0),
        ),
        (
            "flip-inline flip-start flip-block",
            (20.0, 10.0, 40.0, 30.0),
        ),
        ("flip-y flip-start flip-x", (20.0, 10.0, 40.0, 30.0)),
        ("flip-x flip-start flip-y", (20.0, 10.0, 40.0, 30.0)),
        ("flip-start flip-block", (20.0, 360.0, 40.0, 30.0)),
        ("flip-inline flip-start", (20.0, 360.0, 40.0, 30.0)),
        ("flip-start flip-y", (20.0, 360.0, 40.0, 30.0)),
        ("flip-x flip-start", (20.0, 360.0, 40.0, 30.0)),
        ("flip-start flip-inline", (340.0, 10.0, 40.0, 30.0)),
        ("flip-block flip-start", (340.0, 10.0, 40.0, 30.0)),
        ("flip-start flip-x", (340.0, 10.0, 40.0, 30.0)),
        ("flip-y flip-start", (340.0, 10.0, 40.0, 30.0)),
        (
            "flip-start flip-block flip-inline",
            (340.0, 360.0, 40.0, 30.0),
        ),
        (
            "flip-start flip-inline flip-block",
            (340.0, 360.0, 40.0, 30.0),
        ),
        (
            "flip-inline flip-block flip-start",
            (340.0, 360.0, 40.0, 30.0),
        ),
        (
            "flip-block flip-inline flip-start",
            (340.0, 360.0, 40.0, 30.0),
        ),
        ("flip-start flip-y flip-x", (340.0, 360.0, 40.0, 30.0)),
        ("flip-start flip-x flip-y", (340.0, 360.0, 40.0, 30.0)),
        ("flip-x flip-y flip-start", (340.0, 360.0, 40.0, 30.0)),
        ("flip-y flip-x flip-start", (340.0, 360.0, 40.0, 30.0)),
    ];
    let mut page = Page::new(
        "@position-try --pf { left: 10px; top: 20px; }
         #cb { position: absolute; width: 400px; height: 400px; border: 1px solid black; }
         #target { position: absolute; left: 99999px; width: 30px; height: 40px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view#cb", "");
    let target = page.el(cb, "view#target", "");
    for (tactic, expected) in cases {
        page.doc
            .set_inline(target, &format!("position-try-fallbacks: --pf {tactic}"));
        page.layout();
        assert_eq!(page.offset(target, cb), expected, "{tactic:?}");
    }
}

/// wpt `position-try-order-basic.html`, every case: `position-try-order`
/// sorts the options by their inset-modified containing block, stably.
/// Adaptation: the `#target`/`#ref` pair becomes one box laid out under
/// each `position-try` value in turn.
#[test]
fn wpt_position_try_order_basic() {
    let mut page = Page::new(
        "#cb { position: absolute; width: 400px; height: 400px; border: 1px solid black; }
         #anchor { position: absolute; left: 150px; top: 200px; width: 150px; height: 150px;
                   anchor-name: --a; }
         #target { position: absolute; left: 450px; width: 40px; height: 40px;
                   position-anchor: --a; }
         @position-try --right { inset: unset; left: anchor(right); }
         @position-try --left { inset: unset; right: anchor(left); }
         @position-try --top { inset: unset; bottom: anchor(top); }
         @position-try --bottom { inset: unset; top: anchor(bottom); }
         @position-try --right-sweep { inset: unset; top: anchor(top); bottom: anchor(bottom);
                                       left: anchor(right); align-self: center; }
         @position-try --left-sweep { inset: unset; top: anchor(top); bottom: anchor(bottom);
                                      right: anchor(left); align-self: center; }
         @position-try --bottom-sweep { left: anchor(left); right: anchor(right);
                                        top: anchor(bottom); justify-self: center; }
         @position-try --top-sweep { left: anchor(left); right: anchor(right);
                                     bottom: anchor(top); justify-self: center; }",
    );
    let root = page.root();
    let cb = page.el(root, "view#cb", "");
    page.el(cb, "view#anchor", "");
    let target = page.el(cb, "view#target", "");
    let at = |page: &mut Page, position_try: &str| {
        page.doc
            .set_inline(target, &format!("position-try: {position_try}"));
        page.layout();
        let (x, y, ..) = page.offset(target, cb);
        (x, y)
    };
    let cases = [
        ("--right", "--right"),
        ("--left", "--left"),
        ("--top", "--top"),
        ("--bottom", "--bottom"),
        ("--right, --left, --bottom, --top", "--right"),
        ("normal --right, --left, --bottom, --top", "--right"),
        ("normal --top, --left, --bottom, --right", "--top"),
        ("most-block-size --right, --left", "--right"),
        ("most-height --right, --left", "--right"),
        ("most-inline-size --right, --left", "--left"),
        ("most-width --right, --left", "--left"),
        ("most-inline-size --bottom, --top", "--bottom"),
        ("most-width --bottom, --top", "--bottom"),
        ("most-block-size --bottom, --top", "--top"),
        ("most-height --bottom, --top", "--top"),
        (
            "most-inline-size --right, --left, --bottom, --top",
            "--bottom",
        ),
        ("most-inline-size --right, --left, --top, --bottom", "--top"),
        (
            "most-block-size --bottom, --top, --right, --left",
            "--right",
        ),
        ("most-block-size --bottom, --top, --left, --right", "--left"),
        (
            "most-inline-size --left-sweep, --bottom-sweep",
            "--left-sweep",
        ),
        (
            "most-inline-size --bottom-sweep, --left-sweep",
            "--bottom-sweep",
        ),
        (
            "most-block-size --left-sweep, --bottom-sweep",
            "--left-sweep",
        ),
        (
            "most-block-size --bottom-sweep, --left-sweep",
            "--left-sweep",
        ),
        (
            "most-inline-size --right-sweep, --left-sweep, --bottom-sweep, --top-sweep",
            "--left-sweep",
        ),
        (
            "most-block-size --right-sweep, --left-sweep, --bottom-sweep, --top-sweep",
            "--top-sweep",
        ),
        (
            "most-inline-size --right-sweep, --left-sweep, --bottom-sweep, --top-sweep, --bottom",
            "--bottom",
        ),
        (
            "most-block-size --right-sweep, --left-sweep, --bottom-sweep, --top-sweep, --right",
            "--right",
        ),
    ];
    for (position_try, expected) in cases {
        let got = at(&mut page, position_try);
        let want = at(&mut page, expected);
        assert_eq!(got, want, "{position_try} | {expected}");
    }
}

// ---------------------------------------------------------------------------
// position-anchor: match-parent.

/// wpt `position-anchor-match-parent-rendering.html`, every case without a
/// pseudo-element: a `fixed` probe matching its parent's default anchor
/// sits at the anchor's bottom-right corner `(70, 40)`, else at its
/// fallback `(20, 70)`. Adaptation: the file's `fixed` probes and parents
/// resolve against the viewport, where every probe would see every case's
/// `--a` (and the last one would win) and fall back to one viewport point,
/// while its reference draws each probe inside its own case; each `.case`
/// here is the fixed containing block (`transform`) and scopes `--a`
/// (`anchor-scope`), which is what the reference draws. The "Parent is not
/// positioned" case is not ported: the fork's `lynx` grammar has no
/// `position: static` (Lynx's own `position` values), and its
/// relatively positioned sibling case covers the same rule. Reftest →
/// geometry.
#[test]
fn wpt_position_anchor_match_parent_rendering() {
    let mut page = Page::new(
        ".cases { flex-direction: row; flex-wrap: wrap; width: 642px; }
         .case { position: relative; width: 122px; height: 102px; margin: 4px;
                 transform: translateX(0px); anchor-scope: --a; }
         .anchor { position: absolute; anchor-name: --a; left: 20px; top: 20px;
                   width: 50px; height: 20px; }
         .parent { position: fixed; top: 0px; left: 0px; right: 0px; bottom: 0px; }
         .probe { position: fixed; position-anchor: match-parent;
                  left: anchor(right, 20px); top: anchor(bottom, 70px);
                  width: 20px; height: 20px; }",
    );
    let root = page.root();
    let cases = page.el(root, "view.cases", "");
    // (parent chain as (classes, inline style), probe inline, anchored?)
    let specs: Vec<MatchParentCase> = vec![
        // Parent has a default anchor element.
        (vec![("view.parent", "position-anchor: --a")], "", true),
        // Parent has `position-anchor: none`.
        (vec![("view.parent", "position-anchor: none")], "", false),
        // Parent's `position-anchor` selects no anchor element.
        (
            vec![("view.parent", "position-anchor: --nonexistent")],
            "",
            false,
        ),
        // Parent is relatively positioned, so `position-anchor` does not apply.
        (
            vec![("view.parent", "position: relative; position-anchor: --a")],
            "",
            false,
        ),
        // Parent also uses `match-parent`.
        (
            vec![
                ("view.parent", "position-anchor: --a"),
                ("view.parent", "position-anchor: match-parent"),
            ],
            "",
            true,
        ),
        // Chain of `match-parent` parents, several levels deep.
        (
            vec![
                ("view.parent", "position-anchor: --a"),
                ("view.parent", "position-anchor: match-parent"),
                ("view.parent", "position-anchor: match-parent"),
                ("view.parent", "position-anchor: match-parent"),
            ],
            "",
            true,
        ),
        // Parent's `match-parent` resolved to no anchor element.
        (
            vec![
                ("view.parent", "position-anchor: none"),
                ("view.parent", "position-anchor: match-parent"),
            ],
            "",
            false,
        ),
        // Only the grandparent has a default anchor element.
        (
            vec![("view.parent", "position-anchor: --a"), ("view.parent", "")],
            "",
            false,
        ),
        // Anchored ancestor, with plain wrappers in between.
        (
            vec![
                ("view.parent", "position-anchor: --a"),
                ("view", ""),
                ("view", ""),
            ],
            "",
            false,
        ),
        // Intermediate `match-parent` is not absolutely positioned.
        (
            vec![
                ("view.parent", "position-anchor: --a"),
                ("view", "position-anchor: match-parent"),
            ],
            "",
            false,
        ),
        // Parent's default anchor element is not acceptable for the probe,
        // whose containing block is the parent itself.
        (
            vec![("view.parent", "position-anchor: --a")],
            "position: absolute",
            false,
        ),
    ];
    let mut probes = Vec::new();
    for (index, (parents, probe_inline, anchored)) in specs.into_iter().enumerate() {
        let case = page.el(cases, "view.case", "");
        page.el(case, "view.anchor", "");
        let mut parent = case;
        for (spec, inline) in parents {
            parent = page.el(parent, spec, inline);
        }
        let probe = page.el(parent, "view.probe", probe_inline);
        probes.push((index, case, probe, anchored));
    }
    page.layout();
    for (index, case, probe, anchored) in probes {
        let (x, y, ..) = page.offset(probe, case);
        let expected = if anchored { (70.0, 40.0) } else { (20.0, 70.0) };
        assert_eq!((x, y), expected, "case {index}");
    }
}

// ---------------------------------------------------------------------------
// Dynamic changes and the settle loop.

/// wpt `anchor-position-dynamic-001.html`: the anchors' geometry changes
/// after a first layout, and the query box follows. Adaptation: the top
/// margins do not collapse (see `wpt_anchor_position_001`).
#[test]
fn wpt_anchor_position_dynamic_001() {
    let mut page = Page::new(
        "#a1 { anchor-name: --a1; margin-left: 100px; margin-top: 100px; width: 50px;
               height: 50px; }
         .after #a1 { width: 100px; height: 100px; }
         #a2 { anchor-name: --a2; margin-left: 250px; margin-top: 350px; width: 100px;
               height: 100px; }
         .after #a2 { margin-left: 500px; margin-top: 100px; }
         #target { position: absolute; left: anchor(--a1 right); top: anchor(--a1 bottom);
                   right: anchor(--a2 left); bottom: anchor(--a2 top); }",
    );
    let root = page.root();
    let container = page.el(root, "view.cb#container", "");
    page.el(container, "view#a1", "");
    page.el(container, "view#a2", "");
    let target = page.el(container, "view#target", "");
    page.layout();
    page.doc.add_class(container, "after");
    page.layout();
    assert_eq!(page.offset(target, container), (200.0, 200.0, 300.0, 100.0));
}

/// wpt `position-anchor-003.html`: changing `position-anchor` relays the
/// box out against the new default anchor. Adaptation: the anchors live in
/// a positioned container, and the `fixed` box resolves against the
/// viewport as the file's does.
#[test]
fn wpt_position_anchor_003() {
    let mut page = Page::new(
        "#target { position: fixed; width: 100px; height: 100px; top: anchor(top);
                   left: anchor(right); position-anchor: --a; }
         #target.after { position-anchor: --b; }
         #anchor1, #anchor2 { width: 100px; height: 100px; }
         #anchor1 { anchor-name: --a; }
         #anchor2 { margin-left: 100px; anchor-name: --b; }",
    );
    let root = page.root();
    page.el(root, "view#anchor1", "");
    page.el(root, "view#anchor2", "");
    let target = page.el(root, "view#target", "");
    page.layout();
    assert_eq!(page.abs(target).0, 100.0);
    page.doc.add_class(target, "after");
    page.layout();
    assert_eq!((page.abs(target).0, page.abs(target).1), (200.0, 100.0));
}

/// wpt `anchor-scope-dynamic.html`, the first two cases: a scope appearing
/// and disappearing on an anchor's ancestor changes the target.
#[test]
fn wpt_anchor_scope_dynamic() {
    for scope in ["all", "--a"] {
        let mut page = Page::new(
            ".anchor-a { anchor-name: --a; height: 10px; }
             .anchored-a { position: absolute; position-anchor: --a; top: anchor(bottom);
                           left: anchor(left); width: 5px; height: 5px; }",
        );
        let root = page.root();
        let main = page.el(root, "view.cb", "width: 100px; height: 100px");
        page.el(main, "view.anchor-a", "");
        page.el(main, "view.anchor-a", "");
        let dynamic = page.el(main, "view", "");
        page.el(dynamic, "view.anchor-a", "");
        page.el(dynamic, "view.anchor-a", "");
        let anchored = page.el(main, "view.anchored-a", "");
        page.layout();
        assert_eq!(page.offset(anchored, main).1, 40.0, "{scope}: initially");
        page.doc
            .set_inline(dynamic, &format!("anchor-scope: {scope}"));
        page.layout();
        assert_eq!(page.offset(anchored, main).1, 20.0, "{scope}: scoped");
        page.doc.set_inline(dynamic, "");
        page.layout();
        assert_eq!(page.offset(anchored, main).1, 40.0, "{scope}: unscoped");
    }
}

/// The settle loop: an anchor inside a `contain: strict` sibling resizes.
/// The containment boundary stops the dirty walk, so the containing block
/// never runs again on its own; the settle loop re-asks the reader's query
/// and lays it out again in the same `layout()`.
#[test]
fn an_anchor_behind_a_containment_boundary_moves_its_reader() {
    let mut page = Page::new(
        ".boundary { contain: strict; width: 200px; height: 200px; }
         .anchor { anchor-name: --a; width: 40px; height: 30px; }
         .reader { position: absolute; top: anchor(--a bottom); left: 0px;
                   width: 10px; height: 10px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
    let boundary = page.el(cb, "view.boundary", "");
    let anchor = page.el(boundary, "view.anchor", "");
    let reader = page.el(cb, "view.reader", "");
    page.layout();
    assert_eq!(page.offset(reader, cb).1, 30.0);
    page.doc.set_inline(anchor, "height: 70px");
    page.layout();
    assert_eq!(page.offset(reader, cb).1, 70.0);
}

/// The settle loop, other direction: an absolutely positioned anchor that
/// escapes a static wrapper is placed by the rounding tail, after the box
/// reading it; the loop lays the reader out again against where it landed.
#[test]
fn an_escaping_anchor_placed_after_its_reader_is_read_where_it_lands() {
    let mut page = Page::new(
        ".anchor { anchor-name: --a; position: absolute; left: 50px; top: 60px;
                   width: 40px; height: 30px; }
         .reader { position: absolute; top: anchor(--a bottom); left: anchor(--a right);
                   width: 10px; height: 10px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
    let wrapper = page.el(cb, "view", "margin-top: 5px");
    page.el(wrapper, "view.anchor", "");
    let reader = page.el(cb, "view.reader", "");
    page.layout();
    assert_eq!(page.offset(reader, cb), (90.0, 90.0, 10.0, 10.0));
}

/// A removed anchor drops its reader to the fallback; a new one is picked
/// up without any change to the reader.
#[test]
fn adding_and_removing_an_anchor_rereads() {
    let mut page = Page::new(
        ".anchor { anchor-name: --a; width: 40px; height: 30px; }
         .reader { position: absolute; top: anchor(--a bottom, 7px); left: 0px;
                   width: 10px; height: 10px; }
         .deep { contain: strict; width: 100px; height: 100px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
    let deep = page.el(cb, "view.deep", "");
    let reader = page.el(cb, "view.reader", "");
    page.layout();
    assert_eq!(page.offset(reader, cb).1, 7.0);
    let anchor = page.el(deep, "view.anchor", "");
    page.layout();
    assert_eq!(page.offset(reader, cb).1, 30.0);
    page.doc.dom.remove_element(anchor);
    page.layout();
    assert_eq!(page.offset(reader, cb).1, 7.0);
}

// ---------------------------------------------------------------------------
// §2.5 anchor relevance and §3.3 remembered scroll offsets.

/// §2.5: a `content-visibility: auto` subtree far outside the viewport
/// holding the target anchor of a shown positioned box outside it is
/// relevant to the user, so its contents are laid out and the box anchors.
#[test]
fn an_auto_subtree_holding_an_anchor_stays_relevant() {
    let mut page = Page::new(
        ".spacer { height: 3000px; }
         .auto { content-visibility: auto; width: 100px; height: 100px; }
         .anchor { anchor-name: --a; width: 40px; height: 30px; }
         .reader { position: absolute; top: 0px; left: anchor(--a right, 7px);
                   width: 10px; height: 10px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
    page.el(cb, "view.spacer", "");
    let auto = page.el(cb, "view.auto", "");
    page.el(auto, "view.anchor", "");
    let reader = page.el(cb, "view.reader", "");
    page.render();
    assert_eq!(page.offset(reader, cb).0, 40.0);

    // Without the reader nothing keeps it relevant, and it skips.
    let lonely = {
        let mut page = Page::new(
            ".spacer { height: 3000px; }
             .auto { content-visibility: auto; width: 100px; height: 100px; }
             .anchor { anchor-name: --a; width: 40px; height: 30px; }",
        );
        let root = page.root();
        let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
        page.el(cb, "view.spacer", "");
        let auto = page.el(cb, "view.auto", "");
        let anchor = page.el(auto, "view.anchor", "");
        page.render();
        page.abs(anchor)
    };
    assert_eq!(
        (lonely.2, lonely.3),
        (0.0, 0.0),
        "a skipped subtree lays nothing out"
    );
}

/// §3.3: an anchor inside a scroll container is read at the scroll offset
/// remembered at the reader's last anchor recalculation point — here, when
/// it began generating a box — so scrolling alone moves nothing in layout;
/// the next recalculation point picks the new offset up.
#[test]
fn remembered_scroll_offsets_hold_until_a_recalculation_point() {
    let mut page = Page::new(
        ".scroller { overflow: scroll; width: 200px; height: 100px; }
         .filler { height: 500px; }
         .anchor { anchor-name: --a; width: 40px; height: 30px; margin-top: 50px; }
         .reader { position: absolute; top: anchor(--a bottom); left: 0px;
                   width: 10px; height: 10px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
    let scroller = page.el(cb, "view.scroller", "");
    let content = page.el(scroller, "view", "");
    page.el(content, "view.anchor", "");
    page.el(content, "view.filler", "");
    let reader = page.el(cb, "view.reader", "");
    page.layout();
    assert_eq!(page.offset(reader, cb).1, 80.0);

    page.doc
        .dom
        .scroll_to(scroller, dom::Vector2D::new(0.0, 20.0));
    // An unrelated relayout of the reader: its anchor answer is unchanged.
    page.doc.set_inline(reader, "width: 11px");
    page.layout();
    assert_eq!(page.offset(reader, cb).1, 80.0);

    // It stops and starts generating a box: a recalculation point.
    page.doc.set_inline(reader, "display: none");
    page.layout();
    page.doc.set_inline(reader, "");
    page.layout();
    assert_eq!(page.offset(reader, cb).1, 60.0);
}

/// wpt `anchor-name-cross-shadow.html`, both cases: a name set on a
/// shadow part by the document's `::part()` rule, and one set on the host
/// by the shadow tree's `:host` rule, are each matched from the tree whose
/// styles declared them.
#[test]
fn wpt_anchor_name_cross_shadow() {
    let mut page = Page::new(
        ".cb { position: absolute; }
         #host1::part(anchor) { anchor-name: --a1; margin-left: 15px; }
         #target1 { position: absolute; left: anchor(--a1 left); }",
    );
    let root = page.root();
    let cb1 = page.el(root, "view.cb", "");
    let host1 = page.el(cb1, "view#host1", "");
    let target1 = page.el(cb1, "view#target1", "");
    let cb2 = page.el(root, "view.cb", "top: 100px");
    let host2 = page.el(cb2, "view#host2", "");
    let shadow1 = page.doc.dom.attach_shadow(host1, ShadowRootMode::Open);
    page.doc
        .dom
        .add_shadow_stylesheet(shadow1, "view { display: flex; }");
    let part = page.el(shadow1, "view", "");
    page.doc.set_attr(part, "part", "anchor");
    let shadow2 = page.doc.dom.attach_shadow(host2, ShadowRootMode::Open);
    page.doc.dom.add_shadow_stylesheet(
        shadow2,
        "view { display: flex; }
         :host { anchor-name: --a2; margin-left: 15px; }
         #target2 { position: absolute; left: anchor(--a2 left); }",
    );
    let target2 = page.el(shadow2, "view#target2", "");
    page.layout();
    assert_eq!(page.offset(target1, cb1).0, 15.0, "::part()");
    assert_eq!(page.offset(target2, cb2).0, 15.0, ":host");
}

/// wpt `anchor-scope-shadow-all.html`: `anchor-scope: all` set on a shadow
/// part by the document's styles is tree-scoped to the document, so it
/// does not scope the names the shadow tree's styles declare.
#[test]
fn wpt_anchor_scope_shadow_all() {
    let mut page = Page::new("#host::part(scope) { anchor-scope: all; }");
    let root = page.root();
    let host = page.el(root, "view#host", "");
    let shadow = page.doc.dom.attach_shadow(host, ShadowRootMode::Open);
    page.doc.dom.add_shadow_stylesheet(
        shadow,
        "view { display: flex; flex-direction: column; flex-shrink: 0; }
         .anchored { position: absolute; top: anchor(bottom, 1px); position-anchor: --a;
                     width: 5px; height: 5px; }
         .anchor { height: 10px; anchor-name: --a; }
         .cb { position: relative; width: 200px; height: 200px; border: 1px solid black; }",
    );
    let cb = page.el(shadow, "view.cb", "");
    page.el(cb, "view.anchor", "");
    let scope = page.el(cb, "view", "");
    page.doc.set_attr(scope, "part", "scope");
    let anchored = page.el(scope, "view.anchored", "");
    page.layout();
    assert_eq!(page.offset(anchored, cb).1, 10.0);
}

/// wpt `anchor-scope-shadow-names.html`, every case: a scope matches only
/// names of its own tree — a document `::part()` scope, a shadow `.scope`
/// around a slotted box, a shadow `::slotted()` scope on the box itself and
/// a shadow `:host` scope all leave a document-tree lookup alone.
#[test]
fn wpt_anchor_scope_shadow_names() {
    let shadow_view = "view { display: flex; flex-direction: column; flex-shrink: 0; }";
    let light = ".anchor { height: 10px; anchor-name: --a; }
         .cb { position: relative; width: 50px; height: 50px; border: 1px solid black; }
         .anchored { position: absolute; top: anchor(bottom, 1px); position-anchor: --a;
                     width: 5px; height: 5px; }";

    // `test_part`: the scope comes from a document `::part()` rule, the
    // names and the lookup from the shadow tree.
    {
        let mut page = Page::new(".host::part(scope) { anchor-scope: --a; }");
        let root = page.root();
        let host = page.el(root, "view.host", "");
        let shadow = page.doc.dom.attach_shadow(host, ShadowRootMode::Open);
        page.doc
            .dom
            .add_shadow_stylesheet(shadow, &format!("{shadow_view}\n{light}"));
        let cb = page.el(shadow, "view.cb", "");
        page.el(cb, "view.anchor", "");
        let scope = page.el(cb, "view", "");
        page.doc.set_attr(scope, "part", "scope");
        let anchored = page.el(scope, "view.anchored", "");
        page.layout();
        assert_eq!(page.offset(anchored, cb).1, 10.0, "::part() scope");
    }
    // `test_slot`, `test_slotted`, `test_host`: a document-tree lookup from
    // a box slotted into a shadow tree whose styles declare a scope.
    for (case, shadow_css, wrap) in [
        ("slot", ".scope { anchor-scope: --a; }", true),
        (
            "::slotted()",
            "::slotted(view) { anchor-scope: --a; }",
            false,
        ),
        (":host", ":host { anchor-scope: --a; }", false),
    ] {
        let mut page = Page::new(light);
        let root = page.root();
        let cb = page.el(root, "view.cb", "");
        page.el(cb, "view.anchor", "");
        let host = page.el(cb, "view.host", "");
        let anchored = page.el(host, "view.anchored", "");
        let shadow = page.doc.dom.attach_shadow(host, ShadowRootMode::Open);
        page.doc
            .dom
            .add_shadow_stylesheet(shadow, &format!("{shadow_view}\n{shadow_css}"));
        let parent = if wrap {
            page.el(shadow, "view.scope", "")
        } else {
            shadow
        };
        page.el(parent, "slot", "");
        page.layout();
        assert_eq!(page.offset(anchored, cb).1, 10.0, "{case}");
    }
}

/// wpt `last-successful-intermediate-ignored.html`: a layout between two
/// rendering updates in which the base style fits does not replace the last
/// successful option; only a rendering update records one.
#[test]
fn wpt_last_successful_intermediate_ignored() {
    let mut page = Page::new(
        "#container { width: 400px; height: 400px; }
         #anchor { position: relative; top: 100px; left: 100px; width: 100px;
                   height: 100px; anchor-name: --a; }
         #anchored { position-anchor: --a; position-try-fallbacks: flip-block;
                     position: absolute; width: 100px; height: 200px;
                     position-area: top center; }",
    );
    let root = page.root();
    let container = page.el(root, "view.cb#container", "");
    let anchor = page.el(container, "view#anchor", "");
    let anchored = page.el(container, "view#anchored", "");
    page.render();
    assert_eq!(
        page.offset(anchored, container).1,
        200.0,
        "starts with flip-block"
    );

    page.doc.set_inline(anchor, "top: 200px");
    page.layout();
    assert_eq!(
        page.offset(anchored, container).1,
        0.0,
        "the base fits, for now"
    );
    page.doc.set_inline(anchor, "top: 150px");
    page.render();
    assert_eq!(page.offset(anchored, container).1, 200.0, "flip-block kept");
}

/// The page `position-anchor-001.html` and `-002.html` share: two 100×100
/// anchors, and a `fixed` box per anchor forced into an `@position-try`
/// option that places and sizes it from its default anchor.
const POSITION_ANCHOR_PAGE: &str = ".anchor { width: 100px; height: 100px; }
     .target { position: fixed; position-try-fallbacks: --pf; left: 999999px; }
     @position-try --pf { top: anchor(bottom, 0px); left: anchor(left, 0px);
                          width: anchor-size(width, 0px); height: anchor-size(height, 0px); }";

/// wpt `position-anchor-001.html`: `position-anchor` picks each box's
/// default anchor, which its fallback option's unnamed functions read.
/// Adaptation: reftest → geometry; the Ahem text is left out.
#[test]
fn wpt_position_anchor_001() {
    let mut page = Page::new(&format!(
        "{POSITION_ANCHOR_PAGE}
         #anchor1 {{ anchor-name: --a1; margin-left: 100px; }}
         #target1 {{ position-anchor: --a1; }}
         #anchor2 {{ anchor-name: --a2; margin-left: 300px; margin-top: 100px; }}
         #target2 {{ position-anchor: --a2; }}"
    ));
    let root = page.root();
    page.el(root, "view.anchor#anchor1", "");
    page.el(root, "view.anchor#anchor2", "");
    let target1 = page.el(root, "view.target#target1", "");
    let target2 = page.el(root, "view.target#target2", "");
    page.layout();
    assert_eq!(page.abs(target1), (100.0, 100.0, 100.0, 100.0));
    assert_eq!(page.abs(target2), (300.0, 300.0, 100.0, 100.0));
}

/// wpt `position-anchor-002.html`: each anchor is a shadow host naming
/// itself `--a` through `:host`, and its slotted box asks for `--a` through
/// `::slotted()` — both from the shadow tree, so they match, and the host
/// (the nearest ancestor) wins over the document's own `--a`.
#[test]
fn wpt_position_anchor_002() {
    let mut page = Page::new(&format!(
        "{POSITION_ANCHOR_PAGE}
         #fake-anchor {{ anchor-name: --a; }}
         #anchor1 {{ margin-left: 100px; }}
         #anchor2 {{ margin-left: 300px; margin-top: 100px; }}"
    ));
    let root = page.root();
    page.el(root, "view#fake-anchor", "");
    let anchor1 = page.el(root, "view.anchor#anchor1", "");
    let target1 = page.el(anchor1, "view.target#target1", "");
    let anchor2 = page.el(root, "view.anchor#anchor2", "");
    let target2 = page.el(anchor2, "view.target#target2", "");
    for host in [anchor1, anchor2] {
        let shadow = page.doc.dom.attach_shadow(host, ShadowRootMode::Open);
        page.doc.dom.add_shadow_stylesheet(
            shadow,
            ":host { anchor-name: --a; }
             ::slotted(.target) { position-anchor: --a; }",
        );
        page.el(shadow, "slot", "");
    }
    page.layout();
    assert_eq!(page.abs(target1), (100.0, 100.0, 100.0, 100.0));
    assert_eq!(page.abs(target2), (300.0, 300.0, 100.0, 100.0));
}

/// A box whose default anchor does not exist yet is laid out without one,
/// and picks it up when it appears behind a containment boundary.
#[test]
fn a_default_anchor_appearing_later_is_picked_up() {
    let mut page = Page::new(
        ".anchor { anchor-name: --a; width: 40px; height: 30px; margin-left: 60px; }
         .reader { position: absolute; position-anchor: --a; position-area: bottom span-right;
                   width: 10px; height: 10px; }
         .deep { contain: strict; width: 200px; height: 100px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
    let deep = page.el(cb, "view.deep", "");
    let reader = page.el(cb, "view.reader", "");
    page.layout();
    assert_eq!(page.offset(reader, cb), (0.0, 0.0, 10.0, 10.0));
    page.el(deep, "view.anchor", "");
    page.layout();
    assert_eq!(page.offset(reader, cb), (60.0, 30.0, 10.0, 10.0));
}
