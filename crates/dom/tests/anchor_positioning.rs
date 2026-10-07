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
//! **Where the Editor's Draft and a file disagree, the ED wins** and the
//! port says so: §2.3 now prefers an acceptable *ancestor* over the last
//! element in tree order (`anchor-name-001.html`,
//! `anchor-position-003.html`), and loosely matched names reach into
//! shadow trees (`anchor-name-in-shadow.html`'s second case).
//!
//! **Not ported, and why:** top-layer, popover, dialog and `::backdrop`
//! cases (the top layer exists, but §2.3's top-layer acceptability clause is
//! not implemented, and `dom` has no popover or dialog element);
//! pseudo-element cases (`::before` anchors and implicit anchors);
//! `writing-mode` and `vertical-*` cases (only
//! `direction` exists); multicol, inline-fragmentation, table and fieldset
//! cases (none of those boxes exists here); `transform-*` (the layout box
//! is the anchor box, §28); `zoom`, print and iframe cases; CSSOM, Typed OM,
//! `getComputedStyle`-inset and IDL cases, and the `@position-try` rule
//! caching files (CSSOM); animation, transition and interpolation cases;
//! container-query cases (`@container` is not supported); `ident()`
//! (css-values-5, not in the fork); `CSS.registerProperty`; removing a
//! stylesheet (`remove-position-try-rules-001.html` — no API for it);
//! `-crash` files; `anchor-scroll-*`, `position-visibility-*` and the
//! other scroll-compensation files (they drive scrolling and visibility
//! through the browser's rendering loop; `bounding_client_rect` reports the
//! default scroll shift — see
//! `bounding_client_rect_includes_the_default_scroll_shift` — and the
//! painter's tests in `crates/bobcat-core/src/paint/event_loop_tests.rs`
//! and `crates/dom/src/visual/anchored.rs` cover §3.3 and §6.6);
//! `position-try-order-include-base.html` (it passes only if the base
//! style is re-sorted while it fits; the ED determines fallback only on
//! overflow); `position-area-fixed.html` and the fixed-position and
//! `position-area-overflow-icb-*` reftests (they size the root element
//! against the initial containing block, which is the viewport here); the
//! remaining reftests; and parse/computed-value cases (the fork's
//! `lynx_anchor_positioning.rs` has them).

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

/// One `anchor-name-mutation.html` scenario: the anchors named at first,
/// the expected `(left, top)`, the anchors whose name is then added (`true`)
/// or removed, and the expected `(left, top)` after.
type MutationCase = (
    &'static [usize],
    (f32, f32),
    &'static [(usize, bool)],
    (f32, f32),
);

/// One `anchor-scope-*` template: its name, the outline, and each query
/// box's expected `(left, top)`.
type ScopeCase = (&'static str, &'static str, &'static [(f32, f32)]);

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
#[allow(clippy::too_many_lines, reason = "the file's twelve cases, one table")]
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
    let cases: [BorderCase; 12] = [
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
        (
            "view.cb",
            None,
            "view.anchor1",
            "view.target.paddings",
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

/// An absolutely positioned anchor that escapes a static wrapper is laid
/// out by its containing block's absolute pass, in tree order, before the
/// later box reading it — not after it, by a pass of its own.
#[test]
fn an_escaping_anchor_is_laid_out_before_its_reader() {
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

/// The page css-position-3's layout order exists for: a chain of anchors
/// alternating between a containing block's own absolutely positioned
/// children and boxes that escape static wrappers into it. Each box's left
/// edge is the right edge of the box before it, so every box reads one
/// placed earlier in tree order in the same absolute pass, and the chain is
/// right after one `layout()` — and stays right after a change at its head.
#[test]
fn an_anchor_chain_through_escaping_boxes_settles_in_tree_order() {
    let mut page = Page::new(
        ".x1 { anchor-name: --x1; width: 10px; height: 10px; }
         .link { position: absolute; top: 0px; width: 10px; height: 10px; }
         .y1 { anchor-name: --y1; left: anchor(--x1 right); }
         .x2 { anchor-name: --x2; left: anchor(--y1 right); }
         .y2 { anchor-name: --y2; left: anchor(--x2 right); }
         .x3 { anchor-name: --x3; left: anchor(--y2 right); }
         .y3 { left: anchor(--x3 right); }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
    let x1 = page.el(cb, "view.x1", "");
    let y1 = page.el(cb, "view.link.y1", "");
    let wrapper = page.el(cb, "view", "");
    let x2 = page.el(wrapper, "view.link.x2", "");
    let y2 = page.el(cb, "view.link.y2", "");
    let deeper = page.el(cb, "view", "");
    let inner = page.el(deeper, "view", "");
    let x3 = page.el(inner, "view.link.x3", "");
    let y3 = page.el(cb, "view.link.y3", "");
    page.layout();
    let left = |page: &Page, id| page.offset(id, cb).0;
    // Two hops through one escaping box, then three.
    assert_eq!(
        [x2, y2, x3, y3].map(|id| left(&page, id)),
        [20.0, 30.0, 40.0, 50.0]
    );
    assert_eq!(left(&page, y1), 10.0);

    page.doc.set_inline(x1, "margin-left: 5px");
    page.layout();
    assert_eq!(
        [y1, x2, y2, x3, y3].map(|id| left(&page, id)),
        [15.0, 25.0, 35.0, 45.0, 55.0]
    );
}

/// A box that escapes a subtree relaid in place keeps following its static
/// position: its containing block, above the subtree, does not run, so the
/// rounding tail places it from what the relay recorded. (A flex
/// container's static position for an out-of-flow child is that of a sole
/// item, so `justify-content: center` makes it depend on the box's size.)
#[test]
fn an_escaping_box_follows_an_in_place_relayout() {
    let mut page = Page::new(
        ".frame { width: 200px; height: 200px; justify-content: center; }
         .escaping { position: absolute; left: 3px; width: 10px; height: 10px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
    let frame = page.el(cb, "view.frame", "");
    page.el(frame, "view", "height: 20px");
    let escaping = page.el(frame, "view.escaping", "");
    page.layout();
    assert_eq!(page.offset(escaping, cb), (3.0, 95.0, 10.0, 10.0));
    page.doc.set_inline(escaping, "height: 30px");
    page.layout();
    assert_eq!(page.offset(escaping, cb), (3.0, 85.0, 10.0, 30.0));
}

/// A subtree moved under another containing block takes its escaping boxes
/// with it, caches and all: they are laid out by the new one.
#[test]
fn an_escaping_box_moves_with_its_subtree() {
    let mut page = Page::new(
        ".escaping { position: absolute; right: 0px; top: 0px; width: 10px; height: 10px; }",
    );
    let root = page.root();
    let first = page.el(root, "view.cb", "width: 300px; height: 100px");
    let second = page.el(root, "view.cb", "width: 200px; height: 100px");
    let wrapper = page.el(first, "view", "");
    let escaping = page.el(wrapper, "view.escaping", "");
    page.layout();
    assert_eq!(page.offset(escaping, first).0, 290.0);
    page.doc.dom.append_child(second, wrapper);
    page.layout();
    assert_eq!(page.offset(escaping, second).0, 190.0);
}

/// A position option that makes both insets of an axis `auto` places the
/// box at its static position — which a grid container computes for a box
/// with options whatever its own style's insets say (here `justify-items:
/// center` puts it at 75, not at the padding edge).
#[test]
fn an_option_with_auto_insets_reads_the_grid_static_position() {
    let mut page = Page::new(
        ".grid { display: grid; grid-template-columns: 200px; grid-template-rows: 200px;
                 justify-items: center; width: 200px; height: 200px; }
         .box { position: absolute; left: 0px; top: 300px; width: 50px; height: 50px;
                position-try-fallbacks: --auto; }
         @position-try --auto { left: auto; top: 0px; }",
    );
    let root = page.root();
    let grid = page.el(root, "view.grid.cb", "");
    let target = page.el(grid, "view.box", "");
    page.layout();
    assert_eq!(page.offset(target, grid), (75.0, 0.0, 50.0, 50.0));
}

/// The same for a box escaping a Lynx `linear` parent: the parent measures
/// it for its static position when it has options, so the option's `auto`
/// insets land on the cross-axis gravity (75), not on a zero-size box's
/// (100).
#[test]
fn an_option_with_auto_insets_reads_the_linear_static_position() {
    let mut page = Page::new(
        ".lin { display: linear; linear-direction: column; align-items: center;
                width: 200px; height: 200px; }
         .box { position: absolute; left: 0px; top: 300px; width: 50px; height: 50px;
                position-try-fallbacks: --auto; }
         @position-try --auto { left: auto; top: 0px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 200px; height: 200px");
    let linear = page.el(cb, "view.lin", "");
    let target = page.el(linear, "view.box", "");
    page.layout();
    assert_eq!(page.offset(target, cb), (75.0, 0.0, 50.0, 50.0));
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

/// The page `scrollable-containing-block-size.html` shares: a 100×100
/// padding box scroller holding a 180×180 filler with the anchor inside,
/// and an inset-0, stretched box with a default anchor.
fn scrollable_size_page() -> Page {
    Page::new(
        ".scroller { overflow: hidden; width: 80px; height: 80px; margin: 10px;
                     border: 3px solid black; padding: 10px; }
         .filler { min-width: 180px; min-height: 180px; }
         .relative { position: relative; left: 20px; top: 40px; }
         .translate { transform: translateX(50px); }
         .anchor { anchor-name: --a; }
         .target { position-anchor: --a; position: absolute; top: 0px; left: 0px; right: 0px;
                   bottom: 0px; justify-self: stretch; align-self: stretch; }",
    )
}

/// One scroller of [`scrollable_size_page`]: `inline` on the scroller, the
/// filler's classes; answers the scroller and the target.
fn scrollable_size_case(page: &mut Page, inline: &str, filler: &str) -> (NodeId, NodeId) {
    let root = page.root();
    let scroller = page.el(root, "view.cb.scroller", inline);
    let filler = page.el(scroller, filler, "");
    page.el(filler, "view.anchor", "");
    (scroller, page.el(scroller, "view.target", ""))
}

/// wpt `scrollable-containing-block-size.html`, the rows without a
/// relative shift or a reversed direction: the box is laid out against its
/// scroller's scrolling area. Adaptation: block rows are column flexboxes.
/// Where this engine's scrolling area differs from the file's (which ends
/// past the scroller's end padding), the row asserts the engine's: its
/// flexbox scrollable overflow ends at the content's far edge (190), its
/// grid's includes the end padding (200, as the file). Not ported: the
/// `column-reverse`, `row-reverse` and `direction: rtl` rows, whose
/// scrollable overflow extends before the padding edge, which this
/// engine's scroll containers never do.
#[test]
fn wpt_scrollable_containing_block_size() {
    let mut page = scrollable_size_page();
    let cases = [
        ("", "view.filler", 190.0),
        ("", "view.filler.translate", 190.0),
        ("display: flex; flex-direction: row", "view.filler", 190.0),
        (
            "display: flex; flex-direction: row",
            "view.filler.translate",
            190.0,
        ),
        ("display: grid", "view.filler", 200.0),
        ("display: grid", "view.filler.translate", 200.0),
    ];
    let mut built: Vec<_> = cases
        .iter()
        .map(|&(inline, filler, size)| {
            let (scroller, target) = scrollable_size_case(&mut page, inline, filler);
            (scroller, target, size, inline, filler)
        })
        .collect();
    // "Grid layout with template": the anchor is the grid item.
    let root = page.root();
    let scroller = page.el(
        root,
        "view.cb.scroller",
        "display: grid; grid-template-rows: 180px; grid-template-columns: 180px",
    );
    page.el(scroller, "view.anchor", "");
    let target = page.el(scroller, "view.target", "");
    built.push((scroller, target, 200.0, "grid template", ""));
    page.layout();
    for (scroller, target, size, inline, filler) in built {
        assert_eq!(
            page.offset(target, scroller),
            (0.0, 0.0, size, size),
            "{inline:?} {filler}"
        );
    }
}

/// wpt `scrollable-containing-block-size.html`, the relative-shift rows,
/// as the file expects them.
#[test]
#[ignore = "GAP: a relatively positioned child's offset counts toward this engine's flexbox \
            scrollable overflow (the box is 210×230 in block/flex rows; grid matches the file)"]
fn wpt_scrollable_containing_block_size_relative_shift() {
    let mut page = scrollable_size_page();
    let cases: Vec<_> = ["", "display: flex; flex-direction: row", "display: grid"]
        .into_iter()
        .map(|inline| {
            let (scroller, target) =
                scrollable_size_case(&mut page, inline, "view.filler.relative");
            (scroller, target, inline)
        })
        .collect();
    page.layout();
    for (scroller, target, inline) in cases {
        assert_eq!(
            page.offset(target, scroller),
            (0.0, 0.0, 200.0, 200.0),
            "{inline:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Target lookup: more of §2.3.

/// Builds `outline` under `parent`: one element per line, indented two
/// spaces per level below the least indented line, written
/// `spec [| inline style] [=> expected]`. Answers the elements carrying an
/// expectation, in tree order.
fn outline(page: &mut Page, parent: NodeId, outline: &str) -> Vec<(NodeId, f32)> {
    let lines: Vec<&str> = outline
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let indent = |line: &str| line.len() - line.trim_start().len();
    let base = lines.iter().map(|line| indent(line)).min().unwrap_or(0);
    let mut stack: Vec<(usize, NodeId)> = vec![(0, parent)];
    let mut expectations = Vec::new();
    for line in lines {
        let depth = (indent(line) - base) / 2 + 1;
        let (rest, expected) = match line.trim().split_once("=>") {
            Some((rest, expected)) => (rest, Some(expected.trim().parse::<f32>().unwrap())),
            None => (line.trim(), None),
        };
        let (spec, inline) = rest.split_once('|').unwrap_or((rest, ""));
        while stack.last().is_some_and(|&(level, _)| level >= depth) {
            stack.pop();
        }
        let parent = stack.last().expect("an outline line nested too deep").1;
        let id = page.el(parent, spec.trim(), inline.trim());
        if let Some(expected) = expected {
            expectations.push((id, expected));
        }
        stack.push((depth, id));
    }
    expectations
}

/// Lays `page` out and checks every `(box, expected width)` pair.
fn assert_widths(page: &mut Page, boxes: &[(NodeId, f32)], case: &str) {
    page.layout();
    for (index, &(id, expected)) in boxes.iter().enumerate() {
        assert_eq!(page.abs(id).2, expected, "{case}: target {index}");
    }
}

/// The styles `anchor-name-001.html` to `-003.html` share.
const ANCHOR_NAME_PAGE: &str = ".relpos { position: relative; }
     .abspos { position: absolute; }
     .anchor1 { anchor-name: --a1; width: 10px; height: 10px; }
     .target { position: absolute; width: anchor-size(--a1 width); height: 10px; }";

/// wpt `anchor-name-001.html`: several acceptable `--a1` anchors, the last
/// in tree order wins. **The middle target differs from the file:** it sits
/// inside the 10px `--a1` anchor, which is its ancestor and acceptable (in
/// flow in the same containing block), and the Editor's Draft's §2.3 now
/// checks ancestors first ("If an ancestor of query el satisfies the
/// following conditions, return the nearest such element"), so it reads
/// 10px; the file predates that rule and expects the last one's 30px.
#[test]
fn wpt_anchor_name_001() {
    let mut page = Page::new(ANCHOR_NAME_PAGE);
    let root = page.root();
    let boxes = outline(
        &mut page,
        root,
        "
         view.relpos
           view.target => 30
           view.anchor1 | width: 10px
             view.anchor1 | width: 20px
             view.target => 10
           view.anchor1 | width: 30px
           view.target => 30",
    );
    assert_widths(&mut page, &boxes, "anchor-name-001");
}

/// wpt `anchor-name-002.html`: an absolutely positioned anchor is
/// acceptable only to boxes after it, directly or through the containing
/// blocks of its ancestors.
#[test]
fn wpt_anchor_name_002() {
    let mut page = Page::new(ANCHOR_NAME_PAGE);
    let root = page.root();
    let boxes = outline(
        &mut page,
        root,
        "
         view.relpos
           view
             view.relpos
               view.target => 0
               view.abspos
                 view.relpos
                   view.target => 0
                   view.anchor1 | position: absolute
                   view.target => 10
                 view.target => 10
               view.target => 10
           view.target => 10",
    );
    assert_widths(&mut page, &boxes, "anchor-name-002");
}

/// wpt `anchor-name-003.html`, all five groups: in-flow and out-of-flow
/// anchors in the query box's own and its ancestors' containing blocks,
/// before and after the ones propagated from below.
#[test]
fn wpt_anchor_name_003() {
    let groups = [
        "
         view.relpos
           view.target => 30
           view
             view.target => 30
             view.relpos
               view.target => 0
               view.abspos
                 view.target => 30
                 view.relpos
                   view.target => 40
                   view.anchor1 | width: 20px
                   view.anchor1 | position: absolute; width: 10px
                   view.anchor1 | width: 40px
                   view.anchor1 | position: absolute; width: 30px
                   view.target => 30
               view.target => 30
             view.target => 30
           view.target => 30",
        "
         view.relpos
           view
             view.relpos
               view.target => 0
               view.abspos
                 view.relpos
                   view.target => 20
                   view.anchor1 | width: 20px
                   view.anchor1 | position: absolute; width: 10px
                   view.target => 10
                 view.anchor1 | width: 50px
                 view.target => 50
               view.target => 50
             view.anchor1 | width: 60px
             view.target => 70
           view.anchor1 | width: 70px
           view.target => 70",
        "
         view.relpos
           view
             view.relpos
               view.target => 0
               view.abspos
                 view.relpos
                   view.target => 20
                   view.anchor1 | width: 20px
                   view.anchor1 | position: absolute; width: 10px
                   view.target => 10
                 view.anchor1 | position: absolute; width: 110px
                 view.target => 110
               view.target => 110
             view.target => 110
           view.anchor1 | position: absolute; width: 100px
           view.target => 100",
        "
         view.relpos
           view
             view.relpos
               view.abspos
                 view.relpos
                   view.target => 20
                   view.anchor1 | position: absolute; width: 10px
                   view.anchor1 | width: 20px
                   view.target => 20
                 view.anchor1 | width: 120px
                 view.target => 120
               view.anchor1 | width: 110px
               view.target => 110
             view.target => 100
           view.anchor1 | width: 100px
           view.target => 100",
        "
         view.relpos
           view.target => 10
           view.anchor1 | position: absolute; width: 100px
           view
             view.target => 10
             view.relpos
               view.target => 0
               view.anchor1 | position: absolute; width: 110px
               view.abspos
                 view.target => 10
                 view.anchor1 | position: absolute; width: 120px
                 view.relpos
                   view.target => 20
                   view.anchor1 | width: 20px
                   view.anchor1 | position: absolute; width: 10px
                   view.target => 10
                 view.target => 10
               view.target => 10
             view.target => 10
           view.target => 10",
    ];
    for (index, group) in groups.into_iter().enumerate() {
        let mut page = Page::new(ANCHOR_NAME_PAGE);
        let root = page.root();
        let boxes = outline(&mut page, root, group);
        assert_widths(&mut page, &boxes, &format!("anchor-name-003 group {index}"));
    }
}

/// wpt `anchor-name-004.html`: one anchor under two names; a third name
/// nobody declares takes the fallback.
#[test]
fn wpt_anchor_name_004() {
    let mut page = Page::new(
        ".relpos { position: relative; }
         .anchor1 { anchor-name: --a1, --a2; width: 30px; height: 10px; }
         .target { position: absolute; height: 10px; }
         #target1 { width: anchor-size(--a1 width); }
         #target2 { width: anchor-size(--a2 width); }
         #target3 { width: anchor-size(--a3 width, 11px); }",
    );
    let root = page.root();
    let boxes = outline(
        &mut page,
        root,
        "
         view.relpos
           view.anchor1 | width: 30px
           view.target#target1 => 30
           view.target#target2 => 30
           view.target#target3 => 11",
    );
    assert_widths(&mut page, &boxes, "anchor-name-004");
}

/// wpt `anchor-name-008.html`: a `fixed` box anchors to an absolutely
/// positioned anchor inside a `fixed` container: the container, whose
/// containing block is the query box's, is acceptable, so its child is.
#[test]
fn wpt_anchor_name_008() {
    let mut page = Page::new(
        ".containing-block { position: fixed; width: 200px; height: 200px; }
         #anchor { left: 100px; width: 100px; height: 100px; anchor-name: --anchor;
                   position: absolute; }
         #anchored { width: 100px; height: 100px; position: fixed;
                     left: anchor(--anchor left); top: anchor(--anchor bottom); }",
    );
    let root = page.root();
    let cb = page.el(root, "view.containing-block", "");
    page.el(cb, "view#anchor", "");
    let anchored = page.el(cb, "view#anchored", "");
    page.layout();
    assert_eq!(page.abs(anchored), (100.0, 100.0, 100.0, 100.0));
}

/// wpt `anchor-name-in-shadow-002.html`: two shadow trees declaring the
/// same `--a` each resolve their own. Adaptation: each shadow root gets
/// its own copy of the sheet (the file's point is that a shared sheet does
/// not confuse the tree scopes; this engine does not share sheets between
/// shadow roots at all).
#[test]
fn wpt_anchor_name_in_shadow_002() {
    let mut page = Page::new(
        ".host { width: 100px; height: 100px; }
         #host2 { margin-left: 200px; }",
    );
    let root = page.root();
    let mut targets = Vec::new();
    for spec in ["view.host#host1", "view.host#host2"] {
        let host = page.el(root, spec, "");
        let shadow = page.doc.dom.attach_shadow(host, ShadowRootMode::Open);
        page.doc.dom.add_shadow_stylesheet(
            shadow,
            "view { display: flex; flex-direction: column; flex-shrink: 0;
                    width: 100px; height: 100px; }
             #anchor { anchor-name: --a; }
             #target { position: fixed; left: anchor(--a left); top: anchor(--a bottom); }",
        );
        page.el(shadow, "view#anchor", "");
        targets.push(page.el(shadow, "view#target", ""));
    }
    page.layout();
    assert_eq!(page.abs(targets[0]).0, 0.0);
    assert_eq!(page.abs(targets[0]).1, 100.0);
    assert_eq!(page.abs(targets[1]).0, 200.0);
    assert_eq!(page.abs(targets[1]).1, 200.0);
}

/// wpt `anchor-name-mutation.html`, all fifteen cases: an `anchor-name`
/// added, removed, moved to another element, and a second candidate added
/// after or before the current one, under the three positioning methods.
#[test]
fn wpt_anchor_name_mutation() {
    const NONE: (f32, f32) = (0.0, 0.0);
    const FIRST: (f32, f32) = (100.0, 100.0);
    const SECOND: (f32, f32) = (100.0, 200.0);
    let methods = [
        "positioned-using-anchor-function-explicit-name",
        "positioned-using-anchor-function-implicit-name",
        "positioned-using-position-area",
    ];
    // (initially named, expected, then toggled on (+) or off (-), expected)
    let scenarios: [MutationCase; 5] = [
        (&[], NONE, &[(0, true)], FIRST),
        (&[0], FIRST, &[(0, false)], NONE),
        (&[0], FIRST, &[(0, false), (1, true)], SECOND),
        (&[0], FIRST, &[(1, true)], SECOND),
        (&[1], SECOND, &[(0, true)], SECOND),
    ];
    for method in methods {
        for (index, (named, before, toggles, after)) in scenarios.iter().enumerate() {
            let mut page = Page::new(
                ".containing-block { position: relative; width: 300px; height: 300px;
                                     border: 1px solid black; }
                 .cell { width: 100px; height: 100px; }
                 #anchor-1 { position: absolute; top: 0px; left: 0px; }
                 #anchor-2 { position: absolute; top: 100px; left: 0px; }
                 .anchor { anchor-name: --anchor; }
                 #anchor-positioned { position: absolute; top: 0px; left: 0px; }
                 .positioned-using-anchor-function-explicit-name {
                     position: absolute; top: anchor(--anchor bottom) !important;
                     left: anchor(--anchor right) !important; }
                 .positioned-using-anchor-function-implicit-name {
                     position: absolute; position-anchor: --anchor;
                     top: anchor(bottom) !important; left: anchor(right) !important; }
                 .positioned-using-position-area {
                     position: absolute; position-anchor: --anchor;
                     position-area: bottom right; }",
            );
            let root = page.root();
            let cb = page.el(root, "view.containing-block", "");
            let anchors = [
                page.el(cb, "view.cell#anchor-1", ""),
                page.el(cb, "view.cell#anchor-2", ""),
            ];
            let positioned = page.el(cb, "view.cell#anchor-positioned", "");
            for &anchor in *named {
                page.doc.add_class(anchors[anchor], "anchor");
            }
            page.doc.add_class(positioned, method);
            page.layout();
            let (x, y, ..) = page.offset(positioned, cb);
            assert_eq!((x, y), *before, "{method} case {index}: before");
            for &(anchor, on) in *toggles {
                if on {
                    page.doc.add_class(anchors[anchor], "anchor");
                } else {
                    page.doc.remove_class(anchors[anchor], "anchor");
                }
            }
            page.layout();
            let (x, y, ..) = page.offset(positioned, cb);
            assert_eq!((x, y), *after, "{method} case {index}: after");
        }
    }
}

/// wpt `anchor-position-003.html`: several `--a1` anchors in one
/// containing block. **The second group's first target differs from the
/// file:** it sits inside the 5×7 `--a1` anchor, an acceptable ancestor,
/// which the Editor's Draft's §2.3 prefers over the last one in tree order
/// (see `wpt_anchor_name_001`), so it reads 5, not the file's 9. In the
/// third group the same ancestor generates the box's containing block and
/// is not acceptable, so the inner anchor wins as in the file.
#[test]
fn wpt_anchor_position_003() {
    let mut page = Page::new(
        ".not-positioned-cb { transform: translate(0px, 0px); }
         .anchor1 { anchor-name: --a1; }
         .size5x7 { width: 5px; height: 7px; }
         .size9x11 { width: 9px; height: 11px; }
         .target { position: absolute; left: anchor(--a1 right); }",
    );
    let root = page.root();
    let mut cases = Vec::new();
    for (group, expected) in [
        (
            "
             view.anchor1.size5x7
             view.anchor1.size9x11
             view.target => 9",
            vec![9.0],
        ),
        (
            "
             view.anchor1.size5x7
               view.anchor1.size9x11
               view.target => 5
             view.target => 9",
            vec![5.0, 9.0],
        ),
        (
            "
             view.anchor1.size5x7.not-positioned-cb
               view.anchor1.size9x11
               view.target => 9
             view.target => 9",
            vec![9.0, 9.0],
        ),
    ] {
        let cb = page.el(root, "view.cb", "");
        let boxes = outline(&mut page, cb, group);
        assert_eq!(boxes.len(), expected.len());
        cases.push((cb, boxes));
    }
    page.layout();
    for (index, (cb, boxes)) in cases.into_iter().enumerate() {
        for (target, expected) in boxes {
            assert_eq!(page.offset(target, cb).0, expected, "group {index}");
        }
    }
}

/// wpt `anchor-position-004.html`, its horizontal-tb half: `anchor()` with
/// percentages and `center` on each physical inset. Not ported: the
/// `vertical-rl` half (no `writing-mode`).
#[test]
fn wpt_anchor_position_004() {
    let mut page = Page::new(
        ".relpos { position: relative; width: 200px; }
         .spacer { width: 10px; height: 10px; }
         #anchor { anchor-name: --a1; margin: 20px; width: 100px; height: 200px; }
         .target { position: absolute; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.relpos", "");
    page.el(cb, "view.spacer", "");
    page.el(cb, "view#anchor", "");
    let sides = [
        ("0%", 20.0, 30.0),
        ("20%", 40.0, 70.0),
        ("50%", 70.0, 130.0),
        ("center", 70.0, 130.0),
        ("80%", 100.0, 190.0),
        ("100%", 120.0, 230.0),
    ];
    let mut targets = Vec::new();
    for (side, x, y) in sides {
        for (property, horizontal) in [
            ("left", true),
            ("right", true),
            ("top", false),
            ("bottom", false),
        ] {
            let inline = format!("{property}: anchor(--a1 {side})");
            let target = page.el(cb, "view.target", &inline);
            targets.push((target, inline, horizontal, if horizontal { x } else { y }));
        }
    }
    page.layout();
    for (target, inline, horizontal, expected) in targets {
        let (x, y, ..) = page.offset(target, cb);
        assert_eq!(if horizontal { x } else { y }, expected, "{inline}");
    }
}

/// wpt `anchor-position-borders-002.html`, every case: `direction: rtl`
/// scroll containers with and without borders as the containing block, as
/// an intermediate containing block, and around the anchor; the box covers
/// the anchor's border box exactly.
#[test]
fn wpt_anchor_position_borders_002() {
    let mut page = Page::new(
        ".cb { position: relative; border-bottom: 2px solid gray; }
         .not-positioned-cb { transform: translate(0px, 0px); }
         .scroller { overflow: scroll; }
         .borders { border-width: 5px 6px 7px 8px; border-style: solid; }
         .rtl { direction: rtl; }
         .spacer { height: 9px; }
         .anchor1 { anchor-name: --a1; margin-right: 50px; width: 31px; height: 31px; }
         .target { position: absolute; left: anchor(--a1 left); right: anchor(--a1 right);
                   top: anchor(--a1 top); bottom: anchor(--a1 bottom); }",
    );
    let root = page.root();
    page.el(root, "view.spacer", "");
    let groups = [
        "
         view.cb.scroller.rtl
           view.spacer
           view.anchor1
           view.target",
        "
         view.cb.scroller.borders.rtl
           view.spacer
           view.anchor1
           view.target",
        "
         view.cb
           view.scroller.borders.rtl
             view.spacer
             view.anchor1
           view.target",
        "
         view.cb.scroller.borders.rtl
           view.not-positioned-cb
             view.spacer
             view.anchor1
           view.target",
        "
         view.cb.scroller.borders.rtl
           view.not-positioned-cb.scroller.borders
             view.spacer
             view.anchor1
           view.target",
    ];
    let mut pairs = Vec::new();
    for group in groups {
        let marked = group
            .replace("view.anchor1", "view.anchor1 => 1")
            .replace("view.target", "view.target => 2");
        let boxes = outline(&mut page, root, &marked);
        pairs.push((boxes[0].0, boxes[1].0));
    }
    page.layout();
    for (index, (anchor, target)) in pairs.into_iter().enumerate() {
        assert_eq!(page.abs(target), page.abs(anchor), "case {index}");
    }
}

/// wpt `anchor-position-dynamic-002.html`: anchors in the same and in a
/// different containing block resize after the first layout.
#[test]
fn wpt_anchor_position_dynamic_002() {
    let mut page = Page::new(
        "#container { position: relative; }
         #anchor1 { anchor-name: --a1; }
         #anchor2 { anchor-name: --a2; }
         #anchor1, #anchor2 { width: 5px; height: 7px; }
         .after #anchor1, .after #anchor2 { width: 10px; }
         .target { position: absolute; }",
    );
    let root = page.root();
    let container = page.el(root, "view#container", "");
    page.el(container, "view#anchor1", "");
    let left1 = page.el(container, "view.target", "left: anchor(--a1 right)");
    let width1 = page.el(container, "view.target", "width: anchor-size(--a1 width)");
    let wrapper = page.el(container, "view", "");
    page.el(wrapper, "view#anchor2", "");
    let left2 = page.el(container, "view.target", "left: anchor(--a2 right)");
    let width2 = page.el(container, "view.target", "width: anchor-size(--a2 width)");
    for (class, expected) in [(None, 5.0), (Some("after"), 10.0)] {
        if let Some(class) = class {
            page.doc.add_class(container, class);
        }
        page.layout();
        assert_eq!(page.offset(left1, container).0, expected);
        assert_eq!(page.offset(left2, container).0, expected);
        assert_eq!(page.abs(width1).2, expected);
        assert_eq!(page.abs(width2).2, expected);
    }
}

/// wpt `anchor-position-dynamic-003.html`, the `contain: layout` and the
/// scroll container cases: an anchor that changes size inside another
/// formatting context relays its reader. Not ported: the float, table and
/// inline-block cases (none of those boxes exists here).
#[test]
fn wpt_anchor_position_dynamic_003() {
    let mut page = Page::new(
        ".containing-block { position: absolute; }
         .anchor { anchor-name: --a1; width: 50px; height: 70px; }
         .after .anchor { width: 70px; height: 50px; }
         .target { position: absolute; left: anchor(--a1 right); top: anchor(--a1 bottom);
                   width: anchor-size(--a1 width); height: anchor-size(--a1 height); }
         .contain { contain: layout; }
         .scroller { overflow: scroll; width: 20px; height: 20px; }",
    );
    let root = page.root();
    let body = page.el(root, "view", "");
    let mut targets = Vec::new();
    for context in ["view.contain", "view.scroller"] {
        let cb = page.el(body, "view.containing-block", "");
        let context = page.el(cb, context, "");
        page.el(context, "view.anchor", "");
        targets.push((cb, page.el(cb, "view.target", "")));
    }
    page.layout();
    for &(cb, target) in &targets {
        assert_eq!(page.offset(target, cb), (50.0, 70.0, 50.0, 70.0));
    }
    page.doc.add_class(body, "after");
    page.layout();
    for &(cb, target) in &targets {
        assert_eq!(page.offset(target, cb), (70.0, 50.0, 70.0, 50.0));
    }
}

/// wpt `anchor-position-dynamic-004.html`: the anchor sits behind a
/// `contain: strict` boundary and moves after the first layout.
#[test]
fn wpt_anchor_position_dynamic_004() {
    let mut page = Page::new(
        "#anchor1 { anchor-name: --a1; margin-left: 15px; width: 30px; height: 20px; }
         .after #anchor1 { margin-left: 50px; }
         .target { position: absolute; left: anchor(--a1 left); top: anchor(--a1 top);
                   right: anchor(--a1 right); bottom: anchor(--a1 bottom); }",
    );
    let root = page.root();
    let body = page.el(root, "view", "");
    let cb = page.el(body, "view.cb", "");
    let strict = page.el(cb, "view", "contain: strict; height: 50px");
    page.el(strict, "view", "height: 10px");
    page.el(strict, "view#anchor1", "");
    let target = page.el(cb, "view.target", "");
    page.layout();
    assert_eq!(page.offset(target, cb), (15.0, 10.0, 30.0, 20.0));
    page.doc.add_class(body, "after");
    page.layout();
    assert_eq!(page.offset(target, cb), (50.0, 10.0, 30.0, 20.0));
}

/// wpt `anchor-position-principal-box.html`: a `display: contents` element
/// generates no box, so its `anchor-name` names nothing; its child with
/// the same name is the anchor.
#[test]
fn wpt_anchor_position_principal_box() {
    let mut page = Page::new(
        "#outer { anchor-name: --anchor; display: contents; }
         #inner { anchor-name: --anchor; }
         #filler { height: 100px; }
         #anchored { position: absolute; top: anchor(--anchor top); }",
    );
    let root = page.root();
    let outer = page.el(root, "view#outer", "");
    page.el(outer, "view#filler", "");
    page.el(outer, "view#inner", "");
    let anchored = page.el(root, "view#anchored", "");
    page.layout();
    assert_eq!(page.abs(anchored).1, 100.0);
}

/// wpt `anchor-position-sibling-index.html` and
/// `anchor-position-flip-sibling-index.html`: `sibling-index()` inside an
/// `anchor()` side percentage, and the same under `flip-block` (40% becomes
/// 60% of the anchor on the other inset). Adaptation: the files read
/// `getComputedStyle(abs).top`; this is the box's position.
#[test]
fn wpt_anchor_position_sibling_index() {
    for (inline, expected) in [
        ("top: anchor(calc(25% * sibling-index()))", 50.0),
        (
            "bottom: anchor(calc(20% * sibling-index())); position-try-fallbacks: flip-block",
            60.0,
        ),
    ] {
        let mut page = Page::new(
            "#anchor { anchor-name: --a; width: 100px; height: 100px; }
             #abs { position-anchor: --a; position: absolute; width: 100px; height: 100px; }",
        );
        let root = page.root();
        let wrapper = page.el(root, "view", "");
        page.el(wrapper, "view#anchor", "");
        let abs = page.el(wrapper, "view#abs", inline);
        page.layout();
        assert_eq!(page.abs(abs).1, expected, "{inline}");
    }
}

/// wpt `anchor-positioned-containing-block-resize.html`, the geometry
/// half: the containing block shrinks and the box stays on its anchor.
/// Not ported: the `getComputedStyle` inset readbacks (this engine's
/// readback reports the computed `anchor()`, not a used length).
#[test]
fn wpt_anchor_positioned_containing_block_resize() {
    let mut page = Page::new(
        ".anchor, .anchored { width: 100px; height: 100px; position: absolute; }
         .anchor { left: 300px; top: 200px; anchor-name: --a; }
         .anchored { position-anchor: --a; right: anchor(left); bottom: anchor(top); }
         .container { position: relative; width: 500px; height: 500px;
                      border: 2px solid red; }
         .resize { width: 400px; height: 400px; }",
    );
    let root = page.root();
    let container = page.el(root, "view.container", "");
    page.el(container, "view.anchor", "");
    let wrapper = page.el(container, "view", "");
    let anchored = page.el(wrapper, "view.anchored", "");
    page.layout();
    assert_eq!(
        page.offset(anchored, container),
        (200.0, 100.0, 100.0, 100.0)
    );
    page.doc.add_class(container, "resize");
    page.layout();
    assert_eq!(
        page.offset(anchored, container),
        (200.0, 100.0, 100.0, 100.0)
    );
}

/// wpt `remove-anchor-dirty-layout.html`: removing the anchor after a
/// rendering update and rendering again. The file only requires no crash;
/// the box falls back to its static position, which in a column flexbox is
/// the containing block's start.
#[test]
fn wpt_remove_anchor_dirty_layout() {
    let mut page = Page::new(
        "#anchor { anchor-name: --a; height: 20px; }
         #target { position: absolute; top: anchor(top); position-anchor: --a; }",
    );
    let root = page.root();
    page.el(root, "view", "height: 30px");
    let anchor = page.el(root, "view#anchor", "");
    let target = page.el(root, "view#target", "");
    page.render();
    assert_eq!(page.abs(target).1, 30.0);
    page.doc.dom.remove_element(anchor);
    page.render();
    assert_eq!(page.abs(target).1, 0.0, "static position");
}

// ---------------------------------------------------------------------------
// anchor-center (§4.2) through the host.

/// The container `anchor-center-003.html` and `-004.html` share, inside a
/// `margin-left: 8px` wrapper standing in for the body's margin (its top
/// margin collapses with the container's in block flow, so it is left out).
fn anchor_center_page(anchor: &str, target: &str) -> (Page, NodeId) {
    let mut page = Page::new(&format!(
        ".container {{ width: 100px; height: 100px; border: 3px solid black;
                      position: relative; margin: 50px; }}
         .anchor {{ anchor-name: --anchor; position: relative; width: 50px;
                   height: 50px; {anchor} }}
         {target}"
    ));
    let root = page.root();
    let body = page.el(root, "view", "margin-left: 8px");
    let container = page.el(body, "view.container", "");
    page.el(container, "view.anchor", "");
    (page, container)
}

/// wpt `anchor-center-003.html`: a `fixed` box with `justify-self:
/// anchor-center` and an auto width sizes to its content and centers on the
/// anchor.
#[test]
fn wpt_anchor_center_003() {
    let (mut page, container) = anchor_center_page(
        "left: 40px; top: 5px;",
        ".target { position-anchor: --anchor; position: fixed;
                   justify-self: anchor-center; top: anchor(bottom); }",
    );
    let target = page.el(container, "view.target", "");
    page.el(target, "view", "width: 30px; height: 20px");
    page.layout();
    let (x, _, width, _) = page.abs(target);
    assert_eq!((x, width), (111.0, 30.0));
}

/// wpt `anchor-center-004.html`: `anchor-center` zeroes `auto` margins on
/// its axis.
#[test]
fn wpt_anchor_center_004() {
    let (mut page, container) = anchor_center_page(
        "left: 30px; top: 20px;",
        ".target { position-anchor: --anchor; width: 24px; height: 24px; position: fixed; }
         .justify { justify-self: anchor-center; top: anchor(bottom);
                    margin-left: auto; margin-right: auto; }
         .align { align-self: anchor-center; right: anchor(left);
                  margin-top: auto; margin-bottom: auto; }",
    );
    let justify = page.el(container, "view.target.justify", "");
    let align = page.el(container, "view.target.align", "");
    page.layout();
    assert_eq!(page.abs(justify).0, 104.0);
    assert_eq!(page.abs(align).1, 86.0);
}

/// wpt `anchor-center-offset-change.html`: the anchor grows and the
/// centered box moves.
#[test]
fn wpt_anchor_center_offset_change() {
    let mut page = Page::new(
        "#cb { position: relative; width: 200px; height: 200px; }
         #anchor { width: 100px; height: 100px; anchor-name: --anchor; }
         #anchored { position: absolute; width: 100px; height: 100px;
                     position-anchor: --anchor; align-self: anchor-center;
                     left: anchor(--unknown right, 0px); }",
    );
    let root = page.root();
    let cb = page.el(root, "view#cb", "");
    let anchor = page.el(cb, "view#anchor", "");
    let anchored = page.el(cb, "view#anchored", "");
    page.layout();
    assert_eq!(page.offset(anchored, cb).1, 0.0);
    page.doc.set_inline(anchor, "height: 200px");
    page.layout();
    assert_eq!(page.offset(anchored, cb), (0.0, 50.0, 100.0, 100.0));
}

// ---------------------------------------------------------------------------
// position-area (§3.1) through the host.

/// Lays `anchored` out under each `(position-area, expected)` pair and
/// checks its offset and size against `cb`'s padding box.
fn assert_areas(
    page: &mut Page,
    cb: NodeId,
    anchored: NodeId,
    extra: &str,
    cases: &[(&str, Rect4)],
) {
    for &(area, expected) in cases {
        page.doc
            .set_inline(anchored, &format!("position-area: {area}; {extra}"));
        page.layout();
        assert_eq!(page.offset(anchored, cb), expected, "{area} {extra}");
    }
}

/// The page `position-area-anchor-outside.html` and
/// `-partially-outside.html` share: a 400px bordered container and a
/// stretched box anchored to `anchor`'s absolutely positioned box.
fn area_outside_page(anchor: &str) -> (Page, NodeId, NodeId) {
    let mut page = Page::new(&format!(
        "#container {{ position: relative; width: 400px; height: 400px; margin: 0px auto;
                      border: 2px solid black; }}
         #anchor {{ position: absolute; width: 100px; height: 100px; anchor-name: --anchor;
                   {anchor} }}
         #anchored {{ position: absolute; align-self: stretch; justify-self: stretch;
                     position-anchor: --anchor; }}"
    ));
    let root = page.root();
    let container = page.el(root, "view#container", "");
    page.el(container, "view#anchor", "");
    let anchored = page.el(container, "view#anchored", "");
    (page, container, anchored)
}

/// wpt `position-area-anchor-outside.html`, every case: the anchor lies
/// outside the containing block, so the grid's outer lines extend to it and
/// some tracks are empty.
#[test]
fn wpt_position_area_anchor_outside() {
    let (mut page, cb, anchored) = area_outside_page("left: -200px; top: 500px;");
    assert_areas(
        &mut page,
        cb,
        anchored,
        "",
        &[
            ("span-all", (-200.0, 0.0, 600.0, 600.0)),
            ("left span-all", (-200.0, 0.0, 0.0, 600.0)),
            ("span-left span-all", (-200.0, 0.0, 100.0, 600.0)),
            ("span-all center", (-200.0, 0.0, 100.0, 600.0)),
            ("span-right span-all", (-200.0, 0.0, 600.0, 600.0)),
            ("right span-all", (-100.0, 0.0, 500.0, 600.0)),
            ("top span-all", (-200.0, 0.0, 600.0, 500.0)),
            ("span-top span-all", (-200.0, 0.0, 600.0, 600.0)),
            ("center span-all", (-200.0, 500.0, 600.0, 100.0)),
            ("span-bottom span-all", (-200.0, 500.0, 600.0, 100.0)),
            ("bottom span-all", (-200.0, 600.0, 600.0, 0.0)),
        ],
    );
}

/// wpt `position-area-anchor-partially-outside.html`, every case.
#[test]
fn wpt_position_area_anchor_partially_outside() {
    let (mut page, cb, anchored) = area_outside_page("right: -50px; top: -50px;");
    assert_areas(
        &mut page,
        cb,
        anchored,
        "",
        &[
            ("span-all", (0.0, -50.0, 450.0, 450.0)),
            ("left span-all", (0.0, -50.0, 350.0, 450.0)),
            ("span-left span-all", (0.0, -50.0, 450.0, 450.0)),
            ("span-all center", (350.0, -50.0, 100.0, 450.0)),
            ("span-right span-all", (350.0, -50.0, 100.0, 450.0)),
            ("right span-all", (450.0, -50.0, 0.0, 450.0)),
            ("top span-all", (0.0, -50.0, 450.0, 0.0)),
            ("span-top span-all", (0.0, -50.0, 450.0, 100.0)),
            ("center span-all", (0.0, -50.0, 450.0, 100.0)),
            ("span-bottom span-all", (0.0, -50.0, 450.0, 450.0)),
            ("bottom span-all", (0.0, 50.0, 450.0, 350.0)),
        ],
    );
}

/// wpt `position-area-with-insets.html`, every case: insets apply inside
/// the `position-area` region; without a default anchor only the insets
/// apply. The anchored box comes before its in-flow anchor in tree order.
#[test]
fn wpt_position_area_with_insets() {
    let mut page = Page::new(
        "#container { position: absolute; width: 400px; height: 400px; }
         #anchored { position: absolute; align-self: stretch; justify-self: stretch;
                     position-anchor: --anchor; }
         #anchor { margin-top: 150px; margin-left: 100px; width: 150px; height: 75px;
                   anchor-name: --anchor; }",
    );
    let root = page.root();
    let cb = page.el(root, "view#container", "");
    let anchored = page.el(cb, "view#anchored", "");
    page.el(cb, "view#anchor", "");
    for (area, insets, expected) in [
        (
            "span-all",
            "top: 5px; bottom: 5px; left: 5px; right: 5px",
            (5.0, 5.0, 390.0, 390.0),
        ),
        (
            "center center",
            "top: 10px; bottom: 40px; left: 5px; right: 15px",
            (105.0, 160.0, 130.0, 25.0),
        ),
        (
            "left bottom",
            "top: 10px; bottom: 40px; left: 5px; right: 15px",
            (5.0, 235.0, 80.0, 125.0),
        ),
        (
            "span-right center",
            "top: 20%; bottom: auto; left: auto; right: 25%",
            (100.0, 165.0, 225.0, 60.0),
        ),
    ] {
        page.doc
            .set_inline(anchored, &format!("position-area: {area}; {insets}"));
        page.layout();
        assert_eq!(page.offset(anchored, cb), expected, "{area}");
    }
    page.doc.set_inline(
        anchored,
        "position-anchor: auto; position-area: bottom right;
         left: 50px; right: 100px; top: 30px; bottom: 10px",
    );
    page.layout();
    assert_eq!(
        page.offset(anchored, cb),
        (50.0, 30.0, 250.0, 360.0),
        "auto"
    );
}

/// wpt `position-area-in-grid.html`, both cases: the grid area (rows 2–3,
/// column 3 to the padding edge) is the pre-modification containing block,
/// and the anchor outside it extends the `position-area` grid.
#[test]
fn wpt_position_area_in_grid() {
    let mut page = Page::new(
        "#container { display: grid; grid-template-rows: 1fr 1fr 1fr 1fr;
                      grid-template-columns: 1fr 1fr 1fr 1fr; position: relative;
                      width: 400px; height: 400px; }
         #anchor { position: absolute; left: 100px; top: 150px; width: 150px; height: 75px;
                   anchor-name: --anchor; }
         #anchored { grid-row-start: 2; grid-row-end: span 2; grid-column-start: 3;
                     grid-column-end: auto; position: absolute; align-self: stretch;
                     justify-self: stretch; position-anchor: --anchor;
                     border: 3px solid orange; }",
    );
    let root = page.root();
    let cb = page.el(root, "view#container", "");
    page.el(cb, "view#anchor", "");
    let anchored = page.el(cb, "view#anchored", "");
    assert_areas(
        &mut page,
        cb,
        anchored,
        "left: auto; right: auto; top: auto; bottom: auto",
        &[("span-bottom span-left", (100.0, 150.0, 150.0, 150.0))],
    );
    assert_areas(
        &mut page,
        cb,
        anchored,
        "left: 10px; right: 10px; top: 10px; bottom: 10px",
        &[("span-bottom span-left", (110.0, 160.0, 130.0, 130.0))],
    );
}

/// wpt `position-area-chain.html`: five boxes each placed right of the one
/// before by `position-area`, all named `--box`.
#[test]
fn wpt_position_area_chain() {
    let mut page = Page::new(
        ".containing-block { border: 1px solid black; position: relative; width: 500px;
                             height: 200px; }
         .box { position: absolute; anchor-name: --box; position-area: center right;
                position-anchor: --box; width: 50px; height: 50px;
                border-right: 10px solid white; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.containing-block", "");
    let boxes: Vec<NodeId> = (0..5).map(|_| page.el(cb, "view.box", "")).collect();
    page.layout();
    for (index, id) in boxes.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let expected = 60.0 * index as f32;
        assert_eq!(page.offset(id, cb).0, expected, "box {}", index + 1);
        assert_eq!(page.offset(id, cb).1, 0.0, "box {}", index + 1);
    }
}

/// wpt `position-area-value.html`, every case: a `position-area` value as
/// a `position-try-fallbacks` entry places the box as the property does.
/// Adaptation: one box laid out under each value in turn (as
/// `wpt_position_try_order_basic` does), the reference first.
#[test]
fn wpt_position_area_value() {
    let mut page = Page::new(
        "#cb { position: relative; width: 200px; height: 200px; border: 1px solid black; }
         #anchor { position: absolute; left: 100px; top: 100px; width: 80px; height: 80px;
                   anchor-name: --a; }
         #target { position: absolute; width: 40px; height: 40px; position-area: bottom right;
                   position-anchor: --a; }",
    );
    let root = page.root();
    let cb = page.el(root, "view#cb", "");
    page.el(cb, "view#anchor", "");
    let target = page.el(cb, "view#target", "");
    for area in [
        "top left",
        "span-top left",
        "top span-left",
        "top center",
        "left center",
        "start center",
        "center start",
    ] {
        page.doc
            .set_inline(target, &format!("position-area: {area}"));
        page.layout();
        let reference = page.offset(target, cb);
        page.doc
            .set_inline(target, &format!("position-try-fallbacks: {area}"));
        page.layout();
        assert_eq!(page.offset(target, cb), reference, "{area}");
    }
}

/// wpt `scrollable-containing-block-position-area.html`, every case: the
/// `position-area` grid is built on the scrollable containing block.
/// Adaptation: this engine's flexbox scrolling area ends at the content's
/// far edge (190px from the padding-box origin), not past the end padding
/// (the file's 200px) — see `wpt_scrollable_containing_block_size`.
#[test]
fn wpt_scrollable_containing_block_position_area() {
    let mut page = Page::new(
        ".scroller { overflow: hidden; position: relative; width: 80px; height: 80px;
                     margin: 10px; border: 3px solid black; padding: 10px; }
         .filler { min-width: 180px; min-height: 180px; }
         .anchor { anchor-name: --a; width: 50px; height: 50px; position: relative;
                   left: 60px; top: 60px; }
         .target { position: absolute; position-anchor: --a; justify-self: stretch;
                   align-self: stretch; }",
    );
    let root = page.root();
    let mut targets = Vec::new();
    for (area, expected) in [
        ("top", (0.0, 0.0, 190.0, 70.0)),
        ("right", (120.0, 0.0, 70.0, 190.0)),
        ("bottom", (0.0, 120.0, 190.0, 70.0)),
        ("left", (0.0, 0.0, 70.0, 190.0)),
    ] {
        let scroller = page.el(root, "view.scroller", "");
        let filler = page.el(scroller, "view.filler", "");
        page.el(filler, "view.anchor", "");
        let target = page.el(scroller, "view.target", &format!("position-area: {area}"));
        targets.push((scroller, target, area, expected));
    }
    page.layout();
    for (scroller, target, area, expected) in targets {
        assert_eq!(page.offset(target, scroller), expected, "{area}");
    }
}

/// wpt `position-area-computed-insets.html`: `position-area` leaves the
/// computed insets `auto`. Adaptation: Typed OM → computed-value readback.
#[test]
fn wpt_position_area_computed_insets() {
    let mut page = Page::new("#abs { position: absolute; position-area: span-all; }");
    let root = page.root();
    let abs = page.el(root, "view#abs", "");
    page.layout();
    assert_eq!(page.computed(abs, "position-area"), "span-all");
    for inset in ["left", "right", "top", "bottom"] {
        assert_eq!(page.computed(abs, inset), "auto", "{inset}");
    }
}

/// wpt `anchor-in-anchor-positioned.html`: an anchor inside an
/// anchor-positioned box is found by a later positioned box.
#[test]
fn wpt_anchor_in_anchor_positioned() {
    let mut page = Page::new(
        ".containing-block { border: 1px solid black; position: relative; width: 200px;
                             height: 150px; }
         .box { width: 50px; height: 50px; }
         #anchor-1 { position: absolute; top: 50px; left: 50px; anchor-name: --anchor-1; }
         #anchor-positioned-1 { position: absolute; position-anchor: --anchor-1;
                                position-area: top right; }
         #anchor-2 { anchor-name: --anchor-2; }
         #anchor-positioned-2 { position: absolute; position-anchor: --anchor-2;
                                position-area: bottom right; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.containing-block", "");
    page.el(cb, "view.box#anchor-1", "");
    let first = page.el(cb, "view.box#anchor-positioned-1", "");
    page.el(first, "view.box#anchor-2", "");
    let second = page.el(cb, "view.box#anchor-positioned-2", "");
    page.layout();
    assert_eq!(page.offset(first, cb), (100.0, 0.0, 50.0, 50.0));
    assert_eq!(page.offset(second, cb), (150.0, 50.0, 50.0, 50.0));
}

// ---------------------------------------------------------------------------
// anchor() and its fallbacks.

/// wpt `anchor-function-chain.html`: five boxes each placed 10px right of
/// the one before; the first has no `--box` before it, so its `left` is
/// invalid at computed-value time and it keeps its static position.
#[test]
fn wpt_anchor_function_chain() {
    let mut page = Page::new(
        ".containing-block { border: 1px solid black; position: relative; width: 500px;
                             height: 200px; }
         .box { position: absolute; left: calc(anchor(--box right) + 10px);
                anchor-name: --box; width: 50px; height: 50px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.containing-block", "");
    let boxes: Vec<NodeId> = (0..5).map(|_| page.el(cb, "view.box", "")).collect();
    page.layout();
    for (index, id) in boxes.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let expected = 60.0 * index as f32;
        assert_eq!(
            page.offset(id, cb),
            (expected, 0.0, 50.0, 50.0),
            "box {}",
            index + 1
        );
    }
}

/// wpt `anchor-fallback-invalidation.html`: a class adds `anchor-size()`
/// whose fallbacks equal the old sizes; the box still resizes to the
/// anchor.
#[test]
fn wpt_anchor_fallback_invalidation() {
    let mut page = Page::new(
        "#cb { position: relative; width: 200px; height: 200px; border: 1px solid black; }
         #anchor { anchor-name: --a; position: absolute; width: 40px; height: 30px;
                   left: 75px; top: 75px; }
         #anchored { position: absolute; width: 50px; height: 50px; }
         #anchored.change { width: anchor-size(--a width, 50px);
                            height: anchor-size(--a height, 50px); }",
    );
    let root = page.root();
    let cb = page.el(root, "view#cb", "");
    page.el(cb, "view#anchor", "");
    let anchored = page.el(cb, "view#anchored", "");
    page.layout();
    assert_eq!(page.abs(anchored).2, 50.0);
    page.doc.add_class(anchored, "change");
    page.layout();
    let (.., width, height) = page.abs(anchored);
    assert_eq!((width, height), (40.0, 30.0));
}

/// wpt `anchor-query-fallback.html`, every case but the two with an
/// `anchor-size()` inside an `anchor-size()` fallback (below): fallbacks
/// for a missing anchor and a wrong-axis side, percentage and nested
/// `calc()` fallbacks, and anchor functions inside fallbacks. Adaptation:
/// the container is a row flexbox explicitly (the file's `display: flex`);
/// two extra rows check the anchors the fallbacks read.
#[test]
fn wpt_anchor_query_fallback() {
    let mut page = Page::new(
        "#container { position: relative; flex-direction: row; flex-wrap: wrap;
                      width: 300px; }
         .flex-item { width: 100px; height: 50px; flex: auto; }
         #a1 { anchor-name: --a1; }
         #a2 { anchor-name: --a2; }
         .target { position: absolute; }",
    );
    let root = page.root();
    let container = page.el(root, "view#container", "");
    page.el(container, "view.flex-item#a1", "");
    for _ in 0..7 {
        page.el(container, "view.flex-item", "");
    }
    page.el(container, "view.flex-item#a2", "");
    // (inline style, which of x/y/width/height, expected)
    let cases: [(&str, usize, f32); 16] = [
        ("left: anchor(--inexist-anchor left, 50px)", 0, 50.0),
        ("width: anchor-size(--inexist-anchor width, 50px)", 2, 50.0),
        ("left: anchor(--a1 top, 50px)", 0, 50.0),
        ("left: anchor(--a1 bottom, 50px)", 0, 50.0),
        ("top: anchor(--a1 left, 50px)", 1, 50.0),
        ("top: anchor(--a1 right, 50px)", 1, 50.0),
        ("left: anchor(--inexist-anchor left, 50%)", 0, 150.0),
        (
            "left: anchor(--inexist-anchor left, calc(20% + 20px))",
            0,
            80.0,
        ),
        (
            "left: calc(anchor(--inexist-anchor left, calc(anchor(--inexist-anchor left, 20%) \
             + 20px)) + 20px)",
            0,
            100.0,
        ),
        (
            "left: calc(anchor(--inexist-anchor left, calc(anchor-size(--inexist-anchor width, \
             20%) + 20px)) + 20px)",
            0,
            100.0,
        ),
        ("top: anchor(--a1 left, anchor(--a2 top))", 1, 100.0),
        (
            "top: anchor(--a1 left, calc((anchor(--a1 bottom) + anchor(--a2 top)) / 2))",
            1,
            75.0,
        ),
        ("width: anchor-size(--inexist-anchor width, 50%)", 2, 150.0),
        (
            "width: anchor-size(--inexist-anchor width, calc(20% + 20px))",
            2,
            80.0,
        ),
        ("left: anchor(--a1 right)", 0, 100.0),
        ("top: anchor(--a2 top)", 1, 100.0),
    ];
    let targets: Vec<_> = cases
        .iter()
        .map(|&(inline, field, expected)| {
            (
                page.el(container, "view.target", inline),
                inline,
                field,
                expected,
            )
        })
        .collect();
    page.layout();
    for (target, inline, field, expected) in targets {
        let rect = page.offset(target, container);
        let got = [rect.0, rect.1, rect.2, rect.3][field];
        assert_eq!(got, expected, "{inline}");
    }
}

/// wpt `anchor-query-fallback.html`, the two cases whose `anchor-size()`
/// fallback is itself an `anchor-size()` (alone, or in `calc()`). The
/// fork's `anchor-size()` grammar accepts only a `<length-percentage>`
/// without anchor functions as its fallback, so both declarations are
/// dropped at parse time (`anchor()`'s fallback does accept them).
#[test]
#[ignore = "GAP: the fork does not parse an anchor function inside an anchor-size() fallback"]
fn wpt_anchor_query_fallback_nested_anchor_size() {
    let mut page = Page::new(
        "#container { position: relative; flex-direction: row; flex-wrap: wrap;
                      width: 300px; }
         .flex-item { width: 100px; height: 50px; flex: auto; }
         #a1 { anchor-name: --a1; }
         #a2 { anchor-name: --a2; }
         .target { position: absolute; }",
    );
    let root = page.root();
    let container = page.el(root, "view#container", "");
    page.el(container, "view.flex-item#a1", "");
    for _ in 0..7 {
        page.el(container, "view.flex-item", "");
    }
    page.el(container, "view.flex-item#a2", "");
    let plain = page.el(
        container,
        "view.target",
        "height: anchor-size(--inexist-anchor height, anchor-size(--a1 width))",
    );
    let calc = page.el(
        container,
        "view.target",
        "height: anchor-size(--inexist-anchor height, calc((anchor-size(--a1 width) + \
         anchor-size(--a2 height)) / 2))",
    );
    page.layout();
    assert_eq!(page.abs(plain).3, 100.0);
    assert_eq!(page.abs(calc).3, 75.0);
}

/// wpt `anchor-invalid-fallback.html`, every case: an unresolvable anchor
/// function without a fallback makes its declaration invalid at
/// computed-value time, so the box lays out as the reference with every
/// inset and size `unset`. Adaptation: the files compare `getComputedStyle`
/// values; this compares boxes (each holding a 10px content box for the
/// file's "X"), and the `max-*`/`margin` fallback cases compare geometry.
#[test]
#[allow(clippy::too_many_lines, reason = "the file's cases, one table")]
fn wpt_anchor_invalid_fallback() {
    let mut page = Page::new(
        "page { --top: top; }
         #cb { position: relative; width: 200px; height: 200px; border: 1px solid black; }
         #anchor { anchor-name: --a; position: absolute; width: 50px; height: 40px;
                   left: 75px; top: 75px; }
         .target { position: absolute; }
         @layer base { #revert { top: anchor(top); } }
         #revert { top: revert-layer; }
         @position-try --pt { left: 10px; top: anchor(top); }
         #flip { left: 9999px; position-try-fallbacks: --pt flip-block; }",
    );
    let root = page.root();
    let cb = page.el(root, "view#cb", "");
    page.el(cb, "view#anchor", "");
    let main = page.el(cb, "view#main", "");
    let reference = page.el(cb, "view.target", "");
    page.el(reference, "view", "width: 10px; height: 10px");
    let target = |page: &mut Page, spec: &str, inline: &str| {
        let target = page.el(main, spec, inline);
        page.el(target, "view", "width: 10px; height: 10px");
        target
    };
    let valid: [(&str, Rect4); 5] = [
        (
            "position-anchor: --a; left: anchor(right); top: anchor(top);
             width: anchor-size(width); height: anchor-size(height)",
            (125.0, 75.0, 50.0, 40.0),
        ),
        (
            "left: anchor(right, 17px); top: anchor(top, 18px);
             width: anchor-size(width, 42px); height: anchor-size(height, 43px)",
            (17.0, 18.0, 42.0, 43.0),
        ),
        (
            "left: anchor(right, 8.5%); top: calc(8.5% + anchor(top, 1px));
             width: anchor-size(width, 21%); height: calc(21% + anchor-size(height, 1px))",
            (17.0, 18.0, 42.0, 43.0),
        ),
        (
            "width: 200px; height: 200px; max-width: anchor-size(width, 28%);
             max-height: calc(28% + anchor-size(height, 1px))",
            (0.0, 0.0, 56.0, 57.0),
        ),
        (
            "margin-left: anchor-size(width, 6%); margin-top: calc(6% + anchor-size(height, 1px))",
            (12.0, 13.0, 10.0, 10.0),
        ),
    ];
    let valid: Vec<_> = valid
        .into_iter()
        .map(|(inline, expected)| (target(&mut page, "view.target", inline), inline, expected))
        .collect();
    let invalid = [
        "left: anchor(left)",
        "right: anchor(right)",
        "bottom: anchor(bottom)",
        "top: anchor(top)",
        "width: anchor-size(width)",
        "height: anchor-size(height)",
        "min-width: anchor-size(width)",
        "min-height: anchor-size(height)",
        "max-width: anchor-size(width)",
        "max-height: anchor-size(height)",
        "left: anchor(--unknown left)",
        "width: anchor-size(--unknown width)",
        "left: anchor(--a top)",
        "top: anchor(--a left)",
        "width: anchor(--a left)",
        "left: calc(anchor(left) + 10px)",
        "right: calc(anchor(right) + 10px)",
        "bottom: calc(anchor(bottom) + 10px)",
        "top: calc(anchor(top) + 10px)",
        "min-width: calc(anchor-size(width) + 10px)",
        "min-height: calc(anchor-size(height) + 10px)",
        "max-width: calc(anchor-size(width) + 10px)",
        "max-height: calc(anchor-size(height) + 10px)",
        "top: anchor(top, anchor(--unknown top))",
        "width: anchor-size(width, anchor-size(--unknown width))",
        "top: min(10px, anchor(top))",
        "top: max(10px, anchor(top))",
        "top: abs(anchor(top) - 100px)",
        "top: calc(sign(anchor(top) - 100px) * 20px)",
        "top: anchor(var(--top))",
        "top: anchor(var(--unknown, top))",
        "top: anchor(var(--unknown))",
    ];
    let invalid: Vec<_> = invalid
        .into_iter()
        .map(|inline| (target(&mut page, "view.target", inline), inline))
        .collect();
    let revert = target(&mut page, "view.target#revert", "");
    let flip = target(&mut page, "view.target#flip", "");
    page.layout();
    for (id, inline, expected) in valid {
        assert_eq!(page.offset(id, cb), expected, "{inline}");
    }
    let expected = page.offset(reference, cb);
    for (id, inline) in invalid {
        assert_eq!(page.offset(id, cb), expected, "{inline}");
    }
    assert_eq!(page.offset(revert, cb), expected, "revert-layer");
    assert_eq!(
        page.offset(flip, cb),
        (10.0, expected.1, expected.2, expected.3),
        "flip to an invalid anchor()"
    );
}

/// wpt `anchor-function-zero-fallback.html`, every case: a math function
/// that resolves to a unitless zero is a `<number>`, not a
/// `<length-percentage>`, so it is no fallback; a bare `0` is.
#[test]
fn wpt_anchor_function_zero_fallback() {
    for (property, value) in [
        ("width", "anchor-size(width, calc(0))"),
        ("width", "anchor-size(width, min(0, 0))"),
        ("height", "anchor-size(height, max(0))"),
        ("left", "anchor(left, calc(0))"),
    ] {
        assert!(!common::parses(property, value), "{property}: {value}");
    }
    for (property, value) in [
        ("width", "anchor-size(width, 0)"),
        ("width", "anchor-size(width, 10px)"),
    ] {
        assert!(common::parses(property, value), "{property}: {value}");
    }
}

/// wpt `anchor-inherited.html`: an anchor function inherits as the length
/// it resolved to. This engine keeps the function in the computed value and
/// resolves it at layout, only for an absolutely positioned box, so the
/// relatively positioned child's inherited `top`/`left`/`width`/`height`
/// are unresolvable there and take their initial values.
#[test]
#[ignore = "GAP: anchor functions inherit as functions, not as the resolved length (§28)"]
fn wpt_anchor_inherited() {
    let mut page = Page::new(
        ".cb { width: 400px; height: 400px; position: relative; border: 1px solid black; }
         .anchor { width: 100px; height: 100px; top: 10px; left: 20px; position: absolute;
                   anchor-name: --a; }
         .anchored { position-anchor: --a; position: absolute; top: anchor(top);
                     left: anchor(left); width: anchor-size(width);
                     height: anchor-size(height); }
         .child { position-anchor: --unknown; position: relative; top: inherit;
                  left: inherit; width: inherit; height: inherit; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "");
    page.el(cb, "view.anchor", "");
    let anchored = page.el(cb, "view.anchored", "");
    let child = page.el(anchored, "view.child", "");
    page.layout();
    let (x, y, ..) = page.abs(anchored);
    let (cx, cy, width, height) = page.abs(child);
    assert_eq!((cx - x, cy - y, width, height), (20.0, 10.0, 100.0, 100.0));
}

// ---------------------------------------------------------------------------
// Position fallback (§6.5) through the host.

/// wpt `position-try-002.html`: the base style overflows its inset-modified
/// containing block, so the first fallback that fits is used. Adaptation:
/// the inline-block spacer is a 200×100 child.
#[test]
fn wpt_position_try_002() {
    let mut page = Page::new(
        ".cb { width: 400px; height: 400px; transform: scale(1); }
         .anchor1 { anchor-name: --a; margin-left: 100px; width: 100px; height: 100px; }
         .target { position: absolute; position-try-fallbacks: --f1, --f2;
                   width: min-content; height: 100px; left: 0px; right: anchor(--a left);
                   top: anchor(--a top); }
         @position-try --f1 { left: anchor(--a right); right: 0px; top: anchor(--a top); }
         @position-try --f2 { inset: 0px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "");
    page.el(cb, "view.anchor1", "");
    let target = page.el(cb, "view.target", "");
    page.el(target, "view", "width: 200px; height: 100px");
    page.layout();
    assert_eq!(page.offset(target, cb), (200.0, 0.0, 200.0, 100.0));
}

/// wpt `position-try-003.html`, the first and third cases: options that
/// exceed the end edge or the size of the inset-modified containing block
/// are skipped. Not ported: the second case (`vertical-rl`).
#[test]
fn wpt_position_try_003() {
    let mut page = Page::new(
        ".cb { width: 200px; height: 200px; transform: scale(1); }
         .spacer { height: 50px; }
         .anchor { width: 100px; height: 100px; margin-left: 50px; anchor-name: --a; }
         .anchored { position: absolute; width: 50px; height: 50px; }
         .exceeds-end { position-try-fallbacks: --exceeds-end-1, --exceeds-end-2;
                        left: 0px; right: anchor(--a left); width: 100px; }
         @position-try --exceeds-end-1 { inset: auto; top: 0px; bottom: anchor(--a top);
                                         width: auto; height: 100px; }
         @position-try --exceeds-end-2 { inset: auto; top: 11px; left: 22px; width: auto;
                                         height: auto; }
         .exceeds-size { position-try-fallbacks: --exceeds-size-1, --exceeds-size-2;
                         top: anchor(--a bottom); left: auto; right: auto; width: 300px; }
         @position-try --exceeds-size-1 { inset: auto; left: anchor(--a right); width: auto;
                                          height: 300px; }
         @position-try --exceeds-size-2 { inset: auto; width: auto; top: 11px; left: 22px; }",
    );
    let root = page.root();
    let mut targets = Vec::new();
    for class in ["exceeds-end", "exceeds-size"] {
        let cb = page.el(root, "view.cb", "");
        page.el(cb, "view.spacer", "");
        page.el(cb, "view.anchor", "");
        targets.push((
            cb,
            page.el(cb, &format!("view.anchored.{class}"), ""),
            class,
        ));
    }
    page.layout();
    for (cb, target, class) in targets {
        let (x, y, ..) = page.offset(target, cb);
        assert_eq!((x, y), (22.0, 11.0), "{class}");
    }
}

/// wpt `position-try-004.html`: margins in an option; the used margins are
/// the chosen option's. Adaptation: the file's `data-expected-margin-*`
/// read the computed margins, which this engine's readback reports from
/// the chosen option.
#[test]
fn wpt_position_try_004() {
    let mut page = Page::new(
        ".cb { width: 300px; height: 150px; position: relative; }
         .anchor { position: absolute; width: 100px; height: 100px; top: 25px;
                   anchor-name: --a; }
         .target { position: absolute; width: 100px; height: 100px;
                   position-try-fallbacks: --fallback; top: anchor(--a top);
                   right: anchor(--a left); margin-top: 10px; margin-right: 10px; }
         @position-try --fallback { inset: auto; bottom: anchor(--a bottom);
                                    left: anchor(--a right); margin: 0px;
                                    margin-bottom: 10px; margin-left: 10px; }",
    );
    let root = page.root();
    let mut targets = Vec::new();
    for (anchor, x, margins) in [
        ("left: 110px", 0.0, ["0px", "10px", "10px", "0px"]),
        ("right: 110px", 200.0, ["10px", "0px", "0px", "10px"]),
    ] {
        let cb = page.el(root, "view.cb", "");
        page.el(cb, "view.anchor", anchor);
        targets.push((cb, page.el(cb, "view.target", ""), anchor, x, margins));
    }
    page.layout();
    for (cb, target, anchor, x, margins) in targets {
        assert_eq!(page.offset(target, cb).0, x, "{anchor}");
        for (side, expected) in ["margin-left", "margin-right", "margin-top", "margin-bottom"]
            .into_iter()
            .zip(margins)
        {
            assert_eq!(page.computed(target, side), expected, "{anchor}: {side}");
        }
    }
}

/// wpt `position-try-dynamic.html`: setting `position-try-fallbacks` after
/// the first layout moves the overflowing box to its option.
#[test]
fn wpt_position_try_dynamic() {
    let mut page = Page::new(
        "@position-try --fallback1 { left: anchor(--a1 right); }
         #anchor { anchor-name: --a1; width: 100px; height: 100px; }
         #anchored { position: absolute; left: 999999px; width: 100px; height: 100px; }",
    );
    let root = page.root();
    page.el(root, "view#anchor", "");
    let anchored = page.el(root, "view#anchored", "");
    page.layout();
    assert_eq!(page.abs(anchored).0, 999_999.0);
    page.doc
        .set_inline(anchored, "position-try-fallbacks: --fallback1");
    page.layout();
    assert_eq!(page.abs(anchored).0, 100.0);
}

/// wpt `position-try-fallbacks-limit.html`: names without a rule are not in
/// the options list, and at least five options are tried.
#[test]
fn wpt_position_try_fallbacks_limit() {
    let mut page = Page::new(
        "#container { position: relative; width: 200px; height: 200px; }
         .positioned { width: 200px; height: 200px; position: absolute; top: 0px;
                       left: 10px; }
         @position-try --bar { left: 0px; }
         #t1 { position-try-fallbacks: --foo, --foo, --foo, --foo, --foo, --foo, --foo,
                                       --bar; }
         @position-try --f1 { left: 10px; }
         @position-try --f2 { left: 10px; }
         @position-try --f3 { left: 10px; }
         @position-try --f4 { left: 10px; }
         @position-try --f5 { left: 20px; width: 20px; }
         #t2 { position-try-fallbacks: --f1, --f2, --f3, --f4, --f5; }",
    );
    let root = page.root();
    let container = page.el(root, "view#container", "");
    let t1 = page.el(container, "view.positioned#t1", "");
    let t2 = page.el(container, "view.positioned#t2", "");
    page.layout();
    assert_eq!(page.offset(t1, container).0, 0.0);
    assert_eq!(page.offset(t2, container).0, 20.0);
}

/// wpt `position-try-grid-001.html`: the options of a box whose containing
/// block is a grid area, anchored to an element inside a grid item.
#[test]
fn wpt_position_try_grid_001() {
    let mut page = Page::new(
        ".grid { display: grid; grid-template-columns: repeat(4, 100px);
                 grid-template-rows: 50px 100px 50px 50px; }
         .anchor1 { anchor-name: --a1; margin-left: 15px; width: 20px; height: 30px; }
         .target { grid-column: 2 / 4; grid-row: 2 / 4; position: absolute;
                   position-try-fallbacks: --f1, --f2, --f3; width: 100px; height: 100px;
                   position-anchor: --a1; right: anchor(left); top: anchor(top); }
         @position-try --f1 { inset: auto; left: anchor(right); top: anchor(top);
                              width: 250px; }
         @position-try --f2 { inset: auto; left: anchor(right); top: anchor(top); }
         @position-try --f3 { inset: auto; left: 0px; top: 0px; width: 0px; height: 0px; }",
    );
    let root = page.root();
    let wrapper = page.el(root, "view", "");
    page.el(wrapper, "view", "height: 10px");
    let grid = page.el(wrapper, "view.grid.cb", "");
    for index in 1..=16 {
        let item = page.el(grid, "view", "");
        if index == 6 {
            page.el(item, "view", "height: 20px");
            page.el(item, "view.anchor1", "");
        }
    }
    let target = page.el(grid, "view.target", "");
    page.layout();
    let (x, y, _, height) = page.offset(target, grid);
    assert_eq!((x, y, height), (135.0, 70.0, 100.0));
}

/// wpt `position-try-position-anchor.html`: an option changes
/// `position-anchor`, and the option's unnamed `anchor()` reads the new
/// default anchor.
#[test]
fn wpt_position_try_position_anchor() {
    let mut page = Page::new(
        "#cb { position: relative; width: 400px; height: 400px; }
         .anchor { width: 100px; height: 100px; }
         #anchor-a { anchor-name: --a; margin-left: 100px; }
         #anchor-b { anchor-name: --b; }
         #anchored { position: absolute; left: anchor(right); width: 300px; height: 100px;
                     position-anchor: --a; position-try-fallbacks: --pf; }
         @position-try --pf { position-anchor: --b; }",
    );
    let root = page.root();
    let cb = page.el(root, "view#cb", "");
    page.el(cb, "view.anchor#anchor-a", "");
    page.el(cb, "view.anchor#anchor-b", "");
    let anchored = page.el(cb, "view#anchored", "");
    page.layout();
    assert_eq!(page.offset(anchored, cb).0, 100.0);
}

/// wpt `position-try-order-inset-modified-containing-block.html`, every
/// case: margins are not part of the inset-modified containing block, so
/// `most-width`/`most-height` see a tie and keep list order. Adaptation:
/// one box laid out under each value in turn.
#[test]
fn wpt_position_try_order_inset_modified_containing_block() {
    let mut page = Page::new(
        "#cb { position: absolute; width: 400px; height: 400px; border: 1px solid black; }
         #target { position: absolute; left: 450px; height: 40px; }
         @position-try --margin { left: 0px; right: 0px; margin: 100px; }
         @position-try --no-margin { left: 0px; right: 0px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view#cb", "");
    let target = page.el(cb, "view#target", "");
    let at = |page: &mut Page, position_try: &str| {
        page.doc
            .set_inline(target, &format!("position-try: {position_try}"));
        page.layout();
        (
            page.offset(target, cb).0,
            page.computed(target, "margin-left"),
        )
    };
    for (position_try, expected) in [
        ("most-width --margin, --no-margin", "--margin"),
        ("most-width --no-margin, --margin", "--no-margin"),
        ("most-height --margin, --no-margin", "--margin"),
        ("most-height --no-margin, --margin", "--no-margin"),
    ] {
        let got = at(&mut page, position_try);
        let want = at(&mut page, expected);
        assert_eq!(got, want, "{position_try} | {expected}");
    }
}

/// wpt `position-try-order-position-area.html`, every case: the
/// `position-area` version of `position-try-order-basic.html`.
/// Adaptation: one box laid out under each value in turn.
#[test]
fn wpt_position_try_order_position_area() {
    let mut page = Page::new(
        "#cb { position: absolute; width: 400px; height: 400px; border: 1px solid black; }
         #anchor { position: absolute; left: 150px; top: 200px; width: 150px; height: 150px;
                   anchor-name: --a; }
         #target { position: absolute; left: 450px; width: 40px; height: 40px;
                   position-anchor: --a; align-self: start; justify-self: start; }
         @position-try --right { inset: unset; position-area: right; }
         @position-try --left { inset: unset; position-area: left; }
         @position-try --top { inset: unset; position-area: top; }
         @position-try --bottom { inset: unset; position-area: bottom; }
         @position-try --right-sweep { inset: unset; position-area: right center; }
         @position-try --left-sweep { inset: unset; position-area: left center; }
         @position-try --bottom-sweep { inset: unset; position-area: bottom center; }
         @position-try --top-sweep { inset: unset; position-area: top center; }",
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

/// wpt `position-try-cascade.html`, the rule, inline style, `!important`
/// and `revert`/`revert-layer` cases: the Position Fallback Origin sits
/// above author normal declarations (inline ones included) and below
/// `!important` ones; `revert` in it goes to the user origin and
/// `revert-layer` to the author origin. Not ported: the animation and
/// transition cases (they need a document timeline this harness does not
/// drive).
#[test]
fn wpt_position_try_cascade() {
    let mut page = Page::new(
        ".cb { position: relative; width: 100px; height: 100px; }
         .abs { position: absolute; left: 0px; top: 0px; width: 150px; height: 25px;
                position-try-fallbacks: --pf; }
         @position-try --pf { width: 50px; left: 50px; top: 50px; }
         #abs_important { left: 10px !important; }
         #abs_revert { position-try-fallbacks: --pf-revert; }
         @layer author-layer { #abs_revert { top: 30px; left: 30px; } }
         #abs_revert { top: 20px; left: 20px; width: 200px; height: 200px; }
         @position-try --pf-revert { left: revert; top: revert-layer; width: 30px;
                                     height: 30px; }",
    );
    let root = page.root();
    let mut cases = Vec::new();
    for (spec, inline, expected) in [
        ("view.abs#abs_try", "", (50.0, 50.0, 50.0, 25.0)),
        (
            "view.abs#abs_inline",
            "left: 20px",
            (50.0, 50.0, 50.0, 25.0),
        ),
        ("view.abs#abs_important", "", (10.0, 50.0, 50.0, 25.0)),
        ("view.abs#abs_revert", "", (0.0, 20.0, 30.0, 30.0)),
    ] {
        let cb = page.el(root, "view.cb", "");
        cases.push((cb, page.el(cb, spec, inline), spec, expected));
    }
    page.layout();
    for (cb, target, spec, expected) in cases {
        assert_eq!(page.offset(target, cb), expected, "{spec}");
    }
}

/// wpt `position-try-custom-property.html`, both cases: `var()` inside an
/// `@position-try` rule, in longhands and in the `inset` shorthand.
/// Adaptation: in a column flexbox the base style's static position is the
/// containing block's start, where the box fits; `top: 60px` makes it
/// overflow as the file's block-flow static position does.
#[test]
fn wpt_position_try_custom_property() {
    let mut page = Page::new(
        ".cb { position: relative; width: 195px; height: 70px; border-bottom: 1px solid black; }
         .spacer { width: 1px; height: 20px; }
         .anchor1 { anchor-name: --a1; margin-left: 45px; width: 100px; height: 30px; }
         .target { position: absolute; width: 40px; height: 15px; margin: 5px; top: 60px;
                   --left: anchor(--a1 right); --top: anchor(--a1 top); }
         .fallback1 { position-try-fallbacks: --fallback1; }
         .fallback2 { position-try-fallbacks: --fallback2; }
         @position-try --fallback1 { left: var(--left); top: var(--top); }
         @position-try --fallback2 { inset: var(--top) 0px 0px var(--left); }",
    );
    let root = page.root();
    let mut targets = Vec::new();
    for class in ["fallback1", "fallback2"] {
        let cb = page.el(root, "view.cb", "");
        page.el(cb, "view.spacer", "");
        page.el(cb, "view.anchor1", "");
        targets.push((cb, page.el(cb, &format!("view.target.{class}"), ""), class));
    }
    page.layout();
    for (cb, target, class) in targets {
        let (x, y, ..) = page.offset(target, cb);
        assert_eq!((x, y), (150.0, 25.0), "{class}");
    }
}

/// wpt `position-try-tree-scoped.html`, every case: `@position-try` names
/// are tree-scoped — a reference sees its own tree's rules and its
/// ancestors' trees', never a descendant's — and a `:host`, `::slotted()`
/// or `::part()` rule's reference resolves in the tree of its stylesheet.
#[test]
fn wpt_position_try_tree_scoped() {
    let mut page = Page::new(
        "@position-try --doc { left: 100px; }
         .abs { width: 100px; position: absolute; left: 999999px; }
         #doc_pf_doc { position-try-fallbacks: --doc; }
         #doc_pf_outer { position-try-fallbacks: --outer; }
         #doc_pf_inner { position-try-fallbacks: --inner; }
         #host_slotted_part { width: 100px; }
         @position-try --host-slot-part { left: 1px; }
         #host_slotted_part::part(part) { position-try-fallbacks: --host-slot-part; }",
    );
    let shadow_view = "view { display: flex; flex-direction: column; flex-shrink: 0; }";
    let root = page.root();
    let doc_boxes = [
        (page.el(root, "view.abs#doc_pf_doc", ""), 100.0),
        (page.el(root, "view.abs#doc_pf_outer", ""), 999_999.0),
        (page.el(root, "view.abs#doc_pf_inner", ""), 999_999.0),
    ];
    let outer_host = page.el(root, "view#outer_host", "");
    let outer = page.doc.dom.attach_shadow(outer_host, ShadowRootMode::Open);
    page.doc.dom.add_shadow_stylesheet(
        outer,
        &format!(
            "{shadow_view}
             @position-try --outer {{ left: 200px; }}
             .abs {{ position: absolute; left: 999999px; }}
             #outer_pf_doc {{ position-try-fallbacks: --doc; }}
             #outer_pf_outer {{ position-try-fallbacks: --outer; }}
             #outer_pf_inner {{ position-try-fallbacks: --inner; }}"
        ),
    );
    let outer_boxes = [
        (page.el(outer, "view.abs#outer_pf_doc", ""), 100.0),
        (page.el(outer, "view.abs#outer_pf_outer", ""), 200.0),
        (page.el(outer, "view.abs#outer_pf_inner", ""), 999_999.0),
    ];
    let inner_host = page.el(outer, "view#inner_host", "");
    let inner = page.doc.dom.attach_shadow(inner_host, ShadowRootMode::Open);
    page.doc.dom.add_shadow_stylesheet(
        inner,
        &format!(
            "{shadow_view}
             @position-try --inner {{ left: 300px; }}
             .abs {{ position: absolute; left: 999999px; }}
             #inner_pf_doc {{ position-try-fallbacks: --doc; }}
             #inner_pf_outer {{ position-try-fallbacks: --outer; }}
             #inner_pf_inner {{ position-try-fallbacks: --inner; }}"
        ),
    );
    let inner_boxes = [
        (page.el(inner, "view.abs#inner_pf_doc", ""), 100.0),
        (page.el(inner, "view.abs#inner_pf_outer", ""), 200.0),
        (page.el(inner, "view.abs#inner_pf_inner", ""), 300.0),
    ];
    let host = page.el(root, "view#host_slotted_part", "");
    let slotted = page.el(host, "view#slotted", "");
    let shadow = page.doc.dom.attach_shadow(host, ShadowRootMode::Open);
    page.doc.dom.add_shadow_stylesheet(
        shadow,
        &format!(
            "{shadow_view}
             @position-try --host-slot-part {{ left: 2px; }}
             ::slotted(#slotted), :host {{ position: absolute; left: 999999px;
                                          position-try-fallbacks: --host-slot-part; }}
             #part {{ position: absolute; left: 999999px; }}"
        ),
    );
    let part = page.el(shadow, "view#part", "");
    page.doc.set_attr(part, "part", "part");
    page.el(shadow, "slot", "");
    page.layout();
    for (index, (id, expected)) in doc_boxes
        .into_iter()
        .chain(outer_boxes)
        .chain(inner_boxes)
        .enumerate()
    {
        assert_eq!(page.abs(id).0, expected, "box {index}");
    }
    assert_eq!(page.abs(host).0, 2.0, ":host");
    assert_eq!(page.offset(slotted, host).0, 2.0, "::slotted()");
    assert_eq!(page.offset(part, host).0, 1.0, "::part()");
}

/// wpt `try-tactic-basic-anchor.html`, every case: `flip-block`,
/// `flip-inline` and both, on `anchor()` insets. Adaptation: the file reads
/// `getComputedStyle` insets; this reads the box's position.
#[test]
fn wpt_try_tactic_basic_anchor() {
    let mut page = Page::new(
        "#cb { position: absolute; width: 200px; height: 200px; border: 1px solid black; }
         #anchor { position: absolute; left: 100px; top: 100px; width: 50px; height: 50px;
                   anchor-name: --anchor; }
         .target { position: absolute; width: 100px; height: 100px;
                   position-try-fallbacks: flip-block, flip-inline, flip-block flip-inline; }
         #target1 { left: anchor(--anchor left); top: anchor(--anchor bottom); }
         #target2 { left: anchor(--anchor right); top: anchor(--anchor top); }
         #target3 { left: anchor(--anchor right); top: anchor(--anchor bottom); }",
    );
    let root = page.root();
    let cb = page.el(root, "view#cb", "");
    page.el(cb, "view#anchor", "");
    let targets = [
        (page.el(cb, "view.target#target1", ""), (100.0, 0.0)),
        (page.el(cb, "view.target#target2", ""), (0.0, 100.0)),
        (page.el(cb, "view.target#target3", ""), (0.0, 0.0)),
    ];
    page.layout();
    for (index, (target, expected)) in targets.into_iter().enumerate() {
        let (x, y, ..) = page.offset(target, cb);
        assert_eq!((x, y), expected, "target{}", index + 1);
    }
}

/// wpt `try-tactic-back-to-base.html`: nothing fits, so the base style
/// stays (readback and geometry).
#[test]
fn wpt_try_tactic_back_to_base() {
    let mut page = Page::new(
        "#cb { position: absolute; width: 400px; height: 200px; border: 1px solid black; }
         #target { position: absolute; width: 50px; height: 50px; left: 180px; top: 190px;
                   position-try-fallbacks: flip-block; }",
    );
    let root = page.root();
    let cb = page.el(root, "view#cb", "");
    let target = page.el(cb, "view#target", "");
    page.layout();
    assert_eq!(page.computed(target, "left"), "180px");
    assert_eq!(page.computed(target, "top"), "190px");
    assert_eq!(page.offset(target, cb), (180.0, 190.0, 50.0, 50.0));
}

/// wpt `try-tactic-base.html`: `flip-start` swaps the sizes, and the
/// readback reports the chosen option's.
#[test]
fn wpt_try_tactic_base() {
    let mut page = Page::new(
        "#cb { position: absolute; width: 400px; height: 200px; border: 1px solid black; }
         #target { position: absolute; width: 150px; height: 300px; border: 3px solid black; }",
    );
    let root = page.root();
    let cb = page.el(root, "view#cb", "");
    let target = page.el(cb, "view#target", "");
    page.layout();
    assert_eq!(page.computed(target, "width"), "150px");
    assert_eq!(page.computed(target, "height"), "300px");
    page.doc
        .set_inline(target, "position-try-fallbacks: flip-start");
    page.layout();
    assert_eq!(page.computed(target, "width"), "300px");
    assert_eq!(page.computed(target, "height"), "150px");
}

/// wpt `at-position-try-invalidation.html`, the first three cases: a rule
/// appearing later, and a later rule of the same name overriding it.
/// Adaptation: the rules arrive as added stylesheets (the file enables a
/// `media="print"` sheet and inserts into it). Not ported: the last case,
/// which disables the sheet (this document cannot remove one).
#[test]
fn wpt_at_position_try_invalidation() {
    let mut page = Page::new(
        "#anchor { anchor-name: --a; margin-left: 100px; width: 100px; height: 100px; }
         #anchored { position: absolute; width: 100px; height: 100px;
                     position-try-fallbacks: --pf; left: 999999px; }",
    );
    let root = page.root();
    let wrapper = page.el(root, "view", "");
    page.el(wrapper, "view#anchor", "");
    let anchored = page.el(wrapper, "view#anchored", "");
    page.layout();
    assert_eq!(page.abs(anchored).0, 999_999.0, "no rule");
    page.doc
        .add_css("@position-try --pf { left: anchor(--a left); }");
    page.layout();
    assert_eq!(page.abs(anchored).0, 100.0, "rule added");
    page.doc
        .add_css("@position-try --pf { left: anchor(--a right); }");
    page.layout();
    assert_eq!(page.abs(anchored).0, 200.0, "overriding rule");
}

/// wpt `at-position-try-invalidation-shadow-dom.html`: a rule added to a
/// shadow tree's styles reaches its `:host` and `::slotted()` references.
#[test]
fn wpt_at_position_try_invalidation_shadow_dom() {
    let mut page = Page::new("#host { width: 200px; }");
    let root = page.root();
    let host = page.el(root, "view#host", "");
    let slotted = page.el(host, "view#slotted", "");
    let shadow = page.doc.dom.attach_shadow(host, ShadowRootMode::Open);
    page.doc.dom.add_shadow_stylesheet(
        shadow,
        "::slotted(#slotted), :host { position-try-fallbacks: --pf; position: absolute;
                                      left: 999999px; }",
    );
    page.el(shadow, "slot", "");
    page.layout();
    assert_eq!(page.abs(host).0, 999_999.0);
    assert_eq!(page.offset(slotted, host).0, 999_999.0);
    page.doc
        .dom
        .add_shadow_stylesheet(shadow, "@position-try --pf { left: 100px; }");
    page.layout();
    assert_eq!(page.abs(host).0, 100.0);
    assert_eq!(page.offset(slotted, host).0, 100.0);
}

/// wpt `base-style-invalidation.html`: a base-style change that makes the
/// box overflow picks the `--pt flip-start` option; changing it back
/// returns to the base style.
#[test]
fn wpt_base_style_invalidation() {
    let mut page = Page::new(
        "@position-try --pt { width: 50px; }
         #cb { position: relative; width: 200px; height: 200px; border: 1px solid black; }
         #anchor { position: absolute; left: 75px; top: 75px; width: 50px; height: 50px;
                   anchor-name: --a; }
         #anchored { position: absolute; position-anchor: --a;
                     position-try-fallbacks: --pt flip-start; inset: 0px; top: anchor(top);
                     bottom: anchor(bottom); right: calc(anchor(left) + 5px); width: 50px;
                     height: 50px; justify-self: end; }
         #anchored.flip { width: 300px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view#cb", "");
    page.el(cb, "view#anchor", "");
    let anchored = page.el(cb, "view#anchored", "");
    let position = |page: &mut Page| {
        page.layout();
        let (x, y, ..) = page.offset(anchored, cb);
        (x, y)
    };
    assert_eq!(position(&mut page), (20.0, 75.0), "base");
    page.doc.add_class(anchored, "flip");
    assert_eq!(position(&mut page), (75.0, 20.0), "flipped");
    page.doc.remove_class(anchored, "flip");
    assert_eq!(position(&mut page), (20.0, 75.0), "base again");
}

/// The page `last-successful-change-fallbacks-position-area.html` and
/// `last-successful-fallback-to-base-style.html` share: a 600×300
/// container, a 100px anchor at (100, 100) and a 200×100 box left of it.
fn last_successful_page(fallbacks: &str) -> (Page, NodeId, NodeId, NodeId) {
    let mut page = Page::new(&format!(
        "#container {{ width: 600px; height: 300px; }}
         #anchor {{ position: relative; top: 100px; left: 100px; width: 100px; height: 100px;
                   anchor-name: --a; }}
         #anchored {{ position-anchor: --a; position-try-fallbacks: {fallbacks};
                     position: absolute; width: 200px; height: 100px;
                     position-area: left center; }}"
    ));
    let root = page.root();
    let container = page.el(root, "view.cb#container", "");
    let anchor = page.el(container, "view#anchor", "");
    let anchored = page.el(container, "view#anchored", "");
    (page, container, anchor, anchored)
}

/// wpt `last-successful-change-fallbacks.html`: changing
/// `position-try-fallbacks` forgets the last successful option.
#[test]
fn wpt_last_successful_change_fallbacks() {
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
    assert_eq!(page.offset(anchored, container).1, 200.0, "flip-block");
    page.doc.set_inline(anchor, "top: 150px");
    page.render();
    assert_eq!(page.offset(anchored, container).1, 200.0, "no fit: keep");
    page.doc
        .set_inline(anchored, "position-try-fallbacks: flip-block, --foo");
    page.render();
    page.render();
    assert_eq!(page.offset(anchored, container).1, 0.0, "forgotten");
}

/// wpt `last-successful-change-fallbacks-position-area.html`: setting
/// `position-try-fallbacks` to the value it already has is no change, so
/// the last successful option is kept; a different value forgets it.
#[test]
fn wpt_last_successful_change_fallbacks_position_area() {
    let (mut page, container, anchor, anchored) = last_successful_page("right center");
    page.render();
    assert_eq!(page.offset(anchored, container).0, 200.0, "fallback");
    page.doc.set_inline(anchor, "left: 300px");
    page.doc
        .set_inline(anchored, "position-try-fallbacks: right center");
    page.render();
    page.render();
    assert_eq!(
        page.offset(anchored, container).0,
        400.0,
        "same value: kept"
    );
    page.doc
        .set_inline(anchored, "position-try-fallbacks: right top");
    page.render();
    page.render();
    assert_eq!(page.offset(anchored, container).0, 100.0, "changed: base");
}

/// wpt `last-successful-fallback-to-base-style.html`: from a fallback back
/// to the base style, which then stays while both fit.
#[test]
fn wpt_last_successful_fallback_to_base_style() {
    let (mut page, container, anchor, anchored) = last_successful_page("flip-inline");
    page.render();
    assert_eq!(page.offset(anchored, container).0, 200.0, "flip-inline");
    page.doc.set_inline(anchor, "left: 350px");
    page.render();
    assert_eq!(page.offset(anchored, container).0, 150.0, "base fits");
    page.doc.set_inline(anchor, "left: 300px");
    page.render();
    assert_eq!(page.offset(anchored, container).0, 100.0, "base kept");
}

/// wpt `anchor-fallback-scroll-axis.html`: a `fixed` box shown while its
/// anchor's scroller is scrolled remembers that offset; a later horizontal
/// scroll shifts it off the viewport, which re-determines its fallback at
/// the next rendering update. The file reads `getBoundingClientRect()`,
/// which includes the default scroll shift; here both readings are taken
/// when the shift is zero (right after a recalculation point) or when the
/// chosen option does not compensate.
#[test]
fn wpt_anchor_fallback_scroll_axis() {
    let mut page = Page::new(
        "#scroller { overflow: scroll; width: 400px; height: 400px; border: 1px solid black; }
         #spacer { height: 1000px; width: 1000px; }
         #anchor { anchor-name: --anchor; width: 100px; height: 100px; margin-top: 150px;
                   margin-left: 150px; }
         #anchored { position: fixed; position-anchor: --anchor; right: anchor(left);
                     left: auto; top: 150px; width: 100px; height: 100px;
                     position-try-fallbacks: --fallback; display: none; }
         @position-try --fallback { left: 0px; right: auto; }",
    );
    let root = page.root();
    let scroller = page.el(root, "view#scroller", "");
    let spacer = page.el(scroller, "view#spacer", "");
    page.el(spacer, "view#anchor", "");
    let anchored = page.el(root, "view#anchored", "");
    page.render();
    page.doc
        .dom
        .scroll_to(scroller, dom::Vector2D::new(0.0, 400.0));
    page.render();
    page.doc.set_inline(anchored, "display: flex");
    page.render();
    assert_eq!(page.abs(anchored).0, 51.0, "fits at its remembered offset");
    page.doc
        .dom
        .scroll_to(scroller, dom::Vector2D::new(300.0, 400.0));
    page.render();
    assert_eq!(page.abs(anchored).0, 0.0, "the fallback after the scroll");
}

// ---------------------------------------------------------------------------
// anchor-scope (§2.2): the remaining cases.

/// The styles `anchor-scope-basic.html`, `-dynamic.html` and
/// `-display-contents.tentative.html` share; `scope_display` is appended to
/// every scope class.
fn scope_css(scope_display: &str) -> String {
    format!(
        ".scope-all {{ anchor-scope: all; {scope_display} }}
         .scope-a {{ anchor-scope: --a; {scope_display} }}
         .scope-b {{ anchor-scope: --b; {scope_display} }}
         .scope-ab {{ anchor-scope: --a, --b; {scope_display} }}
         .anchor-a {{ anchor-name: --a; }}
         .anchor-b {{ anchor-name: --b; }}
         .anchor-ab {{ anchor-name: --a, --b; }}
         .anchor-a, .anchor-b, .anchor-ab {{ height: 10px; }}
         .anchored-a {{ position-anchor: --a; }}
         .anchored-b {{ position-anchor: --b; }}
         .anchored-a, .anchored-b {{ position: absolute; top: anchor(bottom);
                                     left: anchor(left); width: 5px; height: 5px; }}
         .abs {{ position: absolute; width: 5px; height: 5px; }}"
    )
}

/// One `anchor-scope-*` template inside a fresh 100px `main`: answers each
/// query box marked `=> 0` in the outline with its `(left, top)`.
fn scope_case(css: &str, template: &str) -> Vec<(f32, f32)> {
    let mut page = Page::new(css);
    let root = page.root();
    let main = page.el(
        root,
        "view.cb",
        "width: 100px; height: 100px; border: 1px solid black",
    );
    let queries = outline(&mut page, main, template);
    page.layout();
    queries
        .into_iter()
        .map(|(id, _)| {
            let (x, y, ..) = page.offset(id, main);
            (x, y)
        })
        .collect()
}

/// wpt `anchor-scope-basic.html`, the cases `wpt_anchor_scope_basic` does
/// not cover: a scope for one name leaves the others alone, and
/// out-of-flow anchors inside and outside a scope.
#[test]
fn wpt_anchor_scope_basic_names_and_out_of_flow() {
    let css = scope_css("");
    let cases: [ScopeCase; 5] = [
        (
            "--a scopes only --a",
            "
            view.anchor-b
            view.anchor-a
            view.scope-a
              view.anchor-b
              view.anchor-ab
              view.anchor-a
            view.anchored-a => 0
            view.anchored-b => 0",
            &[(0.0, 20.0), (0.0, 40.0)],
        ),
        (
            "--b scopes only --b",
            "
            view.anchor-b
            view.anchor-a
            view.scope-b
              view.anchor-a
              view.anchor-b
            view.anchored-a => 0
            view.anchored-b => 0",
            &[(0.0, 30.0), (0.0, 10.0)],
        ),
        (
            "out-of-flow anchors",
            "
            view.anchor-b.abs | left: 10px
            view.anchor-a.abs | left: 20px
            view.scope-a
              view.anchor-b.abs | left: 30px
              view.anchor-a.abs | left: 40px
            view.anchored-a => 0
            view.anchored-b => 0",
            &[(20.0, 5.0), (30.0, 5.0)],
        ),
        (
            "out-of-flow and in-flow anchors",
            "
            view.anchor-b
            view.anchor-a
            view.scope-a
              view.anchor-b
              view.anchor-a.abs | top: 50px
            view.anchored-a => 0
            view.anchored-b => 0",
            &[(0.0, 20.0), (0.0, 30.0)],
        ),
        (
            "out-of-flow and in-flow anchors, reverse",
            "
            view.anchor-b
            view.anchor-a.abs | top: 50px
            view.scope-a
              view.anchor-b
              view.anchor-a
            view.anchored-a => 0
            view.anchored-b => 0",
            &[(0.0, 55.0), (0.0, 20.0)],
        ),
    ];
    for (case, template, expected) in cases {
        assert_eq!(scope_case(&css, template), expected, "{case}");
    }
}

/// wpt `anchor-scope-dynamic.html`, the last two cases: a scope for a name
/// nobody references changes nothing, and a scope for `--a` appearing and
/// going away moves only the `--a` reader.
#[test]
fn wpt_anchor_scope_dynamic_names() {
    let css = scope_css("");
    // "anchor-scope:--b appearing dynamically (--b never referenced)".
    {
        let mut page = Page::new(&css);
        let root = page.root();
        let main = page.el(root, "view.cb", "width: 100px; height: 100px");
        page.el(main, "view.anchor-a", "");
        page.el(main, "view.anchor-a", "");
        let dynamic = page.el(main, "view", "");
        page.el(dynamic, "view.anchor-a", "");
        page.el(dynamic, "view.anchor-a", "");
        let anchored = page.el(main, "view.anchored-a", "");
        for scope in ["", "anchor-scope: --b", ""] {
            page.doc.set_inline(dynamic, scope);
            page.layout();
            assert_eq!(page.offset(anchored, main).1, 40.0, "{scope:?}");
        }
    }
    // "anchor-scope:--a appearing dynamically scopes only --a".
    {
        let mut page = Page::new(&css);
        let root = page.root();
        let main = page.el(root, "view.cb", "width: 100px; height: 100px");
        page.el(main, "view.anchor-b", "");
        page.el(main, "view.anchor-a", "");
        let dynamic = page.el(main, "view", "");
        page.el(dynamic, "view.anchor-b", "");
        page.el(dynamic, "view.anchor-a", "");
        let a = page.el(main, "view.anchored-a", "");
        let b = page.el(main, "view.anchored-b", "");
        for (scope, expected_a) in [("", 40.0), ("anchor-scope: --a", 20.0), ("", 40.0)] {
            page.doc.set_inline(dynamic, scope);
            page.layout();
            assert_eq!(page.offset(a, main).1, expected_a, "{scope:?}: --a");
            assert_eq!(page.offset(b, main).1, 30.0, "{scope:?}: --b");
        }
    }
}

/// wpt `anchor-scope-display-contents.tentative.html`, every case: a
/// `display: contents` scope still scopes its subtree; an anchor that is
/// itself `display: contents` names nothing (the first case). The file
/// reads `getComputedStyle().top`; this reads the box's position, which
/// for an unresolvable `anchor()` is its static position.
#[test]
#[allow(clippy::too_many_lines, reason = "the file's ten templates, one table")]
fn wpt_anchor_scope_display_contents() {
    let css = scope_css("display: contents;");
    let cases: [ScopeCase; 10] = [
        (
            "defined and scoped by the same element",
            "
            view.scope-a.anchor-a
              view.anchored-a => 0",
            &[(0.0, 0.0)],
        ),
        (
            "sibling cannot anchor into the scope",
            "
            view.anchor-a
            view.anchor-a
            view.anchor-a
            view.scope-a.anchor-a
            view.anchored-a => 0",
            &[(0.0, 30.0)],
        ),
        (
            "all on common ancestor",
            "
            view.scope-all
              view.anchor-a
              view.anchor-a
              view.anchor-a
              view.anchor-a
              view.anchored-a => 0",
            &[(0.0, 40.0)],
        ),
        (
            "--a on common ancestor",
            "
            view.scope-a
              view.anchor-a
              view.anchor-a
              view.anchor-a
              view.anchor-a
              view.anchored-a => 0",
            &[(0.0, 40.0)],
        ),
        (
            "all on sibling",
            "
            view.anchor-a
            view.anchor-a
            view.scope-all
              view.anchor-a
              view.anchor-a
            view.anchored-a => 0",
            &[(0.0, 20.0)],
        ),
        (
            "all scopes multiple names",
            "
            view.anchor-b
            view.anchor-a
            view.scope-all
              view.anchor-b
              view.anchor-a
            view.anchored-a => 0
            view.anchored-b => 0",
            &[(0.0, 20.0), (0.0, 10.0)],
        ),
        (
            "--a, --b scopes both",
            "
            view.anchor-b
            view.anchor-a
            view.scope-ab
              view.anchor-b
              view.anchor-a
            view.anchored-a => 0
            view.anchored-b => 0",
            &[(0.0, 20.0), (0.0, 10.0)],
        ),
        (
            "--a scopes only --a",
            "
            view.anchor-b
            view.anchor-a
            view.scope-a
              view.anchor-b
              view.anchor-ab
              view.anchor-a
            view.anchored-a => 0
            view.anchored-b => 0",
            &[(0.0, 20.0), (0.0, 40.0)],
        ),
        (
            "--b scopes only --b",
            "
            view.anchor-b
            view.anchor-a
            view.scope-b
              view.anchor-a
              view.anchor-b
            view.anchored-a => 0
            view.anchored-b => 0",
            &[(0.0, 30.0), (0.0, 10.0)],
        ),
        (
            "out-of-flow anchors",
            "
            view.anchor-b.abs | left: 10px
            view.anchor-a.abs | left: 20px
            view.scope-a
              view.anchor-b.abs | left: 30px
              view.anchor-a.abs | left: 40px
            view.anchored-a => 0
            view.anchored-b => 0",
            &[(20.0, 5.0), (30.0, 5.0)],
        ),
    ];
    for (case, template, expected) in cases {
        assert_eq!(scope_case(&css, template), expected, "{case}");
    }
}

/// wpt `anchor-scope-shadow-flat-tree.html`: a scope in a shadow tree
/// covers a slotted box, because scoping follows the flat tree.
#[test]
fn wpt_anchor_scope_shadow_flat_tree() {
    let mut page = Page::new("");
    let root = page.root();
    let host = page.el(root, "view#host", "");
    let outer = page.el(host, "view.outer_anchored", "");
    let shadow = page.doc.dom.attach_shadow(host, ShadowRootMode::Open);
    page.doc.dom.add_shadow_stylesheet(
        shadow,
        "view { display: flex; flex-direction: column; flex-shrink: 0; }
         ::slotted(.outer_anchored), .inner_anchored { position: absolute;
             top: anchor(bottom, 1px); position-anchor: --a; width: 5px; height: 5px; }
         .anchor { height: 10px; anchor-name: --a; }
         .cb { position: relative; width: 200px; height: 200px; border: 1px solid black; }
         .scope { anchor-scope: --a; }",
    );
    let cb = page.el(shadow, "view.cb", "");
    page.el(cb, "view.anchor", "");
    let scope = page.el(cb, "view.scope", "");
    page.el(scope, "view.anchor", "");
    page.el(scope, "slot", "");
    let inner = page.el(cb, "view.inner_anchored", "");
    page.layout();
    assert_eq!(page.offset(outer, cb).1, 20.0, "slotted, inside the scope");
    assert_eq!(page.offset(inner, cb).1, 10.0, "outside the scope");
}

/// wpt `chrome-443261872.html`: an `anchor-name` in a shadow element's
/// inline style keeps its tree scope when another inline property changes.
#[test]
fn wpt_chrome_443261872() {
    let mut page = Page::new("");
    let root = page.root();
    let host = page.el(root, "view#host", "");
    let shadow = page.doc.dom.attach_shadow(host, ShadowRootMode::Open);
    page.doc.dom.add_shadow_stylesheet(
        shadow,
        "view { display: flex; flex-direction: column; flex-shrink: 0; }
         #anchor { width: 100px; height: 100px; }
         #anchored { position-anchor: --panel-anchor; position: absolute;
                     inset: anchor(top) anchor(right) anchor(bottom) anchor(left); }",
    );
    let anchor = page.el(
        shadow,
        "view#anchor",
        "anchor-name: --panel-anchor; color: yellow",
    );
    let anchored = page.el(shadow, "view#anchored", "");
    page.layout();
    assert_eq!(page.abs(anchored).2, 100.0);
    page.doc
        .set_inline(anchor, "anchor-name: --panel-anchor; color: pink");
    page.layout();
    assert_eq!(page.abs(anchored).2, 100.0);
}

// ---------------------------------------------------------------------------
// Reftests reinterpreted as geometry.

/// wpt `position-try-fallbacks-001.html` and `-002.html`: `flip-block` on
/// `bottom: anchor(outside)` and `bottom: anchor(inside)`. Reftest (a green
/// 100px square) → the box's geometry.
#[test]
fn wpt_position_try_fallbacks_001_002() {
    for (side, height, expected) in [
        ("outside", 80.0, (0.0, 20.0, 100.0, 80.0)),
        ("inside", 100.0, (0.0, 0.0, 100.0, 100.0)),
    ] {
        let mut page = Page::new("#anchor { anchor-name: --a; height: 20px; }");
        let root = page.root();
        let cb = page.el(root, "view.cb", "width: 100px; height: 100px");
        page.el(cb, "view#anchor", "");
        let target = page.el(
            cb,
            "view",
            &format!(
                "position: absolute; position-anchor: --a; bottom: anchor({side});
                 position-try-fallbacks: flip-block; width: 100px; height: {height}px"
            ),
        );
        page.layout();
        assert_eq!(page.offset(target, cb), expected, "{side}");
    }
}

/// wpt `anchor-name-006.html` and `-007.html`: an anchor inside two and
/// three nested `fixed` boxes is acceptable to a `fixed` box outside them.
/// Reftest → the box's size.
#[test]
fn wpt_anchor_name_006_007() {
    for depth in [2, 3] {
        let mut page = Page::new(
            "#overlay { position: fixed; position-anchor: --a; width: anchor-size(width);
                        height: anchor-size(height); }",
        );
        let root = page.root();
        let mut parent = root;
        for _ in 0..depth {
            parent = page.el(parent, "view", "position: fixed");
        }
        page.el(
            parent,
            "view",
            "anchor-name: --a; width: 100px; height: 100px",
        );
        let overlay = page.el(root, "view#overlay", "");
        page.layout();
        let (.., width, height) = page.abs(overlay);
        assert_eq!((width, height), (100.0, 100.0), "depth {depth}");
    }
}

/// wpt `no-anchor-anchor-center.html`: the only `--dropdownAnchor` is not
/// acceptable to a box inside a `fixed` container (neither it nor the
/// element generating its containing block shares the box's containing
/// block), so `anchor-center` is `center`. Reftest → geometry, with
/// `box-sizing: content-box` as in the file.
#[test]
fn wpt_no_anchor_anchor_center() {
    let mut page = Page::new(
        ".anchor { anchor-name: --dropdownAnchor; width: 100px; height: 20px; }
         .container { position: fixed; top: 20px; left: 50px; width: 300px; height: 200px; }
         .target { position-anchor: --dropdownAnchor; left: 10px; right: 10px;
                   position: absolute; justify-self: anchor-center; width: 80px;
                   height: 20px; border: 1px solid black; box-sizing: content-box; }",
    );
    let root = page.root();
    page.el(root, "view.anchor", "");
    let container = page.el(root, "view.container", "");
    let target = page.el(container, "view.target", "");
    page.layout();
    let (x, _, width, _) = page.offset(target, container);
    assert_eq!((x, width), (109.0, 82.0));
}

/// wpt `position-area-no-default-anchor.html`: `position-area` without a
/// default anchor does nothing. Reftest → geometry.
#[test]
fn wpt_position_area_no_default_anchor() {
    let mut page = Page::new(".abspos { position: absolute; width: 100px; height: 100px; }");
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 100px; height: 200px");
    page.el(cb, "view.abspos", "");
    let area = page.el(cb, "view.abspos", "position-area: left");
    page.layout();
    assert_eq!(page.offset(area, cb), (0.0, 0.0, 100.0, 100.0));
}

/// wpt `anchor-in-css-min-max-function.html`: `anchor()` inside `min()` and
/// `max()` with several arguments. Reftest → geometry.
#[test]
fn wpt_anchor_in_css_min_max_function() {
    let mut page = Page::new(
        ".container { display: grid; grid-template-columns: repeat(3, 100px); gap: 10px; }
         .box { width: 100px; height: 100px; }
         #anchor1 { anchor-name: --anchor1; }
         #anchor2 { anchor-name: --anchor2; }
         #anchor3 { anchor-name: --anchor3; }
         #target { position: absolute;
                   top: min(anchor(--anchor1 bottom), anchor(--anchor2 bottom),
                            anchor(--anchor3 top));
                   left: max(anchor(--anchor1 left), anchor(--anchor2 left),
                             anchor(--anchor3 left)); }",
    );
    let root = page.root();
    let container = page.el(root, "view.container", "");
    page.el(container, "view.box#anchor1", "");
    page.el(container, "view.box#anchor2", "");
    let anchor3 = page.el(container, "view.box#anchor3", "");
    let target = page.el(root, "view.box#target", "");
    page.layout();
    assert_eq!(page.abs(target), page.abs(anchor3));
}

/// wpt `sticky-anchor-position-invalid.html`: `anchor()` on a sticky box is
/// always unresolvable, so its fallback is the sticky inset. Reftest → the
/// box's position after the scroll.
#[test]
fn wpt_sticky_anchor_position_invalid() {
    let mut page = Page::new(
        "#scroll-container { width: 200px; height: 200px; overflow: scroll; }
         #scroller { height: 400px; }
         #sticky { position: sticky; height: 150px; top: anchor(--invalid top, 42px); }",
    );
    let root = page.root();
    let container = page.el(root, "view#scroll-container", "");
    let scroller = page.el(container, "view#scroller", "");
    let sticky = page.el(scroller, "view#sticky", "");
    page.layout();
    page.doc
        .dom
        .scroll_to(container, dom::Vector2D::new(0.0, 50.0));
    assert_eq!(page.abs(sticky).1, 42.0);
}

/// wpt `anchor-position-non-anchored-fallback.html`: after a layout picked
/// the fallback, the anchor shrinks and the base style fits again at the
/// rendering update. Reftest → geometry, the viewport 600px tall.
#[test]
fn wpt_anchor_position_non_anchored_fallback() {
    let mut page = Page::new(
        "#anchor { anchor-name: --anchor; width: 100px; height: 600px; }
         #anchored { top: anchor(--anchor bottom); width: 100px; height: 50px;
                     position: absolute; position-try: --bottom; }
         @position-try --bottom { top: auto; bottom: 0px; }",
    );
    let root = page.root();
    let anchor = page.el(root, "view#anchor", "");
    let anchored = page.el(root, "view#anchored", "");
    page.layout();
    assert_eq!(page.abs(anchored).1, 550.0, "the fallback");
    page.doc.set_inline(anchor, "height: 50px");
    page.render();
    assert_eq!(page.abs(anchored).1, 50.0, "the base again");
}

/// wpt `under-invalidation.html`: the base style's `min-height` overflows,
/// the option's does not. Reftest → geometry.
#[test]
fn wpt_under_invalidation() {
    let mut page = Page::new(
        "#container { position: relative; width: 100px; height: 100px; }
         #anchor { position: absolute; anchor-name: --a; width: 100px; height: 10px;
                   top: 90px; }
         #target { position: absolute; width: 100px; min-height: 150px;
                   bottom: anchor(--a top); position-try-fallbacks: --fallback; }
         @position-try --fallback { min-height: 90px; }",
    );
    let root = page.root();
    let container = page.el(root, "view#container", "");
    page.el(container, "view#anchor", "");
    let target = page.el(container, "view#target", "");
    page.layout();
    assert_eq!(page.offset(target, container), (0.0, 0.0, 100.0, 90.0));
}

/// wpt `anchor-center-002.html`, the flexbox half: `anchor-center` on an
/// in-flow item is `center` (§4.2: "If the box is not absolutely
/// positioned … this value behaves as center"). Reftest → geometry. The
/// grid half uses plain `center` in the file; `in_flow_anchor_center_is_center_in_grid`
/// runs the keyword itself there.
#[test]
fn wpt_anchor_center_002() {
    let mut page = Page::new("");
    let root = page.root();
    let container = page.el(
        root,
        "view",
        "flex-direction: row; width: 100px; height: 100px",
    );
    let item = page.el(
        container,
        "view",
        "width: 40px; height: 40px; align-self: anchor-center",
    );
    page.layout();
    assert_eq!(page.offset(item, container).1, 30.0);
}

/// §4.2's in-flow rule on a grid item, both axes: `anchor-center` is
/// `center`.
#[test]
fn in_flow_anchor_center_is_center_in_grid() {
    let mut page = Page::new("");
    let root = page.root();
    let container = page.el(
        root,
        "view",
        "display: grid; grid-template-columns: 100px; grid-template-rows: 100px",
    );
    let item = page.el(
        container,
        "view",
        "width: 40px; height: 20px; justify-self: anchor-center; align-self: anchor-center",
    );
    page.layout();
    assert_eq!(page.offset(item, container), (30.0, 40.0, 40.0, 20.0));
}

/// wpt `inherit-height-from-fallback.html`: a child inherits `height` from
/// the chosen option. This engine lays the chosen option's accepted
/// properties out on the box itself but cascades its descendants from the
/// base style, so the child inherits `auto`. Reftest → geometry.
#[test]
#[ignore = "GAP: descendants inherit from the base style, not the chosen position option (§28)"]
fn wpt_inherit_height_from_fallback() {
    let mut page = Page::new(
        "#anchor { anchor-name: --a1; width: 0px; height: 100px; }
         #anchored { position-area: left center; position: absolute; position-anchor: --a1;
                     position-try-fallbacks: --f1; width: 100px; }
         #child { height: inherit; }
         @position-try --f1 { position-area: right center; height: 100px; }",
    );
    let root = page.root();
    let container = page.el(root, "view.cb", "");
    page.el(container, "view#anchor", "");
    let anchored = page.el(container, "view#anchored", "");
    let child = page.el(anchored, "view#child", "");
    page.layout();
    assert_eq!(page.abs(anchored).3, 100.0, "the option's height");
    assert_eq!(page.abs(child).3, 100.0, "inherited from the option");
}

/// Not a WPT port: layout places the box against the remembered scroll
/// offsets and the painter applies the default scroll shift, but
/// `bounding_client_rect` (the `boundingClientRect` UI method,
/// `getBoundingClientRect`) adds the shift, as browsers do — for the box
/// and for its descendants.
#[test]
fn bounding_client_rect_includes_the_default_scroll_shift() {
    let mut page = Page::new(
        ".scroller { overflow: scroll; width: 200px; height: 100px; }
         .filler { height: 500px; }
         .anchor { anchor-name: --a; width: 40px; height: 30px; margin-top: 50px; }
         .anchored { position: absolute; position-anchor: --a; position-area: bottom;
                     width: 10px; height: 10px; }",
    );
    let root = page.root();
    let cb = page.el(root, "view.cb", "width: 400px; height: 400px");
    let scroller = page.el(cb, "view.scroller", "");
    let content = page.el(scroller, "view", "");
    let anchor = page.el(content, "view.anchor", "");
    page.el(content, "view.filler", "");
    let anchored = page.el(cb, "view.anchored", "");
    let child = page.el(anchored, "view", "height: 5px");
    page.render();
    assert_eq!(page.abs(anchored).1, 80.0);
    assert_eq!(page.abs(child).1, 80.0);
    page.doc
        .dom
        .scroll_to(scroller, dom::Vector2D::new(0.0, 20.0));
    page.render();
    assert_eq!(page.abs(anchor).1, 30.0, "the anchor scrolled");
    assert_eq!(page.abs(anchored).1, 60.0, "the box follows it");
    assert_eq!(page.abs(child).1, 60.0, "and so does its content");
}

/// wpt `registered-custom-property-anchor.html`, both cases: `anchor()` and
/// `anchor-size()` are not valid values of a registered custom property
/// accepting `<length>`, `<length-percentage>` or `<number>`, so each
/// computes to its initial value.
#[test]
fn wpt_registered_custom_property_anchor() {
    let mut page = Page::new(
        "@property --length { syntax: \"<length>\"; inherits: false; initial-value: 0px; }
         @property --length-percentage { syntax: \"<length-percentage>\"; inherits: false;
                                          initial-value: 0px; }
         @property --number { syntax: \"<number>\"; inherits: false; initial-value: 0; }
         #anchor { --length: anchor(--foo bottom, 5px);
                   --length-percentage: anchor(--foo bottom, 10%);
                   --number: sign(anchor(--foo bottom, 100px)); }
         #anchor-size { --length: anchor-size(--foo block, 7px);
                        --length-percentage: anchor-size(--foo block, 20%);
                        --number: sign(anchor-size(--foo block, 100px)); }",
    );
    let root = page.root();
    let anchor = page.el(root, "view#anchor", "");
    let anchor_size = page.el(root, "view#anchor-size", "");
    page.layout();
    for (id, case) in [(anchor, "anchor()"), (anchor_size, "anchor-size()")] {
        assert_eq!(page.computed(id, "--length"), "0px", "{case}");
        assert_eq!(page.computed(id, "--length-percentage"), "0px", "{case}");
        assert_eq!(page.computed(id, "--number"), "0", "{case}");
    }
}
