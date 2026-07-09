//! Integration tests for the minimal [`StyleEngine`] wiring (M2).
//!
//! These build a tiny tree through the `lynx-widget` PAPI, add an author
//! stylesheet, and assert the resolved [`ComputedValues`] — colours, `rpx`
//! lengths, inline-vs-class precedence, `display: linear`, the fork's
//! `linear-weight` longhand, and inheritance.
//!
//! The computed `rpx`/`linear-weight` values are exact integers by
//! construction, so exact float equality is intentional here.
#![allow(clippy::float_cmp)]

use app_units::Au;
use lynx_style::{EngineMetrics, StyleEngine};
use stylo::color::AbsoluteColor;
use stylo::stylesheets::Origin;
use stylo::values::computed::Size;
use stylo::values::specified::box_::DisplayInside;

/// A 750×1334 CSS-px view (so `1rpx = 1px` by default) at DPR 2.
fn metrics() -> EngineMetrics {
    EngineMetrics::new(750.0, 1334.0, 2.0)
}

/// The Lynx unit bases (`rpx`/`ppx`/`sp`) are process-global in the stylo
/// fork, and every [`StyleEngine::new`] writes them — so tests that resolve
/// styles must not run concurrently. Each test takes this lock first.
fn metrics_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Resolve the computed width of a fixed-length `width` value, in CSS px.
fn width_px(size: Size) -> f32 {
    match size {
        Size::LengthPercentage(lp) => lp.0.to_pixel_length(Au::new(0)).px(),
        other => panic!("expected a length width, got {other:?}"),
    }
}

#[test]
fn class_rule_sets_color() {
    let _metrics = metrics_lock();
    let mut engine = StyleEngine::new(metrics());
    engine.add_stylesheet_str(".c { color: red; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(view, page).unwrap();
    doc.set_classes(view, "c").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(view).unwrap(), None);
    assert_eq!(
        computed.clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
    );
}

#[test]
fn rpx_resolves_against_screen_width() {
    let _metrics = metrics_lock();
    // screen_width 750 → 1rpx = 1px → 100rpx = 100px.
    let mut engine = StyleEngine::new(EngineMetrics {
        screen_width: 750.0,
        ..metrics()
    });
    engine.add_stylesheet_str(".box { width: 100rpx; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(view, page).unwrap();
    doc.set_classes(view, "box").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(view).unwrap(), None);
    assert_eq!(width_px(computed.clone_width()), 100.0);
}

#[test]
fn rpx_follows_screen_width_change() {
    let _metrics = metrics_lock();
    // Same CSS, wider screen: screen_width 1500 → 1rpx = 2px → 100rpx = 200px.
    let mut engine = StyleEngine::new(EngineMetrics {
        screen_width: 1500.0,
        ..metrics()
    });
    engine.add_stylesheet_str(".box { width: 100rpx; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(view, page).unwrap();
    doc.set_classes(view, "box").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(view).unwrap(), None);
    assert_eq!(width_px(computed.clone_width()), 200.0);

    // Narrowing the screen live (no re-ingestion) makes rpx follow.
    engine.set_screen_metrics(2.0, 750.0, 1.0);
    let computed = engine.resolve_widget(doc.widget_ref(view).unwrap(), None);
    assert_eq!(width_px(computed.clone_width()), 100.0);
}

#[test]
fn inline_style_beats_class_rule() {
    let _metrics = metrics_lock();
    let mut engine = StyleEngine::new(metrics());
    engine.add_stylesheet_str(".c { color: red; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(view, page).unwrap();
    doc.set_classes(view, "c").unwrap();
    doc.set_inline_styles(view, "color: blue").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(view).unwrap(), None);
    assert_eq!(
        computed.clone_color(),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "inline normal declaration must beat the matched class rule"
    );
}

#[test]
fn display_linear_computes_to_lynx_linear() {
    let _metrics = metrics_lock();
    let mut engine = StyleEngine::new(metrics());
    engine.add_stylesheet_str(".row { display: linear; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(view, page).unwrap();
    doc.set_classes(view, "row").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(view).unwrap(), None);
    assert_eq!(computed.clone_display().inside(), DisplayInside::LynxLinear);
}

#[test]
fn linear_weight_longhand_computes() {
    let _metrics = metrics_lock();
    let mut engine = StyleEngine::new(metrics());
    engine.add_stylesheet_str(".item { linear-weight: 2; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(view, page).unwrap();
    doc.set_classes(view, "item").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(view).unwrap(), None);
    assert_eq!(computed.clone_linear_weight().0, 2.0);
}

#[test]
fn color_inherits_into_child() {
    let _metrics = metrics_lock();
    let mut engine = StyleEngine::new(metrics());
    // `color` is inherited; the child has no own `color`.
    engine.add_stylesheet_str(".parent { color: green; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let parent = doc.create_view();
    doc.append_element(parent, page).unwrap();
    doc.set_classes(parent, "parent").unwrap();
    let child = doc.create_text();
    doc.append_element(child, parent).unwrap();

    let parent_style = engine.resolve_widget(doc.widget_ref(parent).unwrap(), None);
    let green = AbsoluteColor::srgb_legacy(0, 128, 0, 1.0);
    assert_eq!(parent_style.clone_color(), green);

    let child_style = engine.resolve_widget(doc.widget_ref(child).unwrap(), Some(&parent_style));
    assert_eq!(
        child_style.clone_color(),
        green,
        "child with no own color inherits the parent's"
    );
}

#[test]
fn writeback_stores_computed_and_clears_dirty() {
    let _metrics = metrics_lock();
    let mut engine = StyleEngine::new(metrics());
    engine.add_stylesheet_str(".c { color: red; }", Origin::Author);

    let mut doc = engine.new_widget_tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(view, page).unwrap();
    doc.set_classes(view, "c").unwrap();

    let computed = engine.resolve_widget(doc.widget_ref(view).unwrap(), None);
    doc.set_computed(view, computed).unwrap();

    assert!(doc.computed(view).is_some());
    assert!(!doc.widget(view).unwrap().style_dirty);
}

/// A standalone helper module documenting the (verified) `-webkit-text-stroke`
/// bucket answer — see the crate-level report. This test just asserts what the
/// servo build of the vendored stylo actually exposes.
#[test]
fn webkit_text_stroke_is_absent_in_servo_build() {
    use lynx_style::property_is_supported;
    // `-webkit-text-stroke*` is gated `engine = "gecko"` in the vendored stylo,
    // so it is NOT compiled into the servo build lynx-vello uses. This documents
    // that `text-stroke*` must be a fork-added (bucket B) property later.
    assert!(!property_is_supported("-webkit-text-stroke-width"));
    assert!(!property_is_supported("-webkit-text-stroke-color"));
    // A plain servo-supported property, as a positive control.
    assert!(property_is_supported("color"));
}
