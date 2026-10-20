//! css-cascade-6 §2.5 scoped styles (`@scope`) and css-nesting "Nesting
//! @scope" through a real document: scoping roots and limits, proximity in
//! the cascade, `:scope` and `&`, nested declarations, `@scope` nested in
//! style and group rules, shadow trees, restyle invalidation and the style
//! sharing cache. Matching, proximity, invalidation and sharing are stylo's;
//! the one `terminus` addition is the implicit root of a shadow tree's sheet
//! (`style::engine::ShadowStyleSheet`).
//!
//! Every case is a web-platform-tests port from `css/css-cascade/`, named
//! `wpt_<file-stem>_<case>` with the WPT case title above it. A WPT
//! `<template>` is spelled in the small markup dialect [`build`] reads: a
//! `<style>` becomes a document stylesheet, or a shadow stylesheet inside a
//! `<template shadowrootmode=open>`, and `getComputedStyle` is a computed
//! value read after a flush.
//!
//! **Implicit roots.** A document stylesheet here has no owner node, so its
//! implicit `@scope` root is the document element (stylo's
//! `ImplicitScopeRoot::DocumentElement`, Gecko's resolution for a sheet
//! without one); a shadow tree's sheet resolves to the shadow host, as a
//! constructed sheet adopted by a shadow root does in a browser. A WPT case
//! whose implicit root is the parent of an owner `<style>` element is
//! restated against the document element where its point survives, and says
//! what changed; otherwise it is listed below.
//!
//! **Ported in full:** `scope-evaluation.html`, `scope-proximity.html`,
//! `scope-specificity.html`, `scope-shadow.html`, `scope-layer.html`,
//! `scope-media.html`, `scope-supports.html`,
//! `scope-featureless.html`, `scope-name-defining-rules.html`,
//! `scope-hover.html`, `scope-focus.html`,
//! `scope-declaration-list-crash.html`, `scope-inner-pseudo-scope-crash.html`
//! and `scope-implicit-crash-print.html` (no panic), except the cases below.
//!
//! **Ported in part, and the cases left out:**
//!
//! - `scope-deep.html`: its 90 nested rules pass cssparser's 75-block nesting limit, so the
//!   innermost rule is dropped at parse time and the span stays black — Firefox 157's result;
//!   Chrome 155 and Safari 27 pass. The port pins both that and the deepest sheet that parses
//!   whole.
//! - `scope-evaluation.html`: "Nested scopes, `:has(:scope ...)` in inner from-selector", "Nested
//!   scopes, inner scoping root must be in the outer scope" and "Scope root with `:has()`" —
//!   `:has()` is behind `layout.css.has-selector.enabled` (false) in the fork, so those preludes do
//!   not parse.
//! - `scope-declarations.html`: the computed cases; "Scoped declarations apply to implicit scoping
//!   root" is restated against the document element. The `CSSNestedDeclarations` cases are CSSOM
//!   and live in the fork's `style/tests/lynx_scope.rs`.
//! - `scope-nesting.html`: the computed cases. Left out: the three `insertRule` cases and the three
//!   `selectorText` cases (CSSOM mutation APIs this engine does not carry).
//! - `scope-implicit.html`: restated against the document element. "@scope without prelude
//!   implicitly scopes to parent of owner node" keeps its inner half; "Proximity of multiple
//!   implicit @scope" becomes three document sheets with one shared root, so order of appearance
//!   decides; the two "sandwiched" cases are inverted (the document element is never inside an
//!   explicit scope) and gain a shadow-tree port where the outcome matches the WPT one.
//! - `scope-shadow-sharing.html` (a reftest): the inner owner `<style>` is dropped; the two hosts'
//!   shared-sheet outcome is asserted on computed values.
//! - `scope-ua-shadow-host.html` (a reftest): the `<input>` UA shadow root is an element with an
//!   attached shadow root.
//! - `scope-invalidation.html`: every DOM-mutation case. Left out: the four `:has()` cases (gated,
//!   as above), the two `:nth-child(… of S)` cases (`layout.css.nth-child-of.enabled` is false in
//!   the fork), the three `selectorText` cases (CSSOM), and "Sandwiched deep implicit scope root,
//!   :scope in subject" (its implicit root is an owner `<style>`'s parent inside `.a`).
//! - `scope-style-sharing-001.html`: tests 9 and 10 (`:has()`); tests 5 and 6 (implicit root = an
//!   owner `<style>`'s parent) are restated with that element as a shadow host whose shadow sheet
//!   holds the rule.
//! - `scope-style-sharing-002.html`: tests 9 and 10 (`:has()`); tests 5 and 6 (the implicit root is
//!   the parent of the element under test, which a shadow sheet cannot style); tests 7 and 8
//!   restated with a shadow host as above.
//!
//! **Not ported, and why:** `at-scope-parsing.html`,
//! `at-scope-relative-syntax.html`, `scope-cssom.html`,
//! `scope-visited-cssom.html` and `scope-rule-attr-with-colons.html` (CSSOM;
//! the parsing halves are in the fork's `lynx_scope.rs`); `scope-container.html`
//! (`@container` is not supported); `scope-starting-style.html`
//! (`@starting-style` is behind a false pref); `scope-part.html` (`::part()`
//! across `@scope` is out of this change); `scope-pseudo-element.html`
//! (`::before`/`::after` boxes are deferred, `docs/style-assumptions.md`);
//! `scope-visited.html` (no link history, `:visited` never matches);
//! `scope-overlapping-has.html` and `scope-has-crash.html` (`:has()` is
//! gated); the `scope-import-*.html` files (`@import` needs a stylesheet
//! loader `terminus` does not have); `scope-implicit-00{1..6}-print.html`,
//! `scope-implicit-external.html`, `scope-implicit-remove-sheet.html` and
//! `scope-implicit-sibling-mutations.html` (owner `<style>`/`<link>` nodes,
//! sheet removal, print media); and `scope-implicit-constructed-document.html`,
//! which expects a document-adopted sheet's implicit root to be the
//! *document node* (`:scope` matches nothing, the root element is in scope)
//! — no engine ships that yet (Chrome 155 and Safari 27 match nothing,
//! Gecko and this engine use the document element).

mod common;

use common::Doc;
use terminus::{ElementState, NodeId, ShadowRootMode};

const GREEN: &str = "rgb(0, 128, 0)";
const BLACK: &str = "rgb(0, 0, 0)";
const TRANSPARENT: &str = "rgba(0, 0, 0, 0)";

/// The baseline every `assert_not_green` in the WPT files reads.
const MAIN_BLACK: &str = "main * { background-color: black; }";

/// A document whose tree is `<page><main id=main>…</main></page>`, built from
/// WPT template markup.
struct Page {
    doc: Doc,
    main: NodeId,
}

impl Page {
    /// Mounts `css` (the WPT file's own `<style>`), then builds `markup` under
    /// `main` and flushes.
    fn new(css: &str, markup: &str) -> Self {
        let mut doc = Doc::new();
        if !css.is_empty() {
            doc.add_css(css);
        }
        let main = doc.el(doc.root, "main#main");
        build(&mut doc, main, markup);
        doc.flush();
        Self { doc, main }
    }

    /// Builds `markup` directly under the document element, so `main` is the
    /// document element itself.
    fn at_root(css: &str, markup: &str) -> Self {
        let mut doc = Doc::new();
        if !css.is_empty() {
            doc.add_css(css);
        }
        let root = doc.root;
        build(&mut doc, root, markup);
        doc.flush();
        Self { doc, main: root }
    }

    fn root(&self) -> NodeId {
        self.doc.root
    }

    fn flush(&mut self) {
        self.doc.flush();
    }

    /// `main.querySelector(selectors)`.
    fn q(&self, selectors: &str) -> NodeId {
        self.q_in(self.main, selectors)
    }

    /// `root.querySelector(selectors)`, for a shadow root or a host.
    fn q_in(&self, root: NodeId, selectors: &str) -> NodeId {
        self.doc
            .dom
            .query_selector(root, selectors)
            .unwrap_or_else(|_| panic!("selector `{selectors}` must parse"))
            .unwrap_or_else(|| panic!("no element matches `{selectors}`"))
    }

    /// `main.querySelectorAll(selectors)`.
    fn all(&self, selectors: &str) -> Vec<NodeId> {
        self.doc
            .dom
            .query_selector_all(self.main, selectors)
            .unwrap_or_else(|_| panic!("selector `{selectors}` must parse"))
    }

    fn shadow(&self, host: &str) -> NodeId {
        let host = self.q(host);
        self.doc
            .dom
            .shadow_root(host)
            .expect("the host has a shadow root")
    }

    fn bg_of(&self, id: NodeId) -> String {
        self.doc.value(id, "background-color")
    }

    fn z_of(&self, id: NodeId) -> String {
        self.doc.value(id, "z-index")
    }

    fn bg(&self, selectors: &str) -> String {
        self.bg_of(self.q(selectors))
    }

    fn z(&self, selectors: &str) -> String {
        self.z_of(self.q(selectors))
    }

    fn border(&self, selectors: &str) -> String {
        self.doc.value(self.q(selectors), "border-top-color")
    }

    fn green(&self, selectors: &str, why: &str) {
        assert_eq!(self.bg(selectors), GREEN, "`{selectors}` is green: {why}");
    }

    fn not_green(&self, selectors: &str, why: &str) {
        assert_eq!(
            self.bg(selectors),
            BLACK,
            "`{selectors}` is not green: {why}"
        );
    }

    fn green_id(&self, id: NodeId, what: &str, why: &str) {
        assert_eq!(self.bg_of(id), GREEN, "{what} is green: {why}");
    }

    fn not_green_id(&self, id: NodeId, what: &str, why: &str) {
        assert_eq!(self.bg_of(id), BLACK, "{what} is not green: {why}");
    }
}

/// Builds WPT template markup under `parent`.
///
/// The dialect: elements with `id`, `class` and other attributes (unquoted,
/// quoted or bare); `<style>` mounts its text as a document stylesheet, or as
/// a shadow stylesheet of the enclosing shadow root; `<template
/// shadowrootmode=open>` attaches a shadow root to the element it sits in and
/// builds its content there; `<input>` is void; comments and text are
/// skipped.
fn build(doc: &mut Doc, parent: NodeId, markup: &str) {
    // (tag, container, the shadow root a `<style>` here belongs to)
    let mut stack: Vec<(String, NodeId, Option<NodeId>)> = vec![(String::new(), parent, None)];
    let mut rest = markup;
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        if let Some(after) = rest.strip_prefix("<!--") {
            let end = after.find("-->").expect("unterminated comment");
            rest = &after[end + 3..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("</") {
            let end = after.find('>').expect("unterminated close tag");
            let (open, _, _) = stack.pop().expect("unbalanced close tag");
            assert_eq!(open, after[..end].trim(), "mismatched close tag");
            rest = &after[end + 1..];
            continue;
        }
        let end = rest.find('>').expect("unterminated tag");
        let (name, attrs) = parse_tag(&rest[1..end]);
        rest = &rest[end + 1..];
        let &(_, container, shadow) = stack.last().expect("the parent stays on the stack");
        match name.as_str() {
            "style" => {
                let close = rest.find("</style>").expect("unterminated <style>");
                let css = &rest[..close];
                rest = &rest[close + "</style>".len()..];
                match shadow {
                    Some(root) => doc.dom.add_shadow_stylesheet(root, css),
                    None => doc.add_css(css),
                }
            }
            "template" => {
                assert!(
                    attrs
                        .iter()
                        .any(|(n, v)| n == "shadowrootmode" && v == "open"),
                    "only declarative open shadow roots are templates here"
                );
                let root = doc.dom.attach_shadow(container, ShadowRootMode::Open);
                stack.push((name, root, Some(root)));
            }
            _ => {
                let id = doc.dom.create_element(&name, ());
                for (attr, value) in &attrs {
                    match attr.as_str() {
                        "id" => doc.dom.set_id_attribute(id, Some(value)),
                        "class" => {
                            for class in value.split_whitespace() {
                                doc.dom.add_class(id, class);
                            }
                        }
                        _ => doc.dom.set_attribute(id, attr, value),
                    }
                }
                doc.dom.append_child(container, id);
                if name != "input" {
                    stack.push((name, id, shadow));
                }
            }
        }
    }
    assert_eq!(stack.len(), 1, "unclosed tags in the markup");
}

fn parse_tag(tag: &str) -> (String, Vec<(String, String)>) {
    let tag = tag.trim();
    let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
    let name = tag[..name_end].to_owned();
    let mut attrs = Vec::new();
    let mut rest = tag[name_end..].trim_start();
    while !rest.is_empty() {
        let attr_end = rest
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(rest.len());
        let attr = rest[..attr_end].to_owned();
        rest = &rest[attr_end..];
        let value = if let Some(after) = rest.strip_prefix('=') {
            if let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') {
                let body = &after[1..];
                let close = body.find(quote).expect("unterminated attribute value");
                rest = &body[close + 1..];
                body[..close].to_owned()
            } else {
                let value_end = after.find(char::is_whitespace).unwrap_or(after.len());
                rest = &after[value_end..];
                after[..value_end].to_owned()
            }
        } else {
            String::new()
        };
        attrs.push((attr, value));
        rest = rest.trim_start();
    }
    (name, attrs)
}

// ---------------------------------------------------------------------------
// scope-evaluation.html
// ---------------------------------------------------------------------------

const EVAL_BASE: &str = ":where(main *) { background-color: black; }";

fn eval_page(markup: &str) -> Page {
    Page::new(EVAL_BASE, markup)
}

// WPT css/css-cascade/scope-evaluation.html, "Single scope".
#[test]
fn wpt_scope_evaluation_single_scope() {
    let page = eval_page(
        "<style>@scope (.a) { span { background-color: green; } }</style>
         <div class=a><span>green</span></div>
         <div class=b><span>not green</span></div>
         <span>not green</span>",
    );
    page.green(".a > span", "a descendant of the root is in scope");
    page.not_green(".b > span", "no .a ancestor, no scope");
    page.not_green(":scope > span", "no .a ancestor, no scope");
}

// WPT css/css-cascade/scope-evaluation.html, "Scope can not match its own root
// without :scope".
#[test]
fn wpt_scope_evaluation_root_needs_scope_pseudo() {
    let page = eval_page(
        "<style>@scope (.a) { .a { background-color: green; } }</style>
         <div class=a><span>not green</span></div>",
    );
    page.not_green(
        ".a",
        "a selector without :scope or & matches descendants only",
    );
    page.not_green(".a > span", "the span is not .a");
}

// WPT css/css-cascade/scope-evaluation.html, "Selecting self with :scope".
#[test]
fn wpt_scope_evaluation_selecting_self_with_scope() {
    let page = eval_page(
        "<style>@scope (.a) { :scope { background-color: green; } }</style>
         <div class=a><span>not green</span></div>",
    );
    page.green(".a", ":scope is the scoping root");
    page.not_green(".a > span", ":scope is the root only");
}

// WPT css/css-cascade/scope-evaluation.html, "Single scope with limit".
#[test]
fn wpt_scope_evaluation_single_scope_with_limit() {
    let page = eval_page(
        "<style>@scope (.a) to (.c) { span { background-color: green; } }</style>
         <div class=a>
           <div class=b><span>green</span></div>
           <div class=c><span>not green</span></div>
         </div>",
    );
    page.green(".b > span", "inside the root, outside any limit");
    page.not_green(".c > span", "a limit's descendants are out of scope");
}

// WPT css/css-cascade/scope-evaluation.html, "Single scope, :scope pseudo in
// main selector".
#[test]
fn wpt_scope_evaluation_scope_pseudo_in_main_selector() {
    let page = eval_page(
        "<style>@scope (.a) { :scope > span { background-color: green; } }</style>
         <div class=a>
           <span>green</span>
           <div class=b><span>not green</span></div>
         </div>",
    );
    page.green(".a > span", "a child of :scope");
    page.not_green(".b > span", "a grandchild of :scope");
}

// WPT css/css-cascade/scope-evaluation.html, "Single scope, :scope pseudo in
// to-selector".
#[test]
fn wpt_scope_evaluation_scope_pseudo_in_to_selector() {
    let page = eval_page(
        "<style>@scope (.a) to (:scope > .b) { span { background-color: green; } }</style>
         <div class=a>
           <div class=b><span>not green</span></div>
           <div class=c><div class=b><span>green</span></div></div>
         </div>",
    );
    page.not_green(".a > .b > span", "under the limit `:scope > .b`");
    page.green(".a > .c > .b > span", "that .b is not a child of the root");
}

// WPT css/css-cascade/scope-evaluation.html, "Multiple scopes, :scope pseudo
// in to-selector".
#[test]
fn wpt_scope_evaluation_multiple_scopes_scope_pseudo_in_to_selector() {
    let page = eval_page(
        "<style>@scope (.a) to (:scope > .b) { span { background-color: green; } }</style>
         <div class=a>
           <div class=b><span>not green</span></div>
           <div class=a><div class=b><span>green</span></div></div>
         </div>",
    );
    page.not_green(".a > .b > span", "limited in both scopes");
    page.green(
        ".a > .a > .b > span",
        "in the outer .a scope, though limited in the inner one",
    );
}

// WPT css/css-cascade/scope-evaluation.html, "Inner @scope with :scope in
// from-selector".
#[test]
fn wpt_scope_evaluation_inner_scope_with_scope_pseudo_in_from_selector() {
    let page = eval_page(
        "<style>
           @scope (.a) { @scope (:scope > .b) { span { background-color: green; } } }
         </style>
         <div class=a>
           <div class=b><span>green</span></div>
           <div><div class=b><span>not green</span></div></div>
         </div>",
    );
    page.green(
        ".a > .b > span",
        "the inner root is a child of the outer root",
    );
    page.not_green(".a > div > .b > span", "that .b is not a child of .a");
}

// WPT css/css-cascade/scope-evaluation.html, "Multiple scopes from same
// @scope-rule, only one limited".
#[test]
fn wpt_scope_evaluation_multiple_scopes_only_one_limited() {
    let page = eval_page(
        "<style>@scope (.a) to (:scope > .b) { .c { background-color: green; } }</style>
         <div class=a><div><div class=a><div class=b><div class=c></div></div></div></div></div>",
    );
    page.green(".c", "the outer .a scope does not limit .c");
}

// WPT css/css-cascade/scope-evaluation.html, "Multiple scopes from same
// @scope-rule, both limited".
#[test]
fn wpt_scope_evaluation_multiple_scopes_both_limited() {
    let page = eval_page(
        "<style>@scope (.a) to (.b) { .c { background-color: green; } }</style>
         <div class=a><div><div class=a><div class=b><div class=c></div></div></div></div></div>",
    );
    page.not_green(".c", ".b limits both scopes");
}

// WPT css/css-cascade/scope-evaluation.html, "Nested scopes".
#[test]
fn wpt_scope_evaluation_nested_scopes() {
    let page = eval_page(
        "<style>@scope (.a) { @scope (.b) { span { background-color: green; } } }</style>
         <div class=a>
           <div class=b><span>green</span></div>
           <span>not green</span>
         </div>
         <div class=b><span>not green</span></div>",
    );
    page.green(".a > .b > span", "inside .b inside .a");
    page.not_green(".a > span", "not inside .b");
    page.not_green(":scope > .b > span", "that .b is outside .a");
}

// WPT css/css-cascade/scope-evaluation.html, "Nested scopes, reverse".
#[test]
fn wpt_scope_evaluation_nested_scopes_reverse() {
    let page = eval_page(
        "<style>@scope (.b) { @scope (.a) { span { background-color: green; } } }</style>
         <div class=a>
           <div class=b><span>not green</span></div>
           <span>not green</span>
         </div>",
    );
    page.not_green(
        ".a > .b > span",
        "the inner root must be inside the outer scope",
    );
    page.not_green(".a > span", "no .b ancestor");
}

// WPT css/css-cascade/scope-evaluation.html, "Nested scopes, with
// to-selector".
#[test]
fn wpt_scope_evaluation_nested_scopes_with_to_selector() {
    let page = eval_page(
        "<style>
           @scope (.a) { @scope (.b) to (.c) { span { background-color: green; } } }
         </style>
         <div class=a>
           <div class=b><span>green</span></div>
           <div class=b><div class=c><span>not green</span></div></div>
           <span>not green</span>
         </div>
         <div class=b><span>not green</span></div>",
    );
    page.green(".a > .b > span", "in both scopes");
    page.not_green(".a > span", "not inside .b");
    page.not_green(".a > .b > .c > span", "under the inner limit");
    page.not_green(":scope > .b > span", "that .b is outside .a");
}

// WPT css/css-cascade/scope-evaluation.html, "Nested scopes, inner scoping
// root may be the outer scoping root".
#[test]
fn wpt_scope_evaluation_inner_root_may_be_outer_root() {
    let page = eval_page(
        "<style>@scope (.a) { @scope (:scope) { span { background-color: green; } } }</style>
         <div class=a><span>green</span></div>",
    );
    page.green(".a > span", "the scoping root is in its own scope");
}

// WPT css/css-cascade/scope-evaluation.html, ":scope selecting itself".
#[test]
fn wpt_scope_evaluation_scope_selecting_itself() {
    let page = eval_page(
        "<style>@scope (.a) { :scope { background-color: green; } }</style>
         <div class=a></div>",
    );
    page.green(".a", ":scope matches the root");
}

// WPT css/css-cascade/scope-evaluation.html, "The scoping limit is not in
// scope".
#[test]
fn wpt_scope_evaluation_limit_is_not_in_scope() {
    let page = eval_page(
        "<style>@scope (.a) to (.b) { * { background-color: green; } }</style>
         <div id=above>
           <div class=a><div><div class=b><div id=below></div></div></div></div>
           <div id=adjacent></div>
         </div>",
    );
    let why = "exactly the root's descendants above the limit are in scope";
    page.not_green("#above", why);
    page.not_green("#adjacent", why);
    page.not_green(".a", why);
    page.green(".a > div", why);
    page.not_green(".b", why);
    page.not_green("#below", why);
}

// WPT css/css-cascade/scope-evaluation.html, "Simulated inclusive scoping
// limit".
#[test]
fn wpt_scope_evaluation_simulated_inclusive_limit() {
    let page = eval_page(
        "<style>@scope (.a) to (.b > *) { * { background-color: green; } }</style>
         <div id=above>
           <div class=a><div><div class=b><div id=limit></div></div></div></div>
           <div id=adjacent></div>
         </div>",
    );
    let why = "`.b > *` limits below .b, so .b stays in scope";
    page.not_green("#above", why);
    page.not_green("#adjacent", why);
    page.not_green(".a", why);
    page.green(".a > div", why);
    page.green(".b", why);
    page.not_green("#limit", why);
}

// WPT css/css-cascade/scope-evaluation.html, "Scope with no elements".
#[test]
fn wpt_scope_evaluation_scope_with_no_elements() {
    let page = eval_page(
        "<style>@scope (.a) to (:scope) { * { background-color: green; } }</style>
         <div id=above>
           <div class=a><div><div class=b><div id=inner></div></div></div></div>
           <div id=adjacent></div>
         </div>",
    );
    let why = "a limit of `:scope` empties the scope";
    for selectors in ["#above", "#adjacent", ".a", ".a > div", ".b", "#inner"] {
        page.not_green(selectors, why);
    }
}

// WPT css/css-cascade/scope-evaluation.html, ":scope direct adjacent
// sibling".
#[test]
fn wpt_scope_evaluation_scope_direct_adjacent_sibling() {
    let page = eval_page(
        "<style>@scope (.a) { :scope + .c { background-color: green; } }</style>
         <div class=a><div class=a></div><div class=c></div></div>",
    );
    page.not_green(".c", "a sibling of the root is never in its scope");
}

// WPT css/css-cascade/scope-evaluation.html, ":scope indirect adjacent
// sibling".
#[test]
fn wpt_scope_evaluation_scope_indirect_adjacent_sibling() {
    let page = eval_page(
        "<style>@scope (.a) { :scope + .c { background-color: green; } }</style>
         <div class=a><div class=a></div><div></div><div class=c></div></div>",
    );
    page.not_green(".c", "a sibling of the root is never in its scope");
}

// WPT css/css-cascade/scope-evaluation.html, "Relative selector inside
// @scope".
#[test]
fn wpt_scope_evaluation_relative_selector() {
    let page = eval_page(
        "<style>@scope (.a) { > span { background-color: green; } }</style>
         <div class=a><span>green</span></div>",
    );
    page.green(".a > span", "`> span` is relative to :scope");
}

// WPT css/css-cascade/scope-evaluation.html, ":scope in two different
// compounds".
#[test]
fn wpt_scope_evaluation_scope_in_two_compounds() {
    let page = eval_page(
        "<style>@scope (.a) { :scope > :scope { background-color: green; } }</style>
         <div class=a><div id=inner class=a></div></div>",
    );
    page.not_green(".a", "one root per match, so :scope > :scope never matches");
    page.not_green(
        "#inner",
        "one root per match, so :scope > :scope never matches",
    );
}

// WPT css/css-cascade/scope-evaluation.html, "Any scope limit makes the
// element out of scope".
#[test]
fn wpt_scope_evaluation_any_limit_excludes() {
    let page = eval_page(
        "<style>@scope (.a) to (.b, .c) { * { background-color: green; } }</style>
         <div class=a>
           <span id=in></span>
           <div class=b><span id=out></span><div class=c></div></div>
         </div>",
    );
    page.green("#in", "no limit above it");
    page.not_green("#out", "under the .b limit");
}

// ---------------------------------------------------------------------------
// scope-proximity.html
// ---------------------------------------------------------------------------

// WPT css/css-cascade/scope-proximity.html, "Alternating light/dark".
#[test]
fn wpt_scope_proximity_alternating_light_dark() {
    let page = Page::new(
        MAIN_BLACK,
        "<style>
           .item { padding: 0px; border: 5px solid red; }
           @scope (.light) { [id] { border-color: rgb(100, 100, 100); } }
           @scope (.dark) { [id] { border-color: rgb(200, 200, 200); } }
         </style>
         <div class=light><div id=item1>
           <div class=dark><div id=item2>
             <div class=light><div id=item3>
               <div class=dark><div id=item4></div></div>
             </div></div>
           </div></div>
         </div></div>",
    );
    let why = "the nearer scoping root wins at equal specificity";
    assert_eq!(page.border("#item1"), "rgb(100, 100, 100)", "{why}");
    assert_eq!(page.border("#item2"), "rgb(200, 200, 200)", "{why}");
    assert_eq!(page.border("#item3"), "rgb(100, 100, 100)", "{why}");
    assert_eq!(page.border("#item4"), "rgb(200, 200, 200)", "{why}");
}

// WPT css/css-cascade/scope-proximity.html, "Proximity wins over order of
// appearance".
#[test]
fn wpt_scope_proximity_wins_over_order() {
    let page = Page::new(
        MAIN_BLACK,
        "<style>
           @scope (.b) { [id] { border-color: green; } }
           @scope (.a) { [id] { border-color: red; } }
         </style>
         <div class=a><div class=b><span id=item></span></div></div>",
    );
    assert_eq!(
        page.border("#item"),
        GREEN,
        "proximity sits before order of appearance in the cascade"
    );
}

// WPT css/css-cascade/scope-proximity.html, "Specificity wins over
// proximity".
#[test]
fn wpt_scope_proximity_specificity_wins() {
    let page = Page::new(
        MAIN_BLACK,
        "<style>
           @scope (.a) { span[id] { border-color: green; } }
           @scope (.b) { [id] { border-color: red; } }
         </style>
         <div class=a><div class=b><span id=item></span></div></div>",
    );
    assert_eq!(
        page.border("#item"),
        GREEN,
        "specificity sits before proximity in the cascade"
    );
}

// WPT css/css-cascade/scope-proximity.html, "Identical root with further
// proximity is not ignored".
#[test]
fn wpt_scope_proximity_identical_root_further() {
    let page = Page::new(
        MAIN_BLACK,
        "<style>@scope (.foo) { .bar span[id] { border-color: green; } }</style>
         <div class=foo><div class=\"foo bar\"><span id=item></span></div></div>",
    );
    assert_eq!(
        page.border("#item"),
        GREEN,
        "the outer .foo root still matches when the nearer one cannot"
    );
}

// WPT css/css-cascade/scope-proximity.html, "Most proximate match wins under
// multiple scoping roots".
#[test]
fn wpt_scope_proximity_most_proximate_root() {
    let page = Page::new(
        MAIN_BLACK,
        "<style>
           @scope (.scope) { :where(&) { border-color: green; } }
           @scope (#outer) { :where(:scope) :where(#inner) { border-color: red; } }
         </style>
         <div id=outer class=scope><div><div id=inner class=scope></div></div></div>",
    );
    assert_eq!(
        page.border("#inner"),
        GREEN,
        "the rule's nearest matching root (#inner itself, proximity 0) is used"
    );
}

// ---------------------------------------------------------------------------
// scope-specificity.html
// ---------------------------------------------------------------------------

/// WPT `format_scoped_rule`.
fn format_scoped_rule(scoped: &[&str], declarations: &str) -> String {
    let (prelude, rest) = scoped.split_first().expect("a prelude and a selector");
    let content = match rest {
        [selector] => format!("{selector} {{ {declarations} }}"),
        _ => format_scoped_rule(rest, declarations),
    };
    format!("{prelude} {{ {content} }}")
}

/// WPT `test_scope_specificity`: the scoped rule's specificity equals the
/// reference selector's, and proximity breaks the tie.
fn specificity_case(scoped: &[&str], reference: &str) {
    const TREE: &str = "<div id=a class=a><div id=b class=b></div></div>";
    let scoped_rule = format_scoped_rule(scoped, "z-index:1");
    let ref_rule = format!(":is({reference}) {{ z-index:2 }}");
    let case = format!("{} and {reference}", format_scoped_rule(scoped, ""));
    for (css, expected, step) in [
        (scoped_rule.clone(), "1", "scoped rule"),
        (ref_rule.clone(), "2", "unscoped rule"),
        (
            format!("{scoped_rule} {ref_rule}"),
            "1",
            "scoped + unscoped: proximity wins the tie",
        ),
        (
            format!("{ref_rule} {scoped_rule}"),
            "1",
            "unscoped + scoped: proximity wins the tie",
        ),
        (
            format!("div{ref_rule} {scoped_rule}"),
            "2",
            "one more type selector outweighs proximity",
        ),
    ] {
        let page = Page::new(&css, TREE);
        assert_eq!(page.z(reference), expected, "{case}: {step}");
    }
}

// WPT css/css-cascade/scope-specificity.html, every case.
#[test]
fn wpt_scope_specificity_all() {
    // Implicit `:scope ` before inner selectors adds no specificity.
    specificity_case(&["@scope (#main)", ".b"], ".b");
    specificity_case(&["@scope (#main) to (.b)", ".a"], ".a");
    specificity_case(&["@scope (#main, .foo, .bar)", "#a"], "#a");
    specificity_case(&["@scope (#main)", "div.b"], "div.b");
    specificity_case(&["@scope (#main)", ":scope .b"], ".a .b");
    // `&` behaves like `:where(:scope)`.
    specificity_case(&["@scope (#main)", "& .b"], ":where(#main) .b");
    specificity_case(&["@scope (#main)", "div .b"], "div .b");
    specificity_case(&["@scope (#main)", "@scope (.a)", ".b"], ".b");
    // An explicit `:scope` adds specificity.
    specificity_case(&["@scope (#main)", ":scope .b"], ":scope .b");
    // `&` behaves like `:where(:scope)` for an implicit root too. The WPT
    // sheet sits in `main`; here the document element is the implicit root,
    // and the unscoped `:scope` is the document element as well.
    specificity_case(&["@scope", "& .b"], ":where(:scope) .b");
    // Relative selector syntax adds no specificity.
    specificity_case(&["@scope (#main)", "> .a"], ":where(#main) > .a");
}

// ---------------------------------------------------------------------------
// scope-declarations.html
// ---------------------------------------------------------------------------

// WPT css/css-cascade/scope-declarations.html, "Scoped declarations apply to
// the scoping root".
#[test]
fn wpt_scope_declarations_apply_to_root() {
    let page = Page::new(
        "",
        "<style>@scope (.a) { z-index: 1; }</style><div class=a></div>",
    );
    assert_eq!(
        page.z(".a"),
        "1",
        "bare declarations in @scope are `:where(:scope)` rules"
    );
}

// WPT css/css-cascade/scope-declarations.html, "Scoped declarations apply to
// implicit scoping root". The WPT root is the owner `<style>`'s parent; a
// document sheet's implicit root is the document element.
#[test]
fn wpt_scope_declarations_apply_to_implicit_root() {
    let page = Page::new(
        "",
        "<style>@scope { z-index: 1; }</style><div class=a></div>",
    );
    assert_eq!(
        page.z_of(page.root()),
        "1",
        "bare declarations apply to the implicit root"
    );
    assert_eq!(page.z(".a"), "auto", "and only to the root");
}

// WPT css/css-cascade/scope-declarations.html, "Scoped declarations apply
// with zero specificity".
#[test]
fn wpt_scope_declarations_zero_specificity() {
    let page = Page::new(
        "",
        "<style>
           @scope (.a) { :where(:scope) { z-index: 1; } z-index: 2; }
           @scope (.b) { z-index: 1; :where(:scope) { z-index: 2; } }
           @scope (.c) { :scope { z-index: 1; } z-index: 2; }
         </style>
         <div class=a></div><div class=b></div><div class=c></div>",
    );
    assert_eq!(page.z(".a"), "2", "equal to :where(:scope), later wins");
    assert_eq!(page.z(".b"), "2", "equal to :where(:scope), later wins");
    assert_eq!(page.z(".c"), "1", ":scope outweighs bare declarations");
}

// ---------------------------------------------------------------------------
// scope-nesting.html
// ---------------------------------------------------------------------------

/// The `a`/`b`/`below`/`outside` tree three scope-end cases share.
const SCOPE_END_TREE: &str = "
    <div class=\"a b\"><div class=b><div id=below></div></div></div>
  </div>
  <div id=outside></div>";

fn scope_end_case(css: &str, case: &str) {
    let page = Page::new("", &format!("<div><style>{css}</style>{SCOPE_END_TREE}"));
    assert_eq!(page.z(".a"), "1", "{case}: the root is in scope");
    assert_eq!(page.z(".a > .b"), "auto", "{case}: the limit is not");
    assert_eq!(page.z("#below"), "auto", "{case}");
    assert_eq!(page.z("#outside"), "auto", "{case}");
}

// WPT css/css-cascade/scope-nesting.html, "Nesting-selector in <scope-end>".
#[test]
fn wpt_scope_nesting_nesting_selector_in_scope_end() {
    scope_end_case(
        "@scope (.a) to (& > .b) { :scope { z-index:1; } }
         @scope (.a) to (.b&) { :scope { z-index:42; } }",
        "`&` in <scope-end> is the scoping root",
    );
}

// WPT css/css-cascade/scope-nesting.html, "Implicit :scope in <scope-end>".
#[test]
fn wpt_scope_nesting_implicit_scope_in_scope_end() {
    scope_end_case(
        "@scope (.a) to (.b) { :scope { z-index:1; } }
         @scope (.a) to (.b:scope) { :scope { z-index:42; } }",
        "<scope-end> is relative to :scope",
    );
}

// WPT css/css-cascade/scope-nesting.html, "Relative selectors in
// <scope-end>".
#[test]
fn wpt_scope_nesting_relative_selectors_in_scope_end() {
    scope_end_case(
        "@scope (.a) to (> .b) { *, :scope { z-index:1; } }",
        "a relative <scope-end> hangs off :scope",
    );
}

// WPT css/css-cascade/scope-nesting.html, "Nested nesting-selectors within
// scope's <stylesheet> select inclusive descendants of the scope root".
#[test]
fn wpt_scope_nesting_nested_nesting_selectors() {
    let page = Page::new(
        "",
        "<div><style>@scope (#div) { & { z-index: 1; & { z-index: 2; } } }</style>
         <div id=div></div></div>",
    );
    assert_eq!(
        page.z("#div"),
        "2",
        "`& {{ & {{}} }}` still selects the root"
    );
}

// WPT css/css-cascade/scope-nesting.html, "Nesting-selector within :scope
// rule".
#[test]
fn wpt_scope_nesting_nesting_selector_within_scope_rule() {
    let page = Page::new(
        "",
        "<div><style>@scope (#div) { :scope { z-index: 1; & { z-index: 2; } } }</style>
         <div id=div></div></div>",
    );
    assert_eq!(page.z("#div"), "2", "`&` inside `:scope {{}}` is :scope");
}

// WPT css/css-cascade/scope-nesting.html, "Nesting-selector within :scope
// rule (double nested)".
#[test]
fn wpt_scope_nesting_nesting_selector_within_scope_rule_double() {
    let page = Page::new(
        "",
        "<div><style>@scope (#div) { :scope { z-index: 1; & { & { z-index: 2; } } } }</style>
         <div id=div></div></div>",
    );
    assert_eq!(
        page.z("#div"),
        "2",
        "`& {{ & {{}} }}` inside `:scope {{}}` is :scope"
    );
}

// WPT css/css-cascade/scope-nesting.html, "@scope nested within style rule".
#[test]
fn wpt_scope_nesting_scope_within_style_rule() {
    let page = Page::new(
        "",
        "<div><style>.a { @scope (.b) { .c { z-index: 1; } } }</style>
         <div class=a>
           <div class=b><div class=c></div></div>
           <div id=out_of_scope class=c></div>
         </div></div>",
    );
    assert_eq!(page.z(".c"), "1", "the nested scope's root is `.a .b`");
    assert_eq!(page.z("#out_of_scope"), "auto", "no .b root above it");
}

// WPT css/css-cascade/scope-nesting.html, "Parent pseudo class within
// scope-start".
#[test]
fn wpt_scope_nesting_parent_pseudo_in_scope_start() {
    let page = Page::new(
        "",
        "<div><style>.a { @scope (&.b) { :scope { z-index: 1; } } }</style>
         <div class=a></div><div class=b></div><div class=\"a b\"></div></div>",
    );
    assert_eq!(page.z(".a:not(.b)"), "auto", "`&.b` needs both classes");
    assert_eq!(page.z(".b:not(.a)"), "auto", "`&.b` needs both classes");
    assert_eq!(
        page.z(".a.b"),
        "1",
        "`&` in <scope-start> is the parent rule"
    );
}

// WPT css/css-cascade/scope-nesting.html, "Parent pseudo class within body of
// nested @scope".
#[test]
fn wpt_scope_nesting_parent_pseudo_in_nested_scope_body() {
    let page = Page::new(
        "",
        "<div><style>.a { @scope (.b) { &.c { z-index: 1; } } }</style>
         <div class=a>
           <div class=b>
             <div class=c></div>
             <div class=\"a c\"></div>
             <div class=\"a b c\" matching></div>
           </div>
         </div>
         <div>
           <div class=a></div><div class=b></div><div class=c></div>
           <div class=\"a b\"></div><div class=\"a c\"></div><div class=\"b c\"></div>
         </div></div>",
    );
    let matching = page.all("div[matching]");
    assert_eq!(matching.len(), 1);
    for id in matching {
        assert_eq!(
            page.z_of(id),
            "1",
            "`&` in the body is the scope start, which is `.a .b`"
        );
    }
    for id in page.all("div:not([matching])") {
        assert_eq!(page.z_of(id), "auto", "only `.a .b.c` matches `&.c`");
    }
}

// WPT css/css-cascade/scope-nesting.html, "Implicit rule within nested
// @scope".
#[test]
fn wpt_scope_nesting_implicit_rule_within_nested_scope() {
    let page = Page::new(
        "",
        "<div><style>.a { @scope (.b) { z-index: 1; } }</style>
         <div class=a><div class=b><div class=c></div></div></div></div>",
    );
    assert_eq!(
        page.z(".a"),
        "auto",
        "bare declarations apply to the root only"
    );
    assert_eq!(page.z(".b"), "1", "bare declarations apply to the root");
    assert_eq!(
        page.z(".c"),
        "auto",
        "bare declarations apply to the root only"
    );
}

// WPT css/css-cascade/scope-nesting.html, "Implicit rule within nested
// @scope (proximity)".
#[test]
fn wpt_scope_nesting_implicit_rule_within_nested_scope_proximity() {
    let page = Page::new(
        "",
        "<div><style>
           .a { @scope (:where(& .b)) { z-index: 1; } }
           :where(.b) { z-index: 2; }
         </style>
         <div class=a><div class=\"b x\"><div class=c></div></div></div></div>",
    );
    assert_eq!(page.z(".a"), "auto", "not the root");
    assert_eq!(
        page.z(".b"),
        "1",
        "bare declarations are `:where(:scope)` and win by proximity, not `&`"
    );
    assert_eq!(page.z(".c"), "auto", "not the root");
}

// WPT css/css-cascade/scope-nesting.html, "Nested :scope inside an :is".
#[test]
fn wpt_scope_nesting_nested_scope_inside_is() {
    let page = Page::new(
        "",
        "<div><style>
           @scope (.a) {
             .b {
               &:is(:scope.x *) { z-index: 1; }
               &:is(:root:scope *) { z-index: 2; }
             }
             .c:is(:scope.x *) { z-index: 3; }
           }
         </style>
         <div class=b></div>
         <div class=\"a x\"><div class=b></div><div class=c></div></div></div>",
    );
    assert_eq!(page.z(".b"), "auto", "outside the scope");
    assert_eq!(
        page.z(".a .b"),
        "1",
        ":scope inside :is() is the scoping root"
    );
    assert_eq!(page.z(".c"), "3", ":scope inside :is() is the scoping root");
}

// WPT css/css-cascade/scope-nesting.html, ":scope within nested and scoped
// rule".
#[test]
fn wpt_scope_nesting_scope_within_nested_scoped_rule() {
    let page = Page::new(
        "",
        "<div><style>@scope (.b) { .a:not(:scope) { & :scope { z-index: 1; } } }</style>
         <div class=b></div><div class=a><div class=b></div></div></div>",
    );
    assert_eq!(page.z(".b"), "auto", "no .a ancestor");
    assert_eq!(page.z(".a .b"), "1", "`.a :scope` with the .b root");
}

// WPT css/css-cascade/scope-nesting.html, ":scope within nested and scoped
// rule (implied &)".
#[test]
fn wpt_scope_nesting_scope_within_nested_scoped_rule_implied() {
    let page = Page::new(
        "",
        "<div><style>@scope (.b) { .a:not(:scope) { :scope { z-index: 1; } } }</style>
         <div class=b></div><div class=a><div class=b></div></div></div>",
    );
    assert_eq!(page.z(".b"), "auto", "no .a ancestor");
    assert_eq!(page.z(".a .b"), "1", "the implied `&` makes `.a :scope`");
}

// WPT css/css-cascade/scope-nesting.html, ":scope within nested and scoped
// rule (relative)".
#[test]
fn wpt_scope_nesting_scope_within_nested_scoped_rule_relative() {
    let page = Page::new(
        "",
        "<div><style>@scope (.b) { .a:not(:scope) { > :scope { z-index: 1; } } }</style>
         <div class=b></div><div class=a><div class=b></div></div></div>",
    );
    assert_eq!(page.z(".b"), "auto", "no .a parent");
    assert_eq!(page.z(".a .b"), "1", "`.a > :scope` with the .b root");
}

// WPT css/css-cascade/scope-nesting.html, "Scoped nested group rule".
#[test]
fn wpt_scope_nesting_scoped_nested_group_rule() {
    let page = Page::new(
        "",
        "<div><style>@scope (.a) { .b:not(:scope) { @media (width) { z-index: 1; } } }</style>
         <div class=b></div><div class=a><div class=b></div></div></div>",
    );
    assert_eq!(page.z(".b"), "auto", "outside the scope");
    assert_eq!(
        page.z(".a .b"),
        "1",
        "declarations in a nested @media keep the scope"
    );
}

// WPT css/css-cascade/scope-nesting.html, "Scoped nested within another
// scope".
#[test]
fn wpt_scope_nesting_scoped_within_scoped() {
    let page = Page::new(
        "",
        "<div><style>
           @scope (.a) {
             @scope(#descendant) { :scope { z-index: 1; } }
             @scope (> #child) { :scope { z-index: 1; } }
           }
         </style>
         <div class=a><div id=descendant></div><div id=child></div></div></div>",
    );
    assert_eq!(
        page.z("#descendant"),
        "1",
        "an inner root inside the outer scope"
    );
    assert_eq!(page.z("#child"), "1", "a relative inner <scope-start>");
}

// WPT css/css-cascade/scope-nesting.html, "Implicit (prelude-less) @scope as
// a nested group rule".
#[test]
fn wpt_scope_nesting_implicit_scope_as_nested_group_rule() {
    let page = Page::new(
        "",
        "<div class=nest><style>.nest { @scope { #child { color: green; } } }</style>
         <div id=child>Foo</div></div>",
    );
    assert_eq!(
        page.doc.value(page.q("#child"), "color"),
        GREEN,
        "a prelude-less @scope nested in a style rule still applies"
    );
}

// ---------------------------------------------------------------------------
// scope-implicit.html, restated against the document element
// ---------------------------------------------------------------------------

// WPT css/css-cascade/scope-implicit.html, "@scope without prelude implicitly
// scopes to parent of owner node". The root here is the document element, so
// the `#outer` half (an element outside the root) has no counterpart.
#[test]
fn wpt_scope_implicit_basic() {
    let page = Page::at_root(
        "@scope { .a { z-index:1; } }",
        "<div><div id=inner class=a></div></div>",
    );
    assert_eq!(
        page.z("#inner"),
        "1",
        "a descendant of the implicit root is in scope"
    );
}

// WPT css/css-cascade/scope-implicit.html, ":scope can style implicit root".
#[test]
fn wpt_scope_implicit_scope_pseudo() {
    let page = Page::at_root(
        "@scope { :scope { z-index:1; } }",
        "<div><div></div></div><div><div id=root><div><div></div></div></div></div>",
    );
    assert_eq!(
        page.z_of(page.root()),
        "1",
        ":scope is the implicit root, the document element"
    );
    for id in page.all("div") {
        assert_eq!(page.z_of(id), "auto", "only the root matches :scope");
    }
}

// WPT css/css-cascade/scope-implicit.html, "@scope works with two identical
// stylesheets".
#[test]
fn wpt_scope_implicit_duplicate() {
    let mut page = Page::at_root(
        "@scope { .a { z-index:1; } }",
        "<div><div id=first class=a></div></div><div><div id=second class=a></div></div>",
    );
    page.doc.add_css("@scope { .a { z-index:1; } }");
    page.flush();
    assert_eq!(page.z("#first"), "1", "identical sheets both apply");
    assert_eq!(page.z("#second"), "1", "identical sheets both apply");
}

// WPT css/css-cascade/scope-implicit.html, "@scope with effectively empty
// :is() must not match anything".
#[test]
fn wpt_scope_implicit_forgiving() {
    let page = Page::new(
        "@scope ($invalid) { #a { z-index:1; } }",
        "<div id=a></div>",
    );
    assert_eq!(
        page.z("#a"),
        "auto",
        "an invalid <scope-start> matches no root"
    );
}

// WPT css/css-cascade/scope-implicit.html, "Implicit @scope has implicitly
// added :scope descendant combinator". `#div` is the document element here.
#[test]
fn wpt_scope_implicit_descendant() {
    let mut page = Page::at_root("@scope { #div { z-index:1; } }", "<div id=inner></div>");
    let root = page.root();
    page.doc.set_id(root, Some("div"));
    page.flush();
    assert_eq!(
        page.z_of(root),
        "auto",
        "an inner selector is `:scope <descendant>`, so the root never matches it"
    );
}

// WPT css/css-cascade/scope-implicit.html, "Implicit @scope with inner
// relative selector".
#[test]
fn wpt_scope_implicit_relative() {
    let page = Page::at_root(
        "@scope { > div { z-index:1; } }",
        "<div id=child><div id=inner></div></div>",
    );
    assert_eq!(
        page.z_of(page.root()),
        "auto",
        "the root is not its own child"
    );
    assert_eq!(
        page.z("#child"),
        "1",
        "`> div` is a child of the implicit root"
    );
    assert_eq!(page.z("#inner"), "auto", "a grandchild");
}

// WPT css/css-cascade/scope-implicit.html, "Implicit @scope with inner
// nesting selector".
#[test]
fn wpt_scope_implicit_nesting_selector() {
    let page = Page::at_root("@scope { & { z-index:1; } }", "<div id=inner></div>");
    assert_eq!(
        page.z_of(page.root()),
        "1",
        "`&` is :scope, the implicit root"
    );
    assert_eq!(page.z("#inner"), "auto", "only the root");
}

// WPT css/css-cascade/scope-implicit.html, "Implicit @scope with limit". The
// `#outer` half has no counterpart, as in `wpt_scope_implicit_basic`.
#[test]
fn wpt_scope_implicit_limit() {
    let page = Page::at_root(
        "@scope to (.b) { .a { z-index:1; } }",
        "<div id=inner class=a><div class=b><div id=outside_limit class=a></div></div></div>",
    );
    assert_eq!(page.z("#inner"), "1", "above the limit");
    assert_eq!(page.z("#outside_limit"), "auto", "under the limit");
}

// WPT css/css-cascade/scope-implicit.html, "Proximity calculation of
// multiple implicit @scope". All three document sheets share the document
// element as their root, so proximity ties and order of appearance decides;
// the WPT's middle sheet is an owner `<style>` with a nearer root.
#[test]
fn wpt_scope_implicit_concurrent_proximity() {
    let mut page = Page::at_root(
        "@scope { * { z-index: 1; } }",
        "<div><div id=inner></div></div><div id=outer></div>",
    );
    page.doc.add_css("@scope { * { z-index: 2; } }");
    page.doc.add_css("@scope { * { z-index: 3; } }");
    page.flush();
    assert_eq!(
        page.z("#inner"),
        "3",
        "one shared root: the last sheet wins"
    );
    assert_eq!(
        page.z("#outer"),
        "3",
        "one shared root: the last sheet wins"
    );
}

// WPT css/css-cascade/scope-implicit.html, "Implicit @scope sandwiched
// between non-implicit scopes" and "Implicit @scope sandwiched right under an
// explicit scope", inverted: the document element is never inside the
// `#outer` scope, so the implicit scope under it is empty (Gecko's
// resolution). `wpt_scope_implicit_in_explicit_shadow` has the WPT outcome.
#[test]
fn wpt_scope_implicit_sandwich_document_root() {
    let page = Page::new(
        "@scope (#outer) { @scope { @scope (#inner) { :scope { z-index: 1; } } } }
         @scope (#outer) { @scope { :scope { z-index: 1; } } }",
        "<div id=outer><div class=foo><div id=inner></div></div></div>",
    );
    assert_eq!(
        page.z("#inner"),
        "auto",
        "the implicit root is outside #outer's scope"
    );
    assert_eq!(
        page.z("#outer"),
        "auto",
        "the implicit root is outside #outer's scope"
    );
    assert_eq!(
        page.z_of(page.root()),
        "auto",
        "the root is not in #outer's scope"
    );
}

// WPT css/css-cascade/scope-implicit.html, "Implicit @scope sandwiched right
// under an explicit scope", with the sheet in `#outer`'s shadow tree: its
// implicit root is the host, which is the explicit scope's own root.
#[test]
fn wpt_scope_implicit_in_explicit_shadow() {
    let page = Page::new(
        "",
        "<div id=outer><template shadowrootmode=open>
           <style>@scope (:host) { @scope { :scope { z-index: 1; } } }</style>
         </template></div>",
    );
    assert_eq!(
        page.z("#outer"),
        "1",
        "an implicit root equal to the outer root is inside the outer scope"
    );
}

// ---------------------------------------------------------------------------
// scope-shadow.html
// ---------------------------------------------------------------------------

// WPT css/css-cascade/scope-shadow.html, "@scope can match :host".
#[test]
fn wpt_scope_shadow_match_host() {
    let page = Page::new(
        "",
        "<div id=host_plain><template shadowrootmode=open>
           <style>@scope (:host) { .a { z-index: 1; } }</style>
           <div class=a></div>
         </template></div>",
    );
    let a = page.q_in(page.shadow("#host_plain"), ".a");
    assert_eq!(page.z_of(a), "1", "<scope-start> can be the host");
}

// WPT css/css-cascade/scope-shadow.html, "@scope can match :host(...)".
#[test]
fn wpt_scope_shadow_match_host_functional() {
    let page = Page::new(
        "",
        "<div id=host_functional><template shadowrootmode=open>
           <style>
             @scope (:host(div)) { .a { z-index: 1; } }
             @scope (:host(span)) { .a { z-index: 42; } }
           </style>
           <div class=a></div>
         </template></div>",
    );
    let a = page.q_in(page.shadow("#host_functional"), ".a");
    assert_eq!(
        page.z_of(a),
        "1",
        ":host(div) matches the div host, :host(span) does not"
    );
}

// WPT css/css-cascade/scope-shadow.html, ":scope matches host via the
// scoping root".
#[test]
fn wpt_scope_shadow_scope_subject() {
    let page = Page::new(
        "",
        "<div id=host_scope_subject><template shadowrootmode=open>
           <style>@scope (:host) { :scope { z-index: 1; } }</style>
           <div class=a></div>
         </template></div>",
    );
    assert_eq!(
        page.z("#host_scope_subject"),
        "1",
        ":scope is the host root"
    );
}

// WPT css/css-cascade/scope-shadow.html, ":scope within :is() matches host
// via the scoping root".
#[test]
fn wpt_scope_shadow_scope_subject_is() {
    let page = Page::new(
        "",
        "<div id=host_scope_subject_is>
           <div class=host><template shadowrootmode=open>
             <style>
               :is(:scope, .a, .host) { z-index: 2; }
               @scope (:host) { :is(:scope, .a) { z-index: 1; } }
             </style>
             <div class=a></div>
           </template></div>
           <div class=a></div>
         </div>",
    );
    let host = page.q("#host_scope_subject_is > .host");
    assert_eq!(page.z_of(host), "1", ":scope inside :is() is the host root");
    let a = page.q_in(page.shadow("#host_scope_subject_is > .host"), ".a");
    assert_eq!(
        page.z_of(a),
        "1",
        "and .a inside the shadow tree is in scope"
    );
    assert_eq!(
        page.z("#host_scope_subject_is > .a"),
        "auto",
        "a shadow sheet never reaches the light tree"
    );
}

// WPT css/css-cascade/scope-shadow.html, "Implicit @scope as direct child of
// shadow root".
#[test]
fn wpt_scope_shadow_implicit_direct_child() {
    let page = Page::new(
        "",
        "<div id=implicit_scope_shadow_parent>
           <div class=host><template shadowrootmode=open>
             <style>@scope { :scope { z-index: 1; } :scope > .a { z-index: 2; } }</style>
             <div class=a></div>
           </template></div>
         </div>",
    );
    let host = page.q("#implicit_scope_shadow_parent > .host");
    let a = page.q_in(page.shadow("#implicit_scope_shadow_parent > .host"), ".a");
    assert_eq!(
        page.z_of(host),
        "1",
        "a shadow sheet's implicit root is the host"
    );
    assert_eq!(
        page.z_of(a),
        "2",
        ":scope > .a reaches the shadow tree's top level"
    );
}

// WPT css/css-cascade/scope-shadow.html, "Implicit @scope in construted
// stylesheet". A shadow sheet here is the analogue of the adopted sheet.
#[test]
fn wpt_scope_shadow_implicit_constructed() {
    let mut page = Page::new(
        "",
        "<div id=implicit_scope_constructed>
           <div class=host><template shadowrootmode=open><div class=a></div></template></div>
         </div>",
    );
    let shadow = page.shadow("#implicit_scope_constructed > .host");
    // The WPT text leaves the `@scope` block unclosed; CSS closes it at EOF.
    page.doc.dom.add_shadow_stylesheet(
        shadow,
        "@scope {
           :scope { z-index: 1; }
           :scope .a { z-index: 2; }",
    );
    page.flush();
    let host = page.q("#implicit_scope_constructed > .host");
    let a = page.q_in(shadow, ".a");
    assert_eq!(
        page.z_of(host),
        "1",
        "an adopted sheet's implicit root is the host"
    );
    assert_eq!(page.z_of(a), "2", "and its shadow tree is in scope");
}

// WPT css/css-cascade/scope-shadow.html, "Matching :host via &, :scope
// (subject)".
#[test]
fn wpt_scope_shadow_parent_pseudo_subject() {
    let page = Page::new(
        "",
        "<div id=scope_via_parent_pseudo_subject>
           <div class=host><template shadowrootmode=open>
             <style>@scope (:host) { :scope { & { z-index: 1; } } }</style>
           </template></div>
         </div>",
    );
    assert_eq!(
        page.z("#scope_via_parent_pseudo_subject > .host"),
        "1",
        "`&` under `:scope {{}}` is the host root"
    );
}

// WPT css/css-cascade/scope-shadow.html, "Matching :host via &, :scope
// (non-subject)".
#[test]
fn wpt_scope_shadow_parent_pseudo_non_subject() {
    let page = Page::new(
        "",
        "<div id=scope_via_parent_pseudo_non_subject>
           <div class=host><template shadowrootmode=open>
             <style>@scope (:host) { :scope .a { & { z-index: 1; } } }</style>
             <div class=a></div>
           </template></div>
         </div>",
    );
    let a = page.q_in(
        page.shadow("#scope_via_parent_pseudo_non_subject > .host"),
        ".a",
    );
    assert_eq!(
        page.z_of(a),
        "1",
        "`:scope .a` crosses from the host into its tree"
    );
}

// WPT css/css-cascade/scope-shadow.html, "Matching :host via &, :scope
// (non-subject, >)".
#[test]
fn wpt_scope_shadow_parent_pseudo_non_subject_child() {
    let page = Page::new(
        "",
        "<div id=scope_via_parent_pseudo_non_subject_child>
           <div class=host><template shadowrootmode=open>
             <style>@scope (:host) { :scope > .a { & { z-index: 1; } } }</style>
             <div class=a></div>
           </template></div>
         </div>",
    );
    let a = page.q_in(
        page.shadow("#scope_via_parent_pseudo_non_subject_child > .host"),
        ".a",
    );
    assert_eq!(
        page.z_of(a),
        "1",
        "`:scope > .a` crosses from the host into its tree"
    );
}

// ---------------------------------------------------------------------------
// scope-shadow-sharing.html (reftest), scope-ua-shadow-host.html (reftest)
// ---------------------------------------------------------------------------

// WPT css/css-cascade/scope-shadow-sharing.html: two shadow trees holding
// the same sheet text each resolve their own implicit root. The owner
// `<style>` nested in the first div is dropped (it has no counterpart here).
#[test]
fn wpt_scope_shadow_sharing() {
    const SHEET: &str = "
        div { background-color: white; }
        @scope { div { background-color: red; } }
        @scope (.explicit-scope) { div { background-color: green; } }";
    const TREE: &str = "<div class=plain><div></div></div>\
                        <div class=explicit-scope><div></div></div>";
    let page = Page::new(
        "",
        &format!(
            "<div id=host1><template shadowrootmode=open><style>{SHEET}</style>{TREE}</template></div>
             <div id=host2><template shadowrootmode=open><style>{SHEET}</style>{TREE}</template></div>"
        ),
    );
    for host in ["#host1", "#host2"] {
        let shadow = page.shadow(host);
        for (selectors, expected, why) in [
            (
                ".plain",
                "rgb(255, 0, 0)",
                "the implicit root (the host) scopes it",
            ),
            (
                ".plain > div",
                "rgb(255, 0, 0)",
                "the implicit root (the host) scopes it",
            ),
            (
                ".explicit-scope",
                "rgb(255, 0, 0)",
                "the explicit root does not match itself without :scope",
            ),
            (
                ".explicit-scope > div",
                GREEN,
                "the nearer explicit root wins by proximity",
            ),
        ] {
            assert_eq!(
                page.bg_of(page.q_in(shadow, selectors)),
                expected,
                "{host} `{selectors}`: {why}"
            );
        }
    }
}

// WPT css/css-cascade/scope-ua-shadow-host.html: an element with a shadow
// root of its own (the `<input>`'s UA shadow tree) still matches scoped rules
// from the tree it lives in.
#[test]
fn wpt_scope_ua_shadow_host() {
    let mut page = Page::new(
        "input { border: none; padding: 0; margin: 0; background-color: red; }
         @scope (.test) { input { width: 100px; height: 100px; background-color: green; } }",
        "<div class=test><input></div>",
    );
    let input = page.q("input");
    page.doc.dom.attach_shadow(input, ShadowRootMode::Open);
    page.flush();
    assert_eq!(
        page.bg_of(input),
        GREEN,
        "a shadow host is matched like any element"
    );
    assert_eq!(page.doc.value(input, "width"), "100px");
    assert_eq!(page.doc.value(input, "height"), "100px");
}

// ---------------------------------------------------------------------------
// scope-invalidation.html
// ---------------------------------------------------------------------------

fn inval_page(markup: &str) -> Page {
    Page::new(MAIN_BLACK, markup)
}

impl Page {
    fn add_class(&mut self, id: NodeId, class: &str) {
        self.doc.add_class(id, class);
        self.flush();
    }

    fn remove_class(&mut self, id: NodeId, class: &str) {
        self.doc.remove_class(id, class);
        self.flush();
    }
}

// WPT css/css-cascade/scope-invalidation.html, "Element becoming scope
// root".
#[test]
fn wpt_scope_invalidation_element_becoming_root() {
    let mut page = inval_page(
        "<style>@scope (.a) { span { background-color: green; } }</style>
         <div><span></span></div>",
    );
    let (div, span) = (page.q("div"), page.q("div > span"));
    let why = "toggling .a on the parent restyles the span";
    page.not_green_id(span, "span", why);
    page.add_class(div, "a");
    page.green_id(span, "span", why);
    page.remove_class(div, "a");
    page.not_green_id(span, "span", why);
}

// WPT css/css-cascade/scope-invalidation.html, "Element becoming scope root
// (selector list)".
#[test]
fn wpt_scope_invalidation_element_becoming_root_list() {
    let mut page = inval_page(
        "<style>@scope (.a, .b) { span { background-color: green; } }</style>
         <div><span></span></div>",
    );
    let (div, span) = (page.q("div"), page.q("div > span"));
    for class in ["a", "b"] {
        let why = format!("toggling .{class} on the parent restyles the span");
        page.not_green_id(span, "span", &why);
        page.add_class(div, class);
        page.green_id(span, "span", &why);
        page.remove_class(div, class);
        page.not_green_id(span, "span", &why);
    }
}

// WPT css/css-cascade/scope-invalidation.html, "Element becoming scope root,
// with inner :scope rule".
#[test]
fn wpt_scope_invalidation_becoming_root_inner_scope_rule() {
    let mut page = inval_page(
        "<style>@scope (.a) { :scope { background-color: green; } }</style>
         <div class=b></div>",
    );
    let b = page.q(".b");
    let why = "toggling .a restyles the root itself";
    page.not_green_id(b, ".b", why);
    page.add_class(b, "a");
    page.green_id(b, ".b", why);
    page.remove_class(b, "a");
    page.not_green_id(b, ".b", why);
}

// WPT css/css-cascade/scope-invalidation.html, "Parent element becoming
// scope limit".
#[test]
fn wpt_scope_invalidation_parent_becoming_limit() {
    let mut page = inval_page(
        "<style>@scope (.a) to (.b) { span { background-color: green; } }</style>
         <div class=a><div><span></span></div></div>",
    );
    let (inner, span) = (page.q(".a > div"), page.q(".a > div > span"));
    let why = "toggling the limit class on the parent restyles the span";
    page.green_id(span, "span", why);
    page.add_class(inner, "b");
    page.not_green_id(span, "span", why);
    page.remove_class(inner, "b");
    page.green_id(span, "span", why);
}

// WPT css/css-cascade/scope-invalidation.html, "Parent element becoming
// scope limit (selector list)".
#[test]
fn wpt_scope_invalidation_parent_becoming_limit_list() {
    let mut page = inval_page(
        "<style>@scope (.a) to (.b, .c) { span { background-color: green; } }</style>
         <div class=a><div><span></span></div></div>",
    );
    let (inner, span) = (page.q(".a > div"), page.q(".a > div > span"));
    for class in ["b", "c"] {
        let why = format!("toggling the .{class} limit on the parent restyles the span");
        page.green_id(span, "span", &why);
        page.add_class(inner, class);
        page.not_green_id(span, "span", &why);
        page.remove_class(inner, class);
        page.green_id(span, "span", &why);
    }
}

// WPT css/css-cascade/scope-invalidation.html, "Subject element becoming
// scope limit".
#[test]
fn wpt_scope_invalidation_subject_becoming_limit() {
    let mut page = inval_page(
        "<style>@scope (.a) to (.b) { span { background-color: green; } }</style>
         <div class=a><div><span></span></div></div>",
    );
    let span = page.q(".a > div > span");
    let why = "the subject turning into the limit leaves the scope";
    page.green_id(span, "span", why);
    page.add_class(span, "b");
    page.not_green_id(span, "span", why);
    page.remove_class(span, "b");
    page.green_id(span, "span", why);
}

// WPT css/css-cascade/scope-invalidation.html, "Parent element affecting
// scope limit".
#[test]
fn wpt_scope_invalidation_parent_affecting_limit() {
    let mut page = inval_page(
        "<style>@scope (.a) to (.b .c) { span { background-color: green; } }</style>
         <div class=a><div><div class=c><span></span></div></div></div>",
    );
    let (intermediate, span) = (page.q(".a > div"), page.q("span"));
    let why = "an ancestor completing the `.b .c` limit restyles the span";
    page.green_id(span, "span", why);
    page.add_class(intermediate, "b");
    page.not_green_id(span, "span", why);
    page.remove_class(intermediate, "b");
    page.green_id(span, "span", why);
}

// WPT css/css-cascade/scope-invalidation.html, "Sibling element affecting
// scope limit".
#[test]
fn wpt_scope_invalidation_sibling_affecting_limit() {
    let mut page = inval_page(
        "<style>@scope (.a) to (.b ~ .c) { span { background-color: green; } }</style>
         <div class=a><div></div><div></div><div></div><div></div>
           <div class=c><span></span></div></div>",
    );
    let (sibling, span) = (page.q(".a > div"), page.q("span"));
    let why = "a sibling completing the `.b ~ .c` limit restyles the span";
    page.green_id(span, "span", why);
    page.add_class(sibling, "b");
    page.not_green_id(span, "span", why);
    page.remove_class(sibling, "b");
    page.green_id(span, "span", why);
}

// WPT css/css-cascade/scope-invalidation.html, "Toggling inner/outer scope
// roots".
#[test]
fn wpt_scope_invalidation_toggling_inner_outer_roots() {
    let mut page = inval_page(
        "<style>@scope (.a) { @scope (.b) { span { background-color: green; } } }</style>
         <div><div><span></span></div></div>",
    );
    let (outer, inner, span) = (
        page.q(":scope > div"),
        page.q(":scope > div > div"),
        page.q("div > div > span"),
    );
    let why = "both nested roots must hold";
    page.not_green_id(span, "span", why);
    page.add_class(outer, "a");
    page.not_green_id(span, "span", why);
    page.add_class(inner, "b");
    page.green_id(span, "span", why);
    page.remove_class(inner, "b");
    page.not_green_id(span, "span", why);
    page.add_class(inner, "b");
    page.green_id(span, "span", why);
    page.remove_class(outer, "a");
    page.not_green_id(span, "span", why);
    page.add_class(outer, "a");
    page.green_id(span, "span", why);
}

// WPT css/css-cascade/scope-invalidation.html, "Element becoming root, with
// :scope in subject".
#[test]
fn wpt_scope_invalidation_becoming_root_scope_subject() {
    let mut page =
        inval_page("<style>@scope (.a) { :scope { background-color:green; } }</style><div></div>");
    let div = page.q("main > div");
    let why = "toggling .a restyles the root";
    page.not_green_id(div, "div", why);
    page.add_class(div, "a");
    page.green_id(div, "div", why);
    page.remove_class(div, "a");
    page.not_green_id(div, "div", why);
}

// WPT css/css-cascade/scope-invalidation.html, "Element becoming root, with
// :scope selected by ~ combinator".
#[test]
fn wpt_scope_invalidation_becoming_root_sibling_combinator() {
    let mut page = inval_page(
        "<style>@scope (.a) { .b ~ :scope { background-color:green; } }</style>
         <div></div><div></div>",
    );
    let (div1, div2) = (
        page.q("main > div:nth-of-type(1)"),
        page.q("main > div:nth-of-type(2)"),
    );
    let why = "`.b ~ :scope` needs both the root and the sibling";
    page.not_green_id(div2, "div2", why);
    page.add_class(div1, "b");
    page.not_green_id(div2, "div2", why);
    page.add_class(div2, "a");
    page.green_id(div2, "div2", why);
    page.remove_class(div1, "b");
    page.not_green_id(div2, "div2", why);
}

// WPT css/css-cascade/scope-invalidation.html, "Element becoming root via ~
// combinator".
#[test]
fn wpt_scope_invalidation_becoming_root_via_subsequent_sibling() {
    let mut page = inval_page(
        "<style>@scope (.a ~ .b) { .c { background-color:green; } }</style>
         <div><div></div><div></div><div></div><div class=b><div class=c></div></div></div>",
    );
    let (root, c) = (page.q("div > div:first-child"), page.q(".c"));
    let why = "an earlier sibling completing `.a ~ .b` restyles .c";
    page.not_green_id(c, ".c", why);
    page.add_class(root, "a");
    page.green_id(c, ".c", why);
    page.remove_class(root, "a");
    page.not_green_id(c, ".c", why);
}

// WPT css/css-cascade/scope-invalidation.html, "Element becoming root via +
// combinator".
#[test]
fn wpt_scope_invalidation_becoming_root_via_next_sibling() {
    let mut page = inval_page(
        "<style>@scope (.a + .b) { .c { background-color:green; } }</style>
         <div><div></div><div class=b><div class=c></div></div></div>",
    );
    let (root, c) = (page.q("div > div:first-child"), page.q(".c"));
    let why = "the previous sibling completing `.a + .b` restyles .c";
    page.not_green_id(c, ".c", why);
    page.add_class(root, "a");
    page.green_id(c, ".c", why);
    page.remove_class(root, "a");
    page.not_green_id(c, ".c", why);
}

// WPT css/css-cascade/scope-invalidation.html, ":not(scope) in subject".
#[test]
fn wpt_scope_invalidation_not_scope_in_subject() {
    let mut page = inval_page(
        "<style>@scope (.root) { :not(:scope) { background-color:green; } }</style>
         <div class=root><div class=a></div><div class=b></div><div class=c></div></div>
         <div class=a></div>",
    );
    let root = page.q(".root");
    let a1 = page.q(".root > .a");
    let b = page.q(".root > .b");
    let c = page.q(".root > .c");
    let a2 = page.q("main > .a");
    let check = |page: &Page, scoped: bool| {
        let why = "`:not(:scope)` matches the root's descendants only while it is a root";
        page.not_green_id(root, "root", why);
        for (id, what) in [(a1, "a1"), (b, "b"), (c, "c")] {
            if scoped {
                page.green_id(id, what, why);
            } else {
                page.not_green_id(id, what, why);
            }
        }
        page.not_green_id(a2, "a2", why);
    };
    check(&page, true);
    page.remove_class(root, "root");
    check(&page, false);
    page.add_class(root, "root");
    check(&page, true);
}

// WPT css/css-cascade/scope-invalidation.html, ":not(scope) in ancestor".
#[test]
fn wpt_scope_invalidation_not_scope_in_ancestor() {
    let mut page = inval_page(
        "<style>@scope (.root) { :not(:scope) > .a { background-color:green; } }</style>
         <div class=root><div class=a></div><div><div class=a></div></div></div>",
    );
    let root = page.q(".root");
    let (outer_a, inner_a) = (page.q(".root > .a"), page.q(".root > div > .a"));
    let why = "`:not(:scope) > .a` follows the root class";
    page.not_green_id(outer_a, "outer .a", why);
    page.green_id(inner_a, "inner .a", why);
    page.remove_class(root, "root");
    page.not_green_id(outer_a, "outer .a", why);
    page.not_green_id(inner_a, "inner .a", why);
    page.add_class(root, "root");
    page.not_green_id(outer_a, "outer .a", why);
    page.green_id(inner_a, "inner .a", why);
}

// WPT css/css-cascade/scope-invalidation.html, ":not(scope) in limit
// subject".
#[test]
fn wpt_scope_invalidation_not_scope_in_limit_subject() {
    let mut page = inval_page(
        "<style>@scope (.root) to (:not(:scope)) { :is(div, :scope) { background-color: green; } }</style>
         <div class=root><div class=a></div></div>",
    );
    let (root, a) = (page.q(".root"), page.q(".root > .a"));
    let why = "a `:not(:scope)` limit leaves only the root in scope";
    page.green_id(root, "root", why);
    page.not_green_id(a, ".a", why);
    page.remove_class(root, "root");
    page.not_green_id(root, "root", why);
    page.not_green_id(a, ".a", why);
    page.add_class(root, "root");
    page.green_id(root, "root", why);
    page.not_green_id(a, ".a", why);
}

// WPT css/css-cascade/scope-invalidation.html, ":not(scope) in limit
// ancestor".
#[test]
fn wpt_scope_invalidation_not_scope_in_limit_ancestor() {
    let mut page = inval_page(
        "<style>@scope (.root) to (:not(:scope) > .a) { :is(div, :scope) { background-color: green; } }</style>
         <div class=root><div class=a><div class=a></div></div></div>",
    );
    let root = page.q(".root");
    let (outer_a, inner_a) = (page.q(".root > .a"), page.q(".root > .a > .a"));
    let why = "the `:not(:scope) > .a` limit follows the root class";
    let check = |page: &Page, scoped: bool| {
        if scoped {
            page.green_id(root, "root", why);
            page.green_id(outer_a, "outer .a", why);
        } else {
            page.not_green_id(root, "root", why);
            page.not_green_id(outer_a, "outer .a", why);
        }
        page.not_green_id(inner_a, "inner .a", why);
    };
    check(&page, true);
    page.remove_class(root, "root");
    check(&page, false);
    page.add_class(root, "root");
    check(&page, true);
}

// WPT css/css-cascade/scope-invalidation.html, "Ancestor element affecting
// nested scope root (Through latter selector in list)".
#[test]
fn wpt_scope_invalidation_ancestor_affecting_nested_root() {
    let mut page = inval_page(
        "<style>@scope (.a .b) { @scope(.c .d, .e .f) { .g { background-color: green; } } }</style>
         <div class=a><div class=b><div class=e><div class=f><div class=g></div></div></div></div></div>",
    );
    let (a, g) = (page.q(".a"), page.q(".g"));
    let why = "the outer root's ancestor class gates the inner scope";
    page.green_id(g, ".g", why);
    page.remove_class(a, "a");
    page.not_green_id(g, ".g", why);
    page.add_class(a, "a");
    page.green_id(g, ".g", why);
}

// WPT css/css-cascade/scope-invalidation.html, "Parent element of subject
// affecting scope limit".
#[test]
fn wpt_scope_invalidation_parent_of_subject_affecting_limit() {
    let mut page = inval_page(
        "<style>@scope (.a) to (.b .c) { :scope .d .f { background-color: green; } }</style>
         <div class=a><div class=b><div class=d><div class=c><div class=f></div></div></div></div></div>",
    );
    let (b, f) = (page.q(".b"), page.q(".f"));
    let why = "removing .b dissolves the `.b .c` limit above .f";
    page.not_green_id(f, ".f", why);
    page.remove_class(b, "b");
    page.green_id(f, ".f", why);
    page.add_class(b, "b");
    page.not_green_id(f, ".f", why);
}

// WPT css/css-cascade/scope-invalidation.html, ":empty in scope root,
// invalidated by child insertion/removal".
#[test]
fn wpt_scope_invalidation_empty_root() {
    let mut page = inval_page(
        "<style>@scope (.root:empty) { :scope { background-color: green; } }</style>
         <div class=root></div>",
    );
    let root = page.q(".root");
    let why = "a child insertion or removal flips `:empty` on the root";
    page.green_id(root, "root", why);
    let child = page.doc.dom.create_element("div", ());
    page.doc.dom.append_child(root, child);
    page.flush();
    page.not_green_id(root, "root", why);
    page.doc.dom.remove_element(child);
    page.flush();
    page.green_id(root, "root", why);
}

// WPT css/css-cascade/scope-invalidation.html, ":nth-child() in scope root,
// invalidated by sibling insertion/removal".
#[test]
fn wpt_scope_invalidation_nth_child_root() {
    let mut page = inval_page(
        "<style>@scope (:nth-child(2)) { :scope { background-color: green; } }</style>
         <div id=wrapper><div class=first></div><div class=second></div></div>",
    );
    let wrapper = page.q("#wrapper");
    let (first, second) = (page.q("#wrapper > .first"), page.q("#wrapper > .second"));
    let why = "inserting a sibling shifts which element is :nth-child(2)";
    page.not_green_id(first, ".first", why);
    page.green_id(second, ".second", why);
    let inserted = page.doc.dom.create_element("div", ());
    page.doc.dom.insert_before(wrapper, inserted, Some(first));
    page.flush();
    page.not_green_id(inserted, "inserted", why);
    page.green_id(first, ".first", why);
    page.not_green_id(second, ".second", why);
    page.doc.dom.remove_element(inserted);
    page.flush();
    page.not_green_id(first, ".first", why);
    page.green_id(second, ".second", why);
}

// ---------------------------------------------------------------------------
// scope-style-sharing-001.html and -002.html
// ---------------------------------------------------------------------------

/// Two elements whose computed `z-index` must differ although the sharing
/// cache would otherwise hand one's style to the other.
fn sharing_case(css: &str, markup: &str, first: &str, second: &str, case: &str) {
    let page = Page::new(css, markup);
    assert_eq!(page.z("#first"), first, "{case}: #first");
    assert_eq!(page.z("#second"), second, "{case}: #second");
}

// WPT css/css-cascade/scope-style-sharing-001.html, tests 1-4, 7, 8, 11-13.
#[test]
fn wpt_scope_style_sharing_001_siblings() {
    let siblings =
        "<div id=first class=\"sibling foo\"></div><div id=second class=\"sibling foo\"></div>";
    let in_start = format!("<div class=scope-start>{siblings}</div>");
    let bare = "<div id=first class=sibling></div><div id=second class=sibling></div>";
    for (css, markup, first, second, case) in [
        (
            "@scope (.scope-start) to (.sibling + .sibling) { .foo { z-index: 1; } }",
            in_start.as_str(),
            "1",
            "auto",
            "second sibling exits scope",
        ),
        (
            "@scope (.scope-start) to (.sibling:not(.sibling + div)) { .foo { z-index: 1; } }",
            in_start.as_str(),
            "auto",
            "1",
            "first sibling matches scope",
        ),
        (
            "@scope (.sibling:not(.sibling + div)) { :scope { z-index: 1; } }",
            bare,
            "1",
            "auto",
            "first sibling enters scope",
        ),
        (
            "@scope (.sibling + .sibling) { :scope { z-index: 1; } }",
            bare,
            "auto",
            "1",
            "second sibling enters scope",
        ),
        (
            "@scope to (.sibling + .sibling) { .foo { z-index: 1; } }",
            siblings,
            "1",
            "auto",
            "second sibling exits implicit scope",
        ),
        (
            "@scope to (.sibling:not(.sibling + div)) { .foo { z-index: 1; } }",
            siblings,
            "auto",
            "1",
            "first sibling exits implicit scope",
        ),
        (
            "@scope (#first) { :scope { z-index: 1; } }",
            bare,
            "1",
            "auto",
            "first sibling enters scope with ID selector",
        ),
        (
            "@scope (#second) { :scope { z-index: 1; } }",
            bare,
            "auto",
            "1",
            "second sibling enters scope with ID selector",
        ),
        (
            "@scope (.root) { > div:nth-of-type(2) { z-index: 1; } }",
            "<div class=root><div id=first></div><div id=second></div></div>",
            "auto",
            "1",
            "with child relative implicit scope selector",
        ),
    ] {
        sharing_case(
            css,
            markup,
            first,
            second,
            &format!("@scope with sibling style sharing, {case}"),
        );
    }
}

/// A shadow host whose shadow sheet's implicit root is itself.
const HOST_SCOPE: &str = "<template shadowrootmode=open>\
    <style>@scope { :scope { z-index: 1; } }</style></template>";

// WPT css/css-cascade/scope-style-sharing-001.html, tests 5 and 6 ("first/
// second sibling has implicit scope"): the owner `<style>`'s parent becomes a
// shadow host whose shadow sheet holds the rule.
#[test]
fn wpt_scope_style_sharing_001_implicit_scope_host() {
    sharing_case(
        "",
        &format!(
            "<div id=first class=sibling>{HOST_SCOPE}</div><div id=second class=sibling></div>"
        ),
        "1",
        "auto",
        "@scope with sibling style sharing, first sibling has implicit scope",
    );
    sharing_case(
        "",
        &format!(
            "<div id=first class=sibling></div><div id=second class=sibling>{HOST_SCOPE}</div>"
        ),
        "auto",
        "1",
        "@scope with sibling style sharing, second sibling has implicit scope",
    );
}

// WPT css/css-cascade/scope-style-sharing-002.html, tests 1-4 and 11-17.
#[test]
fn wpt_scope_style_sharing_002_cousins() {
    let cousins = "<div class=scope-start><div id=first class=foo></div></div>\
                   <div class=scope-start><div id=second class=foo></div></div>";
    let parents = "<div id=first-parent><div id=first class=foo></div></div>\
                   <div id=second-parent><div id=second class=foo></div></div>";
    let attr_first = "<div class=root scope><div id=first class=foo></div></div>\
                      <div class=root><div id=second class=foo></div></div>";
    let attr_second = "<div class=root><div id=first class=foo></div></div>\
                       <div class=root scope><div id=second class=foo></div></div>";
    let order = "<div class=root scope-order=first><div id=first class=foo></div></div>\
                 <div class=root scope-order=second><div id=second class=foo></div></div>";
    for (css, markup, first, second, case) in [
        (
            "@scope (.scope-start) to (:scope:last-of-type > .foo) { .foo { z-index: 1; } }",
            cousins,
            "1",
            "auto",
            "second sibling exits scope",
        ),
        (
            "@scope (.scope-start) to (:scope:first-of-type > .foo) { .foo { z-index: 1; } }
             .scope-start:first-child { color: green; }",
            cousins,
            "auto",
            "1",
            "first sibling exits scope",
        ),
        (
            "@scope (.scope-start:first-of-type > .foo) { :scope { z-index: 1; } }",
            cousins,
            "1",
            "auto",
            "first sibling enters scope",
        ),
        (
            "@scope (.scope-start:last-of-type > .foo) { :scope { z-index: 1; } }",
            cousins,
            "auto",
            "1",
            "second sibling enters scope",
        ),
        (
            "@scope (#first-parent) { .foo { z-index: 1; } }",
            parents,
            "1",
            "auto",
            "first sibling enters scope ID selector",
        ),
        (
            "@scope (#second-parent) { .foo { z-index: 1; } }",
            parents,
            "auto",
            "1",
            "second sibling enters scope ID selector",
        ),
        (
            "@scope (.root) { > div:nth-of-type(2) > div { z-index: 1; } }",
            "<div class=root><div class=parent><div id=first></div></div>\
             <div class=parent><div id=second></div></div></div>",
            "auto",
            "1",
            "with child relative implicit scope selector",
        ),
        (
            "@scope (.root[scope]) { .foo { z-index: 1; } }",
            attr_first,
            "1",
            "auto",
            "first sibling enters scope trivial attribute selector",
        ),
        (
            "@scope (.root[scope]) { .foo { z-index: 1; } }",
            attr_second,
            "auto",
            "1",
            "second sibling enters scope trivial attribute selector",
        ),
        (
            "@scope (.root[scope-order=\"first\"]) { .foo { z-index: 1; } }",
            order,
            "1",
            "auto",
            "first sibling enters scope attribute selector",
        ),
        (
            "@scope (.root[scope-order=\"second\"]) { .foo { z-index: 1; } }",
            order,
            "auto",
            "1",
            "second sibling enters scope attribute selector",
        ),
    ] {
        sharing_case(
            css,
            markup,
            first,
            second,
            &format!("@scope with cousin style sharing, {case}"),
        );
    }
}

// WPT css/css-cascade/scope-style-sharing-002.html, tests 7 and 8 ("second/
// first sibling does not have implicit scope"), with the owner `<style>`'s
// parent as a shadow host.
#[test]
fn wpt_scope_style_sharing_002_implicit_scope_host() {
    sharing_case(
        "",
        &format!(
            "<div class=scope-start><div id=first class=foo>{HOST_SCOPE}</div></div>\
             <div class=scope-start><div id=second class=foo></div></div>"
        ),
        "1",
        "auto",
        "@scope with cousin style sharing, second sibling does not have implicit scope",
    );
    sharing_case(
        "",
        &format!(
            "<div class=scope-start><div id=first class=foo></div></div>\
             <div class=scope-start><div id=second class=foo>{HOST_SCOPE}</div></div>"
        ),
        "auto",
        "1",
        "@scope with cousin style sharing, first sibling does not have implicit scope",
    );
}

// ---------------------------------------------------------------------------
// scope-layer.html, scope-media.html, scope-supports.html
// ---------------------------------------------------------------------------

const GROUP_TREE: &str = "<div class=a><div class=b></div></div><div class=b></div>";

fn group_rule_case(css: &str, case: &str) {
    let page = Page::new(css, GROUP_TREE);
    assert_eq!(
        page.z("main > .a"),
        "1",
        "{case}: :scope inside the group rule"
    );
    assert_eq!(page.bg("main > .a > .b"), GREEN, "{case}: in scope");
    assert_eq!(page.bg("main > .b"), TRANSPARENT, "{case}: out of scope");
}

// WPT css/css-cascade/scope-layer.html, "Style rules within @layer are
// scoped".
#[test]
fn wpt_scope_layer() {
    group_rule_case(
        "@scope (.a) {
           @layer base, theme;
           @layer theme {
             :scope { z-index: 1; }
             .b { background-color: green; }
           }
         }
         @layer base {
           .a { z-index: 0; }
           .a .b { background-color: red; }
         }",
        "the theme layer beats base, and its rules keep the scope",
    );
}

// WPT css/css-cascade/scope-media.html, "Style rules within @media are
// scoped".
#[test]
fn wpt_scope_media() {
    group_rule_case(
        "@scope (.a) {
           @media (width > 0px) {
             :scope { z-index: 1; }
             .b { background-color: green; }
           }
         }",
        "rules in a nested @media keep the scope",
    );
}

// WPT css/css-cascade/scope-supports.html, "Style rules within @supports are
// scoped".
#[test]
fn wpt_scope_supports() {
    group_rule_case(
        "@scope (.a) {
           @supports (width:0px) {
             :scope { z-index: 1; }
             .b { background-color: green; }
           }
         }",
        "rules in a nested @supports keep the scope",
    );
}

// ---------------------------------------------------------------------------
// scope-deep.html, scope-featureless.html, scope-name-defining-rules.html
// ---------------------------------------------------------------------------

/// `count` nested `@scope (.sN)` rules around `span { green }`, and the
/// matching element chain; returns the span's background.
fn deep_scope_background(count: usize) -> String {
    let mut css = String::from("span { background-color: green; }");
    for i in (0..count).rev() {
        css = format!("@scope (.s{i}) {{ {css} }}");
    }
    let mut markup = String::from("<span></span>");
    for i in (0..count).rev() {
        markup = format!("<div class=s{i}>{markup}</div>");
    }
    let page = Page::new(&format!("{MAIN_BLACK} {css}"), &markup);
    let spans = page.all("span");
    assert_eq!(spans.len(), 1);
    page.bg_of(spans[0])
}

// WPT css/css-cascade/scope-deep.html, "Deep @scope nesting". The WPT nests
// 90 rules. cssparser stops descending at 75 nested blocks
// (`Parser::REASONABLE_NESTED_BLOCK_LIMIT`, a stack bound stylo inherits), so
// the rules below that depth are dropped at parse time: Firefox 157 fails
// this file the same way, Chrome 155 and Safari 27 pass it. 74 `@scope`
// blocks plus the style rule's block is the deepest sheet that parses whole.
#[test]
fn wpt_scope_deep() {
    assert_eq!(
        deep_scope_background(74),
        GREEN,
        "74 nested @scope rules all hold, and so does the 75th block"
    );
    assert_eq!(
        deep_scope_background(90),
        BLACK,
        "at the WPT's 90 the innermost rule is past cssparser's 75-block limit (Firefox's result)"
    );
}

// WPT css/css-cascade/scope-featureless.html (reftest): `:scope` outside
// @scope is the root element, not a featureless match.
#[test]
fn wpt_scope_featureless() {
    let page = Page::new(
        ":root { background-color: white; color: white; }
         :scope { --font-color: black; }
         #dut { color: var(--font-color); }",
        "<div id=dut>Test</div>",
    );
    assert_eq!(
        page.doc.value(page.q("#dut"), "color"),
        BLACK,
        "`:scope` outside @scope matches the root, whose custom property #dut inherits"
    );
}

/// Ticks the timeline once, so the animations the flush created start, and
/// reads `#animating`'s background.
fn animated_background(page: &mut Page) -> String {
    page.doc.dom.advance_animations(0.0);
    page.bg("#animating")
}

// WPT css/css-cascade/scope-name-defining-rules.html, "@keyframes is
// unaffected by @scope".
#[test]
fn wpt_scope_name_defining_rules_keyframes() {
    let mut page = Page::new(
        "@scope (#inner) {
           @keyframes --my-anim {
             from { background-color: rgb(0, 0, 255); }
             to { background-color: rgb(0, 0, 255); }
           }
         }
         @scope (#outer) {
           @keyframes --my-anim {
             from { background-color: rgb(0, 128, 0); }
             to { background-color: rgb(0, 128, 0); }
           }
         }
         #animating { animation: --my-anim 1000s linear; }",
        "<div id=outer><div id=inner><div id=animating></div></div></div>",
    );
    assert_eq!(
        animated_background(&mut page),
        GREEN,
        "@keyframes in @scope is global; the later one wins"
    );
}

// WPT css/css-cascade/scope-name-defining-rules.html, "@keyframes is
// unaffected by non-matching @scope".
#[test]
fn wpt_scope_name_defining_rules_keyframes_non_matching() {
    let mut page = Page::new(
        "@scope (#nomatch) {
           @keyframes --my-anim {
             from { background-color: rgb(0, 128, 0); }
             to { background-color: rgb(0, 128, 0); }
           }
         }
         #animating { animation: --my-anim 1000s linear; }",
        "<div id=animating></div>",
    );
    assert_eq!(
        animated_background(&mut page),
        GREEN,
        "@keyframes in a @scope that matches nothing still defines the name"
    );
}

// WPT css/css-cascade/scope-name-defining-rules.html, "@property is
// unaffected by @scope". The registered initial value is read through
// `width: var(--my-prop)`.
#[test]
fn wpt_scope_name_defining_rules_property() {
    let page = Page::new(
        "@scope (#inner) {
           @property --my-prop { syntax: \"<length>\"; initial-value: 1px; inherits: false; }
         }
         @scope (#outer) {
           @property --my-prop { syntax: \"<length>\"; initial-value: 2px; inherits: false; }
         }
         #subject { width: var(--my-prop); }",
        "<div id=outer><div id=inner><div id=subject></div></div></div>",
    );
    assert_eq!(
        page.doc.value(page.q("#subject"), "width"),
        "2px",
        "@property in @scope is global; the later registration wins"
    );
}

// WPT css/css-cascade/scope-name-defining-rules.html, "@property is
// unaffected by non-matching @scope".
#[test]
fn wpt_scope_name_defining_rules_property_non_matching() {
    let page = Page::new(
        "@scope (#nomatch) {
           @property --my-prop { syntax: \"<length>\"; initial-value: 2px; inherits: false; }
         }
         #subject { width: var(--my-prop); }",
        "<div id=subject></div>",
    );
    assert_eq!(
        page.doc.value(page.q("#subject"), "width"),
        "2px",
        "@property in a @scope that matches nothing still registers"
    );
}

// ---------------------------------------------------------------------------
// scope-hover.html, scope-focus.html: the state bits set directly
// ---------------------------------------------------------------------------

fn state_cases(state: ElementState, pseudo: &str) {
    // ":<state> via :scope in subject"
    let mut page = Page::new(
        "",
        &format!(
            "<div><style>@scope (.a:{pseudo}) {{ :scope {{ z-index: 1; }} }}</style>\
             <div class=a>1</div></div>"
        ),
    );
    let a = page.q(".a");
    assert_eq!(
        page.z_of(a),
        "auto",
        ":{pseudo} via :scope in subject: before"
    );
    page.doc.dom.add_element_state(a, state);
    page.flush();
    assert_eq!(page.z_of(a), "1", ":{pseudo} on the root makes it a root");

    // ":<state> via :scope in non-subject"
    let mut page = Page::new(
        "",
        &format!(
            "<div><style>@scope (.a:{pseudo}) {{ :scope .b {{ z-index: 1; }} }}</style>\
             <div class=a><div class=b>2</div></div></div>"
        ),
    );
    let (a, b) = (page.q(".a"), page.q(".b"));
    assert_eq!(
        page.z_of(b),
        "auto",
        ":{pseudo} via :scope in non-subject: before"
    );
    page.doc.dom.add_element_state(a, state);
    page.flush();
    assert_eq!(
        page.z_of(b),
        "1",
        ":{pseudo} on the root restyles its descendant"
    );

    // ":<state> in limit, :scope in subject"
    let mut page = Page::new(
        "",
        &format!(
            "<div><style>@scope (.a) to (:scope:{pseudo}) {{ :scope {{ z-index: 1; }} }}</style>\
             <div class=a tabindex=0>3</div></div>"
        ),
    );
    let a = page.q(".a");
    assert_eq!(page.z_of(a), "1", ":{pseudo} in limit: before");
    page.doc.dom.add_element_state(a, state);
    page.flush();
    assert_eq!(
        page.z_of(a),
        "auto",
        ":{pseudo} on the root makes it its own limit"
    );

    // ":<state> in intermediate limit, :scope in subject"
    let mut page = Page::new(
        "",
        &format!(
            "<div><style>@scope (.a) to (.b:{pseudo}) {{ .b {{ z-index: 1; }} }}</style>\
             <div class=a tabindex=0><div class=b tabindex=1>4</div></div></div>"
        ),
    );
    let b = page.q(".b");
    assert_eq!(page.z_of(b), "1", ":{pseudo} in intermediate limit: before");
    page.doc.dom.add_element_state(b, state);
    page.flush();
    assert_eq!(page.z_of(b), "auto", ":{pseudo} turns .b into a limit");
}

// WPT css/css-cascade/scope-hover.html, all four cases.
#[test]
fn wpt_scope_hover() {
    state_cases(ElementState::HOVER, "hover");
}

// WPT css/css-cascade/scope-focus.html, all four cases.
#[test]
fn wpt_scope_focus() {
    state_cases(ElementState::FOCUS, "focus");
}

// ---------------------------------------------------------------------------
// Crash tests: they pass if nothing panics
// ---------------------------------------------------------------------------

// WPT css/css-cascade/scope-declaration-list-crash.html.
#[test]
fn wpt_scope_declaration_list_crash() {
    let page = Page::new(
        "@scope (div) { z-index: 1; }
         @scope (div) { .a; }",
        "<p>PASS if no crash.</p>",
    );
    assert_eq!(page.z("p"), "auto");
}

// WPT css/css-cascade/scope-inner-pseudo-scope-crash.html.
#[test]
fn wpt_scope_inner_pseudo_scope_crash() {
    let page = Page::new(
        "@scope (.a) { @scope(:scope) { :scope { background-color: green; } } }",
        "<div class=a>PASS if no crash</div>",
    );
    assert_eq!(
        page.bg(".a"),
        GREEN,
        ":scope as the inner root is the outer root"
    );
}

// WPT css/css-cascade/scope-implicit-crash-print.html (print media aside).
#[test]
fn wpt_scope_implicit_crash() {
    let page = Page::at_root("@scope { :scope { background-color: transparent; } }", "");
    assert_eq!(page.bg_of(page.root()), TRANSPARENT);
}
