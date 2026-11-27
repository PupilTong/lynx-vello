//! Ports of the web-platform-tests `css/css-animations` testharness files
//! (WPT commit `8ffeed333b8ebc424002bdc518a13342a036b88f`).
//!
//! Adaptation. terminus has no JavaScript, so each port builds the same DOM
//! and CSS, drives the document's animation clock, and reads computed style
//! with [`Document::computed_style_text`]. The WPT idioms map as follows:
//!
//! - `anim.currentTime = N` or an animation that has run for `N` ms becomes `advance_animations(N /
//!   1000)` after the flush that created the animation and a first tick at `0.0`. A test that fixes
//!   the time with `paused` plus a negative delay keeps `paused` and a zero clock; a test that
//!   scrubs a paused animation drops `paused` and ticks instead.
//! - `div.style.x = ...` becomes `set_inline` and a flush.
//! - `await waitForAnimationFrames(n)` becomes ticks at later clock values.
//! - `<div>` becomes `<view>`, because the Lynx grammar rejects `display: block` and `display:
//!   inline`; where a test needs a second `display` value, `flex` replaces `block` or `inline`.
//!
//! Not portable, and recorded as NA in `wpt/ported.tsv`: tests about the Web
//! Animations objects (`Animation`, `CSSAnimation`, `KeyframeEffect`,
//! `getAnimations()`, `getKeyframes()`, `ready`/`finished` promises,
//! `startTime`, `idlharness`), the animation event interfaces and their
//! dispatch order, pseudo-elements, `<dialog>` and `::backdrop`, `<iframe>`,
//! SVG filter primitives, CSSOM rule editing, and CSS the Lynx grammar lacks
//! (`color-mix()`, `oklch()`, relative colors, `animation-composition`,
//! `column-width`, `column-rule-color`, `text-decoration-inset`).
#![allow(
    clippy::float_cmp,
    reason = "the clock values are exact binary fractions"
)]

mod common;

use common::{Doc, specified};
use terminus::link::NodeId;

/// The computed value of `property`, serialized as `getComputedStyle` does.
fn val(doc: &Doc, id: NodeId, property: &str) -> String {
    doc.dom
        .computed_style_text(id, property, false)
        .unwrap_or_else(|| panic!("`{property}` has no computed value"))
}

/// Lays the document out and ticks the clock at `0.0`, the frame every
/// animation created so far starts on.
fn start(doc: &mut Doc) {
    doc.flush();
    doc.dom.advance_animations(0.0);
}

/// Moves the animation clock to `seconds` since [`start`].
fn tick(doc: &mut Doc, seconds: f64) {
    doc.dom.advance_animations(seconds);
}

/// A document with `css` and one `view` per spec under the root.
fn doc_with(css: &str, specs: &[&str]) -> (Doc, Vec<NodeId>) {
    let mut doc = Doc::with_css(css);
    let root = doc.root;
    let ids = doc.els(root, specs);
    (doc, ids)
}

// ---------------------------------------------------------------------------
// animation-base-response-001.html .. 003.html
// ---------------------------------------------------------------------------

const BASE_RESPONSE_001: &str = "
    @keyframes font_size_animation { from { font-size: 10px; } to { font-size: 20px; } }
    @keyframes var_animation { from { --x: 10px; } to { --x: 20px; } }
    #targets > view {
        animation-duration: 1000s;
        animation-delay: -500s;
        animation-timing-function: steps(2, end);
    }
    #target1 { animation-name: font_size_animation; font-size: 1px; width: 1em; }
    #ref1 { width: 15px; }
    #target2 { animation-name: font_size_animation; font-size: 1px; width: 1ex; }
    #ref2 { font-size: 15px; width: 1ex; }
    #target3 { animation-name: var_animation; --x: 0px; width: var(--x); }
    #ref3 { width: 20px; }
";

fn base_response_001() -> Doc {
    let mut doc = Doc::with_css(BASE_RESPONSE_001);
    let root = doc.root;
    let targets = doc.el(root, "view#targets");
    doc.els(targets, &["view#target1", "view#target2", "view#target3"]);
    let refs = doc.el(root, "view#refs");
    doc.els(refs, &["view#ref1", "view#ref2", "view#ref3"]);
    start(&mut doc);
    doc
}

fn by_id(doc: &Doc, id: &str) -> NodeId {
    doc.dom
        .query_selector(doc.root, &format!("#{id}"))
        .expect("the selector parses")
        .unwrap_or_else(|| panic!("no element #{id}"))
}

/// animation-base-response-001.html, "em units respond to font-size animation".
#[test]
fn wpt_animation_base_response_001_em_units() {
    let doc = base_response_001();
    assert_eq!(
        val(&doc, by_id(&doc, "target1"), "width"),
        val(&doc, by_id(&doc, "ref1"), "width")
    );
}

/// animation-base-response-001.html, "ex units respond to font-size animation".
#[test]
fn wpt_animation_base_response_001_ex_units() {
    let doc = base_response_001();
    assert_eq!(
        val(&doc, by_id(&doc, "target2"), "width"),
        val(&doc, by_id(&doc, "ref2"), "width")
    );
}

/// animation-base-response-001.html, "`var()` references respond to custom
/// property animation".
#[test]
fn wpt_animation_base_response_001_var_references() {
    let doc = base_response_001();
    assert_eq!(
        val(&doc, by_id(&doc, "target3"), "width"),
        val(&doc, by_id(&doc, "ref3"), "width")
    );
}

/// animation-base-response-002.html, "Animated font-size on root affects rem
/// units".
#[test]
fn wpt_animation_base_response_002_rem_units() {
    let (mut doc, ids) = doc_with(
        "@keyframes font_size_animation { from { font-size: 10px; } to { font-size: 20px; } }
         :root { font-size: 1px; animation: font_size_animation steps(2, end) 1000s -500s; }
         #target1 { width: 1rem; }",
        &["view#target1"],
    );
    start(&mut doc);
    assert_eq!(val(&doc, ids[0], "width"), "15px");
}

/// animation-base-response-003.html, "Identical elements are all responsive to
/// font-size animation".
#[test]
fn wpt_animation_base_response_003_identical_elements() {
    let (mut doc, ids) = doc_with(
        "@keyframes font_size_animation { from { font-size: 10px; } to { font-size: 20px; } }
         view { font-size: 1px; min-width: 1em;
                animation: font_size_animation steps(2, end) 1000s -500s; }",
        &["view", "view", "view"],
    );
    start(&mut doc);
    for id in ids {
        assert_eq!(val(&doc, id, "min-width"), "15px");
    }
}

// ---------------------------------------------------------------------------
// animation-change-underlying-value-changed-in-flight.html
// ---------------------------------------------------------------------------

const IMPLICIT_KEYFRAMES: &str = "
    @keyframes implicit-from { to { margin-left: 100px } }
    @keyframes implicit-to { from { margin-left: 100px } }
";

fn change_underlying(name: &str) -> (String, String) {
    let (mut doc, ids) = doc_with(IMPLICIT_KEYFRAMES, &["view"]);
    let div = ids[0];
    doc.set_inline(
        div,
        &format!("animation: {name} 10s paused steps(2, start)"),
    );
    start(&mut doc);
    let before = val(&doc, div, "margin-left");
    doc.set_inline(
        div,
        &format!("animation: {name} 10s paused steps(2, start); margin-left: 200px"),
    );
    doc.flush();
    (before, val(&doc, div, "margin-left"))
}

/// animation-change-underlying-value-changed-in-flight.html, "Changing the
/// underlying value of an animated property with an implicit 0% keyframe".
#[test]
#[ignore = "terminus: animated value keeps the stale implicit keyframe after the underlying style changes"]
fn wpt_animation_change_underlying_value_implicit_from() {
    let (before, after) = change_underlying("implicit-from");
    assert_eq!(before, "50px", "before changing the underlying style");
    assert_eq!(after, "150px", "after changing the underlying style");
}

/// animation-change-underlying-value-changed-in-flight.html, "Changing the
/// underlying value of an animated property with an implicit 100% keyframe".
#[test]
#[ignore = "terminus: animated value keeps the stale implicit keyframe after the underlying style changes"]
fn wpt_animation_change_underlying_value_implicit_to() {
    let (before, after) = change_underlying("implicit-to");
    assert_eq!(before, "50px", "before changing the underlying style");
    assert_eq!(after, "150px", "after changing the underlying style");
}

// ---------------------------------------------------------------------------
// animation-css-variable-dependent-property.html
// ---------------------------------------------------------------------------

/// animation-css-variable-dependent-property.html, "Dependent property updates
/// correctly". The paused animation scrubbed with `currentTime` runs on the
/// clock instead.
#[test]
fn wpt_animation_css_variable_dependent_property_updates() {
    let (mut doc, ids) = doc_with(
        "@property --c { syntax: \"<color>\"; inherits: true; initial-value: black; }
         @keyframes color-shift { 0% { --c: black; } 100% { --c: white; } }
         #target { color: var(--c); animation: color-shift 1s linear 1 forwards; }",
        &["view#target"],
    );
    start(&mut doc);
    assert_eq!(val(&doc, ids[0], "color"), "rgb(0, 0, 0)");
    tick(&mut doc, 0.5);
    assert_eq!(val(&doc, ids[0], "color"), "rgb(128, 128, 128)");
    tick(&mut doc, 1.0);
    assert_eq!(val(&doc, ids[0], "color"), "rgb(255, 255, 255)");
}

// ---------------------------------------------------------------------------
// animation-css-variable-in-keyframe-adjusted.html
// ---------------------------------------------------------------------------

const VARIABLE_KEYFRAMES: &str = "
    @keyframes anim {
        from { margin-left: var(--margin-left) }
        to { margin-left: calc(var(--margin-left) * 2) }
    }
";

/// animation-css-variable-in-keyframe-adjusted.html, "Animations reflect
/// changes to variables on element".
#[test]
#[ignore = "terminus: var() in keyframes is not re-resolved after the variable changes"]
fn wpt_animation_css_variable_in_keyframe_adjusted_element() {
    let (mut doc, ids) = doc_with(VARIABLE_KEYFRAMES, &["view"]);
    let div = ids[0];
    doc.set_inline(div, "--margin-left: 100px; animation: anim 1s linear");
    start(&mut doc);
    tick(&mut doc, 0.5);
    assert_eq!(val(&doc, div, "margin-left"), "150px", "before updating");
    doc.set_inline(div, "--margin-left: 200px; animation: anim 1s linear");
    doc.flush();
    assert_eq!(val(&doc, div, "margin-left"), "300px", "after updating");
}

/// animation-css-variable-in-keyframe-adjusted.html, "Animations reflect
/// changes to variables on parent element".
#[test]
#[ignore = "terminus: var() in keyframes is not re-resolved after the inherited variable changes"]
fn wpt_animation_css_variable_in_keyframe_adjusted_parent() {
    let mut doc = Doc::with_css(VARIABLE_KEYFRAMES);
    let root = doc.root;
    let parent = doc.el(root, "view");
    let div = doc.el(parent, "view");
    doc.set_inline(parent, "--margin-left: 100px");
    doc.set_inline(div, "animation: anim 1s linear");
    start(&mut doc);
    tick(&mut doc, 0.5);
    assert_eq!(val(&doc, div, "margin-left"), "150px", "before updating");
    doc.set_inline(parent, "--margin-left: 200px");
    doc.flush();
    assert_eq!(val(&doc, div, "margin-left"), "300px", "after updating");
}

// ---------------------------------------------------------------------------
// animation-important-001.html (the ::before, :visited, setKeyframes and
// CSS.registerProperty subtests are not portable)
// ---------------------------------------------------------------------------

const IMPORTANT_001: &str = "
    @keyframes bgcolor_animation {
        from { background-color: rgb(10, 10, 10); }
        to { background-color: rgb(20, 20, 20); }
    }
    @keyframes color_and_bg_animation {
        from { background-color: rgb(10, 10, 10); color: rgb(10, 10, 10); }
        to { background-color: rgb(20, 20, 20); color: rgb(20, 20, 20); }
    }
    view {
        animation-duration: 1000s;
        animation-delay: -500s;
        animation-timing-function: steps(2, end);
    }
    #target1 { animation-name: bgcolor_animation; }
    #target2 { background-color: rgb(0, 128, 0) !important; animation-name: bgcolor_animation; }
    #target3 { color: rgb(0, 128, 0) !important; animation-name: color_and_bg_animation; }
";

fn important_001() -> (Doc, Vec<NodeId>) {
    let (mut doc, ids) = doc_with(
        IMPORTANT_001,
        &["view#target1", "view#target2", "view#target3"],
    );
    start(&mut doc);
    (doc, ids)
}

/// animation-important-001.html, "Interpolated value is observable".
#[test]
fn wpt_animation_important_001_interpolated_value_is_observable() {
    let (doc, ids) = important_001();
    assert_eq!(val(&doc, ids[0], "background-color"), "rgb(15, 15, 15)");
}

/// animation-important-001.html, "Important rules override animations".
#[test]
fn wpt_animation_important_001_important_overrides_animation() {
    let (doc, ids) = important_001();
    assert_eq!(val(&doc, ids[1], "background-color"), "rgb(0, 128, 0)");
}

/// animation-important-001.html, "Non-overriden interpolations are
/// observable".
#[test]
fn wpt_animation_important_001_non_overridden_interpolations() {
    let (doc, ids) = important_001();
    assert_eq!(val(&doc, ids[2], "color"), "rgb(0, 128, 0)");
    assert_eq!(val(&doc, ids[2], "background-color"), "rgb(15, 15, 15)");
}

// ---------------------------------------------------------------------------
// animation-important-with-transition.html. The `getAnimations()` and
// `instanceof` assertions are dropped; the computed-value assertions stay.
// ---------------------------------------------------------------------------

const IMPORTANT_TRANSITION: &str = "
    @keyframes changeColor {
        from { color: rgb(0, 0, 255); }
        to { color: rgb(0, 0, 255); }
    }
    @keyframes changeColorAndOpacity {
        from { color: rgb(0, 0, 255); opacity: 0; }
        to { color: rgb(0, 0, 255); opacity: 0; }
    }
    #target1 {
        color: rgb(255, 0, 0) !important;
        opacity: 1;
        transition: opacity 1000s steps(1);
        animation: changeColor 1000s forwards;
    }
    #target2 { transition: color 1000s linear; }
    #target2.green { color: rgb(0, 128, 0) !important; }
    #target2.red { color: rgb(255, 0, 0) !important; }
    #target3 {
        color: rgb(255, 0, 0) !important;
        transition: opacity 1000s linear;
        animation: changeColor 1000s forwards;
    }
    #target3.start { opacity: 1 !important; }
    #target3.end { opacity: 0.5 !important; }
    #target4 {
        color: rgb(255, 0, 0) !important;
        opacity: 1 !important;
        background-color: white;
        transition: background-color 1000s steps(1);
        animation: changeColorAndOpacity 1000s forwards;
    }
";

fn important_transition() -> (Doc, Vec<NodeId>) {
    let (mut doc, ids) = doc_with(
        IMPORTANT_TRANSITION,
        &[
            "view#target1",
            "view#target2.green",
            "view#target3.start",
            "view#target4",
        ],
    );
    start(&mut doc);
    (doc, ids)
}

/// animation-important-with-transition.html, "!important overrides animation
/// when no transition is running".
#[test]
fn wpt_animation_important_with_transition_no_transition() {
    let (doc, ids) = important_transition();
    assert_eq!(val(&doc, ids[0], "color"), "rgb(255, 0, 0)");
}

/// animation-important-with-transition.html, "!important overrides animation
/// even when a CSS transition is concurrently running".
#[test]
fn wpt_animation_important_with_transition_concurrent_transition() {
    let (mut doc, ids) = important_transition();
    doc.set_inline(ids[0], "opacity: 0.5");
    doc.flush();
    tick(&mut doc, 0.0);
    assert_eq!(
        val(&doc, ids[0], "color"),
        "rgb(255, 0, 0)",
        "color: red !important overrides the animation while a transition runs"
    );
}

/// animation-important-with-transition.html, "!important does not override a
/// running CSS transition".
#[test]
fn wpt_animation_important_with_transition_does_not_override_transition() {
    let (mut doc, ids) = important_transition();
    assert_eq!(val(&doc, ids[1], "color"), "rgb(0, 128, 0)");
    doc.dom.set_classes(ids[1], "red");
    doc.flush();
    tick(&mut doc, 0.0);
    tick(&mut doc, 500.0);
    assert_ne!(
        val(&doc, ids[1], "color"),
        "rgb(255, 0, 0)",
        "the transition on color is not snapped to the !important target"
    );
}

/// animation-important-with-transition.html, "!important overrides animation
/// but not transition on the same element".
#[test]
fn wpt_animation_important_with_transition_animation_not_transition() {
    let (mut doc, ids) = important_transition();
    doc.dom.set_classes(ids[2], "end");
    doc.flush();
    tick(&mut doc, 0.0);
    tick(&mut doc, 500.0);
    assert_eq!(
        val(&doc, ids[2], "color"),
        "rgb(255, 0, 0)",
        "color: red !important overrides the animation on color"
    );
    assert_ne!(
        val(&doc, ids[2], "opacity"),
        "0.5",
        "the opacity transition is not snapped to the !important target"
    );
}

/// animation-important-with-transition.html, "!important overrides multiple
/// animated properties even with a concurrent transition".
#[test]
fn wpt_animation_important_with_transition_multiple_properties() {
    let (mut doc, ids) = important_transition();
    doc.set_inline(ids[3], "background-color: rgb(0, 128, 0)");
    doc.flush();
    tick(&mut doc, 0.0);
    assert_eq!(val(&doc, ids[3], "color"), "rgb(255, 0, 0)");
    assert_eq!(val(&doc, ids[3], "opacity"), "1");
}

// ---------------------------------------------------------------------------
// animation-iteration-count-009.html, 010.html, calc.html
// ---------------------------------------------------------------------------

fn margin_after(animation: &str) -> String {
    let (mut doc, ids) = doc_with(
        "@keyframes margin-animation { from { margin-left: 0px; } to { margin-left: 100px; } }",
        &["view"],
    );
    doc.set_inline(ids[0], &format!("animation: {animation}"));
    start(&mut doc);
    val(&doc, ids[0], "margin-left")
}

/// animation-iteration-count-009.html, "Basic floating point iteration count".
#[test]
fn wpt_animation_iteration_count_009_basic() {
    assert_eq!(
        margin_after("margin-animation 1s -10s linear 1.5 normal forwards paused"),
        "50px"
    );
}

/// animation-iteration-count-009.html, "Floating point iteration count after
/// multiple iterations".
#[test]
fn wpt_animation_iteration_count_009_after_multiple_iterations() {
    assert_eq!(
        margin_after("margin-animation 1s -10s linear 3.25 normal forwards paused"),
        "25px"
    );
}

/// animation-iteration-count-009.html, "Floating point iteration count during
/// first iteration".
#[test]
fn wpt_animation_iteration_count_009_first_iteration() {
    assert_eq!(
        margin_after("margin-animation 1s -10s linear 0.75 normal forwards paused"),
        "75px"
    );
}

/// animation-iteration-count-009.html, "Floating point iteration count with
/// alternating directions".
#[test]
fn wpt_animation_iteration_count_009_alternating() {
    assert_eq!(
        margin_after("margin-animation 1s -10s linear 1.75 alternate forwards paused"),
        "25px"
    );
}

/// animation-iteration-count-010.html, one subtest per `animation-iteration-count`
/// value. `float: left`, the border and the margin only lay the boxes out and
/// are dropped.
#[test]
fn wpt_animation_iteration_count_010_paused_iteration_counts() {
    let cases = [
        ("1", "rgba(0, 0, 0, 0)"),
        ("2", "rgba(0, 0, 0, 0)"),
        ("2.1", "rgb(0, 128, 0)"),
        ("3", "rgb(0, 128, 0)"),
        ("4", "rgb(0, 128, 0)"),
        ("infinite", "rgb(0, 128, 0)"),
    ];
    for (count, expected) in cases {
        let (mut doc, ids) = doc_with(
            "@keyframes keyframes { from { background: green } to { background: red } }
             view { animation: 1s -2s infinite paused keyframes; width: 100px; height: 100px; }",
            &["view"],
        );
        doc.set_inline(ids[0], &format!("animation-iteration-count: {count}"));
        start(&mut doc);
        assert_eq!(
            val(&doc, ids[0], "background-color"),
            expected,
            "animation-iteration-count: {count}"
        );
    }
}

/// animation-iteration-count-calc.html, "`calc()` should be accepted in
/// animation-iteration-count."
#[test]
fn wpt_animation_iteration_count_calc_accepted() {
    let (mut doc, ids) = doc_with("", &["view"]);
    doc.set_inline(ids[0], "animation-iteration-count: calc(1 + 3)");
    doc.flush();
    assert_eq!(val(&doc, ids[0], "animation-iteration-count"), "4");
}

// ---------------------------------------------------------------------------
// animation-multiple-from-to-keyframes-with-only-timing-function.html
// ---------------------------------------------------------------------------

/// animation-multiple-from-to-keyframes-with-only-timing-function.html,
/// "Animations should not use an implicit value when the first rule within a
/// @keyframes rule specifies "from" and "to" but only specifies a timing
/// function."
#[test]
fn wpt_animation_multiple_from_to_keyframes_with_only_timing_function() {
    let (mut doc, ids) = doc_with(
        "@keyframes multiple-from-to-keyframes {
            from, to { animation-timing-function: cubic-bezier(0, 0, 1, 1) }
            from { left: 50px }
            to { left: 150px }
        }",
        &["view"],
    );
    doc.set_inline(
        ids[0],
        "animation: multiple-from-to-keyframes 100s -50s linear",
    );
    start(&mut doc);
    assert_eq!(val(&doc, ids[0], "left"), "100px");
}

// ---------------------------------------------------------------------------
// animation-name-case-sensitive.html, animation-name-inline-style.html
// ---------------------------------------------------------------------------

/// animation-name-case-sensitive.html, "animation-name is case-sensitive". The
/// inline declaration is read back through the computed `animation-name`.
#[test]
fn wpt_animation_name_case_sensitive() {
    let (mut doc, ids) = doc_with("", &["view"]);
    doc.set_inline(ids[0], "animation-name: fooBar");
    doc.flush();
    assert_eq!(val(&doc, ids[0], "animation-name"), "fooBar");
}

/// animation-name-inline-style.html, "Animation starts when the name is
/// mutated via inline style".
#[test]
fn wpt_animation_name_inline_style() {
    let (mut doc, ids) = doc_with(
        "#target { color: red }
         @keyframes green { from, to { color: green } }",
        &["view#target"],
    );
    let target = ids[0];
    doc.set_inline(target, "animation-duration: 1s");
    doc.flush();
    doc.set_inline(target, "animation-duration: 1s; animation-name: none");
    doc.flush();
    assert_eq!(val(&doc, target, "color"), "rgb(255, 0, 0)");
    doc.set_inline(target, "animation-duration: 1s; animation-name: green");
    start(&mut doc);
    assert_eq!(val(&doc, target, "color"), "rgb(0, 128, 0)");
}

// ---------------------------------------------------------------------------
// animation-play-state-005.tentative.html
// ---------------------------------------------------------------------------

/// animation-play-state-005.tentative.html, "CSS animation should not restart
/// after resetting its animation-play-state". The `animationend` listener
/// becomes a tick past the end of the 100ms animation.
#[test]
#[ignore = "terminus: a finished animation restarts when animation-play-state is toggled"]
fn wpt_animation_play_state_005_no_restart_after_reset() {
    let (mut doc, ids) = doc_with(
        "@keyframes anim {
            from { transform: translateX(100px); }
            to { transform: translateX(200px); }
        }",
        &["view"],
    );
    let div = ids[0];
    doc.set_inline(div, "animation: anim 100ms");
    start(&mut doc);
    tick(&mut doc, 0.2);
    doc.set_inline(div, "animation: anim 100ms; animation-play-state: paused");
    doc.flush();
    doc.set_inline(div, "animation: anim 100ms; animation-play-state: running");
    doc.flush();
    tick(&mut doc, 0.3);
    assert!(
        !doc.dom.has_active_animations(),
        "the finished animation did not start again"
    );
    assert_eq!(val(&doc, div, "transform"), "none");
}

// ---------------------------------------------------------------------------
// animation-restarted-after-changing-iteration-count-after-completion.html
// ---------------------------------------------------------------------------

/// animation-restarted-after-changing-iteration-count-after-completion.html,
/// "Setting 'animation-iteration-count: infinite' after a CSS Animation is
/// completed restarts the animation." `getAnimations().length` becomes
/// `has_active_animations` and the sampled `margin-left`.
#[test]
fn wpt_animation_restarted_after_changing_iteration_count_after_completion() {
    let (mut doc, ids) = doc_with("@keyframes anim { to { margin-left: 100px } }", &["view"]);
    let target = ids[0];
    doc.set_inline(target, "animation: anim 0.1s linear");
    start(&mut doc);
    assert!(
        doc.dom.has_active_animations(),
        "an animation runs initially"
    );
    tick(&mut doc, 0.2);
    assert!(
        !doc.dom.has_active_animations(),
        "no animation runs after completion"
    );
    assert_eq!(val(&doc, target, "margin-left"), "0px");

    doc.set_inline(
        target,
        "animation: anim 0.1s linear; animation-iteration-count: infinite",
    );
    doc.flush();
    assert!(
        doc.dom.has_active_animations(),
        "an animation runs again once animation-iteration-count is set"
    );
    tick(&mut doc, 0.3);
    tick(&mut doc, 0.35);
    assert_eq!(val(&doc, target, "margin-left"), "50px");
}

// ---------------------------------------------------------------------------
// animation-style-element-replaced-with-keyframes-rule-of-same-name.html
// ---------------------------------------------------------------------------

/// animation-style-element-replaced-with-keyframes-rule-of-same-name.html,
/// "Replacing a <style> element with a new <style> element while both
/// containing the different @keyframes rule with the same name dynamically
/// updates running animations." terminus cannot remove a stylesheet, so the
/// second sheet is added after the first and its rule of the same name wins
/// the cascade.
#[test]
#[ignore = "terminus: a later @keyframes rule of the same name does not update the running animation"]
fn wpt_animation_style_element_replaced_with_keyframes_rule_of_same_name() {
    let (mut doc, ids) = doc_with(
        "@keyframes anim-custom { from, to { left: 100px } }",
        &["view"],
    );
    let div = ids[0];
    doc.set_inline(div, "animation: anim-custom 100s");
    start(&mut doc);
    assert_eq!(
        val(&doc, div, "left"),
        "100px",
        "the initial @keyframes rule"
    );
    doc.add_css("@keyframes anim-custom { from, to { left: 200px } }");
    doc.flush();
    assert_eq!(val(&doc, div, "left"), "200px", "the new @keyframes rule");
}

// ---------------------------------------------------------------------------
// computed-style-animation-parsing.html, style-animation-parsing.html
// ---------------------------------------------------------------------------

const ANIMATION_LONGHANDS: [&str; 8] = [
    "animation-name",
    "animation-duration",
    "animation-timing-function",
    "animation-delay",
    "animation-iteration-count",
    "animation-direction",
    "animation-fill-mode",
    "animation-play-state",
];

/// The `animation` shorthand parsed from `input`, serialized as CSSOM does.
fn serialized_animation(input: &str) -> String {
    specified("animation", input).unwrap_or_default()
}

/// The shared body of both parsing files: `input` and its serialization must
/// give the same longhands and the expected `animation-name`. The
/// `computedStyle.animation` shorthand is not exposed, so the round trip
/// compares all eight computed longhands.
fn check_animation_parsing(input: &str, name: &str) {
    let (mut doc, ids) = doc_with("", &["view", "view"]);
    let serialized = serialized_animation(input);
    doc.set_inline(ids[0], &format!("animation: {input}"));
    doc.set_inline(ids[1], &format!("animation: {serialized}"));
    doc.flush();
    for id in &ids {
        assert_eq!(val(&doc, *id, "animation-name"), name, "input `{input}`");
    }
    for longhand in ANIMATION_LONGHANDS {
        assert_eq!(
            val(&doc, ids[0], longhand),
            val(&doc, ids[1], longhand),
            "`{input}` and its serialization `{serialized}` differ in {longhand}"
        );
    }
    assert_eq!(
        serialized_animation(&serialized),
        serialized,
        "`{input}` serializes stably"
    );
}

/// computed-style-animation-parsing.html, "Test animation name being empty.".
#[test]
fn wpt_computed_style_animation_parsing_empty() {
    check_animation_parsing("", "none");
}

/// computed-style-animation-parsing.html, "Test a non-conflicting animation
/// name.".
#[test]
fn wpt_computed_style_animation_parsing_non_conflicting() {
    check_animation_parsing("myShorthandAnim", "myShorthandAnim");
}

/// computed-style-animation-parsing.html, "Test an animation name that is the
/// same as a possible animation timing-function.".
#[test]
fn wpt_computed_style_animation_parsing_timing_function_names() {
    for (input, name) in [
        ("ease", "none"),
        ("ease-in", "none"),
        ("ease-in-out", "none"),
        ("ease-out", "none"),
        ("linear", "none"),
        ("step-end", "none"),
        ("step-start", "none"),
        ("ease ease", "ease"),
        ("ease linear", "linear"),
    ] {
        check_animation_parsing(input, name);
    }
}

/// computed-style-animation-parsing.html, "Test an animation name that is the
/// same as a possible animation iteration-count.".
#[test]
fn wpt_computed_style_animation_parsing_iteration_count_names() {
    check_animation_parsing("infinite", "none");
    check_animation_parsing("infinite infinite", "infinite");
}

/// computed-style-animation-parsing.html, "Test an animation name that is the
/// same as a possible animation fill-mode.".
#[test]
fn wpt_computed_style_animation_parsing_fill_mode_names() {
    check_animation_parsing("none", "none");
    check_animation_parsing("forwards", "none");
    check_animation_parsing("none forwards", "forwards");
}

/// computed-style-animation-parsing.html, "Test an animation name that is the
/// same as a possible animation direction.".
#[test]
fn wpt_computed_style_animation_parsing_direction_names() {
    check_animation_parsing("normal", "none");
    check_animation_parsing("reverse", "none");
    check_animation_parsing("normal normal", "normal");
    check_animation_parsing("normal reverse", "reverse");
}

/// computed-style-animation-parsing.html, "Test an animation name that is the
/// same as a possible running state.".
#[test]
fn wpt_computed_style_animation_parsing_play_state_names() {
    check_animation_parsing("running", "none");
    check_animation_parsing("paused", "none");
    check_animation_parsing("running running", "running");
    check_animation_parsing("running paused", "paused");
}

/// style-animation-parsing.html, "Test animation name being empty.". An empty
/// inline `animation` declares nothing, which computes to `none`.
#[test]
fn wpt_style_animation_parsing_empty() {
    check_animation_parsing("", "none");
}

/// style-animation-parsing.html, "Test a non-conflicting animation name.".
#[test]
fn wpt_style_animation_parsing_non_conflicting() {
    check_animation_parsing("myShorthandAnim", "myShorthandAnim");
}

/// style-animation-parsing.html, "Test an animation name that is the same as a
/// possible animation fill-mode.".
#[test]
fn wpt_style_animation_parsing_fill_mode_names() {
    check_animation_parsing("none", "none");
    check_animation_parsing("forwards", "none");
    check_animation_parsing("none forwards", "forwards");
}

/// style-animation-parsing.html, "Test an animation name that is the same as a
/// possible animation direction.".
#[test]
fn wpt_style_animation_parsing_direction_names() {
    check_animation_parsing("normal", "none");
    check_animation_parsing("reverse", "none");
    check_animation_parsing("normal normal", "normal");
    check_animation_parsing("normal reverse", "reverse");
}

/// style-animation-parsing.html, "Test an animation name that is the same as a
/// possible running state.".
#[test]
fn wpt_style_animation_parsing_play_state_names() {
    check_animation_parsing("running", "none");
    check_animation_parsing("paused", "none");
    check_animation_parsing("running running", "running");
    check_animation_parsing("running paused", "paused");
}

// ---------------------------------------------------------------------------
// display-contents-animates.html and the display: none family
// ---------------------------------------------------------------------------

/// display-contents-animates.html, "Animations inside of display:contents
/// should work properly." The `animationstart` count is dropped.
#[test]
fn wpt_display_contents_animates_inherited_visibility() {
    let mut doc = Doc::with_css(
        "@keyframes display1 { 0% { visibility: visible; } 100% { visibility: hidden; } }
         .animate1 { animation: display1 1s infinite; }",
    );
    let root = doc.root;
    let outer = doc.el(root, "view#outer");
    doc.set_inline(outer, "visibility: hidden; display: contents");
    let inner = doc.el(outer, "view#inner");
    start(&mut doc);
    doc.add_class(outer, "animate1");
    doc.flush();
    tick(&mut doc, 0.016);
    tick(&mut doc, 0.033);
    assert_eq!(val(&doc, inner, "visibility"), "visible");
}

/// The shared body of the `display-none-dont-cancel.tentative.html` subtests:
/// adds `class` to a flex element, ticks two frames and reads `display`. The
/// `animationstart` count is dropped.
fn display_during_animation(css: &str, class: &str, inline: &str) -> (Doc, NodeId) {
    let mut doc = Doc::with_css(css);
    let root = doc.root;
    let target = doc.el(root, "view");
    if !inline.is_empty() {
        doc.set_inline(target, inline);
    }
    start(&mut doc);
    doc.add_class(target, class);
    doc.flush();
    tick(&mut doc, 0.016);
    tick(&mut doc, 0.033);
    (doc, target)
}

/// display-none-dont-cancel.tentative.html, "display:none animating to
/// display:inline should be inline for the whole animation." `flex` replaces
/// `inline`.
#[test]
fn wpt_display_none_dont_cancel_none_to_flex() {
    let (doc, target) = display_during_animation(
        "@keyframes display1 { 0% { display: none; } 100% { display: flex; } }
         .animate1 { animation: display1 1s infinite; }",
        "animate1",
        "",
    );
    assert_eq!(val(&doc, target, "display"), "flex");
}

/// display-none-dont-cancel.tentative.html, "A CSS variable of display:none
/// animating to display:inline should be inline for the whole animation."
/// `flex` replaces `inline`.
#[test]
fn wpt_display_none_dont_cancel_variable_none_to_flex() {
    let (doc, target) = display_during_animation(
        "@keyframes display2 { 0% { display: var(--none-value); } 100% { display: flex; } }
         .animate2 { animation: display2 1s infinite; }",
        "animate2",
        "--none-value: none",
    );
    assert_eq!(val(&doc, target, "display"), "flex");
}

/// display-none-dont-cancel.tentative.html, "Animating from display:none to
/// display:none should not cancel the animation."
#[test]
#[ignore = "terminus: animating display none to none computes to the base display"]
fn wpt_display_none_dont_cancel_none_to_none() {
    let (doc, target) = display_during_animation(
        "@keyframes display3 { 0% { display: none; } 100% { display: none; } }
         .animate3 { animation: display3 1s infinite; }",
        "animate3",
        "",
    );
    assert_eq!(val(&doc, target, "display"), "none");
    assert!(
        doc.dom.has_active_animations(),
        "the animation keeps running"
    );
}

/// display-none-dont-cancel.tentative.html, "Animating from display:none to
/// display:none with an intermediate variable should not cancel the
/// animation."
#[test]
#[ignore = "terminus: animating display none to none through var() computes to the base display"]
fn wpt_display_none_dont_cancel_variable_none_to_none() {
    let (doc, target) = display_during_animation(
        "@keyframes display4 {
            0% { display: var(--none-value); }
            100% { display: var(--none-value); }
         }
         .animate4 { animation: display4 1s infinite; }",
        "animate4",
        "--none-value: none",
    );
    assert_eq!(val(&doc, target, "display"), "none");
    assert!(
        doc.dom.has_active_animations(),
        "the animation keeps running"
    );
}

/// display-none-dont-cancel.tentative.html, "Animating a variable of "none"
/// which gets set to display elsewhere should not cancel the animation."
/// `flex` replaces `block` in the `var()` fallback.
#[test]
fn wpt_display_none_dont_cancel_animated_variable() {
    let (doc, target) = display_during_animation(
        "@keyframes display5 { 0% { --display: none; } 100% { --display: none; } }
         .animate5 { animation: display5 1s infinite; }
         view { display: var(--display, flex); }",
        "animate5",
        "",
    );
    assert_eq!(val(&doc, target, "display"), "none");
    assert!(
        doc.dom.has_active_animations(),
        "the animation keeps running"
    );
}

/// display-none-to-display-block-dont-cancel.tentative.html, "display:none
/// animating to display:block should be block for the whole animation."
/// `flex` replaces `block`.
#[test]
fn wpt_display_none_to_display_block_dont_cancel() {
    let (mut doc, ids) = doc_with(
        "@keyframes display-animation { 0% { display: none; } 100% { display: flex; } }
         #target { display: none; }
         #target.animate { animation: display-animation 1s; display: flex; }",
        &["view#target"],
    );
    let target = ids[0];
    start(&mut doc);
    assert_eq!(val(&doc, target, "display"), "none", "before the animation");
    doc.add_class(target, "animate");
    doc.flush();
    tick(&mut doc, 0.016);
    tick(&mut doc, 0.033);
    assert_eq!(val(&doc, target, "display"), "flex", "during the animation");
}

/// display-none-to-display-block-while-running.tentative.html, "An animation
/// returning display to block displays the element while it is still
/// running." The `ResizeObserver` becomes the laid-out width after a tick past
/// the reveal time; `flex` replaces `block`.
#[test]
fn wpt_display_none_to_display_block_while_running() {
    let (mut doc, ids) = doc_with(
        "@keyframes reveal {
            0% { display: none; }
            30% { display: none; }
            100% { display: flex; }
         }
         #target { width: 100px; height: 100px; animation: reveal 200ms infinite; }",
        &["view#target"],
    );
    let target = ids[0];
    start(&mut doc);
    tick(&mut doc, 0.1);
    doc.flush();
    let layout = doc
        .dom
        .rounded_layout(target)
        .expect("the element has a box");
    assert!(layout.size.width > 0.0, "the element gets a box at 100ms");
}

/// display-none-prevents-starting-in-subtree.html, "Elements in a "display:
/// none" tree cannot start CSS Animations." `getAnimations({ subtree: true
/// }).length` becomes `has_active_animations`.
#[test]
#[ignore = "terminus: display: none on an ancestor does not cancel the running animation"]
fn wpt_display_none_prevents_starting_in_subtree() {
    let mut doc = Doc::with_css(
        "@keyframes margin { 100% { margin-left: 200px } }
         #child { animation: margin 1s forwards; }",
    );
    let root = doc.root;
    let container = doc.el(root, "view#container");
    let middle = doc.el(container, "view");
    let child = doc.el(middle, "view#child");
    start(&mut doc);
    assert!(
        doc.dom.has_active_animations(),
        "an animation runs on the child initially"
    );
    doc.set_inline(container, "display: none");
    doc.flush();
    tick(&mut doc, 0.1);
    assert!(
        !doc.dom.has_active_animations(),
        "display: none on the container canceled the animation"
    );
    doc.set_inline(container, "display: none; margin-left: 50px");
    doc.set_inline(child, "margin-left: 100px");
    doc.flush();
    tick(&mut doc, 0.2);
    assert!(
        !doc.dom.has_active_animations(),
        "changing styles does not restart the animation"
    );
}

// ---------------------------------------------------------------------------
// empty-pseudo-class-with-animation.html
// ---------------------------------------------------------------------------

/// empty-pseudo-class-with-animation.html, "Setting an "animation" style
/// property on an element does not interfere with the :empty pseudo-class."
#[test]
fn wpt_empty_pseudo_class_with_animation() {
    let mut doc = Doc::with_css(
        ".container { width: 100px; height: 100px; background-color: rgb(0, 255, 0); }
         .container:empty { background-color: rgb(255, 0, 0); animation: anim 1s; }
         @keyframes anim { }",
    );
    let root = doc.root;
    let container = doc.el(root, "view.container");
    start(&mut doc);
    assert_eq!(val(&doc, container, "background-color"), "rgb(255, 0, 0)");
    tick(&mut doc, 0.016);
    tick(&mut doc, 0.033);
    doc.el(container, "view");
    doc.flush();
    assert_eq!(val(&doc, container, "background-color"), "rgb(0, 255, 0)");
}

// ---------------------------------------------------------------------------
// inheritance.html
// ---------------------------------------------------------------------------

/// `assert_not_inherited(property, initial, other)`: the target computes to
/// `initial`, and `other` set on its container does not reach it.
fn assert_not_inherited(property: &str, initial: &str, other: &str) {
    let mut doc = Doc::new();
    let root = doc.root;
    let container = doc.el(root, "view#container");
    let target = doc.el(container, "view#target");
    doc.flush();
    assert_eq!(val(&doc, target, property), initial, "initial {property}");
    doc.set_inline(container, &format!("{property}: {other}"));
    doc.flush();
    assert_eq!(
        val(&doc, container, property),
        other,
        "{property} on the container"
    );
    assert_eq!(
        val(&doc, target, property),
        initial,
        "{property} is not inherited"
    );
}

/// inheritance.html, "animation-delay".
#[test]
fn wpt_inheritance_animation_delay() {
    assert_not_inherited("animation-delay", "0s", "2s");
}

/// inheritance.html, "animation-direction".
#[test]
fn wpt_inheritance_animation_direction() {
    assert_not_inherited("animation-direction", "normal", "reverse");
}

/// inheritance.html, "animation-duration".
#[test]
fn wpt_inheritance_animation_duration() {
    assert_not_inherited("animation-duration", "0s", "3s");
}

/// inheritance.html, "animation-fill-mode".
#[test]
fn wpt_inheritance_animation_fill_mode() {
    assert_not_inherited("animation-fill-mode", "none", "forwards");
}

/// inheritance.html, "animation-iteration-count".
#[test]
fn wpt_inheritance_animation_iteration_count() {
    assert_not_inherited("animation-iteration-count", "1", "4");
}

/// inheritance.html, "animation-name".
#[test]
fn wpt_inheritance_animation_name() {
    assert_not_inherited("animation-name", "none", "spinner");
}

/// inheritance.html, "animation-play-state".
#[test]
fn wpt_inheritance_animation_play_state() {
    assert_not_inherited("animation-play-state", "running", "paused");
}

/// inheritance.html, "animation-timing-function".
#[test]
fn wpt_inheritance_animation_timing_function() {
    assert_not_inherited("animation-timing-function", "ease", "linear");
}

// ---------------------------------------------------------------------------
// keyframes-unrelated-custom-property.html
// ---------------------------------------------------------------------------

/// keyframes-unrelated-custom-property.html, "Unrelated custom properties do
/// not conflict with each other".
#[test]
fn wpt_keyframes_unrelated_custom_property() {
    let (mut doc, ids) = doc_with(
        "@keyframes test {
            0% { --x: green; }
            100% { --x: green; }
            0% { --unused: yellow; }
         }
         #div { animation: test 10s linear paused; background-color: var(--x, red);
                width: 100px; height: 100px; }",
        &["view#div"],
    );
    start(&mut doc);
    assert_eq!(val(&doc, ids[0], "background-color"), "rgb(0, 128, 0)");
}

// ---------------------------------------------------------------------------
// missing-values-{first,middle,last}-keyframe.html
// ---------------------------------------------------------------------------

/// missing-values-first-keyframe.html, "Missing property values in the first
/// keyframe are correctly interpolated from a neutral keyframe value". The
/// paused animation scrubbed to 500ms runs on the clock instead.
#[test]
fn wpt_missing_values_first_keyframe() {
    let (mut doc, ids) = doc_with(
        ".box { position: relative; width: 100px; height: 100px; left: 0;
               background-color: green; }
         #box1 { left: 200px; animation: move-left 2s linear; }
         #box2 { transform: translateX(200px); animation: move-transform 2s linear; }
         @keyframes move-left {
            0% { opacity: 1; }
            25% { opacity: 1; }
            50% { left: 0; opacity: 1; }
            100% { left: 0; opacity: 0; }
         }
         @keyframes move-transform {
            0% { opacity: 1; }
            25% { opacity: 1; }
            50% { transform: translateX(0); opacity: 1; }
            100% { transform: translateX(0); opacity: 0; }
         }",
        &["view.box#box1", "view.box#box2"],
    );
    start(&mut doc);
    tick(&mut doc, 0.5);
    assert_eq!(val(&doc, ids[0], "left"), "100px");
    assert_eq!(val(&doc, ids[1], "transform"), "translateX(100px)");
}

/// missing-values-middle-keyframe.html, "Animation interpolates correctly when
/// some keyframes don't declare all values".
#[test]
fn wpt_missing_values_middle_keyframe() {
    let (mut doc, ids) = doc_with(
        "@keyframes move-left {
            0% { margin-left: 0px; opacity: 0; }
            25% { opacity: 1; }
            50% { }
            75% { opacity: 0; }
            100% { margin-left: 100px; opacity: 1; }
         }
         .box { position: relative; width: 100px; height: 100px; background-color: green; }",
        &["view.box#box1"],
    );
    doc.set_inline(
        ids[0],
        "animation: move-left 1s -10s linear 0.5 normal forwards paused",
    );
    start(&mut doc);
    assert_eq!(val(&doc, ids[0], "opacity"), "0.5");
    assert_eq!(val(&doc, ids[0], "margin-left"), "50px");
}

/// missing-values-last-keyframe.html, "Missing property values in the last
/// keyframe are correctly interpolated from a neutral keyframe value". The
/// 10s animation is sampled at 7.5s.
#[test]
fn wpt_missing_values_last_keyframe() {
    let (mut doc, ids) = doc_with(
        ".box { position: relative; width: 100px; height: 100px; left: 0;
               background-color: green; }
         #box1 { left: 200px; animation: move-left 10s linear; }
         #box2 { transform: translateX(200px); animation: move-transform 10s linear; }
         @keyframes move-left {
            0% { left: 0; opacity: 0; }
            50% { left: 0; opacity: 1; }
            75% { opacity: 1; }
            100% { opacity: 1; }
         }
         @keyframes move-transform {
            0% { transform: translateX(0); opacity: 0; }
            50% { transform: translateX(0); opacity: 1; }
            75% { opacity: 1; }
            100% { opacity: 1; }
         }",
        &["view.box#box1", "view.box#box2"],
    );
    start(&mut doc);
    tick(&mut doc, 7.5);
    assert_eq!(val(&doc, ids[0], "left"), "100px");
    assert_eq!(val(&doc, ids[1], "transform"), "translateX(100px)");
}

// ---------------------------------------------------------------------------
// responsive/line-height.html
// ---------------------------------------------------------------------------

/// responsive/line-height.html, "line-height responds to inherited changes".
#[test]
#[ignore = "terminus: inherit in keyframes is not re-resolved after the parent value changes"]
fn wpt_responsive_line_height_inherited_changes() {
    let mut doc = Doc::with_css(
        "#target { animation-name: line-height-animation; animation-duration: 4s;
                  animation-timing-function: linear; animation-delay: -2s;
                  animation-play-state: paused; }
         @keyframes line-height-animation {
            from { line-height: inherit; }
            to { line-height: 20px; }
         }",
    );
    let root = doc.root;
    let container = doc.el(root, "view#container");
    let target = doc.el(container, "view#target");
    start(&mut doc);
    for (container_height, expected) in [("100px", "60px"), ("50px", "35px"), ("100px", "60px")] {
        doc.set_inline(container, &format!("line-height: {container_height}"));
        doc.flush();
        assert_eq!(val(&doc, target, "line-height"), expected);
    }
}

// ---------------------------------------------------------------------------
// revert-rule-keyframes.html, revert-rule-keyframes-dynamic.html
// ---------------------------------------------------------------------------

const REVERT_RULE: &str = "
    @property --x { syntax: \"<length>\"; inherits: false; initial-value: 0px; }
    @keyframes anim { from { width: revert-rule; --x: revert-rule; } to { width: 200px; --x: 200px; } }
    #target { width: 100px; --x: 100px; animation: anim 3600s steps(2, start); }
";

/// revert-rule-keyframes.html, "revert-rule in keyframes (width)".
#[test]
fn wpt_revert_rule_keyframes_width() {
    let (mut doc, ids) = doc_with(REVERT_RULE, &["view#target"]);
    start(&mut doc);
    assert_eq!(val(&doc, ids[0], "width"), "150px");
}

/// revert-rule-keyframes.html, "revert-rule in keyframes (--x)".
#[test]
fn wpt_revert_rule_keyframes_custom_property() {
    let (mut doc, ids) = doc_with(REVERT_RULE, &["view#target"]);
    start(&mut doc);
    assert_eq!(val(&doc, ids[0], "--x"), "150px");
}

const REVERT_RULE_DYNAMIC: &str = "
    @property --x { syntax: \"<length>\"; inherits: false; initial-value: 0px; }
    @keyframes anim { from { width: revert-rule; } to { width: 200px; } }
    @keyframes anim-custom { from { --x: revert-rule; } to { --x: 200px; } }
    #target { width: 100px; --x: 100px;
              animation: anim 3600s steps(2, start), anim-custom 3600s steps(2, start); }
";

/// revert-rule-keyframes-dynamic.html, "revert-rule in keyframes, reverted-to
/// value changing (standard property)".
#[test]
#[ignore = "terminus: revert-rule in keyframes is not re-resolved after the reverted-to value changes"]
fn wpt_revert_rule_keyframes_dynamic_standard_property() {
    let (mut doc, ids) = doc_with(REVERT_RULE_DYNAMIC, &["view#target"]);
    start(&mut doc);
    assert_eq!(val(&doc, ids[0], "width"), "150px");
    doc.set_inline(ids[0], "width: 120px");
    doc.flush();
    assert_eq!(val(&doc, ids[0], "width"), "160px");
}

/// revert-rule-keyframes-dynamic.html, "revert-rule in keyframes, reverted-to
/// value changing (custom property)".
#[test]
#[ignore = "terminus: revert-rule in keyframes is not re-resolved after the reverted-to value changes"]
fn wpt_revert_rule_keyframes_dynamic_custom_property() {
    let (mut doc, ids) = doc_with(REVERT_RULE_DYNAMIC, &["view#target"]);
    start(&mut doc);
    assert_eq!(val(&doc, ids[0], "--x"), "150px");
    doc.set_inline(ids[0], "--x: 120px");
    doc.flush();
    assert_eq!(val(&doc, ids[0], "--x"), "160px");
}

// ---------------------------------------------------------------------------
// sample-on-last-keyframe.html
// ---------------------------------------------------------------------------

/// sample-on-last-keyframe.html, "Opacity at start of reverse running
/// animation".
#[test]
fn wpt_sample_on_last_keyframe_reverse_start() {
    let (mut doc, ids) = doc_with(
        "#test { animation: test 1s reverse paused; }
         @keyframes test { from { opacity: 0; } to { opacity: 0.5; } }",
        &["view#test"],
    );
    start(&mut doc);
    assert_eq!(val(&doc, ids[0], "opacity"), "0.5");
}

// ---------------------------------------------------------------------------
// transition-properties-not-animatable.html
// ---------------------------------------------------------------------------

/// `test_not_animatable`: an animation of `property` between `from` and `to`
/// leaves the computed value at `underlying`, halfway through.
fn assert_not_animatable(property: &str, from: &str, to: &str, underlying: &str) {
    let (mut doc, ids) = doc_with(
        &format!(
            "@keyframes anim {{ from {{ {property}: {from} }} to {{ {property}: {to} }} }}
             view {{ animation: anim 1s linear forwards; }}"
        ),
        &["view"],
    );
    start(&mut doc);
    tick(&mut doc, 0.5);
    assert_eq!(val(&doc, ids[0], property), underlying);
}

/// transition-properties-not-animatable.html, "transition-duration".
#[test]
fn wpt_transition_properties_not_animatable_duration() {
    assert_not_animatable("transition-duration", "1s", "2s", "0s");
}

/// transition-properties-not-animatable.html, "transition-delay".
#[test]
fn wpt_transition_properties_not_animatable_delay() {
    assert_not_animatable("transition-delay", "1s", "2s", "0s");
}

/// transition-properties-not-animatable.html, "transition-property".
#[test]
fn wpt_transition_properties_not_animatable_property() {
    assert_not_animatable("transition-property", "color", "background-color", "all");
}
