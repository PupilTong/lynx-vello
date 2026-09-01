//! The `@keyframes` timeline from where an embedder stands: a host arranges a
//! stylesheet and a script and nothing else, and the committed frame moves on
//! its own.
//!
//! The timeline itself is engine-owned — there is no clock to name, install,
//! or step from out here. The deterministic sample-by-sample coverage lives in
//! `bobcat_core::paint::animation_tests`, inside the crate, where the frame
//! clock can be pinned.

mod support;

use std::sync::Arc;
use std::time::Duration;

use bobcat_core::{
    DrawTarget, LynxView, NoWakeup, PageConfig, PreparsedDeclaration, PreparsedKeyframe,
    PreparsedRule, PreparsedStyleSheet, ViewSources,
};
use support::{FetcherDouble, wait_for_script};

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

fn resources(sheet: PreparsedStyleSheet) -> FetcherDouble {
    FetcherDouble::new(SLIDER_SCRIPT.as_bytes().to_vec())
        .resolving_to(SCRIPT_URL)
        .with_preparsed_style_sheet(sheet)
}

/// A view built the only way there is to build one: its sheet and its entry
/// module are its construction inputs.
async fn booted() -> LynxView {
    let fetcher = Arc::new(resources(slider_sheet()));
    let mut view = LynxView::new(
        PageConfig::default(),
        Arc::new(NoWakeup),
        32.0,
        24.0,
        1.0,
        DrawTarget::Offscreen,
        ViewSources {
            style_sheets: vec![STYLE_URL.to_owned()],
            ..ViewSources::new(support::factory(fetcher), SCRIPT_URL)
        },
    )
    .await
    .expect("view");
    wait_for_script(&mut view).expect("script execution");
    view
}

/// The x of the leftmost red pixel in the committed frame.
fn red_left_edge(view: &mut LynxView) -> usize {
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
    // The view answers this from the painter it owns, over the frame that
    // tick just read, so there is no reply to be ahead of.
    assert!(
        view.is_animating(),
        "the engine's clock keeps the animation asking for frames"
    );
    let first = red_left_edge(&mut view);

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

/// The slider's `translateX` keyframes export as a curve, so the square
/// moves between two captures with no tick in between — no `BeginFrame`,
/// no restyle, no commit: the same committed frame composed at two clock
/// readings.
#[tokio::test]
async fn an_exported_curve_moves_pixels_between_commits() {
    let mut view = booted().await;
    // The synchronizing tick promotes the pending animation to running,
    // which is when the commit exports its curve.
    view.tick(true).expect("first frame");

    let first = red_left_edge(&mut view);
    tokio::time::sleep(Duration::from_millis(250)).await;
    let later = red_left_edge(&mut view);

    assert_ne!(
        later, first,
        "an exported curve animates in composition, between commits"
    );
}
