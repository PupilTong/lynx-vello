//! A realm's `require`: the host's source protocol, awaited synchronously.
//!
//! Not the main thread's and not a worker's — *a realm's*, like
//! [`crate::timers`]. Both kinds import `createRequire` from
//! [`REQUIRE_MODULE_SPECIFIER`], both resolve specifiers through the
//! normalizer their imports use, and both reach the same
//! [`SourceRequest::Module`] the ESM loader does.
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

use quickjs_rust_bridge::{RequiredKind, RequiredSource};

use crate::esm::REQUIRE_MODULE_SPECIFIER;
use crate::jobs::JsThreadHandle;
use crate::link::HostOutbox;
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::resource::{LoadedSource, SourceRequest, unanswered_source};
use crate::script::ScriptError;

const CREATE_REQUIRE_EXPORT: &str = "createRequire";

pub(crate) fn install(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    sources: HostOutbox,
    thread: JsThreadHandle,
) -> Result<(), ScriptError> {
    let token = sources.token().clone();
    engine.register_create_require(
        js_runtime,
        REQUIRE_MODULE_SPECIFIER,
        CREATE_REQUIRE_EXPORT,
        move |url| {
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
            }
        },
    )
}

/// JSON exactly when the *response* URL's path says so. The path alone, so a
/// query or a fragment is no part of the answer.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "a URL path is case-sensitive, as is Node's own extension match"
)]
fn kind_of(url: &str) -> RequiredKind {
    match url::Url::parse(url) {
        Ok(url) if url.path().ends_with(".json") => RequiredKind::Json,
        _ => RequiredKind::CommonJs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_the_response_paths_extension_and_nothing_else() {
        assert_eq!(kind_of("app:///config.json"), RequiredKind::Json);
        assert_eq!(kind_of("app:///config.json?v=2#top"), RequiredKind::Json);
        assert_eq!(
            kind_of("app:///main.cjs?fallback=.json"),
            RequiredKind::CommonJs
        );
        assert_eq!(kind_of("not a url"), RequiredKind::CommonJs);
    }
}
