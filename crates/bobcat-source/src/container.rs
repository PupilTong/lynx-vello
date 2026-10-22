//! One Lynx container, as resources: what every `.web.bundle` and every
//! `.lynx.bundle` becomes — a page's own, a `<frame>`'s, or one
//! `lynx.fetchBundle` fetched.
//!
//! There is **one registration path** for a container, whoever asked for it.
//! [`ContainerSources`] decodes the bytes once and names every body and every
//! stylesheet the container carries by one URL rule, derived from the
//! container's own URL `U`; [`PageSource`](crate::PageSource) registers a
//! page's container through it, and [`BundleInstaller`] — the
//! [`ContainerInstaller`] the reference fetcher offers every fetch's bytes
//! to — registers a fetched one through it. Nothing is evaluated here; a
//! realm loads a body afterwards, synchronously, through the same loader a
//! `require` uses, by the URL it builds for it.
//!
//! # The URL rule
//!
//! Every URL hangs **under** `U`'s path `P` (one trailing `/` of it dropped),
//! with `U`'s `?#` suffix `S` kept after it. A name is encoded the way
//! `form_urlencoded` encodes it, a space as `%20`, after one leading `/` is
//! stripped. Both realms write exactly these strings, over
//! `packages/bobcat-element/src/section-url.ts`:
//!
//! | What | URL | Registered as |
//! |---|---|---|
//! | the Lepus `root` (or the native entry named explicitly, aliased) | `P/root.js S` | [`mts_entry_source`] |
//! | a Lepus chunk or string custom section named `main-thread` / `*__main-thread` | `P/<name>.js S` | [`MTS_CHUNK_PREAMBLE`], `export default `, the body |
//! | any other Lepus chunk | `P/<name>.js S` | verbatim |
//! | any other string custom section | `P/<name>.js S` | a `.json` name verbatim, else [`bts_module_source`] |
//! | a manifest path `p` | `P/<p> S`, no `.js` appended, a `?`/`#` inside `p` escaped | the same choice by name |
//! | in a `.web.bundle` with no entry, a manifest path `/<n>` that is a bare name, again | `P/<n>.js S` | the same choice by name |
//! | a named stylesheet `k` | `P/custom_section/<k>.css S` | a [`PreparsedStyleSheet`] |
//! | the container's own non-empty `StyleInfo` | `P/style-info.css S` | a lowered [`PreparsedStyleSheet`] |
//! | the BTS boot script (every container with an entry) | `U` with its fragment replaced by `#background-thread` | `background_boot_source` |
//!
//! Who asks for each: a page's boot imports `root.js`; MTS's
//! `__LoadLepusChunk` a Lepus chunk and its `lynx.loadScript` (a lazy
//! container's prepare) a `main-thread` section; BTS's
//! `lynx.loadScript(key, {bundleName})` a custom section, and its
//! `lynx.requireModule` and `nativeApp.loadScript` a manifest path;
//! `__LoadStyleSheet` a named stylesheet. A page's view lists its
//! `style-info.css` as its author sheet; a fetched container's is registered,
//! adopted by a `<frame src>` boot into the frame's shadow root, and never
//! asked for by a lazy component (native applies a lazy bundle's CSS only
//! through `__LoadStyleSheet('CSS')`, `docs/tracking/deviations.md`).
//!
//! A `.lynx.bundle`'s decoder maps each custom source section `n` to both
//! `custom_sections[n]` and `manifest["/n"]`, so its body lands at both
//! `P/n.js` and `P/n` — two URLs, and so two module evaluations if a card
//! asks for it by both.
//!
//! A `.web.bundle` assembled from custom sections — a lazy bundle compiled on
//! the `lynx.fetchBundle` path — has no custom section left: the web encoder
//! moves each one that is neither CSS nor bytecode into the manifest as
//! `/<n>` (`WebEncodePlugin.routeSections` in
//! `@lynx-js/template-webpack-plugin`), and the background thread still asks
//! for it by name. Such a container has no entry, and each of its manifest
//! paths that is a bare name — one segment, no extension, as a section's name
//! is — lands at `P/n.js` too, the same two URLs.
//!
//! # Collisions
//!
//! Two names that reach one URL with one text are one registration. Two that
//! reach it with different texts, or a stylesheet URL some script also
//! reaches, are [`SourceError::ConflictingSection`]: no half of a container
//! is registered. A container URL that cannot be a base has no path to hang
//! anything under, and is [`SourceError::OpaqueContainerUrl`].
//!
//! The URLs are equal only while the two bases are: the container URL is
//! resolved by the fetcher against its own base, a section load by the engine
//! against the view's
//! [`ViewSources::base_url`](bobcat_core::ViewSources::base_url), so an
//! embedder gives its fetcher the view's base — the CLI, the server and the
//! browser name a page's input URL as both.

use std::collections::BTreeMap;
use std::sync::Arc;

use bobcat_core::{MTS_CHUNK_PREAMBLE, PageConfig, PreparsedStyleSheet};
use bobcat_resources::{ContainerInstaller, RegisterError, Registrar, Resources};
use url::Url;

use crate::page::{
    BundleTarget, SourceError, background_boot_source, bounded_diagnostic, bts_module_source,
    diagnostic_url, looks_like_lynx_xml, looks_like_native_bundle, looks_like_web_bundle,
    mts_entry_source,
};

/// The Lepus chunk a container's entry is.
const ROOT: &str = "root";
/// The section name a lazy container's MTS body carries, and the suffix
/// `native::decode` sorts a main-thread source by.
const MAIN_THREAD_SECTION: &str = "main-thread";
const MAIN_THREAD_SUFFIX: &str = "__main-thread";
const SCRIPT_MEDIA_TYPE: &str = "text/javascript; charset=utf-8";

/// One decoded container: every body and stylesheet it carries, at the URL
/// each answers under, and what a view booting it as a page is built from.
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerSources {
    url: Url,
    target: BundleTarget,
    /// Every script body, ordered by URL, as the text registered for it.
    scripts: Vec<(Url, Arc<str>)>,
    /// One named stylesheet per CSS custom section, ordered by URL: an entry
    /// of the `CustomSectionsStyleInfo` section, a legacy `encoding: "CSS"`
    /// JSON section, or a `.lynx.bundle`'s CSS custom section.
    style_sheets: Vec<(Url, Arc<PreparsedStyleSheet>)>,
    /// The container's own `StyleInfo`, lowered, when it holds any rule.
    style_info: Option<(Url, Arc<PreparsedStyleSheet>)>,
    entry: Option<Url>,
    background_entry: Option<Url>,
    config: PageConfig,
}

impl ContainerSources {
    /// Decodes one container's bytes, sniffing which of the two encodings
    /// they are. The entry is the Lepus chunk named `root`; a container
    /// without one — a lazy bundle, a native external module — has no entry.
    ///
    /// # Errors
    ///
    /// [`SourceError::NotAContainer`] for Lynx XML, which is a page rather
    /// than a container of sections; a decode error for bytes that are not a
    /// container; [`SourceError::OpaqueContainerUrl`] and
    /// [`SourceError::ConflictingSection`] as the module documentation says.
    pub fn from_bytes(url: &Url, bytes: &[u8]) -> Result<Self, SourceError> {
        if looks_like_lynx_xml(bytes) {
            return Err(SourceError::NotAContainer(diagnostic_url(url)));
        }
        if looks_like_native_bundle(bytes) {
            return Self::from_template(url, &decode_native(url, bytes)?, BundleTarget::Lynx, None);
        }
        let template =
            crate::web::decode(bytes).map_err(|source| SourceError::DecodeWebBundle {
                input: diagnostic_url(url),
                source,
            })?;
        Self::from_template(url, &template, BundleTarget::Web, None)
    }

    /// Decodes a source-based `.lynx.bundle` whose entry is the main-thread
    /// module named `entry` rather than `root`: native external modules carry
    /// no `root`, and nothing guesses one. That chunk is registered under its
    /// own name and, as the entry, at `P/root.js` too.
    ///
    /// # Errors
    ///
    /// [`SourceError::MissingNativeEntry`] when no Lepus chunk is named
    /// `entry`, and every error [`Self::from_bytes`] has.
    pub fn from_native_bundle(url: &Url, bytes: &[u8], entry: &str) -> Result<Self, SourceError> {
        Self::from_template(
            url,
            &decode_native(url, bytes)?,
            BundleTarget::Lynx,
            Some(entry),
        )
    }

    pub(crate) fn from_template(
        url: &Url,
        template: &crate::web::WebTemplate,
        target: BundleTarget,
        entry: Option<&str>,
    ) -> Result<Self, SourceError> {
        if url.cannot_be_a_base() {
            return Err(SourceError::OpaqueContainerUrl(diagnostic_url(url)));
        }
        let entry_name = match entry {
            Some(name) if template.lepus_code.contains_key(name) => Some(name),
            Some(name) => {
                return Err(SourceError::MissingNativeEntry {
                    input: diagnostic_url(url),
                    entry: bounded_diagnostic(name.to_owned()),
                });
            }
            None => template.lepus_code.contains_key(ROOT).then_some(ROOT),
        };

        let mut scripts = Scripts::new(url);
        for (name, body) in &template.lepus_code {
            scripts.insert(named_chunk_url(url, name), lepus_source(name, body))?;
        }
        // A native entry named explicitly is the entry at `root.js` as well
        // as the chunk under its own name.
        if let Some(name) = entry_name.filter(|name| *name != ROOT) {
            scripts.insert(
                named_chunk_url(url, ROOT),
                mts_entry_source(&template.lepus_code[name]),
            )?;
        }
        if let Some(serde_json::Value::Object(sections)) = template.custom_sections.as_ref() {
            for (name, section) in sections {
                // A section whose content is not a string carries no body — a
                // named stylesheet's is an object — and so is no script.
                if let Some(content) = section.get("content").and_then(serde_json::Value::as_str) {
                    scripts.insert(
                        named_chunk_url(url, name),
                        body_source(target, name, content),
                    )?;
                }
            }
        }
        for (path, body) in &template.manifest {
            scripts.insert(bundle_path_url(url, path), body_source(target, path, body))?;
        }
        // A `.web.bundle` with no entry was assembled from custom sections,
        // and the web encoder keeps none of them as one: it routes each
        // section that is neither CSS nor bytecode into the manifest as
        // `/<name>` (`WebEncodePlugin.routeSections`). Whoever asks for such a
        // section asks for it by name, `lynx.loadScript(name, {bundleName})`,
        // so each manifest path that is a bare name — one segment, no
        // extension — answers at its section URL as well, the two URLs a
        // `.lynx.bundle`'s section has. A path with an extension is a file the
        // manifest carries (`/answer.js`), never a section's name.
        if target == BundleTarget::Web && entry_name.is_none() {
            for (path, body) in &template.manifest {
                let name = bare(path);
                if !name.is_empty() && !name.contains(['/', '.']) {
                    scripts.insert(named_chunk_url(url, name), body_source(target, name, body))?;
                }
            }
        }
        let entry = entry_name.map(|_| named_chunk_url(url, ROOT));
        // Every container a view or a `<frame>` can boot has a BTS boot, so
        // whoever boots it connects a BTS to an entry that exists: a card with
        // no manifest gets one that registers the container and loads
        // nothing.
        let background_entry = if entry.is_some() {
            let boot = background_boot_url(url);
            scripts.insert(
                boot.clone(),
                background_boot_source(url, template.manifest.contains_key("/app-service.js")),
            )?;
            Some(boot)
        } else {
            None
        };
        let scripts = scripts.finish();

        let style_sheets: Vec<(Url, Arc<PreparsedStyleSheet>)> =
            crate::custom_style::named_style_sheets(template)
                .into_iter()
                .map(|(key, sheet)| (named_style_url(url, &key), sheet))
                .collect();
        let style_info = template
            .style_info
            .as_ref()
            .map(crate::lower_style::to_preparsed_style_sheet)
            .filter(|sheet| !sheet.is_empty())
            .map(|sheet| (bundle_style_url(url), Arc::new(sheet)));
        for (sheet_url, _) in style_sheets.iter().chain(&style_info) {
            if scripts
                .binary_search_by(|(script_url, _)| script_url.cmp(sheet_url))
                .is_ok()
            {
                return Err(conflict(url, sheet_url));
            }
        }

        Ok(Self {
            url: url.clone(),
            target,
            scripts,
            style_sheets,
            style_info,
            entry,
            background_entry,
            config: PageConfig {
                default_display_linear: template.config_flag("defaultDisplayLinear"),
                default_overflow_visible: template.config_flag("defaultOverflowVisible"),
                enable_css_selector: template.config_flag("enableCSSSelector"),
                enable_js_data_processor: template.config_flag("enableJSDataProcessor"),
                enable_css_inheritance: template.config_flag("enableCSSInheritance"),
            },
        })
    }

    /// The container's own URL, `U`, which everything here hangs under.
    #[must_use]
    pub const fn url(&self) -> &Url {
        &self.url
    }

    /// Which compiler built the container, and so the shape of its BTS bodies.
    #[must_use]
    pub const fn target(&self) -> BundleTarget {
        self.target
    }

    /// The entry a view booting this container imports, `P/root.js S`, or
    /// `None` for a container with no `root`.
    #[must_use]
    pub const fn entry(&self) -> Option<&Url> {
        self.entry.as_ref()
    }

    /// The BTS boot script, `U#background-thread`, for a container with an
    /// entry.
    #[must_use]
    pub const fn background_entry(&self) -> Option<&Url> {
        self.background_entry.as_ref()
    }

    /// The container's own `StyleInfo`, `P/style-info.css S`, when it holds
    /// any rule. A page lists it as its view's author sheet; nothing else asks
    /// for it.
    #[must_use]
    pub fn style_info(&self) -> Option<&Url> {
        self.style_info.as_ref().map(|(url, _)| url)
    }

    /// Page policy decoded from the container's configuration. Only a page's
    /// is read: a lazy container's is ignored, page policy being the page's.
    #[must_use]
    pub const fn config(&self) -> PageConfig {
        self.config
    }

    /// Every script body, ordered by URL, as the text registered for it.
    #[must_use]
    pub fn scripts(&self) -> &[(Url, Arc<str>)] {
        &self.scripts
    }

    /// Every named stylesheet, ordered by URL. The container's own
    /// `StyleInfo` is not among them: see [`Self::style_info`].
    #[must_use]
    pub fn style_sheets(&self) -> &[(Url, Arc<PreparsedStyleSheet>)] {
        &self.style_sheets
    }

    /// Registers every script, named stylesheet and the `StyleInfo` with an
    /// embedder-owned resource system, each replacing an earlier registration
    /// of its URL.
    ///
    /// Every URL was derived from an already-parsed [`Url`], so registration
    /// cannot fail URL validation.
    pub fn register_with(&self, resources: &Resources) {
        self.register(resources)
            .expect("a container's registration URLs are derived from a parsed URL");
    }

    /// Registers the same set through a [`ContainerInstaller`]'s
    /// [`Registrar`].
    ///
    /// # Errors
    ///
    /// What the registrar answers, which for URLs derived from a parsed
    /// [`Url`] is nothing.
    pub fn register_through(&self, registrar: &Registrar) -> Result<(), RegisterError> {
        self.register(registrar)
    }

    fn register(&self, sink: &impl Sink) -> Result<(), RegisterError> {
        for registration in self.registrations() {
            match registration {
                Registration::Script(url, source) => sink.script(url, source)?,
                Registration::StyleSheet(url, sheet) => sink.style_sheet(url, sheet)?,
            }
        }
        Ok(())
    }

    /// The one list both `register_*` drain: the scripts, then the named
    /// stylesheets, then the container's own `StyleInfo`.
    fn registrations(&self) -> impl Iterator<Item = Registration<'_>> {
        self.scripts
            .iter()
            .map(|(url, source)| Registration::Script(url, source))
            .chain(
                self.style_sheets
                    .iter()
                    .chain(&self.style_info)
                    .map(|(url, sheet)| Registration::StyleSheet(url, sheet)),
            )
    }
}

enum Registration<'a> {
    Script(&'a Url, &'a str),
    StyleSheet(&'a Url, &'a Arc<PreparsedStyleSheet>),
}

/// What a registration is written into: the embedder's [`Resources`], or a
/// [`ContainerInstaller`]'s [`Registrar`].
trait Sink {
    fn script(&self, url: &Url, source: &str) -> Result<(), RegisterError>;
    fn style_sheet(&self, url: &Url, sheet: &Arc<PreparsedStyleSheet>)
    -> Result<(), RegisterError>;
}

impl Sink for Resources {
    fn script(&self, url: &Url, source: &str) -> Result<(), RegisterError> {
        self.register(
            url.as_str(),
            source.as_bytes().to_vec(),
            Some(SCRIPT_MEDIA_TYPE),
        )
        .map(drop)
    }

    fn style_sheet(
        &self,
        url: &Url,
        sheet: &Arc<PreparsedStyleSheet>,
    ) -> Result<(), RegisterError> {
        self.register_style_sheet(url.as_str(), Arc::clone(sheet))
            .map(drop)
    }
}

impl Sink for Registrar {
    fn script(&self, url: &Url, source: &str) -> Result<(), RegisterError> {
        self.register(
            url.as_str(),
            source.as_bytes().to_vec(),
            Some(SCRIPT_MEDIA_TYPE),
        )
        .map(drop)
    }

    fn style_sheet(
        &self,
        url: &Url,
        sheet: &Arc<PreparsedStyleSheet>,
    ) -> Result<(), RegisterError> {
        self.register_style_sheet(url.as_str(), Arc::clone(sheet))
            .map(drop)
    }
}

/// The script set being collected: one text per URL, an equal text for an
/// equal URL deduplicated, a different one refused.
struct Scripts<'a> {
    container: &'a Url,
    scripts: BTreeMap<Url, Arc<str>>,
}

impl<'a> Scripts<'a> {
    const fn new(container: &'a Url) -> Self {
        Self {
            container,
            scripts: BTreeMap::new(),
        }
    }

    fn insert(&mut self, url: Url, source: String) -> Result<(), SourceError> {
        match self.scripts.get(&url) {
            Some(existing) if **existing == *source => Ok(()),
            Some(_) => Err(conflict(self.container, &url)),
            None => {
                self.scripts.insert(url, Arc::from(source));
                Ok(())
            }
        }
    }

    fn finish(self) -> Vec<(Url, Arc<str>)> {
        self.scripts.into_iter().collect()
    }
}

fn conflict(container: &Url, url: &Url) -> SourceError {
    SourceError::ConflictingSection {
        input: diagnostic_url(container),
        url: diagnostic_url(url),
    }
}

fn decode_native(url: &Url, bytes: &[u8]) -> Result<crate::web::WebTemplate, SourceError> {
    crate::native::decode(bytes).map_err(|source| SourceError::DecodeNativeBundle {
        input: diagnostic_url(url),
        source,
    })
}

/// A name with one leading `/` stripped, which is how either spelling of a
/// rooted name reaches one URL and one shape.
fn bare(name: &str) -> &str {
    name.strip_prefix('/').unwrap_or(name)
}

fn is_main_thread(name: &str) -> bool {
    name == MAIN_THREAD_SECTION || name.ends_with(MAIN_THREAD_SUFFIX)
}

/// One Lepus chunk as the source registered for it: the entry's module for
/// `root`, a main-thread section's for `main-thread`/`*__main-thread`, and
/// otherwise the chunk verbatim, because the realm compiles a named chunk as
/// a function body whose parameters are the bindings `MTS_CHUNK_PREAMBLE`
/// gives the entry — so an `import` could not appear in it anyway.
fn lepus_source(name: &str, body: &str) -> String {
    let name = bare(name);
    if name == ROOT {
        mts_entry_source(body)
    } else if is_main_thread(name) {
        main_thread_source(body)
    } else {
        body.to_owned()
    }
}

/// One custom section or manifest body as the source registered for it,
/// chosen by its name:
///
/// - a **`main-thread`** body is an MTS body ([`main_thread_source`]);
/// - a **`.json`** body is a value, registered verbatim, its own URL telling the loader to parse
///   it;
/// - everything else is a BTS body, through [`bts_module_source`].
fn body_source(target: BundleTarget, name: &str, body: &str) -> String {
    let name = bare(name);
    if is_main_thread(name) {
        main_thread_source(body)
    } else if json_path(name) {
        body.to_owned()
    } else {
        bts_module_source(target, body)
    }
}

/// An MTS section: the bindings a card's entry has, which
/// [`mts_entry_source`] puts in front of a page's root, and `export default`
/// in front of the body, because a `.lynx.bundle`'s body is one expression
/// and what `ReactLynx` calls is that expression's value
/// (`lynx.loadScript('main-thread', …)(entry)`). One physical line, so the
/// body keeps its line numbering.
fn main_thread_source(body: &str) -> String {
    const EXPORT: &str = "export default ";
    let mut source = String::with_capacity(MTS_CHUNK_PREAMBLE.len() + EXPORT.len() + body.len());
    source.push_str(MTS_CHUNK_PREAMBLE);
    source.push_str(EXPORT);
    source.push_str(body);
    source
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

/// `tail` under the container URL's path, one trailing `/` of that path
/// dropped, the `?#` suffix kept. `Url::set_path` escapes what a path cannot
/// hold, a `?` or `#` among it, exactly as the realm's URL parser escapes the
/// string `section-url.ts` writes.
fn under(container: &Url, tail: &str) -> Url {
    let mut url = container.clone();
    let path = container.path();
    url.set_path(&format!(
        "{}/{tail}",
        path.strip_suffix('/').unwrap_or(path)
    ));
    url
}

/// One name as `form_urlencoded` encodes it, a space as `%20`.
fn encoded(name: &str) -> String {
    url::form_urlencoded::byte_serialize(name.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// `P/<encoded name>.js S`: a Lepus chunk or a custom section, one leading
/// `/` stripped first — `sectionURL` in `section-url.ts`.
fn named_chunk_url(container: &Url, name: &str) -> Url {
    under(container, &format!("{}.js", encoded(bare(name))))
}

/// `P/<path> S`: a manifest path as the path it is, one leading `/` stripped
/// and no `.js` appended — `bundlePathURL` in `section-url.ts`.
fn bundle_path_url(container: &Url, path: &str) -> Url {
    under(container, bare(path))
}

/// `P/custom_section/<encoded key>.css S`: a named stylesheet, so the
/// compiler's `CSS` section is `custom_section/CSS.css` — `styleSheetURL` in
/// `section-url.ts`.
fn named_style_url(container: &Url, key: &str) -> Url {
    under(container, &format!("custom_section/{}.css", encoded(key)))
}

/// `P/style-info.css S`: the container's own `StyleInfo` —
/// `bundleStyleSheetURL` in `section-url.ts`.
fn bundle_style_url(container: &Url) -> Url {
    under(container, "style-info.css")
}

/// `U#background-thread`: the BTS boot script, the container URL with its
/// fragment replaced — `backgroundThreadURL` in `section-url.ts`.
fn background_boot_url(container: &Url) -> Url {
    let mut url = container.clone();
    url.set_fragment(Some("background-thread"));
    url
}

/// The one [`ContainerInstaller`] every embedder that decodes Lynx containers
/// installs: [`ContainerSources::from_bytes`], registered.
///
/// It **sniffs first**, because a plain fetch carries anything: only bytes
/// that open with a native or a web container's magic are decoded, and
/// anything else — XML, an image, a script — leaves the fetch alone by
/// answering `Ok(false)`. What fails a fetch is the narrow case of a
/// container that would not decode or register.
#[derive(Clone, Copy, Debug, Default)]
pub struct BundleInstaller;

impl ContainerInstaller for BundleInstaller {
    fn install(&self, url: &Url, bytes: &[u8], registrar: &Registrar) -> Result<bool, String> {
        if !looks_like_native_bundle(bytes) && !looks_like_web_bundle(bytes) {
            return Ok(false);
        }
        ContainerSources::from_bytes(url, bytes)
            .map_err(|error| error.to_string())?
            .register_through(registrar)
            .map_err(|error| error.to_string())?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE: &str = "https://cdn.test/app/card.web.bundle";

    fn url(text: &str) -> Url {
        Url::parse(text).expect("a URL")
    }

    fn template(
        lepus: &[(&str, &str)],
        manifest: &[(&str, &str)],
        sections: serde_json::Value,
    ) -> crate::web::WebTemplate {
        crate::web::WebTemplate {
            lepus_code: lepus
                .iter()
                .map(|(name, body)| ((*name).to_owned(), (*body).to_owned()))
                .collect(),
            manifest: manifest
                .iter()
                .map(|(path, body)| ((*path).to_owned(), (*body).to_owned()))
                .collect(),
            custom_sections: Some(sections),
            ..crate::web::WebTemplate::default()
        }
    }

    fn script_urls(sources: &ContainerSources) -> Vec<&str> {
        sources
            .scripts()
            .iter()
            .map(|(url, _)| url.as_str())
            .collect()
    }

    fn script<'a>(sources: &'a ContainerSources, at: &str) -> &'a str {
        sources
            .scripts()
            .iter()
            .find(|(url, _)| url.as_str() == at)
            .map_or_else(
                || panic!("no script at {at}: {:?}", script_urls(sources)),
                |(_, source)| source.as_ref(),
            )
    }

    /// The table `packages/bobcat-element/test/section-url.test.ts` pins, row
    /// for row: the two halves agree only while the rule does.
    #[test]
    fn the_container_url_rule_is_the_realms_own() {
        let bundle = url(BUNDLE);
        let at = |suffix: &str| format!("{BUNDLE}{suffix}");
        let with = |suffix: &str| url(&at(suffix));
        // A section is an encoded `.js` file under the container.
        assert_eq!(
            named_chunk_url(&bundle, "background").as_str(),
            at("/background.js")
        );
        assert_eq!(
            named_chunk_url(&bundle, "/background").as_str(),
            at("/background.js")
        );
        assert_eq!(
            named_chunk_url(&bundle, "A &/中").as_str(),
            at("/A%20%26%2F%E4%B8%AD.js")
        );
        assert_eq!(
            named_chunk_url(&bundle, "it's (x)!~*").as_str(),
            at("/it%27s%20%28x%29%21%7E*.js")
        );
        assert_eq!(
            named_chunk_url(&with("?v=1#f"), "main-thread").as_str(),
            at("/main-thread.js?v=1#f")
        );
        assert_eq!(named_chunk_url(&with("/"), "root").as_str(), at("/root.js"));

        // A manifest path is the path it is under the container.
        assert_eq!(
            bundle_path_url(&bundle, "/app-service.js").as_str(),
            at("/app-service.js")
        );
        assert_eq!(
            bundle_path_url(&bundle, "app-service.js").as_str(),
            at("/app-service.js")
        );
        assert_eq!(
            bundle_path_url(&bundle, "/data.json").as_str(),
            at("/data.json")
        );
        assert_eq!(
            bundle_path_url(&bundle, "/chunk.js").as_str(),
            at("/chunk.js")
        );
        assert_eq!(
            bundle_path_url(&with("?rev=1#f"), "/a/b/c.js").as_str(),
            at("/a/b/c.js?rev=1#f")
        );
        assert_eq!(bundle_path_url(&with("/"), "/x.js").as_str(), at("/x.js"));
        assert_eq!(
            bundle_path_url(&bundle, "//x.js").as_str(),
            at("//x.js"),
            "one leading `/` only"
        );
        assert_eq!(
            bundle_path_url(&with("?rev=1"), "/q?x#y.js").as_str(),
            at("/q%3Fx%23y.js?rev=1"),
            "a `?` or `#` inside the path stays in the path"
        );

        // A named stylesheet is under `custom_section/`.
        assert_eq!(
            named_style_url(&bundle, "CSS").as_str(),
            at("/custom_section/CSS.css")
        );
        assert_eq!(
            named_style_url(&with("/#entry"), "A &/中").as_str(),
            at("/custom_section/A%20%26%2F%E4%B8%AD.css#entry")
        );

        // The container's own StyleInfo is `style-info.css`.
        assert_eq!(bundle_style_url(&bundle).as_str(), at("/style-info.css"));
        assert_eq!(
            bundle_style_url(&with("?rev=1")).as_str(),
            at("/style-info.css?rev=1")
        );
        assert_eq!(
            bundle_style_url(&with("/#f")).as_str(),
            at("/style-info.css#f")
        );

        // The BTS boot script is the container itself, at another fragment.
        assert_eq!(
            background_boot_url(&with("?rev=1#f")).as_str(),
            at("?rev=1#background-thread")
        );
        assert_eq!(
            background_boot_url(&with("?rev=1")).as_str(),
            at("?rev=1#background-thread")
        );
        assert_eq!(
            background_boot_url(&with("#a#b")).as_str(),
            at("#background-thread")
        );
        assert_eq!(
            background_boot_url(&with("/")).as_str(),
            at("/#background-thread")
        );
    }

    /// The root is the entry, as the module a page boots: the entry's own
    /// preamble and the body, and no `export default` — whoever fetched it.
    #[test]
    fn a_root_is_the_entry_module_and_names_the_boot() {
        let sources = ContainerSources::from_template(
            &url(BUNDLE),
            &template(
                &[("root", "renderPage();"), ("chunk", "globalThis.ran = 1;")],
                &[("/app-service.js", "module.exports = {};")],
                serde_json::json!({}),
            ),
            BundleTarget::Web,
            None,
        )
        .expect("a page container");

        assert_eq!(
            sources.entry().map(Url::as_str),
            Some(format!("{BUNDLE}/root.js").as_str())
        );
        let root = script(&sources, &format!("{BUNDLE}/root.js"));
        assert_eq!(root, mts_entry_source("renderPage();"));
        assert!(!root.contains("export default"));
        assert_eq!(
            script(&sources, &format!("{BUNDLE}/chunk.js")),
            "globalThis.ran = 1;",
            "a named chunk is verbatim"
        );
        assert_eq!(
            sources.background_entry().map(Url::as_str),
            Some(format!("{BUNDLE}#background-thread").as_str())
        );
        assert_eq!(
            script(&sources, &format!("{BUNDLE}#background-thread")),
            background_boot_source(&url(BUNDLE), true)
        );
    }

    /// A `.web.bundle` assembled from custom sections — a lazy bundle on the
    /// `lynx.fetchBundle` path — carries its background section as the
    /// manifest path `/background`, because the web encoder routes every
    /// section that is neither CSS nor bytecode there
    /// (`@lynx-js/template-webpack-plugin`, `WebEncodePlugin.routeSections`).
    /// The background thread asks for it as the section it was:
    /// `lynx.loadScript('background', {bundleName})`, `P/background.js`. So
    /// it answers at both URLs, as a `.lynx.bundle`'s section does.
    #[test]
    fn a_web_container_without_root_answers_its_routed_sections_by_name() {
        let sources = ContainerSources::from_template(
            &url(BUNDLE),
            &template(
                &[("main-thread", "(function (e) {})")],
                &[("/background", "(function(){})()")],
                serde_json::json!({}),
            ),
            BundleTarget::Web,
            None,
        )
        .expect("a lazy web container");
        assert_eq!(
            script_urls(&sources),
            [
                format!("{BUNDLE}/background"),
                format!("{BUNDLE}/background.js"),
                format!("{BUNDLE}/main-thread.js"),
            ]
        );
        assert_eq!(
            script(&sources, &format!("{BUNDLE}/background.js")),
            script(&sources, &format!("{BUNDLE}/background")),
            "one body, the same module text at both URLs"
        );
    }

    /// A web page has a `root`, and its manifest paths are the paths they
    /// are: `/app-service.js` is not also a section named `app-service.js`.
    #[test]
    fn a_web_page_answers_its_manifest_paths_only_as_paths() {
        let sources = ContainerSources::from_template(
            &url(BUNDLE),
            &template(
                &[("root", "(function (e) {})")],
                &[("/app-service.js", "(function(){})()")],
                serde_json::json!({}),
            ),
            BundleTarget::Web,
            None,
        )
        .expect("a web page");
        assert!(
            !script_urls(&sources).contains(&format!("{BUNDLE}/app-service.js.js").as_str()),
            "{:?}",
            script_urls(&sources)
        );
    }

    /// No `root`, no entry and no boot, whatever else the container carries:
    /// a sole `*__main-thread` chunk is never guessed to be the entry.
    #[test]
    fn a_container_without_root_has_no_entry() {
        let sources = ContainerSources::from_template(
            &url(BUNDLE),
            &template(
                &[("lib__main-thread", "(function (e) {})")],
                &[("/background", "(function(){})()")],
                serde_json::json!({}),
            ),
            BundleTarget::Lynx,
            None,
        )
        .expect("a lazy container");
        assert!(sources.entry().is_none());
        assert!(sources.background_entry().is_none());
        assert_eq!(
            script_urls(&sources),
            [
                format!("{BUNDLE}/background"),
                format!("{BUNDLE}/lib__main-thread.js")
            ]
        );
    }

    /// A native entry named explicitly is registered twice: under its own
    /// name, in the main-thread section's shape, and as the entry at
    /// `root.js`.
    #[test]
    fn an_explicit_native_entry_is_aliased_to_root() {
        let sources = ContainerSources::from_template(
            &url(BUNDLE),
            &template(
                &[("card__main-thread", "boot();")],
                &[],
                serde_json::json!({}),
            ),
            BundleTarget::Lynx,
            Some("card__main-thread"),
        )
        .expect("a native page");
        assert_eq!(
            sources.entry().map(Url::as_str),
            Some(format!("{BUNDLE}/root.js").as_str())
        );
        assert_eq!(
            script(&sources, &format!("{BUNDLE}/root.js")),
            mts_entry_source("boot();")
        );
        assert_eq!(
            script(&sources, &format!("{BUNDLE}/card__main-thread.js")),
            main_thread_source("boot();")
        );
        // No manifest still boots a BTS: one that registers the container
        // and loads nothing.
        assert_eq!(
            sources.background_entry().map(Url::as_str),
            Some(format!("{BUNDLE}#background-thread").as_str())
        );
        assert_eq!(
            script(&sources, &format!("{BUNDLE}#background-thread")),
            background_boot_source(&url(BUNDLE), false)
        );

        let missing = ContainerSources::from_template(
            &url(BUNDLE),
            &template(&[], &[], serde_json::json!({})),
            BundleTarget::Lynx,
            Some("card__main-thread"),
        )
        .expect_err("no such entry");
        assert!(matches!(missing, SourceError::MissingNativeEntry { .. }));
    }

    /// A manifest path hangs under the container as the path it is, while a
    /// custom section of the same name is a `.js` file beside it: a native
    /// container's one body reaches both.
    #[test]
    fn a_manifest_path_is_the_path_it_is() {
        let sources = ContainerSources::from_template(
            &url(&format!("{BUNDLE}?rev=1")),
            &template(
                &[],
                &[
                    ("/background", "(function(){})()"),
                    ("/app-service.js", "(function(){})()"),
                    ("/nested/data.json", r#"{"a":1}"#),
                ],
                serde_json::json!({
                    "background": {"content": "(function(){})()"},
                    "binary": {"content": [1, 2, 3]},
                }),
            ),
            BundleTarget::Lynx,
            None,
        )
        .expect("a container");
        assert_eq!(
            script_urls(&sources),
            [
                format!("{BUNDLE}/app-service.js?rev=1"),
                format!("{BUNDLE}/background.js?rev=1"),
                format!("{BUNDLE}/background?rev=1"),
                format!("{BUNDLE}/nested/data.json?rev=1"),
            ],
            "a section whose content is not a string carries no body to register"
        );
        assert_eq!(
            script(&sources, &format!("{BUNDLE}/nested/data.json?rev=1")),
            r#"{"a":1}"#,
            "a `.json` body is the JSON it is"
        );
        assert_eq!(
            script(&sources, &format!("{BUNDLE}/background?rev=1")),
            bts_module_source(BundleTarget::Lynx, "(function(){})()")
        );
    }

    /// The MTS body is the entry's own binding list plus `export default`, and
    /// all of it is one physical line, so the body keeps its line numbering.
    #[test]
    fn a_main_thread_body_is_the_mts_preamble_and_a_default_export() {
        let source = body_source(BundleTarget::Lynx, "/main-thread", "(function (e) {})");
        assert!(source.starts_with(MTS_CHUNK_PREAMBLE));
        assert!(source.ends_with("export default (function (e) {})"));
        assert_eq!(
            source.lines().count(),
            1,
            "a body starts on the line it started on in the container"
        );
        assert_eq!(
            body_source(BundleTarget::Lynx, "child__main-thread", "1"),
            body_source(BundleTarget::Lynx, "main-thread", "1"),
            "the suffix a native container sorts a main-thread source by names the same shape"
        );
        assert_eq!(
            lepus_source("child__main-thread", "1"),
            body_source(BundleTarget::Web, "child__main-thread", "1"),
            "a Lepus chunk and a custom section of one main-thread name are one shape"
        );
    }

    #[test]
    fn a_background_body_is_the_bts_module_its_container_calls_for() {
        assert_eq!(
            body_source(BundleTarget::Lynx, "background", "(function(){})()"),
            bts_module_source(BundleTarget::Lynx, "(function(){})()")
        );
        let web = body_source(BundleTarget::Web, "background", "module.exports = 1;");
        assert!(
            web.contains("const module = {exports: {}}"),
            "a web-target body gets the CommonJS adapter"
        );
    }

    #[test]
    fn a_json_body_is_registered_verbatim() {
        for target in [BundleTarget::Web, BundleTarget::Lynx] {
            assert_eq!(
                body_source(target, "/data.json", r#"{"a":1}"#),
                r#"{"a":1}"#
            );
            assert_eq!(body_source(target, "data.json", r#"{"a":1}"#), r#"{"a":1}"#);
        }
    }

    /// One URL, one text: an equal text deduplicates, a different one refuses
    /// the whole container, and so does a stylesheet URL a script reaches.
    #[test]
    fn two_texts_at_one_url_conflict() {
        let same = ContainerSources::from_template(
            &url(BUNDLE),
            &template(
                &[],
                &[("/answer.js", "module.exports = 42")],
                serde_json::json!({"answer": {"content": "module.exports = 42"}}),
            ),
            BundleTarget::Web,
            None,
        )
        .expect("one text at one URL is one registration");
        assert_eq!(script_urls(&same), [format!("{BUNDLE}/answer.js")]);

        let different = ContainerSources::from_template(
            &url("https://user:secret@cdn.test/app/card.web.bundle?token=1"),
            &template(
                &[],
                &[("/answer.js", "module.exports = 41")],
                serde_json::json!({"answer": {"content": "module.exports = 42"}}),
            ),
            BundleTarget::Web,
            None,
        )
        .expect_err("two texts at one URL");
        assert!(
            matches!(&different, SourceError::ConflictingSection { input, url }
                if input == BUNDLE && *url == format!("{BUNDLE}/answer.js")),
            "{different:?}"
        );
        assert!(!different.to_string().contains("secret"));

        let sheet = ContainerSources::from_template(
            &url(BUNDLE),
            &template(
                &[],
                &[("/custom_section/CSS.css", "module.exports = 1")],
                serde_json::json!({"CSS": {"encoding": "CSS", "content": {"ruleList": [
                    {"type": "StyleRule", "selectorText": {"value": ".a"}, "style": []}
                ]}}}),
            ),
            BundleTarget::Web,
            None,
        )
        .expect_err("a stylesheet URL a script also reaches");
        assert!(
            matches!(&sheet, SourceError::ConflictingSection { url, .. }
                if *url == format!("{BUNDLE}/custom_section/CSS.css")),
            "{sheet:?}"
        );
    }

    /// A URL that cannot be a base has no path to hang a section under.
    #[test]
    fn an_opaque_container_url_is_refused() {
        let error = ContainerSources::from_template(
            &url("data:application/octet-stream,secret"),
            &template(&[("root", "")], &[], serde_json::json!({})),
            BundleTarget::Web,
            None,
        )
        .expect_err("an opaque URL");
        assert!(
            matches!(&error, SourceError::OpaqueContainerUrl(input) if input == "data:[redacted]"),
            "{error:?}"
        );
    }

    /// Lynx XML is a page, not a container: a direct caller is told so rather
    /// than getting a decoder's bad-magic error.
    #[test]
    fn lynx_xml_is_not_a_container() {
        let error = ContainerSources::from_bytes(&url("app:///page.xml"), b"<view></view>")
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
