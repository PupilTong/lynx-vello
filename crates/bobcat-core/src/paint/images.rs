//! The painter's end of the image system: the store it owns, the inbox it
//! takes reports from, and the pixels one commit draws.
//!
//! The whole image resource system lives on this thread. The Lynx main thread
//! holds names and load states; it never sees a store, a buffer or a
//! `peniko::ImageData`, and no channel between the two can carry one.
//!
//! Because the store never leaves this thread it is held by value, as a type
//! parameter rather than a trait object, and needs neither `Send` nor `Sync` —
//! which is what lets a wasm store hold browser objects directly. Nor does the
//! handle it reports through: every type on this path is a concrete,
//! thread-bound value, and the only thing here that crosses a thread is the
//! batch of [`ImageEvent`]s the painter forwards to the Lynx main thread once
//! it has drained them.
//!
//! A host that decodes off-thread synchronises that itself. It already drives
//! the painter's turns, so it has somewhere to do it; putting the machinery
//! here would charge every host for a capability the browser — the one host
//! that will actually load images — does not use, since its decode callbacks
//! land on the painter's own event loop.

use std::sync::Arc;

use dom::vello::peniko::ImageData;
use dom::{ImageEvent, ImageInbox, ImageReports};

use crate::resource::ResourceFetcher;

/// The painter's image state: the host's resource system, and the pixels the
/// current commit draws.
///
/// The resolved table is deliberately **not** a second cache. It is one
/// commit's pixels in draw order, rebuilt whenever the commit or the image
/// epoch moves, applying no policy of its own and holding shallow
/// `ImageData` clones — the same `Blob`, so one entry costs a reference count
/// rather than a bitmap. Every decision about what stays in memory belongs to
/// the host.
///
/// It is indexed rather than keyed: composition replays the program on every
/// frame that scrolls, and a slice index costs nothing where a URL hash would
/// have cost a lookup per draw per frame.
pub(crate) struct PainterImages<F> {
    store: F,
    inbox: ImageInbox,
    /// `(commit_id, epoch)` the table was built for.
    key: Option<(u64, u64)>,
    /// One entry per image draw of that commit, in draw order.
    resolved: Vec<Option<ImageData>>,
    /// Scratch for the distinct sources of one resolve, reused across
    /// commits. Only the hint the host is given is read from it.
    sources: Vec<Arc<str>>,
    /// Bumped when a report changes what an already-composed frame would
    /// draw, so a target rendered before that is not re-served after it.
    epoch: u64,
}

impl<F> std::fmt::Debug for PainterImages<F> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PainterImages")
            .field("epoch", &self.epoch)
            .field("resolved", &self.resolved.len())
            .finish_non_exhaustive()
    }
}

impl<F: ResourceFetcher> PainterImages<F> {
    /// Mints this view's sink and builds the host's resource system from it.
    ///
    /// The sink comes first, and the store is built *from* it, so a store
    /// that exists without its report channel is unrepresentable and the two
    /// are paired by construction. That pairing is per view: a host whose
    /// registry outlives the view returns a per-view value holding a shared
    /// handle on it, and that value — not the registry — is what carries the
    /// sink. A load in flight when a view is replaced therefore reports to
    /// the queue it was started for, which teardown has already detached,
    /// rather than into its successor's document.
    ///
    /// A builder rather than a built value for one more reason: it runs only
    /// once every fallible step of construction has succeeded, so a host pays
    /// for a resource system only for a view that will exist.
    pub(crate) fn new<B>(build: B) -> Self
    where
        B: FnOnce(ImageReports) -> F,
    {
        let (reports, inbox) = ImageInbox::new();
        let store = build(reports);
        Self {
            store,
            inbox,
            key: None,
            resolved: Vec::new(),
            sources: Vec::new(),
            epoch: 0,
        }
    }

    /// The host's resource system, for the startup loads that need it.
    pub(crate) fn store(&self) -> &F {
        &self.store
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Names every source the document asked about, starting whatever load
    /// each needs.
    ///
    /// Non-blocking, and there is nothing to send back: an answer arrives
    /// later through the sink, keyed by the same source string.
    pub(crate) fn request(&self, sources: Vec<Arc<str>>) {
        for source in sources {
            self.store.request_image(source.as_ref());
        }
    }

    /// Takes the reports the store has queued, bumping the epoch when there
    /// are any. Empty when a wakeup raced another drain.
    pub(crate) fn take_reports(&mut self) -> Vec<ImageEvent> {
        let events = self.inbox.drain();
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
        frame.resolve_images(&self.store, &mut self.resolved, &mut self.sources);
        self.store.retain_images(&self.sources);
        self.key = Some(key);
    }

    /// This commit's pixels, in draw order.
    pub(crate) fn resolved(&self) -> &[Option<ImageData>] {
        &self.resolved
    }
}

impl<F> PainterImages<F> {
    /// Stops accepting reports. Called once, at teardown, before the store
    /// drops with the painter.
    pub(crate) fn detach(&self) {
        self.inbox.detach();
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {

    /// Decision: one bitmap, reused. Every read of one source must hand back
    /// a clone sharing the *same* `Blob`, because `Blob::id()` is what vello
    /// keys its image atlas on — two ids for one image would be two GPU
    /// uploads and two atlas slots.
    #[test]
    fn every_read_of_one_image_keeps_the_same_buffer_identity() {
        use dom::FrameImages as _;

        let images = flashbulb::TestImages::new();
        let pixels = flashbulb::rgba8(1, 1, vec![1, 2, 3, 255]);
        let published = pixels.data.id();
        images.insert("app:///pixel.png", pixels);

        let first = images
            .read("app:///pixel.png")
            .expect("a published image reads back");
        let second = images
            .read("app:///pixel.png")
            .expect("and reads back again");
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

    /// A source the host carries no pixels for reads as nothing rather than
    /// standing in for another image.
    #[test]
    fn an_unpublished_source_reads_as_nothing() {
        use dom::FrameImages as _;

        let images = flashbulb::TestImages::new();
        assert!(images.read("app:///missing.png").is_none());
    }
}
