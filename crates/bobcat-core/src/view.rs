//! The opaque per-view runtime facade exposed to embedders.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fmt, str};

use http::HeaderMap;

use crate::clock::{AnimationClock, SystemClock};
#[cfg(not(target_arch = "wasm32"))]
use crate::engine::Screenshot;
use crate::engine::{
    Engine, EngineError, EngineEvent, EventRequester, FrameSize, NoWindow, Window, WindowTarget,
};
use crate::image::DecodedImage;
use crate::input::InputEvent;
use crate::resource::{
    BufferedResourceRequest, CachePolicy, CancellationToken, RequestContext, RequestId,
    ResolveRequest, ResourceDescriptor, ResourceError, ResourceErrorKind, ResourceErrorPhase,
    ResourceFetcher, ResourceHints, ResourceKind, ResourceLocator, ResourcePriority,
    ResourceRequest, RetryAdvice, StyleSheetPayload,
};
use crate::script::ScriptEngineFactory;
use crate::tree::PageConfig;

/// Names a resource kind in a message a host reads.
fn cancelled_what(kind: &ResourceKind) -> &'static str {
    match kind {
        ResourceKind::StyleSheet => "stylesheet",
        _ => "script",
    }
}

const MAX_SCRIPT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_STYLE_SHEET_BYTES: u64 = 16 * 1024 * 1024;
static NEXT_REQUEST_NAMESPACE: AtomicU64 = AtomicU64::new(1);

/// A running Lynx view.
///
/// The document, element tree, script realm, and presentation engine are all
/// private implementation state. Embedders provide resources, a JavaScript VM
/// factory, a draw target, and normalized OS events.
pub struct LynxView<'window, W: Window = NoWindow, C: AnimationClock = SystemClock> {
    resource_fetcher: Arc<dyn ResourceFetcher>,
    script_engine_factory: Arc<dyn ScriptEngineFactory>,
    engine: Engine<'window, W, C>,
    script_started: bool,
    request_namespace: u64,
    next_request_sequence: u64,
}

/// The offscreen composition of [`LynxView`].
pub type OffscreenLynxView<C = SystemClock> = LynxView<'static, NoWindow, C>;

impl<W: Window, C: AnimationClock> fmt::Debug for LynxView<'_, W, C> {
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
}

impl<W: Window> LynxView<'_, W, SystemClock> {
    /// Creates a view at the supplied CSS viewport and device-pixel ratio, on
    /// the platform's monotonic clock.
    ///
    /// [`Self::with_animation_clock`] is the constructor for a host that has a
    /// better reading of a frame's time, or wants a reproducible one.
    pub fn new(
        config: PageConfig,
        resource_fetcher: Arc<dyn ResourceFetcher>,
        script_engine_factory: Arc<dyn ScriptEngineFactory>,
        event_requester: Arc<dyn EventRequester>,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<Self, LynxViewError> {
        Self::with_animation_clock(
            config,
            resource_fetcher,
            script_engine_factory,
            event_requester,
            width,
            height,
            device_pixel_ratio,
            SystemClock::new(),
        )
    }
}

impl<'window, W: Window, C: AnimationClock> LynxView<'window, W, C> {
    /// Creates a view whose animations run on `clock`.
    ///
    /// The timeline is part of the view's type and cannot be replaced later.
    /// A host that drives the reading — a browser writing
    /// `requestAnimationFrame`'s timestamp, a test stepping a
    /// [`crate::ManualClock`] — keeps its own handle and passes a clone, since
    /// a shared clock is itself a clock.
    #[allow(clippy::too_many_arguments)]
    pub fn with_animation_clock(
        config: PageConfig,
        resource_fetcher: Arc<dyn ResourceFetcher>,
        script_engine_factory: Arc<dyn ScriptEngineFactory>,
        event_requester: Arc<dyn EventRequester>,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        clock: C,
    ) -> Result<Self, LynxViewError> {
        Ok(Self {
            resource_fetcher,
            script_engine_factory,
            engine: Engine::new(
                config,
                event_requester,
                width,
                height,
                device_pixel_ratio,
                clock,
            )?,
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
        self.execute_script_with_cancellation(url, CancellationToken::new())
            .await
    }

    /// Fetches and starts an entry script with host-controlled cancellation.
    ///
    /// Dropping the returned future cancels `cancellation`; retain a clone to
    /// cancel it from another task. The same token is carried in every
    /// [`RequestContext`] passed to the resource provider.
    pub async fn execute_script_with_cancellation(
        &mut self,
        url: &str,
        cancellation: CancellationToken,
    ) -> Result<(), LynxViewError> {
        let cancellation_guard = cancellation.clone().drop_guard();
        let result = self.execute_script_inner(url, cancellation).await;
        cancellation_guard.disarm();
        result
    }

    async fn execute_script_inner(
        &mut self,
        url: &str,
        cancellation: CancellationToken,
    ) -> Result<(), LynxViewError> {
        if self.script_started {
            return Err(EngineError::ScriptAlreadyStarted.into());
        }
        let (request, source_name) = self
            .resolve_for_fetch(
                url,
                ResourceKind::ExternalJs,
                MAX_SCRIPT_BYTES,
                &cancellation,
            )
            .await?;
        let request_id = request.request.context.id;
        let response = cancellation
            .run_until_cancelled(self.resource_fetcher.fetch_resource(request))
            .await;
        if cancellation.is_cancelled() || response.is_none() {
            return Err(Self::cancelled_resource_error(
                request_id,
                &ResourceKind::ExternalJs,
                ResourceErrorPhase::ReadBody,
                Arc::from(source_name.as_str()),
            )
            .into());
        }
        let response = response.expect("checked above")?;
        let source = str::from_utf8(&response.bytes).map_err(|error| {
            LynxViewError::InvalidScriptEncoding {
                url: source_name.clone(),
                message: error.to_string(),
            }
        })?;
        if cancellation.is_cancelled() {
            return Err(Self::cancelled_resource_error(
                request_id,
                &ResourceKind::ExternalJs,
                ResourceErrorPhase::ReadBody,
                Arc::from(source_name.as_str()),
            )
            .into());
        }
        self.engine.spawn_script(
            source.to_owned(),
            source_name,
            Arc::clone(&self.script_engine_factory),
        )?;
        self.script_started = true;
        Ok(())
    }

    fn cancelled_resource_error(
        request_id: RequestId,
        kind: &ResourceKind,
        phase: ResourceErrorPhase,
        locator: Arc<str>,
    ) -> ResourceError {
        ResourceError {
            request_id: Some(request_id),
            kind: ResourceErrorKind::Cancelled,
            phase,
            locator: Some(locator),
            status: None,
            message: format!(
                "the {} resource request was cancelled",
                cancelled_what(kind)
            )
            .into(),
            retry: RetryAdvice::Never,
        }
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

    /// Installs decoded pixels under a CSS image URL in the private paint
    /// registry, replacing an earlier registration for the same URL.
    pub fn register_image_url(
        &mut self,
        url: impl Into<String>,
        image: &DecodedImage,
    ) -> Result<(), EngineError> {
        self.engine.register_image_url(url, image)
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
        self.load_style_sheet_with_cancellation(url, CancellationToken::new())
            .await
    }

    /// Loads an author stylesheet with host-controlled cancellation.
    ///
    /// Dropping the returned future cancels `cancellation`, the same way
    /// [`Self::execute_script_with_cancellation`] does.
    pub async fn load_style_sheet_with_cancellation(
        &mut self,
        url: &str,
        cancellation: CancellationToken,
    ) -> Result<(), LynxViewError> {
        let cancellation_guard = cancellation.clone().drop_guard();
        let result = self.load_style_sheet_inner(url, cancellation).await;
        cancellation_guard.disarm();
        result
    }

    async fn load_style_sheet_inner(
        &mut self,
        url: &str,
        cancellation: CancellationToken,
    ) -> Result<(), LynxViewError> {
        let (request, source_name) = self
            .resolve_for_fetch(
                url,
                ResourceKind::StyleSheet,
                MAX_STYLE_SHEET_BYTES,
                &cancellation,
            )
            .await?;
        let request_id = request.request.context.id;
        let cancelled = || {
            LynxViewError::from(Self::cancelled_resource_error(
                request_id,
                &ResourceKind::StyleSheet,
                ResourceErrorPhase::ReadBody,
                Arc::from(source_name.as_str()),
            ))
        };
        let response = cancellation
            .run_until_cancelled(self.resource_fetcher.fetch_style_sheet(request))
            .await;
        if cancellation.is_cancelled() || response.is_none() {
            return Err(cancelled());
        }
        let response = response.expect("checked above")?;
        // A sheet cancelled after its bytes arrived must not still mount, the
        // same way a cancelled script does not still start.
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
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

    /// Resolves a locator and builds the buffered request for it — the
    /// prologue every URL-shaped load shares.
    async fn resolve_for_fetch(
        &mut self,
        url: &str,
        kind: ResourceKind,
        max_bytes: u64,
        cancellation: &CancellationToken,
    ) -> Result<(BufferedResourceRequest, String), LynxViewError> {
        let context = self.next_request_context(cancellation.clone());
        let original_locator: Arc<str> = Arc::from(url);
        let descriptor = ResourceDescriptor {
            locator: ResourceLocator {
                specifier: Arc::clone(&original_locator),
                base_url: None,
            },
            kind,
            hints: ResourceHints::None,
        };
        let resolved = cancellation
            .run_until_cancelled(self.resource_fetcher.resolve_locator(ResolveRequest {
                context: context.clone(),
                resource: descriptor.clone(),
                percent_decode: false,
            }))
            .await;
        if cancellation.is_cancelled() || resolved.is_none() {
            return Err(Self::cancelled_resource_error(
                context.id,
                &descriptor.kind,
                ResourceErrorPhase::Resolve,
                original_locator,
            )
            .into());
        }
        let resolved = resolved.expect("checked above")?;
        let source_name = resolved.url.to_string();
        Ok((
            BufferedResourceRequest {
                request: ResourceRequest {
                    context,
                    resource: resolved,
                    headers: HeaderMap::new(),
                    cache_policy: CachePolicy::Default,
                },
                max_bytes,
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

    fn next_request_context(&mut self, cancellation: CancellationToken) -> RequestContext {
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
            cancellation,
            priority: ResourcePriority::Critical,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<'window, W: Window, C: AnimationClock> LynxView<'window, W, C> {
    pub fn attach_window(
        &mut self,
        window: &'window W,
        size: FrameSize,
    ) -> Result<(), EngineError> {
        self.engine.attach_window(window, size)
    }
}

impl<C: AnimationClock> LynxView<'static, NoWindow, C> {
    pub fn attach_offscreen(&mut self) -> Result<(), EngineError> {
        self.engine.attach_offscreen()
    }

    pub fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        self.engine.tick(force)
    }
}

impl<W: Window, C: AnimationClock> LynxView<'_, W, C> {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture(&mut self) -> Result<Screenshot, EngineError> {
        self.engine.capture()
    }
}
