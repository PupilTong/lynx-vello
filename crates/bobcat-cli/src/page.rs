//! Loading a `.web.bundle` into the plain values the engine consumes.
//!
//! This is embedder IO: read the bytes, decode the template, and hand the
//! script source and page config to [`bobcat_core::engine::Engine`]. The
//! pipeline itself — tree, commits, style, layout, paint, scheduling — is
//! the engine's, not this crate's.

use bobcat_core::lynx_element::PageConfig;
use url::Url;

use crate::CliError;

#[derive(Debug)]
pub(crate) struct Program {
    pub(crate) input: String,
    pub(crate) source: String,
    pub(crate) config: PageConfig,
    /// The decoded `StyleInfo` section re-serialized as author CSS, empty when
    /// the bundle carried no rules.
    pub(crate) author_css: String,
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
        let mut template =
            lynx_template_decoder::decode(&bytes).map_err(|source| CliError::Decode {
                input: input.to_string(),
                source,
            })?;
        let source = template
            .lepus_code
            .remove("root")
            .ok_or_else(|| CliError::MissingRoot(input.to_string()))?;
        let config = PageConfig {
            default_display_linear: template.config_flag("defaultDisplayLinear"),
            default_overflow_visible: template.config_flag("defaultOverflowVisible"),
            enable_css_selector: template.config_flag("enableCSSSelector"),
        };
        // The `StyleInfo` section is pre-parsed CSS, not CSS source; the
        // decoder re-serializes its own model, and the engine's CSS engine
        // parses the text. Every `cssId` lands in one stylesheet — correct for
        // the `enableRemoveCSSScope` bundles today's toolchain emits, and the
        // recorded limit for scoped ones.
        let author_css = template
            .style_info
            .as_ref()
            .map(lynx_template_decoder::StyleInfo::to_css)
            .unwrap_or_default();
        Ok(Self {
            input: input.to_string(),
            source,
            config,
            author_css,
        })
    }
}
