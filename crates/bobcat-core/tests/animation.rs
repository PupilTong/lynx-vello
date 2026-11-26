//! The `@keyframes` timeline from where an embedder stands: a host arranges a
//! stylesheet and a script and nothing else, and the committed frame moves on
//! its own.
//!
//! The timeline itself is engine-owned — there is no clock to name, install,
//! or step from out here. The deterministic sample-by-sample coverage lives in
//! `bobcat_core::paint::animation_tests`, inside the crate, where the frame
//! clock can be pinned.

mod support;

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use bobcat_core::{
    DrawTarget, LynxView, NoWakeup, Painter, PreparsedDeclaration, PreparsedKeyframe,
    PreparsedRule, PreparsedStyleSheet, ViewSources,
};
use support::{FetcherDouble, solo_view, tick, wait_for_script};

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const SCRIPT_URL: &str = "app:///main.js";
const STYLE_URL: &str = "app:///author.css";

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
        property: property.to_owned().into(),
        value: value.to_owned(),
        important: false,
    }
}

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
    FetcherDouble::card(SLIDER_SCRIPT)
        .resolving_to(SCRIPT_URL)
        .with_preparsed_style_sheet(sheet)
}

async fn booted() -> (LynxView<Rc<FetcherDouble>>, Painter) {
    let fetcher = Rc::new(resources(slider_sheet()));
    let (mut view, painter) = solo_view(
        Arc::new(NoWakeup),
        32.0,
        24.0,
        1.0,
        DrawTarget::Offscreen,
        |_reports| fetcher,
        ViewSources {
            style_sheets: vec![STYLE_URL.to_owned()],
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US")
        },
    )
    .await
    .expect("view");
    wait_for_script(&mut view).expect("script execution");
    (view, painter)
}

fn red_left_edge(painter: &mut Painter) -> usize {
    let shot = painter.capture().expect("capture the committed frame");
    let width = usize::try_from(shot.size.width).expect("the frame is addressable");
    shot.pixels
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|&(_, &pixel)| pixel == [255, 0, 0, 255])
        .map(|(index, _)| index % width)
        .min()
        .expect("the animated square is painted")
}

#[tokio::test]
async fn a_view_animates_with_the_host_arranging_no_timeline() {
    let (_view, mut painter) = booted().await;

    tick(&mut painter, true).expect("first frame");
    assert!(
        painter.is_animating(),
        "the engine's clock keeps the animation asking for frames"
    );
    let first = red_left_edge(&mut painter);

    tokio::time::sleep(Duration::from_millis(250)).await;
    tick(&mut painter, true).expect("later frame");
    let later = red_left_edge(&mut painter);

    assert_ne!(
        later, first,
        "the animation advances on real time, with the host naming nothing"
    );
}

#[tokio::test]
async fn an_exported_curve_moves_pixels_between_commits() {
    let (_view, mut painter) = booted().await;
    tick(&mut painter, true).expect("first frame");

    let first = red_left_edge(&mut painter);
    tokio::time::sleep(Duration::from_millis(250)).await;
    let later = red_left_edge(&mut painter);

    assert_ne!(
        later, first,
        "an exported curve animates in composition, between commits"
    );
}
