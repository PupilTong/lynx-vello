//! The opaque per-view runtime facade exposed to embedders.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fmt, str};

use http::HeaderMap;
use tokio_util::sync::CancellationToken;

#[cfg(not(target_arch = "wasm32"))]
use crate::engine::Screenshot;
use crate::engine::{Engine, EngineError, EngineEvent, FrameSize, NoWindow, Window, WindowTarget};
use crate::input::InputEvent;
use crate::resource::{
    BufferedResourceRequest, CachePolicy, RequestContext, RequestId, ResolveRequest,
    ResourceDescriptor, ResourceFetcher, ResourceHints, ResourceKind, ResourceLocator,
    ResourcePriority, ResourceRequest,
};
use crate::script::ScriptEngineFactory;
use crate::tree::PageConfig;

const MAX_SCRIPT_BYTES: u64 = 16 * 1024 * 1024;
static NEXT_REQUEST_NAMESPACE: AtomicU64 = AtomicU64::new(1);

/// A running Lynx view.
///
/// The document, element tree, script realm, and presentation engine are all
/// private implementation state. Embedders provide resources, a JavaScript VM
/// factory, a draw target, and normalized OS events.
pub struct LynxView<'window, W: Window = NoWindow> {
    resource_fetcher: Arc<dyn ResourceFetcher>,
    script_engine_factory: Arc<dyn ScriptEngineFactory>,
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
    #[error("loading author stylesheets is not implemented: `{url}`")]
    StyleSheetUnsupported { url: String },
}

impl<'window, W: Window> LynxView<'window, W> {
    /// Creates a view at the supplied CSS viewport and device-pixel ratio.
    pub fn new(
        config: PageConfig,
        resource_fetcher: Arc<dyn ResourceFetcher>,
        script_engine_factory: Arc<dyn ScriptEngineFactory>,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<Self, LynxViewError> {
        Ok(Self {
            resource_fetcher,
            script_engine_factory,
            engine: Engine::new(config, width, height, device_pixel_ratio)?,
            script_started: false,
            request_namespace: NEXT_REQUEST_NAMESPACE.fetch_add(1, Ordering::Relaxed),
            next_request_sequence: 0,
        })
    }

    /// Fetches a UTF-8 main-thread script through the injected resource
    /// provider and starts it on the engine-owned Lynx main thread.
    ///
    /// Completion is reported through [`EngineEvent::ScriptFinished`].
    /// A view currently accepts one entry script; a second call is rejected.
    pub async fn execute_script(&mut self, url: &str) -> Result<(), LynxViewError> {
        if self.script_started {
            return Err(EngineError::ScriptAlreadyStarted.into());
        }
        let context = self.next_request_context();
        let descriptor = ResourceDescriptor {
            locator: ResourceLocator {
                specifier: Arc::from(url),
                base_url: None,
            },
            kind: ResourceKind::ExternalJs,
            hints: ResourceHints::None,
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
        let response = self
            .resource_fetcher
            .fetch_resource(BufferedResourceRequest {
                request: ResourceRequest {
                    context,
                    resource: resolved,
                    headers: HeaderMap::new(),
                    cache_policy: CachePolicy::Default,
                },
                max_bytes: MAX_SCRIPT_BYTES,
            })
            .await?;
        let source = str::from_utf8(&response.bytes).map_err(|error| {
            LynxViewError::InvalidScriptEncoding {
                url: source_name.clone(),
                message: error.to_string(),
            }
        })?;
        self.engine.spawn_script(
            source.to_owned(),
            source_name,
            Arc::clone(&self.script_engine_factory),
        )?;
        self.script_started = true;
        Ok(())
    }

    /// Reserves the URL-based stylesheet entry point without exposing direct
    /// CSS or document mutation to the embedder.
    pub fn load_style_sheet(&mut self, url: &str) -> Result<(), LynxViewError> {
        Err(LynxViewError::StyleSheetUnsupported {
            url: url.to_owned(),
        })
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
            cancellation: CancellationToken::new(),
            priority: ResourcePriority::Critical,
        }
    }
}

impl<'window, W: Window> LynxView<'window, W> {
    #[cfg(not(target_arch = "wasm32"))]
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
