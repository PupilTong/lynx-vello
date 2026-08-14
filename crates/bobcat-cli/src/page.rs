//! Loading a `.web.bundle` into the plain values the engine consumes.
//!
//! This is embedder IO: read the bytes, decode the template, and hand the
//! script source and page config to [`bobcat_core::engine::Engine`]. The
//! pipeline itself — tree, commits, style, layout, paint, scheduling — is
//! the engine's, not this crate's.

use bobcat_core::tree::PageConfig;
use url::Url;

use crate::CliError;

#[derive(Debug)]
pub(crate) struct Program {
    pub(crate) input: String,
    pub(crate) source: String,
    pub(crate) config: PageConfig,
    author_rule_count: usize,
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
        let author_rule_count = template.style_info.as_ref().map_or(0, |style_info| {
            style_info
                .css_id_to_style_sheet
                .values()
                .map(|sheet| sheet.rules.len())
                .sum()
        });
        Ok(Self {
            input: input.to_string(),
            source,
            config,
            author_rule_count,
        })
    }

    pub(crate) fn warn_about_dropped_author_rules(&self) {
        if self.author_rule_count != 0 {
            eprintln!(
                "bobcat: warning: {} contains {} decoded author rule(s), but StyleInfo ingestion \
                 is not implemented yet; author styles are omitted",
                self.input, self.author_rule_count
            );
        }
    }
}
