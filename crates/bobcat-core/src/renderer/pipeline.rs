use std::cell::Ref;

use dom::input;
use lynx_element::{ElementTree, PageConfig, Viewport};
use pulsar::vello::Scene;

use super::RenderError;
use crate::quickjs::MainThreadRuntime;

const MAX_RENDER_DIMENSION: u32 = 16_384;

/// A physical render-target size in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

/// A CPU-readable frame returned only when an embedder explicitly captures.
#[derive(Debug)]
pub struct CapturedFrame {
    size: FrameSize,
    pixels: Vec<u8>,
}

impl CapturedFrame {
    pub(super) const fn new(size: FrameSize, pixels: Vec<u8>) -> Self {
        Self { size, pixels }
    }

    #[must_use]
    pub const fn size(&self) -> FrameSize {
        self.size
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }
}

/// A booted native runtime whose rendering machinery is intentionally not
/// exposed. Pair it with [`super::HeadlessRenderer`] or
/// [`super::WindowRenderer`].
pub struct RenderRuntime {
    runtime: MainThreadRuntime,
    viewport: Viewport,
    frame_size: FrameSize,
}

/// A decoded main-thread program ready to boot into either a window-derived
/// or explicitly configured viewport.
pub struct RenderProgram {
    input: String,
    source: String,
    config: PageConfig,
}

impl std::fmt::Debug for RenderProgram {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RenderProgram")
            .field("input", &self.input)
            .field("source_len", &self.source.len())
            .field("config", &self.config)
            .finish()
    }
}

impl RenderProgram {
    #[must_use]
    pub fn new(input: impl Into<String>, source: impl Into<String>, config: PageConfig) -> Self {
        Self {
            input: input.into(),
            source: source.into(),
            config,
        }
    }

    /// Boots the program at an explicit viewport (the headless path).
    pub fn boot(self, viewport: Viewport) -> Result<RenderRuntime, RenderError> {
        RenderRuntime::boot(self, viewport)
    }
}

impl std::fmt::Debug for RenderRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RenderRuntime")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .finish_non_exhaustive()
    }
}

impl RenderRuntime {
    /// Boots one main-thread web-bundle program at the requested viewport.
    pub fn new(
        input: impl Into<String>,
        source: &str,
        config: PageConfig,
        viewport: Viewport,
    ) -> Result<Self, RenderError> {
        Self::boot(RenderProgram::new(input, source, config), viewport)
    }

    fn boot(program: RenderProgram, viewport: Viewport) -> Result<Self, RenderError> {
        let frame_size = frame_size(viewport.width, viewport.height, viewport.device_pixel_ratio)?;
        let mut runtime = MainThreadRuntime::new(ElementTree::new(viewport, program.config))
            .map_err(RenderError::RuntimeInitialization)?;
        runtime
            .run_main_thread_script(&program.source)
            .map_err(|source| RenderError::Runtime {
                input: program.input,
                source,
            })?;
        Ok(Self {
            runtime,
            viewport,
            frame_size,
        })
    }

    /// Routes host input and performs the resolved UA default action.
    pub fn handle_input(&mut self, event: input::InputEvent) -> input::InputResponse {
        self.runtime.elements_mut().handle_input(event)
    }

    /// Registers font data for text measurement.
    pub fn register_fonts(&mut self, bytes: &[u8]) -> usize {
        self.runtime.elements_mut().register_fonts(bytes)
    }

    pub(super) fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(), RenderError> {
        let next_size = frame_size(width, height, device_pixel_ratio)?;
        let size_changed = self.viewport.width.to_bits() != width.to_bits()
            || self.viewport.height.to_bits() != height.to_bits();
        let scale_changed =
            self.viewport.device_pixel_ratio.to_bits() != device_pixel_ratio.to_bits();
        if !size_changed && !scale_changed {
            return Ok(());
        }

        {
            let mut elements = self.runtime.elements_mut();
            if size_changed {
                elements.set_viewport(width, height);
            }
            if scale_changed {
                elements.set_device_pixel_ratio(device_pixel_ratio);
            }
        }
        self.viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        self.frame_size = next_size;
        Ok(())
    }

    pub(super) fn prepare_frame(&mut self) -> PreparedFrame<'_> {
        let changed = self
            .runtime
            .elements_mut()
            .document_mut()
            .render_if_needed();
        PreparedFrame {
            elements: self.runtime.elements(),
            size: self.frame_size,
            changed,
        }
    }

    pub(super) fn needs_frame(&self) -> bool {
        self.runtime.elements().document().needs_render()
    }
}

pub(super) struct PreparedFrame<'a> {
    elements: Ref<'a, ElementTree>,
    pub(super) size: FrameSize,
    pub(super) changed: bool,
}

impl PreparedFrame<'_> {
    pub(super) fn scene(&self) -> Ref<'_, Scene> {
        self.elements.document().scene()
    }
}

fn frame_size(width: f32, height: f32, device_pixel_ratio: f32) -> Result<FrameSize, RenderError> {
    if !width.is_finite()
        || !height.is_finite()
        || !device_pixel_ratio.is_finite()
        || width <= 0.0
        || height <= 0.0
        || device_pixel_ratio <= 0.0
    {
        return Err(RenderError::InvalidViewport(format!(
            "CSS size and device-pixel ratio must be finite and positive, got \
             {width}\u{d7}{height} at {device_pixel_ratio}\u{d7}"
        )));
    }

    let physical_width = f64::from(width) * f64::from(device_pixel_ratio);
    let physical_height = f64::from(height) * f64::from(device_pixel_ratio);
    if physical_width > f64::from(MAX_RENDER_DIMENSION)
        || physical_height > f64::from(MAX_RENDER_DIMENSION)
    {
        return Err(RenderError::InvalidViewport(format!(
            "the physical render target may not exceed \
             {MAX_RENDER_DIMENSION}\u{d7}{MAX_RENDER_DIMENSION}, got \
             {physical_width:.0}\u{d7}{physical_height:.0}"
        )));
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite positive values were bounded to 16384 immediately above"
    )]
    Ok(FrameSize {
        width: physical_width.round().max(1.0) as u32,
        height: physical_height.round().max(1.0) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::frame_size;

    #[test]
    fn frame_size_applies_the_device_scale_once() {
        let size = frame_size(393.0, 727.0, 2.0).unwrap();
        assert_eq!((size.width, size.height), (786, 1_454));
    }

    #[test]
    fn frame_size_rejects_unbounded_targets() {
        let error = frame_size(20_000.0, 100.0, 1.0).unwrap_err();
        assert!(error.to_string().contains("16384"));
    }
}
