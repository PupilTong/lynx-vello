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

#[derive(Clone, Debug)]
pub enum MediaSource {
    Url(Url),
    Bytes {
        url: Url,
        bytes: Bytes,
        media_type: Option<String>,
    },
}

impl MediaSource {
    #[must_use]
    pub const fn url(&self) -> &Url {
        match self {
            Self::Url(url) | Self::Bytes { url, .. } => url,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MediaSettings {
    pub looping: bool,
    pub muted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaCommand {
    Play,
    Pause,
    Loop(bool),
    Muted(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaErrorKind {
    Source,
    Playback,
    Unsupported,
}

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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MediaEvent {
    FirstFrame {
        width: u32,
        height: u32,
        duration: Duration,
    },
    Ended,
    Failed(MediaError),
}

pub trait MediaSink<F>: Threaded {
    fn wants_frames(&self) -> bool;

    fn frame(&self, frame: PictureFrame<F>, descriptor: ImageDescriptor);

    fn event(&self, event: MediaEvent);
}

pub trait MediaPlayer {
    fn play(&self);
    fn pause(&self);
    fn set_loop(&self, looping: bool);
    fn set_muted(&self, muted: bool);

    fn command(&self, command: MediaCommand) {
        match command {
            MediaCommand::Play => self.play(),
            MediaCommand::Pause => self.pause(),
            MediaCommand::Loop(looping) => self.set_loop(looping),
            MediaCommand::Muted(muted) => self.set_muted(muted),
        }
    }
}

pub trait Media<F>: Threaded {
    type Player: MediaPlayer;

    fn reads(&self, url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
    }

    fn open<S: MediaSink<F>>(
        &self,
        source: MediaSource,
        settings: MediaSettings,
        sink: S,
    ) -> Result<Self::Player, MediaError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoMedia;

pub const NO_MEDIA_MESSAGE: &str = "this platform has no video player";

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
