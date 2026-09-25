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

use std::rc::Rc;

use quickjs_rust_bridge::HostValue;

use crate::esm::HOST_MODULE_SPECIFIER;
use crate::future::FutureTable;
use crate::link::HostOutbox;
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::resource::{LoadedSource, SourceRequest, unanswered_source};
use crate::script::ScriptError;

/// Starts one fetch and answers the id of the future its outcome settles —
/// or `true`, for a URL this view has fetched already.
const FETCH_EXPORT: &str = "fetchResource";

/// Installs the one member a realm fetches a URL through.
///
/// It evaluates nothing, touches no document and reads no byte of what comes
/// back: it registers the fetch as a future of this realm's and answers that
/// future's id — unless the view's fetcher says it already holds that URL, in
/// which case it answers `true` and starts nothing.
pub(crate) fn install(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    sources: &HostOutbox,
    futures: &Rc<FutureTable>,
) -> Result<(), ScriptError> {
    let requests = sources.clone();
    let futures = Rc::clone(futures);
    install_member(engine, js_runtime, FETCH_EXPORT, 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.fetchResource";
        let url = string_argument(NAME, arguments, 0)?.to_owned();
        // Already fetched, as the fetcher itself answers: nothing is
        // requested and the realm has its outcome before this call returns.
        if requests.fetch_probe().is_some_and(|probe| probe(&url)) {
            return Ok(HostValue::Boolean(true));
        }
        let answer = requests.request(SourceRequest::Fetch { url });
        // Registered rather than awaited: which of the two ways out the realm
        // takes — the synchronous `Future.wait` or the `.then` the owner's
        // epilogue settles — is the caller's business, not this member's.
        let id = futures.register(async move {
            match answer.await {
                Ok(Ok(LoadedSource::Fetched)) => Ok(HostValue::Undefined),
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
    js_runtime: &mut ScriptRuntime,
    name: &str,
    arity: u8,
    callback: impl FnMut(&[HostValue]) -> Result<HostValue, String> + 'static,
) -> Result<(), ScriptError> {
    engine.register_host_module_function(
        js_runtime,
        HOST_MODULE_SPECIFIER,
        name,
        arity,
        Box::new(callback),
    )
}

fn string_argument<'a>(
    function: &str,
    arguments: &'a [HostValue],
    index: usize,
) -> Result<&'a str, String> {
    match arguments.get(index) {
        Some(HostValue::String(value)) => Ok(value),
        _ => Err(format!("{function} expects a string for argument {index}")),
    }
}
