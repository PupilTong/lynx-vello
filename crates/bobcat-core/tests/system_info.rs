//! What `SystemInfo` reports on both threads: the screen the embedder named,
//! whether measured or — for a host with no screen — its own capture size in
//! physical pixels, which it names explicitly.
//!
//! Asserted over a real group, a real BTS Worker and a real fetcher, because
//! the three numbers take two steps: they are written into the MTS boot
//! module, and the MTS realm posts its own `SystemInfo` to the BTS Worker in
//! the `initialize` message, where `bobcat:bts-runtime` takes it before it
//! imports the BTS entry.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::resource::{
    LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase, ResourceFetcher,
    RetryAdvice, SourceCompletion, SourceRequest,
};
use bobcat_core::{
    DrawTarget, EngineEvent, LynxGroup, NoWakeup, Painter, ScreenMetrics, StyleThreads, ViewSources,
};

const MAIN_URL: &str = "app:///main.js";
const BACKGROUND_URL: &str = "app:///background.js";

/// The create-time viewport both views below are built at, in CSS pixels.
const VIEW_WIDTH: f32 = 32.0;
const VIEW_HEIGHT: f32 = 24.0;

/// The MTS entry: it prints the three numbers and then renders one element,
/// so boot finishes. `SystemInfo` is one of the bindings the entry preamble
/// gives it.
const MAIN_ENTRY: &str = r"
console.log('mts ' + SystemInfo.pixelRatio + ' ' + SystemInfo.pixelWidth + ' ' +
  SystemInfo.pixelHeight);
globalThis.renderPage = function () {
  __AppendElement(__CreatePage('card', 0), __CreateView(0));
};
";

/// The BTS entry: the same three numbers, which `bobcat:bts-runtime` took
/// from the `initialize` message before this entry was imported.
const BACKGROUND_ENTRY: &str = r"
import { console, SystemInfo } from 'bobcat:bts-runtime';
console.log('bts ' + SystemInfo.pixelRatio + ' ' + SystemInfo.pixelWidth + ' ' +
  SystemInfo.pixelHeight);
";

/// Serves this test's two entries out of memory.
struct Entries;

impl bobcat_core::FrameImages for Entries {
    fn read(
        &self,
        _source: &str,
        _hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::vello::peniko::ImageData> {
        None
    }

    fn retain(&self, _frame: &[Arc<str>]) {}
}

impl ResourceFetcher for Entries {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        let specifier = match &request {
            SourceRequest::Module(url)
            | SourceRequest::StyleSheet(url)
            | SourceRequest::Font { url }
            | SourceRequest::Fetch { url } => url.clone(),
        };
        let source = match specifier.as_str() {
            MAIN_URL => Some(MAIN_ENTRY),
            BACKGROUND_URL => Some(BACKGROUND_ENTRY),
            _ => None,
        };
        completion.complete(source.map_or_else(
            || {
                Err(ResourceError {
                    kind: ResourceErrorKind::NotFound,
                    phase: ResourceErrorPhase::Resolve,
                    locator: Some(Arc::from(specifier.as_str())),
                    message: "this host serves two entries".into(),
                    retry: RetryAdvice::Never,
                }
                .into())
            },
            |source| {
                Ok(LoadedSource::Module {
                    source: source.to_owned(),
                    url: specifier.clone(),
                })
            },
        ));
    }
}

/// Boots one view at `device_pixel_ratio` reporting `screen`, over the
/// entry `main` and the BTS entry `background` named against `base`, and
/// returns what its two realms printed.
///
/// The painter is not optional: both lines are pumped out past boot's flush,
/// and an unbound view never gets there.
async fn printed(
    device_pixel_ratio: f32,
    screen: ScreenMetrics,
    base: &str,
    main: &str,
    background: &str,
) -> (String, String) {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let mut sources = ViewSources::new(base, main, screen);
    sources.background_entry = Some(background.to_owned());
    let mut view = group
        .create_lynx_view(
            VIEW_WIDTH,
            VIEW_HEIGHT,
            device_pixel_ratio,
            |_reports| Entries,
            Vec::new(),
            sources,
        )
        .expect("the view is built");
    let mut painter = Painter::new(
        DrawTarget::Offscreen,
        VIEW_WIDTH,
        VIEW_HEIGHT,
        device_pixel_ratio,
    )
    .await
    .expect("the painter is built");
    painter.attach(&view).expect("a fresh view takes a painter");

    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut main_thread, mut background) = (None, None);
    while main_thread.is_none() || background.is_none() {
        for event in view.pump() {
            match event {
                EngineEvent::ConsoleMessage { message, .. } => {
                    if let Some(rest) = message.strip_prefix("mts ") {
                        main_thread = Some(rest.to_owned());
                    } else if let Some(rest) = message.strip_prefix("bts ") {
                        background = Some(rest.to_owned());
                    }
                }
                EngineEvent::StartupFailed(error) => panic!("boot failed: {error}"),
                EngineEvent::WorkerThrew { error, .. }
                | EngineEvent::WorkerEnded { error, .. }
                | EngineEvent::ScriptRunError(error)
                | EngineEvent::Panicked(error) => {
                    panic!("the realm failed: {}", error.message)
                }
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "both realms printed SystemInfo");
        std::thread::sleep(Duration::from_millis(1));
    }
    (
        main_thread.expect("the MTS entry printed"),
        background.expect("the BTS entry printed"),
    )
}

#[tokio::test]
async fn both_realms_report_the_screen_the_embedder_measured() {
    // A phone's screen, and a view a fraction of its size at another scale:
    // nothing in the answer may come from the viewport.
    let (main_thread, background) = printed(
        1.0,
        ScreenMetrics {
            pixel_ratio: 3.0,
            pixel_width: 1170.0,
            pixel_height: 2532.0,
        },
        "app:///",
        MAIN_URL,
        BACKGROUND_URL,
    )
    .await;
    assert_eq!(main_thread, "3 1170 2532");
    assert_eq!(background, "3 1170 2532");
}

/// A ratio an `f32` cannot hold exactly reads the same in both realms: the
/// MTS boot module is written with the number's shortest decimal, and the
/// BTS is posted the MTS realm's `SystemInfo`, so it reports the number that
/// decimal reads as, not the `f32`'s own value (`1.100000023841858`).
#[tokio::test]
async fn both_realms_report_a_ratio_an_f32_cannot_hold_as_the_same_number() {
    let (main_thread, background) = printed(
        1.0,
        ScreenMetrics {
            pixel_ratio: 1.1,
            pixel_width: 1287.0,
            pixel_height: 2785.0,
        },
        "app:///",
        MAIN_URL,
        BACKGROUND_URL,
    )
    .await;
    assert_eq!(main_thread, "1.1 1287 2785");
    assert_eq!(background, "1.1 1287 2785");
}

#[tokio::test]
async fn a_host_with_no_screen_reports_its_viewport_in_physical_pixels() {
    let (main_thread, background) = printed(
        2.0,
        ScreenMetrics::for_viewport(VIEW_WIDTH, VIEW_HEIGHT, 2.0),
        "app:///",
        MAIN_URL,
        BACKGROUND_URL,
    )
    .await;
    assert_eq!(main_thread, "2 64 48");
    assert_eq!(background, "2 64 48");
}

/// The view resolves both entries against its base before either is
/// requested. [`Entries`] answers only `app:///main.js` and
/// `app:///background.js`, so both realms printing is what shows that
/// neither request reached the fetcher as the relative string.
#[tokio::test]
async fn relative_entries_resolve_against_the_view_base() {
    let (main_thread, background) = printed(
        1.0,
        ScreenMetrics::for_viewport(VIEW_WIDTH, VIEW_HEIGHT, 1.0),
        "app:///",
        "main.js",
        "./background.js",
    )
    .await;
    assert_eq!(main_thread, "1 32 24");
    assert_eq!(background, "1 32 24");
}
