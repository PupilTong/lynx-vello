//! `bobcat` command-line runner.
//!
//! The CLI owns process concerns — argument parsing, local `file:///` input,
//! the debugger-like command prompt, PNG output, and native window events.
//! [`bobcat_core::renderer`] owns frame scheduling, scene freshness, and GPU
//! presentation for both headed and headless execution.

use std::ffi::OsString;
use std::path::PathBuf;

mod args;
mod command;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod headed;
mod headless;
mod page;
mod screenshot;

pub use screenshot::ScreenshotError;

pub const USAGE: &str = "\
Usage:
  bobcat -i <file:///absolute/path/to/card.web.bundle> [OPTIONS]

Options:
  -i, --input URL       local web-bundle URL (only file:/// is supported)
      --headless        render without opening a window
      --vsync FPS       headless frame-clock rate, 1..1000 (default: 60)
      --viewport WxH    initial CSS-pixel viewport (default: 393x727)
      --dpr RATIO       headless device-pixel ratio (default: 1)
  -h, --help            show this help
  -V, --version         show the version

bobcat accepts debugger-style commands on stdin. Use `screenshot [PATH]`
at the (bobcat) prompt to capture the live renderer, and enter `help` for
the full command list. Headed mode is available on macOS and Linux.";

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
    #[error("could not decode web bundle `{input}`: {source}")]
    Decode {
        input: String,
        #[source]
        source: lynx_template_decoder::DecodeError,
    },
    #[error("web bundle `{0}` has no `lepusCode.root` entry")]
    MissingRoot(String),
    #[error(transparent)]
    Renderer(#[from] bobcat_core::renderer::RenderError),
    #[error("could not start the command console: {0}")]
    Console(#[source] std::io::Error),
    #[error("could not write screenshot `{path}`: {source}")]
    Screenshot {
        path: PathBuf,
        #[source]
        source: ScreenshotError,
    },
    #[error("could not run the native window: {0}")]
    Window(String),
    #[error(
        "headed mode is currently supported only on macOS and Linux; use `--headless` on this \
         platform"
    )]
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

/// Parses the process arguments and runs Bobcat.
pub fn run_from_env() -> Result<(), CliError> {
    run(std::env::args_os().skip(1))
}

/// Parses `arguments` (excluding the executable name) and runs Bobcat.
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
                headless::run(program, &options)
            } else {
                run_headed(program, &options)
            }
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run_headed(program: page::Program, options: &args::Options) -> Result<(), CliError> {
    headed::run(program, options)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn run_headed(_program: page::Program, _options: &args::Options) -> Result<(), CliError> {
    Err(CliError::UnsupportedHeadedPlatform)
}
