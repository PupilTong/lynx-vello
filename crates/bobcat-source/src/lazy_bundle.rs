//! A lazy container, as resources: what a fetched `.lynx.bundle` or
//! `.web.bundle` becomes.
//!
//! A `ReactLynx` `lazy(() => import(...))` compiles to a container of its own —
//! a `.lynx.bundle` or a `.web.bundle` with no page root — that the card asks
//! for at runtime with `lynx.fetchBundle`. The engine's half of that is a
//! plain [`SourceRequest::Fetch`](bobcat_core::resource::SourceRequest::Fetch)
//! and nothing more: `bobcat-core` knows only that a URL was fetched. What
//! makes the container's sections *loadable* is this
//! [`ContainerInstaller`], which the reference fetcher offers every fetch's
//! bytes to — it decodes what it recognizes and **registers its bodies and
//! its stylesheets under the URLs the realm will ask for them by**. Nothing
//! is evaluated here; the realm loads a section afterwards, synchronously,
//! through the same loader a `require` uses.
//!
//! Those URLs are equal only while the two bases are. The container's URL,
//! and so every URL registered under it, is resolved by the fetcher against
//! its own base; a section load is resolved by the engine against the view's
//! [`ViewSources::base_url`](bobcat_core::ViewSources::base_url). An embedder
//! therefore gives its fetcher the view's base: the CLI, the server and the
//! browser name a [`PageSource`](crate::PageSource)'s input URL as both.
//!
//! # The URL rule
//!
//! One rule, and it is [`named_chunk_url`]'s: every body of a container at
//! `<url>` is registered at `<url path>/<encoded section name>.js`, keeping
//! the container URL's `?#` suffix, and every named stylesheet at
//! [`named_style_url`] — `<url path>/index.css` for the compiler's `CSS`
//! section. Both realms write exactly these strings: MTS in
//! `main-thread-runtime.ts`'s `chunkURL`/`styleSheetURL`, BTS in
//! `lynx-modules.ts`'s `bodyUrl` for a lazy `bundleName`, both over
//! `packages/bobcat-element/src/section-url.ts`.
//!
//! This is *not* the page container's rule, which puts its bodies beside its
//! own input URL: a lazy container's `bundleName` is a string the card wrote,
//! not a URL the engine resolved a manifest against, and the sections have to
//! hang off it unambiguously.
//!
//! # What a section becomes
//!
//! The same three shapes [`bts_module_source`] already writes, chosen by the
//! section's name:
//!
//! - a **`main-thread`** section is an MTS body, so its module carries [`MTS_CHUNK_PREAMBLE`] — the
//!   bindings a card's entry has — and `export default` in front of it, because a `.lynx.bundle`'s
//!   body is one expression and what `ReactLynx` calls is that expression's value
//!   (`lynx.loadScript('main-thread', …)(entry)`);
//! - a **`.json`** body is a value, registered verbatim, its own URL telling the loader to parse
//!   it;
//! - everything else is a BTS body, through [`bts_module_source`].
//!
//! # What is not installed
//!
//! The container's own `style_info`. Native applies a lazy bundle's CSS only
//! through `__LoadStyleSheet('CSS')` — the `CSS` custom section — where
//! web-core pushes the `StyleInfo` unscoped during the fetch; this engine
//! follows native (`docs/tracking/deviations.md`). Its `config` is ignored
//! too: page policy is the page's.

use std::collections::BTreeMap;
use std::sync::Arc;

use bobcat_core::{MTS_CHUNK_PREAMBLE, PreparsedStyleSheet};
use bobcat_resources::{ContainerInstaller, Registrar};
use url::Url;

use crate::page::{
    BundleTarget, SourceError, bounded_diagnostic, bts_module_source, looks_like_lynx_xml,
    looks_like_native_bundle, looks_like_web_bundle, named_chunk_url, named_style_url,
};

/// Everything one decoded container answers later, at the URL each answers
/// under.
#[derive(Debug, Default)]
pub struct LazyBundleSources {
    /// One ES module or JSON body per section, as the text registered for it,
    /// ordered by URL. A container carries one body under both a manifest
    /// path and a section name, and a URL is one module per realm, so the two
    /// names come to share one entry.
    pub scripts: Vec<(Url, String)>,
    /// One named stylesheet per `encoding: "CSS"` section.
    pub style_sheets: Vec<(Url, Arc<PreparsedStyleSheet>)>,
}

/// The suffix `native::decode` sorts a main-thread source by, and the plain
/// section name a lazy container's MTS body carries.
const MAIN_THREAD_SECTION: &str = "main-thread";
const MAIN_THREAD_SUFFIX: &str = "__main-thread";

/// Decodes one container's bytes into the resources its sections answer as.
///
/// Pure, so the URL rule and the module shapes are testable without a
/// resource system. `url` is the resolved URL the container was fetched
/// from, which every registration below hangs off.
///
/// # Errors
///
/// [`SourceError`] for bytes that are not a Lynx container. Lynx XML is a
/// *page*, not a container, and is refused rather than half-installed.
pub fn lazy_bundle_sources(url: &Url, bytes: &[u8]) -> Result<LazyBundleSources, SourceError> {
    if looks_like_lynx_xml(bytes) {
        return Err(SourceError::NotAContainer(bounded_diagnostic(
            url.to_string(),
        )));
    }
    let (template, target) = if looks_like_native_bundle(bytes) {
        (
            crate::native::decode(bytes).map_err(|source| SourceError::DecodeNativeBundle {
                input: bounded_diagnostic(url.to_string()),
                source,
            })?,
            BundleTarget::Lynx,
        )
    } else {
        (
            crate::web::decode(bytes).map_err(|source| SourceError::DecodeWebBundle {
                input: bounded_diagnostic(url.to_string()),
                source,
            })?,
            BundleTarget::Web,
        )
    };

    // A lazy container has no page root, so nothing here asks for one. Its
    // Lepus chunks, its string custom sections and its manifest paths are all
    // *bodies*, each at the one URL its name names.
    let mut scripts: BTreeMap<Url, String> = BTreeMap::new();
    let mut register = |name: &str, body: &str| {
        scripts.insert(
            section_url(url, name),
            section_module_source(target, name, body),
        );
    };
    for (name, chunk) in &template.lepus_code {
        register(name, chunk);
    }
    if let Some(serde_json::Value::Object(entries)) = template.custom_sections.as_ref() {
        for (name, section) in entries {
            // A section whose content is not a string carries no body — a
            // named stylesheet's is an object — and so is no module.
            if let Some(content) = section.get("content").and_then(serde_json::Value::as_str) {
                register(name, content);
            }
        }
    }
    // The manifest last, so it wins an equal URL: a native container carries
    // one body under both a manifest path and a section name, and the
    // manifest's is the text `bundle_modules` keeps for the page's own.
    for (path, text) in &template.manifest {
        register(path, text);
    }
    Ok(LazyBundleSources {
        scripts: scripts.into_iter().collect(),
        style_sheets: crate::custom_style::named_style_sheets(&template)
            .into_iter()
            .map(|(key, sheet)| (named_style_url(url, &key), sheet))
            .collect(),
    })
}

/// The URL one section name answers at, which is `named_chunk_url`'s with one
/// leading `/` stripped first: a container carries one body under both
/// `background` and `/background`, and they are one URL.
fn section_url(bundle: &Url, name: &str) -> Url {
    named_chunk_url(bundle, name.strip_prefix('/').unwrap_or(name))
}

/// One body as the source registered for it, chosen by its name.
fn section_module_source(target: BundleTarget, name: &str, body: &str) -> String {
    let bare = name.strip_prefix('/').unwrap_or(name);
    if bare == MAIN_THREAD_SECTION || bare.ends_with(MAIN_THREAD_SUFFIX) {
        // An MTS body sees what a card's entry sees. It is one expression,
        // like every `.lynx.bundle` body, so the module's default export is
        // what native's host would have kept as its completion value — which
        // for a lazy bundle's `main-thread` section is the function `ReactLynx`
        // calls with the component entry.
        let mut source =
            String::with_capacity(MTS_CHUNK_PREAMBLE.len() + "export default ".len() + body.len());
        source.push_str(MTS_CHUNK_PREAMBLE);
        source.push_str("export default ");
        source.push_str(body);
        return source;
    }
    if json_path(bare) {
        return body.to_owned();
    }
    bts_module_source(target, body)
}

/// A name the loader would read as JSON: its path's extension and nothing
/// else, as in `bobcat-core`'s own `kind_of`.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "a URL path is case-sensitive, as is Node's own extension match"
)]
fn json_path(name: &str) -> bool {
    name.ends_with(".json")
}

/// The [`ContainerInstaller`] every embedder that decodes Lynx containers
/// installs: [`lazy_bundle_sources`], registered.
///
/// It **sniffs first**, because a plain fetch carries anything: only bytes
/// that open with a native or a web container's magic are decoded, and
/// anything else — XML, an image, a script — leaves the fetch alone by
/// answering `Ok(false)`. What fails a fetch is the narrow case of a
/// container that would not decode.
#[derive(Clone, Copy, Debug, Default)]
pub struct LazyBundleInstaller;

impl ContainerInstaller for LazyBundleInstaller {
    fn install(&self, url: &Url, bytes: &[u8], registrar: &Registrar) -> Result<bool, String> {
        if !looks_like_native_bundle(bytes) && !looks_like_web_bundle(bytes) {
            return Ok(false);
        }
        let sources = lazy_bundle_sources(url, bytes).map_err(|error| error.to_string())?;
        for (url, source) in sources.scripts {
            registrar
                .register(url.as_str(), source, Some("text/javascript; charset=utf-8"))
                .map_err(|error| error.to_string())?;
        }
        for (url, sheet) in sources.style_sheets {
            registrar
                .register_style_sheet(url.as_str(), sheet)
                .map_err(|error| error.to_string())?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("a URL")
    }

    /// The one rule both realms write: a section hangs *under* the container
    /// URL's path, either spelling of a rooted name reaching one URL, and the
    /// container's `?#` suffix is kept.
    #[test]
    fn a_section_hangs_under_the_container_url() {
        let bundle = url("https://cdn.test/lazy-bundle/child.bundle");
        assert_eq!(
            section_url(&bundle, "background").as_str(),
            "https://cdn.test/lazy-bundle/child.bundle/background.js"
        );
        assert_eq!(
            section_url(&bundle, "/background"),
            section_url(&bundle, "background")
        );
        assert_eq!(
            section_url(&bundle, "main-thread").as_str(),
            "https://cdn.test/lazy-bundle/child.bundle/main-thread.js"
        );
        assert_eq!(
            named_style_url(&bundle, "CSS").as_str(),
            "https://cdn.test/lazy-bundle/child.bundle/index.css"
        );
    }

    #[test]
    fn a_query_suffix_is_kept_after_the_section() {
        let bundle = url("https://cdn.test/child.bundle?v=1");
        assert_eq!(
            section_url(&bundle, "background").as_str(),
            "https://cdn.test/child.bundle/background.js?v=1"
        );
        assert_eq!(
            named_style_url(&bundle, "CSS").as_str(),
            "https://cdn.test/child.bundle/index.css?v=1"
        );
    }

    /// The MTS body is the entry's own binding list plus `export default`, and
    /// all of it is one physical line, so the body keeps its line numbering.
    #[test]
    fn a_main_thread_body_is_the_entry_preamble_and_a_default_export() {
        let source = section_module_source(BundleTarget::Lynx, "/main-thread", "(function (e) {})");
        assert!(source.starts_with(MTS_CHUNK_PREAMBLE));
        assert!(source.ends_with("export default (function (e) {})"));
        assert_eq!(
            source.lines().count(),
            1,
            "a body starts on the line it started on in the container"
        );
        assert_eq!(
            section_module_source(BundleTarget::Lynx, "child__main-thread", "1"),
            section_module_source(BundleTarget::Lynx, "main-thread", "1"),
            "the suffix a native container sorts a main-thread source by names the same shape"
        );
    }

    #[test]
    fn a_background_body_is_the_bts_module_its_container_calls_for() {
        assert_eq!(
            section_module_source(BundleTarget::Lynx, "background", "(function(){})()"),
            bts_module_source(BundleTarget::Lynx, "(function(){})()")
        );
        let web = section_module_source(BundleTarget::Web, "background", "module.exports = 1;");
        assert!(
            web.contains("const module = {exports: {}}"),
            "a web-target body gets the CommonJS adapter"
        );
    }

    #[test]
    fn a_json_body_is_registered_verbatim() {
        assert_eq!(
            section_module_source(BundleTarget::Lynx, "/data.json", r#"{"a":1}"#),
            r#"{"a":1}"#
        );
    }

    /// Lynx XML is a page, not a container: a direct caller of the pure
    /// function is told so rather than getting a decoder's bad-magic error.
    #[test]
    fn lynx_xml_is_not_a_container() {
        let error = lazy_bundle_sources(&url("app:///page.xml"), b"<view></view>")
            .expect_err("XML is refused");
        assert!(matches!(error, SourceError::NotAContainer(_)));
    }

    /// What the installer decodes at all: a plain fetch carries anything, so
    /// only the two container magics are its business, and everything else is
    /// a fetch it leaves alone.
    #[test]
    fn only_a_container_magic_is_installed() {
        // The two little-endian magics `web::decode` checks, as bytes.
        assert!(looks_like_web_bundle(b"SDRAWROF\0\0\0\0"));
        assert!(!looks_like_web_bundle(b"SDRA"));
        assert!(!looks_like_web_bundle(b"<view></view>"));
        // An image, a script and a page are all ordinary fetches.
        for bytes in [
            b"\x89PNG\r\n\x1a\n".as_slice(),
            b"export default 1;\n".as_slice(),
            b"<view></view>".as_slice(),
        ] {
            assert!(!looks_like_web_bundle(bytes) && !looks_like_native_bundle(bytes));
        }
    }
}
