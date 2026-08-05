use std::cell::{Ref, RefCell};
use std::rc::Rc;

#[cfg(target_os = "macos")]
use bobcat_core::lynx_element::PapiError;
#[cfg(target_os = "macos")]
use bobcat_core::lynx_element::dom::input::{InputEvent, InputResponse};
use bobcat_core::lynx_element::dom::vello::Scene;
use bobcat_core::lynx_element::{ElementOp, ElementTree, PageConfig, Viewport};
use bobcat_core::quickjs::{CommitError, MainThreadRuntime, local_commit_sink};
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

    /// Boots everything on the calling thread — realm, script, tree — and
    /// returns the pipeline once the whole main-thread boot has committed.
    /// This is the headless composition; the windowed shell uses
    /// [`Self::split`] to run the script half on its own thread.
    pub(crate) fn boot(
        self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<FramePipeline, CliError> {
        let (pipeline, script) = self.split(width, height, device_pixel_ratio)?;
        script.run(local_commit_sink(&pipeline.elements))?;
        Ok(pipeline)
    }

    /// Splits the program into the engine half — the element tree behind a
    /// [`FramePipeline`] — and the script half a shell may run anywhere,
    /// wired back only through its commit sink.
    pub(crate) fn split(
        self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(FramePipeline, ScriptJob), CliError> {
        let frame_size = frame_size(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        let elements = Rc::new(RefCell::new(ElementTree::new(viewport, self.config)));

        if self.author_rule_count != 0 {
            eprintln!(
                "bobcat: warning: {} contains {} decoded author rule(s), but StyleInfo ingestion \
                 is not implemented yet; author styles are omitted",
                self.input, self.author_rule_count
            );
        }

        Ok((
            FramePipeline {
                elements,
                viewport,
                frame_size,
            },
            ScriptJob {
                input: self.input,
                source: self.source,
            },
        ))
    }
}

/// The main-thread script half of a booted program, detached from the element
/// tree so a shell can run it on a thread of its own choosing.
#[derive(Debug)]
pub(crate) struct ScriptJob {
    input: String,
    source: String,
}

impl ScriptJob {
    /// Builds the realm and runs the whole main-thread boot, committing every
    /// `__FlushElementTree` batch through `commit`. Blocks the calling thread
    /// until the script completes.
    pub(crate) fn run(
        self,
        commit: impl FnMut(Vec<ElementOp>) -> Result<(), CommitError> + 'static,
    ) -> Result<(), CliError> {
        let mut runtime =
            MainThreadRuntime::new(commit).map_err(CliError::RuntimeInitialization)?;
        runtime
            .run_main_thread_script(&self.source)
            .map_err(|source| CliError::Runtime {
                input: self.input,
                source,
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameSize {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) struct PreparedFrame<'a> {
    elements: Ref<'a, ElementTree>,
    pub(crate) size: FrameSize,
    /// Whether this call repainted the scene: `false` means the scene is
    /// byte-identical to the previously prepared frame, so a host that
    /// already submitted that frame may skip the GPU entirely.
    pub(crate) changed: bool,
}

impl PreparedFrame<'_> {
    /// The scene retained by the document's private painter.
    pub(crate) fn scene(&self) -> Ref<'_, Scene> {
        self.elements.document().scene()
    }
}

/// The engine half of a booted program: the element tree, the viewport, and
/// the frame preparation over them. Whichever thread owns this owns the tree;
/// script sides reach it only through committed [`ElementOp`] batches.
pub(crate) struct FramePipeline {
    elements: Rc<RefCell<ElementTree>>,
    viewport: Viewport,
    frame_size: FrameSize,
}

impl std::fmt::Debug for FramePipeline {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FramePipeline")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
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
            let mut elements = self.elements.borrow_mut();
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

    pub(crate) fn prepare_frame(&mut self) -> PreparedFrame<'_> {
        let changed = self.elements.borrow_mut().document_mut().render();
        PreparedFrame {
            elements: self.elements.borrow(),
            size: self.frame_size,
            changed,
        }
    }

    /// Applies one flushed batch from the script thread and runs the style +
    /// layout commit, exactly as [`local_commit_sink`] would have on a single
    /// thread. An `Err` means the batch diverged from the recorder's shadow
    /// and must be rejected rather than half-trusted.
    #[cfg(target_os = "macos")]
    pub(crate) fn apply_commit(&mut self, ops: &[ElementOp]) -> Result<(), PapiError> {
        let mut elements = self.elements.borrow_mut();
        for op in ops {
            elements.apply(op)?;
        }
        elements.flush_element_tree();
        Ok(())
    }

    /// Routes one host input event and performs the UA default action it
    /// resolves to.
    ///
    /// The element layer keeps the visual frame private; routing reads the
    /// frame retained by the last render, so events target what the window
    /// showed. When input changes scrolling, `prepare_frame` observes the new
    /// visual epoch and refreshes the retained scene (and with it the frame).
    #[cfg(target_os = "macos")]
    pub(crate) fn handle_input(&mut self, event: InputEvent) -> InputResponse {
        self.elements.borrow_mut().handle_input(event)
    }

    /// Whether the document has visual changes the painted scene does not
    /// reflect yet.
    #[cfg(target_os = "macos")]
    pub(crate) fn needs_frame(&self) -> bool {
        self.elements.borrow().document().needs_render()
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

    /// The windowed shell's commit protocol, minus the window: the script runs
    /// on its own thread, every `__FlushElementTree` batch crosses a channel,
    /// and the engine side applies it and acknowledges — the exact wiring
    /// `macos.rs` builds over the winit proxy.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_script_job_commits_across_threads() {
        use std::cell::RefCell;
        use std::rc::Rc;
        use std::sync::mpsc;

        use bobcat_core::lynx_element::{ElementOp, ElementTree, PageConfig, Viewport};
        use bobcat_core::quickjs::CommitError;

        use super::{FramePipeline, FrameSize, ScriptJob};

        type Ack = mpsc::Sender<Result<(), CommitError>>;

        let script = ScriptJob {
            input: "threaded-smoke".to_owned(),
            source: r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  __AppendElement(page, __CreateView(0));
                  __FlushElementTree();
                  __AppendElement(page, __CreateView(0));
                };
                "
            .to_owned(),
        };

        let (commits, committed) = mpsc::channel::<(Vec<ElementOp>, Ack)>();
        let script_thread = std::thread::spawn(move || {
            script.run(move |ops| {
                let (ack, acknowledged) = mpsc::channel();
                commits
                    .send((ops, ack))
                    .map_err(|_| CommitError::Disconnected)?;
                acknowledged.recv().map_err(|_| CommitError::Disconnected)?
            })
        });

        let viewport = Viewport::new(393.0, 727.0);
        let mut pipeline = FramePipeline {
            elements: Rc::new(RefCell::new(ElementTree::new(
                viewport,
                PageConfig::default(),
            ))),
            viewport,
            frame_size: FrameSize {
                width: 393,
                height: 727,
            },
        };
        // The engine side: drain commits until the script hangs up. The boot
        // sequence flushes twice — once mid-script, once at its end.
        let mut batches = 0;
        while let Ok((ops, ack)) = committed.recv() {
            let result = pipeline.apply_commit(&ops).map_err(CommitError::Rejected);
            ack.send(result).expect("the script waits for the ack");
            batches += 1;
        }
        script_thread
            .join()
            .expect("the script thread must not panic")
            .expect("the script must boot");

        assert_eq!(batches, 2, "renderPage flushes once, the boot once more");
        let elements = pipeline.elements.borrow();
        let page = elements.page().expect("the page was created");
        let page_node = elements.node_id(page).expect("a live page");
        assert_eq!(
            elements
                .document()
                .get(page_node)
                .unwrap()
                .child_ids()
                .len(),
            2,
            "both views landed, one per committed batch"
        );
        drop(elements);
        assert!(
            pipeline.prepare_frame().changed,
            "the committed tree paints a fresh scene"
        );
    }
}
