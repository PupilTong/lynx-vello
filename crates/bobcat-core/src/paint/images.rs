//! The painter's end of the image system: the store it owns, the sink the
//! store reports through, and the pixels one commit draws.
//!
//! The whole image resource system lives on this thread. The Lynx main thread
//! holds names and load states; it never sees a store, a buffer or a
//! `peniko::ImageData`, and no channel between the two can carry one.
//!
//! Because the store never leaves this thread, it is held by [`Rc`] and needs
//! neither `Send` nor `Sync` — which is what lets a wasm store hold browser
//! objects directly. Nor does the factory that builds it: a view is painted by
//! whichever thread constructed it, so the store is built there and stays
//! there. The one direction that does cross threads is the store's own loaders
//! reporting completion, and that goes through [`ImageSink`], which is
//! `Send + Sync`.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dom::vello::peniko::ImageData;
use dom::{FrameImages, ImageEvent, ImageId, ImageRef, ImageSink};
use rustc_hash::FxHashMap;

use crate::resource::ResourceFetcher;
use crate::view::EventRequester;

/// Completed loads waiting for the painter to take its next turn.
///
/// Shared by the sink and the painter. The sink never touches the `Painter`
/// itself, so a store that outlives the view holds nothing that could block
/// the painter thread's join.
#[derive(Debug, Default)]
pub(crate) struct ImageQueue {
    events: Mutex<Vec<ImageEvent>>,
    /// Set at teardown. Lives here rather than on the sink because the
    /// painter holds the queue and the sink is owned by the store, which the
    /// painter cannot reach into once it has handed it over.
    detached: AtomicBool,
}

impl ImageQueue {
    /// Stops reporting, so a load completing against a torn-down view is
    /// dropped rather than queued into a painter that will never read it.
    pub(crate) fn detach(&self) {
        self.detached.store(true, Ordering::Relaxed);
    }

    fn is_detached(&self) -> bool {
        self.detached.load(Ordering::Relaxed)
    }

    fn push(&self, event: ImageEvent) {
        self.events
            .lock()
            .unwrap_or_else(|error| panic!("the image queue is poisoned: {error}"))
            .push(event);
    }

    pub(crate) fn drain(&self) -> Vec<ImageEvent> {
        std::mem::take(
            &mut *self
                .events
                .lock()
                .unwrap_or_else(|error| panic!("the image queue is poisoned: {error}")),
        )
    }
}

/// The sink handed to the store, generic over the host's wakeup so no
/// `dyn EventRequester` reappears anywhere.
///
/// Announce first, then ask for the turn — the same order the Lynx main
/// thread's own sender uses, so the fact is already readable when the turn
/// arrives. There is no command to send: the painter is the host's own
/// thread, and it drains this queue at the start of every turn. Waking the
/// host is therefore the whole mechanism, on every platform.
pub(crate) struct PainterImageSink<R: EventRequester> {
    queue: Arc<ImageQueue>,
    requester: Arc<R>,
}

impl<R: EventRequester> PainterImageSink<R> {
    pub(crate) fn new(queue: Arc<ImageQueue>, requester: Arc<R>) -> Self {
        Self { queue, requester }
    }

    fn post(&self, event: ImageEvent) {
        if self.queue.is_detached() {
            return;
        }
        self.queue.push(event);
        self.requester.request_event();
    }
}

impl<R: EventRequester> ImageSink for PainterImageSink<R> {
    fn loaded(&self, id: ImageId, generation: u32, width: u32, height: u32) {
        self.post(ImageEvent::Loaded {
            id,
            generation,
            width,
            height,
        });
    }

    fn failed(&self, id: ImageId) {
        self.post(ImageEvent::Failed { id });
    }
}

/// The painter's image state: the store, and the pixels the current commit
/// draws.
///
/// The resolved map is deliberately **not** a second cache. It holds one
/// commit's working set, is cleared whenever the commit or the image epoch
/// moves, applies no policy of its own, and stores shallow `ImageData` clones
/// — the same `Blob`, so one entry costs a reference count rather than a
/// bitmap. Every decision about what stays in memory belongs to the store.
#[derive(Default)]
pub(crate) struct PainterImages {
    store: Option<Rc<dyn ResourceFetcher>>,
    queue: Arc<ImageQueue>,
    /// `(commit_id, epoch)` the resolved map was built for.
    key: Option<(u64, u64)>,
    resolved: FxHashMap<ImageRef, ImageData>,
    /// This commit's distinct images in first-draw order — the store's
    /// residency hint, and the set the resolve pass reads.
    working_set: Vec<ImageRef>,
    /// Bumped by every batch of load reports, so a frame composed before an
    /// image arrived is not mistaken for one composed after.
    epoch: u64,
}

impl std::fmt::Debug for PainterImages {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PainterImages")
            .field("installed", &self.store.is_some())
            .field("epoch", &self.epoch)
            .field("resolved", &self.resolved.len())
            .finish_non_exhaustive()
    }
}

impl PainterImages {
    /// The sink this view's store reports through.
    pub(crate) fn sink<R: EventRequester>(&self, requester: Arc<R>) -> Arc<dyn ImageSink> {
        Arc::new(PainterImageSink::new(Arc::clone(&self.queue), requester))
    }

    pub(crate) fn install(&mut self, store: Rc<dyn ResourceFetcher>) {
        self.store = Some(store);
    }

    /// An owned handle on the host's resource system.
    ///
    /// Owned rather than borrowed because a `ResourceFuture` borrows the
    /// fetcher across an await, and the caller uses the link afterwards.
    pub(crate) fn fetcher(&self) -> Option<Rc<dyn ResourceFetcher>> {
        self.store.clone()
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Names every source, returning the bindings for the main thread.
    ///
    /// Non-blocking: `request` starts work and answers with an id at once.
    pub(crate) fn request(&self, sources: Vec<Arc<str>>) -> Vec<ImageEvent> {
        let Some(store) = &self.store else {
            return Vec::new();
        };
        sources
            .into_iter()
            .filter_map(|source| {
                store
                    .request_image(source.as_ref())
                    .map(|id| ImageEvent::Bound { source, id })
            })
            .collect()
    }

    pub(crate) fn release(&self, ids: &[ImageId]) {
        if let Some(store) = &self.store {
            for id in ids {
                store.release_image(*id);
            }
        }
    }

    /// Takes the reports the store has queued, bumping the epoch when there
    /// are any. Empty when a wakeup raced another drain.
    pub(crate) fn take_reports(&mut self) -> Vec<ImageEvent> {
        let events = self.queue.drain();
        if !events.is_empty() {
            self.epoch = self.epoch.wrapping_add(1);
        }
        events
    }

    /// Reads every image `frame` draws, once per `(commit, epoch)`.
    ///
    /// **May block.** A store is allowed — required, in fact — to restore a
    /// bitmap it evicted, and doing so inside this call is the whole point of
    /// the synchronous read. It therefore runs before a swap-chain image is
    /// acquired, never while one is held.
    pub(crate) fn resolve(&mut self, frame: &dom::CommittedFrame) {
        let key = (frame.commit_id(), self.epoch);
        if self.key == Some(key) {
            return;
        }
        self.working_set.clear();
        self.resolved.clear();
        if let Some(store) = &self.store {
            frame.collect_images(&mut self.working_set);
            store.retain_images(&self.working_set);
            for image in &self.working_set {
                if let Some(data) = store.read(*image) {
                    self.resolved.insert(*image, data);
                }
            }
        }
        self.key = Some(key);
    }

    /// Stops accepting reports. Called once, at teardown, before the store
    /// drops with the painter.
    pub(crate) fn detach(&self) {
        self.queue.detach();
    }
}

impl FrameImages for PainterImages {
    fn read(&self, image: ImageRef) -> Option<ImageData> {
        // A clone of the store's own `ImageData`: the same `Blob`, so this is
        // one reference count and never a pixel copy, and every read of one
        // `ImageRef` keeps the same `Blob::id()` for vello's atlas.
        self.resolved.get(&image).cloned()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use dom::{ImageEvent, ImageId, ImageSink};

    use super::{ImageQueue, PainterImageSink};
    use crate::view::NoWakeup;

    fn id(raw: u32) -> ImageId {
        ImageId(NonZeroU32::new(raw).expect("a non-zero id"))
    }

    /// A report is readable before the wakeup that carries it, and each one
    /// asks the host for exactly one turn.
    #[test]
    fn reports_queue_before_the_wakeup_that_carries_them() {
        #[derive(Default)]
        struct CountingWakeup(AtomicUsize);
        impl crate::view::EventRequester for CountingWakeup {
            fn request_event(&self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let queue = Arc::new(ImageQueue::default());
        let requester = Arc::new(CountingWakeup::default());
        let sink = PainterImageSink::new(Arc::clone(&queue), Arc::clone(&requester));

        sink.loaded(id(1), 1, 4, 4);
        sink.failed(id(2));

        assert_eq!(
            queue.drain(),
            vec![
                ImageEvent::Loaded {
                    id: id(1),
                    generation: 1,
                    width: 4,
                    height: 4
                },
                ImageEvent::Failed { id: id(2) },
            ]
        );
        assert!(queue.drain().is_empty(), "a drain empties the queue");
        assert_eq!(
            requester.0.load(Ordering::Relaxed),
            2,
            "each report asks the host for a turn"
        );
    }

    /// Decision: one bitmap, reused. Every read of one `ImageRef` must hand
    /// back a clone sharing the *same* `Blob`, because `Blob::id()` is what
    /// vello keys its image atlas on — two ids for one image would be two
    /// GPU uploads and two atlas slots.
    #[test]
    fn every_read_of_one_image_keeps_the_same_buffer_identity() {
        use dom::{FrameImages as _, ImageRef};

        let images = flashbulb::TestImages::new();
        let pixels = flashbulb::rgba8(1, 1, vec![1, 2, 3, 255]);
        let published = pixels.data.id();
        images.insert("app:///pixel.png", pixels);

        let image = ImageRef {
            id: images.request("app:///pixel.png"),
            generation: 1,
        };

        let first = images.read(image).expect("a published image reads back");
        let second = images.read(image).expect("and reads back again");
        assert_eq!(
            first.data.id(),
            published,
            "the buffer that comes out is the one that went in — no copy"
        );
        assert_eq!(
            first.data.id(),
            second.data.id(),
            "and every later read is the same GPU resource"
        );
    }

    /// A source the store carries no pixels for reads as nothing rather than
    /// standing in for another image.
    #[test]
    fn an_unpublished_source_reads_as_nothing() {
        use dom::{FrameImages as _, ImageRef};

        let images = flashbulb::TestImages::new();
        let id = images.request("app:///missing.png");
        assert!(images.read(ImageRef { id, generation: 1 }).is_none());
    }

    /// A store outliving its view must not keep queuing into a dead painter.
    #[test]
    fn a_detached_sink_reports_nothing() {
        let queue = Arc::new(ImageQueue::default());
        let sink = PainterImageSink::new(Arc::clone(&queue), Arc::new(NoWakeup));

        queue.detach();
        sink.loaded(id(1), 1, 4, 4);

        assert!(queue.drain().is_empty());
    }
}
