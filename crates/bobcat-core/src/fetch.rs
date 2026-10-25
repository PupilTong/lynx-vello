//! A realm's `fetchResource`: one URL fetched for its outcome, as
//! [`crate::future`]'s first producer; `lynx.fetchBundle` is written over it
//! in `bundle-fetch.ts`.
//!
//! Not the main thread's and not a worker's — *a realm's*, like
//! [`crate::timers`] and [`crate::require`]. Both kinds get the member.
//!
//! # What is not here
//!
//! Everything about the bytes. [`SourceRequest::Fetch`] asks the host to
//! fetch a URL and keep it, exactly as an image source is fetched; what the
//! fetcher makes of what it got — a cache entry, a Lynx container whose
//! sections it registers — is the fetcher's own business, and nothing about
//! it comes back. What this member answers is a number: the id of the
//! [`FutureTable`] entry the fetch's outcome settles.
//!
//! The wait and the asynchronous settle are [`crate::future`]'s too, so
//! nothing here parks, spawns or delivers.
//!
//! # The one synchronous answer
//!
//! A URL this view's fetcher has **already fetched** answers `true` in the
//! same call, and no request is made. That is the whole of what
//! [`ResourceFetcher::fetch_probe`](crate::resource::ResourceFetcher::fetch_probe)
//! buys, and the realm needs it: a Promise resolved in a later job is not
//! what a repeat fetch is to a card. Native answers one out of
//! `TemplateAssembler::FindTemplateBundle` and web-core out of its promise
//! cache, both **in the same task**, and `ReactLynx`'s
//! `rLynxPrepareLazyBundleMTS` is written to exactly that — its
//! `loadScript('main-thread')` has to have run before the `callLepusMethod`
//! reply reaches the background thread.
//!
//! Core still keeps no cache. What has been fetched is the *fetcher's*
//! knowledge, asked for through a `Send + Sync` probe the view handed over at
//! construction, and a host that gave none simply never answers `true`.
//!
//! # The URL a fetch names
//!
//! `resolveUrl(url)` answers `url` resolved against the view's
//! [`ViewSources::base_url`](crate::ViewSources::base_url) by URL rules — the
//! resolution a synchronous load makes before it asks the host, and the one
//! the view's fetcher makes of a URL it is handed. A realm that goes on to
//! build other URLs out of the one it fetched — a `<frame>`'s card, whose
//! sections, background thread and workers all hang under the card's URL —
//! resolves it once, so what it fetches and what it builds name one
//! absolute URL. It requests nothing.
//!
//! # The `resource.lazyBundle` entry
//!
//! A fetch the realm names a lazy bundle load — `fetchResource(url, true)`,
//! which `lynx.fetchBundle` makes and a `<frame>`'s fetch of its card does
//! not, since native's frame load (`LazyBundleLoader::LoadFrameBundle`,
//! `kFrame`) reports no such entry — is timed while someone observes
//! `resource` — the Rust embedder through
//! [`EntryTypes::RESOURCE`](crate::performance::EntryTypes::RESOURCE), or a
//! BTS observer of `resource` — when it makes a request: the
//! call's instant is its `requireStart`, the request asks the fetcher for its
//! own readings ([`SourceRequest::Fetch::timed`]), and the outcome's delivery
//! to the realm completes one Lynx `LazyBundleEntry`, opened by the realm
//! that called ([`LazyBundleEntry::from_fetch`]). A URL already fetched,
//! answered `true` at once, is native's cached load, which reports nothing.
//! While nobody observes, a fetch reads one relaxed atomic for it and takes
//! no instant.

use std::rc::Rc;
use std::sync::Arc;

use quickjs_rust_bridge::{HostArgument, HostValue};

use crate::clock::ClockInstant;
use crate::esm::HOST_MODULE_SPECIFIER;
use crate::future::FutureTable;
use crate::link::HostOutbox;
use crate::main::quickjs::ScriptEngine;
use crate::performance::{LazyBundleEntry, Opener};
use crate::resource::{LoadedSource, SourceRequest, unanswered_source};
use crate::script::ScriptError;

/// Starts one fetch and answers the id of the future its outcome settles —
/// or `true`, for a URL this view has fetched already. A second argument,
/// `true`, names the fetch a lazy bundle load, which a `resource.lazyBundle`
/// entry reports.
const FETCH_EXPORT: &str = "fetchResource";

/// Resolves a URL against the view's base, as a fetch of it resolves it.
const RESOLVE_EXPORT: &str = "resolveUrl";

/// Installs the one member a realm fetches a URL through, and the one that
/// resolves a URL the way that fetch does.
///
/// Neither evaluates anything or touches a document. `fetchResource` reads no
/// byte of what comes back: it registers the fetch as a future of this
/// realm's and answers that future's id — unless the view's fetcher says it
/// already holds that URL, in which case it answers `true` and starts
/// nothing. `resolveUrl` answers a string, and throws for a URL that does not
/// resolve.
///
/// `opener` is the realm the fetches are timed as — the main-thread realm or
/// `<frame>` card realm, or the worker — which decides which BTS observer
/// hears its `resource.lazyBundle` entries.
pub(crate) fn install(
    engine: &mut ScriptEngine,
    sources: &HostOutbox,
    futures: &Rc<FutureTable>,
    opener: Opener,
) -> Result<(), ScriptError> {
    let resolving = sources.clone();
    install_member(engine, RESOLVE_EXPORT, 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.resolveUrl";
        resolving
            .resolve(string_argument(NAME, arguments, 0)?)
            .map(HostValue::String)
    })?;
    let requests = sources.clone();
    let futures = Rc::clone(futures);
    install_member(engine, FETCH_EXPORT, 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.fetchResource";
        let url = string_argument(NAME, arguments, 0)?.to_owned();
        let lazy_bundle = matches!(arguments.get(1), Some(HostArgument::Boolean(true)));
        // Already fetched, as the fetcher itself answers: nothing is
        // requested and the realm has its outcome before this call returns.
        if requests.fetch_probe().is_some_and(|probe| probe(&url)) {
            return Ok(HostValue::Boolean(true));
        }
        // The one read a lazy bundle's fetch makes while nobody observes
        // `resource`.
        let registry = requests.registry();
        let timing = (lazy_bundle && registry.times_resources())
            .then(|| (ClockInstant::now(), Arc::<str>::from(url.as_str())));
        let answer = requests.request(SourceRequest::Fetch {
            url,
            timed: timing.is_some(),
        });
        let timed = timing.map(|timing| (timing, Arc::clone(registry), requests.clone()));
        // Registered rather than awaited: which of the two ways out the realm
        // takes — the synchronous `Future.wait` or the `.then` the owner's
        // epilogue settles — is the caller's business, not this member's.
        let id = futures.register(async move {
            let answer = answer.await;
            if let Some(((started, url), registry, host)) = timed {
                let outcome = match &answer {
                    Ok(Ok(LoadedSource::Fetched(outcome))) => Some(outcome),
                    _ => None,
                };
                let entry = LazyBundleEntry::from_fetch(url, started, ClockInstant::now(), outcome);
                if registry.note_lazy_bundle(opener, entry) {
                    // The embedder's next `pump` reports it, and tells the
                    // BTS observer owed it.
                    host.wake();
                }
            }
            match answer {
                Ok(Ok(LoadedSource::Fetched(_))) => Ok(HostValue::Undefined),
                Ok(Ok(LoadedSource::Module { .. })) => {
                    Err("the fetcher returned a script".to_owned())
                }
                Ok(Ok(LoadedSource::StyleSheet(_))) => {
                    Err("the fetcher returned a stylesheet".to_owned())
                }
                Ok(Ok(LoadedSource::Font(_))) => Err("the fetcher returned a font".to_owned()),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err(unanswered_source().to_string()),
            }
        });
        Ok(HostValue::Number(f64::from(id)))
    })
}

fn install_member(
    engine: &mut ScriptEngine,
    name: &str,
    arity: u8,
    callback: impl FnMut(&[HostArgument<'_>]) -> Result<HostValue, String> + 'static,
) -> Result<(), ScriptError> {
    engine.register_host_module_function(HOST_MODULE_SPECIFIER, name, arity, callback)
}

fn string_argument<'a>(
    function: &str,
    arguments: &[HostArgument<'a>],
    index: usize,
) -> Result<&'a str, String> {
    match arguments.get(index) {
        Some(&HostArgument::String(value)) => Ok(value),
        _ => Err(format!("{function} expects a string for argument {index}")),
    }
}
