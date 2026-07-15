//! End-to-end coverage of the styling system: `StyleInfo` ingestion (direct
//! construction, cssId scoping, import flattening, `@keyframes`/`@font-face`),
//! the UA default sheet + page config, and the stylo-traversal flush
//! (initial styling, invalidation-set restyles, parallel == sequential).
#![allow(clippy::float_cmp)]

use app_units::Au;
use lynx_template_decoder::style_info::{
    CssProperty, CssPropertyId, DeclarationBlock, ParsedDeclaration, Rule, RulePrelude, RuleType,
    Selector, SimpleSelector, SimpleSelectorType, StyleInfo, StyleSheet, ValueToken, token_types,
};
use lynx_widget::{EngineMetrics, PageConfig, Parallelism, PseudoState, WidgetId, WidgetTree};
use stylo::color::AbsoluteColor;
use stylo::values::computed::Size;
use stylo::values::specified::box_::{DisplayInside, Overflow};

/// A 750×1334 CSS-px view (so `1rpx = 1px`) at DPR 2.
fn metrics() -> EngineMetrics {
    EngineMetrics::new(750.0, 1334.0, 2.0)
}

fn width_px(size: Size) -> f32 {
    match size {
        Size::LengthPercentage(lp) => lp.0.to_pixel_length(Au::new(0)).px(),
        other => panic!("expected a length width, got {other:?}"),
    }
}

fn red() -> AbsoluteColor {
    AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
}

fn color_of(tree: &WidgetTree, id: WidgetId) -> AbsoluteColor {
    tree.computed(id).expect("styled").clone_color()
}

// --- StyleInfo construction helpers -------------------------------------

fn simple(selector_type: SimpleSelectorType, value: &str) -> SimpleSelector {
    SimpleSelector {
        selector_type,
        value: value.to_owned(),
    }
}

fn class_selector(name: &str) -> Selector {
    Selector {
        simple_selectors: vec![simple(SimpleSelectorType::ClassSelector, name)],
    }
}

fn declaration(id: CssPropertyId, unknown_name: Option<&str>, value: &str) -> ParsedDeclaration {
    ParsedDeclaration {
        property_id: CssProperty {
            id,
            unknown_name: unknown_name.map(str::to_owned),
        },
        value_token_list: vec![ValueToken {
            token_type: token_types::IDENT_TOKEN,
            value: value.to_owned(),
        }],
        is_important: false,
    }
}

fn style_rule(selectors: Vec<Selector>, declarations: Vec<ParsedDeclaration>) -> Rule {
    Rule {
        rule_type: RuleType::Declaration,
        prelude: RulePrelude {
            selector_list: selectors,
        },
        declaration_block: DeclarationBlock { declarations },
        nested_rules: vec![],
    }
}

fn style_info(sheets: Vec<(i32, StyleSheet)>) -> StyleInfo {
    StyleInfo {
        css_id_to_style_sheet: sheets.into_iter().collect(),
        style_content_str_size_hint: 0,
    }
}

// --- flush basics --------------------------------------------------------

#[test]
fn flush_styles_the_tree_and_inherits() {
    let mut tree = WidgetTree::with_metrics(metrics());
    tree.add_stylesheet_str(".c { color: red; }", lynx_widget::StylesheetOrigin::Author);

    let page = tree.create_page();
    let view = tree.create_view();
    let inner = tree.create_view();
    tree.append_element(view, page).unwrap();
    tree.append_element(inner, view).unwrap();
    tree.set_classes(view, "c").unwrap();

    tree.flush_styles();

    assert_eq!(color_of(&tree, view), red());
    assert_eq!(
        color_of(&tree, inner),
        red(),
        "color inherits into the child"
    );
    assert!(!tree.has_dirty(), "flush clears the dirty state");
}

#[test]
fn ua_defaults_apply_and_follow_page_config() {
    // Default config: containers are linear, border-box, overflow hidden.
    let mut tree = WidgetTree::with_metrics(metrics());
    let page = tree.create_page();
    let view = tree.create_view();
    let text = tree.create_text();
    tree.append_element(view, page).unwrap();
    tree.append_element(text, page).unwrap();
    tree.flush_styles();

    let view_style = tree.computed(view).unwrap();
    assert_eq!(
        view_style.clone_display().inside(),
        DisplayInside::LynxLinear
    );
    assert_eq!(view_style.clone_overflow_x(), Overflow::Hidden);
    assert_eq!(
        view_style.clone_box_sizing(),
        stylo::properties::longhands::box_sizing::computed_value::T::BorderBox
    );
    let text_style = tree.computed(text).unwrap();
    assert_eq!(text_style.clone_display().inside(), DisplayInside::Flex);

    // defaultDisplayLinear=false + defaultOverflowVisible=true, as UA styles.
    let mut tree = WidgetTree::with_page_config(
        metrics(),
        PageConfig {
            default_display_linear: false,
            default_overflow_visible: true,
        },
    );
    let page = tree.create_page();
    let view = tree.create_view();
    tree.append_element(view, page).unwrap();
    tree.flush_styles();

    let view_style = tree.computed(view).unwrap();
    assert_eq!(view_style.clone_display().inside(), DisplayInside::Flex);
    assert_eq!(view_style.clone_overflow_x(), Overflow::Visible);
}

#[test]
fn author_styles_override_ua_defaults() {
    let mut tree = WidgetTree::with_metrics(metrics());
    tree.add_stylesheet_str(
        ".v { overflow: visible; display: flex; }",
        lynx_widget::StylesheetOrigin::Author,
    );
    let page = tree.create_page();
    let view = tree.create_view();
    tree.append_element(view, page).unwrap();
    tree.set_classes(view, "v").unwrap();
    tree.flush_styles();

    let style = tree.computed(view).unwrap();
    assert_eq!(style.clone_overflow_x(), Overflow::Visible);
    assert_eq!(style.clone_display().inside(), DisplayInside::Flex);
}

// --- incremental restyles -------------------------------------------------

#[test]
fn class_flip_restyles_precisely() {
    let mut tree = WidgetTree::with_metrics(metrics());
    tree.add_stylesheet_str(
        ".hot { color: red; }",
        lynx_widget::StylesheetOrigin::Author,
    );

    let page = tree.create_page();
    let target = tree.create_view();
    let bystander = tree.create_view();
    tree.append_element(target, page).unwrap();
    tree.append_element(bystander, page).unwrap();
    tree.flush_styles();

    let before_target = tree.computed(target).unwrap();
    let before_bystander = tree.computed(bystander).unwrap();
    assert_ne!(before_target.clone_color(), red());

    tree.set_classes(target, "hot").unwrap();
    assert!(tree.has_dirty());
    tree.flush_styles();

    assert_eq!(color_of(&tree, target), red());
    let after_bystander = tree.computed(bystander).unwrap();
    assert!(
        stylo::servo_arc::Arc::ptr_eq(&before_bystander, &after_bystander),
        "an unrelated sibling must keep its computed style identity"
    );
    drop(before_target);
}

#[test]
fn inline_style_update_applies_on_flush() {
    let mut tree = WidgetTree::with_metrics(metrics());
    tree.add_stylesheet_str(".c { color: red; }", lynx_widget::StylesheetOrigin::Author);

    let page = tree.create_page();
    let view = tree.create_view();
    tree.append_element(view, page).unwrap();
    tree.set_classes(view, "c").unwrap();
    tree.flush_styles();
    assert_eq!(color_of(&tree, view), red());

    tree.add_inline_style(view, "color", "blue").unwrap();
    tree.flush_styles();
    assert_eq!(
        color_of(&tree, view),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "inline declaration replaces the class rule's color"
    );
}

#[test]
fn pseudo_state_change_restyles_via_snapshot() {
    let mut tree = WidgetTree::with_metrics(metrics());
    tree.add_stylesheet_str(
        ".btn:active { color: red; }",
        lynx_widget::StylesheetOrigin::Author,
    );

    let page = tree.create_page();
    let btn = tree.create_view();
    tree.append_element(btn, page).unwrap();
    tree.set_classes(btn, "btn").unwrap();
    tree.flush_styles();
    assert_ne!(color_of(&tree, btn), red());

    tree.set_pseudo_state(btn, PseudoState::ACTIVE, true)
        .unwrap();
    tree.flush_styles();
    assert_eq!(color_of(&tree, btn), red());

    tree.set_pseudo_state(btn, PseudoState::ACTIVE, false)
        .unwrap();
    tree.flush_styles();
    assert_ne!(color_of(&tree, btn), red());
}

#[test]
fn empty_flip_restyles_later_sibling() {
    // `.list:empty + .hint` — removing the list's only child flips `:empty`,
    // which must restyle the *later sibling* (selector-flags-driven
    // structural invalidation).
    let mut tree = WidgetTree::with_metrics(metrics());
    tree.add_stylesheet_str(
        ".list:empty + .hint { color: red; }",
        lynx_widget::StylesheetOrigin::Author,
    );

    let page = tree.create_page();
    let list = tree.create_view();
    let hint = tree.create_view();
    let child = tree.create_view();
    tree.append_element(list, page).unwrap();
    tree.append_element(hint, page).unwrap();
    tree.set_classes(list, "list").unwrap();
    tree.set_classes(hint, "hint").unwrap();
    tree.append_element(child, list).unwrap();
    tree.flush_styles();
    assert_ne!(color_of(&tree, hint), red());

    tree.remove_element(list, child).unwrap();
    tree.flush_styles();
    assert_eq!(color_of(&tree, hint), red());

    tree.append_element(child, list).unwrap();
    tree.flush_styles();
    assert_ne!(color_of(&tree, hint), red());
}

// --- StyleInfo ingestion ---------------------------------------------------

#[test]
fn scoped_rules_match_only_their_css_id() {
    let info = style_info(vec![(
        2,
        StyleSheet {
            imports: vec![],
            rules: vec![style_rule(
                vec![class_selector("card")],
                vec![declaration(CssPropertyId::Color, None, "red")],
            )],
        },
    )]);

    let mut tree = WidgetTree::with_metrics(metrics());
    tree.load_style_info(&info);

    let page = tree.create_page();
    let scoped = tree.create_view();
    let unscoped = tree.create_view();
    tree.append_element(scoped, page).unwrap();
    tree.append_element(unscoped, page).unwrap();
    tree.set_classes(scoped, "card").unwrap();
    tree.set_classes(unscoped, "card").unwrap();
    tree.set_css_id(&[scoped], 2).unwrap();
    tree.flush_styles();

    assert_eq!(
        color_of(&tree, scoped),
        red(),
        ":where([l-css-id=\"2\"])-guarded rule matches the scoped widget"
    );
    assert_ne!(
        color_of(&tree, unscoped),
        red(),
        "widgets outside the css-id scope must not match"
    );
}

#[test]
fn css_id_zero_rules_are_global() {
    let info = style_info(vec![(
        0,
        StyleSheet {
            imports: vec![],
            rules: vec![style_rule(
                vec![class_selector("any")],
                vec![declaration(CssPropertyId::Color, None, "red")],
            )],
        },
    )]);

    let mut tree = WidgetTree::with_metrics(metrics());
    tree.load_style_info(&info);

    let page = tree.create_page();
    let plain = tree.create_view();
    let with_scope = tree.create_view();
    tree.append_element(plain, page).unwrap();
    tree.append_element(with_scope, page).unwrap();
    tree.set_classes(plain, "any").unwrap();
    tree.set_classes(with_scope, "any").unwrap();
    tree.set_css_id(&[with_scope], 7).unwrap();
    tree.flush_styles();

    assert_eq!(color_of(&tree, plain), red());
    assert_eq!(color_of(&tree, with_scope), red());
}

#[test]
fn imports_flatten_to_every_importer_scope() {
    // Fragment 1 imports fragment 2: fragment 2's rules apply both under
    // scope 2 and scope 1 (web-core's `imported_by` flattening).
    let info = style_info(vec![
        (
            1,
            StyleSheet {
                imports: vec![2],
                rules: vec![],
            },
        ),
        (
            2,
            StyleSheet {
                imports: vec![],
                rules: vec![style_rule(
                    vec![class_selector("imported")],
                    vec![declaration(CssPropertyId::Color, None, "red")],
                )],
            },
        ),
    ]);

    let mut tree = WidgetTree::with_metrics(metrics());
    tree.load_style_info(&info);

    let page = tree.create_page();
    let importer = tree.create_view();
    let owner = tree.create_view();
    let outsider = tree.create_view();
    for id in [importer, owner, outsider] {
        tree.append_element(id, page).unwrap();
        tree.set_classes(id, "imported").unwrap();
    }
    tree.set_css_id(&[importer], 1).unwrap();
    tree.set_css_id(&[owner], 2).unwrap();
    tree.set_css_id(&[outsider], 3).unwrap();
    tree.flush_styles();

    assert_eq!(color_of(&tree, importer), red(), "importer scope applies");
    assert_eq!(color_of(&tree, owner), red(), "owning scope applies");
    assert_ne!(color_of(&tree, outsider), red(), "unrelated scope must not");
}

#[test]
fn keyframes_and_font_face_are_registered() {
    let keyframes = Rule {
        rule_type: RuleType::KeyFrames,
        prelude: RulePrelude {
            selector_list: vec![Selector {
                simple_selectors: vec![simple(SimpleSelectorType::UnknownText, "spin")],
            }],
        },
        declaration_block: DeclarationBlock {
            declarations: vec![],
        },
        nested_rules: vec![Rule {
            rule_type: RuleType::Declaration,
            prelude: RulePrelude {
                selector_list: vec![Selector {
                    simple_selectors: vec![simple(SimpleSelectorType::UnknownText, "to")],
                }],
            },
            declaration_block: DeclarationBlock {
                declarations: vec![declaration(
                    CssPropertyId::Transform,
                    None,
                    "rotate(360deg)",
                )],
            },
            nested_rules: vec![],
        }],
    };
    let font_face = Rule {
        rule_type: RuleType::FontFace,
        prelude: RulePrelude {
            selector_list: vec![],
        },
        declaration_block: DeclarationBlock {
            declarations: vec![
                declaration(CssPropertyId::FontFamily, None, "MyFont"),
                declaration(CssPropertyId::Unknown, Some("src"), "url(\"myfont.woff2\")"),
            ],
        },
        nested_rules: vec![],
    };
    let info = style_info(vec![(
        0,
        StyleSheet {
            imports: vec![],
            rules: vec![keyframes, font_face],
        },
    )]);

    let mut tree = WidgetTree::with_metrics(metrics());
    tree.load_style_info(&info);
    assert_eq!(tree.document().font_face_count(), 1);

    let page = tree.create_page();
    tree.flush_styles();
    assert!(
        tree.document()
            .has_keyframes_animation("spin", tree.widget_ref(page).unwrap()),
        "@keyframes spin must be registered with the stylist"
    );
}

#[test]
fn fixture_bundle_styles_end_to_end() {
    // Real .web.bundle → decode → ingest → flush.
    const BUNDLE: &[u8] = include_bytes!(
        "../../lynx-template-decoder/tests/fixtures/basic-class-selector.web.bundle"
    );
    let template = lynx_template_decoder::decode(BUNDLE).unwrap();
    let info = template.style_info.expect("fixture carries StyleInfo");

    let mut tree = WidgetTree::with_metrics(metrics());
    tree.load_style_info(&info);

    let page = tree.create_page();
    let card = tree.create_view();
    tree.append_element(card, page).unwrap();
    tree.set_classes(card, "basic").unwrap();
    tree.flush_styles();

    // `.basic { background-color: pink; height: 100px; width: 100px }`
    let style = tree.computed(card).unwrap();
    assert_eq!(width_px(style.clone_width()), 100.0);
    assert_eq!(width_px(style.clone_height()), 100.0);
}

// --- scheduling -------------------------------------------------------------

#[test]
fn parallel_flush_matches_sequential() {
    fn build(parallelism: Parallelism) -> Vec<(f32, AbsoluteColor)> {
        let mut tree = WidgetTree::with_metrics(metrics());
        tree.add_stylesheet_str(
            ".outer { color: red; } .even { width: 10px; } .odd { width: 20px; color: blue; }",
            lynx_widget::StylesheetOrigin::Author,
        );
        let page = tree.create_page();
        let mut leaves = Vec::new();
        for section in 0..16 {
            let container = tree.create_view();
            tree.append_element(container, page).unwrap();
            tree.set_classes(container, "outer").unwrap();
            for item in 0..16 {
                let leaf = tree.create_view();
                tree.append_element(leaf, container).unwrap();
                tree.set_classes(
                    leaf,
                    if (section + item) % 2 == 0 {
                        "even"
                    } else {
                        "odd"
                    },
                )
                .unwrap();
                leaves.push(leaf);
            }
        }
        tree.flush_styles_with(parallelism);
        leaves
            .into_iter()
            .map(|leaf| {
                let style = tree.computed(leaf).unwrap();
                (width_px(style.clone_width()), style.clone_color())
            })
            .collect()
    }

    let sequential = build(Parallelism::Sequential);
    let parallel = build(Parallelism::Auto);
    assert_eq!(sequential, parallel);
}

// --- review-driven regression tests -----------------------------------------

#[test]
fn edge_child_restyle_on_append_and_prepend() {
    // Appending displaces the old `:last-child` (prepending the old
    // `:first-child`) one slot inward; the displaced element must drop its
    // edge styling.
    let mut tree = WidgetTree::with_metrics(metrics());
    tree.add_stylesheet_str(
        ".item:last-child { color: red; } .item:first-child { width: 42px; }",
        lynx_widget::StylesheetOrigin::Author,
    );

    let page = tree.create_page();
    let a = tree.create_view();
    let b = tree.create_view();
    for id in [a, b] {
        tree.append_element(id, page).unwrap();
        tree.set_classes(id, "item").unwrap();
    }
    tree.flush_styles();
    assert_eq!(color_of(&tree, b), red(), "b is :last-child");
    assert_eq!(
        width_px(tree.computed(a).unwrap().clone_width()),
        42.0,
        "a is :first-child"
    );

    // Append: b stops being :last-child.
    let c = tree.create_view();
    tree.append_element(c, page).unwrap();
    tree.set_classes(c, "item").unwrap();
    tree.flush_styles();
    assert_ne!(
        color_of(&tree, b),
        red(),
        "displaced old :last-child must be restyled on append"
    );
    assert_eq!(
        color_of(&tree, c),
        red(),
        "new last child gains the styling"
    );

    // Prepend: a stops being :first-child.
    let front = tree.create_view();
    tree.insert_element_before(front, page, Some(a)).unwrap();
    tree.set_classes(front, "item").unwrap();
    tree.flush_styles();
    assert!(
        matches!(tree.computed(a).unwrap().clone_width(), Size::Auto),
        "displaced old :first-child must be restyled back to width:auto on prepend"
    );
    assert_eq!(
        width_px(tree.computed(front).unwrap().clone_width()),
        42.0,
        "new first child gains the styling"
    );
}

#[test]
fn viewport_change_restyles_flushed_tree() {
    let mut tree = WidgetTree::with_metrics(metrics());
    tree.add_stylesheet_str(
        ".box { width: 100rpx; }",
        lynx_widget::StylesheetOrigin::Author,
    );

    let page = tree.create_page();
    let view = tree.create_view();
    tree.append_element(view, page).unwrap();
    tree.set_classes(view, "box").unwrap();
    tree.flush_styles();
    assert_eq!(width_px(tree.computed(view).unwrap().clone_width()), 100.0);

    // 1rpx = viewport_width / 750: doubling the width doubles rpx lengths.
    tree.set_viewport(1500.0, 1334.0);
    tree.flush_styles();
    assert_eq!(
        width_px(tree.computed(view).unwrap().clone_width()),
        200.0,
        "rpx lengths must re-resolve against the new viewport"
    );
}

#[test]
fn self_importing_fragment_is_dropped() {
    // web-core parity: a self-import is a one-node cycle; the fragment (and
    // anything only reachable through it) is dropped entirely.
    let info = style_info(vec![
        (
            5,
            StyleSheet {
                imports: vec![5, 6],
                rules: vec![style_rule(
                    vec![class_selector("cyclic")],
                    vec![declaration(CssPropertyId::Color, None, "red")],
                )],
            },
        ),
        (
            6,
            StyleSheet {
                imports: vec![],
                rules: vec![style_rule(
                    vec![class_selector("leaf")],
                    vec![declaration(CssPropertyId::Color, None, "red")],
                )],
            },
        ),
    ]);

    let mut tree = WidgetTree::with_metrics(metrics());
    tree.load_style_info(&info);

    let page = tree.create_page();
    let cyclic = tree.create_view();
    let leaf = tree.create_view();
    tree.append_element(cyclic, page).unwrap();
    tree.append_element(leaf, page).unwrap();
    tree.set_classes(cyclic, "cyclic").unwrap();
    tree.set_classes(leaf, "leaf").unwrap();
    tree.set_css_id(&[cyclic], 5).unwrap();
    tree.set_css_id(&[leaf], 6).unwrap();
    tree.flush_styles();

    assert_ne!(
        color_of(&tree, cyclic),
        red(),
        "self-importing fragment's rules must be dropped"
    );
    assert_ne!(
        color_of(&tree, leaf),
        red(),
        "fragment 6 is only reachable through the cycle and drops with it"
    );
}
