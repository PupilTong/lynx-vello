//! The in-crate test harness: a real group, a real view, and a host that
//! answers from memory.
//!
//! There is no lighter seam on purpose. Boot is one straight-line task now —
//! sources, then the realm, then the entry — so a harness that skipped it
//! would have to reimplement it, and would be testing its own copy. What this
//! replaces instead is the *host*: an [`InlineFetcher`] answers every source
//! request from a string it was given, and a view that has nowhere to draw
//! pays for no GPU device.

use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustc_hash::FxHashMap;

use crate::resource::{
    LoadedSource, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceErrorKind, ResourceErrorPhase, ResourceFetcher, ResourceRequest, ResourceResponse,
    RetryAdvice, SourceCompletion, SourceRequest, StyleSheetSource,
};
use crate::style::PreparsedStyleSheet;
use crate::view::{
    DrawTarget, EngineEvent, EventRequester, LynxGroup, LynxView, NoWakeup, StyleThreads,
    ViewSources,
};

/// How long a test waits on work that should already be happening.
///
/// A hang detector rather than a performance assertion: the first document a
/// test process builds pays for the platform's font registry, which is
/// seconds on some machines and nothing on others.
const PATIENCE: Duration = Duration::from_secs(30);

/// One author stylesheet, in whichever of the two forms a host has it.
pub(crate) enum TestSheet {
    Text(String),
    Preparsed(Arc<PreparsedStyleSheet>),
}

/// A host whose whole resource system is the strings it was built with.
pub(crate) struct InlineFetcher {
    entry: String,
    sheets: FxHashMap<String, TestSheet>,
}

impl dom::FrameImages for InlineFetcher {
    fn read(
        &self,
        _source: &str,
        _hint: dom::ImageSizeHint,
    ) -> Option<dom::vello::peniko::ImageData> {
        None
    }
}

impl ResourceFetcher for InlineFetcher {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        let answer = match request {
            SourceRequest::Entry(url) => Ok(LoadedSource::Entry {
                source: self.entry.clone(),
                url,
            }),
            SourceRequest::StyleSheet(url) => match self.sheets.get(&url) {
                Some(TestSheet::Text(css)) => Ok(LoadedSource::StyleSheet(StyleSheetSource::Text(
                    css.clone(),
                ))),
                Some(TestSheet::Preparsed(sheet)) => Ok(LoadedSource::StyleSheet(
                    StyleSheetSource::Preparsed(Arc::clone(sheet)),
                )),
                None => Err(missing(&url)),
            },
            SourceRequest::Module(url) => Err(missing(&url)),
            SourceRequest::Worker { specifier, .. } => Err(missing(&specifier)),
        };
        completion.complete(answer);
    }

    fn supports_capability(&self, _capability: ResourceCapability) -> bool {
        false
    }

    async fn resolve_locator(
        &self,
        _request: ResolveRequest,
    ) -> Result<ResolvedLocator, ResourceError> {
        panic!("the harness resolves nothing: every source it serves is already named")
    }

    async fn fetch_resource(
        &self,
        _request: ResourceRequest,
    ) -> Result<ResourceResponse, ResourceError> {
        panic!("the harness moves no bytes")
    }
}

fn missing(locator: &str) -> crate::LynxViewError {
    ResourceError {
        request_id: None,
        kind: ResourceErrorKind::NotFound,
        phase: ResourceErrorPhase::Resolve,
        locator: Some(locator.into()),
        status: None,
        message: format!("the harness was given nothing for `{locator}`").into(),
        retry: RetryAdvice::Never,
    }
    .into()
}

/// A view the crate's own tests drive, over the host above.
pub(crate) type TestView = LynxView<Rc<InlineFetcher>>;

/// One view to build: its entry, its author sheets in cascade order, and
/// where — if anywhere — it draws.
pub(crate) struct TestViewSpec {
    entry: String,
    sheets: Vec<(String, TestSheet)>,
    target: DrawTarget,
    width: f32,
    height: f32,
}

impl TestViewSpec {
    /// A phone-shaped view with nowhere to draw, which is what a test about
    /// routing, events or timers wants.
    pub(crate) fn new(entry: &str) -> Self {
        Self {
            entry: entry.to_owned(),
            sheets: Vec::new(),
            target: DrawTarget::None,
            width: 393.0,
            height: 727.0,
        }
    }

    /// One author sheet as CSS text, mounted before the entry runs.
    pub(crate) fn with_style_sheet(mut self, css: &str) -> Self {
        let url = format!("app:///sheet-{}.css", self.sheets.len());
        self.sheets.push((url, TestSheet::Text(css.to_owned())));
        self
    }

    /// One author sheet the host decoded itself, as a bundle embedder's does.
    pub(crate) fn with_preparsed_style_sheet(mut self, sheet: PreparsedStyleSheet) -> Self {
        let url = format!("app:///sheet-{}.css", self.sheets.len());
        self.sheets
            .push((url, TestSheet::Preparsed(Arc::new(sheet))));
        self
    }

    /// A view that renders into a texture of its own, for a test that reads
    /// pixels back.
    pub(crate) fn offscreen(mut self, width: f32, height: f32) -> Self {
        self.target = DrawTarget::Offscreen;
        self.width = width;
        self.height = height;
        self
    }

    /// Builds the view without waiting for it to boot.
    pub(crate) fn create<R: EventRequester>(self, requester: Arc<R>) -> TestView {
        let Self {
            entry,
            sheets,
            target,
            width,
            height,
        } = self;
        let sources = ViewSources {
            style_sheets: sheets.iter().map(|(url, _)| url.clone()).collect(),
            ..ViewSources::new("app:///main.js")
        };
        let fetcher = Rc::new(InlineFetcher {
            entry,
            sheets: sheets.into_iter().collect(),
        });
        block_on(async move {
            let group = LynxGroup::new(requester, StyleThreads::Sequential)
                .await
                .expect("the test group starts");
            group
                .create_lynx_view(width, height, 1.0, target, |_reports| fetcher, sources)
                .await
                .expect("the test view is built")
        })
    }

    /// Builds the view and waits for its entry to finish.
    pub(crate) fn boot(self) -> TestView {
        let mut view = self.create(Arc::new(NoWakeup));
        wait_for_boot(&mut view);
        view
    }
}

/// Drives ordinary painter turns until the entry finishes.
pub(crate) fn wait_for_boot(view: &mut TestView) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => {
                    assert!(
                        view.published_frame().is_some(),
                        "boot's flush publishes before ScriptFinished is pumped"
                    );
                    return;
                }
                EngineEvent::StartupFailed(error) => panic!("the view did not boot: {error}"),
                EngineEvent::ScriptRunError(error) => panic!("the entry module failed: {error}"),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "the entry module did not finish");
        std::thread::yield_now();
    }
}

/// Runs `future` to completion on this thread.
///
/// Construction is the only asynchronous part of the public API, so every
/// test needs an executor for exactly that and then drives the view
/// synchronously, as an embedder does.
pub(crate) fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a test runtime asks the platform for nothing")
        .block_on(future)
}
