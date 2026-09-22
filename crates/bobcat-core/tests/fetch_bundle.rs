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
    LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase, ResourceFetcher,
    RetryAdvice, SourceCompletion, SourceRequest, StyleSheetSource,
};
use bobcat_core::{
    DrawTarget, EngineEvent, LynxGroup, NoWakeup, Painter, StyleThreads, ViewSources,
};

const MAIN_URL: &str = "app:///main.js";
const BACKGROUND_URL: &str = "app:///background.js";
/// The container both realms ask for, by the string the card writes.
const LAZY: &str = "app:///lazy.bundle";
/// A container whose fetch is retained, so a `wait` times out on it.
const SLOW: &str = "app:///slow.bundle";

/// The MTS card.
///
/// It fetches the container twice: once outstanding, once the fetcher
/// already holds. The second `fetchBundle` settles synchronously through the
/// probe, so its `.then` runs **inline** — which is what `ReactLynx`'s
/// `rLynxPrepareLazyBundleMTS` relies on, and what the log order below
/// proves.
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

/// The BTS entry. It fetches the same container, loads its `background`
/// section — whose `init` logs — and pins that a `.then` on a settled handle
/// is a *posted* task here, unlike MTS.
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

/// How long the retained container fetch is held for, which has to outlast
/// the card's own `wait(0.05)` by enough that the timeout is the reason it
/// returned.
const HELD_FOR: Duration = Duration::from_millis(300);

/// Every source a page is made of, plus the containers this host installs
/// when it recognizes what a plain fetch brought back.
struct Files {
    /// Fetches retained rather than answered, released by a later
    /// `service_images` — the host's own turn, as `DelayedBackground` does in
    /// `bobcat-source`'s tests — once [`HELD_FOR`] has passed.
    held: Mutex<VecDeque<(Instant, SourceCompletion)>>,
    /// How many times this host was asked to fetch [`LAZY`], across both
    /// realms of the view.
    lazy_requests: Arc<AtomicUsize>,
    /// Every URL a fetch has completed for, which is what `fetch_probe`
    /// answers out of — the reference fetcher's own `FetchIndex`, in
    /// miniature. Shared with the view's other realm, because the fetcher is
    /// the view's.
    fetched: Arc<Mutex<HashSet<String>>>,
}

impl bobcat_core::FrameImages for Files {
    fn read(
        &self,
        _source: &str,
        _hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::vello::peniko::ImageData> {
        None
    }

    fn retain(&self, _frame: &[Arc<str>]) {}
}

impl Files {
    /// Remembers one completed fetch, which is what makes the next
    /// `fetchBundle` of it settle at once. Only a *successful* one: a failure
    /// is never remembered, as native remembers none.
    fn remember(&self, url: &str) {
        self.fetched.lock().unwrap().insert(url.to_owned());
    }

    /// The scripts this host serves, by URL.
    ///
    /// The container's sections sit at exactly the URLs the realm derives
    /// from the string it passed — `<container>/<section>.js` — because that
    /// is the one rule `named_chunk_url` and `section-url.ts` share.
    fn script(url: &str) -> Option<String> {
        Some(match url {
            MAIN_URL => MAIN_ENTRY.to_owned(),
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
            // A plain fetch, which this host happens to make a container of:
            // the fetch, the decode and the registration all happened here,
            // and what comes back says only that the fetch is over.
            SourceRequest::Fetch { url } => match url.as_str() {
                LAZY => {
                    self.lazy_requests.fetch_add(1, Ordering::SeqCst);
                    self.remember(url);
                    Ok(LoadedSource::Fetched)
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
                if url == "app:///lazy.bundle/index.css" {
                    Ok(LoadedSource::StyleSheet(StyleSheetSource::Text(
                        ".lazy-box { background-color: blue; }".to_owned(),
                    )))
                } else {
                    Err(missing(url))
                }
            }
            SourceRequest::Entry(url) | SourceRequest::Module(url) => Files::script(url)
                .map(|source| LoadedSource::Entry {
                    source,
                    url: url.clone(),
                })
                .ok_or_else(|| missing(url)),
            SourceRequest::Font { url } => Err(missing(url)),
            SourceRequest::Worker { specifier, .. } => Err(missing(specifier)),
        };
        completion.complete(answer);
    }

    /// The host's own turn, which is where a retained fetch is released: the
    /// timeout in the card has to be able to pass first.
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
            completion.complete(Ok(LoadedSource::Fetched));
        }
    }

    /// The probe a realm asks before making a fetch. `Send + Sync` because it
    /// is called from `bobcat-main` and from `bobcat-workers`, never from
    /// here.
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

/// Boots one view and collects what its two realms logged, until `wanted`
/// messages have arrived and MTS boot has finished.
async fn logs_of(wanted: usize) -> (Vec<String>, usize) {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let mut sources = ViewSources::new(MAIN_URL);
    sources.background_entry = Some(BACKGROUND_URL.to_owned());
    let lazy_requests = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&lazy_requests);
    // One set per view, shared by every realm of it, the way the reference
    // fetcher's is: what the MTS realm fetched is what the BTS Worker's probe
    // sees.
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
    // Boot's first flush waits for a painter to bind the view, and this waits
    // for MTS boot to finish.
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
                EngineEvent::WorkerFailed(error) | EngineEvent::ScriptRunError(error) => {
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

/// Everything one card does with a container, in the order the two realms do
/// it.
///
/// The MTS half is asserted as a *sequence*, because the order is the
/// contract: `mts inline 0` before `mts after then` is a repeat fetch
/// settling through the fetcher's probe, in the realm's own job, and
/// `mts timeout -2` before `mts late 0` is a timeout that cancelled nothing.
#[tokio::test]
async fn both_realms_fetch_a_container_and_load_its_sections_by_url() {
    // Eight MTS lines and four BTS ones; the two interleave, so they are
    // separated rather than compared as one list.
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
            // `url` is the string the card passed, echoed, and a settled
            // record carries no message.
            "mts wait 0 app:///lazy.bundle 0",
            // The `main-thread` section is the function `ReactLynx` calls.
            "mts section loaded app:///lazy.bundle",
            // Its stylesheet adopts without throwing, at the URL
            // `named_style_url` writes.
            "mts adopted app:///lazy.bundle/index.css",
            // Inline: the fetcher's probe answered in this very job, so the
            // callback ran before `then` returned.
            "mts inline 0",
            "mts after then",
            "mts failed -1 true",
            // The timeout is native's record, word for word, and the fetch
            // went on: the later delivery still runs the callback.
            "mts timeout -2 ResponsePromise wait timeout after 0.05 seconds for url: app:///slow.bundle",
            "mts late 0",
        ]
    );
    assert_eq!(
        bts,
        [
            "bts wait 0 app:///lazy.bundle",
            // The `background` section's body handed over an `{init}`, which
            // `lynx.loadScript` initialized.
            "bts section init",
            // Posted, not inline: `after then` is logged first.
            "bts after then",
            "bts posted 0",
        ]
    );
    // Four `fetchBundle` calls of one URL across two realms, and **one**
    // fetch: core remembers nothing, but the fetcher does, and its probe is
    // what every realm of the view asks before requesting anything.
    assert_eq!(lazy_requests, 1);
}
