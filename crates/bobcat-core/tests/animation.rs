//! The `@keyframes` timeline from where an embedder stands: a host arranges a
//! stylesheet and a script and nothing else, and the committed frame moves on
//! its own.
//!
//! The timeline itself is engine-owned — there is no clock to name, install,
//! or step from out here. The deterministic sample-by-sample coverage lives in
//! `bobcat_core::engine::animation_tests`, inside the crate, where the frame
//! clock can be pinned.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::resource::ResourceFetcher;
use bobcat_core::{
    EngineEvent, OffscreenLynxView, PageConfig, PreparsedDeclaration, PreparsedKeyframe,
    PreparsedRule, PreparsedStyleSheet, ScriptRunError,
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

fn resources(sheet: PreparsedStyleSheet) -> Arc<dyn ResourceFetcher> {
    Arc::new(
        FetcherDouble::new(SLIDER_SCRIPT.as_bytes().to_vec())
            .resolving_to(SCRIPT_URL)
            .with_preparsed_style_sheet(sheet),
    )
}

/// A view built the only way there is to build one.
async fn booted() -> OffscreenLynxView {
    let mut view = OffscreenLynxView::new(
        PageConfig::default(),
        resources(slider_sheet()),
        Arc::new(|| {}),
        32.0,
        24.0,
        1.0,
    )
    .expect("view");
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

async fn wait_for_script(view: &mut OffscreenLynxView) -> Result<(), ScriptRunError> {
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
fn red_left_edge(view: &mut OffscreenLynxView) -> usize {
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

/// A host that arranges nothing still gets a running animation: the view's
/// timeline is the engine's, and no embedder API touches it.
#[tokio::test]
async fn a_view_animates_with_the_host_arranging_no_timeline() {
    let mut view = booted().await;

    view.tick(true).expect("first frame");
    let first = red_left_edge(&mut view);
    assert!(
        view.is_animating(),
        "the engine's clock keeps the animation asking for frames"
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
        "the animation advances on real time, with the host naming nothing"
    );
}
