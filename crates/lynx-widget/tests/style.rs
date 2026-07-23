//! Integration coverage for the Lynx adapter over `w3c-dom`'s cascade.
#![allow(clippy::float_cmp)]

use app_units::Au;
use lynx_widget::{StyleEngine, ViewMetrics};
use stylo::color::AbsoluteColor;
use stylo::stylesheets::Origin;
use stylo::values::computed::Size;
use stylo::values::specified::box_::DisplayInside;

fn metrics() -> ViewMetrics {
    ViewMetrics::new(750.0, 1334.0, 2.0)
}

fn width_px(size: Size) -> f32 {
    match size {
        Size::LengthPercentage(lp) => lp.0.to_pixel_length(Au::new(0)).px(),
        other => panic!("expected a length width, got {other:?}"),
    }
}

#[test]
fn class_rule_sets_color() {
    let engine = StyleEngine::new(metrics());
    let mut doc = engine.new_tree();
    engine.add_stylesheet(&mut doc, ".c { color: red; }", Origin::Author);
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_child(&page, &view).unwrap();
    doc.set_classes(&view, "c").unwrap();

    engine.flush_styles(&mut doc);
    let computed = doc.computed_style(&view).unwrap().unwrap();
    assert_eq!(
        computed.clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
    );
}

#[test]
fn widget_trees_created_by_one_adapter_do_not_share_stylesheets() {
    let engine = StyleEngine::new(metrics());
    let mut first = engine.new_tree();
    let mut second = engine.new_tree();
    engine.add_stylesheet(&mut first, ".probe { color: red; }", Origin::Author);

    let first_page = first.create_page();
    let first_probe = first.create_view();
    first.append_child(&first_page, &first_probe).unwrap();
    first.set_classes(&first_probe, "probe").unwrap();
    let second_page = second.create_page();
    let second_probe = second.create_view();
    second.append_child(&second_page, &second_probe).unwrap();
    second.set_classes(&second_probe, "probe").unwrap();

    engine.flush_styles(&mut first);
    engine.flush_styles(&mut second);
    let first_color = first
        .computed_style(&first_probe)
        .unwrap()
        .unwrap()
        .clone_color();
    let second_color = second
        .computed_style(&second_probe)
        .unwrap()
        .unwrap()
        .clone_color();
    assert_eq!(first_color, AbsoluteColor::srgb_legacy(255, 0, 0, 1.0));
    assert_ne!(second_color, first_color);
}

#[test]
fn rpx_resolves_against_viewport_width() {
    let engine = StyleEngine::new(metrics());
    let mut doc = engine.new_tree();
    engine.add_stylesheet(&mut doc, ".box { width: 100rpx; }", Origin::Author);
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_child(&page, &view).unwrap();
    doc.set_classes(&view, "box").unwrap();

    engine.flush_styles(&mut doc);
    let computed = doc.computed_style(&view).unwrap().unwrap();
    assert_eq!(width_px(computed.clone_width()), 100.0);
}

#[test]
fn rpx_follows_viewport_change() {
    let engine = StyleEngine::new(ViewMetrics {
        viewport_width: 1500.0,
        ..metrics()
    });
    let mut doc = engine.new_tree();
    engine.add_stylesheet(&mut doc, ".box { width: 100rpx; }", Origin::Author);
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_child(&page, &view).unwrap();
    doc.set_classes(&view, "box").unwrap();

    engine.flush_styles(&mut doc);
    let computed = doc.computed_style(&view).unwrap().unwrap();
    assert_eq!(width_px(computed.clone_width()), 200.0);

    engine.set_viewport(&mut doc, 750.0, 1334.0);
    engine.flush_styles(&mut doc);
    let computed = doc.computed_style(&view).unwrap().unwrap();
    assert_eq!(width_px(computed.clone_width()), 100.0);
}

#[test]
fn inline_style_beats_class_rule() {
    let engine = StyleEngine::new(metrics());
    let mut doc = engine.new_tree();
    engine.add_stylesheet(&mut doc, ".c { color: red; }", Origin::Author);
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_child(&page, &view).unwrap();
    doc.set_classes(&view, "c").unwrap();
    doc.set_inline_styles(&view, "color: blue").unwrap();

    engine.flush_styles(&mut doc);
    let computed = doc.computed_style(&view).unwrap().unwrap();
    assert_eq!(
        computed.clone_color(),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "inline normal declaration must beat the matched class rule"
    );
}

#[test]
fn display_linear_computes_to_lynx_linear() {
    let engine = StyleEngine::new(metrics());
    let mut doc = engine.new_tree();
    engine.add_stylesheet(&mut doc, ".row { display: linear; }", Origin::Author);
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_child(&page, &view).unwrap();
    doc.set_classes(&view, "row").unwrap();

    engine.flush_styles(&mut doc);
    let computed = doc.computed_style(&view).unwrap().unwrap();
    assert_eq!(computed.clone_display().inside(), DisplayInside::LynxLinear);
}

#[test]
fn linear_weight_longhand_computes() {
    let engine = StyleEngine::new(metrics());
    let mut doc = engine.new_tree();
    engine.add_stylesheet(&mut doc, ".item { linear-weight: 2; }", Origin::Author);
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_child(&page, &view).unwrap();
    doc.set_classes(&view, "item").unwrap();

    engine.flush_styles(&mut doc);
    let computed = doc.computed_style(&view).unwrap().unwrap();
    assert_eq!(computed.clone_linear_weight().0, 2.0);
}

#[test]
fn color_inherits_into_child() {
    let engine = StyleEngine::new(metrics());
    let mut doc = engine.new_tree();
    engine.add_stylesheet(&mut doc, ".parent { color: green; }", Origin::Author);
    let page = doc.create_page();
    let parent = doc.create_view();
    doc.append_child(&page, &parent).unwrap();
    doc.set_classes(&parent, "parent").unwrap();
    let child = doc.create_text();
    doc.append_child(&parent, &child).unwrap();

    engine.flush_styles(&mut doc);
    let parent_style = doc.computed_style(&parent).unwrap().unwrap();
    let green = AbsoluteColor::srgb_legacy(0, 128, 0, 1.0);
    assert_eq!(parent_style.clone_color(), green);

    let child_style = doc.computed_style(&child).unwrap().unwrap();
    assert_eq!(child_style.clone_color(), green);
}

#[test]
fn computed_style_is_written_only_by_the_style_flush() {
    let engine = StyleEngine::new(metrics());
    let mut doc = engine.new_tree();
    engine.add_stylesheet(&mut doc, ".c { color: red; }", Origin::Author);
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_child(&page, &view).unwrap();
    doc.set_classes(&view, "c").unwrap();

    assert!(doc.computed_style(&view).unwrap().is_none());
    engine.flush_styles(&mut doc);
    assert!(doc.computed_style(&view).unwrap().is_some());
}

#[test]
fn text_stroke_is_supported() {
    use lynx_widget::property_is_supported;

    assert!(property_is_supported("text-stroke"));
    assert!(property_is_supported("text-stroke-color"));
    assert!(property_is_supported("text-stroke-width"));
    assert!(!property_is_supported("-webkit-text-stroke-width"));
    assert!(!property_is_supported("-webkit-text-stroke-color"));
    assert!(property_is_supported("color"));
}
