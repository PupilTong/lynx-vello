//! `bobcat` command-line runner.
//!
//! The CLI owns process concerns — argument parsing, local `file:///` input,
//! frame scheduling, the debugger-like command prompt, PNG output, and the
//! macOS window. It directly composes the internal runtime/document/GPU crates;
//! headed and headless paths consume the same retained scene.

use std::ffi::OsString;
use std::path::PathBuf;

mod args;
mod command;
mod headless;
#[cfg(target_os = "macos")]
mod macos;
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
    #[error("could not decode web bundle `{input}`: {source}")]
    Decode {
        input: String,
        #[source]
        source: lynx_template_decoder::DecodeError,
    },
    #[error("web bundle `{0}` has no `lepusCode.root` entry")]
    MissingRoot(String),
    #[error("could not initialize the main-thread runtime: {0}")]
    RuntimeInitialization(#[source] bobcat_core::quickjs::QuickJsInitializationError),
    #[error("could not run web bundle `{input}`: {source}")]
    Runtime {
        input: String,
        #[source]
        source: bobcat_core::quickjs::MainThreadError,
    },
    #[error("invalid viewport: {0}")]
    Viewport(String),
    #[error("{0}")]
    Gpu(#[source] dom::render::gpu::GpuError),
    #[error("could not start the command console: {0}")]
    Console(#[source] std::io::Error),
    #[error("could not write screenshot `{path}`: {source}")]
    Screenshot {
        path: PathBuf,
        #[source]
        source: ScreenshotError,
    },
    #[error("window rendering failed: {0}")]
    Render(String),
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

#[cfg(target_os = "macos")]
fn run_headed(program: page::Program, options: &args::Options) -> Result<(), CliError> {
    macos::run(program, options)
}

#[cfg(not(target_os = "macos"))]
fn run_headed(_program: page::Program, _options: &args::Options) -> Result<(), CliError> {
    Err(CliError::UnsupportedHeadedPlatform)
}
