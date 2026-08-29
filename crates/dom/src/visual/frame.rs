//! The committed frame: one commit's output, owned outright.
//!
//! A [`CommittedFrame`] carries everything the thread that does not hold the
//! document needs from one commit — the paint-order tables, the encoded
//! scene, and the scroll-slot table — with no borrow of the document that
//! built it. `NodeId`s inside it stay safe however stale the frame gets: an
//! id is retired on free and never reissued, so it resolves to the node it
//! was built for or to nothing, never to a stranger. Liveness is therefore
//! not this type's concern; a consumer that must act on a node re-validates
//! against the document at the point of action.
//!
//! The scroll-slot table exists so input recognition can run without the
//! document: the nearest-scrollable walk, scroll chaining, and clamping all
//! read published geometry here instead of live styles. Offsets and bounds
//! are as of the commit; between commits they are a snapshot, which is the
//! same screen-semantics staleness hit testing already accepts.

use euclid::default::{Point2D, Size2D, Vector2D};

use super::PaintOrder;
use crate::NodeId;
use crate::paint::compose::{self, ComposeOp};
use crate::scroll::ScrollAxes;
use crate::vello::Scene;

/// One scroll container in the committed frame, linked to the nearest scroll
/// container on its containing-block chain.
///
/// The chain is containing-block-based, not DOM-based, because that is what
/// scrolling follows: a box is only carried by, and only chains into,
/// scroll containers it is laid out inside of (CSS2 §11.1.1). The builder
/// assigns entries with the same escape rules the paint order itself uses,
/// so an absolutely-positioned box whose containing block is outside its
/// DOM-side scroller correctly links past it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollSlot {
    /// The scroll container element.
    pub node: NodeId,
    /// The nearest enclosing scroll container on the containing-block chain.
    pub parent: Option<u32>,
    /// The axes the user may scroll directly (`overflow: scroll`); a
    /// `hidden` container is in the table — it scrolls programmatically and
    /// carries chain structure — with both flags off.
    pub user_scrollable: ScrollAxes,
    /// The committed, already-clamped offset.
    pub offset: Vector2D<f32>,
    /// The largest offset the committed geometry admits, per axis.
    pub max_offset: Vector2D<f32>,
    /// The scrollport (padding box) size, which is also what the encode
    /// window is sized from.
    pub scrollport: Size2D<f32>,
}

impl ScrollSlot {
    /// The offset range the committed encode covers on each axis — the
    /// window compose may move through without a recommit. Sized in
    /// scrollports around the committed offset, clamped to what the geometry
    /// admits.
    #[must_use]
    pub fn encode_window(&self) -> (Vector2D<f32>, Vector2D<f32>) {
        let slack = Vector2D::new(
            if self.max_offset.x > 0.0 {
                self.scrollport.width * ENCODE_WINDOW_SCROLLPORTS
            } else {
                0.0
            },
            if self.max_offset.y > 0.0 {
                self.scrollport.height * ENCODE_WINDOW_SCROLLPORTS
            } else {
                0.0
            },
        );
        let low = Vector2D::new(
            (self.offset.x - slack.x).max(0.0),
            (self.offset.y - slack.y).max(0.0),
        );
        let high = Vector2D::new(
            (self.offset.x + slack.x).min(self.max_offset.x),
            (self.offset.y + slack.y).min(self.max_offset.y),
        );
        (low, high)
    }
}

/// How far past the committed offset, in scrollports per scrollable axis,
/// the encode covers — the compose headroom before a refill commit is due.
pub const ENCODE_WINDOW_SCROLLPORTS: f32 = 1.0;

/// What a frame hit test reports: the element to act on, plus the scroll
/// slot recognition starts its chain walk from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitTarget {
    /// The topmost hit-testable element at the point. May be freed by the
    /// time a consumer acts on it; the id then resolves to nothing.
    pub node: NodeId,
    /// The nearest ancestor-or-self scroll container of the hit item, as an
    /// index into [`CommittedFrame::scroll_slots`].
    pub scroll: Option<u32>,
}

/// One commit's published output. Immutable, self-contained, and shared by
/// `Arc`: the committer retains one for its own queries, the compositor holds
/// one to draw and route input from.
///
/// The scene is carried *split*: per-chain fragments plus the compose
/// program over them, so scroll offsets apply at composition. The
/// [`Self::scene`] accessor materializes the frame at its committed offsets
/// once, on demand, for consumers that want the whole picture.
pub struct CommittedFrame {
    pub(crate) order: PaintOrder,
    pub(crate) presentation: Presentation,
    pub(crate) animations_active: bool,
    pub(crate) viewport: Size2D<f32>,
    pub(crate) device_pixel_ratio: f32,
}

/// The split scene: fragments, the program that assembles them, and the
/// frame already composed at its committed offsets.
pub(crate) struct Presentation {
    pub(crate) fragments: Vec<Scene>,
    pub(crate) program: Vec<ComposeOp>,
    committed: CommittedScene,
}

/// How the committed-offset composition is stored.
///
/// The common frame shape — no scroll containers, no group effects — has no
/// walker-level ops and nothing to translate: its program is one untranslated
/// fragment append, so the composition *is* that fragment and borrowing it
/// costs nothing. Every other shape is composed once, at commit, into a
/// scene from the painter's pool.
#[allow(
    clippy::large_enum_variant,
    reason = "one value per frame, and boxing the scene would reintroduce \
              the very allocation this type exists to avoid; `allow`, not \
              `expect` - the lint's size threshold is target-dependent and \
              an expectation unfulfilled on wasm32 fails that build"
)]
pub(crate) enum CommittedScene {
    /// `fragments[0]` is the whole composed frame.
    Whole,
    /// Composed at commit into a pooled scene.
    Composed(Scene),
}

impl Presentation {
    pub(crate) fn new(
        fragments: Vec<Scene>,
        program: Vec<ComposeOp>,
        committed: CommittedScene,
    ) -> Self {
        Self {
            fragments,
            program,
            committed,
        }
    }

    /// Dismantles the presentation for the painter's pools: the fragment
    /// scenes and their container, the program's storage, and the composed
    /// scene when one was built.
    pub(crate) fn into_parts(self) -> (Vec<Scene>, Vec<ComposeOp>, Option<Scene>) {
        let composed = match self.committed {
            CommittedScene::Whole => None,
            CommittedScene::Composed(scene) => Some(scene),
        };
        (self.fragments, self.program, composed)
    }
}

impl std::fmt::Debug for CommittedFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommittedFrame")
            .field("epoch", &self.epoch())
            .field("viewport", &self.viewport)
            .finish_non_exhaustive()
    }
}

impl CommittedFrame {
    /// The frame composed at its committed offsets, valid for
    /// [`Self::viewport`] at [`Self::device_pixel_ratio`] — the frame's one
    /// fragment when that is the whole program, otherwise the composition
    /// built at commit.
    #[must_use]
    pub fn scene(&self) -> &Scene {
        match &self.presentation.committed {
            CommittedScene::Whole => &self.presentation.fragments[0],
            CommittedScene::Composed(scene) => scene,
        }
    }

    /// Composes the frame into `scene` with each scroll slot at the offset
    /// `offset_of` reports for it, falling back to the committed one.
    ///
    /// This is the compositor's per-frame path: scrolling recomposes instead
    /// of recommitting, for as long as every overridden offset stays inside
    /// its slot's [`ScrollSlot::encode_window`].
    pub fn compose_into(
        &self,
        scene: &mut Scene,
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
    ) {
        compose::replay(
            scene,
            &self.presentation.fragments,
            &self.presentation.program,
            self.order.slots(),
            self.device_pixel_ratio,
            offset_of,
        );
    }

    /// The document's visual epoch this frame was built at. Monotonic across
    /// a document's life, so it orders commits.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.order.visual_epoch()
    }

    /// Whether the document had a running animation at commit time — the
    /// compositor's cue to keep asking the committer for frames.
    #[must_use]
    pub const fn animations_active(&self) -> bool {
        self.animations_active
    }

    /// The CSS-px viewport this frame was committed for.
    #[must_use]
    pub const fn viewport(&self) -> Size2D<f32> {
        self.viewport
    }

    #[must_use]
    pub const fn device_pixel_ratio(&self) -> f32 {
        self.device_pixel_ratio
    }

    /// The frame's scroll containers, chain-linked; see [`ScrollSlot`].
    #[must_use]
    pub fn scroll_slots(&self) -> &[ScrollSlot] {
        self.order.slots()
    }

    /// The slot a scroll container node has in this frame, if it is one.
    #[must_use]
    pub fn slot_of(&self, node: NodeId) -> Option<u32> {
        self.order
            .slots()
            .iter()
            .position(|slot| slot.node == node)
            .map(|index| u32::try_from(index).expect("a frame cannot hold 2^32 scroll containers"))
    }

    /// The topmost hit-testable element at `point`, with its scroll slot.
    ///
    /// The frame is baked unscrolled; `offset_of` supplies each slot's
    /// current offset (the compositor's between-commit values), falling back
    /// to the committed ones. No liveness filter: this type has no document.
    /// Ids never alias, so a consumer acting on the node re-validates where
    /// it acts.
    #[must_use]
    pub fn hit(
        &self,
        point: Point2D<f32>,
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
    ) -> Option<HitTarget> {
        self.order
            .raw_hits_at(point, offset_of, self.device_pixel_ratio)
            .next()
    }

    /// The first slot on the chain from `from` (inclusive) the user may
    /// scroll on any of `axes` — the published-data equivalent of the
    /// document's `nearest_user_scrollable`.
    #[must_use]
    pub fn nearest_user_scrollable(&self, from: Option<u32>, axes: ScrollAxes) -> Option<u32> {
        let slots = self.order.slots();
        let mut current = from;
        while let Some(index) = current {
            let slot = slots[index as usize];
            if (slot.user_scrollable.x && axes.x) || (slot.user_scrollable.y && axes.y) {
                return Some(index);
            }
            current = slot.parent;
        }
        None
    }
}

impl PaintOrder {
    /// Front-to-back hits with their scroll slots, no liveness filter.
    pub(crate) fn raw_hits_at<'frame>(
        &'frame self,
        point: Point2D<f32>,
        offset_of: &'frame (dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>> + 'frame),
        ratio: f32,
    ) -> impl Iterator<Item = HitTarget> + 'frame {
        self.items().iter().rev().filter_map(move |item| {
            let node = self.item_hit(item, point, offset_of, ratio)?;
            Some(HitTarget {
                node,
                scroll: item.slot,
            })
        })
    }
}

/// The whole point of the type: it crosses threads.
#[allow(dead_code, reason = "compile-time thread-safety assertion")]
const fn assert_frame_is_shareable()
where
    CommittedFrame: Send + Sync,
{
}

#[cfg(test)]
mod tests {
    use euclid::default::Point2D;

    use crate::tree::document::tests::device;
    use crate::{Document, StylesheetOrigin};

    fn scrolling_page() -> (Document<()>, crate::NodeId, crate::NodeId) {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .scroller { display: flex; overflow: scroll; width: 200px; height: 200px; }
             .content { flex-shrink: 0; width: 200px; height: 1000px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let scroller = document.create_element("view", ());
        document.add_class(scroller, "scroller");
        document.append_child(root, scroller);
        let content = document.create_element("view", ());
        document.add_class(content, "content");
        document.append_child(scroller, content);
        (document, root, scroller)
    }

    #[test]
    fn a_commit_publishes_the_scroll_table_and_slotted_hits() {
        let (mut document, root, scroller) = scrolling_page();
        let frame = document.commit();

        let slots = frame.scroll_slots();
        assert_eq!(slots.len(), 1, "one scroll container, one slot");
        assert_eq!(slots[0].node, scroller);
        assert_eq!(slots[0].parent, None);
        assert!(slots[0].user_scrollable.y);
        assert!((slots[0].max_offset.y - 800.0).abs() < 0.5);

        let inside = frame
            .hit(Point2D::new(50.0, 50.0), &|_| None)
            .expect("content hit");
        assert_eq!(inside.scroll, Some(0), "content carries its scroller");
        let outside = frame
            .hit(Point2D::new(500.0, 500.0), &|_| None)
            .expect("page hit");
        assert_eq!(outside.node, root);
        assert_eq!(outside.scroll, None, "the page is no scroll container");
    }

    #[test]
    fn a_composable_scroll_recommits_nothing_and_the_next_commit_publishes_it() {
        let (mut document, root, scroller) = scrolling_page();
        let before = document.commit();
        document.scroll_to(scroller, crate::Vector2D::new(0.0, 120.0));
        assert!(
            !document.needs_render(),
            "a scroll the retained frame can compose invalidates nothing"
        );
        assert_eq!(
            document.commit().epoch(),
            before.epoch(),
            "no new frame is built for it"
        );

        // Any real commit picks the live offset up into the slot table.
        document.set_inline_style(root, "background-color: teal");
        let frame = document.commit();
        assert!((frame.scroll_slots()[0].offset.y - 120.0).abs() < f32::EPSILON);
    }

    #[test]
    fn an_absolute_escapee_links_past_its_dom_side_scroller() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .outer { display: flex; overflow: scroll; width: 400px; height: 400px; }
             .inner { display: flex; overflow: scroll; width: 200px; height: 200px; }
             .content { flex-shrink: 0; width: 200px; height: 1000px; }
             .escapee { position: absolute; left: 10px; top: 10px;
                        width: 20px; height: 20px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let outer = document.create_element("view", ());
        document.add_class(outer, "outer");
        document.append_child(root, outer);
        let inner = document.create_element("view", ());
        document.add_class(inner, "inner");
        document.append_child(outer, inner);
        let content = document.create_element("view", ());
        document.add_class(content, "content");
        document.append_child(inner, content);
        let escapee = document.create_element("view", ());
        document.add_class(escapee, "escapee");
        document.append_child(inner, escapee);

        let frame = document.commit();
        let slots = frame.scroll_slots();
        let inner_slot = frame.slot_of(inner).expect("the inner scroller has a slot");
        assert_eq!(
            slots[inner_slot as usize].parent,
            frame.slot_of(outer),
            "nested scrollers chain outward"
        );

        // The escapee's containing block is the viewport (no positioned
        // ancestor), so its item must carry no slot at all — it neither
        // scrolls with the inner scroller nor chains into it.
        let hit = frame
            .hit(Point2D::new(15.0, 15.0), &|_| None)
            .expect("escapee hit");
        assert_eq!(hit.node, escapee);
        assert_eq!(hit.scroll, None, "the escapee left both scrollers");
    }
}
