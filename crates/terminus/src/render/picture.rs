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
//! A frame is CPU pixels today. [`PictureFrame::Platform`] holds a value of
//! the embedder's own type, named by
//! [`FrameImages::PlatformFrame`](crate::render::image::FrameImages::PlatformFrame);
//! the painter accepts only stores whose platform frame type is uninhabited
//! ([`std::convert::Infallible`]), so no platform frame reaches it yet.
//!
//! The thread that builds the document holds a picture's key and its natural
//! size, never a frame.

use std::convert::Infallible;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::clock::ClockInstant;
use crate::peniko::ImageData;
use crate::render::image::{FrameImage, ImageDescriptor};

/// A picture one element owns, unique in the process: two ids are never
/// equal unless one was copied from the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PictureId(NonZeroU64);

impl PictureId {
    /// An id no other picture in the process has had.
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

    /// The id as a number, for logs and hashing.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// What an image draw names its picture by.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PictureKey {
    /// The raw source string the page wrote. A document's registry hands out
    /// one allocation per source, so every draw of one URL in one frame
    /// shares it.
    Url(Arc<str>),
    /// A picture one element owns, whose frames a producer publishes.
    Id(PictureId),
}

impl PictureKey {
    /// The URL, for a [`Self::Url`] picture.
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Url(url) => Some(url),
            Self::Id(_) => None,
        }
    }

    /// The id, for a [`Self::Id`] picture.
    #[must_use]
    pub const fn id(&self) -> Option<PictureId> {
        match self {
            Self::Url(_) => None,
            Self::Id(id) => Some(*id),
        }
    }

    /// Whether `self` and `other` name the same picture, by the cheapest
    /// test that is exact for keys a frame holds: a URL by its allocation,
    /// since one frame's draws of one URL share it, and an id by value.
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

/// One frame of a picture.
#[derive(Clone, Debug)]
pub enum PictureFrame<P = Infallible> {
    /// CPU pixels, which the painter converts and writes into the picture's
    /// texture.
    Pixels(ImageData),
    /// A frame in the embedder's own form. Not drawn yet: the painter accepts
    /// only stores whose platform frame type is uninhabited.
    Platform(P),
}

/// A frame taken out of a slot, with the generation it was published as.
#[derive(Clone, Debug)]
pub struct TakenFrame<P = Infallible> {
    /// The generation [`FrameImage::generation`] answered for this frame.
    pub generation: u64,
    pub frame: PictureFrame<P>,
}

/// What a slot holds: its newest frame, until the painter takes it, and
/// what describes that frame after it has been taken.
#[derive(Debug)]
struct SlotState<P> {
    frame: Option<PictureFrame<P>>,
    descriptor: Option<ImageDescriptor>,
    generation: u64,
    next_change: Option<ClockInstant>,
}

/// One picture's newest frame, shared between the producer that publishes
/// it and the store that answers the painter for it.
///
/// Cloning shares the slot. A producer on any thread publishes; the store,
/// on the painter's thread, describes and takes. The generation starts at
/// zero, with no frame, and each publish moves it on by one.
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
    /// An empty slot: nothing published yet.
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
        // A producer that panicked mid-publish left a whole frame or none.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the newest frame with `pixels`, moving the generation on,
    /// and records `next_change`, the instant the picture will next change
    /// by itself when the producer knows it. Answers whether the host has to
    /// be woken: `true` unless an earlier frame was still waiting to be
    /// taken, whose wake is still owed.
    #[must_use = "a wake the host is owed is lost"]
    pub fn publish_pixels(&self, pixels: ImageData, next_change: Option<ClockInstant>) -> bool {
        let descriptor = ImageDescriptor::of(&pixels);
        let mut state = self.lock();
        let waiting = state.frame.replace(PictureFrame::Pixels(pixels)).is_some();
        state.descriptor = Some(descriptor);
        state.generation += 1;
        state.next_change = next_change;
        !waiting
    }

    /// What the painter is told about the picture: the newest frame's
    /// descriptor, its generation and the next change, with no pixels —
    /// those are [`Self::take`]n. `None` before the first publish.
    #[must_use]
    pub fn describe(&self) -> Option<FrameImage> {
        let state = self.lock();
        Some(FrameImage {
            descriptor: state.descriptor?,
            pixels: None,
            generation: state.generation,
            next_change: state.next_change,
        })
    }

    /// Takes the newest frame, if it has not been taken yet.
    #[must_use]
    pub fn take(&self) -> Option<TakenFrame<P>> {
        let mut state = self.lock();
        let frame = state.frame.take()?;
        Some(TakenFrame {
            generation: state.generation,
            frame,
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use super::{PictureFrame, PictureId, PictureKey, PictureSlot};
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

    /// A slot holds the newest frame only: a publish over a frame nobody
    /// took replaces it and owes no second wake, the generation moves on by
    /// one per publish, a frame is taken once, and the description outlives
    /// the take.
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
        let PictureFrame::Pixels(pixels) = taken.frame;
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

    /// Ids are unique, and a URL key names its picture by allocation.
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
