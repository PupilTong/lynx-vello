//! The composite plan: which contiguous runs of the compose program bake
//! into retained per-scroller textures, and the per-frame steps that draw
//! the frame from those textures plus the ops left raw.
//!
//! A plane is one maximal contiguous run of program ops that all ride one
//! scroll head `s` with no animation chain: baked once per commit into a
//! texture covering the slot's scrollport expanded by its encode window, it
//! moves under scrolling as a single textured draw. Everything else — root
//! content (already viewport-culled, so screen-bounded), animation-chained
//! content, groups the bake rules refuse — replays raw each frame, in
//! program order, so z-order and clipping are exact by construction rather
//! than by any overlap analysis.
//!
//! A push group bakes as part of a run only when its whole subtree rides
//! the run's head, nothing in it samples an animation, and every
//! non-`SrcOver` blend inside has an isolating layer between it and the
//! plane — a blend against the plane's transparent backdrop would read
//! pixels the full composition would have had. The group's own layer must
//! be a clip or plain `SrcOver` for the same reason.
//!
//! Clip-only pushes riding an *outer* chain are the one outer-chain shape a
//! run absorbs. The walker re-pushes an item's whole clip chain inside
//! every group scope, so scroller content re-pushes the scrollport clip —
//! which rides the chain outside the scroller and must not translate with
//! the plane. An item composes on chain `s` exactly when its clip context
//! was snapshotted inside `s`, so every outer-chain clip inside a run is a
//! re-push of the run slot's own clip chain: the bake skips them, and the
//! composite pushes that chain — each clip at its own chain's offset —
//! around the plane's draw instead.

use std::ops::Range;

use crate::paint::compose::{ComposeChain, ComposeOp};
use crate::paint::walker::clip_bounds;
use crate::vello::kurbo::Rect;
use crate::vello::peniko::{BlendMode, Compose, Mix};
use crate::visual::{ClipNode, ScrollSlot};

/// vello's shared image atlas is hard-capped at 8192×8192, and an image
/// that cannot fit is silently not drawn. Planes claim at most half the
/// atlas, leaving the rest to content images; a frame needing more, or a
/// single plane over the dimension cap, composes without layers.
const MAX_PLANE_DIMENSION: f64 = 8192.0;
const PLANE_AREA_BUDGET: f64 = 8192.0 * 8192.0 / 2.0;

/// Device pixels of margin around a plane's rect, covering antialiased ink
/// straddling its edge.
const PLANE_SLACK_DEVICE_PX: f64 = 2.0;

/// One step of the per-frame composite.
#[derive(Debug)]
pub(crate) enum CompositeStep {
    /// Replay this program range raw.
    Ops(Range<u32>),
    /// Draw retained plane `index` at its slot's current offset.
    Plane(u32),
}

/// One retained layer: the program run it bakes and the bake-space device
/// rect its texture covers.
#[derive(Debug)]
pub(crate) struct PlaneSpec {
    pub(crate) ops: Range<u32>,
    pub(crate) slot: u32,
    /// Integer-valued device-px rect in bake space (the unscrolled frame).
    pub(crate) rect: Rect,
}

/// The frame's layer decomposition. Steps partition the program; planes are
/// what a GPU target retains between commits.
#[derive(Debug)]
pub struct CompositePlan {
    pub(crate) steps: Vec<CompositeStep>,
    pub(crate) planes: Vec<PlaneSpec>,
}

impl CompositePlan {
    /// How many retained textures this frame wants.
    #[must_use]
    pub fn plane_count(&self) -> usize {
        self.planes.len()
    }

    /// Plane `index`'s texture size in device pixels.
    #[must_use]
    pub fn plane_size(&self, index: usize) -> (u32, u32) {
        let rect = self.planes[index].rect;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "plane rects are built non-negative, integer-valued, and dimension-capped"
        )]
        (rect.width() as u32, rect.height() as u32)
    }
}

fn is_plain(blend: BlendMode) -> bool {
    blend.mix == Mix::Normal && blend.compose == Compose::SrcOver
}

/// What one op contributes to enclosing groups' bake eligibility.
#[derive(Clone, Copy, PartialEq)]
enum Rides {
    /// Rides scroll head `s`, no animation.
    Head(u32),
    /// Anything else: root chain, animation chain, sampled alpha.
    Foreign,
}

fn rides(chain: ComposeChain) -> Rides {
    match (chain.scroll, chain.animation) {
        (Some(slot), None) => Rides::Head(slot),
        _ => Rides::Foreign,
    }
}

#[derive(Clone, Copy)]
enum Lattice {
    Empty,
    Uniform(u32),
    Mixed,
}

impl Lattice {
    fn merge(self, rides: Rides) -> Self {
        match (self, rides) {
            (Self::Empty, Rides::Head(slot)) => Self::Uniform(slot),
            (Self::Uniform(slot), Rides::Head(other)) if slot == other => self,
            _ => Self::Mixed,
        }
    }
}

struct OpenGroup {
    /// Index of the push op.
    index: usize,
    lattice: Lattice,
    /// No blend inside lacks an isolating layer within this group.
    isolation_ok: bool,
}

/// Per push op: its matching pop and the scroll head its whole group may
/// bake under, if any.
struct GroupInfo {
    pop: u32,
    head: Option<u32>,
}

/// One pass over the program computing, per push, whether its whole group
/// bakes and under which head.
fn summarize_groups(program: &[ComposeOp]) -> Vec<Option<GroupInfo>> {
    let mut info: Vec<Option<GroupInfo>> = Vec::with_capacity(program.len());
    info.resize_with(program.len(), || None);
    let mut open: Vec<OpenGroup> = Vec::new();
    // Indices of currently open isolating (non-clip) pushes.
    let mut isolating: Vec<usize> = Vec::new();
    for (index, op) in program.iter().enumerate() {
        match op {
            ComposeOp::Fragment { chain, .. } => {
                if let Some(top) = open.last_mut() {
                    top.lattice = top.lattice.merge(rides(*chain));
                }
            }
            ComposeOp::Push {
                clip_only,
                blend,
                chain,
                alpha_animation,
                ..
            } => {
                if !clip_only && !is_plain(*blend) {
                    // Every open group this blend is bare in — no isolating
                    // layer between them — must not bake: against a plane's
                    // transparent backdrop the blend reads different pixels.
                    let nearest = isolating.last().copied();
                    for group in &mut open {
                        if nearest.is_none_or(|iso| group.index > iso) {
                            group.isolation_ok = false;
                        }
                    }
                }
                let contributes = if alpha_animation.is_some() {
                    Some(Rides::Foreign)
                } else if *clip_only && rides(*chain) == Rides::Foreign {
                    // An outer-chain clip re-push: absorbed by the bake (its
                    // shape is applied around the plane draw instead), so it
                    // neither poisons nor heads a group.
                    None
                } else {
                    Some(rides(*chain))
                };
                let mut lattice = Lattice::Empty;
                if let Some(contributes) = contributes {
                    if let Some(top) = open.last_mut() {
                        top.lattice = top.lattice.merge(contributes);
                    }
                    lattice = lattice.merge(contributes);
                }
                open.push(OpenGroup {
                    index,
                    lattice,
                    isolation_ok: true,
                });
                if !clip_only {
                    isolating.push(index);
                }
            }
            ComposeOp::Pop => {
                let group = open.pop().expect("the program's pushes balance its pops");
                if isolating.last() == Some(&group.index) {
                    isolating.pop();
                }
                let ComposeOp::Push {
                    clip_only, blend, ..
                } = &program[group.index]
                else {
                    unreachable!("an open group starts at a push");
                };
                let isolates_or_plain = *clip_only || is_plain(*blend);
                let head = match group.lattice {
                    Lattice::Uniform(slot) if group.isolation_ok && isolates_or_plain => Some(slot),
                    _ => None,
                };
                info[group.index] = Some(GroupInfo {
                    pop: u32::try_from(index).expect("op indices fit u32"),
                    head,
                });
                if let Some(parent) = open.last_mut() {
                    parent.lattice = match group.lattice {
                        Lattice::Empty => parent.lattice,
                        Lattice::Uniform(slot) => parent.lattice.merge(Rides::Head(slot)),
                        Lattice::Mixed => Lattice::Mixed,
                    };
                }
            }
        }
    }
    info
}

/// The bake-space device rect slot `slot`'s planes cover: the scrollport
/// clip's bounds expanded by the slot's encode window — every point visible
/// through the clip at any in-window offset — with antialiasing slack,
/// rounded out to whole device pixels. `None` when the clip is unresolvable
/// or the rect exceeds the atlas dimension cap.
fn plane_rect(slot: &ScrollSlot, clips: &[ClipNode], ratio: f32) -> Option<Rect> {
    let clip = &clips[slot.clip? as usize];
    let bounds = clip_bounds(clip)?;
    let (low, high) = slot.encode_window();
    let ratio = f64::from(ratio);
    if !(ratio.is_finite() && ratio > 0.0) {
        return None;
    }
    let rect = Rect::new(
        ((bounds.x0 + f64::from(low.x)) * ratio - PLANE_SLACK_DEVICE_PX).floor(),
        ((bounds.y0 + f64::from(low.y)) * ratio - PLANE_SLACK_DEVICE_PX).floor(),
        ((bounds.x1 + f64::from(high.x)) * ratio + PLANE_SLACK_DEVICE_PX).ceil(),
        ((bounds.y1 + f64::from(high.y)) * ratio + PLANE_SLACK_DEVICE_PX).ceil(),
    );
    let fits = rect.x0.is_finite()
        && rect.y0.is_finite()
        && rect.width() >= 1.0
        && rect.height() >= 1.0
        && rect.width() <= MAX_PLANE_DIMENSION
        && rect.height() <= MAX_PLANE_DIMENSION;
    fits.then_some(rect)
}

/// Builds the frame's plan, or `None` when nothing layers (no scroller
/// content, or the planes together would overrun the atlas budget).
pub(crate) fn plan(
    program: &[ComposeOp],
    slots: &[ScrollSlot],
    clips: &[ClipNode],
    ratio: f32,
) -> Option<CompositePlan> {
    if slots.is_empty() {
        return None;
    }
    let info = summarize_groups(program);
    let rects: Vec<Option<Rect>> = slots
        .iter()
        .map(|slot| plane_rect(slot, clips, ratio))
        .collect();

    let mut steps = Vec::new();
    let mut planes = Vec::new();
    let as_u32 = |index: usize| u32::try_from(index).expect("op indices fit u32");
    // The cursor state: where the current raw range began, and the open run.
    let mut raw_start = 0_usize;
    let mut run: Option<(u32, usize)> = None;
    let mut index = 0_usize;
    while index < program.len() {
        // The head this op (or whole group) bakes under, and the index just
        // past it.
        let (head, next) = match &program[index] {
            ComposeOp::Fragment { chain, .. } => match rides(*chain) {
                Rides::Head(slot) => (Some(slot), index + 1),
                Rides::Foreign => (None, index + 1),
            },
            ComposeOp::Push { .. } => {
                let group = info[index]
                    .as_ref()
                    .expect("every push was summarized with its pop");
                match group.head {
                    Some(slot) => (Some(slot), group.pop as usize + 1),
                    None => (None, index + 1),
                }
            }
            ComposeOp::Pop => (None, index + 1),
        };
        let head = head.filter(|&slot| rects[slot as usize].is_some());
        if let Some((open, start)) = run
            && head != Some(open)
        {
            planes.push(PlaneSpec {
                ops: as_u32(start)..as_u32(index),
                slot: open,
                rect: rects[open as usize].expect("the run opened on a sized slot"),
            });
            steps.push(CompositeStep::Plane(as_u32(planes.len() - 1)));
            run = None;
            raw_start = index;
        }
        if let Some(slot) = head
            && run.is_none()
        {
            if raw_start < index {
                steps.push(CompositeStep::Ops(as_u32(raw_start)..as_u32(index)));
            }
            run = Some((slot, index));
        }
        index = next;
    }
    if let Some((open, start)) = run {
        planes.push(PlaneSpec {
            ops: as_u32(start)..as_u32(program.len()),
            slot: open,
            rect: rects[open as usize].expect("the run opened on a sized slot"),
        });
        steps.push(CompositeStep::Plane(as_u32(planes.len() - 1)));
    } else if raw_start < program.len() {
        steps.push(CompositeStep::Ops(as_u32(raw_start)..as_u32(program.len())));
    }

    if planes.is_empty() {
        return None;
    }
    let area: f64 = planes
        .iter()
        .map(|plane| plane.rect.width() * plane.rect.height())
        .sum();
    (area <= PLANE_AREA_BUDGET).then_some(CompositePlan { steps, planes })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{CompositePlan, CompositeStep};
    use crate::paint::compose::ComposeOp;
    use crate::test_common::Doc;
    use crate::vello::peniko::{Compose, Mix};
    use crate::visual::CommittedFrame;

    const SCROLL_PAGE: &str = "page { display: flex; width: 800px; height: 600px; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     width: 200px; height: 200px; }
         .row { display: flex; flex-shrink: 0; width: 200px; height: 100px;
                background-color: teal; }";

    fn scroller_doc(extra_css: &str, rows: usize, row_extra: &str) -> Doc {
        let mut doc = Doc::with_css(&format!("{SCROLL_PAGE} {extra_css}"));
        let root = doc.root;
        let scroller = doc.el(root, "view.scroller");
        for _ in 0..rows {
            let row = doc.el(scroller, "view.row");
            if !row_extra.is_empty() {
                doc.set_inline(row, row_extra);
            }
        }
        doc
    }

    /// Every op index, in order, with the step (or plane) it landed in —
    /// the partition the composite replays.
    fn covered_ops(plan: &CompositePlan) -> Vec<(u32, bool)> {
        let mut ops = Vec::new();
        for step in &plan.steps {
            match step {
                CompositeStep::Ops(range) => ops.extend(range.clone().map(|op| (op, false))),
                CompositeStep::Plane(plane) => {
                    ops.extend(
                        plan.planes[*plane as usize]
                            .ops
                            .clone()
                            .map(|op| (op, true)),
                    );
                }
            }
        }
        ops
    }

    fn assert_partitions(frame: &CommittedFrame, plan: &CompositePlan) {
        let ops = covered_ops(plan);
        let expected: Vec<u32> =
            (0..u32::try_from(frame.presentation.program.len()).unwrap()).collect();
        let sequence: Vec<u32> = ops.iter().map(|(op, _)| *op).collect();
        assert_eq!(
            sequence, expected,
            "the steps must partition the program in order"
        );
    }

    #[test]
    fn a_scroller_frame_plans_one_plane_over_its_content() {
        let mut doc = scroller_doc("", 4, "");
        let frame = doc.dom.commit();
        let plan = frame.composite_plan().expect("scroller content layers");
        assert_eq!(plan.plane_count(), 1, "one scroller, one contiguous run");
        assert_partitions(&frame, plan);

        // The plane covers the scrollport expanded by the encode window: the
        // scroller sits at the page origin, 200 wide, content 400 tall, so
        // the window reaches offset 200 and the rect spans y 0..=400.
        let rect = plan.planes[0].rect;
        assert!(rect.x0 <= 0.0 && rect.y0 <= 0.0);
        assert!(rect.x1 >= 200.0 && rect.y1 >= 400.0);
        assert!(rect.x1 <= 210.0 && rect.y1 <= 410.0, "slack stays small");
        assert!(
            frame.scene().is_none(),
            "a layered frame materializes no second whole-frame encoding"
        );
    }

    #[test]
    fn an_isolating_group_inside_the_scroller_bakes_into_the_plane() {
        let mut doc = scroller_doc("", 3, "opacity: 0.5; border-radius: 8px; overflow: hidden");
        let frame = doc.dom.commit();
        let plan = frame.composite_plan().expect("scroller content layers");
        assert_eq!(
            plan.plane_count(),
            1,
            "opacity groups and item clips ride the run instead of splitting it"
        );
        assert_partitions(&frame, plan);
    }

    #[test]
    fn a_bare_blend_inside_the_scroller_stays_out_of_the_plane() {
        let mut doc = scroller_doc("", 3, "mix-blend-mode: multiply");
        let frame = doc.dom.commit();
        // The blend groups must not bake: against the plane's transparent
        // backdrop, multiply would read pixels the full composition never
        // shows. Whether anything is left to layer is the plan's call; what
        // is asserted is that no plane range contains a non-SrcOver push.
        if let Some(plan) = frame.composite_plan() {
            assert_partitions(&frame, plan);
            for (op, in_plane) in covered_ops(plan) {
                if let ComposeOp::Push {
                    clip_only, blend, ..
                } = &frame.presentation.program[op as usize]
                    && !clip_only
                    && (blend.mix != Mix::Normal || blend.compose != Compose::SrcOver)
                {
                    assert!(!in_plane, "op {op}: a bare blend must replay raw");
                }
            }
        }
    }

    #[test]
    fn nested_scrollers_plan_separate_planes() {
        let mut doc = Doc::with_css(
            "page { display: flex; width: 800px; height: 600px; }
             .outer { display: flex; flex-direction: column; overflow: scroll;
                      width: 400px; height: 400px; }
             .inner { display: flex; flex-direction: column; flex-shrink: 0;
                      overflow: scroll; width: 400px; height: 200px; }
             .row { display: flex; flex-shrink: 0; width: 400px; height: 150px;
                    background-color: teal; }",
        );
        let root = doc.root;
        let outer = doc.el(root, "view.outer");
        let inner = doc.el(outer, "view.inner");
        for _ in 0..3 {
            doc.el(inner, "view.row");
        }
        for _ in 0..3 {
            doc.el(outer, "view.row");
        }
        let frame = doc.dom.commit();
        let plan = frame.composite_plan().expect("both scrollers layer");
        assert_partitions(&frame, plan);
        let mut slots: Vec<u32> = plan.planes.iter().map(|plane| plane.slot).collect();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), 2, "each scroller gets its own planes");
    }

    #[test]
    fn a_plane_over_the_atlas_dimension_cap_refuses_to_layer() {
        let mut doc = Doc::with_device(crate::test_common::device(9000.0, 9000.0));
        doc.add_css(
            "page { display: flex; width: 9000px; height: 9000px; }
             .scroller { display: flex; flex-direction: column; overflow: scroll;
                         width: 8500px; height: 8500px; }
             .row { display: flex; flex-shrink: 0; width: 8500px; height: 5000px;
                    background-color: teal; }",
        );
        let root = doc.root;
        let scroller = doc.el(root, "view.scroller");
        for _ in 0..3 {
            doc.el(scroller, "view.row");
        }
        let frame = doc.dom.commit();
        assert!(
            frame.composite_plan().is_none(),
            "a plane past 8192 device px cannot live in the atlas"
        );
        assert!(
            frame.scene().is_some(),
            "the refused frame composes at commit instead"
        );
    }

    #[test]
    fn a_frame_without_scrollers_has_no_plan() {
        let mut doc = Doc::with_css("page { width: 100px; height: 100px; background-color: red; }");
        let frame = doc.dom.commit();
        assert!(frame.composite_plan().is_none());
        assert!(frame.scene().is_some());
    }
}
