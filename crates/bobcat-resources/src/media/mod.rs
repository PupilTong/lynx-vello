//! Videos: the `Id` pictures a page asked to open with the platform's media
//! player ([`PlaybackRequest::Open`](bobcat_core::PlaybackRequest)), each
//! with its player, its slot, and what it reports.
//!
//! The player is the platform's ([`vant::media::Media`], an associated type
//! of the [`Platform`]): `AVFoundation` on macOS ([`avfoundation`]), the
//! browser's `HTMLVideoElement` in `bobcat-wasm`, none on Linux, whose
//! every open fails ([`vant::media::NoMedia`]). It owns the clock, its audio
//! is the master, and it hands each new frame to the picture's [`Output`]
//! on whatever thread it produces frames on. The output publishes the frame
//! into the picture's slot ([`PictureSlot::publish`]) and wakes the host
//! when the slot says a wake is owed; the painter takes it at its next
//! adoption. Nothing on the painter's thread decodes or waits.
//!
//! # Drawn and not drawn
//!
//! A picture is drawn from the first describe of it until no view's drawn
//! set ([`FrameImages::retain`](bobcat_core::FrameImages::retain)) includes
//! it, as an animated picture's playhead is. While it is not drawn its
//! player keeps playing — its audio goes on, and it ends when it ends — but
//! its frames are not published: the output tells the player it wants none
//! ([`MediaSink::wants_frames`]) and keeps one handed over anyway — the
//! frame a video stops on always is — without publishing it. The
//! newest frame handed over is kept, so a picture drawn again whose texture
//! the painter let go of is published that frame again when the painter
//! wants it ([`FrameImages::want`](bobcat_core::FrameImages::want)), paused
//! or not.
//!
//! # Sources
//!
//! A source is resolved as an image's is. A URL the platform's player reads
//! itself ([`Media::reads`]: `http` and `https` in the browser, `file` on
//! macOS) is handed over as one. Any other — a registered resource, a
//! `data:` URL, an `http` video on macOS — is fetched by this crate first
//! and handed over as its bytes ([`MediaSource::Bytes`]), which is what lets
//! a card's own videos play: no platform player reads this crate's
//! registry. Commands that arrive while the bytes are fetched are
//! applied to the player once it is open.
//!
//! # Reports
//!
//! The player's three reports ([`MediaEvent`]) are kept by the output and
//! drained in the host's turn ([`MediaPictures::service`]) into the view
//! that opened the picture, as [`PlaybackReport::Media`], with the image
//! reports. An open that fails at once is reported in the same turn, and a
//! `play` of a picture whose player failed reports that failure again, as
//! iOS's player does (`LynxVideoView.m:201-209`).

#[cfg(target_os = "macos")]
pub mod avfoundation;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use bobcat_core::{
    FrameImage, ImageDescriptor, ImageReports, MediaCommand, MediaError, MediaErrorKind,
    MediaEvent, MediaSettings, PictureFrame, PictureId, PictureSlot, PlaybackReport,
    PlaybackRequest, TakenFrame,
};
use rustc_hash::FxHashMap;
use url::Url;
use vant::Platform;
use vant::image::residency::ViewKey;
use vant::media::{Media, MediaPlayer, MediaSink, MediaSource};

use crate::executor::Wakeup;
use crate::{FrameOf, Resources, SharedHandle};

/// The player `P`'s media capability opens.
pub(crate) type PlayerOf<P> = <<P as Platform>::Media as Media<FrameOf<P>>>::Player;

/// What one picture's player hands over, shared between the player, on
/// whatever thread it produces frames on, and the store, on the painter's.
struct Output<F> {
    slot: PictureSlot<F>,
    /// Whether a view draws the picture: frames are published only then.
    drawn: AtomicBool,
    /// The newest frame published, published again when the painter wants
    /// the picture and has no texture for it.
    newest: Mutex<Option<(PictureFrame<F>, ImageDescriptor)>>,
    /// Reports not drained into the view yet, in order.
    events: Mutex<Vec<MediaEvent>>,
    wakeup: Wakeup,
}

/// A picture's output, as its player holds it: the [`MediaSink`] this store
/// hands every [`Media::open`].
pub(crate) struct Sink<F>(Arc<Output<F>>);

impl<F> Clone for Sink<F> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<F> Sink<F> {
    fn new(wakeup: Wakeup) -> Self {
        Self(Arc::new(Output {
            slot: PictureSlot::new(),
            drawn: AtomicBool::new(false),
            newest: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            wakeup,
        }))
    }

    fn take_events(&self) -> Vec<MediaEvent> {
        std::mem::take(&mut *self.0.events.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl<F: Clone> Sink<F> {
    /// Publishes `frame` into the slot, waking the host when a wake is owed.
    fn publish(&self, frame: PictureFrame<F>, descriptor: ImageDescriptor) {
        if self.0.slot.publish(frame, descriptor, None) {
            (self.0.wakeup)();
        }
    }

    /// Publishes the newest frame again, if there is one.
    fn republish(&self) {
        let newest = self
            .0
            .newest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some((frame, descriptor)) = newest {
            self.publish(frame, descriptor);
        }
    }
}

impl<F: Clone + vant::Threaded> MediaSink<F> for Sink<F> {
    fn wants_frames(&self) -> bool {
        self.0.drawn.load(Ordering::Acquire)
    }

    /// Keeps `frame` as the newest, and publishes it while the picture is
    /// drawn. A frame handed over while nobody draws the picture (a
    /// player's first, the one it stops on, or every one from a player that
    /// skips none) is kept for the draw that comes.
    fn frame(&self, frame: PictureFrame<F>, descriptor: ImageDescriptor) {
        *self.0.newest.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((frame.clone(), descriptor));
        if self.wants_frames() {
            self.publish(frame, descriptor);
        }
    }

    fn event(&self, event: MediaEvent) {
        self.0
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
        (self.0.wakeup)();
    }
}

/// Where a picture's player is.
enum Player<P: Platform> {
    /// Its bytes are being fetched; these commands wait for the open.
    Fetching(Vec<MediaCommand>),
    Open(PlayerOf<P>),
    /// It failed before it could play, with this error.
    Failed(MediaError),
}

/// One picture a view opened.
struct Picture<P: Platform> {
    sink: Sink<FrameOf<P>>,
    player: Player<P>,
    /// What the open asked for; a command that waited for the bytes is
    /// sent after it.
    settings: MediaSettings,
    reports: ImageReports,
    view: ViewKey,
}

/// Every video of one scope: the pictures, keyed by id, and each view's
/// drawn set of them.
pub(crate) struct MediaPictures<P: Platform> {
    pictures: FxHashMap<PictureId, Picture<P>>,
    views: FxHashMap<ViewKey, Vec<PictureId>>,
}

impl<P: Platform> Default for MediaPictures<P> {
    fn default() -> Self {
        Self {
            pictures: FxHashMap::default(),
            views: FxHashMap::default(),
        }
    }
}

impl<P: Platform> std::fmt::Debug for MediaPictures<P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MediaPictures")
            .field("pictures", &self.pictures.len())
            .finish_non_exhaustive()
    }
}

impl<P: Platform> MediaPictures<P> {
    /// Whether `picture` is a video this store opened.
    pub(crate) fn contains(&self, picture: PictureId) -> bool {
        !self.pictures.is_empty() && self.pictures.contains_key(&picture)
    }

    /// Opens `picture` for `view`, whose reports go to `reports`: the
    /// player opened at once on `source` when it is `Some`, otherwise the
    /// picture waits for its bytes ([`Self::fetched`]). A picture already
    /// open is left as it is.
    #[expect(
        clippy::too_many_arguments,
        reason = "the request's two parts, its view's two, the platform and the wake"
    )]
    pub(crate) fn open(
        &mut self,
        media: &P::Media,
        picture: PictureId,
        source: Option<MediaSource>,
        settings: MediaSettings,
        reports: ImageReports,
        view: ViewKey,
        wakeup: Wakeup,
    ) {
        if self.pictures.contains_key(&picture) {
            return;
        }
        let sink = Sink::new(wakeup);
        let player = match source {
            Some(source) => Self::start(media, source, settings, &sink, &reports, picture),
            None => Player::Fetching(Vec::new()),
        };
        self.pictures.insert(
            picture,
            Picture {
                sink,
                player,
                settings,
                reports,
                view,
            },
        );
    }

    /// Opens a player, reporting a failure to open at once.
    fn start(
        media: &P::Media,
        source: MediaSource,
        settings: MediaSettings,
        sink: &Sink<FrameOf<P>>,
        reports: &ImageReports,
        picture: PictureId,
    ) -> Player<P> {
        match media.open(source, settings, sink.clone()) {
            Ok(player) => Player::Open(player),
            Err(error) => {
                reports.playback(
                    picture,
                    PlaybackReport::Media(MediaEvent::Failed(error.clone())),
                );
                Player::Failed(error)
            }
        }
    }

    /// The bytes of `picture`'s source arrived, or failed to: its player is
    /// opened on them and handed the commands that waited.
    pub(crate) fn fetched(
        &mut self,
        media: &P::Media,
        picture: PictureId,
        source: Result<MediaSource, String>,
    ) {
        let Some(entry) = self.pictures.get_mut(&picture) else {
            return;
        };
        let Player::Fetching(waiting) = &mut entry.player else {
            return;
        };
        let waiting = std::mem::take(waiting);
        entry.player = match source {
            Ok(source) => Self::start(
                media,
                source,
                entry.settings,
                &entry.sink,
                &entry.reports,
                picture,
            ),
            Err(message) => {
                let error = MediaError::new(MediaErrorKind::Source, message);
                entry.reports.playback(
                    picture,
                    PlaybackReport::Media(MediaEvent::Failed(error.clone())),
                );
                Player::Failed(error)
            }
        };
        for command in waiting {
            Self::apply(entry, picture, command);
        }
    }

    /// Sends `command` to `picture`'s player.
    pub(crate) fn command(&mut self, picture: PictureId, command: MediaCommand) {
        if let Some(entry) = self.pictures.get_mut(&picture) {
            Self::apply(entry, picture, command);
        }
    }

    fn apply(entry: &mut Picture<P>, picture: PictureId, command: MediaCommand) {
        match &mut entry.player {
            Player::Fetching(waiting) => waiting.push(command),
            Player::Open(player) => player.command(command),
            Player::Failed(error) => {
                if command == MediaCommand::Play {
                    entry.reports.playback(
                        picture,
                        PlaybackReport::Media(MediaEvent::Failed(error.clone())),
                    );
                }
            }
        }
    }

    /// `picture` is gone: its player is closed and its slot let go of.
    pub(crate) fn end(&mut self, picture: PictureId) {
        self.pictures.remove(&picture);
        for drawn in self.views.values_mut() {
            drawn.retain(|held| *held != picture);
        }
    }

    /// Ends every picture `view` opened, and forgets its drawn set.
    pub(crate) fn release_view(&mut self, view: ViewKey) {
        self.pictures.retain(|_, entry| entry.view != view);
        self.views.remove(&view);
        self.restate();
    }

    /// What a frame drawing `picture` draws: its slot's newest frame, with
    /// no next change, since a player says when it has a frame rather than
    /// when it will. A describe is a draw: frames are published from now.
    /// `None` while nothing has been published.
    pub(crate) fn describe(&self, picture: PictureId) -> Option<FrameImage<FrameOf<P>>> {
        let entry = self.pictures.get(&picture)?;
        if !entry.sink.0.drawn.swap(true, Ordering::AcqRel) {
            // Drawn again: what the player published while it was not is
            // gone, and a paused player publishes nothing new.
            entry.sink.republish();
        }
        entry.sink.0.slot.describe()
    }

    /// The painter draws `picture` with neither a frame to take nor a
    /// texture: the newest frame is published again.
    pub(crate) fn want(&self, picture: PictureId) {
        if let Some(entry) = self.pictures.get(&picture) {
            entry.sink.republish();
        }
    }

    /// Takes the newest frame of `picture`'s slot, if it has not been taken.
    pub(crate) fn take_frame(&self, picture: PictureId) -> Option<TakenFrame<FrameOf<P>>> {
        self.pictures.get(&picture)?.sink.0.slot.take()
    }

    /// Records `view`'s drawn set: a picture is drawn while some view's set
    /// includes it.
    pub(crate) fn retain(&mut self, view: ViewKey, pictures: &[PictureId]) {
        if self.pictures.is_empty() {
            return;
        }
        let drawn: Vec<PictureId> = pictures
            .iter()
            .copied()
            .filter(|picture| self.pictures.contains_key(picture))
            .collect();
        self.views.insert(view, drawn);
        self.restate();
    }

    /// Moves every picture to whether the union of the views' sets includes
    /// it.
    fn restate(&self) {
        for (picture, entry) in &self.pictures {
            let drawn = self.views.values().any(|drawn| drawn.contains(picture));
            entry.sink.0.drawn.store(drawn, Ordering::Release);
        }
    }

    /// Drains what every player reported since the last turn into the view
    /// that opened its picture, in order.
    pub(crate) fn service(&self) {
        for (picture, entry) in &self.pictures {
            for event in entry.sink.take_events() {
                entry
                    .reports
                    .playback(*picture, PlaybackReport::Media(event));
            }
        }
    }

    /// The pictures open, for tests.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.pictures.len()
    }
}

/// Serves one of a view's requests for a video: an open, a command or an
/// end. An open whose source the player cannot read itself fetches it first,
/// as a job whose completion opens the player
/// ([`crate::images::Completion::MediaSource`]).
pub(crate) fn request<P: Platform>(
    resources: &Resources<P>,
    view: ViewKey,
    reports: &ImageReports,
    request: PlaybackRequest,
) {
    match request {
        PlaybackRequest::Open {
            picture,
            source,
            settings,
        } => {
            let base = resources.base_url();
            let shared = &resources.shared;
            let resolved = shared.transports.resolve(&source, base.as_ref());
            let (opened, fetch) = match resolved {
                Ok(url)
                    if Media::<FrameOf<P>>::reads(&*shared.media, &url)
                        && !shared.transports.registry.contains(&url) =>
                {
                    (Some(MediaSource::Url(url)), None)
                }
                Ok(url) => (None, Some(url)),
                Err(failure) => {
                    let error = MediaError::new(
                        MediaErrorKind::Source,
                        format!("`{source}` cannot be resolved: {failure}"),
                    );
                    reports.playback(picture, PlaybackReport::Media(MediaEvent::Failed(error)));
                    return;
                }
            };
            resources.media.borrow_mut().open(
                &shared.media,
                picture,
                opened,
                settings,
                reports.clone(),
                view,
                Arc::clone(&shared.wakeup),
            );
            if let Some(url) = fetch {
                spawn_fetch(resources, picture, url);
            }
        }
        PlaybackRequest::Media { picture, command } => {
            resources.media.borrow_mut().command(picture, command);
        }
        PlaybackRequest::End { picture } => resources.media.borrow_mut().end(picture),
        PlaybackRequest::Play { .. } | PlaybackRequest::Control { .. } => {}
    }
}

/// What a fetch for a player answers: the bytes, with the type they came
/// labelled with, or why there are none.
fn fetched_source(
    url: Url,
    fetched: Result<crate::transport::Fetched, vant::error::Failure>,
) -> Result<MediaSource, String> {
    let fetched = fetched.map_err(|failure| format!("`{url}` could not be fetched: {failure}"))?;
    Ok(MediaSource::Bytes {
        url,
        bytes: fetched.bytes,
        media_type: fetched.media_type.map(|media_type| media_type.to_string()),
    })
}

/// Fetches `url`'s bytes for `picture`'s player on the transport's blocking
/// pool.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_fetch<P: Platform>(resources: &Resources<P>, picture: PictureId, url: Url) {
    let shared = SharedHandle::clone(&resources.shared);
    let handle = resources.executor.handle();
    resources.executor.spawn(async move {
        let fetched = {
            let shared = SharedHandle::clone(&shared);
            let url = url.clone();
            crate::executor::blocking(&handle, "video fetch", move || {
                shared.fetch_job(
                    &url,
                    vant::CachePolicy::Default,
                    &http::HeaderMap::new(),
                    crate::file_body::Body::Read,
                )
            })
            .await
        };
        let source = match fetched {
            Ok(fetched) => fetched_source(url, fetched),
            Err(message) => Err(message),
        };
        shared.complete(crate::images::Completion::MediaSource { picture, source });
    });
}

/// Fetches `url`'s bytes for `picture`'s player, in a local task on the
/// Render Worker.
#[cfg(target_arch = "wasm32")]
fn spawn_fetch<P: Platform>(resources: &Resources<P>, picture: PictureId, url: Url) {
    let shared = SharedHandle::clone(&resources.shared);
    wasm_bindgen_futures::spawn_local(async move {
        let fetched = shared
            .transports
            .fetch(&url, vant::CachePolicy::Default, &http::HeaderMap::new())
            .await;
        let source = fetched_source(url, fetched);
        shared.complete(crate::images::Completion::MediaSource { picture, source });
    });
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "tests.rs"]
mod tests;
