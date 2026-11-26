//! The painter's window presentation stack.
//!
//! Built on the thread that owns the window, because creating a surface from
//! a window handle is a main-thread-only call on macOS — and that is the same
//! thread that draws, so it is never handed anywhere: every acquire, render
//! and present happens on the thread that built the painter. Rendering into the
//! retained target texture and presenting it are separate steps: the target is
//! re-rendered only when a new scene was produced, while surface
//! invalidation and re-exposure re-present the retained target with a blit
//! alone.
//!
//! A frame is three calls in order: [`WindowGraphics::acquire`], then
//! [`WindowGraphics::render_to_target`] if a new scene was produced, then
//! [`WindowGraphics::present`]. Acquiring first is deliberate. On
//! `PresentMode::AutoVsync` the swap chain hands over an image only once one
//! is free, so `acquire` is where a frame's back-pressure lands, and every
//! swap-chain image in flight is another display refresh between the wait and
//! the scan-out. Doing the wait up front puts the whole frame — the clock
//! reading included — on the near side of that pipeline, so what is sampled
//! is the frame being produced for the next refresh, not one produced a
//! pipeline-depth earlier.
//!
//! The surface is created on the instance of the process's
//! [`SharedGpu`] and drawn with its device and renderer, so a window renders
//! on the features and limits a headless target does and compiles no shader
//! another target already has; the surface, the retained target and the
//! bake caches are the window's own. A window whose surface the shared
//! adapter cannot present to gets a stack of its own
//! ([`SharedGpu::for_surface`]).

use std::sync::Arc;

use euclid::default::Vector2D;

use super::{CANVAS, FrameSize, PainterError};
use crate::paint::display_list::{DisplayList, ImageRef, Texture};
use crate::render::blur::FilterTextures;
use crate::render::gpu::SharedGpu;
#[cfg(not(target_arch = "wasm32"))]
use crate::render::gpu::read_texture;
use crate::render::raster::Raster;
use crate::render::vector_textures::VectorTextures;
use crate::visual::{CommittedFrame, ScrollSlot};

pub type WindowTarget = wgpu::SurfaceTarget<'static>;

pub(super) struct AcquiredFrame {
    texture: wgpu::SurfaceTexture,
    reconfigure_after: bool,
}

pub(super) enum FrameAcquisition {
    Ready(AcquiredFrame),
    Retry,
}

pub(super) struct WindowGraphics {
    gpu: Arc<SharedGpu>,
    surface: WindowSurface,
    target: RetainedTarget,
    raster: Raster,
    filters: FilterTextures,
    vectors: VectorTextures,
}

struct WindowSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    blitter: wgpu::util::TextureBlitter,
}

struct RetainedTarget {
    width: u32,
    height: u32,
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "read only by a capture, which Wasm has none of")
    )]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl RetainedTarget {
    const USAGE: wgpu::TextureUsages = {
        let usage = Raster::TARGET_USAGE.union(wgpu::TextureUsages::TEXTURE_BINDING);
        if cfg!(target_arch = "wasm32") {
            usage
        } else {
            usage.union(wgpu::TextureUsages::COPY_SRC)
        }
    };

    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("painter window target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: Self::USAGE,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            width,
            height,
            texture,
            view,
        }
    }
}

impl std::fmt::Debug for WindowGraphics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowGraphics")
            .field("surface", &self.surface.surface)
            .field("config", &self.surface.config)
            .finish_non_exhaustive()
    }
}

impl WindowGraphics {
    pub(super) async fn new(
        target: impl Into<WindowTarget>,
        size: FrameSize,
    ) -> Result<Self, PainterError> {
        let (gpu, surface) = SharedGpu::for_surface(target.into())
            .await
            .map_err(PainterError::Render)?;
        Self::from_surface(gpu, surface, size)
    }

    fn from_surface(
        gpu: Arc<SharedGpu>,
        surface: wgpu::Surface<'static>,
        size: FrameSize,
    ) -> Result<Self, PainterError> {
        let format = surface
            .get_capabilities(gpu.adapter())
            .formats
            .into_iter()
            .find(|format| {
                matches!(
                    format,
                    wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
                )
            })
            .ok_or_else(|| {
                PainterError::Render(
                    "the window surface offers neither an RGBA8 nor a BGRA8 format".to_owned(),
                )
            })?;
        let (width, height) = (size.width.max(1), size.height.max(1));
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            color_space: wgpu::SurfaceColorSpace::Auto,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        gpu.configure(&surface, &config);
        let blitter = wgpu::util::TextureBlitter::new(&gpu.device, format);
        let target = RetainedTarget::new(&gpu.device, width, height);
        let raster = Raster::new(&gpu);
        Ok(Self {
            gpu,
            surface: WindowSurface {
                surface,
                config,
                blitter,
            },
            target,
            raster,
            filters: FilterTextures::default(),
            vectors: VectorTextures::default(),
        })
    }

    fn configure_for(&mut self, size: FrameSize) {
        if size.width != 0
            && size.height != 0
            && (self.surface.config.width != size.width
                || self.surface.config.height != size.height)
        {
            let device = &self.gpu.device;
            self.target = RetainedTarget::new(device, size.width, size.height);
            self.surface.config.width = size.width;
            self.surface.config.height = size.height;
            self.gpu
                .configure(&self.surface.surface, &self.surface.config);
        }
    }

    fn reconfigure(&self) {
        self.gpu
            .configure(&self.surface.surface, &self.surface.config);
    }

    pub(super) fn acquire(&mut self, size: FrameSize) -> Result<FrameAcquisition, PainterError> {
        self.configure_for(size);
        let (texture, reconfigure_after) = match self.surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(FrameAcquisition::Retry);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.reconfigure();
                return Ok(FrameAcquisition::Retry);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(PainterError::Render(
                    "the window surface was lost".to_owned(),
                ));
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(PainterError::Render(
                    "surface acquisition raised a wgpu validation error".to_owned(),
                ));
            }
        };
        Ok(FrameAcquisition::Ready(AcquiredFrame {
            texture,
            reconfigure_after,
        }))
    }

    pub(super) fn render_to_target(
        &mut self,
        scene: &DisplayList,
        size: FrameSize,
    ) -> Result<(), PainterError> {
        self.configure_for(size);
        self.raster
            .render_to_texture(
                scene,
                &self.target.view,
                self.target.width,
                self.target.height,
                CANVAS,
            )
            .map_err(|error| PainterError::Render(error.to_string()))
    }

    pub(super) fn prepare_vectors(
        &mut self,
        frame: &CommittedFrame,
    ) -> Result<&[Option<Texture>], PainterError> {
        let Self {
            gpu,
            raster,
            vectors,
            ..
        } = self;
        vectors
            .prepare(frame, raster, gpu)
            .map_err(|error| PainterError::Gpu(error.to_string()))
    }

    pub(super) fn prepare_filters(
        &mut self,
        frame: &CommittedFrame,
        images: &[Option<ImageRef>],
        vectors: &[Option<Texture>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        scroll_generation: u64,
        animation_now: Option<f64>,
    ) -> Result<&[Option<Texture>], PainterError> {
        let Self {
            gpu,
            raster,
            filters,
            ..
        } = self;
        filters
            .prepare(
                raster,
                gpu,
                frame,
                images,
                vectors,
                offset_of,
                scroll_generation,
                animation_now,
                CANVAS,
            )
            .map_err(|error| PainterError::Gpu(error.to_string()))
    }

    pub(super) fn gpu(&self) -> &Arc<SharedGpu> {
        &self.gpu
    }

    pub(super) fn lost_pixels(&self) -> u64 {
        self.raster.lost_pixels()
    }

    pub(super) fn forget_filters(&mut self) {
        self.filters.forget();
    }

    pub(super) fn present(&mut self, acquired: AcquiredFrame) {
        let AcquiredFrame {
            texture: surface_texture,
            reconfigure_after,
        } = acquired;
        let SharedGpu { device, queue, .. } = &*self.gpu;
        let output_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("painter window blit"),
        });
        self.surface
            .blitter
            .copy(device, &mut encoder, &self.target.view, &output_view);
        self.gpu.submit([encoder.finish()]);
        queue.present(surface_texture);
        if reconfigure_after {
            self.reconfigure();
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn capture_frame(&mut self, size: FrameSize) -> Result<Vec<u8>, PainterError> {
        if self.target.width != size.width || self.target.height != size.height {
            return Err(PainterError::Render(format!(
                "the window target is {}\u{d7}{}, not the {}\u{d7}{} being captured",
                self.target.width, self.target.height, size.width, size.height
            )));
        }
        read_texture(&self.gpu, &self.target.texture, size.width, size.height)
            .map_err(|error| PainterError::Gpu(error.to_string()))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::sync::Arc;

    use objc2_quartz_core::CAMetalLayer;

    use super::{FrameAcquisition, WindowGraphics};
    use crate::kurbo::{Affine, Circle, Point, Rect, RoundedRect, Stroke};
    use crate::paint::display_list::DisplayList;
    use crate::painter::{CANVAS, FrameSize};
    use crate::peniko::{Color, Fill, Gradient, Mix};
    use crate::render::gpu::{Headless, SharedGpu};

    fn scene(size: FrameSize) -> DisplayList {
        let (width, height) = (f64::from(size.width), f64::from(size.height));
        let mut scene = DisplayList::new();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgb8(20, 120, 220),
            None,
            &Rect::new(8.0, 8.0, width / 2.0, height / 2.0),
        );
        let gradient = Gradient::new_linear(Point::new(0.0, 0.0), Point::new(width, height))
            .with_stops([Color::from_rgb8(250, 60, 30), Color::from_rgb8(30, 200, 90)]);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &gradient,
            None,
            &RoundedRect::new(width / 4.0, height / 4.0, width - 6.0, height - 6.0, 12.0),
        );
        scene.push_layer(
            Fill::NonZero,
            Mix::Normal,
            0.6,
            Affine::IDENTITY,
            &Circle::new((width / 2.0, height / 2.0), width.min(height) / 3.0),
        );
        scene.fill(
            Fill::NonZero,
            Affine::rotate(0.3),
            Color::from_rgb8(240, 200, 20),
            None,
            &Rect::new(width / 3.0, 0.0, width, height / 2.0),
        );
        scene.pop_layer();
        scene.stroke(
            &Stroke::new(3.0),
            Affine::IDENTITY,
            Color::BLACK,
            None,
            &Circle::new((width / 3.0, height * 0.7), 14.5),
        );
        scene
    }

    fn present_and_capture(window: &mut WindowGraphics, size: FrameSize) -> Vec<u8> {
        let acquired = loop {
            match window
                .acquire(size)
                .expect("the layer's swap chain hands out images")
            {
                FrameAcquisition::Ready(acquired) => break acquired,
                FrameAcquisition::Retry => {}
            }
        };
        window
            .render_to_target(&scene(size), size)
            .expect("the window target renders");
        window.present(acquired);
        window
            .capture_frame(size)
            .expect("the window target reads back")
    }

    fn window_over(layer: &CAMetalLayer, size: FrameSize) -> WindowGraphics {
        let shared = SharedGpu::offscreen().expect("a usable GPU adapter is mandatory");
        let instance = shared.instance().clone();
        #[expect(
            unsafe_code,
            reason = "wgpu takes a bare CAMetalLayer as a raw pointer"
        )]
        // SAFETY: the pointer is to a live `CAMetalLayer`, the one kind of
        // object this target variant names, and every caller holds `layer`
        // past the window, which owns the surface (wgpu's Metal backend also
        // retains the layer for itself).
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(
                std::ptr::from_ref::<CAMetalLayer>(layer).cast_mut().cast(),
            ))
        }
        .expect("wgpu builds a surface over a CAMetalLayer");
        let gpu = pollster::block_on(SharedGpu::presenting(Some(shared), instance, &surface))
            .expect("the shared adapter presents to the layer");
        WindowGraphics::from_surface(gpu, surface, size)
            .expect("the window stack builds over the layer")
    }

    #[test]
    fn a_window_frame_reads_back_as_the_headless_render_of_its_scene() {
        let layer = CAMetalLayer::new();
        let first = FrameSize {
            width: 96,
            height: 72,
        };
        let mut window = window_over(&layer, first);
        let mut headless = Headless::new().expect("a usable GPU adapter is mandatory");
        assert!(
            Arc::ptr_eq(&window.gpu, headless.gpu()),
            "the window and the headless target share one stack"
        );

        let second = FrameSize {
            width: 130,
            height: 50,
        };
        for size in [first, second] {
            let captured = present_and_capture(&mut window, size);
            let expected = headless
                .render(&scene(size), size.width, size.height, CANVAS)
                .expect("the headless render succeeds");
            assert_eq!(
                captured.len(),
                expected.len(),
                "{}x{}: both read back the whole target",
                size.width,
                size.height
            );
            assert!(
                captured
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|&pixel| pixel != [255; 4]),
                "{}x{}: the scene drew something to compare",
                size.width,
                size.height
            );
            assert!(
                captured == expected,
                "{}x{}: the window capture differs from the headless render",
                size.width,
                size.height
            );
        }
        drop(window);
        drop(layer);
    }
    struct Stop<'a>(&'a std::sync::atomic::AtomicBool);

    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            self.0.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[test]
    fn a_window_resizes_while_another_target_renders_on_its_stack() {
        let layer = CAMetalLayer::new();
        let mut window = window_over(
            &layer,
            FrameSize {
                width: 40,
                height: 40,
            },
        );
        let rendering = std::sync::atomic::AtomicBool::new(true);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut headless = Headless::new().expect("a usable GPU adapter is mandatory");
                let size = FrameSize {
                    width: 64,
                    height: 64,
                };
                while rendering.load(std::sync::atomic::Ordering::Relaxed) {
                    headless
                        .render_frame(&scene(size), size.width, size.height, CANVAS)
                        .expect("the headless render succeeds");
                }
            });
            let _stop = Stop(&rendering);
            for step in 0..24 {
                let size = FrameSize {
                    width: 40 + step * 3,
                    height: 60 - step,
                };
                let _ = present_and_capture(&mut window, size);
            }
        });
        drop(window);
        drop(layer);
    }
}
