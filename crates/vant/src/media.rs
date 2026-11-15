//! The media capability: a platform's own player, which opens a video for
//! one picture and publishes its frames.
//!
//! The engine has no video decoder of its own. A [`Media`] opens a source
//! with the platform's player (`AVFoundation` natively on macOS, an
//! `HTMLVideoElement` in the browser) and answers a [`MediaPlayer`] that
//! takes the four commands an element sends it. What the player produces
//! goes to a [`MediaSink`] the caller handed it: each new frame, which the
//! sink publishes into the picture's slot, and the three things a player
//! reports ([`MediaEvent`]): its first frame with the video's natural size,
//! the end of playback, and a failure.
//!
//! The player owns the clock. Its audio is the master, and a frame is
//! handed over when the player says it is due; nothing on the painter's
//! thread decodes or waits. A sink that says it wants no frames
//! ([`MediaSink::wants_frames`], a picture no view draws) may be handed
//! none while the player keeps playing, but is handed the frame the video
//! stops on (a pause, the end), so the picture shows it when drawn again.
//!
//! A platform with no player names [`NoMedia`], whose every open fails with
//! [`MediaErrorKind::Unsupported`].

use std::time::Duration;

use bytes::Bytes;
use url::Url;

use crate::image::{ImageDescriptor, PictureFrame};
use crate::platform::Threaded;

/// What a player is asked to open.
#[derive(Clone, Debug)]
pub enum MediaSource {
    /// A URL the platform's player reads itself ([`Media::reads`]).
    Url(Url),
    /// Bytes the resource system holds (a registered resource, a `data:`
    /// URL), which no platform player can be pointed at by URL. `url` is
    /// what they were fetched as, for messages.
    Bytes {
        url: Url,
        bytes: Bytes,
        /// The media type they were registered or served with, when known.
        media_type: Option<String>,
    },
}

impl MediaSource {
    /// The URL the source names, for messages.
    #[must_use]
    pub const fn url(&self) -> &Url {
        match self {
            Self::Url(url) | Self::Bytes { url, .. } => url,
        }
    }
}

/// How a player starts out: whether it loops at its end and whether its
/// audio is muted. It starts paused, on its first frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MediaSettings {
    pub looping: bool,
    pub muted: bool,
}

/// One instruction to a player.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaCommand {
    /// Play from where it is; from the start after it has ended.
    Play,
    /// Stop advancing; the frame shown stays.
    Pause,
    /// Whether to loop at the end.
    Loop(bool),
    /// Whether the audio is muted.
    Muted(bool),
}

/// Why a player failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaErrorKind {
    /// The source could not be opened: it does not resolve, its bytes could
    /// not be fetched, or the player cannot read it.
    Source,
    /// The source opened and could not be played: a decode failure, or a
    /// `play` the platform refused (a browser's autoplay rule).
    Playback,
    /// The platform has no player.
    Unsupported,
}

/// A player's failure: what kind, and the platform's own message.
#[derive(Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[error("{message}")]
pub struct MediaError {
    pub kind: MediaErrorKind,
    pub message: String,
}

impl MediaError {
    #[must_use]
    pub fn new(kind: MediaErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// What a player reports besides its frames.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MediaEvent {
    /// The first frame is ready to be shown, before any `play`: the video's
    /// natural size in pixels and its duration, zero when it has none (a
    /// live stream). Reported once per open.
    FirstFrame {
        width: u32,
        height: u32,
        duration: Duration,
    },
    /// Playback reached the end and stopped there; a looping player does
    /// not end.
    Ended,
    /// The player failed. One that fails to open reports nothing else.
    Failed(MediaError),
}

/// Where a player hands what it produces. Implemented by the store that
/// owns the picture; a player holds it, on whatever thread its platform
/// produces frames on.
pub trait MediaSink<F>: Threaded {
    /// Whether frames are wanted now: `false` while no view draws the
    /// picture, when a player may skip every frame but the one it stops on.
    fn wants_frames(&self) -> bool;

    /// A new frame, the newest one: CPU pixels (premultiplied RGBA, as the
    /// engine's writer expects) or a frame of the platform's own type, named
    /// by `descriptor` ([`ImageDescriptor::of`] for pixels,
    /// [`ImageDescriptor::platform`] for a platform frame). Must not block.
    fn frame(&self, frame: PictureFrame<F>, descriptor: ImageDescriptor);

    /// One of the three reports, in the order they happened.
    fn event(&self, event: MediaEvent);
}

/// One open video: what an element's methods and attributes reach.
/// Dropping it closes the video and stops its frames.
pub trait MediaPlayer {
    fn play(&self);
    fn pause(&self);
    fn set_loop(&self, looping: bool);
    fn set_muted(&self, muted: bool);

    /// [`Self::play`], [`Self::pause`], [`Self::set_loop`] or
    /// [`Self::set_muted`], by `command`.
    fn command(&self, command: MediaCommand) {
        match command {
            MediaCommand::Play => self.play(),
            MediaCommand::Pause => self.pause(),
            MediaCommand::Loop(looping) => self.set_loop(looping),
            MediaCommand::Muted(muted) => self.set_muted(muted),
        }
    }
}

/// The platform's player: opens a source for one picture, whose frames and
/// reports go to `sink`. `F` is the platform's frame type, the decoder's
/// ([`crate::ImageDecoder::Frame`]): the frames a player hands over are drawn
/// by the same writer as a decoded image's.
///
/// `open` answers at once. Opening a URL is asynchronous on every platform,
/// so a source that turns out not to play is a [`MediaEvent::Failed`]
/// reported later; an `Err` is a failure known before anything started, and
/// is not reported to the sink as well.
pub trait Media<F>: Threaded {
    type Player: MediaPlayer;

    /// Whether [`Self::open`] is handed `url` itself ([`MediaSource::Url`])
    /// rather than its bytes, which the caller fetches first: a player reads
    /// the URLs its platform streams from. The default reads `http` and
    /// `https`.
    fn reads(&self, url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
    }

    /// Opens `source`, paused on its first frame, under `settings`.
    ///
    /// # Errors
    ///
    /// When nothing could be started: a platform with no player
    /// ([`MediaErrorKind::Unsupported`]), or a source it refuses outright
    /// ([`MediaErrorKind::Source`]).
    fn open<S: MediaSink<F>>(
        &self,
        source: MediaSource,
        settings: MediaSettings,
        sink: S,
    ) -> Result<Self::Player, MediaError>;
}

/// The [`Media`] of a platform with no player: every open fails with
/// [`MediaErrorKind::Unsupported`], which the element reports as its
/// `error`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoMedia;

/// The message every [`NoMedia`] open fails with.
pub const NO_MEDIA_MESSAGE: &str = "this platform has no video player";

/// The player [`NoMedia`] never answers.
#[derive(Clone, Copy, Debug)]
pub enum NoPlayer {}

impl MediaPlayer for NoPlayer {
    fn play(&self) {
        match *self {}
    }

    fn pause(&self) {
        match *self {}
    }

    fn set_loop(&self, _looping: bool) {
        match *self {}
    }

    fn set_muted(&self, _muted: bool) {
        match *self {}
    }
}

impl<F> Media<F> for NoMedia {
    type Player = NoPlayer;

    /// Every URL: nothing is fetched for an open that fails anyway.
    fn reads(&self, _url: &Url) -> bool {
        true
    }

    fn open<S: MediaSink<F>>(
        &self,
        _source: MediaSource,
        _settings: MediaSettings,
        _sink: S,
    ) -> Result<NoPlayer, MediaError> {
        Err(MediaError::new(
            MediaErrorKind::Unsupported,
            NO_MEDIA_MESSAGE,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<MediaEvent>>);

    impl MediaSink<Infallible> for std::sync::Arc<Recorder> {
        fn wants_frames(&self) -> bool {
            true
        }

        fn frame(&self, _frame: PictureFrame, _descriptor: ImageDescriptor) {}

        fn event(&self, event: MediaEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    /// A platform with no player fails every open at once, with the kind
    /// an element reports, and reports nothing to the sink.
    #[test]
    fn no_media_fails_every_open() {
        let sink = std::sync::Arc::new(Recorder::default());
        let opened = Media::<Infallible>::open(
            &NoMedia,
            MediaSource::Url(Url::parse("https://example.com/a.mp4").unwrap()),
            MediaSettings::default(),
            std::sync::Arc::clone(&sink),
        );
        let error = opened.expect_err("no player opens");
        assert_eq!(error.kind, MediaErrorKind::Unsupported);
        assert_eq!(error.message, NO_MEDIA_MESSAGE);
        assert!(sink.0.lock().unwrap().is_empty());
    }
}
