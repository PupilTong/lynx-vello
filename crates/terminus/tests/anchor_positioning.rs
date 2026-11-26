//! css-anchor-position-1 through a real document: the `terminus` host's target
//! anchor lookup (§2.3), default anchors (§2.4), anchor relevance (§2.5),
//! position options cascaded at the style harvest (§6), the last successful
//! position option (§6.5.1), the scrollable containing block
//! (css-position-4) and the settle loop (`crates/terminus/src/layout/anchors.rs`).
//! `seldon`'s own rules are pinned against a mock host in
//! `crates/seldon/tests/anchor_positioning.rs`; `anchor-size()` has
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
//! not implemented, and `terminus` has no popover or dialog element);
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
//! and `crates/terminus/src/visual/anchored.rs` cover §3.3 and §6.6);
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
use terminus::link::{NodeId, ShadowRootMode};

type Rect4 = (f32, f32, f32, f32);

type BorderCase = (
    &'static str,
    Option<&'static str>,
    &'static str,
    &'static str,
    Rect4,
);

type MatchParentCase = (Vec<(&'static str, &'static str)>, &'static str, bool);

type MutationCase = (
    &'static [usize],
    (f32, f32),
    &'static [(usize, bool)],
    (f32, f32),
);

type ScopeCase = (&'static str, &'static str, &'static [(f32, f32)]);

struct Page {
    doc: Doc,
}

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

    fn render(&mut self) {
        self.doc.dom.render();
    }

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
    assert_eq!(page.offset(above, cb), (140.0, 80.0, 20.0, 20.0));
    assert_eq!(page.offset(below_right, cb), (200.0, 200.0, 20.0, 20.0));
    assert_eq!(page.offset(no_anchor, cb), (0.0, 0.0, 20.0, 20.0));
}

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
    let specs: Vec<MatchParentCase> = vec![
        (vec![("view.parent", "position-anchor: --a")], "", true),
        (vec![("view.parent", "position-anchor: none")], "", false),
        (
            vec![("view.parent", "position-anchor: --nonexistent")],
            "",
            false,
        ),
        (
            vec![("view.parent", "position: relative; position-anchor: --a")],
            "",
            false,
        ),
        (
            vec![
                ("view.parent", "position-anchor: --a"),
                ("view.parent", "position-anchor: match-parent"),
            ],
            "",
            true,
        ),
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
        (
            vec![
                ("view.parent", "position-anchor: none"),
                ("view.parent", "position-anchor: match-parent"),
            ],
            "",
            false,
        ),
        (
            vec![("view.parent", "position-anchor: --a"), ("view.parent", "")],
            "",
            false,
        ),
        (
            vec![
                ("view.parent", "position-anchor: --a"),
                ("view", ""),
                ("view", ""),
            ],
            "",
            false,
        ),
        (
            vec![
                ("view.parent", "position-anchor: --a"),
                ("view", "position-anchor: match-parent"),
            ],
            "",
            false,
        ),
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

#[test]
fn an_anchored_box_escaping_an_in_place_relayout_keeps_its_scrollable_containing_block() {
    const CSS: &str = ".scroller { overflow: hidden; width: 100px; height: 100px; }
         .anchor { anchor-name: --a; width: 10px; height: 10px; }
         .wide { width: 300px; height: 10px; }
         .frame { width: 50px; height: 50px; min-width: 0px; min-height: 0px; }
         .target { position: absolute; position-anchor: --a; left: 0px; right: 0px;
                   height: 10px; }";
    let build = |target_inline: &str| {
        let mut page = Page::new(CSS);
        let root = page.root();
        let scroller = page.el(root, "view.cb.scroller", "");
        page.el(scroller, "view.anchor", "");
        page.el(scroller, "view.wide", "");
        let frame = page.el(scroller, "view.frame", "");
        let wrapper = page.el(frame, "view", "");
        let target = page.el(wrapper, "view.target", target_inline);
        page.layout();
        (page, scroller, target)
    };
    let (mut page, scroller, target) = build("top: 0px");
    assert_eq!(page.offset(target, scroller), (0.0, 0.0, 300.0, 10.0));
    page.doc.set_inline(target, "top: 5px");
    page.layout();
    let (whole, whole_scroller, whole_target) = build("top: 5px");
    assert_eq!(
        page.offset(target, scroller),
        whole.offset(whole_target, whole_scroller)
    );
    assert_eq!(page.offset(target, scroller), (0.0, 5.0, 300.0, 10.0));
}

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
        .scroll_to(scroller, terminus::link::Vector2D::new(0.0, 20.0));
    page.doc.set_inline(reader, "width: 11px");
    page.layout();
    assert_eq!(page.offset(reader, cb).1, 80.0);

    page.doc.set_inline(reader, "display: none");
    page.layout();
    page.doc.set_inline(reader, "");
    page.layout();
    assert_eq!(page.offset(reader, cb).1, 60.0);
}

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

#[test]
fn wpt_anchor_scope_shadow_names() {
    let shadow_view = "view { display: flex; flex-direction: column; flex-shrink: 0; }";
    let light = ".anchor { height: 10px; anchor-name: --a; }
         .cb { position: relative; width: 50px; height: 50px; border: 1px solid black; }
         .anchored { position: absolute; top: anchor(bottom, 1px); position-anchor: --a;
                     width: 5px; height: 5px; }";

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

const POSITION_ANCHOR_PAGE: &str = ".anchor { width: 100px; height: 100px; }
     .target { position: fixed; position-try-fallbacks: --pf; left: 999999px; }
     @position-try --pf { top: anchor(bottom, 0px); left: anchor(left, 0px);
                          width: anchor-size(width, 0px); height: anchor-size(height, 0px); }";

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

fn scrollable_size_case(page: &mut Page, inline: &str, filler: &str) -> (NodeId, NodeId) {
    let root = page.root();
    let scroller = page.el(root, "view.cb.scroller", inline);
    let filler = page.el(scroller, filler, "");
    page.el(filler, "view.anchor", "");
    (scroller, page.el(scroller, "view.target", ""))
}

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

fn assert_widths(page: &mut Page, boxes: &[(NodeId, f32)], case: &str) {
    page.layout();
    for (index, &(id, expected)) in boxes.iter().enumerate() {
        assert_eq!(page.abs(id).2, expected, "{case}: target {index}");
    }
}

const ANCHOR_NAME_PAGE: &str = ".relpos { position: relative; }
     .abspos { position: absolute; }
     .anchor1 { anchor-name: --a1; width: 10px; height: 10px; }
     .target { position: absolute; width: anchor-size(--a1 width); height: 10px; }";

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
        .scroll_to(scroller, terminus::link::Vector2D::new(0.0, 400.0));
    page.render();
    page.doc.set_inline(anchored, "display: flex");
    page.render();
    assert_eq!(page.abs(anchored).0, 51.0, "fits at its remembered offset");
    page.doc
        .dom
        .scroll_to(scroller, terminus::link::Vector2D::new(300.0, 400.0));
    page.render();
    assert_eq!(page.abs(anchored).0, 0.0, "the fallback after the scroll");
}

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

#[test]
fn wpt_anchor_scope_dynamic_names() {
    let css = scope_css("");
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
        .scroll_to(container, terminus::link::Vector2D::new(0.0, 50.0));
    assert_eq!(page.abs(sticky).1, 42.0);
}

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
        .scroll_to(scroller, terminus::link::Vector2D::new(0.0, 20.0));
    page.render();
    assert_eq!(page.abs(anchor).1, 30.0, "the anchor scrolled");
    assert_eq!(page.abs(anchored).1, 60.0, "the box follows it");
    assert_eq!(page.abs(child).1, 60.0, "and so does its content");
}

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
