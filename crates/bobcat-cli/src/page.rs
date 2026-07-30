use bobcat_quickjs::MainThreadRuntime;
use lynx_element::{PageConfig, Viewport};
use pulsar::vello::Scene;
use pulsar::{ImageStore, Painter};
use url::Url;

use crate::CliError;

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

    pub(crate) fn boot(
        self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<FramePipeline, CliError> {
        let frame_size = frame_size(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        let mut runtime = MainThreadRuntime::new(viewport, self.config)
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

        Ok(FramePipeline {
            runtime,
            painter: Painter::new(),
            images: ImageStore::new(),
            viewport,
            frame_size,
            painted_epoch: None,
        })
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
    runtime: MainThreadRuntime,
    painter: Painter,
    images: ImageStore,
    viewport: Viewport,
    frame_size: FrameSize,
    /// The document's `visual_epoch` the painted scene reflects; `None`
    /// until the first paint (and after a resize, which must repaint
    /// unconditionally).
    painted_epoch: Option<u64>,
}

impl std::fmt::Debug for FramePipeline {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FramePipeline")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .field("painted_epoch", &self.painted_epoch)
            .finish_non_exhaustive()
    }
}

impl FramePipeline {
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
        self.painted_epoch = None;
        Ok(())
    }

    pub(crate) fn prepare_frame(&mut self) -> PreparedFrame<'_> {
        let mut elements = self.runtime.elements_mut();
        let changed = self.painted_epoch != Some(elements.document().visual_epoch());
        if changed {
            let frame = elements.paint_order();
            self.painter
                .paint(elements.document(), &frame, &self.images);
            // Read the epoch after the flush so any bookkeeping done inside
            // `paint_order` is folded into the painted state.
            self.painted_epoch = Some(elements.document().visual_epoch());
        }
        PreparedFrame {
            scene: self.painter.scene(),
            size: self.frame_size,
            changed,
        }
    }

    /// Whether the document has visual changes the painted scene does not
    /// reflect yet.
    #[cfg(target_os = "macos")]
    pub(crate) fn needs_frame(&self) -> bool {
        self.painted_epoch != Some(self.runtime.elements().document().visual_epoch())
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
