//! Integration coverage for the Lynx adapter over `w3c-dom`'s cascade.
#![allow(clippy::float_cmp)]

use app_units::Au;
use lynx_widget::{EngineMetrics, StyleEngine};
use stylo::color::AbsoluteColor;
use stylo::stylesheets::Origin;
use stylo::values::computed::Size;
use stylo::values::specified::box_::DisplayInside;

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

#[test]
fn class_rule_sets_color() {
    let mut engine = StyleEngine::new(metrics());
    engine.add_stylesheet_str(".c { color: red; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(&view, &page).unwrap();
    doc.set_classes(&view, "c").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(&view).unwrap(), None);
    assert_eq!(
        computed.clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
    );
}

#[test]
fn rpx_resolves_against_viewport_width() {
    let mut engine = StyleEngine::new(metrics());
    engine.add_stylesheet_str(".box { width: 100rpx; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(&view, &page).unwrap();
    doc.set_classes(&view, "box").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(&view).unwrap(), None);
    assert_eq!(width_px(computed.clone_width()), 100.0);
}

#[test]
fn rpx_follows_viewport_change() {
    let mut engine = StyleEngine::new(EngineMetrics {
        viewport_width: 1500.0,
        ..metrics()
    });
    engine.add_stylesheet_str(".box { width: 100rpx; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(&view, &page).unwrap();
    doc.set_classes(&view, "box").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(&view).unwrap(), None);
    assert_eq!(width_px(computed.clone_width()), 200.0);

    engine.set_viewport(750.0, 1334.0);
    let computed = engine.resolve_widget(doc.widget_ref(&view).unwrap(), None);
    assert_eq!(width_px(computed.clone_width()), 100.0);
}

#[test]
fn inline_style_beats_class_rule() {
    let mut engine = StyleEngine::new(metrics());
    engine.add_stylesheet_str(".c { color: red; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(&view, &page).unwrap();
    doc.set_classes(&view, "c").unwrap();
    doc.set_inline_styles(&view, "color: blue").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(&view).unwrap(), None);
    assert_eq!(
        computed.clone_color(),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "inline normal declaration must beat the matched class rule"
    );
}

#[test]
fn display_linear_computes_to_lynx_linear() {
    let mut engine = StyleEngine::new(metrics());
    engine.add_stylesheet_str(".row { display: linear; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(&view, &page).unwrap();
    doc.set_classes(&view, "row").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(&view).unwrap(), None);
    assert_eq!(computed.clone_display().inside(), DisplayInside::LynxLinear);
}

#[test]
fn linear_weight_longhand_computes() {
    let mut engine = StyleEngine::new(metrics());
    engine.add_stylesheet_str(".item { linear-weight: 2; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(&view, &page).unwrap();
    doc.set_classes(&view, "item").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(&view).unwrap(), None);
    assert_eq!(computed.clone_linear_weight().0, 2.0);
}

#[test]
fn color_inherits_into_child() {
    let mut engine = StyleEngine::new(metrics());
    engine.add_stylesheet_str(".parent { color: green; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let parent = doc.create_view();
    doc.append_element(&parent, &page).unwrap();
    doc.set_classes(&parent, "parent").unwrap();
    let child = doc.create_text();
    doc.append_element(&child, &parent).unwrap();

    let parent_style = engine.resolve_widget(doc.widget_ref(&parent).unwrap(), None);
    let green = AbsoluteColor::srgb_legacy(0, 128, 0, 1.0);
    assert_eq!(parent_style.clone_color(), green);

    let child_style = engine.resolve_widget(doc.widget_ref(&child).unwrap(), Some(&parent_style));
    assert_eq!(child_style.clone_color(), green);
}

#[test]
fn writeback_stores_computed_and_clears_dirty() {
    let mut engine = StyleEngine::new(metrics());
    engine.add_stylesheet_str(".c { color: red; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(&view, &page).unwrap();
    doc.set_classes(&view, "c").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(&view).unwrap(), None);
    doc.set_computed(&view, computed).unwrap();

    assert!(doc.computed(&view).unwrap().is_some());
    assert!(!doc.widget(&view).unwrap().is_style_dirty());
}

#[test]
fn text_stroke_is_supported() {
    use lynx_widget::property_is_supported;

    assert!(property_is_supported("text-stroke"));
    assert!(property_is_supported("text-stroke-color"));
    assert!(property_is_supported("text-stroke-width"));
    // The `-webkit-` spellings are the hidden canonical properties; only the
    // Lynx `text-stroke*` aliases are author-facing in the fork's grammar.
    assert!(!property_is_supported("-webkit-text-stroke-width"));
    assert!(!property_is_supported("-webkit-text-stroke-color"));
    assert!(property_is_supported("color"));
}
