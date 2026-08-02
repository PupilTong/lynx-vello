//! Product-facing rendering façade.
//!
//! Embedders drive lifecycle, input, and window events through the types in
//! this module. Scene construction, freshness tracking, GPU submission, and
//! frame pacing stay private: no Vello scene or wgpu device/queue crosses this
//! boundary.

mod headless;
mod pipeline;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod window;

use std::fmt;

pub use headless::HeadlessRenderer;
pub use lynx_element::dom::Point2D;
pub use lynx_element::dom::input::{
    DeltaMode, InputEvent, InputResponse, PointerKind, PointerPhase,
};
use lynx_element::dom::pulsar::gpu::GpuError;
pub use lynx_element::{PageConfig, Viewport};
pub use pipeline::{CapturedFrame, FrameSize, RenderProgram, RenderRuntime};
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use window::WindowRenderer;

/// Why a product renderer could not boot, resize, or produce a frame.
#[derive(Debug)]
#[non_exhaustive]
pub enum RenderError {
    /// The viewport or device-pixel ratio is not renderable.
    InvalidViewport(String),
    /// The built-in JavaScript runtime could not be created.
    RuntimeInitialization(crate::quickjs::QuickJsInitializationError),
    /// The web bundle's main-thread script failed.
    Runtime {
        input: String,
        source: crate::quickjs::MainThreadError,
    },
    /// No graphics adapter is available.
    NoAdapter,
    /// The private graphics backend failed.
    Backend(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidViewport(message) => write!(formatter, "invalid viewport: {message}"),
            Self::RuntimeInitialization(error) => error.fmt(formatter),
            Self::Runtime { input, source } => {
                write!(formatter, "could not run web bundle `{input}`: {source}")
            }
            Self::NoAdapter => formatter.write_str("no usable GPU adapter"),
            Self::Backend(message) => write!(formatter, "render backend failed: {message}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RuntimeInitialization(error) => Some(error),
            Self::Runtime { source, .. } => Some(source),
            Self::InvalidViewport(_) | Self::NoAdapter | Self::Backend(_) => None,
        }
    }
}

impl From<GpuError> for RenderError {
    fn from(error: GpuError) -> Self {
        match error {
            GpuError::NoAdapter => Self::NoAdapter,
            GpuError::Render(message) => Self::Backend(message),
        }
    }
}
