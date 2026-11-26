//! `Document::matched_styles`: the style rules, inline block and
//! presentational hints the cascade applied to an element.

#![cfg(feature = "devtools")]

mod common;

use common::Doc;
use terminus::link::{
    MatchedDeclaration, MatchedRule, MatchedSelector, MatchedStyles, NodeId, ShadowRootMode,
    StylesheetOrigin,
};

fn declaration(name: &str, value: &str, important: bool) -> MatchedDeclaration {
    MatchedDeclaration {
        name: name.to_owned(),
        value: value.to_owned(),
        important,
    }
}

fn selector(text: &str, matches: bool) -> MatchedSelector {
    MatchedSelector {
        text: text.to_owned(),
        matches,
    }
}

fn rule(
    origin: StylesheetOrigin,
    selectors: Vec<MatchedSelector>,
    declarations: Vec<MatchedDeclaration>,
) -> MatchedRule {
    MatchedRule {
        origin,
        selectors,
        declarations,
    }
}

fn matched(doc: &Doc, id: NodeId) -> MatchedStyles {
    doc.dom
        .matched_styles(id)
        .expect("a styled element answers its matched styles")
}

#[test]
fn rules_come_in_cascade_order_with_their_origins_and_texts() {
    let mut doc = Doc::new();
    doc.add_ua_css("div { display: block; }");
    doc.add_css(".a { height: 2px; } div { width: 1px; }");
    let element = doc.el(doc.root, "div.a[style=\"margin-top: 5px\"]");
    doc.flush();

    assert_eq!(
        matched(&doc, element),
        MatchedStyles {
            inline: Some(vec![declaration("margin-top", "5px", false)]),
            presentational: None,
            rules: vec![
                rule(
                    StylesheetOrigin::UserAgent,
                    vec![selector("div", true)],
                    vec![declaration("display", "block", false)],
                ),
                rule(
                    StylesheetOrigin::Author,
                    vec![selector("div", true)],
                    vec![declaration("width", "1px", false)],
                ),
                rule(
                    StylesheetOrigin::Author,
                    vec![selector(".a", true)],
                    vec![declaration("height", "2px", false)],
                ),
            ],
        }
    );
}

#[test]
fn a_rule_with_an_important_declaration_is_listed_once_at_its_normal_position() {
    let mut doc = Doc::with_css(
        ".b { width: 1px !important; height: 2px; }
         .b.c { height: 3px; }",
    );
    let element = doc.el(doc.root, "div.b.c");
    doc.flush();

    assert_eq!(
        matched(&doc, element).rules,
        vec![
            rule(
                StylesheetOrigin::Author,
                vec![selector(".b", true)],
                vec![
                    declaration("width", "1px", true),
                    declaration("height", "2px", false),
                ],
            ),
            rule(
                StylesheetOrigin::Author,
                vec![selector(".b.c", true)],
                vec![declaration("height", "3px", false)],
            ),
        ]
    );
}

#[test]
fn every_selector_of_the_list_is_listed_with_whether_it_matches() {
    let mut doc = Doc::with_css(".x, .y { width: 1px; --gap: 2px; }");
    let element = doc.el(doc.root, "div.x");
    doc.flush();

    assert_eq!(
        matched(&doc, element).rules,
        vec![rule(
            StylesheetOrigin::Author,
            vec![selector(".x", true), selector(".y", false)],
            vec![
                declaration("width", "1px", false),
                declaration("--gap", "2px", false),
            ],
        )]
    );
}

#[test]
fn rules_inside_media_and_nested_style_rules_are_found() {
    let mut doc = Doc::with_css(
        "@media (min-width: 1px) { .m { height: 2px; } }
         .p { width: 1px; & .q { height: 3px; } }",
    );
    let parent = doc.el(doc.root, "div.p");
    let element = doc.el(parent, "div.m.q");
    doc.flush();

    assert_eq!(
        matched(&doc, element).rules,
        vec![
            rule(
                StylesheetOrigin::Author,
                vec![selector(".m", true)],
                vec![declaration("height", "2px", false)],
            ),
            rule(
                StylesheetOrigin::Author,
                vec![selector("& .q", true)],
                vec![declaration("height", "3px", false)],
            ),
        ]
    );
}

#[test]
fn shadow_sheet_rules_match_inside_the_tree_and_host_rules_match_the_host() {
    let mut doc = Doc::new();
    let host = doc.el(doc.root, "div");
    let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);
    let inside = doc.el(shadow, "span.in");
    doc.dom
        .add_shadow_stylesheet(shadow, ":host { width: 4px; } .in { height: 5px; }");
    doc.flush();

    assert_eq!(
        matched(&doc, inside).rules,
        vec![rule(
            StylesheetOrigin::Author,
            vec![selector(".in", true)],
            vec![declaration("height", "5px", false)],
        )]
    );
    assert_eq!(
        matched(&doc, host).rules,
        vec![rule(
            StylesheetOrigin::Author,
            vec![selector(":host", true)],
            vec![declaration("width", "4px", false)],
        )]
    );
}

#[test]
fn presentational_hints_are_reported_apart_from_the_rules() {
    let mut doc = Doc::new();
    let element = doc.el(doc.root, "div");
    doc.dom.set_presentational_hint(element, "width", "7px");
    doc.flush();

    let styles = matched(&doc, element);
    assert_eq!(
        styles.presentational,
        Some(vec![declaration("width", "7px", false)])
    );
    assert_eq!(styles.inline, None);
    assert_eq!(styles.rules, Vec::new());
}

#[test]
fn stale_ids_text_nodes_and_unstyled_elements_answer_none() {
    let mut doc = Doc::new();
    let removed = doc.el(doc.root, "div");
    let text = doc.dom.create_text_node("text", ());
    doc.dom.append_child(doc.root, text);
    doc.flush();
    doc.dom.remove_element(removed);
    let unstyled = doc.el(doc.root, "div");

    assert_eq!(doc.dom.matched_styles(text), None);
    assert_eq!(doc.dom.matched_styles(unstyled), None);
    doc.dom.drop_element(removed);
    assert_eq!(doc.dom.matched_styles(removed), None);
}

#[test]
fn ua_only_longhands_are_not_listed() {
    let mut doc = Doc::new();
    doc.add_ua_css("div { -servo-top-layer: auto; height: 1px; }");
    let element = doc.el(doc.root, "div");
    doc.flush();

    assert_eq!(
        matched(&doc, element).rules,
        vec![rule(
            StylesheetOrigin::UserAgent,
            vec![selector("div", true)],
            vec![declaration("height", "1px", false)],
        )]
    );
}
