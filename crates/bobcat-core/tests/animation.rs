#![cfg(feature = "quickjs")]

//! The `@keyframes` timeline, end to end: a host-decoded keyframes rule
//! reaches Stylo, the presenting side advances it against the host's clock,
//! and the committed frame moves.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::resource::ResourceFetcher;
use bobcat_core::{
    AnimationClock, EngineEvent, LynxView, ManualClock, NoWindow, OffscreenLynxView, PageConfig,
    PreparsedDeclaration, PreparsedKeyframe, PreparsedRule, PreparsedStyleSheet, ScriptRunError,
    quickjs_engine_factory,
};
use support::FetcherDouble;

const SCRIPT_URL: &str = "app:///main.js";
const STYLE_URL: &str = "app:///author.css";

/// One 8x8 red square, animated by an author `@keyframes` rule.
const SLIDER_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  const slider = __CreateView(0);
  __SetClasses(slider, 'slider');
  __AppendElement(page, slider);
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

fn resources(sheet: Option<PreparsedStyleSheet>) -> Arc<dyn ResourceFetcher> {
    let mut double = FetcherDouble::new(SLIDER_SCRIPT.as_bytes().to_vec()).resolving_to(SCRIPT_URL);
    if let Some(sheet) = sheet {
        double = double.with_preparsed_style_sheet(sheet);
    }
    Arc::new(double)
}

/// A view built the ordinary way, so its timeline is whatever `new` chooses.
async fn booted(sheet: Option<PreparsedStyleSheet>) -> OffscreenLynxView {
    let view = OffscreenLynxView::new(
        PageConfig::default(),
        resources(sheet),
        quickjs_engine_factory(),
        Arc::new(|| {}),
        32.0,
        24.0,
        1.0,
    )
    .expect("view");
    run(view).await
}

/// A view whose timeline the test names and drives.
async fn booted_on<C: AnimationClock>(
    sheet: Option<PreparsedStyleSheet>,
    clock: C,
) -> OffscreenLynxView<C> {
    let view = LynxView::<'static, NoWindow, C>::with_animation_clock(
        PageConfig::default(),
        resources(sheet),
        quickjs_engine_factory(),
        Arc::new(|| {}),
        32.0,
        24.0,
        1.0,
        clock,
    )
    .expect("view");
    run(view).await
}

async fn run<C: AnimationClock>(mut view: OffscreenLynxView<C>) -> OffscreenLynxView<C> {
    view.load_style_sheet(STYLE_URL)
        .await
        .expect("the pre-parsed sheet mounts");
    view.attach_offscreen()
        .expect("GPU initialization for the offscreen target");
    view.execute_script(SCRIPT_URL)
        .await
        .expect("fetch and start script");
    wait_for_script(&mut view).await.expect("script execution");
    view
}

async fn wait_for_script<C: AnimationClock>(
    view: &mut OffscreenLynxView<C>,
) -> Result<(), ScriptRunError> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(result) = view.pump().into_iter().find_map(|event| match event {
            EngineEvent::ScriptFinished(result) => Some(result),
            _ => None,
        }) {
            return result;
        }
        assert!(Instant::now() < deadline, "script thread did not finish");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// The x of the leftmost red pixel in the committed frame.
fn red_left_edge<C: AnimationClock>(view: &mut OffscreenLynxView<C>) -> usize {
    let shot = view.capture().expect("capture the committed frame");
    let width = usize::try_from(shot.size.width).expect("the frame is addressable");
    shot.pixels
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, pixel)| *pixel == [255, 0, 0, 255])
        .map(|(index, _)| index % width)
        .min()
        .expect("the animated square is painted")
}

#[tokio::test]
async fn a_keyframes_animation_moves_the_committed_frame_on_the_hosts_clock() {
    let clock = Arc::new(ManualClock::new());
    let mut view = booted_on(Some(slider_sheet()), clock.clone()).await;

    clock.set(0.0);
    view.tick(true).expect("render the first frame");
    let start = red_left_edge(&mut view);

    clock.set(0.5);
    view.tick(true).expect("render the half-way frame");
    let middle = red_left_edge(&mut view);

    assert_eq!(start, 0, "at t=0 the square sits at its `from` keyframe");
    assert_eq!(
        middle, 8,
        "half a second into a linear 16px slide, the square has moved 8px"
    );
    assert!(
        view.is_animating(),
        "an infinite animation keeps asking for frames"
    );
}

/// The point of the whole exercise: the frame moved without the script thread
/// being asked for anything. `execute_script` has already finished, so the
/// realm is parked on its command channel for the whole sequence above.
#[tokio::test]
async fn animation_frames_need_no_script_thread_work() {
    let clock = Arc::new(ManualClock::new());
    let mut view = booted_on(Some(slider_sheet()), clock.clone()).await;

    clock.set(0.0);
    view.tick(true).expect("first frame");
    let mut edges = vec![red_left_edge(&mut view)];
    for step in 1..=3 {
        clock.set(f64::from(step) * 0.25);
        view.tick(true).expect("animated frame");
        edges.push(red_left_edge(&mut view));
    }

    assert_eq!(
        edges,
        vec![0, 4, 8, 12],
        "each frame advanced the animation by a quarter of its 16px travel"
    );
    assert!(
        view.pump().is_empty(),
        "and none of it produced an engine event, so nothing crossed to script"
    );
}

/// A host that arranges nothing still gets a running animation: the view's
/// default timeline is the platform's monotonic clock. Installing one is for
/// hosts with a better reading of the frame's time, or for tests that want a
/// reproducible sequence.
#[tokio::test]
async fn a_view_animates_without_the_host_installing_a_clock() {
    let mut view = booted(Some(slider_sheet())).await;

    view.tick(true).expect("first frame");
    let first = red_left_edge(&mut view);
    assert!(
        view.is_animating(),
        "the default clock keeps the animation asking for frames"
    );

    // A quarter of the 1s travel is 4px. The square is *not* asserted to have
    // moved right: the animation is infinite, boot takes an unknown slice of
    // the first second, and a sample near 16px wraps back to 0. Only that it
    // moved at all, which no phase can fake at a quarter of a period.
    tokio::time::sleep(Duration::from_millis(250)).await;
    view.tick(true).expect("later frame");
    let later = red_left_edge(&mut view);

    assert_ne!(
        later, first,
        "the animation advances on its own, with the host naming no clock at all"
    );
}

/// Naming a clock replaces the default rather than enabling animation.
#[tokio::test]
async fn a_named_clock_replaces_the_default_timeline() {
    let clock = Arc::new(ManualClock::new());
    let mut view = booted_on(Some(slider_sheet()), clock.clone()).await;

    clock.set(0.5);
    view.tick(true).expect("frame on the manual clock");
    let held = red_left_edge(&mut view);

    // Real time passes; the manual clock does not, so neither does the frame.
    tokio::time::sleep(Duration::from_millis(250)).await;
    view.tick(true).expect("another frame on the same instant");

    assert_eq!(held, 8, "the manual clock decides where the square is");
    assert_eq!(
        red_left_edge(&mut view),
        held,
        "and wall time cannot move it behind the host's back"
    );
}
