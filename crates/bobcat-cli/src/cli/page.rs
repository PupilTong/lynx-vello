//! Local-file adaptation around [`bobcat_source::PageSource`].
//!
//! `bobcat-source` owns container sniffing, decoding and `StyleInfo` lowering.
//! This CLI remains the embedder: it reads the file, configures the shared
//! reference resource system, registers the decoded in-memory sources, and
//! chooses the cache and wakeup policy for the view it owns.

use bobcat_core::ViewSources;
use bobcat_resources::{DiskCacheConfig, Resources, ResourcesConfig};
use bobcat_source::PageSource;
use url::Url;

use crate::cli::CliError;

/// The disk tier the runner keeps under the user's cache directory.
const DISK_CACHE_BUDGET: u64 = 256 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct Program {
    pub(crate) input: String,
    source: PageSource,
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
        let source = PageSource::from_bytes(input, bytes)?;
        Ok(Self {
            input: source.input_url().to_string(),
            source,
        })
    }

    pub(crate) fn sources(&self) -> ViewSources {
        self.source.view_sources()
    }

    /// Builds this embedder's resource system and registers the script and
    /// author CSS extracted from the input. Relative resource references use
    /// the input URL as their base; everything else is fetched and decoded by
    /// `bobcat-resources`.
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
        let resources = Resources::new(
            ResourcesConfig {
                base_url: Some(self.source.input_url().clone()),
                disk_cache,
                ..ResourcesConfig::default()
            },
            wakeup,
        );
        self.source.register_with(&resources);
        for note in resources.take_notes() {
            eprintln!("bobcat: warning: {note}");
        }
        resources
    }

    /// Preserves the CLI's warning wording while the source crate exposes
    /// structured warnings that other embedders can handle differently.
    pub(crate) fn warn_about_compatibility_limits(&self) {
        for warning in self.source.compatibility_warnings() {
            eprintln!("bobcat: warning: {} carries {warning}", self.input);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_url() -> Url {
        Url::parse("file:///tmp/card.lynx.xml").expect("test URL")
    }

    /// The extracted sources reach the reference resource system under their
    /// private memory URLs, and the input URL is the base for the rest.
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
