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

use quickjs_rust_bridge::{HostValue, RequiredKind, RequiredSource};

use crate::esm::HOST_MODULE_SPECIFIER;
use crate::jobs::JsThreadHandle;
use crate::link::HostOutbox;
use crate::main::quickjs::{ScriptEngine, ScriptRuntime, normalize_module_url};
use crate::resource::{LoadedSource, SourceRequest, unanswered_source};
use crate::script::ScriptError;

/// Resolves a specifier against a base URL, the way an `import` in a module at
/// that base resolves it.
const RESOLVE_EXPORT: &str = "resolveModuleUrl";
/// Loads and compiles one already-resolved URL, synchronously.
const LOAD_EXPORT: &str = "loadModuleSync";

pub(crate) fn install(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    sources: HostOutbox,
    thread: JsThreadHandle,
) -> Result<(), ScriptError> {
    engine.register_host_module_function(
        js_runtime,
        HOST_MODULE_SPECIFIER,
        RESOLVE_EXPORT,
        2,
        Box::new(|arguments| {
            const NAME: &str = "bobcat-internal:host.resolveModuleUrl";
            let base = string_argument(NAME, arguments, 0)?;
            let specifier = string_argument(NAME, arguments, 1)?;
            normalize_module_url(base, specifier).map(HostValue::String)
        }),
    )?;
    let token = sources.token().clone();
    engine.register_synchronous_loader(js_runtime, HOST_MODULE_SPECIFIER, LOAD_EXPORT, move |url| {
        // This receiver lives only for this call, as a stylesheet's does:
        // caching and in-flight sharing are the fetcher's, and the realm's
        // own cache is what makes a URL loaded at most once anyway.
        let answer = sources.request(SourceRequest::Module(url.to_owned()));
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
            LoadedSource::Entry { source, url } => Ok(RequiredSource {
                kind: kind_of(&url),
                url,
                text: source,
            }),
            LoadedSource::StyleSheet(_) => Err("the fetcher returned a stylesheet".to_owned()),
            LoadedSource::Font(_) => Err("the fetcher returned a font".to_owned()),
            LoadedSource::Fetched => Err("the fetcher returned a plain fetch".to_owned()),
        }
    })
}

/// One argument of a host call, as the string it must be.
///
/// Strict, unlike the tree members' own helper: a base and a specifier are
/// URLs, and nothing a missing or null argument could be read as stands in for
/// one, where an id those members are handed has a whole tree to be wrong
/// about.
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

/// How the *response* URL's path says the source is to be read, which is
/// Node 24's rule with the one thing it has that a URL does not — the nearest
/// `package.json` `"type"` — missing: `.json` is JSON, `.mjs` is an ES
/// module, `.cjs` is `CommonJS`, and everything else is the engine's to detect
/// from the text.
///
/// The path alone, so a query or a fragment is no part of the answer, and a
/// URL that does not parse has no path to read: it is detected too.
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
