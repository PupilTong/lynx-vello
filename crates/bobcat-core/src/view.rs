//! The opaque per-view runtime facade exposed to embedders.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fmt, str};

use dom::ImageStore;
use http::HeaderMap;

#[cfg(not(target_arch = "wasm32"))]
use crate::engine::Screenshot;
use crate::engine::{
    Engine, EngineError, EngineEvent, EventRequester, FrameSize, NoWindow, Window, WindowTarget,
};
use crate::input::InputEvent;
use crate::resource::{
    CachePolicy, RequestContext, RequestId, ResolveRequest, ResourceDescriptor, ResourceFetcher,
    ResourcePriority, ResourceRequest, StyleSheetPayload,
};
use crate::tree::PageConfig;

static NEXT_REQUEST_NAMESPACE: AtomicU64 = AtomicU64::new(1);

/// A running Lynx view.
///
/// The document, element tree, script realm, and presentation engine are all
/// private implementation state. Embedders provide resources, a draw target,
/// and normalized OS events.
pub struct LynxView<'window, W: Window = NoWindow> {
    resource_fetcher: Arc<dyn ResourceFetcher>,
    engine: Engine<'window, W>,
    script_started: bool,
    request_namespace: u64,
    next_request_sequence: u64,
}

/// The offscreen composition of [`LynxView`].
pub type OffscreenLynxView = LynxView<'static, NoWindow>;

impl<W: Window> fmt::Debug for LynxView<'_, W> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("LynxView").finish_non_exhaustive()
    }
}

/// Failure to load or start view content.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LynxViewError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Resource(#[from] crate::resource::ResourceError),
    #[error("script `{url}` is not valid UTF-8: {message}")]
    InvalidScriptEncoding { url: String, message: String },
    #[error("stylesheet `{url}` is not valid UTF-8: {message}")]
    InvalidStyleSheetEncoding { url: String, message: String },
    #[error("the image store could not load `{image_source}`: {message}")]
    Image {
        image_source: String,
        message: String,
    },
}

impl<'window, W: Window> LynxView<'window, W> {
    /// Creates a view at the supplied CSS viewport and device-pixel ratio.
    ///
    /// The animation timeline is the engine's own: a host neither names one
    /// nor drives it. Frames are paced by the swap chain, and the engine
    /// samples its clock once per frame, after the acquire that waits on
    /// vsync.
    pub fn new(
        config: PageConfig,
        resource_fetcher: Arc<dyn ResourceFetcher>,
        event_requester: Arc<dyn EventRequester>,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<Self, LynxViewError> {
        Ok(Self {
            resource_fetcher,
            engine: Engine::new(config, event_requester, width, height, device_pixel_ratio)?,
            script_started: false,
            request_namespace: NEXT_REQUEST_NAMESPACE.fetch_add(1, Ordering::Relaxed),
            next_request_sequence: 0,
        })
    }

    /// Fetches a UTF-8 entry MTS module through the injected resource provider
    /// and boots it on the engine-owned Lynx main thread.
    ///
    /// Completion is reported through [`EngineEvent::ScriptFinished`] or
    /// [`EngineEvent::ScriptRunError`].
    /// The resolved URL is the module specifier imported by Bobcat's ESM boot
    /// module. A view currently accepts one entry module; a second call is
    /// rejected.
    pub async fn execute_script(&mut self, url: &str) -> Result<(), LynxViewError> {
        if self.script_started {
            return Err(EngineError::ScriptAlreadyStarted.into());
        }
        let (request, source_name) = self.resolve_for_fetch(url).await?;
        let response = self.resource_fetcher.fetch_resource(request).await?;
        let source = str::from_utf8(&response.bytes).map_err(|error| {
            LynxViewError::InvalidScriptEncoding {
                url: source_name.clone(),
                message: error.to_string(),
            }
        })?;
        self.engine.spawn_script(source.to_owned(), source_name)?;
        self.script_started = true;
        Ok(())
    }

    /// Registers owned font bytes with the private text engine.
    ///
    /// The payload is retained without copying. If a script batch currently
    /// owns the document, the update is rejected with
    /// [`EngineError::ResourceUpdateBusy`] and may be retried after the next
    /// engine event.
    pub fn register_fonts<Data>(&mut self, data: Data) -> Result<usize, EngineError>
    where
        Data: AsRef<[u8]> + Send + Sync + 'static,
    {
        self.engine.register_fonts(dom::FontBlob::new(data))
    }

    /// Selects a registered font family as this view's platform default.
    ///
    /// This is primarily for embedders without system-font discovery, such as
    /// Wasm. It maps CSS `system-ui`, `sans-serif`, and `serif` to `family`
    /// ahead of any platform fallbacks. Call it after [`Self::register_fonts`].
    /// Returns `false` when no registered or system family has that name.
    ///
    /// If a script batch currently owns the document, the update is rejected
    /// with [`EngineError::ResourceUpdateBusy`].
    pub fn set_default_font_family(&mut self, family: &str) -> Result<bool, EngineError> {
        self.engine.set_default_font_family(family)
    }

    /// Installs the [`ImageStore`] every later frame reads.
    ///
    /// The store owns every decoded pixel this view draws: the engine holds
    /// none of its own, asks for one image at a time by source string, and
    /// never decides when a buffer is dropped. A view without a store paints
    /// no images at all.
    ///
    /// If a script batch currently owns the document, the update is rejected
    /// with [`EngineError::ResourceUpdateBusy`].
    pub fn set_image_store(&mut self, store: Arc<dyn ImageStore>) -> Result<(), EngineError> {
        self.engine.set_image_store(store)
    }

    /// Loads one image through the installed store and repaints with it.
    ///
    /// Reaching the store and invalidating the scene each need the document
    /// for the length of one call, and neither holds it across the await, so
    /// a load cannot deadlock a script batch. Both are refused with
    /// [`EngineError::ResourceUpdateBusy`] while a batch owns the document:
    /// refused before the fetch nothing has started, and refused after it the
    /// pixels are already in the store, so asking again costs no transfer.
    pub async fn load_image(&mut self, source: &str) -> Result<(), LynxViewError> {
        let store = self.engine.image_store()?;
        store
            .get(source)
            .await
            .map_err(|error| LynxViewError::Image {
                image_source: source.to_owned(),
                message: error.to_string(),
            })?;
        self.engine.note_images_changed()?;
        Ok(())
    }

    /// Asks the installed store to start loading `source` without waiting for
    /// it, discarding both the pixels and any failure.
    ///
    /// The pixels reach the screen on the first frame after they land only if
    /// something else invalidates the scene; a prefetch is a warm-up, not a
    /// load. Use [`Self::load_image`] for an image the next frame must draw.
    ///
    /// Refused with [`EngineError::ResourceUpdateBusy`] while a script batch
    /// owns the document, because reaching the store needs it.
    pub fn prefetch_image(&self, source: &str) -> Result<(), EngineError> {
        self.engine.image_store()?.prefetch(source);
        Ok(())
    }

    /// Loads an author stylesheet through the injected resource provider and
    /// mounts it on the document.
    ///
    /// The provider answers with either form of
    /// [`StyleSheetPayload`]: CSS text, or a
    /// [`PreparsedStyleSheet`](crate::style::PreparsedStyleSheet) it decoded itself — a
    /// `.web.bundle` ships CSS that a build step already tokenized, and lowering that form
    /// skips the CSS parser rather than reconstructing stylesheet text.
    ///
    /// Mount order is cascade order: sheets loaded later win ties.
    pub async fn load_style_sheet(&mut self, url: &str) -> Result<(), LynxViewError> {
        let (request, source_name) = self.resolve_for_fetch(url).await?;
        let response = self.resource_fetcher.fetch_style_sheet(request).await?;
        match &response.payload {
            StyleSheetPayload::Preparsed(sheet) => self.engine.add_preparsed_style_sheet(sheet)?,
            StyleSheetPayload::Text(bytes) => {
                let css = str::from_utf8(bytes).map_err(|error| {
                    LynxViewError::InvalidStyleSheetEncoding {
                        url: source_name.clone(),
                        message: error.to_string(),
                    }
                })?;
                self.engine.add_style_sheet_text(css)?;
            }
        }
        Ok(())
    }

    /// Resolves a locator and builds the request for it — the prologue every
    /// URL-shaped load shares.
    async fn resolve_for_fetch(
        &mut self,
        url: &str,
    ) -> Result<(ResourceRequest, String), LynxViewError> {
        let context = self.next_request_context();
        let descriptor = ResourceDescriptor {
            specifier: Arc::from(url),
            base_url: None,
        };
        let resolved = self
            .resource_fetcher
            .resolve_locator(ResolveRequest {
                context: context.clone(),
                resource: descriptor,
                percent_decode: false,
            })
            .await?;
        let source_name = resolved.url.to_string();
        Ok((
            ResourceRequest {
                context,
                resource: resolved,
                headers: HeaderMap::new(),
                cache_policy: CachePolicy::Default,
            },
            source_name,
        ))
    }

    #[must_use]
    pub fn frame_size(&self) -> FrameSize {
        self.engine.frame_size()
    }

    pub fn dispatch_input(&mut self, event: InputEvent) {
        self.engine.dispatch_input(event);
    }

    pub fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(), EngineError> {
        self.engine.resize(width, height, device_pixel_ratio)
    }

    pub fn refresh(&self) {
        self.engine.refresh();
    }

    pub fn pump(&mut self) -> Vec<EngineEvent> {
        self.engine.pump()
    }

    pub async fn attach_target(
        &mut self,
        target: impl Into<WindowTarget<'window>>,
        frames: W::Frames,
        size: FrameSize,
    ) -> Result<(), EngineError> {
        self.engine.attach_target(target, frames, size).await
    }

    pub fn notify_redraw(&mut self) -> Result<(), EngineError> {
        self.engine.notify_redraw()
    }

    /// Whether the view owes the timeline another frame: the last produced
    /// frame left an animation running, or an input sequence armed a gesture
    /// deadline (a pending long-press) the frame clock still has to resolve.
    ///
    /// Offscreen embedders that idle their tick loop must keep ticking while
    /// this reports `true`; windowed embedders need nothing, because the
    /// engine requests its own frames through the attached window.
    #[must_use]
    pub fn is_animating(&self) -> bool {
        self.engine.is_animating()
    }

    fn next_request_context(&mut self) -> RequestContext {
        let sequence = self.next_request_sequence;
        self.next_request_sequence = self
            .next_request_sequence
            .checked_add(1)
            .expect("one Lynx view exhausted its resource request id space");
        RequestContext {
            id: RequestId {
                namespace: self.request_namespace,
                sequence,
            },
            priority: ResourcePriority::Critical,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<'window, W: Window> LynxView<'window, W> {
    pub fn attach_window(
        &mut self,
        window: &'window W,
        size: FrameSize,
    ) -> Result<(), EngineError> {
        self.engine.attach_window(window, size)
    }
}

impl LynxView<'static, NoWindow> {
    pub fn attach_offscreen(&mut self) -> Result<(), EngineError> {
        self.engine.attach_offscreen()
    }

    pub fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        self.engine.tick(force)
    }
}

impl<W: Window> LynxView<'_, W> {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture(&mut self) -> Result<Screenshot, EngineError> {
        self.engine.capture()
    }
}
