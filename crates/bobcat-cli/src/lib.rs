//! `bobcat` command-line runner — an embedder of [`bobcat_core::LynxView`].
//!
//! The CLI owns exactly the embedder's share: argument parsing, local
//! `file:///` input bytes, OS initialization (the macOS window and its event
//! loop, the debugger-like command prompt), device metrics, input relay, the
//! draw target, and PNG output. The pipeline — tree, commits, style, layout,
//! paint, frame scheduling, the script and render threads — is the engine's;
//! every CLI event handler is a relay into it.
//!
//! It uses `bobcat-resources` for the decoded input and everything that input
//! names — files beside it, data URLs, HTTP resources, and images — including
//! the embedder-owned caches and image decoding that feed painter turns.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The workspace-wide `unsafe_code = "warn"` says a block may exist; this says
// it must explain itself. Crate-local rather than workspace-wide because `dom`
// and `quickjs-rust-bridge` are not at this bar yet.
#![warn(clippy::undocumented_unsafe_blocks)]

use std::ffi::OsString;
use std::path::PathBuf;

mod args;
mod command;
mod headless;
#[cfg(target_os = "macos")]
mod macos;
mod page;
mod screenshot;
#[cfg(target_os = "macos")]
mod vsync;

pub use screenshot::ScreenshotError;

pub const USAGE: &str = "\
Usage:
  bobcat -i <file:///absolute/path/to/input> [OPTIONS]

Options:
  -i, --input URL       local web bundle or Lynx XML URL (file:/// only)
      --headless        render without opening a window
      --vsync FPS       headless frame-clock rate, 1..1000 (default: 60)
      --viewport WxH    initial CSS-pixel viewport (default: 393x727)
      --dpr RATIO       headless device-pixel ratio (default: 1)
  -h, --help            show this help
  -V, --version         show the version

bobcat accepts debugger-style commands on stdin. Use `screenshot [PATH]`
at the (bobcat) prompt to capture the live renderer, and enter `help` for
the full command list. Headed mode is currently available only on macOS.";

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CliError {
    #[error("{0}")]
    Arguments(String),
    #[error("the input URL cannot be represented as a local path: {0}")]
    InputUrl(String),
    #[error("could not read input `{path}`: {source}")]
    ReadInput {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    PageSource(#[from] bobcat_source::SourceError),
    #[error("could not run input `{input}`: {source}")]
    Script {
        input: String,
        #[source]
        source: bobcat_core::script::ScriptError,
    },
    #[error("could not start input `{input}`: {source}")]
    StartView {
        input: String,
        #[source]
        source: bobcat_core::LynxViewError,
    },
    #[error(transparent)]
    Engine(#[from] bobcat_core::EngineError),
    #[error("could not start the command console: {0}")]
    Console(#[source] std::io::Error),
    #[error("could not write screenshot `{path}`: {source}")]
    Screenshot {
        path: PathBuf,
        #[source]
        source: ScreenshotError,
    },
    #[error("could not run the macOS window: {0}")]
    Window(String),
    #[error("headed mode is currently supported only on macOS; use `--headless` on this platform")]
    UnsupportedHeadedPlatform,
}

impl CliError {
    pub(crate) fn arguments(message: impl Into<String>) -> Self {
        Self::Arguments(message.into())
    }

    #[must_use]
    pub const fn is_argument_error(&self) -> bool {
        matches!(self, Self::Arguments(_))
    }

    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        if self.is_argument_error() { 2 } else { 1 }
    }
}

/// Runs Bobcat with arguments from the current process.
pub fn run_from_env() -> Result<(), CliError> {
    run(std::env::args_os().skip(1))
}

/// Runs Bobcat with an explicit argument sequence.
pub fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<(), CliError> {
    match args::parse(arguments)? {
        args::Invocation::Help => {
            println!("{USAGE}");
            Ok(())
        }
        args::Invocation::Version => {
            println!("bobcat {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        args::Invocation::Run(options) => {
            let program = page::Program::load(&options.input)?;
            if options.headless {
                headless::run(&program, &options)
            } else {
                run_headed(program, &options)
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn run_headed(program: page::Program, options: &args::Options) -> Result<(), CliError> {
    macos::run(program, options)
}

#[cfg(not(target_os = "macos"))]
fn run_headed(_program: page::Program, _options: &args::Options) -> Result<(), CliError> {
    Err(CliError::UnsupportedHeadedPlatform)
}
