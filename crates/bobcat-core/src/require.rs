//! A realm's `require`: the two host members Node's algorithm is written over.
//!
//! Not the main thread's and not a worker's — *a realm's*, like
//! [`crate::timers`]. Both kinds get both members, and both reach the same
//! [`SourceRequest::Module`] the ESM loader does.
//!
//! The algorithm itself — the cache, the `module` object, cycles, eviction,
//! `require.resolve` — is not here. It is
//! [`crate::esm::REQUIRE_MODULE_SPECIFIER`], a source module in
//! `packages/bobcat-element`, like every other built-in.
//! What is here is the two things it cannot do in JavaScript:
//!
//! - `resolveModuleUrl(base, specifier)`, the normalizer an `import` resolves through, so a
//!   `require` and an `import` name the same module by the same URL.
//! - `loadModuleSync(url, parameters)`, which asks the host for that URL and answers with the
//!   source compiled — as the wrapper function of a `CommonJS` file, the parsed value of a JSON
//!   one, or the namespace of an ES module, linked and evaluated. The text never becomes a
//!   JavaScript value, and the compile names the response URL.
//!
//! The URL is resolved against the view's
//! [`ViewSources::base_url`](crate::ViewSources::base_url) by URL rules before
//! it is requested, in every realm of the view, so a rooted or relative
//! section URL a realm builds reaches the fetcher absolute. A URL that does
//! not resolve throws as a failed load does. An absolute URL — which is what
//! `require` and every import hand over, already normalized — resolves to
//! itself. A `lynx:` URL, a native module's ES module, is never requested:
//! it throws here, so a `require` of one, or of a module that imports one,
//! works only once an `import` has loaded it into the realm.
//!
//! An ES module is linked *inline*: every `import` in it, and in what it
//! imports, reaches this same closure as a [`SourceRequest::Module`] of its
//! own and parks its own job, recursively, while the module that imports it
//! is being compiled. Nothing is nested in the thread's sense — each load has
//! returned before the compile that starts the next one — so the waits are
//! one after another, and what a job of this thread sees is one long one.
//!
//! The wait is `adoptStyleSheet`'s, and so is what it costs: it parks the
//! *job* this `require` runs in. Every task of this engine thread goes on
//! running — the channel reads, the lifecycle signals, the routing that
//! answers this very request — and no other job runs, so no promise job of
//! this realm's and no entry of a sibling realm's interleaves with the load.
//!
//! The wait's other arm is the requesting realm's end signal, which a task
//! is what cancels. For a view's MTS realm that is the embedder's release,
//! written from its own thread; for a Worker it is the in-band `Terminate`
//! the message consumer reads during the wait, which ends the worker and
//! cancels the token its own source requests carry.

use quickjs_rust_bridge::{HostArgument, HostValue, RequiredKind, RequiredSource};

use crate::esm::{HOST_MODULE_SPECIFIER, NATIVE_MODULE_ESM_PREFIX};
use crate::jobs::JsThreadHandle;
use crate::link::HostOutbox;
use crate::main::quickjs::{ScriptEngine, normalize_module_url};
use crate::resource::{LoadedSource, SourceRequest, unanswered_source};
use crate::script::ScriptError;

const RESOLVE_EXPORT: &str = "resolveModuleUrl";
const LOAD_EXPORT: &str = "loadModuleSync";

pub(crate) fn install(
    engine: &mut ScriptEngine,
    sources: HostOutbox,
    thread: JsThreadHandle,
) -> Result<(), ScriptError> {
    engine.register_host_module_function(
        HOST_MODULE_SPECIFIER,
        RESOLVE_EXPORT,
        2,
        |arguments| {
            const NAME: &str = "bobcat-internal:host.resolveModuleUrl";
            let base = string_argument(NAME, arguments, 0)?;
            let specifier = string_argument(NAME, arguments, 1)?;
            normalize_module_url(base, specifier).map(HostValue::String)
        },
    )?;
    let token = sources.token().clone();
    engine.register_synchronous_loader(HOST_MODULE_SPECIFIER, LOAD_EXPORT, move |url| {
        let url = sources.resolve(url)?;
        if url.starts_with(NATIVE_MODULE_ESM_PREFIX) {
            return Err(format!(
                "'{url}' is a native module's ES module, which loads by `import` and \
                 cannot be loaded synchronously before an `import` has loaded it"
            ));
        }
        let answer = sources.request(SourceRequest::Module(url));
        let source = thread.wait(async {
            tokio::select! {
                biased;
                () = token.cancelled() => Err("the realm ended".to_owned()),
                result = answer => result
                    .unwrap_or_else(|_| Err(unanswered_source().into()))
                    .map_err(|error| error.to_string()),
            }
        })?;
        match source {
            LoadedSource::Module { source, url } => Ok(RequiredSource {
                kind: kind_of(&url),
                url,
                text: source,
            }),
            LoadedSource::StyleSheet(_) => Err("the fetcher returned a stylesheet".to_owned()),
            LoadedSource::Font(_) => Err("the fetcher returned a font".to_owned()),
            LoadedSource::Fetched(_) => Err("the fetcher returned a plain fetch".to_owned()),
        }
    })
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

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "a URL path is case-sensitive, as is Node's own extension match"
)]
fn kind_of(url: &str) -> RequiredKind {
    let Ok(parsed) = url::Url::parse(url) else {
        return RequiredKind::Detect;
    };
    let path = parsed.path();
    if path.ends_with(".json") {
        RequiredKind::Json
    } else if path.ends_with(".mjs") {
        RequiredKind::Module
    } else if path.ends_with(".cjs") {
        RequiredKind::CommonJs
    } else {
        RequiredKind::Detect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_response_paths_extension_is_what_names_the_shape() {
        assert_eq!(kind_of("app:///config.json"), RequiredKind::Json);
        assert_eq!(kind_of("app:///module.mjs"), RequiredKind::Module);
        assert_eq!(kind_of("app:///script.cjs"), RequiredKind::CommonJs);
        assert_eq!(kind_of("app:///either.js"), RequiredKind::Detect);
        assert_eq!(kind_of("app:///no-extension"), RequiredKind::Detect);
    }

    #[test]
    fn a_query_or_a_fragment_is_no_part_of_the_path() {
        assert_eq!(kind_of("app:///config.json?v=2#top"), RequiredKind::Json);
        assert_eq!(kind_of("app:///module.mjs?v=2"), RequiredKind::Module);
        assert_eq!(
            kind_of("app:///main.cjs?fallback=.json"),
            RequiredKind::CommonJs
        );
        assert_eq!(kind_of("app:///main.js#.mjs"), RequiredKind::Detect);
    }

    #[test]
    fn what_is_not_a_url_has_no_path_to_read() {
        assert_eq!(kind_of("not a url"), RequiredKind::Detect);
    }
}
