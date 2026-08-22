//! The `@keyframes` driver: Stylo owns the animation, the document owns the
//! timeline, and the caller owns the clock.
#![allow(clippy::float_cmp, reason = "Ahem gives exact integral layout numbers")]

mod common;

use common::Doc;

/// A document with one animated `view`, laid out once so the animation has
/// started but not yet been ticked.
fn animated(css: &str, spec: &str) -> (Doc, dom::NodeId) {
    let mut doc = Doc::with_css(css);
    let root = doc.root;
    let node = doc.el(root, spec);
    doc.flush();
    (doc, node)
}

/// The element's laid-out border box, as the layout pass left it.
fn box_of(
    doc: &Doc,
    id: dom::NodeId,
) -> (hughie::geometry::Point<f32>, hughie::geometry::Size<f32>) {
    let layout = doc.dom.rounded_layout(id).expect("the element is laid out");
    (layout.location, layout.size)
}

const SLIDE: &str = "
    @keyframes slide {
        from { transform: translateX(0px); }
        to { transform: translateX(100px); }
    }
    .mover { animation: slide 10s linear; width: 20px; height: 20px; }
";

/// An animated shaping property has to reach the text, and the only path from
/// the animation harvest to a text node's cached measurement is the text-child
/// invalidation the style harvest also does. Without it the element restyles,
/// re-lays out, and the text answers from the box cache it filled at the first
/// font size.
#[test]
fn an_animated_font_size_remeasures_the_text_it_scales() {
    const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");

    let mut doc = Doc::with_css(
        "@keyframes grow { from { font-size: 16px; } to { font-size: 32px; } }
         page { display: flex; width: 400px; height: 100px; align-items: flex-start;
                font-family: Ahem; }
         .label { display: flex; animation: grow 10s linear; }",
    );
    assert_eq!(
        doc.dom.register_fonts(dom::FontBlob::from_static(AHEM)),
        1,
        "Ahem gives every glyph an exact one-em advance"
    );
    let root = doc.root;
    let label = doc.el(root, "text.label");
    let run = doc.dom.create_text_node("hello", ());
    doc.dom.append_child(label, run);

    doc.dom.advance_animations(0.0);
    doc.flush();
    let start = box_of(&doc, run).1;
    assert_eq!(start.width, 80.0, "five Ahem glyphs at 16px");

    let tick = doc.dom.advance_animations(5.0);
    assert!(tick.relayout, "an animated font-size is not a repaint");
    doc.flush();

    assert_eq!(
        box_of(&doc, run).1.width,
        120.0,
        "the text re-measures at the animated 24px"
    );
}

#[test]
fn a_keyframes_animation_starts_without_being_ticked() {
    let (doc, _) = animated(SLIDE, "view.mover");
    assert!(
        doc.dom.has_active_animations(),
        "the style flush that first sees `animation-name` starts the animation"
    );
}

#[test]
fn a_transform_animation_advances_between_samples() {
    let (mut doc, mover) = animated(SLIDE, "view.mover");

    doc.dom.advance_animations(0.0);
    let start = doc.value(mover, "transform");
    doc.dom.advance_animations(5.0);
    let middle = doc.value(mover, "transform");
    doc.dom.advance_animations(10.0);
    let end = doc.value(mover, "transform");

    assert_ne!(start, middle, "the animation moved between samples");
    assert_ne!(middle, end, "and kept moving");
    assert_eq!(start, "translateX(0px)", "t=0 is the `from` keyframe");
    assert_eq!(
        middle, "translateX(50px)",
        "a linear animation is halfway at half its duration"
    );
    assert_eq!(
        end, "none",
        "past the end, `animation-fill-mode: none` gives the base style back"
    );
}

#[test]
fn a_transform_animation_never_relayouts() {
    let (mut doc, mover) = animated(SLIDE, "view.mover");
    doc.dom.advance_animations(0.0);
    doc.flush();
    let before = box_of(&doc, mover);

    for sample in [2.5, 5.0, 7.5] {
        let tick = doc.dom.advance_animations(sample);
        assert!(
            !tick.relayout,
            "a `transform` animation carries no relayout damage (t={sample})"
        );
        assert!(tick.restyled > 0, "but it does re-cascade (t={sample})");
    }

    doc.flush();
    let after = box_of(&doc, mover);
    assert_eq!(before, after, "and it never moves the box");
}

#[test]
fn a_width_animation_does_relayout() {
    let (mut doc, _) = animated(
        "
        @keyframes grow { from { width: 20px; } to { width: 120px; } }
        .mover { animation: grow 10s linear; height: 20px; }
        ",
        "view.mover",
    );
    doc.dom.advance_animations(0.0);
    let tick = doc.dom.advance_animations(5.0);
    assert!(
        tick.relayout,
        "an animation of a geometry property has to reach layout"
    );
}

#[test]
fn a_finished_animation_stops_asking_for_frames() {
    let (mut doc, _) = animated(SLIDE, "view.mover");
    doc.dom.advance_animations(0.0);
    assert!(doc.dom.advance_animations(5.0).needs_next_frame);

    let tick = doc.dom.advance_animations(10.5);
    assert!(
        !tick.needs_next_frame,
        "past its only iteration the animation is finished"
    );
    assert!(
        !doc.dom.has_active_animations(),
        "and the document parks until something else animates"
    );
}

#[test]
fn an_element_with_no_animation_costs_one_bool() {
    let mut doc = Doc::with_css(".plain { width: 10px; }");
    let root = doc.root;
    doc.el(root, "view.plain");
    doc.flush();

    assert!(!doc.dom.has_active_animations());
    assert_eq!(
        doc.dom.advance_animations(1.0),
        dom::AnimationTick::default()
    );
}

#[test]
fn an_infinite_animation_keeps_iterating() {
    let (mut doc, spinner) = animated(
        "
        @keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
        .spinner { animation: spin 1s linear infinite; width: 20px; height: 20px; }
        ",
        "view.spinner",
    );
    doc.dom.advance_animations(0.0);
    let first = doc.value(spinner, "transform");
    // Straddles three whole iterations: the driver has to replay each one.
    doc.dom.advance_animations(3.5);
    let later = doc.value(spinner, "transform");

    assert_ne!(first, later, "an infinite animation is still moving");
    assert!(
        doc.dom.advance_animations(9.0).needs_next_frame,
        "and never stops asking for frames"
    );
}

#[test]
fn animation_time_never_runs_backwards() {
    let (mut doc, mover) = animated(SLIDE, "view.mover");
    doc.dom.advance_animations(5.0);
    let forward = doc.value(mover, "transform");
    doc.dom.advance_animations(1.0);
    assert_eq!(
        doc.value(mover, "transform"),
        forward,
        "a sample behind the timeline is clamped, not rewound"
    );
}

#[test]
fn a_removed_element_leaves_no_animation_behind() {
    let (mut doc, mover) = animated(SLIDE, "view.mover");
    doc.dom.advance_animations(1.0);
    assert!(doc.dom.has_active_animations());

    doc.dom.remove_element(mover);
    doc.flush();
    assert!(
        !doc.dom.has_active_animations(),
        "a freed arena slot must not hand its animations to whatever reuses it"
    );
}

#[test]
fn an_animated_element_still_answers_normal_restyles() {
    let (mut doc, mover) = animated(
        "
        @keyframes fade { from { opacity: 1; } to { opacity: 0; } }
        .mover { animation: fade 10s linear; width: 20px; height: 20px; }
        .wide { width: 90px; }
        ",
        "view.mover",
    );
    doc.dom.advance_animations(5.0);
    doc.add_class(mover, "wide");
    doc.flush();

    assert_eq!(
        doc.value(mover, "width"),
        "90px",
        "an animation tick must not consume the dirty bits a real restyle needs"
    );
    assert!(
        doc.dom.has_active_animations(),
        "and the restyle must not lose the running animation"
    );
}

#[test]
fn fill_mode_forwards_holds_its_value_across_a_later_restyle() {
    let (mut doc, mover) = animated(
        "
        @keyframes fade { from { opacity: 1; } to { opacity: 0.25; } }
        .mover { animation: fade 10s linear forwards; width: 20px; height: 20px; }
        .wide { width: 90px; }
        ",
        "view.mover",
    );
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(11.0);
    assert_eq!(
        doc.value(mover, "opacity"),
        "0.25",
        "`forwards` holds the last keyframe after the animation ends"
    );

    doc.add_class(mover, "wide");
    doc.flush();
    assert_eq!(
        doc.value(mover, "opacity"),
        "0.25",
        "and an unrelated restyle must not drop the held value"
    );
}

#[test]
fn replacing_the_animation_releases_a_held_fill_value() {
    let (mut doc, mover) = animated(
        "
        @keyframes fade { from { opacity: 1; } to { opacity: 0.25; } }
        .mover { animation: fade 10s linear forwards; width: 20px; height: 20px; }
        .still { animation: none; }
        ",
        "view.mover",
    );
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(11.0);
    assert_eq!(doc.value(mover, "opacity"), "0.25");

    doc.add_class(mover, "still");
    doc.flush();
    assert_eq!(
        doc.value(mover, "opacity"),
        "1",
        "an animation the new style no longer names is cancelled, and a \
         cancelled animation stops filling"
    );
    assert!(
        !doc.dom.has_active_animations(),
        "and nothing is left to tick"
    );
}

#[test]
fn a_held_fill_value_costs_no_frames() {
    let (mut doc, _) = animated(
        "
        @keyframes fade { from { opacity: 1; } to { opacity: 0.25; } }
        .mover { animation: fade 10s linear forwards; width: 20px; height: 20px; }
        ",
        "view.mover",
    );
    doc.dom.advance_animations(0.0);
    let tick = doc.dom.advance_animations(11.0);
    assert!(
        !tick.needs_next_frame,
        "holding a value is not the same as still animating"
    );
    assert_eq!(
        doc.dom.advance_animations(12.0),
        dom::AnimationTick::default(),
        "and later frames do no work at all"
    );
}

/// Cancelling a *running* animation has the same hazard as cancelling a
/// filling one: Stylo cancels without marking the set dirty, so nothing
/// replaces the element's `Animations` cascade origin on its own.
#[test]
fn cancelling_a_running_animation_restores_the_un_animated_style() {
    let (mut doc, mover) = animated(
        "
        @keyframes fade { from { opacity: 1; } to { opacity: 0.2; } }
        .mover { animation: fade 10s linear; width: 20px; height: 20px; }
        .still { animation: none; }
        ",
        "view.mover",
    );
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(5.0);
    assert_eq!(doc.value(mover, "opacity"), "0.6", "halfway through");

    doc.add_class(mover, "still");
    doc.flush();
    assert_eq!(
        doc.value(mover, "opacity"),
        "1",
        "a cancelled animation stops contributing to the cascade at once, not \
         at whatever restyle happens next"
    );
    assert!(!doc.dom.has_active_animations());
}

/// A resize now re-cascades rather than re-matches, and the tick that runs
/// between the resize and the next flush must leave both alone: the viewport
/// change still lands, and the animation still advances.
///
/// The hint shape that makes this survivable is pinned separately, in
/// `style::invalidation`'s own tests — this one is the end-to-end reading.
#[test]
fn a_viewport_change_survives_an_animation_tick_before_the_next_flush() {
    let (mut doc, mover) = animated(
        "
        @keyframes slide {
            from { transform: translateX(0px); }
            to { transform: translateX(100px); }
        }
        page { font-size: 5vw; }
        .mover { animation: slide 10s linear; width: 20px; height: 20px; }
        ",
        "view.mover",
    );
    let root = doc.root;
    doc.flush();
    assert_eq!(
        doc.value(root, "font-size"),
        "40px",
        "5vw of the initial 800px viewport"
    );

    doc.dom.advance_animations(0.0);
    doc.dom.set_viewport(400.0, 600.0);
    // The tick lands between the resize and the flush that would consume it.
    doc.dom.advance_animations(5.0);
    doc.flush();

    assert_eq!(
        doc.value(root, "font-size"),
        "20px",
        "the document element's own 5vw must follow the new viewport even \
         though an animation ticked between the resize and the flush"
    );
    assert_eq!(
        doc.value(mover, "transform"),
        "translateX(50px)",
        "and the animation itself still advanced"
    );
}
