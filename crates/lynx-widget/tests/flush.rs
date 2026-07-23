//! End-to-end coverage of the styling system: `StyleInfo` ingestion (direct
//! construction, cssId scoping, import flattening, `@keyframes`/`@font-face`),
//! the UA default sheet + page config, and the stylo-traversal flush
//! (initial styling, invalidation-set restyles, parallel == sequential).
#![allow(clippy::float_cmp)]

use app_units::Au;
use lynx_template_decoder::style_info::{
    CssProperty, CssPropertyId, DeclarationBlock, ParsedDeclaration, Rule, RuleKind, RulePrelude,
    Selector, SimpleSelector, SimpleSelectorKind, StyleInfo, StyleSheet, ValueToken, token_types,
};
use lynx_widget::{
    ElementState, PageConfig, Parallelism, StyleEngine, ViewMetrics, WidgetHandle, WidgetTree,
};
use stylo::color::AbsoluteColor;
use stylo::values::computed::Size;
use stylo::values::specified::box_::{DisplayInside, Overflow};

fn metrics() -> ViewMetrics {
    ViewMetrics::new(750.0, 1334.0, 2.0)
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

fn color_of(tree: &WidgetTree, handle: &WidgetHandle) -> AbsoluteColor {
    tree.computed_style(handle)
        .expect("same-tree handle")
        .expect("styled")
        .clone_color()
}

fn simple(kind: SimpleSelectorKind, value: &str) -> SimpleSelector {
    SimpleSelector {
        kind,
        value: value.to_owned(),
    }
}

fn class_selector(name: &str) -> Selector {
    Selector {
        components: vec![simple(SimpleSelectorKind::Class, name)],
    }
}

fn declaration(id: CssPropertyId, unknown_name: Option<&str>, value: &str) -> ParsedDeclaration {
    ParsedDeclaration {
        property: CssProperty {
            id,
            unknown_name: unknown_name.map(str::to_owned),
        },
        value_tokens: vec![ValueToken {
            token_type: token_types::IDENT_TOKEN,
            value: value.to_owned(),
        }],
        is_important: false,
    }
}

fn style_rule(selectors: Vec<Selector>, declarations: Vec<ParsedDeclaration>) -> Rule {
    Rule {
        kind: RuleKind::Style,
        prelude: RulePrelude { selectors },
        declaration_block: DeclarationBlock { declarations },
        children: vec![],
    }
}

fn style_info(sheets: Vec<(i32, StyleSheet)>) -> StyleInfo {
    StyleInfo {
        css_id_to_style_sheet: sheets.into_iter().collect(),
        style_text_size_hint: 0,
    }
}

#[test]
fn flush_styles_the_tree_and_inherits() {
    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.add_stylesheet(
        &mut tree,
        ".c { color: red; }",
        lynx_widget::StylesheetOrigin::Author,
    );
    let page = tree.create_page();
    let view = tree.create_view();
    let inner = tree.create_view();
    tree.append_child(&page, &view).unwrap();
    tree.append_child(&view, &inner).unwrap();
    tree.set_classes(&view, "c").unwrap();

    engine.flush_styles(&mut tree);

    assert_eq!(color_of(&tree, &view), red());
    assert_eq!(
        color_of(&tree, &inner),
        red(),
        "color inherits into the child"
    );
}

#[test]
fn ua_defaults_apply_and_follow_page_config() {
    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    let page = tree.create_page();
    let view = tree.create_view();
    let text = tree.create_text();
    tree.append_child(&page, &view).unwrap();
    tree.append_child(&page, &text).unwrap();
    engine.flush_styles(&mut tree);

    let view_style = tree.computed_style(&view).unwrap().unwrap();
    assert_eq!(
        view_style.clone_display().inside(),
        DisplayInside::LynxLinear
    );
    assert_eq!(view_style.clone_overflow_x(), Overflow::Hidden);
    assert_eq!(
        view_style.clone_box_sizing(),
        stylo::properties::longhands::box_sizing::computed_value::T::BorderBox
    );
    let text_style = tree.computed_style(&text).unwrap().unwrap();
    assert_eq!(text_style.clone_display().inside(), DisplayInside::Flex);

    let engine = StyleEngine::with_page_config(
        metrics(),
        PageConfig {
            default_display_linear: false,
            default_overflow_visible: true,
        },
    );
    let mut tree = engine.new_tree();
    let page = tree.create_page();
    let view = tree.create_view();
    tree.append_child(&page, &view).unwrap();
    engine.flush_styles(&mut tree);

    let view_style = tree.computed_style(&view).unwrap().unwrap();
    assert_eq!(view_style.clone_display().inside(), DisplayInside::Flex);
    assert_eq!(view_style.clone_overflow_x(), Overflow::Visible);
}

#[test]
fn author_styles_override_ua_defaults() {
    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.add_stylesheet(
        &mut tree,
        ".v { overflow: visible; display: flex; }",
        lynx_widget::StylesheetOrigin::Author,
    );
    let page = tree.create_page();
    let view = tree.create_view();
    tree.append_child(&page, &view).unwrap();
    tree.set_classes(&view, "v").unwrap();
    engine.flush_styles(&mut tree);

    let style = tree.computed_style(&view).unwrap().unwrap();
    assert_eq!(style.clone_overflow_x(), Overflow::Visible);
    assert_eq!(style.clone_display().inside(), DisplayInside::Flex);
}

#[test]
fn class_flip_restyles_precisely() {
    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.add_stylesheet(
        &mut tree,
        ".hot { color: red; }",
        lynx_widget::StylesheetOrigin::Author,
    );

    let page = tree.create_page();
    let target = tree.create_view();
    let bystander = tree.create_view();
    tree.append_child(&page, &target).unwrap();
    tree.append_child(&page, &bystander).unwrap();
    engine.flush_styles(&mut tree);

    let before_target = tree.computed_style(&target).unwrap().unwrap();
    let before_bystander = tree.computed_style(&bystander).unwrap().unwrap();
    assert_ne!(before_target.clone_color(), red());

    tree.set_classes(&target, "hot").unwrap();
    engine.flush_styles(&mut tree);

    assert_eq!(color_of(&tree, &target), red());
    let after_bystander = tree.computed_style(&bystander).unwrap().unwrap();
    assert!(
        stylo::servo_arc::Arc::ptr_eq(&before_bystander, &after_bystander),
        "an unrelated sibling must keep its computed style identity"
    );
    drop(before_target);
}

#[test]
fn inline_style_update_applies_on_flush() {
    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.add_stylesheet(
        &mut tree,
        ".c { color: red; }",
        lynx_widget::StylesheetOrigin::Author,
    );
    let page = tree.create_page();
    let view = tree.create_view();
    tree.append_child(&page, &view).unwrap();
    tree.set_classes(&view, "c").unwrap();
    engine.flush_styles(&mut tree);
    assert_eq!(color_of(&tree, &view), red());

    tree.add_inline_style(&view, "color", "blue").unwrap();
    engine.flush_styles(&mut tree);
    assert_eq!(
        color_of(&tree, &view),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "inline declaration replaces the class rule's color"
    );
}

#[test]
fn pseudo_state_change_restyles_via_snapshot() {
    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.add_stylesheet(
        &mut tree,
        ".btn:active { color: red; }",
        lynx_widget::StylesheetOrigin::Author,
    );

    let page = tree.create_page();
    let btn = tree.create_view();
    tree.append_child(&page, &btn).unwrap();
    tree.set_classes(&btn, "btn").unwrap();
    engine.flush_styles(&mut tree);
    assert_ne!(color_of(&tree, &btn), red());

    tree.enable_pseudo_state(&btn, ElementState::ACTIVE)
        .unwrap();
    engine.flush_styles(&mut tree);
    assert_eq!(color_of(&tree, &btn), red());

    tree.disable_pseudo_state(&btn, ElementState::ACTIVE)
        .unwrap();
    engine.flush_styles(&mut tree);
    assert_ne!(color_of(&tree, &btn), red());
}

#[test]
fn empty_flip_restyles_later_sibling() {
    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.add_stylesheet(
        &mut tree,
        ".list:empty + .hint { color: red; }",
        lynx_widget::StylesheetOrigin::Author,
    );

    let page = tree.create_page();
    let list = tree.create_view();
    let hint = tree.create_view();
    let child = tree.create_view();
    tree.append_child(&page, &list).unwrap();
    tree.append_child(&page, &hint).unwrap();
    tree.set_classes(&list, "list").unwrap();
    tree.set_classes(&hint, "hint").unwrap();
    tree.append_child(&list, &child).unwrap();
    engine.flush_styles(&mut tree);
    assert_ne!(color_of(&tree, &hint), red());

    tree.remove_child(&list, &child).unwrap();
    engine.flush_styles(&mut tree);
    assert_eq!(color_of(&tree, &hint), red());

    tree.append_child(&list, &child).unwrap();
    engine.flush_styles(&mut tree);
    assert_ne!(color_of(&tree, &hint), red());
}

#[test]
fn dataset_attributes_match_and_invalidate_selectors() {
    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.add_stylesheet(
        &mut tree,
        r#"[data-role="hero"] { color: red; }"#,
        lynx_widget::StylesheetOrigin::Author,
    );

    let page = tree.create_page();
    let view = tree.create_view();
    tree.append_child(&page, &view).unwrap();
    engine.flush_styles(&mut tree);
    assert_ne!(color_of(&tree, &view), red());

    tree.set_dataset(&view, [("role", "hero")]).unwrap();
    assert_eq!(
        tree.widget(&view).unwrap().attribute("data-role"),
        Some("hero")
    );
    engine.flush_styles(&mut tree);
    assert_eq!(color_of(&tree, &view), red());

    tree.set_dataset(&view, [("role", "ordinary")]).unwrap();
    engine.flush_styles(&mut tree);
    assert_ne!(color_of(&tree, &view), red());
}

#[test]
fn loading_style_info_restyles_an_already_flushed_tree() {
    let info = style_info(vec![(
        0,
        StyleSheet {
            imports: vec![],
            rules: vec![style_rule(
                vec![class_selector("late")],
                vec![declaration(CssPropertyId::Color, None, "red")],
            )],
        },
    )]);

    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    let page = tree.create_page();
    let view = tree.create_view();
    tree.append_child(&page, &view).unwrap();
    tree.set_classes(&view, "late").unwrap();
    engine.flush_styles(&mut tree);
    assert_ne!(color_of(&tree, &view), red());

    engine.load_style_info(&mut tree, &info);
    engine.flush_styles(&mut tree);
    assert_eq!(color_of(&tree, &view), red());
}

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

    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.load_style_info(&mut tree, &info);
    let page = tree.create_page();
    let scoped = tree.create_view();
    let unscoped = tree.create_view();
    tree.append_child(&page, &scoped).unwrap();
    tree.append_child(&page, &unscoped).unwrap();
    tree.set_classes(&scoped, "card").unwrap();
    tree.set_classes(&unscoped, "card").unwrap();
    tree.set_css_id(&[&scoped], 2).unwrap();
    engine.flush_styles(&mut tree);

    assert_eq!(
        color_of(&tree, &scoped),
        red(),
        ":where([l-css-id=\"2\"])-guarded rule matches the scoped widget"
    );
    assert_ne!(
        color_of(&tree, &unscoped),
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

    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.load_style_info(&mut tree, &info);
    let page = tree.create_page();
    let plain = tree.create_view();
    let with_scope = tree.create_view();
    tree.append_child(&page, &plain).unwrap();
    tree.append_child(&page, &with_scope).unwrap();
    tree.set_classes(&plain, "any").unwrap();
    tree.set_classes(&with_scope, "any").unwrap();
    tree.set_css_id(&[&with_scope], 7).unwrap();
    engine.flush_styles(&mut tree);

    assert_eq!(color_of(&tree, &plain), red());
    assert_eq!(color_of(&tree, &with_scope), red());
}

#[test]
fn imports_flatten_to_every_importer_scope() {
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

    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.load_style_info(&mut tree, &info);
    let page = tree.create_page();
    let importer = tree.create_view();
    let owner = tree.create_view();
    let outsider = tree.create_view();
    for id in [&importer, &owner, &outsider] {
        tree.append_child(&page, id).unwrap();
        tree.set_classes(id, "imported").unwrap();
    }
    tree.set_css_id(&[&importer], 1).unwrap();
    tree.set_css_id(&[&owner], 2).unwrap();
    tree.set_css_id(&[&outsider], 3).unwrap();
    engine.flush_styles(&mut tree);

    assert_eq!(color_of(&tree, &importer), red(), "importer scope applies");
    assert_eq!(color_of(&tree, &owner), red(), "owning scope applies");
    assert_ne!(
        color_of(&tree, &outsider),
        red(),
        "unrelated scope must not"
    );
}

#[test]
fn keyframes_and_font_face_are_registered() {
    let keyframes = Rule {
        kind: RuleKind::Keyframes,
        prelude: RulePrelude {
            selectors: vec![Selector {
                components: vec![simple(SimpleSelectorKind::UnknownText, "spin")],
            }],
        },
        declaration_block: DeclarationBlock {
            declarations: vec![],
        },
        children: vec![Rule {
            kind: RuleKind::Style,
            prelude: RulePrelude {
                selectors: vec![Selector {
                    components: vec![simple(SimpleSelectorKind::UnknownText, "to")],
                }],
            },
            declaration_block: DeclarationBlock {
                declarations: vec![declaration(
                    CssPropertyId::Transform,
                    None,
                    "rotate(360deg)",
                )],
            },
            children: vec![],
        }],
    };
    let font_face = Rule {
        kind: RuleKind::FontFace,
        prelude: RulePrelude { selectors: vec![] },
        declaration_block: DeclarationBlock {
            declarations: vec![
                declaration(CssPropertyId::FontFamily, None, "MyFont"),
                declaration(CssPropertyId::Unknown, Some("src"), "url(\"myfont.woff2\")"),
            ],
        },
        children: vec![],
    };
    let info = style_info(vec![(
        0,
        StyleSheet {
            imports: vec![],
            rules: vec![keyframes, font_face],
        },
    )]);

    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.load_style_info(&mut tree, &info);
    assert_eq!(engine.font_face_count(&tree), 1);
    let page = tree.create_page();
    engine.flush_styles(&mut tree);
    assert!(
        engine.has_keyframes_animation(&tree, "spin", tree.widget(&page).unwrap()),
        "@keyframes spin must be registered with the stylist"
    );
}

#[test]
fn fixture_bundle_styles_end_to_end() {
    const BUNDLE: &[u8] = include_bytes!(
        "../../lynx-template-decoder/tests/fixtures/basic-class-selector.web.bundle"
    );
    let template = lynx_template_decoder::decode(BUNDLE).unwrap();
    let info = template.style_info.expect("fixture carries StyleInfo");

    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.load_style_info(&mut tree, &info);
    let page = tree.create_page();
    let card = tree.create_view();
    tree.append_child(&page, &card).unwrap();
    tree.set_classes(&card, "basic").unwrap();
    engine.flush_styles(&mut tree);

    let style = tree.computed_style(&card).unwrap().unwrap();
    assert_eq!(width_px(style.clone_width()), 100.0);
    assert_eq!(width_px(style.clone_height()), 100.0);
}

#[test]
fn parallel_flush_matches_sequential() {
    fn build(parallelism: Parallelism) -> Vec<(f32, AbsoluteColor)> {
        let engine = StyleEngine::new(metrics());
        let mut tree = engine.new_tree();
        engine.add_stylesheet(
            &mut tree,
            ".outer { color: red; } .even { width: 10px; } .odd { width: 20px; color: blue; }",
            lynx_widget::StylesheetOrigin::Author,
        );
        let page = tree.create_page();
        let mut leaves = Vec::new();
        for section in 0..16 {
            let container = tree.create_view();
            tree.append_child(&page, &container).unwrap();
            tree.set_classes(&container, "outer").unwrap();
            for item in 0..16 {
                let leaf = tree.create_view();
                tree.append_child(&container, &leaf).unwrap();
                tree.set_classes(
                    &leaf,
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
        let mut tree = tree;
        engine.flush_styles_with_parallelism(&mut tree, parallelism);
        leaves
            .into_iter()
            .map(|leaf| {
                let style = tree.computed_style(&leaf).unwrap().unwrap();
                (width_px(style.clone_width()), style.clone_color())
            })
            .collect()
    }

    let sequential = build(Parallelism::Sequential);
    let parallel = build(Parallelism::Auto);
    assert_eq!(sequential, parallel);
}

#[test]
fn edge_child_restyle_on_append_and_prepend() {
    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.add_stylesheet(
        &mut tree,
        ".item:last-child { color: red; } .item:first-child { width: 42px; }",
        lynx_widget::StylesheetOrigin::Author,
    );

    let page = tree.create_page();
    let a = tree.create_view();
    let b = tree.create_view();
    for id in [&a, &b] {
        tree.append_child(&page, id).unwrap();
        tree.set_classes(id, "item").unwrap();
    }
    engine.flush_styles(&mut tree);
    assert_eq!(color_of(&tree, &b), red(), "b is :last-child");
    assert_eq!(
        width_px(tree.computed_style(&a).unwrap().unwrap().clone_width()),
        42.0,
        "a is :first-child"
    );

    let c = tree.create_view();
    tree.append_child(&page, &c).unwrap();
    tree.set_classes(&c, "item").unwrap();
    engine.flush_styles(&mut tree);
    assert_ne!(
        color_of(&tree, &b),
        red(),
        "displaced old :last-child must be restyled on append"
    );
    assert_eq!(
        color_of(&tree, &c),
        red(),
        "new last child gains the styling"
    );

    let front = tree.create_view();
    tree.insert_before(&page, &front, Some(&a)).unwrap();
    tree.set_classes(&front, "item").unwrap();
    engine.flush_styles(&mut tree);
    assert!(
        matches!(
            tree.computed_style(&a).unwrap().unwrap().clone_width(),
            Size::Auto
        ),
        "displaced old :first-child must be restyled back to width:auto on prepend"
    );
    assert_eq!(
        width_px(tree.computed_style(&front).unwrap().unwrap().clone_width()),
        42.0,
        "new first child gains the styling"
    );
}

#[test]
fn viewport_change_restyles_flushed_tree() {
    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.add_stylesheet(
        &mut tree,
        ".box { width: 100rpx; }",
        lynx_widget::StylesheetOrigin::Author,
    );

    let page = tree.create_page();
    let view = tree.create_view();
    tree.append_child(&page, &view).unwrap();
    tree.set_classes(&view, "box").unwrap();
    engine.flush_styles(&mut tree);
    assert_eq!(
        width_px(tree.computed_style(&view).unwrap().unwrap().clone_width()),
        100.0
    );

    engine.set_viewport(&mut tree, 1500.0, 1334.0);
    engine.flush_styles(&mut tree);
    assert_eq!(
        width_px(tree.computed_style(&view).unwrap().unwrap().clone_width()),
        200.0,
        "rpx lengths must re-resolve against the new viewport"
    );
}

#[test]
fn self_importing_fragment_is_dropped() {
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

    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.load_style_info(&mut tree, &info);
    let page = tree.create_page();
    let cyclic = tree.create_view();
    let leaf = tree.create_view();
    tree.append_child(&page, &cyclic).unwrap();
    tree.append_child(&page, &leaf).unwrap();
    tree.set_classes(&cyclic, "cyclic").unwrap();
    tree.set_classes(&leaf, "leaf").unwrap();
    tree.set_css_id(&[&cyclic], 5).unwrap();
    tree.set_css_id(&[&leaf], 6).unwrap();
    engine.flush_styles(&mut tree);

    assert_ne!(
        color_of(&tree, &cyclic),
        red(),
        "self-importing fragment's rules must be dropped"
    );
    assert_ne!(
        color_of(&tree, &leaf),
        red(),
        "fragment 6 is only reachable through the cycle and drops with it"
    );
}
