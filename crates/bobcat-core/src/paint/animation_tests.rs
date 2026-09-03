//! Painting-side `@keyframes` timeline tests, end to end: a decoded rule
//! reaches Stylo, the painting side advances it against the engine's own
//! clock, and the committed frame moves.
//!
//! Inside the crate rather than in `tests/`, because the timeline is not an
//! embedder-facing thing to drive: pinning it is [`FrameClock::pin`], which
//! exists only in a test build. The integration suite covers what a host can
//! actually observe — that animations run with the host arranging nothing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{EntryModule, TestPainter};
use crate::style::{PreparsedDeclaration, PreparsedKeyframe, PreparsedRule, PreparsedStyleSheet};

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

/// A 32x24 offscreen engine with the sheet mounted and the page built.
fn booted() -> TestPainter {
    let viewport = crate::main::tree::Viewport::new(32.0, 24.0);
    let mut engine = TestPainter::start(
        move || {
            let mut document =
                crate::main::tree::new_document(viewport, crate::main::tree::PageConfig::default());
            crate::style::add_preparsed_style_sheet(&mut document, &slider_sheet());
            document
        },
        viewport,
        crate::view::FrameSize::for_viewport(32.0, 24.0, 1.0).expect("a bounded target"),
        Arc::new(super::NoWakeup),
        EntryModule {
            source: SLIDER_SCRIPT.to_owned(),
            url: "app:///main.js".to_owned(),
        },
        super::Output::offscreen().expect("offscreen GPU target"),
    )
    .expect("view");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for event in engine.pump() {
            match event {
                crate::EngineEvent::ScriptFinished => return engine,
                crate::EngineEvent::ScriptRunError(error) => {
                    panic!("the entry module failed: {error}")
                }
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "the entry module did not finish");
        std::thread::yield_now();
    }
}

/// The x of the leftmost red pixel in the committed frame.
fn red_left_edge(engine: &mut TestPainter) -> usize {
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

    engine.clock.pin(0.0);
    engine.tick(true).expect("render the first frame");
    let start = red_left_edge(&mut engine);

    engine.clock.pin(0.5);
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

    engine.clock.pin(0.0);
    engine.tick(true).expect("first frame");
    let mut edges = vec![red_left_edge(&mut engine)];
    for step in 1..=3 {
        engine.clock.pin(f64::from(step) * 0.25);
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

    engine.clock.pin(0.5);
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
