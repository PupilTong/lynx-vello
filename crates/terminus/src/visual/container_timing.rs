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

use crate::clock::ClockInstant;
use crate::kurbo::Rect;
use crate::tree::document::{Document, NodeId};
use crate::visual::frame::CommittedFrame;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContainerPaint {
    pub root: NodeId,
    pub painted: PublicRect<f32>,
    pub size: f32,
    pub first_render: ClockInstant,
    pub latest: ClockInstant,
    pub last_painted: NodeId,
}

#[derive(Debug, Default)]
pub(crate) struct ContainerRoots {
    roots: FxHashMap<NodeId, u32>,
    marking: u32,
}

impl ContainerRoots {
    #[inline]
    pub(crate) fn any(&self) -> bool {
        !self.roots.is_empty()
    }

    pub(crate) fn marking(&self, root: NodeId) -> Option<u32> {
        self.roots.get(&root).copied()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ContainerHit {
    pub(crate) root: NodeId,
    pub(crate) marking: u32,
    pub(crate) painted: NodeId,
    pub(crate) space: Option<u32>,
    pub(crate) clip: Option<usize>,
    pub(crate) rect: Rect,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ContainerRect {
    pub(crate) root: NodeId,
    pub(crate) marking: u32,
    pub(crate) painted: NodeId,
    pub(crate) rect: Rect,
}

impl<T> Document<T> {
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

    pub fn set_container_timing_ignored(&mut self, node: NodeId, ignored: bool) {
        if let Some(node) = self.arenas_mut().get_mut(node)
            && node.is_element()
        {
            node.set_container_timing_ignored(ignored);
        }
    }

    pub fn stop_container_timing(&mut self) {
        for (root, _) in std::mem::take(&mut self.container_roots.roots) {
            if let Some(node) = self.arenas_mut().get_mut(root) {
                node.set_container_timing_root(false);
            }
        }
    }

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

    pub(crate) fn forget_container_timing_root(&mut self, id: NodeId) {
        self.container_roots.roots.remove(&id);
    }

    pub(crate) fn for_each_container_root(
        &self,
        node: NodeId,
        mut found: impl FnMut(NodeId, u32, NodeId),
    ) {
        let tree = self.arenas();
        let Some(mut current) = tree.get(node) else {
            return;
        };
        let mut painted = node;
        if current.is_anonymous_box() {
            painted = self.hit_target(node);
            let Some(container) = tree.get(painted) else {
                return;
            };
            current = container;
        }
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

#[derive(Debug, Default)]
struct Region {
    rects: Vec<Rect>,
}

impl Region {
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

#[derive(Debug)]
struct RootRecord {
    region: Region,
    first_render: ClockInstant,
}

struct Grown {
    key: (NodeId, u32),
    added: f64,
    painted: NodeId,
}

const MAX_PENDING: usize = 1024;

#[derive(Debug, Default)]
pub(crate) struct ContainerTimer {
    last_commit: u64,
    roots: FxHashMap<(NodeId, u32), RootRecord>,
    pending: Vec<ContainerPaint>,
}

impl ContainerTimer {
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

    pub(crate) fn take(&mut self) -> Vec<ContainerPaint> {
        std::mem::take(&mut self.pending)
    }

    pub(crate) fn forget(&mut self) {
        *self = Self::default();
    }
}
