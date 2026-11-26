//! Pictures: what an image draw samples, named by a key, with a generation
//! that changes exactly when its pixels do.
//!
//! A picture is either the URL a page wrote ([`PictureKey::Url`], shared by
//! every draw of that URL in a document) or a process-unique
//! [`PictureId`] one element owns ([`PictureKey::Id`]). Nothing about a draw
//! distinguishes the two: `object-fit`, the sampler, the clip shape and the
//! scroll space apply to both, and the painter asks one store about both
//! ([`FrameImages`](crate::render::image::FrameImages)).
//!
//! # Frames and slots
//!
//! An `Id` picture's content arrives as frames. A [`PictureSlot`] holds a
//! picture's newest frame: a producer on any thread replaces it
//! ([`PictureSlot::publish_pixels`]) and wakes its host when the slot says a
//! wake is owed, and the painter takes it ([`PictureSlot::take`]) when it
//! next adopts. Nothing queues behind the newest frame: a frame replaced
//! before the painter took it is never drawn.
//!
//! A frame is CPU pixels ([`PictureFrame::Pixels`]) or a value of the
//! embedder's own type ([`PictureFrame::Platform`]), named by
//! [`FrameImages::PlatformFrame`](crate::render::image::FrameImages::PlatformFrame)
//! — an `ImageBitmap` in the browser. The painter writes CPU pixels itself;
//! a platform frame writes itself ([`FrameWriter`]) into a texture the
//! painter created for it, on the painter's thread. A store with no
//! platform frames names [`std::convert::Infallible`], which needs no
//! writer.
//!
//! A frame is used on the thread of the store that answers it, which is the
//! painter's. A producer on another thread hands its frame to that thread
//! by its platform's own means first — the browser transfers an
//! `ImageBitmap` or a `VideoFrame` to the Render Worker with `postMessage` —
//! and publishes it there.
//!
//! The thread that builds the document holds a picture's key and its natural
//! size, never a frame.
//!
//! # Playback
//!
//! An `Id` picture may be filled by one of two players. The store plays an
//! animated image's frames under a playhead of its own
//! ([`vant::image::animation`]), or the platform's media player plays a
//! video into it ([`vant::media`]). Whoever owns the picture asks the store
//! for either with a [`PlaybackRequest`] — play a URL's frames under a
//! [`PlayPolicy`], open a video under [`MediaSettings`], one of the four
//! [`PlaybackControl`]s or a [`MediaCommand`], or the end of the picture —
//! and the store reports what the player did as a [`PlaybackReport`]
//! ([`ImageReports::playback`](crate::render::image::ImageReports::playback)),
//! which travels back with the image reports: an animation's loops, or a
//! video's [`MediaEvent`]s. Which element owns the picture, and what it
//! makes of a report, is the embedder's.

use std::convert::Infallible;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

pub use vant::image::PictureFrame;
pub use vant::image::animation::{Control as PlaybackControl, Loops, PlayPolicy};
pub use vant::media::{MediaCommand, MediaError, MediaErrorKind, MediaEvent, MediaSettings};

use crate::clock::ClockInstant;
use crate::peniko::ImageData;
use crate::render::image::{FrameImage, ImageDescriptor};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackRequest {
    Play {
        picture: PictureId,
        source: Arc<str>,
        policy: PlayPolicy,
    },
    Control {
        picture: PictureId,
        control: PlaybackControl,
    },
    Open {
        picture: PictureId,
        source: Arc<str>,
        settings: MediaSettings,
    },
    Media {
        picture: PictureId,
        command: MediaCommand,
    },
    End {
        picture: PictureId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PlaybackReport {
    LoopComplete,
    FinalLoopComplete,
    Media(MediaEvent),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PictureId(NonZeroU64);

impl PictureId {
    #[must_use]
    #[expect(
        clippy::new_without_default,
        reason = "every call answers a different id, which `Default` would hide"
    )]
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Self(NonZeroU64::new(id).expect("the counter starts at one and does not wrap"))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PictureKey {
    Url(Arc<str>),
    Id(PictureId),
}

impl PictureKey {
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Url(url) => Some(url),
            Self::Id(_) => None,
        }
    }

    #[must_use]
    pub const fn id(&self) -> Option<PictureId> {
        match self {
            Self::Url(_) => None,
            Self::Id(id) => Some(*id),
        }
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Url(a), Self::Url(b)) => Arc::ptr_eq(a, b),
            (Self::Id(a), Self::Id(b)) => a == b,
            _ => false,
        }
    }
}

impl From<Arc<str>> for PictureKey {
    fn from(url: Arc<str>) -> Self {
        Self::Url(url)
    }
}

impl From<PictureId> for PictureKey {
    fn from(id: PictureId) -> Self {
        Self::Id(id)
    }
}

#[derive(Debug)]
pub struct FrameTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub texture: &'a wgpu::Texture,
    pub size: (u32, u32),
}

pub trait FrameWriter {
    fn write(&self, target: &FrameTarget<'_>) -> bool;
}

impl FrameWriter for Infallible {
    fn write(&self, _target: &FrameTarget<'_>) -> bool {
        match *self {}
    }
}

#[derive(Clone, Debug)]
pub struct TakenFrame<P = Infallible> {
    pub generation: u64,
    pub descriptor: ImageDescriptor,
    pub frame: PictureFrame<P>,
}

#[derive(Debug)]
struct SlotState<P> {
    frame: Option<PictureFrame<P>>,
    descriptor: Option<ImageDescriptor>,
    generation: u64,
    next_change: Option<ClockInstant>,
}

#[derive(Debug)]
pub struct PictureSlot<P = Infallible> {
    state: Arc<Mutex<SlotState<P>>>,
}

impl<P> Clone for PictureSlot<P> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl<P> Default for PictureSlot<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P> PictureSlot<P> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(SlotState {
                frame: None,
                descriptor: None,
                generation: 0,
                next_change: None,
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SlotState<P>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[must_use = "a wake the host is owed is lost"]
    pub fn publish_pixels(&self, pixels: ImageData, next_change: Option<ClockInstant>) -> bool {
        let descriptor = ImageDescriptor::of(&pixels);
        self.publish(PictureFrame::Pixels(pixels), descriptor, next_change)
    }

    #[must_use = "a wake the host is owed is lost"]
    pub fn publish(
        &self,
        frame: PictureFrame<P>,
        descriptor: ImageDescriptor,
        next_change: Option<ClockInstant>,
    ) -> bool {
        let mut state = self.lock();
        let waiting = state.frame.replace(frame).is_some();
        state.descriptor = Some(descriptor);
        state.generation += 1;
        state.next_change = next_change;
        !waiting
    }

    #[must_use]
    pub fn describe(&self) -> Option<FrameImage<P>> {
        let state = self.lock();
        Some(FrameImage {
            descriptor: state.descriptor?,
            pixels: None,
            generation: state.generation,
            next_change: state.next_change,
        })
    }

    #[must_use]
    pub fn take(&self) -> Option<TakenFrame<P>> {
        let mut state = self.lock();
        let frame = state.frame.take()?;
        Some(TakenFrame {
            generation: state.generation,
            descriptor: state.descriptor?,
            frame,
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use super::{PictureId, PictureKey, PictureSlot};
    use crate::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

    fn texel(value: u8) -> ImageData {
        ImageData {
            data: Blob::new(Arc::new(vec![value; 4])),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn a_slot_holds_the_newest_frame_and_owes_one_wake_per_take() {
        let slot: PictureSlot = PictureSlot::new();
        assert!(slot.describe().is_none(), "nothing published");
        assert!(slot.take().is_none());

        assert!(slot.publish_pixels(texel(1), None), "the first frame wakes");
        assert!(
            !slot.publish_pixels(texel(2), None),
            "its wake is still owed"
        );
        let described = slot.describe().expect("described");
        assert_eq!(described.generation, 2);
        assert!(described.pixels.is_none(), "frames are taken, not lent");

        let taken = slot.take().expect("the newest frame");
        assert_eq!(taken.generation, 2);
        let pixels = taken.frame.into_pixels();
        assert_eq!(
            pixels.data.data(),
            [2; 4],
            "the frame replaced unseen is gone"
        );
        assert!(slot.take().is_none(), "taken once");
        assert_eq!(slot.describe().map(|state| state.generation), Some(2));

        assert!(
            slot.publish_pixels(texel(3), None),
            "a frame after a take wakes"
        );
    }

    #[test]
    fn keys_name_one_picture_each() {
        assert_ne!(PictureId::new(), PictureId::new());
        let url: Arc<str> = Arc::from("app:///a.png");
        let same = PictureKey::Url(Arc::clone(&url));
        assert!(same.same(&PictureKey::Url(url)));
        assert!(!same.same(&PictureKey::Url(Arc::from("app:///a.png"))));
        let id = PictureId::new();
        assert!(PictureKey::Id(id).same(&PictureKey::Id(id)));
        assert!(!PictureKey::Id(id).same(&same));
    }
}
