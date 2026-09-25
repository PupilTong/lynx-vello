//! Painting-side `@keyframes` timeline tests, end to end: a decoded rule
//! reaches Stylo, the painting side advances it against the engine's own
//! clock, and the committed frame moves.
//!
//! Inside the crate rather than in `tests/`, because the timeline is not an
//! embedder-facing thing to drive: pinning it is [`FrameClock::pin`], which
//! exists only in a test build. The integration suite covers what a host can
//! actually observe — that animations run with the host arranging nothing.

use std::time::Duration;

use crate::style::{PreparsedDeclaration, PreparsedKeyframe, PreparsedRule, PreparsedStyleSheet};
use crate::test_support::{TestEngine, TestViewSpec};

/// One 8x8 red square, animated by an author `@keyframes` rule.
const SLIDER_SCRIPT: &str = r"
    globalThis.renderPage = function renderPage() {
      const page = __CreatePage('card', 0);
      const slider = __CreateView(0);
      __SetClasses(slider, 'slider');
      __AppendElement(page, slider);
      __FlushElementTree();
    };
";

fn declaration(property: &str, value: &str) -> PreparsedDeclaration {
    PreparsedDeclaration {
        property: property.to_owned(),
        value: value.to_owned(),
        important: false,
    }
}

/// `.slider` plus a `slide` keyframes rule that translates it 16px right over
/// one second, linearly.
fn slider_sheet() -> PreparsedStyleSheet {
    PreparsedStyleSheet {
        rules: vec![
            PreparsedRule::Keyframes {
                name: "slide".to_owned(),
                keyframes: vec![
                    PreparsedKeyframe {
                        selector: "from".to_owned(),
                        declarations: vec![declaration("transform", "translateX(0px)")],
                    },
                    PreparsedKeyframe {
                        selector: "to".to_owned(),
                        declarations: vec![declaration("transform", "translateX(16px)")],
                    },
                ],
            },
            PreparsedRule::Style {
                selectors: ".slider".to_owned(),
                declarations: vec![
                    declaration("width", "8px"),
                    declaration("height", "8px"),
                    declaration("background-color", "#ff0000"),
                    declaration("animation", "slide 1s linear infinite"),
                ],
            },
        ],
    }
}

/// A 32x24 offscreen view with the sheet mounted and the page built.
fn booted() -> TestEngine {
    TestViewSpec::new(SLIDER_SCRIPT)
        .with_preparsed_style_sheet(slider_sheet())
        .offscreen(32.0, 24.0)
        .boot()
}

/// The same square, inside a box that skips its contents.
const SKIPPED_SLIDER_SCRIPT: &str = r"
    globalThis.renderPage = function renderPage() {
      const page = __CreatePage('card', 0);
      const skipper = __CreateView(0);
      __SetClasses(skipper, 'skipper');
      const slider = __CreateView(0);
      __SetClasses(slider, 'slider');
      __AppendElement(skipper, slider);
      __AppendElement(page, skipper);
      __FlushElementTree();
    };
";

/// [`slider_sheet`] plus the `content-visibility: hidden` box that wraps it.
fn skipping_sheet() -> PreparsedStyleSheet {
    let mut sheet = slider_sheet();
    sheet.rules.push(PreparsedRule::Style {
        selectors: ".skipper".to_owned(),
        declarations: vec![
            declaration("width", "16px"),
            declaration("height", "16px"),
            declaration("content-visibility", "hidden"),
        ],
    });
    sheet
}

/// The x of the leftmost red pixel in the committed frame.
fn red_left_edge(engine: &mut TestEngine) -> usize {
    let shot = engine.capture().expect("capture the committed frame");
    let width = usize::try_from(shot.size.width).expect("the frame is addressable");
    shot.pixels
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, pixel)| *pixel == [255, 0, 0, 255])
        .map(|(index, _)| index % width)
        .min()
        .expect("the animated square is painted")
}

#[test]
fn a_keyframes_animation_moves_the_committed_frame_on_the_frame_clock() {
    let mut engine = booted();

    engine.painter.clock.pin(0.0);
    engine.tick(true).expect("render the first frame");
    let start = red_left_edge(&mut engine);

    engine.painter.clock.pin(0.5);
    engine.tick(true).expect("render the half-way frame");
    let middle = red_left_edge(&mut engine);

    assert_eq!(start, 0, "at t=0 the square sits at its `from` keyframe");
    assert_eq!(
        middle, 8,
        "half a second into a linear 16px slide, the square has moved 8px"
    );
    assert!(
        engine.is_animating(),
        "an infinite animation keeps asking for frames"
    );
}

/// The point of the whole exercise: the frame moved without the script thread
/// being asked for anything. The entry module has already finished, so the
/// realm is parked on its command channel for the whole sequence above.
#[test]
fn animation_frames_need_no_script_thread_work() {
    let mut engine = booted();

    engine.painter.clock.pin(0.0);
    engine.tick(true).expect("first frame");
    let mut edges = vec![red_left_edge(&mut engine)];
    for step in 1..=3 {
        engine.painter.clock.pin(f64::from(step) * 0.25);
        engine.tick(true).expect("animated frame");
        edges.push(red_left_edge(&mut engine));
    }

    assert_eq!(
        edges,
        vec![0, 4, 8, 12],
        "each frame advanced the animation by a quarter of its 16px travel"
    );
    assert!(
        engine.pump().is_empty(),
        "and none of it produced an engine event, so nothing crossed to script"
    );
}

/// Wall time cannot move a pinned frame: the engine reads its clock once per
/// frame and every animation in that frame lands on the one reading.
#[test]
fn one_reading_places_every_animation_in_a_frame() {
    let mut engine = booted();

    // The boot flush created the animation; the first frame is what starts it,
    // so taking that frame at zero puts the animation's origin on the clock's.
    engine.painter.clock.pin(0.0);
    engine
        .tick(true)
        .expect("the frame that starts the animation");

    engine.painter.clock.pin(0.5);
    engine.tick(true).expect("frame on the pinned instant");
    let held = red_left_edge(&mut engine);

    std::thread::sleep(Duration::from_millis(250));
    engine
        .tick(true)
        .expect("another frame on the same instant");

    assert_eq!(
        held, 8,
        "the frame's own reading decides where the square is"
    );
    assert_eq!(
        red_left_edge(&mut engine),
        held,
        "and time passing between frames cannot move it inside one"
    );
}

/// css-contain-2 §4: an animation whose element is in a skipped subtree does
/// not advance, so it is not something the engine owes a frame for. The same
/// infinite animation that keeps
/// [`a_keyframes_animation_moves_the_committed_frame_on_the_frame_clock`]
/// asking forever leaves the host idle from here, which is what a host reads
/// off `Painter::is_animating` (and, on a window, `owes_frame`).
#[test]
fn an_animation_inside_a_skipped_subtree_leaves_the_painter_idle() {
    let mut engine = TestViewSpec::new(SKIPPED_SLIDER_SCRIPT)
        .with_preparsed_style_sheet(skipping_sheet())
        .offscreen(32.0, 24.0)
        .boot();

    engine.painter.clock.pin(0.0);
    engine.tick(true).expect("render the first frame");

    assert!(
        !engine
            .published_frame()
            .expect("boot committed a frame")
            .animations_active(),
        "the commit reports the timeline idle: its only animation is frozen"
    );
    assert!(
        !engine.is_animating(),
        "so the host is never asked for the next frame"
    );
    assert!(
        !engine.painter.owes_frame(),
        "and neither is a windowed one"
    );
}

/// A red header driven by a `scroll-timeline: --list` below it, in a 64x64
/// view: the header slides `slide` px right over the list's `max` px scroll
/// range, and the list holds `list_css`.
fn booted_scroll_driven(slide: u32, max: u32, header_css: &str, list_css: &str) -> TestEngine {
    let script = r"
        globalThis.renderPage = function renderPage() {
          const page = __CreatePage('card', 0);
          const header = __CreateView(0);
          const list = __CreateView(0);
          const filler = __CreateView(0);
          __SetClasses(header, 'header');
          __SetClasses(list, 'list');
          __SetClasses(filler, 'filler');
          __AppendElement(page, header);
          __AppendElement(page, list);
          __AppendElement(list, filler);
          globalThis.held = [page, header, list, filler];
          __FlushElementTree();
        };
    ";
    let filler = 48 + max;
    TestViewSpec::new(script)
        .with_style_sheet(&format!(
            "@keyframes slide {{ from {{ transform: translateX(0px); }}
                                to {{ transform: translateX({slide}px); }} }}
             .header {{ position: absolute; left: 0px; top: 0px; width: 8px; height: 8px;
                        background-color: #ff0000; {header_css} animation-timeline: --list; }}
             .list {{ position: absolute; left: 0px; top: 16px; width: 64px; height: 48px;
                      display: flex; flex-direction: column; overflow: scroll;
                      scroll-timeline: --list; {list_css} }}
             .filler {{ flex-shrink: 0; width: 64px; height: {filler}px; }}"
        ))
        .offscreen(64.0, 64.0)
        .boot()
}

/// One touch on the list at `y`, at the pinned instant `at`.
fn touch_list(engine: &mut TestEngine, at: f64, phase: dom::input::PointerPhase, y: f32) {
    engine.painter.clock.pin(at);
    engine.dispatch_input(dom::input::InputEvent::pointer(
        dom::Point2D::new(32.0, y),
        1,
        dom::input::PointerKind::Touch,
        phase,
    ));
}

fn list_intent(engine: &TestEngine) -> f32 {
    let frame = engine.painter.frame().expect("a frame is held");
    let list = frame.scroll_slots()[0].node;
    engine
        .painter
        .scroll_intents
        .offset_for(list)
        .expect("the list scrolled")
        .y
}

/// scroll-animations-1 on the painter: dragging the list moves the header
/// it drives, pixel for pixel, with no commit and no frame post — the
/// exported curve samples the list's offset at compose time — and at rest
/// nothing is owed.
#[test]
fn a_drag_moves_a_scroll_driven_header_with_no_commit_and_no_frame_post() {
    use dom::input::PointerPhase;

    let mut engine = booted_scroll_driven(200, 200, "animation: slide linear both;", "");
    let boot = engine.published_frame().expect("boot committed a frame");
    assert_eq!(boot.animation_slots().len(), 1, "the header exports");
    assert!(!boot.has_live_curves() && !boot.needs_main_ticks() && !boot.animations_active());
    engine.painter.clock.pin(0.0);
    assert_eq!(red_left_edge(&mut engine), 0);

    // Past the 8px drag slop, then inside half the 48px encode window.
    touch_list(&mut engine, 0.0, PointerPhase::Down, 60.0);
    touch_list(&mut engine, 0.05, PointerPhase::Move, 48.0);
    assert!((list_intent(&engine) - 4.0).abs() < 0.5);
    assert_eq!(red_left_edge(&mut engine), 4, "the header follows the drag");
    touch_list(&mut engine, 0.1, PointerPhase::Move, 40.0);
    assert_eq!(red_left_edge(&mut engine), 12);
    // Held still, then let go: no fling.
    touch_list(&mut engine, 0.5, PointerPhase::Up, 40.0);
    assert!(!engine.is_animating(), "at rest nothing is owed");
    // Past the curve's nominal one-second duration, which it never ends at.
    assert!(
        engine.painter.begin_frame(2.0, false).is_none(),
        "no frame post"
    );
    let adopted = engine
        .probe_document(|tree| tree.scroll_offset(tree.document_element().id()))
        .is_some();
    assert!(adopted, "the view's task answers probes");
    assert_eq!(
        engine
            .published_frame()
            .expect("still published")
            .commit_id(),
        boot.commit_id(),
        "the adopted scroll commits nothing"
    );
    assert_eq!(red_left_edge(&mut engine), 12);
}

/// A fling keeps the header following the list frame by frame after the
/// finger lifts, to where the fling comes to rest.
#[test]
fn a_fling_keeps_a_scroll_driven_header_following() {
    use dom::input::PointerPhase;

    let mut engine = booted_scroll_driven(40, 400, "animation: slide linear both;", "");
    engine.painter.clock.pin(0.0);
    touch_list(&mut engine, 0.0, PointerPhase::Down, 60.0);
    touch_list(&mut engine, 0.01, PointerPhase::Move, 45.0);
    touch_list(&mut engine, 0.02, PointerPhase::Move, 30.0);
    touch_list(&mut engine, 0.02, PointerPhase::Up, 30.0);
    assert!(engine.is_animating(), "the fling owes frames");
    let mut edges = Vec::new();
    for at in [0.05, 0.1, 0.2, 0.4, 1.0, 4.0] {
        engine.painter.clock.pin(at);
        engine.painter.service_gesture_clock(at);
        let edge = red_left_edge(&mut engine);
        // A tenth of the offset, anti-aliased on its left column.
        let expected = list_intent(&engine) / 10.0;
        assert!(
            (f32::from(u8::try_from(edge).expect("inside the view")) - expected).abs() <= 1.0,
            "at {at}: edge {edge}, offset {}",
            list_intent(&engine)
        );
        edges.push(edge);
    }
    assert!(edges.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(edges[0] < edges[edges.len() - 1], "the header moved");
    assert!(!engine.is_animating(), "and rests with the fling");
}

/// A `contain-bounce` stretch past the list's end holds a header with no
/// fill at its last keyframe. The frame composed may be the one main
/// commits at the end, where a missing clamp shows the same pixels, so the
/// clamp itself is pinned by dom's
/// `a_scroll_self_timeline_binds_the_scrollers_own_slot`.
#[test]
fn a_bounce_stretch_holds_a_scroll_driven_header_at_the_edge() {
    use dom::input::PointerPhase;

    let mut engine = booted_scroll_driven(
        16,
        32,
        "animation: slide linear;",
        "overscroll-behavior: contain-bounce;",
    );
    engine.painter.clock.pin(0.0);
    engine.dispatch_input(dom::input::InputEvent::wheel(
        dom::Point2D::new(32.0, 40.0),
        dom::Vector2D::new(0.0, 32.0),
    ));
    assert_eq!(red_left_edge(&mut engine), 16, "at the end");
    touch_list(&mut engine, 0.0, PointerPhase::Down, 60.0);
    touch_list(&mut engine, 0.05, PointerPhase::Move, 20.0);
    assert!(list_intent(&engine) > 32.0, "stretched past the end");
    assert_eq!(
        red_left_edge(&mut engine),
        16,
        "the stretch holds the end's value"
    );
}

/// `scroll(self)` on a scroller that animates by its own offset: its box
/// slides as its content scrolls, while the drag that scrolls it stays
/// latched on it.
#[test]
fn a_scroll_self_timeline_moves_its_scroller_with_its_own_offset() {
    use dom::input::PointerPhase;

    let script = r"
        globalThis.renderPage = function renderPage() {
          const page = __CreatePage('card', 0);
          const list = __CreateView(0);
          const filler = __CreateView(0);
          __SetClasses(list, 'list');
          __SetClasses(filler, 'filler');
          __AppendElement(page, list);
          __AppendElement(list, filler);
          globalThis.held = [page, list, filler];
          __FlushElementTree();
        };
    ";
    let mut engine = TestViewSpec::new(script)
        .with_style_sheet(
            "@keyframes slide { from { transform: translateX(0px); }
                                to { transform: translateX(200px); } }
             .list { position: absolute; left: 0px; top: 16px; width: 16px; height: 48px;
                     display: flex; flex-direction: column; overflow: scroll;
                     background-color: #ff0000;
                     animation: slide linear both; animation-timeline: scroll(self); }
             .filler { flex-shrink: 0; width: 16px; height: 248px; }",
        )
        .offscreen(64.0, 64.0)
        .boot();
    engine.painter.clock.pin(0.0);
    assert_eq!(red_left_edge(&mut engine), 0);
    engine.painter.clock.pin(0.0);
    engine.dispatch_input(dom::input::InputEvent::pointer(
        dom::Point2D::new(8.0, 60.0),
        1,
        dom::input::PointerKind::Touch,
        PointerPhase::Down,
    ));
    engine.painter.clock.pin(0.05);
    engine.dispatch_input(dom::input::InputEvent::pointer(
        dom::Point2D::new(8.0, 40.0),
        1,
        dom::input::PointerKind::Touch,
        PointerPhase::Move,
    ));
    assert!((list_intent(&engine) - 12.0).abs() < 0.5);
    assert_eq!(
        red_left_edge(&mut engine),
        12,
        "the scroller slides by its offset"
    );
}
