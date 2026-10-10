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
//!   because both are scroll containers and the property applies to scroll containers. The engine's
//!   own fourth value, **`contain-bounce`**, fences the same way and is published as the
//!   [`ScrollBox::bounce`] axes: the runtime's painter reads them off the committed frame and
//!   stretches the boundary there. Nothing in this crate stretches — a programmatic scroll clamps.
//!   Its fifth value, **`circular`**, also fences like `contain` and is published as the
//!   [`ScrollBox::circular`] axes: such an axis has no boundary for the painter, which wraps its
//!   live offset around a period of the whole scrolling area. The wrap is the painter's alone — the
//!   document keeps clamping every offset it writes, a programmatic scroll included, and never sees
//!   an offset outside `0..=max`.
//! - **`scroll-capture`** is this engine's own, with no W3C or Lynx counterpart, and it changes the
//!   *order*, not the reach. It is a shorthand over the physical longhands `scroll-capture-x` and
//!   `scroll-capture-y` (no logical pair: a chain walks physical axes), each `auto | nearest [
//!   forward | backward ]?`, and each axis is ordered by its own longhand, so a container can hand
//!   its vertical gestures to its ancestor while its horizontal ones nest inner first. `nearest` on
//!   a container hands a gesture that starts in it to the nearest scroll container above it first;
//!   this container moves only once that ancestor cannot (it is at its boundary in that direction,
//!   or does not scroll that axis at all). The chain then continues outward past the ancestor as
//!   usual. It nests: `nearest` on both of two nested containers visits the grandparent, then the
//!   parent, then the innermost. A direction narrows it: `nearest forward` defers only a delta that
//!   increases the offset on the axis being walked, `nearest backward` only one that decreases it;
//!   the other direction keeps the inner-first order. `nearest` alone is both. This is how
//!   `<scroll-coordinator>` folds its header before its content scrolls, and unfolds it after.
//!   Reach is decided before order, so `nearest` beside `contain` on the same container keeps the
//!   gesture inside it — the ancestor it would have deferred to is exactly what `contain` fences
//!   off.
//!
//! Order is decided **per axis and per step**, from that axis's longhand and
//! the sign of that axis's delta ([`chain_order`], [`chain_orders`]): a link
//! defers only for a delta it [defers](ScrollCapture::defers), and an axis the step does not move
//! keeps the inner-first order. A fling is a step per frame, so each of its
//! frames is ordered afresh. When the two axes' orders coincide the walk is
//! one walk over both; when they differ, [`drive_chain`] walks x in its
//! order and then y in its own.
//!
//! Where each step lands is [`snap`]'s: css-scroll-snap-1 positions per
//! container, applied as a wheel tick lands and when a drag ends. Where a
//! container *starts* is `initial_target`'s: css-scroll-snap-2's
//! `scroll-initial-target`, honoured after the paint build finds the element.
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
//! - The `scroll-behavior` property is absent, so a script-facing scroll names its
//!   [`ScrollBehavior`] outright ([`Document::scroll_to_with`], module `request`). Every offset
//!   this crate writes itself is instantaneous and clamps hard at the boundary, and a snap is a
//!   jump. A smooth scroll, inertia, the `contain-bounce` stretch and the `circular` wrap exist
//!   only on the painter's side, over the committed frame; `overscroll-behavior: none` therefore
//!   does exactly what `contain` does here — there is no document-side boundary effect for it to
//!   suppress.

use euclid::default::{Size2D, Vector2D};
use hughie::style::PositionProperty;
use smallvec::SmallVec;
use stylo::properties::ComputedValues;
use stylo::values::computed::{OverscrollBehavior, ScrollCapture as ComputedScrollCapture};

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
pub(crate) mod initial_target;
mod request;
pub mod snap;
#[cfg(test)]
mod sticky_geometry_tests;

pub use request::{ScrollBehavior, ScrollRequest};
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
    /// `nearest`: the nearest scroll container above this one goes first,
    /// in either direction; this one moves only once that ancestor cannot.
    Nearest,
    /// `nearest forward`: the ancestor goes first for a delta that
    /// increases the offset on the axis being walked; a decreasing one keeps
    /// the inner-first order.
    NearestForward,
    /// `nearest backward`: the ancestor goes first for a delta that
    /// decreases the offset on the axis being walked; an increasing one
    /// keeps the inner-first order.
    NearestBackward,
}

impl ScrollCapture {
    /// Whether this container hands a delta of `sign` on one axis to the
    /// scroll container above it first: a positive delta is forward, a
    /// negative one backward, and a zero (or NaN) delta has no direction and
    /// is never deferred, so an axis the step does not move keeps the
    /// inner-first order.
    #[must_use]
    pub fn defers(self, sign: f32) -> bool {
        let forward = sign > 0.0;
        let backward = sign < 0.0;
        match self {
            Self::Auto => false,
            Self::Nearest => forward || backward,
            Self::NearestForward => forward,
            Self::NearestBackward => backward,
        }
    }
}

/// A container's `scroll-capture` on each physical axis: the
/// `scroll-capture-x` and `scroll-capture-y` longhands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaptureAxes {
    pub x: ScrollCapture,
    pub y: ScrollCapture,
}

impl CaptureAxes {
    /// The same value on both axes, as the `scroll-capture` shorthand sets.
    #[must_use]
    pub const fn both(capture: ScrollCapture) -> Self {
        Self {
            x: capture,
            y: capture,
        }
    }

    /// The value on `axis`.
    #[must_use]
    pub fn on(self, axis: ScrollAxis) -> ScrollCapture {
        match axis {
            ScrollAxis::X => self.x,
            ScrollAxis::Y => self.y,
        }
    }
}

/// One physical scroll axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAxis {
    X,
    Y,
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
    /// Whether the container above goes first, per axis.
    pub capture: CaptureAxes,
}

/// The order a chain of links is visited in on `axis` for a delta of
/// `sign` on it, as indices into `links`.
///
/// `links` is nearest-first: index 0 is the container the gesture starts in
/// and each next index is the scroll container above the previous one. A
/// link whose `scroll-capture` on `axis` [defers](ScrollCapture::defers) `sign` moves
/// to directly after its parent, and the placement runs from the outermost
/// link inward so a nested `nearest` resolves against a parent that has
/// already found its own place. Every other link keeps its inner-first
/// place.
#[must_use]
pub fn chain_order(links: &[ChainLink], axis: ScrollAxis, sign: f32) -> SmallVec<[usize; 4]> {
    let mut order = SmallVec::new();
    for index in (0..links.len()).rev() {
        if links[index].capture.on(axis).defers(sign) && index + 1 < links.len() {
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

/// A chain walk's visiting order on each axis, as indices into the links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainOrders {
    pub x: SmallVec<[usize; 4]>,
    pub y: SmallVec<[usize; 4]>,
}

/// The order a chain of links is visited in for `delta`, per axis: each
/// axis's [`chain_order`], from that axis's own `scroll-capture` and the sign
/// of its own delta. An axis with a zero delta keeps the inner-first order.
#[must_use]
pub fn chain_orders(links: &[ChainLink], delta: Vector2D<f32>) -> ChainOrders {
    ChainOrders {
        x: chain_order(links, ScrollAxis::X, delta.x),
        y: chain_order(links, ScrollAxis::Y, delta.y),
    }
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

/// Drives `delta` through a chain of links, in [`chain_orders`] and within
/// each axis's `overscroll-behavior` reach, calling `scroll` with a link's
/// index and the delta it is admitted; `scroll` applies what it can and
/// returns the delta it absorbed, which is what stops chaining on (a
/// snapped step can absorb more than it moved — see [`resolve_step`]).
/// Returns the index of the first link that absorbed anything and the
/// total absorbed, or `None` when nothing did.
///
/// When both axes visit the links in the same order it is one walk and
/// `scroll` sees both axes at once. When the orders differ — a step that
/// moves one axis only, past a `nearest` link — the x axis is walked in its
/// order and then the y axis in its own, and `scroll` sees a single-axis
/// delta; "first" is then the first link to absorb in that sequence. Two
/// walks rather than an interleaving, because the axes are independent —
/// neither walk reads what the other absorbed — and a link visited at
/// different positions on the two axes has no single place to receive both.
pub fn drive_chain(
    links: &[ChainLink],
    delta: Vector2D<f32>,
    mut scroll: impl FnMut(usize, Vector2D<f32>) -> Vector2D<f32>,
) -> Option<(usize, Vector2D<f32>)> {
    let (reach_x, reach_y) = chain_reach(links);
    let orders = chain_orders(links, delta);
    let mut first = None;
    let mut consumed = Vector2D::zero();
    let mut walk = |order: &[usize], delta: Vector2D<f32>| {
        let mut remaining = delta;
        for &index in order {
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
    };
    if orders.x == orders.y {
        walk(&orders.x, delta);
    } else {
        walk(&orders.x, Vector2D::new(delta.x, 0.0));
        walk(&orders.y, Vector2D::new(0.0, delta.y));
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
    /// The axes whose boundary stretches and springs back
    /// (`overscroll-behavior: contain-bounce`). Published for the runtime's
    /// painter, which owns the stretch; the document itself always clamps.
    pub bounce: ScrollAxes,
    /// The axes with no boundary (`overscroll-behavior: circular`). Policy
    /// only: published for the runtime's painter, which wraps its live
    /// offset around the scrolling area; the document itself always clamps.
    pub circular: ScrollAxes,
    /// Whether the container above goes first, per axis
    /// (`scroll-capture-x` / `scroll-capture-y`).
    pub capture: CaptureAxes,
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
    style.get_overflow_x().is_scrollable() || style.get_overflow_y().is_scrollable()
}

#[must_use]
fn user_scrollable_axes(style: &ComputedValues) -> ScrollAxes {
    ScrollAxes {
        x: style.get_overflow_x().is_user_scrollable(),
        y: style.get_overflow_y().is_user_scrollable(),
    }
}

#[must_use]
fn chaining_axes(style: &ComputedValues) -> ScrollAxes {
    ScrollAxes {
        x: *style.get_overscroll_behavior_x() == OverscrollBehavior::Auto,
        y: *style.get_overscroll_behavior_y() == OverscrollBehavior::Auto,
    }
}

#[must_use]
fn bouncing_axes(style: &ComputedValues) -> ScrollAxes {
    ScrollAxes {
        x: *style.get_overscroll_behavior_x() == OverscrollBehavior::ContainBounce,
        y: *style.get_overscroll_behavior_y() == OverscrollBehavior::ContainBounce,
    }
}

#[must_use]
fn circular_axes(style: &ComputedValues) -> ScrollAxes {
    ScrollAxes {
        x: *style.get_overscroll_behavior_x() == OverscrollBehavior::Circular,
        y: *style.get_overscroll_behavior_y() == OverscrollBehavior::Circular,
    }
}

#[must_use]
fn scroll_capture(style: &ComputedValues) -> CaptureAxes {
    let lower = |value: ComputedScrollCapture| match value {
        ComputedScrollCapture::Auto => ScrollCapture::Auto,
        ComputedScrollCapture::Nearest => ScrollCapture::Nearest,
        ComputedScrollCapture::NearestForward => ScrollCapture::NearestForward,
        ComputedScrollCapture::NearestBackward => ScrollCapture::NearestBackward,
    };
    CaptureAxes {
        x: lower(*style.get_scroll_capture_x()),
        y: lower(*style.get_scroll_capture_y()),
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
    // `content_size` is the scrollable overflow rectangle in border-box
    // coordinates, floored at the padding box's far edge (css-overflow-3
    // §3.3's "the scroll container's own padding box"), so less the start
    // border it is the scrolling area from the scrollport's origin: an empty
    // container's is its scrollport, whatever its far border.
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
        bounce: bouncing_axes(style),
        circular: circular_axes(style),
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
    ///
    /// Records no request for the painter: this is the write the runtime
    /// adopts the painter's own posted offsets with, so a painter holding an
    /// offset of its own for the container may post over it. A scroll the
    /// painter has to honour is [`Self::scroll_to_with`].
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
            // Intersection observations move with the offset whether or not
            // the frame does: a scroll the retained frame composes commits
            // nothing, so no render would say so.
            self.note_intersections_stale();
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

    pub(crate) fn scroll_parent(&self, id: NodeId) -> Option<NodeId> {
        let node = self.get(id)?;
        if !node.is_element() {
            return node.flat_parent_id();
        }
        // A top-layer element's chain ends at it: its containing block is
        // the initial one (`tree::top_layer`).
        if node.arenas().top_layer().places_against_viewport(id) {
            return None;
        }
        let style = node.layout_computed_style()?;
        match *style.get_box().get_position() {
            PositionProperty::Absolute => Self::containing_block(node, false),
            PositionProperty::Fixed => Self::containing_block(node, true),
            PositionProperty::Static | PositionProperty::Relative | PositionProperty::Sticky => {
                box_parent(node).map(Node::id)
            }
        }
    }

    fn containing_block(node: &Node<T>, fixed: bool) -> Option<NodeId> {
        let top_layer = node.arenas().top_layer();
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
            if !top_layer.is_empty() && top_layer.contains(ancestor.id()) {
                return None;
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
                // The document never wraps: a circular axis is the
                // painter's, and every offset written here clamps.
                ScrollAxes::NONE,
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

    /// A slotted scroller chains into the scroller of its host's shadow tree
    /// that holds the slot: the chain walks the flat tree, through the
    /// `display: contents` slot, across the shadow boundary.
    #[test]
    fn a_slotted_scroller_chains_into_the_shadow_scroller_around_its_slot() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .inner { display: flex; flex-shrink: 0; overflow: scroll; width: 100px; height: 100px; }
             .content { flex-shrink: 0; width: 100px; height: 400px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let host = document.create_element("view", ());
        document.append_child(root, host);
        let shadow = document.attach_shadow(host, crate::ShadowRootMode::Open);
        document.add_shadow_stylesheet(
            shadow,
            "slot { display: contents; }
             div { display: flex; flex-direction: column; overflow: scroll;
                   width: 100px; height: 200px; }
             span { display: block; flex-shrink: 0; height: 300px; }",
        );
        let outer = document.create_element("div", ());
        document.append_child(shadow, outer);
        let slot = document.create_element("slot", ());
        document.append_child(outer, slot);
        let filler = document.create_element("span", ());
        document.append_child(outer, filler);
        let inner = document.create_element("view", ());
        document.add_class(inner, "inner");
        document.append_child(host, inner);
        let content = document.create_element("view", ());
        document.add_class(content, "content");
        document.append_child(inner, content);
        document.layout();

        assert_eq!(document.scroll_parent(inner), Some(outer));
        let (named, total) = document
            .scroll_chain(content, Vector2D::new(0.0, 350.0))
            .expect("something scrolled");
        assert_eq!((named, total), (inner, Vector2D::new(0.0, 350.0)));
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 300.0));
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 50.0));
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
            capture: CaptureAxes::both(capture),
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
        let order =
            |captures: &[ScrollCapture]| chain_order(&links(captures), ScrollAxis::Y, 1.0).to_vec();
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

    /// `contain-bounce` is `contain`'s fence in the document: the chain
    /// stops, and a programmatic scroll still clamps — the stretch is the
    /// painter's. What the document publishes is the `bounce` axis flag.
    #[test]
    fn contain_bounce_fences_like_contain_and_publishes_its_axes() {
        let (mut document, outer, inner) = nested_scrollers_with(
            ".inner { overscroll-behavior-y: contain-bounce; flex-shrink: 0; } .tall { width: 500px; }",
        );
        let scroll_box = document.scroll_box(inner).expect("inner is a scroll box");
        assert_eq!(scroll_box.bounce, ScrollAxes { x: false, y: true });
        assert_eq!(scroll_box.chains, ScrollAxes { x: true, y: false });
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(250.0, 400.0)),
            Some((inner, Vector2D::new(250.0, 300.0))),
        );
        assert_eq!(document.scroll_offset(inner), Vector2D::new(200.0, 300.0));
        assert_eq!(
            document.scroll_offset(outer),
            Vector2D::new(50.0, 0.0),
            "x chains out, y is fenced and clamped",
        );
    }

    /// `circular` is `contain`'s fence in the document too: the chain stops
    /// and every offset the document writes clamps — the wrap is the
    /// painter's. What the document publishes is the `circular` axis flag.
    #[test]
    fn circular_fences_like_contain_and_publishes_its_axes() {
        let (mut document, outer, inner) = nested_scrollers_with(
            ".inner { overscroll-behavior-y: circular; flex-shrink: 0; } .tall { width: 500px; }",
        );
        let scroll_box = document.scroll_box(inner).expect("inner is a scroll box");
        assert_eq!(scroll_box.circular, ScrollAxes { x: false, y: true });
        assert_eq!(scroll_box.bounce, ScrollAxes::NONE);
        assert_eq!(scroll_box.chains, ScrollAxes { x: true, y: false });
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(250.0, 400.0)),
            Some((inner, Vector2D::new(250.0, 300.0))),
        );
        assert_eq!(document.scroll_offset(inner), Vector2D::new(200.0, 300.0));
        assert_eq!(
            document.scroll_offset(outer),
            Vector2D::new(50.0, 0.0),
            "x chains out, y is fenced and clamped",
        );
        document.scroll_to(inner, Vector2D::new(0.0, 1000.0));
        assert_eq!(
            document.scroll_offset(inner),
            Vector2D::new(0.0, 300.0),
            "a programmatic scroll clamps too",
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
        assert_eq!(
            inner_slot.capture,
            CaptureAxes::both(ScrollCapture::Nearest)
        );
        assert_eq!(inner_slot.chains, ScrollAxes { x: false, y: true });
        assert_eq!(inner_slot.link().user_scrollable, ScrollAxes::BOTH);
        let outer_slot = slot_of(outer);
        assert_eq!(outer_slot.capture, CaptureAxes::default());
        assert_eq!(outer_slot.chains, ScrollAxes::BOTH);
    }
    #[test]
    fn scroll_capture_defers_the_directions_it_names() {
        use ScrollCapture::{Auto, Nearest, NearestBackward, NearestForward};
        let defers =
            |capture: ScrollCapture| [1.0, -1.0, 0.0, f32::NAN].map(|sign| capture.defers(sign));
        assert_eq!(defers(Auto), [false, false, false, false]);
        assert_eq!(defers(Nearest), [true, true, false, false]);
        assert_eq!(defers(NearestForward), [true, false, false, false]);
        assert_eq!(defers(NearestBackward), [false, true, false, false]);
    }

    #[test]
    fn nested_nearest_forward_goes_outermost_first_forward_only() {
        use ScrollCapture::{Auto, NearestForward};
        let links = [
            link(NearestForward, ScrollAxes::BOTH),
            link(NearestForward, ScrollAxes::BOTH),
            link(Auto, ScrollAxes::BOTH),
        ];
        assert_eq!(chain_order(&links, ScrollAxis::Y, 1.0).to_vec(), [2, 1, 0]);
        assert_eq!(chain_order(&links, ScrollAxis::Y, -1.0).to_vec(), [0, 1, 2]);
        assert_eq!(chain_order(&links, ScrollAxis::Y, 0.0).to_vec(), [0, 1, 2]);
    }

    #[test]
    fn each_axis_orders_the_chain_by_its_own_delta() {
        let links = [
            link(ScrollCapture::NearestForward, ScrollAxes::BOTH),
            link(ScrollCapture::Auto, ScrollAxes::BOTH),
        ];
        let orders = chain_orders(&links, Vector2D::new(0.0, 10.0));
        assert_eq!(
            orders.x.to_vec(),
            [0, 1],
            "an unmoved axis keeps the inner-first order"
        );
        assert_eq!(orders.y.to_vec(), [1, 0]);
        let orders = chain_orders(&links, Vector2D::new(-10.0, 10.0));
        assert_eq!(orders.x.to_vec(), [0, 1], "backward is not deferred");
        assert_eq!(orders.y.to_vec(), [1, 0]);
    }

    #[test]
    fn differing_axis_orders_walk_each_axis_on_its_own() {
        let links = [
            link(ScrollCapture::NearestForward, ScrollAxes::BOTH),
            link(ScrollCapture::Auto, ScrollAxes::BOTH),
        ];
        let mut visited = Vec::new();
        let result = drive_chain(&links, Vector2D::new(-10.0, 10.0), |index, admitted| {
            visited.push((index, admitted));
            admitted * 0.5
        });
        assert_eq!(
            visited,
            [
                (0, Vector2D::new(-10.0, 0.0)),
                (1, Vector2D::new(-5.0, 0.0)),
                (1, Vector2D::new(0.0, 10.0)),
                (0, Vector2D::new(0.0, 5.0)),
            ],
            "x walks inner first, then y walks the parent first, each with its own axis only",
        );
        assert_eq!(result, Some((0, Vector2D::new(-7.5, 7.5))));
    }

    #[test]
    fn nearest_forward_hands_only_forward_deltas_to_the_ancestor() {
        let (mut document, outer, inner) =
            nested_scrollers_with(".inner { scroll-capture: nearest forward; }");
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(0.0, 50.0)),
            Some((outer, Vector2D::new(0.0, 50.0))),
        );
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 50.0));
        assert_eq!(document.scroll_offset(inner), Vector2D::zero());

        document.scroll_to(inner, Vector2D::new(0.0, 100.0));
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(0.0, -120.0)),
            Some((inner, Vector2D::new(0.0, -120.0))),
            "backward is inner first: the inner empties, then the ancestor takes the rest",
        );
        assert_eq!(document.scroll_offset(inner), Vector2D::zero());
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 30.0));
    }

    #[test]
    fn nearest_backward_is_the_mirror() {
        let (mut document, outer, inner) =
            nested_scrollers_with(".inner { scroll-capture: nearest backward; }");
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(0.0, 50.0)),
            Some((inner, Vector2D::new(0.0, 50.0))),
        );
        assert_eq!(document.scroll_offset(outer), Vector2D::zero());

        document.scroll_to(outer, Vector2D::new(0.0, 100.0));
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(0.0, -20.0)),
            Some((outer, Vector2D::new(0.0, -20.0))),
        );
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 80.0));
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 50.0));
    }

    #[test]
    fn a_hidden_ancestor_is_admitted_nothing_whatever_the_direction() {
        let (mut document, outer, inner) = nested_scrollers_with(
            ".inner { scroll-capture: nearest forward; } .outer { overflow-y: hidden; }",
        );
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(0.0, 50.0)),
            Some((inner, Vector2D::new(0.0, 50.0))),
        );
        assert_eq!(document.scroll_offset(outer), Vector2D::zero());
    }

    #[test]
    fn the_published_slot_carries_the_capture_direction() {
        let (mut document, _outer, inner) =
            nested_scrollers_with(".inner { scroll-capture: nearest backward; }");
        let frame = document.commit();
        let slot = frame
            .scroll_slots()
            .iter()
            .find(|slot| slot.node == inner)
            .copied()
            .expect("a scroll container has a slot");
        let backward = CaptureAxes::both(ScrollCapture::NearestBackward);
        assert_eq!(slot.capture, backward);
        assert_eq!(slot.link().capture, backward);
    }
    fn link_y(capture: ScrollCapture) -> ChainLink {
        ChainLink {
            user_scrollable: ScrollAxes::BOTH,
            chains: ScrollAxes::BOTH,
            capture: CaptureAxes {
                x: ScrollCapture::Auto,
                y: capture,
            },
        }
    }

    /// A horizontal scroller inside a horizontal pager inside a vertical
    /// coordinator, the two inner ones capturing on y only: x nests inner
    /// first as usual, y goes outermost first.
    #[test]
    fn a_y_only_capture_leaves_x_nesting_inner_first() {
        use ScrollCapture::{Auto, NearestForward};
        let links = [link_y(NearestForward), link_y(NearestForward), link_y(Auto)];
        assert_eq!(chain_order(&links, ScrollAxis::X, 1.0).to_vec(), [0, 1, 2]);
        assert_eq!(chain_order(&links, ScrollAxis::Y, 1.0).to_vec(), [2, 1, 0]);
        let orders = chain_orders(&links, Vector2D::new(10.0, 10.0));
        assert_eq!(orders.x.to_vec(), [0, 1, 2]);
        assert_eq!(orders.y.to_vec(), [2, 1, 0]);
    }

    #[test]
    fn a_diagonal_step_lands_each_axis_on_its_own_link() {
        let links = [
            link_y(ScrollCapture::NearestForward),
            link(ScrollCapture::Auto, ScrollAxes::BOTH),
        ];
        let mut visited = Vec::new();
        let result = drive_chain(&links, Vector2D::new(10.0, 10.0), |index, admitted| {
            visited.push((index, admitted));
            admitted
        });
        assert_eq!(
            visited,
            [(0, Vector2D::new(10.0, 0.0)), (1, Vector2D::new(0.0, 10.0))],
        );
        assert_eq!(result, Some((0, Vector2D::new(10.0, 10.0))));
    }

    #[test]
    fn the_longhands_capture_one_axis_each() {
        let (mut document, outer, inner) =
            nested_scrollers_with(".inner { scroll-capture-y: nearest forward; }");
        let scroll_box = document.scroll_box(inner).expect("inner is a scroll box");
        assert_eq!(
            scroll_box.capture,
            CaptureAxes {
                x: ScrollCapture::Auto,
                y: ScrollCapture::NearestForward,
            }
        );
        assert_eq!(
            document.scroll_chain(inner, Vector2D::new(50.0, 50.0)),
            Some((inner, Vector2D::new(50.0, 50.0))),
        );
        assert_eq!(
            document.scroll_offset(inner),
            Vector2D::new(50.0, 0.0),
            "x stays inner first; y went to the ancestor",
        );
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 50.0));
    }
}
