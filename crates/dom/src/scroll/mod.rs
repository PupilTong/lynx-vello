//! CSSOM-View scrolling: the scroll box model over the laid-out tree.
//!
//! A **scroll container** is a box whose computed `overflow` is scrollable on
//! either axis (css-overflow-3 §3). Its *scrollport* is its padding box; its
//! *scrolling area* is the union of that scrollport with the scrollable
//! overflow the layout engine accumulated ([`hughie::tree::Layout::content_size`]).
//! The difference between the two is how far the box can scroll.
//!
//! The fork's keyword set is `visible | hidden | scroll | clip` — no `auto` —
//! and the three non-`visible` values are three different things:
//!
//! - `scroll` is a scroll container the user can drag and wheel.
//! - `hidden` is a scroll container that is **not user-scrollable**: it clips, and it moves only
//!   when something asks it to programmatically — conflating it with `scroll` would make every
//!   author-hidden box drag-scrollable. The Lynx UA cascade's own default is `clip`, not `hidden`
//!   (web-elements' common block), so a box clips without becoming a scroll container at all; the
//!   sheet that decides it belongs to the embedder (`bobcat-core`'s `ua_stylesheet`), not to this
//!   crate.
//! - `clip` is not a scroll container at all: it clips and stops, with no scrolling area and no
//!   offset, and its content does not reach into an ancestor's scrolling area either.
//!
//! Offsets are stored per node in the layout arena and clamped against live
//! geometry on every read, so a relayout that shrinks the scrolling area, or a
//! restyle that stops the box being a scroll container, corrects the
//! observable offset without an invalidation hook of its own. Like every other
//! post-layout query here, the clamp reads the **last committed** layout: call
//! [`Document::layout`] first if the tree has been mutated since.
//!
//! # Chaining
//!
//! A delta that one scroll container cannot consume moves on to the next one
//! out on its **containing-block** chain (css-overscroll-1 §2's scroll
//! chaining), and two properties shape that walk. Both are read into a
//! [`ChainLink`] per container, and [`drive_chain`] is the one algorithm
//! over a chain of links — the document runs it here over live geometry,
//! and the runtime's painter runs the same function over the committed
//! frame's scroll-slot table, so the two never disagree about order:
//!
//! - **`overscroll-behavior`** (css-overscroll-1 §3, per axis) is the standard one. `auto` lets a
//!   boundary hand the remainder outward; `contain` and `none` stop the chain at that container:
//!   nothing above it receives that axis's delta, whether the container itself could move or not —
//!   a `hidden` container with `contain` blocks the chain through it just as a `scroll` one does,
//!   because both are scroll containers and the property applies to scroll containers.
//! - **`scroll-capture`** is this engine's own, with no W3C or Lynx counterpart, and it changes the
//!   *order*, not the reach. `nearest` on a container hands a gesture that starts in it to the
//!   nearest scroll container above it first; this container moves only once that ancestor cannot
//!   (it is at its boundary in that direction, or does not scroll that axis at all). The chain then
//!   continues outward past the ancestor as usual. It nests: `nearest` on both of two nested
//!   containers visits the grandparent, then the parent, then the innermost. Reach is decided
//!   before order, so `nearest` beside `contain` on the same container keeps the gesture inside it
//!   — the ancestor it would have deferred to is exactly what `contain` fences off.
//!
//! Where each step lands is [`snap`]'s: css-scroll-snap-1 positions per
//! container, applied as a wheel tick lands and when a drag ends.
//!
//! Nothing in this module knows about input devices. [`crate::input`] drives
//! it from pointer and wheel events; an embedder, or a runtime layer's
//! `scrollTo`-style API, drives it directly.
//!
//! Deliberate limits:
//! - Only element boxes scroll. There is no document/viewport scrolling area: the root box is sized
//!   to the viewport, and page scrolling is a runtime-policy concern the embedder resolves by
//!   making its own root element a scroll container.
//! - The scrolling area does not extend past the last box by the scroll container's own end-side
//!   padding (css-overflow-3 §2.2). That end padding is missing from the layout engine's
//!   accumulated content size, not discarded here.
//! - `scroll-behavior` and rubber-band overscroll are absent: scrolling is instantaneous and clamps
//!   hard at the boundary, and a snap is a jump. `overscroll-behavior: none` therefore does exactly
//!   what `contain` does — there is no boundary effect for it to suppress on top.
//! - **`position: sticky` does not stick.** It parses in the fork's grammar, but the paint build
//!   treats it as normal flow, so a sticky box scrolls away with its container instead of pinning
//!   to the scrollport. Before scrolling existed that was indistinguishable from `relative`; now it
//!   is observable, which is why it is written down here rather than left implied. Its scroll
//!   parent is deliberately its box parent (sticky *is* in flow — that part is right); what is
//!   missing is the offset clamp against the scrollport that css-position-3 §6.3 defines.

use euclid::default::{Size2D, Vector2D};
use hughie::style::PositionProperty;
use smallvec::SmallVec;
use stylo::computed_values::scroll_capture;
use stylo::properties::ComputedValues;
use stylo::values::computed::OverscrollBehavior;

use crate::NodeId;
use crate::layout::{
    box_parent, establishes_absolute_containing_block, establishes_fixed_containing_block,
};
use crate::tree::document::Document;
use crate::tree::node::Node;

/// Per-axis scrolling capability flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollAxes {
    pub x: bool,
    pub y: bool,
}

#[cfg(test)]
mod behavior_tests;
pub mod snap;

pub use snap::{
    PROXIMITY_RATIO, ScrollKind, SnapAxis, SnapAxisPositions, SnapPoint, SnapPositions,
    SnapStrictness, resolve_step, settle_offset,
};

impl ScrollAxes {
    pub const NONE: Self = Self { x: false, y: false };
    pub const BOTH: Self = Self { x: true, y: true };
}

/// How a scroll container orders itself against the scroll container above
/// it when a gesture that starts in it chains: the engine's own
/// `scroll-capture` property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollCapture {
    /// Inner first, the CSS default: this container consumes what it can and
    /// hands the remainder outward.
    #[default]
    Auto,
    /// The nearest scroll container above this one goes first; this one
    /// moves only once that ancestor cannot.
    Nearest,
}

/// One scroll container's part in a chain walk: what it may consume, what
/// it lets past, and where it stands in the order. See the module's
/// *Chaining* section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainLink {
    /// The axes the user may scroll directly (`overflow: scroll`).
    pub user_scrollable: ScrollAxes,
    /// The axes on which a boundary hands the remainder outward —
    /// `overscroll-behavior: auto`. `contain` and `none` clear the flag.
    pub chains: ScrollAxes,
    /// Whether the container above goes first.
    pub capture: ScrollCapture,
}

/// The order a chain of links is visited in, as indices into `links`.
///
/// `links` is nearest-first: index 0 is the container the gesture starts in
/// and each next index is the scroll container above the previous one. A
/// [`ScrollCapture::Nearest`] link moves to directly after its parent, and
/// the placement runs from the outermost link inward so a nested `nearest`
/// resolves against a parent that has already found its own place.
#[must_use]
pub fn chain_order(links: &[ChainLink]) -> SmallVec<[usize; 4]> {
    let mut order = SmallVec::new();
    for index in (0..links.len()).rev() {
        let defers = links[index].capture == ScrollCapture::Nearest && index + 1 < links.len();
        if defers {
            let parent_at = order
                .iter()
                .position(|&placed| placed == index + 1)
                .expect("the parent link was placed before its child");
            order.insert(parent_at + 1, index);
        } else {
            order.insert(0, index);
        }
    }
    order
}

/// How far up the chain each axis's delta may travel: the number of leading
/// links it can reach, cut at the first link that does not chain on that
/// axis (that link itself is still reached).
fn chain_reach(links: &[ChainLink]) -> (usize, usize) {
    let reach = |chains: fn(&ChainLink) -> bool| {
        links
            .iter()
            .position(|link| !chains(link))
            .map_or(links.len(), |fence| fence + 1)
    };
    (reach(|link| link.chains.x), reach(|link| link.chains.y))
}

/// Drives `delta` through a chain of links, in [`chain_order`] and within
/// each axis's `overscroll-behavior` reach, calling `scroll` with a link's
/// index and the delta it is admitted; `scroll` applies what it can and
/// returns the delta it absorbed, which is what stops chaining on (a
/// snapped step can absorb more than it moved — see [`resolve_step`]).
/// Returns the index of the first link that absorbed anything and the
/// total absorbed, or `None` when nothing did.
pub fn drive_chain(
    links: &[ChainLink],
    delta: Vector2D<f32>,
    mut scroll: impl FnMut(usize, Vector2D<f32>) -> Vector2D<f32>,
) -> Option<(usize, Vector2D<f32>)> {
    let (reach_x, reach_y) = chain_reach(links);
    let mut remaining = delta;
    let mut first = None;
    let mut consumed = Vector2D::zero();
    for index in chain_order(links) {
        let link = links[index];
        let admitted = Vector2D::new(
            if link.user_scrollable.x && index < reach_x {
                remaining.x
            } else {
                0.0
            },
            if link.user_scrollable.y && index < reach_y {
                remaining.y
            } else {
                0.0
            },
        );
        if admitted == Vector2D::zero() {
            continue;
        }
        let absorbed = scroll(index, admitted);
        if absorbed != Vector2D::zero() {
            first.get_or_insert(index);
            consumed += absorbed;
            remaining -= absorbed;
        }
        if remaining == Vector2D::zero() {
            break;
        }
    }
    first.map(|index| (index, consumed))
}

/// One scroll container's geometry, in CSS px, with its chaining policy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollBox {
    /// The visible area — the padding box (CSSOM-View `clientWidth`/`clientHeight`).
    pub scrollport: Size2D<f32>,
    /// The scrolling area (CSSOM-View `scrollWidth`/`scrollHeight`), never
    /// smaller than [`Self::scrollport`].
    pub scroll_size: Size2D<f32>,
    /// The current, already-clamped offset (CSSOM-View `scrollLeft`/`scrollTop`).
    pub offset: Vector2D<f32>,
    /// The axes the user may scroll directly.
    pub user_scrollable: ScrollAxes,
    /// The axes a boundary chains past (`overscroll-behavior: auto`).
    pub chains: ScrollAxes,
    /// Whether the container above goes first (`scroll-capture`).
    pub capture: ScrollCapture,
}

impl ScrollBox {
    /// This container's part in a chain walk.
    #[must_use]
    pub fn link(&self) -> ChainLink {
        ChainLink {
            user_scrollable: self.user_scrollable,
            chains: self.chains,
            capture: self.capture,
        }
    }

    /// The largest offset this box admits: how far its scrolling area
    /// overhangs its scrollport, never negative.
    #[must_use]
    pub fn max_offset(&self) -> Vector2D<f32> {
        Vector2D::new(
            (self.scroll_size.width - self.scrollport.width).max(0.0),
            (self.scroll_size.height - self.scrollport.height).max(0.0),
        )
    }
}

#[must_use]
pub(crate) fn is_scroll_container(style: &ComputedValues) -> bool {
    style.clone_overflow_x().is_scrollable() || style.clone_overflow_y().is_scrollable()
}

#[must_use]
fn user_scrollable_axes(style: &ComputedValues) -> ScrollAxes {
    ScrollAxes {
        x: style.clone_overflow_x().is_user_scrollable(),
        y: style.clone_overflow_y().is_user_scrollable(),
    }
}

#[must_use]
fn chaining_axes(style: &ComputedValues) -> ScrollAxes {
    ScrollAxes {
        x: style.clone_overscroll_behavior_x() == OverscrollBehavior::Auto,
        y: style.clone_overscroll_behavior_y() == OverscrollBehavior::Auto,
    }
}

#[must_use]
fn scroll_capture(style: &ComputedValues) -> ScrollCapture {
    match style.clone_scroll_capture() {
        scroll_capture::T::Auto => ScrollCapture::Auto,
        scroll_capture::T::Nearest => ScrollCapture::Nearest,
    }
}

fn clamp_axis(value: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, max)
    } else {
        0.0
    }
}

fn clamp_to(offset: Vector2D<f32>, max: Vector2D<f32>) -> Vector2D<f32> {
    Vector2D::new(clamp_axis(offset.x, max.x), clamp_axis(offset.y, max.y))
}

pub(crate) fn resolve(
    style: &ComputedValues,
    layout: &hughie::tree::Layout,
    stored: Vector2D<f32>,
) -> Option<ScrollBox> {
    if !is_scroll_container(style) {
        return None;
    }
    let scrollport = Size2D::new(
        (layout.size.width - layout.border.left - layout.border.right).max(0.0),
        (layout.size.height - layout.border.top - layout.border.bottom).max(0.0),
    );
    let scroll_size = Size2D::new(
        (layout.content_size.width - layout.border.left).max(scrollport.width),
        (layout.content_size.height - layout.border.top).max(scrollport.height),
    );
    let mut scroll_box = ScrollBox {
        scrollport,
        scroll_size,
        offset: Vector2D::zero(),
        user_scrollable: user_scrollable_axes(style),
        chains: chaining_axes(style),
        capture: scroll_capture(style),
    };
    scroll_box.offset = clamp_to(stored, scroll_box.max_offset());
    Some(scroll_box)
}

impl<T> Document<T> {
    /// Whether this node has scrollable overflow.
    #[must_use]
    pub fn is_scroll_container(&self, id: NodeId) -> bool {
        self.paint_style(id).is_some_and(is_scroll_container)
    }

    /// Returns this node's scroll geometry when it is a scroll container.
    #[must_use]
    pub fn scroll_box(&self, id: NodeId) -> Option<ScrollBox> {
        resolve(
            self.paint_style(id)?,
            self.rounded_layout(id)?,
            self.stored_scroll_offset(id),
        )
    }

    /// Returns the scroll offset clamped to current geometry.
    #[must_use]
    pub fn scroll_offset(&self, id: NodeId) -> Vector2D<f32> {
        self.scroll_box(id)
            .map_or_else(Vector2D::zero, |scroll_box| scroll_box.offset)
    }

    fn stored_scroll_offset(&self, id: NodeId) -> Vector2D<f32> {
        self.slot(id)
            .and_then(|slot| self.layout_state().get(slot))
            .map_or_else(Vector2D::zero, |state| state.scroll_offset)
    }

    /// Scrolls to a clamped offset and returns the applied offset.
    ///
    /// A moved offset does not invalidate the retained frame when that frame
    /// already carries this container as a scroll slot **and still covers the
    /// new offset**: the frame is baked unscrolled and composes offsets at
    /// use, so painting and hit testing both see the move with no rebuild.
    /// Two cases fall back to invalidating. A container the retained frame
    /// does not know — no frame yet, or one built before this box became a
    /// scroll container. And an offset past that slot's
    /// [`encode_window`](crate::visual::ScrollSlot::encode_window), which is
    /// the range the frame was culled — and its
    /// `content-visibility: auto` boxes determined — to stay valid over; past
    /// it, the committed frame simply has no content to compose.
    pub fn scroll_to(&mut self, id: NodeId, offset: Vector2D<f32>) -> Vector2D<f32> {
        debug_assert!(
            offset.x.is_finite() && offset.y.is_finite(),
            "scroll offsets must be finite, got {offset:?}"
        );
        let Some(scroll_box) = self.scroll_box(id) else {
            return Vector2D::zero();
        };
        let clamped = clamp_to(offset, scroll_box.max_offset());
        if clamped != scroll_box.offset {
            let composable = self
                .committed_frame()
                .is_some_and(|frame| frame.covers_scroll_offset(id, clamped));
            if !composable {
                self.note_visual_mutation();
            }
        }
        let slot = self
            .slot(id)
            .expect("a scroll container is a live node with layout-arena state");
        self.layout_state_mut().at_mut(slot).scroll_offset = clamped;
        clamped
    }

    /// Scrolls by a delta and returns the unconsumed remainder.
    pub fn scroll_by(&mut self, id: NodeId, delta: Vector2D<f32>) -> Vector2D<f32> {
        let Some(scroll_box) = self.scroll_box(id) else {
            return delta;
        };
        let applied = self.scroll_to(id, scroll_box.offset + delta);
        scroll_box.offset + delta - applied
    }

    fn scroll_parent(&self, id: NodeId) -> Option<NodeId> {
        let node = self.get(id)?;
        if !node.is_element() {
            return node.flat_parent_id();
        }
        let style = node.layout_computed_style()?;
        match style.clone_position() {
            PositionProperty::Absolute => Self::containing_block(node, false),
            PositionProperty::Fixed => Self::containing_block(node, true),
            PositionProperty::Static | PositionProperty::Relative | PositionProperty::Sticky => {
                box_parent(node).map(Node::id)
            }
        }
    }

    fn containing_block(node: &Node<T>, fixed: bool) -> Option<NodeId> {
        let mut current = box_parent(node);
        while let Some(ancestor) = current {
            let style = ancestor.layout_computed_style()?;
            let establishes = if fixed {
                establishes_fixed_containing_block(ancestor, style)
            } else {
                establishes_absolute_containing_block(ancestor, style)
            };
            if establishes {
                return Some(ancestor.id());
            }
            current = box_parent(ancestor);
        }
        None
    }

    /// Finds the nearest ancestor scrollable on every requested axis.
    #[must_use]
    pub fn nearest_user_scrollable(&self, id: NodeId, axes: ScrollAxes) -> Option<NodeId> {
        let mut current = Some(id);
        while let Some(node_id) = current {
            if let Some(node) = self.get(node_id)
                && node.is_element()
                && let Some(style) = node.layout_computed_style()
            {
                let scrollable = user_scrollable_axes(style);
                if (scrollable.x && axes.x) || (scrollable.y && axes.y) {
                    return Some(node_id);
                }
            }
            current = self.scroll_parent(node_id);
        }
        None
    }

    /// Applies a delta through the scroll chain that starts at `from`, and
    /// returns the first container that moved with the total consumed.
    ///
    /// `from` may be any node: the chain is every scroll container on its
    /// containing-block path, itself included, nearest first — `hidden`
    /// containers too, since they carry chain policy even though only a
    /// `scroll` axis consumes. Order and reach are [`drive_chain`]'s; see
    /// the module's *Chaining* section. The named container is the first to
    /// move in that order, which under `scroll-capture: nearest` can be an
    /// ancestor of the one the gesture started in.
    pub fn scroll_chain(
        &mut self,
        from: NodeId,
        delta: Vector2D<f32>,
    ) -> Option<(NodeId, Vector2D<f32>)> {
        self.chain_with(from, delta, ScrollKind::Gesture)
    }

    /// [`Self::scroll_chain`] for a discrete step with a direction and no
    /// end position of its own — a wheel tick: each container it reaches
    /// snaps as it lands ([`resolve_step`] with [`ScrollKind::Directed`]).
    pub fn scroll_chain_directed(
        &mut self,
        from: NodeId,
        delta: Vector2D<f32>,
    ) -> Option<(NodeId, Vector2D<f32>)> {
        self.chain_with(from, delta, ScrollKind::Directed)
    }

    fn chain_with(
        &mut self,
        from: NodeId,
        delta: Vector2D<f32>,
        kind: ScrollKind,
    ) -> Option<(NodeId, Vector2D<f32>)> {
        let mut nodes: SmallVec<[NodeId; 4]> = SmallVec::new();
        let mut links: SmallVec<[ChainLink; 4]> = SmallVec::new();
        let mut current = Some(from);
        while let Some(id) = current {
            if let Some(scroll_box) = self.scroll_box(id) {
                nodes.push(id);
                links.push(scroll_box.link());
            }
            current = self.scroll_parent(id);
        }
        let (first, consumed) = drive_chain(&links, delta, |index, admitted| {
            let id = nodes[index];
            let Some(scroll_box) = self.scroll_box(id) else {
                return Vector2D::zero();
            };
            let positions = match kind {
                ScrollKind::Directed => self.snap_positions(id),
                ScrollKind::Gesture => None,
            };
            let (applied, absorbed) = resolve_step(
                kind,
                scroll_box.offset,
                admitted,
                scroll_box.max_offset(),
                scroll_box.scrollport,
                positions.as_ref().and_then(SnapPositions::x),
                positions.as_ref().and_then(SnapPositions::y),
            );
            self.scroll_to(id, applied);
            absorbed
        })?;
        Some((nodes[first], consumed))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::StylesheetOrigin;
    use crate::tree::document::tests::device;

    fn nested_scrollers() -> (Document<()>, NodeId, NodeId) {
        nested_scrollers_with("")
    }

    /// An `outer` 200px row-flex scroller (max offset 0,800: its 100px-wide
    /// filler sits beside the inner box) holding an `inner` 100px scroller
    /// (max offset 200,300), with `extra_css` appended.
    fn nested_scrollers_with(extra_css: &str) -> (Document<()>, NodeId, NodeId) {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            &format!(
                "page {{ display: flex; width: 800px; height: 600px; }}
                 .outer {{ display: flex; overflow: scroll; width: 200px; height: 200px; }}
                 .inner {{ display: flex; overflow: scroll; width: 100px; height: 100px; }}
                 .content {{ flex-shrink: 0; width: 300px; height: 400px; }}
                 .tall {{ flex-shrink: 0; width: 100px; height: 1000px; }}
                 {extra_css}"
            ),
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

        let filler = document.create_element("view", ());
        document.add_class(filler, "tall");
        document.append_child(outer, filler);

        document.layout();
        (document, outer, inner)
    }

    #[test]
    fn scroll_geometry_comes_from_the_scrollable_overflow_the_layout_left() {
        let (document, _outer, inner) = nested_scrollers();
        let scroll_box = document.scroll_box(inner).expect("inner is a scroll box");
        assert_eq!(scroll_box.scrollport, Size2D::new(100.0, 100.0));
        assert_eq!(scroll_box.scroll_size, Size2D::new(300.0, 400.0));
        assert_eq!(scroll_box.max_offset(), Vector2D::new(200.0, 300.0));
        assert_eq!(scroll_box.user_scrollable, ScrollAxes::BOTH);
    }

    #[test]
    fn overflow_hidden_is_a_scroll_container_but_not_user_scrollable() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .clip { display: flex; overflow: hidden; width: 100px; height: 100px; }
             .content { flex-shrink: 0; width: 300px; height: 400px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let clip = document.create_element("view", ());
        document.add_class(clip, "clip");
        document.append_child(root, clip);
        let content = document.create_element("view", ());
        document.add_class(content, "content");
        document.append_child(clip, content);
        document.layout();

        assert!(document.is_scroll_container(clip));
        let scroll_box = document.scroll_box(clip).expect("hidden still scrolls");
        assert_eq!(scroll_box.user_scrollable, ScrollAxes::NONE);
        assert_eq!(scroll_box.max_offset(), Vector2D::new(200.0, 300.0));

        assert_eq!(
            document.scroll_to(clip, Vector2D::new(50.0, 60.0)),
            Vector2D::new(50.0, 60.0)
        );
        assert_eq!(
            document.nearest_user_scrollable(content, ScrollAxes::BOTH),
            None,
        );
    }

    #[test]
    fn offsets_clamp_into_range_and_report_the_unconsumed_remainder() {
        let (mut document, _outer, inner) = nested_scrollers();

        assert_eq!(
            document.scroll_to(inner, Vector2D::new(-40.0, 9_000.0)),
            Vector2D::new(0.0, 300.0),
        );
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 300.0));

        assert_eq!(
            document.scroll_by(inner, Vector2D::new(50.0, 25.0)),
            Vector2D::new(0.0, 25.0),
        );
        assert_eq!(document.scroll_offset(inner), Vector2D::new(50.0, 300.0));
    }

    #[test]
    fn a_shrinking_relayout_reclamps_the_offset_without_an_invalidation_hook() {
        let (mut document, _outer, inner) = nested_scrollers();
        document.scroll_to(inner, Vector2D::new(0.0, 300.0));

        document.add_stylesheet(".content { height: 120px; }", StylesheetOrigin::Author);
        document.layout();

        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 20.0));
    }

    #[test]
    fn a_box_that_stops_scrolling_reports_no_offset() {
        let (mut document, _outer, inner) = nested_scrollers();
        document.scroll_to(inner, Vector2D::new(0.0, 100.0));

        document.add_stylesheet(".inner { overflow: visible; }", StylesheetOrigin::Author);
        document.layout();

        assert!(!document.is_scroll_container(inner));
        assert_eq!(document.scroll_box(inner), None);
        assert_eq!(document.scroll_offset(inner), Vector2D::zero());
    }

    #[test]
    fn chaining_hands_the_remainder_to_the_next_scroller_out() {
        let (mut document, outer, inner) = nested_scrollers();

        let (named, total) = document
            .scroll_chain(inner, Vector2D::new(0.0, 400.0))
            .expect("something scrolled");
        assert_eq!(named, inner, "the innermost consumer names the event");
        assert_eq!(total, Vector2D::new(0.0, 400.0));
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 300.0));
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 100.0));

        document.scroll_to(outer, Vector2D::new(0.0, 1_000.0));
        let pinned_at = document.scroll_offset(outer);
        assert_eq!(document.scroll_chain(inner, Vector2D::new(0.0, 50.0)), None);
        assert_eq!(document.scroll_offset(outer), pinned_at);
    }

    #[test]
    fn the_chain_follows_containing_blocks_not_dom_ancestry() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; position: relative; width: 800px; height: 600px; }
             .scroller { display: flex; flex-direction: column; overflow: scroll;
                         width: 100px; height: 100px; }
             .row { flex-shrink: 0; width: 100px; height: 100px; }
             .pinned { display: flex; position: absolute; left: 0; top: 0;
                       width: 50px; height: 50px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let scroller = document.create_element("view", ());
        document.add_class(scroller, "scroller");
        document.append_child(root, scroller);
        for _ in 0..3 {
            let row = document.create_element("view", ());
            document.add_class(row, "row");
            document.append_child(scroller, row);
        }
        let pinned = document.create_element("view", ());
        document.add_class(pinned, "pinned");
        document.append_child(scroller, pinned);
        document.layout();

        assert_eq!(
            document.nearest_user_scrollable(pinned, ScrollAxes::BOTH),
            None,
            "an absolute box anchored on the page has no scroller in its chain",
        );
        assert_eq!(
            document.scroll_chain(pinned, Vector2D::new(0.0, 50.0)),
            None,
        );
        assert_eq!(document.scroll_offset(scroller), Vector2D::zero());

        let row = document
            .get(scroller)
            .and_then(|node| node.child_ids().first().copied())
            .expect("the scroller has rows");
        assert_eq!(
            document.nearest_user_scrollable(row, ScrollAxes::BOTH),
            Some(scroller),
        );

        document.add_stylesheet(
            ".scroller { position: relative; }",
            StylesheetOrigin::Author,
        );
        document.layout();
        assert_eq!(
            document.nearest_user_scrollable(pinned, ScrollAxes::BOTH),
            Some(scroller),
        );
    }

    #[test]
    fn a_non_finite_offset_cannot_poison_the_stored_scroll_position() {
        let (mut document, _outer, inner) = nested_scrollers();
        document.scroll_to(inner, Vector2D::new(0.0, 100.0));

        assert_eq!(
            clamp_to(
                Vector2D::new(f32::NAN, f32::INFINITY),
                Vector2D::new(200.0, 300.0)
            ),
            Vector2D::zero(),
        );
        assert!(document.scroll_offset(inner).y.is_finite());
    }

    #[test]
    fn the_chain_starts_at_the_nearest_scroller_above_the_target() {
        let (mut document, _outer, inner) = nested_scrollers();
        let content = document
            .get(inner)
            .and_then(|node| node.child_ids().first().copied())
            .expect("inner has its content child");

        let (consumer, _) = document
            .scroll_chain(content, Vector2D::new(0.0, 30.0))
            .expect("the ancestor scroller consumes it");
        assert_eq!(consumer, inner);
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 30.0));
    }

    fn link(capture: ScrollCapture, chains: ScrollAxes) -> ChainLink {
        ChainLink {
            user_scrollable: ScrollAxes::BOTH,
            chains,
            capture,
        }
    }

    #[test]
    fn a_capturing_link_is_visited_right_after_its_parent() {
        use ScrollCapture::{Auto, Nearest};
        let links = |captures: &[ScrollCapture]| {
            captures
                .iter()
                .map(|&capture| link(capture, ScrollAxes::BOTH))
                .collect::<Vec<_>>()
        };
        let order = |captures: &[ScrollCapture]| chain_order(&links(captures)).to_vec();
        assert_eq!(order(&[Auto, Auto, Auto]), [0, 1, 2]);
        assert_eq!(order(&[Nearest, Auto, Auto]), [1, 0, 2]);
        assert_eq!(order(&[Nearest, Nearest, Auto]), [2, 1, 0]);
        assert_eq!(order(&[Nearest, Auto, Nearest, Auto]), [1, 0, 3, 2]);
        assert_eq!(
            order(&[Auto, Nearest]),
            [0, 1],
            "the outermost has nothing to defer to"
        );
        assert_eq!(order(&[Nearest]), [0]);
        assert_eq!(order(&[]), [0usize; 0]);
    }

    #[test]
    fn containment_fences_the_links_above_it_whatever_the_order() {
        let links = [
            link(ScrollCapture::Nearest, ScrollAxes { x: true, y: false }),
            link(ScrollCapture::Auto, ScrollAxes::BOTH),
        ];
        let mut visited = Vec::new();
        let result = drive_chain(&links, Vector2D::new(10.0, 10.0), |index, admitted| {
            visited.push((index, admitted));
            admitted
        });
        assert_eq!(
            visited,
            [(1, Vector2D::new(10.0, 0.0)), (0, Vector2D::new(0.0, 10.0))],
            "the parent goes first but only sees the axis that chains; the rest stays inside",
        );
        assert_eq!(result, Some((1, Vector2D::new(10.0, 10.0))));
    }

    #[test]
    fn overscroll_contain_stops_the_chain_at_the_container() {
        let (mut document, outer, inner) =
            nested_scrollers_with(".inner { overscroll-behavior: contain; }");
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(0.0, 400.0)),
            Some((inner, Vector2D::new(0.0, 300.0))),
        );
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 300.0));
        assert_eq!(document.scroll_offset(outer), Vector2D::zero());
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(0.0, 50.0)),
            None,
            "pinned, and the remainder goes nowhere",
        );
        assert_eq!(document.scroll_offset(outer), Vector2D::zero());
    }

    #[test]
    fn overscroll_behavior_none_fences_like_contain_and_is_per_axis() {
        let (mut document, outer, inner) = nested_scrollers_with(
            ".inner { overscroll-behavior-y: none; flex-shrink: 0; } .tall { width: 500px; }",
        );
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(250.0, 400.0)),
            Some((inner, Vector2D::new(250.0, 300.0))),
        );
        assert_eq!(document.scroll_offset(inner), Vector2D::new(200.0, 300.0));
        assert_eq!(
            document.scroll_offset(outer),
            Vector2D::new(50.0, 0.0),
            "x chains out, y is fenced",
        );
    }

    #[test]
    fn a_hidden_container_with_contain_fences_the_chain_through_it() {
        let (mut document, outer, inner) =
            nested_scrollers_with(".inner { overflow: hidden; overscroll-behavior: contain; }");
        let content = document
            .get(inner)
            .and_then(|node| node.child_ids().first().copied())
            .expect("inner has content");
        assert_eq!(
            document.scroll_chain(content, Vector2D::new(0.0, 50.0)),
            None
        );
        assert_eq!(document.scroll_offset(outer), Vector2D::zero());

        document.add_stylesheet(
            ".inner { overscroll-behavior: auto; }",
            StylesheetOrigin::Author,
        );
        document.layout();
        assert_eq!(
            document.scroll_chain(content, Vector2D::new(0.0, 50.0)),
            Some((outer, Vector2D::new(0.0, 50.0))),
            "without it the hidden box is passed through to the outer scroller",
        );
    }

    #[test]
    fn scroll_capture_nearest_scrolls_the_ancestor_first() {
        let (mut document, outer, inner) =
            nested_scrollers_with(".inner { scroll-capture: nearest; }");
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(0.0, 50.0)),
            Some((outer, Vector2D::new(0.0, 50.0))),
        );
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 50.0));
        assert_eq!(document.scroll_offset(inner), Vector2D::zero());

        document.scroll_to(outer, Vector2D::new(0.0, 10_000.0));
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 800.0));
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(0.0, 50.0)),
            Some((inner, Vector2D::new(0.0, 50.0))),
            "the ancestor cannot move, so the container itself scrolls",
        );
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(0.0, 400.0)),
            Some((inner, Vector2D::new(0.0, 250.0))),
        );
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 300.0));

        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(0.0, -20.0)),
            Some((outer, Vector2D::new(0.0, -20.0))),
            "and the other way the ancestor is free again",
        );
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 300.0));
    }

    #[test]
    fn nearest_beside_contain_keeps_the_gesture_inside_the_container() {
        let (mut document, outer, inner) = nested_scrollers_with(
            ".inner { scroll-capture: nearest; overscroll-behavior: contain; }",
        );
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(0.0, 50.0)),
            Some((inner, Vector2D::new(0.0, 50.0))),
        );
        assert_eq!(document.scroll_offset(outer), Vector2D::zero());
    }

    #[test]
    fn the_published_slot_carries_the_chain_policy() {
        let (mut document, outer, inner) = nested_scrollers_with(
            ".inner { scroll-capture: nearest; overscroll-behavior-x: contain; }",
        );
        let frame = document.commit();
        let slot_of = |id: NodeId| {
            frame
                .scroll_slots()
                .iter()
                .find(|slot| slot.node == id)
                .copied()
                .expect("a scroll container has a slot")
        };
        let inner_slot = slot_of(inner);
        assert_eq!(inner_slot.capture, ScrollCapture::Nearest);
        assert_eq!(inner_slot.chains, ScrollAxes { x: false, y: true });
        assert_eq!(inner_slot.link().user_scrollable, ScrollAxes::BOTH);
        let outer_slot = slot_of(outer);
        assert_eq!(outer_slot.capture, ScrollCapture::Auto);
        assert_eq!(outer_slot.chains, ScrollAxes::BOTH);
    }
}
