//! `lynx.fetchBundle` end to end: a real group, a real view, a real BTS
//! Worker and a real fetcher, with both realms fetching a lazy container and
//! then loading its sections by URL.
//!
//! The fetcher double stands in for a host that installs containers: core's
//! request is a **plain fetch** — [`SourceRequest::Fetch`], which answers
//! [`LoadedSource::Fetched`] and nothing else — and it is the *fetcher* that
//! decides the bytes were a container, so the section URLs the realm derives
//! from that URL are ordinary module and stylesheet requests afterwards.
//! That is exactly the contract `bobcat-resources`' `ContainerInstaller`
//! implements. The double also answers a `fetch_probe`, which is how a
//! repeat fetch settles in the realm's own job.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bobcat_core::resource::{
    FetchOutcome, LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase,
    ResourceFetcher, RetryAdvice, SourceCompletion, SourceRequest, StyleSheetSource,
};
use bobcat_core::{
    DrawTarget, EngineEvent, LynxGroup, NoWakeup, Painter, StyleThreads, ViewSources,
};

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const MAIN_URL: &str = "app:///main.js";
const BACKGROUND_URL: &str = "app:///background.js";
const LAZY: &str = "app:///lazy.bundle";
const SLOW: &str = "app:///slow.bundle";

const MAIN_ENTRY: &str = r"
globalThis.renderPage = function () {
  __AppendElement(__CreatePage('card', 0), __CreateView(0));
};
const first = lynx.fetchBundle('app:///lazy.bundle', {});
const settled = first.wait(5);
console.log('mts wait ' + settled.code + ' ' + settled.url + ' ' + settled.error_msg.length);
const body = lynx.loadScript('main-thread', {bundleName: settled.url});
console.log('mts section ' + body('app:///lazy.bundle'));
const sheet = __LoadStyleSheet('CSS', settled.url);
__AdoptStyleSheet(sheet);
console.log('mts adopted ' + sheet.url);

const again = lynx.fetchBundle('app:///lazy.bundle', {});
again.then(function (info) { console.log('mts inline ' + info.code); });
console.log('mts after then');

const failed = lynx.fetchBundle('app:///missing.bundle', {}).wait(5);
console.log('mts failed ' + failed.code + ' ' + (failed.error_msg.length > 0));

const slow = lynx.fetchBundle('app:///slow.bundle', {});
const timedOut = slow.wait(0.05);
console.log('mts timeout ' + timedOut.code + ' ' + timedOut.error_msg);
slow.then(function (info) { console.log('mts late ' + info.code); });
";

const BACKGROUND_ENTRY: &str = r"
import { lynx, console } from 'bobcat:bts-runtime';
const handle = lynx.fetchBundle('app:///lazy.bundle', {});
const settled = handle.wait(5);
console.log('bts wait ' + settled.code + ' ' + settled.url);
lynx.loadScript('background', {bundleName: settled.url});
const again = lynx.fetchBundle('app:///lazy.bundle', {});
again.then(function (info) { console.log('bts posted ' + info.code); });
console.log('bts after then');
";

const HELD_FOR: Duration = Duration::from_millis(300);

struct Files {
    held: Mutex<VecDeque<(Instant, SourceCompletion)>>,
    lazy_requests: Arc<AtomicUsize>,
    fetched: Arc<Mutex<HashSet<String>>>,
}

impl bobcat_core::FrameImages for Files {
    type PlatformFrame = std::convert::Infallible;

    fn describe(
        &self,
        _picture: &bobcat_core::PictureKey,
        _hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::FrameImage> {
        None
    }

    fn want(&self, _picture: &bobcat_core::PictureKey, _hint: bobcat_core::ImageSizeHint) {}

    fn retain(&self, _frame: &[bobcat_core::PictureKey]) {}
}

impl Files {
    fn remember(&self, url: &str) {
        self.fetched.lock().unwrap().insert(url.to_owned());
    }

    fn script(url: &str) -> Option<String> {
        Some(match url {
            MAIN_URL => format!("{}{MAIN_ENTRY}", bobcat_core::MTS_CHUNK_PREAMBLE),
            BACKGROUND_URL => BACKGROUND_ENTRY.to_owned(),
            "app:///lazy.bundle/main-thread.js" => format!(
                "{}export default (function (entry) {{ return 'loaded ' + entry; }})",
                bobcat_core::MTS_CHUNK_PREAMBLE
            ),
            "app:///lazy.bundle/background.js" => format!(
                "{}export default (function () {{ return {{init: function () {{ \
                 console.log('bts section init'); }}}}; }})()",
                bobcat_core::BTS_CHUNK_PREAMBLE
            ),
            _ => return None,
        })
    }
}

impl ResourceFetcher for Files {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        let answer = match &request {
            SourceRequest::Fetch { url, .. } => match url.as_str() {
                LAZY => {
                    self.lazy_requests.fetch_add(1, Ordering::SeqCst);
                    self.remember(url);
                    Ok(LoadedSource::Fetched(FetchOutcome::default()))
                }
                SLOW => {
                    self.held
                        .lock()
                        .unwrap()
                        .push_back((Instant::now() + HELD_FOR, completion));
                    return;
                }
                _ => Err(unsupported(url)),
            },
            SourceRequest::StyleSheet(url) => {
                if url == "app:///lazy.bundle/custom_section/CSS.css" {
                    Ok(LoadedSource::StyleSheet(StyleSheetSource::Text(
                        ".lazy-box { background-color: blue; }".to_owned(),
                    )))
                } else {
                    Err(missing(url))
                }
            }
            SourceRequest::Module(url) => Files::script(url)
                .map(|source| LoadedSource::Module {
                    source,
                    url: url.clone(),
                })
                .ok_or_else(|| missing(url)),
            SourceRequest::Font { url } => Err(missing(url)),
        };
        completion.complete(answer);
    }

    fn service_images(&self) {
        let released = {
            let mut held = self.held.lock().unwrap();
            match held.front() {
                Some((at, _)) if *at <= Instant::now() => held.pop_front(),
                _ => None,
            }
        };
        if let Some((_, completion)) = released {
            self.remember(SLOW);
            completion.complete(Ok(LoadedSource::Fetched(FetchOutcome::default())));
        }
    }

    fn fetch_probe(&self) -> Option<bobcat_core::resource::FetchProbe> {
        let fetched = Arc::clone(&self.fetched);
        Some(Arc::new(move |url: &str| {
            fetched.lock().unwrap().contains(url)
        }))
    }
}

fn missing(locator: &str) -> bobcat_core::LynxViewError {
    ResourceError {
        kind: ResourceErrorKind::NotFound,
        phase: ResourceErrorPhase::Resolve,
        locator: Some(Arc::from(locator)),
        message: "this page has no such file".into(),
        retry: RetryAdvice::Never,
    }
    .into()
}

fn unsupported(locator: &str) -> bobcat_core::LynxViewError {
    ResourceError {
        kind: ResourceErrorKind::UnsupportedOperation,
        phase: ResourceErrorPhase::ReadBody,
        locator: Some(Arc::from(locator)),
        message: "this host installs no such container".into(),
        retry: RetryAdvice::Never,
    }
    .into()
}

async fn logs_of(wanted: usize) -> (Vec<String>, usize) {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let mut sources = ViewSources::new("app:///", MAIN_URL, SCREEN, "en-US");
    sources.background_entry = Some(BACKGROUND_URL.to_owned());
    let lazy_requests = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&lazy_requests);
    let fetched: Arc<Mutex<HashSet<String>>> = Arc::default();
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            move |_reports| Files {
                held: Mutex::new(VecDeque::new()),
                lazy_requests: counting,
                fetched,
            },
            Vec::new(),
            sources,
        )
        .expect("the view is built");
    let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
        .await
        .expect("the painter is built");
    painter.attach(&view).expect("a fresh view takes a painter");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut logged = Vec::new();
    let mut booted = false;
    while !booted || logged.len() < wanted {
        for event in view.pump() {
            match event {
                EngineEvent::ConsoleMessage { message, .. } => logged.push(message),
                EngineEvent::ScriptFinished => booted = true,
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
        assert!(
            Instant::now() < deadline,
            "both realms settled their fetches; logged so far: {logged:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    (logged, lazy_requests.load(Ordering::SeqCst))
}

#[tokio::test]
async fn both_realms_fetch_a_container_and_load_its_sections_by_url() {
    let (logged, lazy_requests) = logs_of(12).await;
    let mts: Vec<&str> = logged
        .iter()
        .map(String::as_str)
        .filter(|message| message.starts_with("mts"))
        .collect();
    let bts: Vec<&str> = logged
        .iter()
        .map(String::as_str)
        .filter(|message| message.starts_with("bts"))
        .collect();

    assert_eq!(
        mts,
        [
            "mts wait 0 app:///lazy.bundle 0",
            "mts section loaded app:///lazy.bundle",
            "mts adopted app:///lazy.bundle/custom_section/CSS.css",
            "mts inline 0",
            "mts after then",
            "mts failed -1 true",
            "mts timeout -2 ResponsePromise wait timeout after 0.05 seconds for url: app:///slow.bundle",
            "mts late 0",
        ]
    );
    assert_eq!(
        bts,
        [
            "bts wait 0 app:///lazy.bundle",
            "bts section init",
            "bts after then",
            "bts posted 0",
        ]
    );
    assert_eq!(lazy_requests, 1);
}
