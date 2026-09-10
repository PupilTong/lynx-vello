//! The painter's per-commit pixels: what one committed frame draws, read out
//! of the view's own resource system once per commit.
//!
//! The whole image resource system lives on this thread, owned by the
//! [`LynxView`](crate::LynxView) and serviced in its `pump`. The Lynx main
//! thread holds names and load states; it never sees a store, a buffer or a
//! `peniko::ImageData`, and no channel between the two can carry one. A
//! painter observing a view reads pixels out of that store through
//! [`FrameImages`](dom::FrameImages) and asks it for nothing else — which is
//! why what it holds is a table of resolved bitmaps rather than a store.
//!
//! A host that decodes off-thread synchronises that itself. It already drives
//! the view's turns, so it has somewhere to do it; putting the machinery
//! here would charge every host for a capability the browser — the one host
//! that will actually load images — does not use, since its decode callbacks
//! land on the embedder's own event loop.

use std::sync::Arc;

use dom::FrameImages;
use dom::vello::peniko::ImageData;

/// The pixels the current commit draws, in draw order.
///
/// The resolved table is deliberately **not** a cache. It is one commit's
/// pixels in draw order, rebuilt whenever the commit moves, applying no
/// policy of its own and holding shallow `ImageData` clones — the same
/// `Blob`, so one entry costs a reference count rather than a bitmap. Every
/// decision about what stays in memory belongs to the host.
///
/// It is indexed rather than keyed: composition replays the program on every
/// frame that scrolls, and a slice index costs nothing where a URL hash would
/// have cost a lookup per draw per frame.
#[derive(Debug, Default)]
pub(crate) struct PainterImages {
    /// The commit the table was built for.
    key: Option<u64>,
    /// One entry per image draw of that commit, in draw order.
    resolved: Vec<Option<ImageData>>,
    /// Scratch for the distinct sources of one resolve, reused across
    /// commits. Only the hint the host is given is read from it.
    sources: Vec<Arc<str>>,
}

impl PainterImages {
    /// Whether the table already holds this commit's pixels.
    pub(crate) fn holds(&self, commit: u64) -> bool {
        self.key == Some(commit)
    }

    /// Reads every image `frame` draws out of `images`, once per commit.
    ///
    /// The commit is the whole key. A report that changes what a frame draws
    /// dirties the document, and every rebuild takes a new commit id, so a
    /// separate image counter would only ever agree with this one.
    ///
    /// **May block.** A store is allowed — required, in fact — to restore a
    /// bitmap it evicted, and doing so inside this call is the whole point of
    /// the synchronous read. When it is safe to pay that is the painter's own
    /// `poll_link` to say: adopting a commit is the only thing that calls
    /// this, and it is what runs before a swap-chain image is acquired.
    pub(crate) fn resolve(&mut self, frame: &dom::CommittedFrame, images: &dyn FrameImages) {
        frame.resolve_images(images, &mut self.resolved, &mut self.sources);
        images.retain(&self.sources);
        self.key = Some(frame.commit_id());
    }

    /// This commit's pixels, in draw order.
    pub(crate) fn resolved(&self) -> &[Option<ImageData>] {
        &self.resolved
    }

    /// Drops the table, so the next commit resolves from scratch.
    ///
    /// What a painter does when it stops observing a view: the bitmaps came
    /// out of that view's store, and commit ids restart at one per document,
    /// so a table still keyed by the old page's commit would be handed to the
    /// next one.
    pub(crate) fn forget(&mut self) {
        self.key = None;
        self.resolved.clear();
        self.sources.clear();
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
            .read("app:///pixel.png", dom::ImageSizeHint::UNBOUNDED)
            .expect("a published image reads back");
        let second = images
            .read("app:///pixel.png", dom::ImageSizeHint::UNBOUNDED)
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
        assert!(
            images
                .read("app:///missing.png", dom::ImageSizeHint::UNBOUNDED)
                .is_none()
        );
    }
}
