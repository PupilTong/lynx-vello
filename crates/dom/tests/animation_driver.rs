//! The `@keyframes` driver: Stylo owns the animation, the document owns the
//! timeline, and the caller owns the clock.
#![allow(clippy::float_cmp, reason = "Ahem gives exact integral layout numbers")]

mod common;

use common::{Doc, device};
use euclid::default::Vector2D;

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
         .label { display: -lynx-text; animation: grow 10s linear; }",
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

    doc.flush();
    // The flush creates the animation; the tick that follows is the frame it
    // starts on, so from here the timeline and the animation share an origin.
    doc.dom.advance_animations(0.0);
    // The paragraph is the element's, so its box is what the animation moves.
    let start = box_of(&doc, label).1;
    assert_eq!(start.width, 80.0, "five Ahem glyphs at 16px");

    let tick = doc.dom.advance_animations(5.0);
    assert!(tick.relayout, "an animated font-size is not a repaint");
    doc.flush();

    assert_eq!(
        box_of(&doc, label).1.width,
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

/// The timeline only moves when someone ticks it, and an idle page is ticked
/// by nobody: the flush that creates an animation therefore reads whatever
/// time the last tick left. The animation still has to start at the first
/// frame that sees it, not however many seconds earlier that reading is.
#[test]
fn an_animation_created_while_the_timeline_was_idle_starts_at_the_first_tick() {
    let (mut doc, mover) = animated(SLIDE, "view.mover");

    // Ten idle seconds: the flush above created the animation at 0, and this
    // is the first frame after it.
    let tick = doc.dom.advance_animations(10.0);
    assert!(tick.needs_next_frame, "a 10s animation has not run out");
    assert_eq!(
        doc.value(mover, "transform"),
        "translateX(0px)",
        "the first frame after the animation was created is its start"
    );

    doc.dom.advance_animations(15.0);
    assert_eq!(
        doc.value(mover, "transform"),
        "translateX(50px)",
        "and five seconds later it is halfway, not over"
    );
    assert!(
        doc.dom.advance_animations(19.9).needs_next_frame,
        "still inside its duration, measured from the first frame"
    );
    assert!(
        !doc.dom.advance_animations(20.1).needs_next_frame,
        "and it ends one whole duration after that frame"
    );
}

const DELAYED: &str = "
    @keyframes slide {
        from { transform: translateX(0px); }
        to { transform: translateX(100px); }
    }
    .mover { animation: slide 10s linear 5s; width: 20px; height: 20px; }
    .wide { width: 90px; }
";

/// A delayed animation is anchored by the same first frame, then left alone:
/// it waits out its delay from there, and the ticks that pass while it waits
/// must not push it any further.
#[test]
fn a_delayed_animation_is_anchored_once_and_waits_from_there() {
    let (mut doc, mover) = animated(DELAYED, "view.mover");

    doc.dom.advance_animations(10.0);
    doc.dom.advance_animations(11.0);
    doc.dom.advance_animations(14.9);
    assert_eq!(
        doc.value(mover, "transform"),
        "none",
        "inside its delay the animation contributes nothing"
    );

    doc.dom.advance_animations(15.0);
    assert_eq!(
        doc.value(mover, "transform"),
        "translateX(0px)",
        "it starts one delay after the frame that anchored it"
    );
    doc.dom.advance_animations(20.0);
    assert_eq!(
        doc.value(mover, "transform"),
        "translateX(50px)",
        "and runs its whole duration from there"
    );
}

/// A negative delay is a head start, and it is a head start on the frame the
/// animation is anchored to.
#[test]
fn a_negative_delay_keeps_its_head_start_at_the_first_tick() {
    let (mut doc, mover) = animated(
        "
        @keyframes slide {
            from { transform: translateX(0px); }
            to { transform: translateX(100px); }
        }
        .mover { animation: slide 10s linear -2s; width: 20px; height: 20px; }
        ",
        "view.mover",
    );

    doc.dom.advance_animations(10.0);
    assert_eq!(
        doc.value(mover, "transform"),
        "translateX(20px)",
        "the first frame finds the animation two of its ten seconds in"
    );
    assert!(
        doc.dom.advance_animations(17.9).needs_next_frame,
        "the head start comes off its end, not its start"
    );
    assert!(!doc.dom.advance_animations(18.1).needs_next_frame);
}

/// The anchor is the driver's own record, keyed by the element and the
/// animation name, because Stylo's `is_new` is not one: a restyle assigns the
/// animation over from the new style and sets that flag again, so a delayed
/// animation on an element that restyles every frame would never start.
#[test]
fn a_restyle_while_a_delayed_animation_waits_does_not_postpone_it() {
    let (mut doc, mover) = animated(DELAYED, "view.mover");

    doc.dom.advance_animations(10.0);
    doc.add_class(mover, "wide");
    doc.flush();
    doc.dom.advance_animations(11.0);
    doc.remove_class(mover, "wide");
    doc.flush();

    doc.dom.advance_animations(15.0);
    assert_eq!(
        doc.value(mover, "transform"),
        "translateX(0px)",
        "the restyles kept the animation's delay running rather than restarting it"
    );
}

/// A transition is created by the flush that changes the property, and takes
/// its start from the first frame after that flush for the same reason an
/// animation does.
#[test]
fn a_transition_created_while_the_timeline_was_idle_starts_at_the_first_tick() {
    let (mut doc, fader) = animated(
        "
        .fader { width: 20px; height: 20px; opacity: 1; transition: opacity 10s linear; }
        .dim { opacity: 0; }
        ",
        "view.fader",
    );
    doc.add_class(fader, "dim");
    doc.flush();
    assert!(
        doc.dom.has_active_animations(),
        "the class change started a transition"
    );

    let tick = doc.dom.advance_animations(10.0);
    assert!(tick.needs_next_frame, "a 10s transition has not run out");
    assert_eq!(
        doc.value(fader, "opacity"),
        "1",
        "the first frame after the transition was created is its start"
    );

    doc.dom.advance_animations(15.0);
    assert_eq!(
        doc.value(fader, "opacity"),
        "0.5",
        "and five seconds later it is halfway"
    );
    assert!(
        !doc.dom.advance_animations(20.1).needs_next_frame,
        "ending one whole duration after that frame"
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

/// One stalled tick crossing three iterations of an alternating fade lands
/// on the value the fourth iteration, running reversed, holds a quarter in.
#[test]
fn a_stalled_alternating_animation_lands_on_its_current_iteration() {
    let (mut doc, fader) = animated(
        "
        @keyframes fade { from { opacity: 1; } to { opacity: 0; } }
        .fader { animation: fade 1s linear infinite alternate; width: 20px; height: 20px; }
        ",
        "view.fader",
    );
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(3.25);
    assert_eq!(doc.value(fader, "opacity"), "0.25");
    doc.dom.advance_animations(7.75);
    assert_eq!(doc.value(fader, "opacity"), "0.75");
}

#[test]
fn animation_time_never_runs_backwards() {
    let (mut doc, mover) = animated(SLIDE, "view.mover");
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(5.0);
    let forward = doc.value(mover, "transform");
    assert_eq!(forward, "translateX(50px)", "halfway through");
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

// ---------------------------------------------------------------------------
// css-contain-2 §4: an animation in a skipped subtree is frozen
// ---------------------------------------------------------------------------
//
// > While an element is skipped, CSS transitions and animations on the
// > element do not update: "New animations are not created even if
// > newly-applied style would start one. Existing animations do not advance in
// > their timeline. Running animations on the element do not end."
// >
// > When an element stops being skipped, animations and transitions are
// > sampled and then resume advancing on their timelines as normal from that
// > point.
// >
// > — https://drafts.csswg.org/css-contain-2/#content-visibility
//
// Every case below goes through `Document::render`, never a bare `layout()`:
// skipping is a fact of the rendering update, and for
// `content-visibility: auto` it is *decided* by one. The engine deviates on
// the first clause only — style is not skipped, so Stylo creates the animation
// anyway and the driver freezes it at its own start.

const SKIPPING: &str = "
    @keyframes slide {
        from { transform: translateX(0px); }
        to { transform: translateX(100px); }
    }
    page { display: flex; width: 200px; height: 200px; align-items: flex-start; }
    .box { width: 100px; height: 100px; }
    .box.skipping { content-visibility: hidden; }
    .mover { width: 20px; height: 20px; }
    .mover.animating { animation: slide 10s linear both; }
";

/// `page > .box > .mover`. `animation-fill-mode: both` is what makes the
/// frozen value readable: without it an animation that never started
/// contributes nothing, and "did not advance" would be indistinguishable from
/// "was never created".
struct Skipping {
    doc: Doc,
    boxed: dom::NodeId,
    mover: dom::NodeId,
}

impl Skipping {
    fn new(skipping: bool, animating: bool) -> Self {
        let mut doc = Doc::with_css(SKIPPING);
        let root = doc.root;
        let boxed = doc.el(
            root,
            if skipping {
                "view.box.skipping"
            } else {
                "view.box"
            },
        );
        let mover = doc.el(
            boxed,
            if animating {
                "view.mover.animating"
            } else {
                "view.mover"
            },
        );
        Self { doc, boxed, mover }
    }

    fn render(&mut self) {
        self.doc.dom.render();
    }

    fn tick(&mut self, now: f64) {
        self.doc.dom.advance_animations(now);
    }

    fn transform(&self) -> String {
        self.doc.value(self.mover, "transform")
    }

    fn animating(&self) -> bool {
        self.doc.dom.has_active_animations()
    }

    fn skip(&mut self, skipping: bool) {
        let boxed = self.boxed;
        if skipping {
            self.doc.add_class(boxed, "skipping");
        } else {
            self.doc.remove_class(boxed, "skipping");
        }
    }
}

#[test]
fn an_animation_under_content_visibility_hidden_neither_advances_nor_owes_frames() {
    let mut skipped = Skipping::new(true, true);
    skipped.render();
    assert!(
        !skipped.animating(),
        "the document's only animation is skipped, so nothing owes a frame"
    );

    skipped.tick(0.0);
    skipped.tick(5.0);
    skipped.tick(9.0);
    assert_eq!(
        skipped.transform(),
        "translateX(0px)",
        "nine seconds of timeline moved past a frozen animation"
    );
    assert!(
        !skipped.animating(),
        "and ticking it did not wake the timeline up"
    );

    // The same page with the box visible, so the assertion above reads as
    // "frozen" rather than as "never created".
    let mut shown = Skipping::new(false, true);
    shown.render();
    assert!(shown.animating(), "the control animates");
    shown.tick(0.0);
    shown.tick(5.0);
    assert_eq!(
        shown.transform(),
        "translateX(50px)",
        "half a duration in, at the same five seconds the skipped one ignored"
    );
}

#[test]
fn a_revealed_subtree_resumes_its_animation_where_it_was_frozen() {
    let mut page = Skipping::new(false, true);
    page.render();
    page.tick(0.0);
    page.tick(3.0);
    assert_eq!(
        page.transform(),
        "translateX(30px)",
        "three of its ten seconds"
    );

    page.skip(true);
    page.render();
    assert!(
        !page.animating(),
        "the box started skipping its contents in this very commit"
    );
    page.tick(8.0);
    page.tick(100.0);
    assert_eq!(
        page.transform(),
        "translateX(30px)",
        "ninety-seven seconds of timeline went by without it"
    );

    page.skip(false);
    page.render();
    assert!(
        page.animating(),
        "and the reveal starts it again in the commit that revealed it"
    );

    page.tick(101.0);
    assert_eq!(
        page.transform(),
        "translateX(30px)",
        "the first tick after a reveal is the point it resumes from, not a \
         ninety-seven-second jump"
    );
    page.tick(105.0);
    assert_eq!(
        page.transform(),
        "translateX(70px)",
        "four seconds on from the three it had when it froze"
    );
}

#[test]
fn an_animation_started_inside_a_skipped_subtree_is_frozen_at_its_start() {
    let mut page = Skipping::new(true, false);
    page.render();
    page.tick(0.0);
    page.tick(5.0);
    assert!(!page.animating(), "nothing animates yet");

    // Skipping contents does not skip style, so Stylo's `process_animations`
    // creates this animation whatever the box above it says. The engine cannot
    // honor "new animations are not created" and freezes it at its own start
    // instead — the one deviation, recorded in `style-assumptions.md` §19.
    let mover = page.mover;
    page.doc.add_class(mover, "animating");
    page.render();
    assert!(
        !page.animating(),
        "created and immediately frozen, owing no frames"
    );
    page.tick(20.0);
    assert_eq!(page.transform(), "translateX(0px)");

    page.skip(false);
    page.render();
    assert!(page.animating());
    page.tick(21.0);
    assert_eq!(
        page.transform(),
        "translateX(0px)",
        "the reveal plays it from zero, not from the fifteen seconds it spent \
         frozen"
    );
    page.tick(26.0);
    assert_eq!(
        page.transform(),
        "translateX(50px)",
        "and half a duration on from there it is halfway"
    );
}

#[test]
fn the_element_that_skips_its_contents_keeps_its_own_animation() {
    let mut doc = Doc::with_css(
        "@keyframes slide {
             from { transform: translateX(0px); }
             to { transform: translateX(100px); }
         }
         page { display: flex; width: 200px; height: 200px; align-items: flex-start; }
         .box { width: 100px; height: 100px; content-visibility: hidden;
                animation: slide 10s linear both; }",
    );
    let root = doc.root;
    let boxed = doc.el(root, "view.box");
    doc.el(boxed, "view.child");

    doc.dom.render();
    assert!(
        doc.dom.has_active_animations(),
        "the spec skips an element's *contents* — its flat tree descendants — \
         not the element"
    );
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(4.0);
    assert_eq!(
        doc.value(boxed, "transform"),
        "translateX(40px)",
        "the skipping box's own animation runs as normal"
    );
    assert!(
        doc.dom.has_active_animations(),
        "and keeps asking for frames"
    );
}

/// The `content-visibility: auto` path: the row is skipped because the
/// rendering update found it outside the region the frame's culling admits,
/// not because anyone wrote `hidden`. The margin is one scrollport past the
/// committed offset in each direction, so an offset of 300 leaves row 0
/// (y 0..20 of a 400px column) out of the admitted band.
#[test]
fn an_animation_in_a_scrolled_away_auto_row_freezes_and_resumes() {
    const ROWS: usize = 20;

    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.add_css(
        "@keyframes slide {
             from { transform: translateX(0px); }
             to { transform: translateX(100px); }
         }
         page { display: flex; width: 200px; height: 100px; align-items: flex-start; }
         .scroller { display: flex; flex-direction: column; overflow: hidden;
                     width: 200px; height: 100px; align-items: flex-start; }
         .row { display: flex; width: 200px; height: 20px; flex-shrink: 0;
                content-visibility: auto; contain-intrinsic-size: 200px 20px; }
         .mover { width: 20px; height: 20px; animation: slide 10s linear both; }",
    );
    let root = doc.root;
    let scroller = doc.el(root, "view.scroller");
    let rows: Vec<dom::NodeId> = (0..ROWS).map(|_| doc.el(scroller, "view.row")).collect();
    let mover = doc.el(rows[0], "view.mover");

    doc.dom.render();
    assert!(
        doc.dom.has_active_animations(),
        "row 0 is on screen, so the rendering update found it relevant"
    );
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(2.0);
    assert_eq!(doc.value(mover, "transform"), "translateX(20px)");

    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 300.0));
    doc.dom.render();
    assert!(
        !doc.dom.has_active_animations(),
        "the row stopped being relevant, so its contents' animation froze"
    );
    doc.dom.advance_animations(8.0);
    doc.dom.advance_animations(40.0);
    assert_eq!(
        doc.value(mover, "transform"),
        "translateX(20px)",
        "thirty-eight seconds off screen move it nowhere"
    );

    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 0.0));
    doc.dom.render();
    assert!(
        doc.dom.has_active_animations(),
        "scrolling back reveals the row in the same commit"
    );
    doc.dom.advance_animations(41.0);
    assert_eq!(doc.value(mover, "transform"), "translateX(20px)");
    doc.dom.advance_animations(44.0);
    assert_eq!(
        doc.value(mover, "transform"),
        "translateX(50px)",
        "three seconds on from the two it had when it froze"
    );
}

/// A reveal is noticed between two ticks, and the tick after it carries the
/// revealed element's start times across the interval. Until that tick its
/// animation does not export — the painter would sample start times about to
/// move — and from it on it does.
#[test]
fn a_revealed_animation_exports_once_its_start_times_are_carried() {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.add_css(
        "@keyframes slide {
             from { transform: translateX(0px); }
             to { transform: translateX(100px); }
         }
         page { display: flex; width: 200px; height: 100px; align-items: flex-start; }
         .scroller { display: flex; flex-direction: column; overflow: hidden;
                     width: 200px; height: 100px; align-items: flex-start; }
         .row { display: flex; width: 200px; height: 20px; flex-shrink: 0;
                content-visibility: auto; contain-intrinsic-size: 200px 20px; }
         .mover { width: 20px; height: 20px; animation: slide 10s linear both; }",
    );
    let root = doc.root;
    let scroller = doc.el(root, "view.scroller");
    let rows: Vec<dom::NodeId> = (0..20).map(|_| doc.el(scroller, "view.row")).collect();
    doc.el(rows[0], "view.mover");

    doc.dom.render();
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(2.0);
    assert!(doc.dom.commit().has_exported_curves(), "the slide exports");

    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 300.0));
    doc.dom.render();
    doc.dom.advance_animations(8.0);
    doc.dom.advance_animations(40.0);

    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 0.0));
    let revealed = doc.dom.commit();
    assert!(
        doc.dom.has_active_animations(),
        "the reveal resumes the slide"
    );
    assert!(
        !revealed.has_exported_curves() && revealed.needs_main_ticks(),
        "the next tick carries its start times, so the reveal's commit ticks it"
    );
    doc.dom.advance_animations(41.0);
    assert!(
        doc.dom.commit().has_exported_curves(),
        "once carried, the slide exports"
    );
}

const SCROLL_DRIVEN: &str = "
    @keyframes fade { from { opacity: 0; } to { opacity: 1; } }
    @keyframes grow { from { width: 100px; } to { width: 300px; } }
    page { display: flex; width: 800px; height: 600px; align-items: flex-start; }
    .scroller { display: flex; flex-direction: column; align-items: flex-start;
                overflow: scroll; width: 200px; height: 200px; }
    .filler { flex-shrink: 0; width: 200px; height: 1180px; }
    .mover { flex-shrink: 0; width: 20px; height: 20px; }
    .driven { animation: fade linear both; animation-timeline: scroll(); }
    .paused { animation-play-state: paused; }
    .hidden { content-visibility: hidden; }
    .wrapper { display: flex; flex-shrink: 0; width: 200px; height: 20px; }";

/// A 200px scroller over 1200px of content — a scroll timeline of 1000px —
/// holding a mover carrying `classes`, then the filler.
fn scroll_driven(classes: &str) -> (Doc, dom::NodeId, dom::NodeId) {
    let mut doc = Doc::with_css(SCROLL_DRIVEN);
    let root = doc.root;
    let scroller = doc.el(root, "view.scroller");
    let mover = doc.el(scroller, &format!("view.mover{classes}"));
    doc.el(scroller, "view.filler");
    doc.flush();
    (doc, scroller, mover)
}

/// Moves the scroller the way an adopted painter scroll does.
fn scroll(doc: &mut Doc, scroller: dom::NodeId, y: f32) {
    doc.dom.scroll_to(scroller, Vector2D::new(0.0, y));
    doc.dom.advance_scroll_timelines(&[scroller]);
}

/// scroll-animations-1: the value follows the scroll container's offset
/// with no clock, and the first layout already shows the one for the
/// offset it found — the stale-timelines pass, not the base value.
#[test]
fn a_scroll_driven_animation_follows_its_scroller() {
    let (mut doc, scroller, mover) = scroll_driven(".driven");
    assert_eq!(
        doc.value(mover, "opacity"),
        "0",
        "the first layout samples offset 0"
    );
    assert!(!doc.dom.has_active_animations(), "no clock frames");
    assert!(
        !doc.dom.commit().has_exported_curves(),
        "no curve carries a scroll timeline yet: as a clock curve it would recompose every frame"
    );
    scroll(&mut doc, scroller, 250.0);
    assert_eq!(doc.value(mover, "opacity"), "0.25", "no flush needed");
    doc.flush();
    assert_eq!(doc.value(mover, "opacity"), "0.25");
    scroll(&mut doc, scroller, 1000.0);
    assert_eq!(doc.value(mover, "opacity"), "1");
    scroll(&mut doc, scroller, 100.0);
    assert_eq!(
        doc.value(mover, "opacity"),
        "0.1",
        "scrolling back un-finishes it"
    );
}

/// An animation created at a scrolled offset shows that offset's value in
/// the layout that creates it.
#[test]
fn a_scroll_driven_animation_starts_at_the_offset_it_finds() {
    let (mut doc, scroller, mover) = scroll_driven("");
    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 500.0));
    doc.add_class(mover, "driven");
    doc.flush();
    assert_eq!(doc.value(mover, "opacity"), "0.5");
}

/// A paused animation holds its sample; one created paused takes the
/// sample of the moment it was created; resuming re-aligns to the scroll
/// position, as Blink does.
#[test]
fn a_paused_scroll_driven_animation_holds_and_resumes_aligned() {
    let (mut doc, scroller, mover) = scroll_driven("");
    doc.dom.scroll_to(scroller, Vector2D::new(0.0, 300.0));
    doc.add_class(mover, "driven");
    doc.add_class(mover, "paused");
    doc.flush();
    assert_eq!(doc.value(mover, "opacity"), "0.3", "created paused at 300");
    scroll(&mut doc, scroller, 600.0);
    doc.flush();
    assert_eq!(doc.value(mover, "opacity"), "0.3", "held while paused");
    doc.remove_class(mover, "paused");
    doc.flush();
    assert_eq!(
        doc.value(mover, "opacity"),
        "0.6",
        "resumed at the scroll position"
    );
    doc.add_class(mover, "paused");
    doc.flush();
    scroll(&mut doc, scroller, 900.0);
    assert_eq!(doc.value(mover, "opacity"), "0.6", "paused again");
}

/// css-contain-2 §4: an animation in skipped contents does not advance on
/// its timeline, and follows it again once revealed.
#[test]
fn a_scroll_driven_animation_in_skipped_contents_holds() {
    let mut doc = Doc::with_css(SCROLL_DRIVEN);
    let root = doc.root;
    let scroller = doc.el(root, "view.scroller");
    let wrapper = doc.el(scroller, "view.wrapper");
    let mover = doc.el(wrapper, "view.mover.driven");
    doc.el(scroller, "view.filler");
    doc.flush();
    scroll(&mut doc, scroller, 200.0);
    assert_eq!(doc.value(mover, "opacity"), "0.2");
    doc.add_class(wrapper, "hidden");
    doc.flush();
    scroll(&mut doc, scroller, 700.0);
    doc.flush();
    assert_eq!(doc.value(mover, "opacity"), "0.2", "frozen while skipped");
    doc.remove_class(wrapper, "hidden");
    doc.flush();
    assert_eq!(doc.value(mover, "opacity"), "0.7", "revealed at the offset");
}

/// An inactive timeline has no effect whatever the fill: the base value
/// shows.
#[test]
fn an_inactive_scroll_timeline_leaves_the_base_value() {
    let mut doc = Doc::with_css(SCROLL_DRIVEN);
    let root = doc.root;
    let unscrolled = doc.el(root, "view.mover.driven");
    let scroller = doc.el(root, "view.scroller");
    let short = doc.el(scroller, "view.mover.driven");
    doc.flush();
    assert_eq!(doc.value(unscrolled, "opacity"), "1", "no scroll container");
    assert_eq!(doc.value(short, "opacity"), "1", "nothing to scroll");
}

/// A scroll-driven `width` reaches layout: the stale-timelines pass lays the
/// first frame out at the sampled width, and a scroll relayouts at the next.
#[test]
fn a_scroll_driven_width_reaches_layout() {
    let (mut doc, scroller, mover) = scroll_driven("");
    doc.set_inline(
        mover,
        "animation: grow linear both; animation-timeline: scroll()",
    );
    doc.flush();
    assert_eq!(box_of(&doc, mover).1.width, 100.0);
    scroll(&mut doc, scroller, 500.0);
    doc.flush();
    assert_eq!(box_of(&doc, mover).1.width, 200.0);
}

/// Absolute values, not only one path against another: `reverse` at a
/// quarter shows keyframe 0.75, `alternate` flips each iteration, a
/// backwards-filled `steps(2, jump-start)` before its range shows the 0%
/// keyframe, not the first step, the last of three `steps(4)` iterations
/// ends on the last keyframe, and a 1s delay before 1s is the first half.
#[test]
fn scroll_driven_values_match_the_keyframes_they_name() {
    for (animation, offset, expected) in [
        ("fade linear both reverse", 250.0, "0.75"),
        ("fade linear both alternate 2", 100.0, "0.2"),
        ("fade linear both alternate 2", 600.0, "0.8"),
        ("fade steps(2, jump-start) both", 0.0, "0.5"),
        ("fade steps(4) 3 both", 1000.0, "1"),
        ("fade 1s 1s linear both", 250.0, "0"),
        ("fade 1s 1s linear both", 750.0, "0.5"),
    ] {
        let (mut doc, scroller, mover) = scroll_driven("");
        doc.set_inline(
            mover,
            &format!("animation: {animation}; animation-timeline: scroll()"),
        );
        doc.flush();
        scroll(&mut doc, scroller, offset);
        assert_eq!(
            doc.value(mover, "opacity"),
            expected,
            "{animation} at {offset}"
        );
    }
    let (mut doc, scroller, mover) = scroll_driven("");
    doc.set_inline(
        mover,
        "animation: fade steps(2, jump-start) both; animation-timeline: scroll();
         animation-range: 50% 100%",
    );
    doc.flush();
    assert_eq!(doc.value(mover, "opacity"), "0", "before the range");
    scroll(&mut doc, scroller, 600.0);
    assert_eq!(
        doc.value(mover, "opacity"),
        "0.5",
        "the first step inside it"
    );
}

/// A paused animation keeps holding when an earlier animation leaves its
/// set and its place in the set moves.
#[test]
fn a_paused_scroll_driven_animation_holds_when_its_set_shrinks() {
    let mut doc = Doc::with_css(&format!(
        "{SCROLL_DRIVEN}
         @keyframes pulse {{ from {{ color: red; }} to {{ color: blue; }} }}"
    ));
    let root = doc.root;
    let scroller = doc.el(root, "view.scroller");
    let mover = doc.el(scroller, "view.mover");
    doc.el(scroller, "view.filler");
    doc.set_inline(
        mover,
        "animation: pulse 10s, fade linear both; animation-timeline: auto, scroll();
         animation-play-state: running, paused",
    );
    doc.flush();
    assert_eq!(doc.value(mover, "opacity"), "0", "created paused at 0");
    scroll(&mut doc, scroller, 500.0);
    doc.flush();
    assert_eq!(doc.value(mover, "opacity"), "0", "held");
    doc.set_inline(
        mover,
        "animation: fade linear both; animation-timeline: scroll();
         animation-play-state: paused",
    );
    doc.flush();
    assert_eq!(doc.value(mover, "opacity"), "0", "still held");
}
