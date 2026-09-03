//! Loading a `.web.bundle` or raw Lynx XML into the values the engine consumes.
//!
//! This is embedder IO: read and sniff the bytes, decode the selected source
//! container, register its scripts and author CSS with the reference resource
//! system, and hand the root URL plus page config to Bobcat. The pipeline
//! itself — tree, commits, style, layout, paint, scheduling — remains the
//! engine's, and everything else a page fetches — its images, above all —
//! goes through `bobcat-resources` like any other embedder's would.

use std::sync::Arc;

use bobcat_core::{PageConfig, PreparsedStyleSheet, ViewSources};
use bobcat_resources::{DiskCacheConfig, Resources, ResourcesConfig};
use url::Url;

use crate::CliError;

/// The disk tier the runner keeps under the user's cache directory.
const DISK_CACHE_BUDGET: u64 = 256 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct Program {
    pub(crate) input: String,
    /// The input's own URL: what a relative `url(...)` in its CSS resolves
    /// against.
    input_url: Url,
    script_url: Url,
    script: Arc<str>,
    /// The conventional background chunk URL for raw XML that contains a
    /// background section. Its presence is retained even when the source is
    /// empty; the current runtime does not execute it.
    background_script: Option<(Url, Arc<str>)>,
    style_sheet: Option<(Url, ProgramStyleSheet)>,
    /// The non-global CSS fragment ids a decoded bundle carries, if any.
    scoped_css_ids: Vec<i32>,
    pub(crate) config: PageConfig,
}

/// Author CSS in the most direct form supplied by the input container.
#[derive(Clone, Debug)]
enum ProgramStyleSheet {
    /// Verbatim UTF-8 CSS from a raw Lynx XML `<style>` section.
    Text(Arc<str>),
    /// A web bundle's rkyv `StyleInfo`, lowered without reserializing it.
    Preparsed(PreparsedStyleSheet),
}

impl Program {
    pub(crate) fn load(input: &Url) -> Result<Self, CliError> {
        let path = input
            .to_file_path()
            .map_err(|()| CliError::InputUrl(input.to_string()))?;
        let bytes = std::fs::read(&path).map_err(|source| CliError::ReadInput {
            path: path.clone(),
            source,
        })?;
        Self::from_bytes(input, &bytes)
    }

    fn from_bytes(input: &Url, bytes: &[u8]) -> Result<Self, CliError> {
        if looks_like_lynx_xml(bytes) {
            Self::from_lynx_xml(input, bytes)
        } else {
            Self::from_web_bundle(input, bytes)
        }
    }

    fn from_web_bundle(input: &Url, bytes: &[u8]) -> Result<Self, CliError> {
        let mut template =
            lynx_template_decoder::decode(bytes).map_err(|source| CliError::Decode {
                input: input.to_string(),
                source,
            })?;
        let source = template
            .lepus_code
            .remove("root")
            .ok_or_else(|| CliError::MissingRoot(input.to_string()))?;
        let script_url = Url::parse("bobcat-memory://bundle/lepus-root.js")
            .expect("the built-in root-script URL must be valid");
        let style_sheet = template
            .style_info
            .as_ref()
            .map(crate::style_info::to_preparsed_style_sheet)
            .filter(|sheet| !sheet.is_empty())
            .map(|sheet| {
                let url = Url::parse("bobcat-memory://bundle/style-info.css")
                    .expect("the built-in stylesheet URL must be valid");
                (url, ProgramStyleSheet::Preparsed(sheet))
            });
        let scoped_css_ids = template
            .style_info
            .as_ref()
            .map_or_else(Vec::new, |style_info| {
                let mut ids: Vec<i32> = style_info
                    .css_id_to_style_sheet
                    .keys()
                    .copied()
                    .filter(|id| *id != 0)
                    .collect();
                ids.sort_unstable();
                ids
            });
        let config = PageConfig {
            default_display_linear: template.config_flag("defaultDisplayLinear"),
            default_overflow_visible: template.config_flag("defaultOverflowVisible"),
            enable_css_selector: template.config_flag("enableCSSSelector"),
        };
        Ok(Self {
            input: input.to_string(),
            input_url: input.clone(),
            script_url,
            script: Arc::from(source),
            background_script: None,
            style_sheet,
            scoped_css_ids,
            config,
        })
    }

    fn from_lynx_xml(input: &Url, bytes: &[u8]) -> Result<Self, CliError> {
        let source =
            std::str::from_utf8(bytes).map_err(|source| CliError::InvalidLynxXmlEncoding {
                input: input.to_string(),
                source,
            })?;
        let xml = lynx_xml::parse(source).map_err(|source| CliError::ParseLynxXml {
            input: input.to_string(),
            source,
        })?;
        let script_url = Url::parse("bobcat-memory://lynx-xml/main-thread.js")
            .expect("the built-in XML main-script URL must be valid");
        let background_script = xml.background_thread_script.map(|source| {
            let url = Url::parse("bobcat-memory://lynx-xml/app-service.js")
                .expect("the built-in XML background-script URL must be valid");
            (url, Arc::from(source))
        });
        let style_sheet = xml.style.map(|source| {
            let url = Url::parse("bobcat-memory://lynx-xml/style.css")
                .expect("the built-in XML stylesheet URL must be valid");
            (url, ProgramStyleSheet::Text(Arc::from(source)))
        });

        Ok(Self {
            input: input.to_string(),
            input_url: input.clone(),
            script_url,
            script: Arc::from(xml.main_thread_script),
            background_script,
            style_sheet,
            scoped_css_ids: Vec::new(),
            config: PageConfig {
                default_display_linear: false,
                default_overflow_visible: false,
                enable_css_selector: true,
            },
        })
    }

    /// The URL the input's author CSS is registered under, if it carried any.
    fn style_sheet_url(&self) -> Option<&Url> {
        self.style_sheet.as_ref().map(|(url, _)| url)
    }

    /// The sources a view for this input is built from: the author CSS this
    /// input carried, if any, and its entry MTS module.
    pub(crate) fn sources(&self) -> ViewSources {
        ViewSources {
            config: self.config,
            style_sheets: self
                .style_sheet_url()
                .map(Url::to_string)
                .into_iter()
                .collect(),
            ..ViewSources::new(self.script_url.to_string())
        }
    }

    /// Builds the resource system this input is served by: the reference
    /// fetcher with the input's extracted sources registered under their
    /// `bobcat-memory://` URLs, the input's own URL as the base every
    /// relative `url(...)` resolves against, and a disk tier under the
    /// user's cache directory. Everything else a page names — a file beside
    /// the input, a `data:` image, an `https:` one — the system fetches and
    /// decodes itself.
    ///
    /// `wakeup` is what a load completing on a worker thread calls; it must
    /// wake the event loop that pumps the view.
    pub(crate) fn resources(&self, wakeup: impl Fn() + Send + Sync + 'static) -> Resources {
        self.resources_with(
            DiskCacheConfig::at_default_location(DISK_CACHE_BUDGET),
            wakeup,
        )
    }

    fn resources_with(
        &self,
        disk_cache: Option<DiskCacheConfig>,
        wakeup: impl Fn() + Send + Sync + 'static,
    ) -> Resources {
        let config = ResourcesConfig {
            base_url: Some(self.input_url.clone()),
            disk_cache,
            ..ResourcesConfig::default()
        };
        let resources = Resources::new(config, wakeup);
        let register = |url: &Url, source: &Arc<str>, media_type: &str| {
            resources
                .register(url.as_str(), source.as_bytes().to_vec(), Some(media_type))
                .expect("the built-in URLs are valid");
        };
        register(
            &self.script_url,
            &self.script,
            "text/javascript; charset=utf-8",
        );
        if let Some((url, source)) = self.background_script.as_ref() {
            register(url, source, "text/javascript; charset=utf-8");
        }
        match self.style_sheet.as_ref() {
            Some((url, ProgramStyleSheet::Text(source))) => {
                register(url, source, "text/css; charset=utf-8");
            }
            Some((url, ProgramStyleSheet::Preparsed(sheet))) => {
                resources
                    .register_style_sheet(url.as_str(), sheet.clone())
                    .expect("the built-in URLs are valid");
            }
            None => {}
        }
        for note in resources.take_notes() {
            eprintln!("bobcat: warning: {note}");
        }
        resources
    }

    /// Reports input features the current runtime retains only approximately
    /// or cannot execute yet.
    ///
    /// A bundle compiled with `enableRemoveCSSScope = false` keeps one
    /// fragment per component and expects each fragment's rules to match only
    /// inside that component. Per-component scoping is not implemented, so
    /// those rules mount globally and two components that style the same class
    /// name will collide. Rendering them is still better than rendering
    /// nothing, but it is not silent.
    pub(crate) fn warn_about_compatibility_limits(&self) {
        if !self.scoped_css_ids.is_empty() {
            let ids: Vec<String> = self
                .scoped_css_ids
                .iter()
                .map(ToString::to_string)
                .collect();
            eprintln!(
                "bobcat: warning: {} carries component-scoped CSS fragments (css ids {}); per-component \
                 scoping is not implemented, so their rules apply globally",
                self.input,
                ids.join(", ")
            );
        }
        if let Some((url, _)) = self.background_script.as_ref() {
            eprintln!(
                "bobcat: warning: {} carries a Lynx XML background-thread script at {}; \
                 background-thread JavaScript is retained but not executed",
                self.input,
                url.path()
            );
        }
    }
}

/// Mirrors web-core's raw-input classification: any run of ASCII whitespace
/// and UTF-8 BOMs is ignored for sniffing, while the XML parser itself remains
/// responsible for enforcing its stricter single-leading-BOM grammar.
fn looks_like_lynx_xml(mut bytes: &[u8]) -> bool {
    const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

    loop {
        let original_len = bytes.len();
        bytes = bytes.trim_ascii_start();
        if let Some(rest) = bytes.strip_prefix(UTF8_BOM) {
            bytes = rest;
        }
        if bytes.len() == original_len {
            break;
        }
    }
    bytes.first() == Some(&b'<')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_url() -> Url {
        Url::parse("file:///tmp/card.lynx.xml").expect("test URL")
    }

    #[test]
    fn sniff_ignores_ascii_whitespace_and_utf8_boms() {
        assert!(looks_like_lynx_xml(b"<lynx"));
        assert!(looks_like_lynx_xml(b"\xef\xbb\xbf \n\t<lynx"));
        assert!(looks_like_lynx_xml(b" \xef\xbb\xbf\n\xef\xbb\xbf<lynx"));
        assert!(!looks_like_lynx_xml(b"SDRA WROF"));
        assert!(!looks_like_lynx_xml(b"  {\"json\":true}"));
    }

    #[test]
    fn xml_preserves_present_empty_style_and_background_sections() {
        let program = Program::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><style></style><script thread=\"main\">main</script><script thread=\"background\"></script></lynx>",
        )
        .expect("valid XML program");

        assert!(!program.config.default_display_linear);
        assert!(!program.config.default_overflow_visible);
        assert!(program.config.enable_css_selector);
        assert_eq!(program.style_sheet_url().map(Url::path), Some("/style.css"));
        assert_eq!(
            program
                .background_script
                .as_ref()
                .map(|(url, _)| url.path()),
            Some("/app-service.js")
        );
        assert!(matches!(
            program.style_sheet.as_ref(),
            Some((_, ProgramStyleSheet::Text(source))) if source.is_empty()
        ));
        assert!(matches!(
            program.background_script.as_ref(),
            Some((_, source)) if source.is_empty()
        ));
    }

    #[test]
    fn xml_without_optional_sections_keeps_them_absent() {
        let program = Program::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><script thread=\"main\">main</script></lynx>",
        )
        .expect("valid XML program");

        assert!(program.style_sheet_url().is_none());
        assert!(program.background_script.is_none());
    }

    #[test]
    fn sniffed_xml_rejects_invalid_utf8_before_parsing() {
        let error = Program::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><script thread=\"main\">\xff</script></lynx>",
        )
        .expect_err("invalid UTF-8 must fail");

        assert!(matches!(error, CliError::InvalidLynxXmlEncoding { .. }));
    }

    #[test]
    fn malformed_sniffed_xml_reports_an_xml_parse_error() {
        let error = Program::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><script thread=\"main\">main</script>",
        )
        .expect_err("malformed XML must fail");

        assert!(matches!(error, CliError::ParseLynxXml { .. }));
    }

    /// The extracted sources reach the resource system under their
    /// `bobcat-memory://` URLs, and the input URL is the base for the rest.
    #[test]
    fn the_program_registers_its_sources_with_the_resource_system() {
        let program = Program::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><style>a{}</style><script thread=\"main\">main</script></lynx>",
        )
        .expect("valid XML program");
        let resources = program.resources_with(None, || {});
        assert_eq!(resources.base_url(), Some(input_url()));
        assert!(resources.unregister("bobcat-memory://lynx-xml/main-thread.js"));
        assert!(resources.unregister("bobcat-memory://lynx-xml/style.css"));
        assert!(!resources.unregister("bobcat-memory://lynx-xml/app-service.js"));
    }
}
