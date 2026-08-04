//! The paint side of the pipeline: bundle loading, and the frame pipeline
//! the window and headless backends both drive.
//!
//! [`FramePipeline`] runs on whichever thread owns the GPU — the process's
//! main thread in headed mode, because winit's event loop and the surface
//! belong there. It owns the [`FrameRenderer`], the scene, the image store, and
//! the render-target size, and it consumes [`Frame`]s from the
//! [`DomThread`]. It never touches a document.
//!
//! [`Program`] is the seam between them: a decoded bundle is plain data, so
//! it can be moved onto the DOM thread and booted there. The runtime it
//! becomes never crosses back.

use bobcat_core::lynx_element::dom::FrameRenderer;
use bobcat_core::lynx_element::dom::input::InputEvent;
use bobcat_core::lynx_element::dom::vello::Scene;
use bobcat_core::lynx_element::{ElementTree, PageConfig, Viewport};
use bobcat_core::quickjs::MainThreadRuntime;
use url::Url;

use crate::CliError;
use crate::dom_thread::DomThread;

const MAX_RENDER_DIMENSION: u32 = 16_384;

#[derive(Debug)]
pub(crate) struct Program {
    input: String,
    source: String,
    config: PageConfig,
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

    /// Creates the runtime and runs the bundle's main-thread script.
    ///
    /// This runs **on the DOM thread**: the `QuickJS` realm it creates is bound
    /// to whichever thread built it, so the decoded program travels and the
    /// runtime stays put.
    pub(crate) fn boot(self, viewport: Viewport) -> Result<MainThreadRuntime, CliError> {
        let mut runtime = MainThreadRuntime::new(ElementTree::new(viewport, self.config))
            .map_err(CliError::RuntimeInitialization)?;
        runtime
            .run_main_thread_script(&self.source)
            .map_err(|source| CliError::Runtime {
                input: self.input.clone(),
                source,
            })?;

        if self.author_rule_count != 0 {
            eprintln!(
                "bobcat: warning: {} contains {} decoded author rule(s), but StyleInfo ingestion \
                 is not implemented yet; author styles are omitted",
                self.input, self.author_rule_count
            );
        }
        Ok(runtime)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameSize {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) struct PreparedFrame<'a> {
    pub(crate) scene: &'a Scene,
    pub(crate) size: FrameSize,
    /// Whether this call repainted the scene: `false` means the scene is
    /// byte-identical to the previously prepared frame, so a host that
    /// already submitted that frame may skip the GPU entirely.
    pub(crate) changed: bool,
}

pub(crate) struct FramePipeline {
    dom: DomThread,
    renderer: FrameRenderer,
    viewport: Viewport,
    frame_size: FrameSize,
    /// Whether the painter's scene holds a frame at all. Before the first
    /// paint, and after a resize, the scene must be rebuilt unconditionally.
    painted: bool,
}

impl std::fmt::Debug for FramePipeline {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FramePipeline")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .field("painted", &self.painted)
            .finish_non_exhaustive()
    }
}

impl FramePipeline {
    /// Starts the DOM thread and paints its first frame.
    ///
    /// `wake` is invoked from the DOM thread whenever a frame is published.
    /// A backend with an event loop uses it to request a redraw; one that
    /// polls at its own clock passes a no-op.
    pub(crate) fn start(
        program: Program,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        wake: impl Fn() + Send + 'static,
    ) -> Result<Self, CliError> {
        let frame_size = frame_size(width, height, device_pixel_ratio)?;
        let dom = DomThread::spawn(program, width, height, device_pixel_ratio, wake)?;
        Ok(Self {
            dom,
            renderer: FrameRenderer::new(),
            viewport: Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio),
            frame_size,
            painted: false,
        })
    }

    pub(crate) fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(), CliError> {
        let next_size = frame_size(width, height, device_pixel_ratio)?;
        let size_changed = self.viewport.width.to_bits() != width.to_bits()
            || self.viewport.height.to_bits() != height.to_bits();
        let scale_changed =
            self.viewport.device_pixel_ratio.to_bits() != device_pixel_ratio.to_bits();
        if !size_changed && !scale_changed {
            return Ok(());
        }

        // The render target is this side's business and must be right before
        // the next present; the relayout it implies is the DOM thread's, and
        // arrives as an ordinary frame.
        self.dom.resize(width, height, device_pixel_ratio);
        self.viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        self.frame_size = next_size;
        self.painted = false;
        Ok(())
    }

    /// Paints the newest published frame, if one arrived since the last call.
    ///
    /// This never blocks on the DOM thread: a frame that is not ready yet
    /// simply is not painted, and the scene keeps whatever it last held. Use
    /// [`Self::sync_frame`] where the current document state is required.
    pub(crate) fn prepare_frame(&mut self) -> Result<PreparedFrame<'_>, CliError> {
        if let Some(error) = self.dom.fatal() {
            return Err(error);
        }
        let changed = match self.dom.take_frame() {
            Some(published) => {
                self.renderer
                    .adopt(published.frame, published.confirmed_scroll);
                self.renderer.render();
                self.painted = true;
                true
            }
            // A resize invalidated the scene but no new frame has landed yet;
            // report "unchanged" rather than presenting a scene sized for the
            // old target.
            None => false,
        };
        Ok(PreparedFrame {
            scene: self.renderer.scene(),
            size: self.frame_size,
            changed,
        })
    }

    /// Waits for a frame reflecting the document's current state, then paints
    /// it.
    ///
    /// The `frame` and `screenshot` console commands mean "what the page
    /// looks like *now*", so they take the barrier rather than whatever
    /// happens to be in the slot.
    pub(crate) fn sync_frame(&mut self) -> Result<PreparedFrame<'_>, CliError> {
        self.dom.sync()?;
        let prepared = self.prepare_frame()?;
        Ok(prepared)
    }

    /// Routes one host input event: scrolls here on the paint thread if the
    /// gesture resolves to one, then forwards it for targeting.
    ///
    /// The frame in hand carries everything hit testing and scrolling need,
    /// so the pixels move this vsync instead of after a DOM round trip. The
    /// event still goes to the document — that is where the target is
    /// resolved — with `default_prevented` set once this side owns the
    /// gesture, which is the seam `dom::input` documents for a host driving
    /// its own scroll physics. Returns whether anything scrolled, so a caller
    /// under `ControlFlow::Wait` knows whether to present.
    pub(crate) fn handle_input(&mut self, mut event: InputEvent) -> bool {
        let response = self.renderer.handle_input(&event);
        let scrolled = !response.scrolled.is_empty();
        for update in response.scrolled {
            self.dom.scrolled(update);
        }
        if scrolled {
            self.renderer.render();
            self.painted = true;
        }
        event.default_prevented = response.owns_gesture;
        self.dom.input(event);
        scrolled
    }

    /// Whether a published frame is waiting to be painted.
    ///
    /// Deliberately *not* "the scene is out of date". After a resize the
    /// scene is stale until the DOM thread republishes, but re-presenting it
    /// meanwhile costs a full GPU pass per vsync and shows the same pixels;
    /// the frame that resolves it arrives on its own and wakes the loop.
    pub(crate) fn needs_frame(&self) -> bool {
        self.dom.has_frame()
    }

    /// Asks the DOM thread to publish if anything changed, without waiting.
    pub(crate) fn request_frame(&self) {
        self.dom.request_frame();
    }
}

fn frame_size(width: f32, height: f32, device_pixel_ratio: f32) -> Result<FrameSize, CliError> {
    if !width.is_finite()
        || !height.is_finite()
        || !device_pixel_ratio.is_finite()
        || width <= 0.0
        || height <= 0.0
        || device_pixel_ratio <= 0.0
    {
        return Err(CliError::Viewport(format!(
            "CSS size and device-pixel ratio must be finite and positive, got \
             {width}\u{d7}{height} at {device_pixel_ratio}\u{d7}"
        )));
    }

    let physical_width = f64::from(width) * f64::from(device_pixel_ratio);
    let physical_height = f64::from(height) * f64::from(device_pixel_ratio);
    if physical_width > f64::from(MAX_RENDER_DIMENSION)
        || physical_height > f64::from(MAX_RENDER_DIMENSION)
    {
        return Err(CliError::Viewport(format!(
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
    let size = FrameSize {
        width: physical_width.round().max(1.0) as u32,
        height: physical_height.round().max(1.0) as u32,
    };
    Ok(size)
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
