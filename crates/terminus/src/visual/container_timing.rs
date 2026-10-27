//! W3C Container Timing (WICG draft, `https://wicg.github.io/container-timing/`):
//! when a marked subtree's content was first painted, and each presented
//! frame that grew the region it has painted.
//!
//! A **root** is an element the embedder marks
//! ([`Document::set_container_timing`], the draft's `containertiming`
//! attribute); an element marked ignored
//! ([`Document::set_container_timing_ignored`], `containertimingignore`) cuts
//! itself and its subtree out of every root above it. The paint walk
//! attributes each contentful paint — a text run with a glyph that is not
//! white space, or an image or vector draw, outside any `::backdrop` and any
//! `opacity: 0` group no animation fades, as Paint Timing counts content —
//! to its roots: its box (the item's box under the matrix it is painted
//! with), cut by its clip chain and the viewport at the committed scroll
//! offsets, in CSS px of the viewport. The committed frame carries those rects
//! ([`crate::visual::frame::CommittedFrame`]), and the presenting painter,
//! after presenting a frame, adds them to each root's painted region and
//! reports every root whose region grew as one [`ContainerPaint`]
//! (`Painter::take_container_paints`).
//!
//! # Which roots a paint counts toward
//!
//! The draft's: the nearest root on the painted node's ancestor chain, and
//! from it the next root out, and so on, until a root that is itself marked
//! ignored, or an ignored element, ends the chain. The chain is the
//! shadow-including one, so a shadow tree paints inside its host's roots
//! with `last_painted` retargeted to the host in the root's tree, and a
//! slotted node counts toward its light-tree ancestors
//! (`docs/tracking/deviations.md`, the one deviation from the draft).
//!
//! # Cost
//!
//! The document counts its roots. The walk copies "any root?" into its
//! scratch once, and with none tests that one `bool` per painted item and
//! nothing else; tree insertions test the same count. Under a root, a
//! contentful item whose node carries `SelfOrAncestorHasContainerTiming`
//! (Blink's name for the node bit) walks its ancestors, and the frame's
//! build maps its rects once.
//!
//! # When recording stops
//!
//! At the first scroll or input the window dispatches; the embedder decides
//! which inputs those are and calls [`Document::stop_container_timing`],
//! which unmarks every root. A root marked after that is recorded again
//! from an empty region (the draft leaves re-enabling undefined).

use euclid::default::Rect as PublicRect;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use vello::kurbo::Rect;

use crate::clock::ClockInstant;
use crate::tree::document::{Document, NodeId};
use crate::visual::frame::CommittedFrame;

/// One root's painted region growing in one presented frame: the draft's
/// `PerformanceContainerTiming`, in CSS px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContainerPaint {
    /// The root (`rootElement`).
    pub root: NodeId,
    /// The bounding box of everything painted under `root` so far, in CSS
    /// px of the viewport (`intersectionRect`).
    pub painted: PublicRect<f32>,
    /// The area of the painted region, not of its bounding box, in square
    /// CSS px (`size`).
    pub size: f32,
    /// When the first frame that painted anything under `root` was
    /// presented (`firstRenderTime`).
    pub first_render: ClockInstant,
    /// When this frame was presented (`startTime`, the draft's `renderTime`).
    pub latest: ClockInstant,
    /// Of the nodes this frame painted under `root`, the one whose paint
    /// added the most new area (`lastPaintedElement`), retargeted into
    /// `root`'s tree.
    pub last_painted: NodeId,
}

/// The document's Container Timing marks.
#[derive(Debug, Default)]
pub(crate) struct ContainerRoots {
    /// Every element marked a root, connected or not, with the marking it
    /// carries now. Its length is the count the walk and the tree's
    /// insertions test.
    roots: FxHashMap<NodeId, u32>,
    /// The last marking issued. Each marking of an element is a new one, so
    /// the painter starts a root marked again with an empty region, as the
    /// draft's new record does.
    marking: u32,
}

impl ContainerRoots {
    /// Whether any element is marked a root.
    #[inline]
    pub(crate) fn any(&self) -> bool {
        !self.roots.is_empty()
    }

    /// The marking `root` carries, if it is a root.
    pub(crate) fn marking(&self, root: NodeId) -> Option<u32> {
        self.roots.get(&root).copied()
    }
}

/// One contentful paint the walk attributed to one root: the item's box in
/// viewport CSS px of the item's own compose space, before its clips.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ContainerHit {
    pub(crate) root: NodeId,
    pub(crate) marking: u32,
    pub(crate) painted: NodeId,
    pub(crate) space: Option<u32>,
    pub(crate) clip: Option<usize>,
    pub(crate) rect: Rect,
}

/// One paint a committed frame carries for one root: the painted box cut
/// by its clips and the viewport at the committed state, in CSS px.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ContainerRect {
    pub(crate) root: NodeId,
    pub(crate) marking: u32,
    pub(crate) painted: NodeId,
    pub(crate) rect: Rect,
}

impl<T> Document<T> {
    /// Marks `root` a Container Timing root, or unmarks it: the draft's
    /// `containertiming` attribute being set or removed.
    ///
    /// Marking sets `SelfOrAncestorHasContainerTiming` on the element's
    /// shadow-including subtree, and from then on the frames built count
    /// the paints under it (the frame already retained is not rebuilt: a
    /// mark set after content painted captures the paints that follow, as
    /// the draft says). Unmarking ends the root's record: marked again, it
    /// starts from an empty region, as the draft's new record does. A node
    /// that is not a live element is ignored.
    pub fn set_container_timing(&mut self, root: NodeId, enabled: bool) {
        let Some(node) = self.arenas_mut().get_mut(root) else {
            return;
        };
        if !node.is_element() {
            return;
        }
        let was = node.set_container_timing_root(enabled);
        if was == enabled {
            return;
        }
        if enabled {
            self.container_roots.marking = self.container_roots.marking.wrapping_add(1);
            let marking = self.container_roots.marking;
            self.container_roots.roots.insert(root, marking);
            self.note_container_timing_subtree(root);
        } else {
            self.container_roots.roots.remove(&root);
        }
    }

    /// Marks `node` ignored, or not: the draft's `containertimingignore`.
    /// An ignored element and its subtree count toward no root above it; a
    /// root that is also ignored still counts its own subtree's paints, and
    /// passes none on to the roots above it.
    pub fn set_container_timing_ignored(&mut self, node: NodeId, ignored: bool) {
        if let Some(node) = self.arenas_mut().get_mut(node)
            && node.is_element()
        {
            node.set_container_timing_ignored(ignored);
        }
    }

    /// Ends Container Timing for every root marked now: the draft's stop at
    /// the first scroll or interaction. Unmarks each, so the frames built
    /// after it count nothing. A root marked afterwards is recorded again.
    pub fn stop_container_timing(&mut self) {
        for (root, _) in std::mem::take(&mut self.container_roots.roots) {
            if let Some(node) = self.arenas_mut().get_mut(root) {
                node.set_container_timing_root(false);
            }
        }
    }

    /// `child` was inserted under `parent`: while any root is marked and
    /// the parent carries `SelfOrAncestorHasContainerTiming`, so does the
    /// inserted subtree. One `bool` test while no root is marked.
    #[inline]
    pub(crate) fn note_container_timing_inserted(&mut self, parent: NodeId, child: NodeId) {
        if !self.container_roots.any() {
            return;
        }
        if self
            .get(parent)
            .is_some_and(crate::tree::node::Node::self_or_ancestor_has_container_timing)
        {
            self.note_container_timing_subtree(child);
        }
    }

    /// Sets `SelfOrAncestorHasContainerTiming` on `from`'s whole
    /// shadow-including subtree. The whole subtree, even below a node that
    /// already carries the bit: a node inserted while no root was marked
    /// has none, under a parent whose bit an earlier root left.
    fn note_container_timing_subtree(&mut self, from: NodeId) {
        let mut stack: SmallVec<[NodeId; 16]> = SmallVec::new();
        stack.push(from);
        while let Some(current) = stack.pop() {
            let Some(node) = self.arenas_mut().get_mut(current) else {
                continue;
            };
            node.note_container_timing_ancestor();
            let node = self.arenas().live(current);
            stack.extend(node.child_ids().iter().copied());
            stack.extend(node.shadow_root_id());
        }
    }

    /// A marked root was freed: it is no longer counted.
    pub(crate) fn forget_container_timing_root(&mut self, id: NodeId) {
        self.container_roots.roots.remove(&id);
    }

    /// Calls `found(root, marking, painted)` for every root a contentful paint of
    /// `node` counts toward, nearest first, `painted` being `node` (an
    /// anonymous box's container for an anonymous box) retargeted into the
    /// root's tree. The module doc says which roots those are.
    pub(crate) fn for_each_container_root(
        &self,
        node: NodeId,
        mut found: impl FnMut(NodeId, u32, NodeId),
    ) {
        let tree = self.arenas();
        let Some(mut current) = tree.get(node) else {
            return;
        };
        // An anonymous box's text is its container's: the box is in no
        // child list and no embedder names it.
        let mut painted = node;
        if current.is_anonymous_box() {
            painted = self.hit_target(node);
            let Some(container) = tree.get(painted) else {
                return;
            };
            current = container;
        }
        // The host the walk last left a shadow tree for: `painted`
        // retargeted into the tree the walk is in now.
        let mut retargeted = painted;
        loop {
            if !current.self_or_ancestor_has_container_timing() {
                return;
            }
            let ignored = current.is_container_timing_ignored();
            if current.is_container_timing_root()
                && let Some(marking) = self.container_roots.marking(current.id())
            {
                found(current.id(), marking, retargeted);
            }
            if ignored {
                return;
            }
            let Some(parent) = current.parent else {
                return;
            };
            let parent = tree.at(parent);
            current = match parent.shadow_host_slot() {
                Some(host) => {
                    let host = tree.at(host);
                    retargeted = host.id();
                    host
                }
                None => parent,
            };
        }
    }
}

/// A set of CSS-px rects, kept disjoint: a root's painted region.
#[derive(Debug, Default)]
struct Region {
    rects: Vec<Rect>,
}

impl Region {
    /// Adds `rect`, answering the area it added.
    fn add(&mut self, rect: Rect) -> f64 {
        let mut pieces: SmallVec<[Rect; 4]> = SmallVec::new();
        pieces.push(rect);
        for existing in &self.rects {
            let mut next: SmallVec<[Rect; 4]> = SmallVec::new();
            for piece in pieces {
                subtract(piece, *existing, &mut next);
            }
            pieces = next;
            if pieces.is_empty() {
                return 0.0;
            }
        }
        let added = pieces.iter().map(Rect::area).sum();
        self.rects.extend(pieces);
        added
    }

    fn area(&self) -> f64 {
        self.rects.iter().map(Rect::area).sum()
    }

    fn bounds(&self) -> Rect {
        self.rects
            .iter()
            .copied()
            .reduce(|united, rect| united.union(rect))
            .unwrap_or(Rect::ZERO)
    }
}

/// `piece` minus `hole`, as up to four disjoint rects appended to `out`.
fn subtract(piece: Rect, hole: Rect, out: &mut SmallVec<[Rect; 4]>) {
    let overlap = piece.intersect(hole);
    if overlap.width() <= 0.0 || overlap.height() <= 0.0 {
        out.push(piece);
        return;
    }
    let mut keep = |rect: Rect| {
        if rect.width() > 0.0 && rect.height() > 0.0 {
            out.push(rect);
        }
    };
    keep(Rect::new(piece.x0, piece.y0, piece.x1, overlap.y0));
    keep(Rect::new(piece.x0, overlap.y1, piece.x1, piece.y1));
    keep(Rect::new(piece.x0, overlap.y0, overlap.x0, overlap.y1));
    keep(Rect::new(overlap.x1, overlap.y0, piece.x1, overlap.y1));
}

/// What a root has painted so far, as the presenting painter accumulated it.
#[derive(Debug)]
struct RootRecord {
    region: Region,
    first_render: ClockInstant,
}

/// One root that grew in one frame, keyed by root and marking: the largest
/// area one node added, and that node.
struct Grown {
    key: (NodeId, u32),
    added: f64,
    painted: NodeId,
}

/// How many reports a painter keeps for an embedder that takes none; the
/// oldest go first.
const MAX_PENDING: usize = 1024;

/// The presenting painter's Container Timing state for the view it shows:
/// each root's painted region, and the reports not taken yet.
#[derive(Debug, Default)]
pub(crate) struct ContainerTimer {
    /// The newest commit whose paints were added: a frame presented again
    /// (a scroll, an animation) adds nothing.
    last_commit: u64,
    /// By root and marking.
    roots: FxHashMap<(NodeId, u32), RootRecord>,
    pending: Vec<ContainerPaint>,
}

impl ContainerTimer {
    /// Adds what `frame` painted under its roots, presented at `presented`,
    /// and reports each root whose region grew. Nothing for a frame whose
    /// paints were already added.
    pub(crate) fn note(&mut self, frame: &CommittedFrame, presented: ClockInstant) {
        let commit_id = frame.commit_id();
        if commit_id <= self.last_commit {
            return;
        }
        self.last_commit = commit_id;
        let mut grown: SmallVec<[Grown; 4]> = SmallVec::new();
        for paint in frame.container_rects() {
            let key = (paint.root, paint.marking);
            let record = self.roots.entry(key).or_insert_with(|| RootRecord {
                region: Region::default(),
                first_render: presented,
            });
            let added = record.region.add(paint.rect);
            if added <= 0.0 {
                continue;
            }
            match grown.iter_mut().find(|grew| grew.key == key) {
                Some(grew) if added > grew.added => {
                    grew.added = added;
                    grew.painted = paint.painted;
                }
                Some(_) => {}
                None => grown.push(Grown {
                    key,
                    added,
                    painted: paint.painted,
                }),
            }
        }
        for Grown {
            key,
            painted: last_painted,
            ..
        } in grown
        {
            let record = &self.roots[&key];
            let root = key.0;
            let bounds = record.region.bounds();
            #[expect(
                clippy::cast_possible_truncation,
                reason = "CSS px of a viewport are far inside f32's range"
            )]
            let painted = PublicRect::new(
                euclid::default::Point2D::new(bounds.x0 as f32, bounds.y0 as f32),
                euclid::default::Size2D::new(bounds.width() as f32, bounds.height() as f32),
            );
            if self.pending.len() >= MAX_PENDING {
                self.pending.remove(0);
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "an area of CSS px is far inside f32's range"
            )]
            let size = record.region.area() as f32;
            self.pending.push(ContainerPaint {
                root,
                painted,
                size,
                first_render: record.first_render,
                latest: presented,
                last_painted,
            });
        }
    }

    /// The reports made since the last call, oldest first.
    pub(crate) fn take(&mut self) -> Vec<ContainerPaint> {
        std::mem::take(&mut self.pending)
    }

    /// Everything, for a painter that shows another view next.
    pub(crate) fn forget(&mut self) {
        *self = Self::default();
    }
}
