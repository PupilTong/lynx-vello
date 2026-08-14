//! Browser Canvas surface exported through NAPI-RS on WASI.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bobcat_core::dom::FontBlob;
use bobcat_core::dom::vello::wgpu;
use bobcat_core::engine::{Engine, FrameRequester, FrameSize, Window, WindowTarget};
use bobcat_core::tree::PageConfig;
use napi::bindgen_prelude::{PromiseRaw, Uint8Array};
use napi::{Env, JsValue as _, Unknown};
use napi_derive::napi;
use wgpu::napi_rs_webgpu::{HtmlCanvasElement, JsCast as _};

#[derive(Clone, Debug, Default)]
struct FrameSignal {
    requested: Arc<AtomicBool>,
}

impl FrameSignal {
    fn take(&self) -> bool {
        self.requested.swap(false, Ordering::AcqRel)
    }
}

impl FrameRequester for FrameSignal {
    fn request_frame(&self) {
        self.requested.store(true, Ordering::Release);
    }
}

/// The type-level browser window used to specialize [`Engine`]. Browser
/// targets are attached from an owned `SurfaceTarget::Canvas`, so no value of
/// this marker type is ever needed.
#[derive(Debug)]
enum BrowserWindow {}

impl Window for BrowserWindow {
    type Target<'window> = WindowTarget<'window>;
    type Frames = FrameSignal;

    fn target(&self) -> Self::Target<'_> {
        match *self {}
    }

    fn frames(&self) -> Self::Frames {
        match *self {}
    }
}

/// One Bobcat view presenting its retained Vello scene to an HTML canvas.
///
/// The contained JavaScript and WebGPU handles are deliberately `!Send`. NAPI
/// keeps instances on the JavaScript thread that owns their environment.
#[napi]
pub struct BobcatCanvas {
    engine: Engine<'static, BrowserWindow>,
    canvas: HtmlCanvasElement,
    frames: FrameSignal,
    /// The embedder-side unique-id allocator: this NAPI surface is its own
    /// element host, so it owns the monotonic counter the way the Element
    /// PAPI runtime does inside the QuickJS realm. Ids are never reused.
    next_unique_id: u32,
}

impl fmt::Debug for BobcatCanvas {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BobcatCanvas")
            .field("engine", &self.engine)
            .finish_non_exhaustive()
    }
}

#[napi]
impl BobcatCanvas {
    /// Mounts an author stylesheet in this view's independent Stylo context.
    #[napi]
    #[allow(clippy::needless_pass_by_value)]
    pub fn add_author_stylesheet(&mut self, css: String) {
        self.engine.add_author_stylesheet(&css);
    }

    /// Binds and returns the permanent page element. The component id and
    /// CSS id are accepted for Element PAPI shape and recorded nowhere: this
    /// embedder has no CSS-scope machinery to route them into.
    #[napi]
    #[allow(clippy::needless_pass_by_value)]
    pub fn create_page(&mut self, _component_id: String, _component_css_id: i32) -> u32 {
        self.engine.elements().create_page();
        1
    }

    /// Creates one detached Lynx `view` element and returns its unique id.
    #[napi]
    pub fn create_view(&mut self, parent_component_unique_id: u32) -> napi::Result<u32> {
        let unique_id = self.next_unique_id;
        self.engine
            .elements()
            .create_element(unique_id, "view", parent_component_unique_id)
            .map_err(napi_error)?;
        self.next_unique_id += 1;
        Ok(unique_id)
    }

    /// Appends an element and returns the appended child id.
    #[napi]
    pub fn append_element(&mut self, parent: u32, child: u32) -> napi::Result<u32> {
        self.engine
            .elements()
            .insert_before(parent, child, None)
            .map_err(napi_error)?;
        Ok(child)
    }

    /// Retires an element subtree.
    #[napi]
    pub fn drop_element(&mut self, element: u32) -> napi::Result<()> {
        self.engine
            .elements()
            .drop_element(element)
            .map_err(napi_error)
    }

    /// Commits pending Element-PAPI mutations and requests a browser frame.
    #[napi]
    pub fn flush_element_tree(&mut self) {
        self.engine.elements().flush_element_tree();
        self.engine.refresh();
    }

    /// Registers owned font bytes for Parley text measurement.
    #[napi]
    pub fn register_fonts(&mut self, bytes: Uint8Array) -> u32 {
        let owned = bytes.to_vec();
        drop(bytes);
        u32::try_from(self.engine.register_fonts(FontBlob::new(owned))).unwrap_or(u32::MAX)
    }

    /// Applies new CSS-pixel metrics and updates the canvas backing size.
    #[napi]
    #[allow(clippy::cast_possible_truncation)]
    pub fn resize(&mut self, width: f64, height: f64, device_pixel_ratio: f64) -> napi::Result<()> {
        self.engine
            .resize(width as f32, height as f32, device_pixel_ratio as f32)
            .map_err(napi_error)?;
        set_canvas_size(&self.canvas, self.engine.frame_size());
        Ok(())
    }

    /// Presents one requested frame, returning whether a redraw was pending.
    #[napi]
    pub fn render_if_requested(&mut self) -> napi::Result<bool> {
        if !self.frames.take() {
            return Ok(false);
        }
        self.engine.notify_redraw().map_err(napi_error)?;
        Ok(true)
    }
}

/// Creates a Bobcat Canvas without moving its WebGPU handles off the browser
/// thread. The returned promise is driven by `napi-rs-webgpu`'s local executor,
/// not by NAPI-RS's `Send` async-work runtime.
#[napi(ts_return_type = "Promise<BobcatCanvas>")]
#[allow(unsafe_code)]
#[allow(clippy::cast_possible_truncation)]
pub fn create_bobcat_canvas(
    env: Env,
    canvas: Unknown,
    width: f64,
    height: f64,
    device_pixel_ratio: f64,
) -> napi::Result<PromiseRaw<'static, BobcatCanvas>> {
    if !wgpu::napi_rs_webgpu::is_installed() {
        // SAFETY: `env` is the live environment for this NAPI call and remains
        // installed for the lifetime of this module on the calling thread.
        unsafe { wgpu::napi_rs_webgpu::install(env.raw()) };
    }

    // SAFETY: `canvas` is a live value in this NAPI call's current handle
    // scope, and both handles belong to the same `env` on this thread.
    let canvas_value = unsafe { wgpu::napi_rs_webgpu::adopt_js_value(env.raw(), canvas.raw()) };
    let canvas = canvas_value
        .dyn_into::<HtmlCanvasElement>()
        .map_err(|_| napi::Error::from_reason("canvas must be an HTMLCanvasElement"))?;

    let frames = FrameSignal::default();
    let mut engine: Engine<'static, BrowserWindow> = Engine::new(
        PageConfig::default(),
        width as f32,
        height as f32,
        device_pixel_ratio as f32,
    )
    .map_err(napi_error)?;
    set_canvas_size(&canvas, engine.frame_size());
    let target: WindowTarget<'static> = WindowTarget::Canvas(canvas.clone());

    let (deferred, promise) = env.create_deferred::<BobcatCanvas, _>()?;
    let promise = PromiseRaw::new(env.raw(), promise.raw());
    wgpu::napi_rs_webgpu::futures::spawn_local(async move {
        match engine
            .attach_target(target, frames.clone(), engine.frame_size())
            .await
        {
            Ok(()) => deferred.resolve(move |_env| {
                Ok(BobcatCanvas {
                    engine,
                    canvas,
                    frames,
                    next_unique_id: 2,
                })
            }),
            Err(error) => deferred.reject(napi_error(error)),
        }
    });

    Ok(promise)
}

fn set_canvas_size(canvas: &HtmlCanvasElement, size: FrameSize) {
    canvas.set_width(size.width);
    canvas.set_height(size.height);
}

fn napi_error(error: impl fmt::Display) -> napi::Error {
    napi::Error::from_reason(error.to_string())
}
