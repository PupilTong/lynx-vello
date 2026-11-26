//! W3C [Resize Observer](https://drafts.csswg.org/resize-observer-1/) as an
//! engine primitive: the observers a document holds, the spec's gather step
//! over them, and the broadcast loop that hands each observer's entries to
//! its handler.
//!
//! The rulings this primitive is built on (`docs/layout-observer-design.md`
//! §5):
//! - **Rust only.** No JavaScript surface exists. The Lynx `layoutchange` adapter and a W3C-shaped
//!   MTS constructor are adapters over this primitive, not part of it.
//! - **Delivery is a posted entry, not the spec's in-rendering loop** (ruling 2). The spec gathers
//!   and broadcasts inside "update the rendering", before paint, in a loop: gather at a depth,
//!   broadcast, lay out again, gather below the shallowest depth delivered, and report the error
//!   `ResizeObserver loop completed with undelivered notifications` for whatever is still active
//!   above it (§3.4.1, §3.4.5, §3.4.6). Here [`Document::update_resize_observations`] gathers after
//!   the commit's layout and queues entries, and the host runs
//!   [`Document::notify_resize_observers`] from a task of its own after the frame is built. A
//!   handler's writes therefore land in the next commit, which lays out again and gathers again; no
//!   observation is ever held back for being shallower than one already delivered, so the depth
//!   filter, the skipped list and the loop error do not exist.
//! - **A handler per observer, as a trait object**, as the intersection primitive has
//!   ([`crate::visual::intersection`], the user's `dyn` ruling of 2026-10-10): one `Box<dyn
//!   ResizeEventHandler<T>>` per observer, so one document holds observers of any number of handler
//!   types and no host type is threaded through `Document<T>`.
//! - **Horizontal writing mode only.** Every box is laid out horizontally, so a size's inline axis
//!   is its width and its block axis its height: `inlineSize = width`, `blockSize = height`, for
//!   every box kind.
//!
//! # Sizes
//!
//! Every size is read from the target's rounded layout
//! ([`Document::rounded_layout`]): the border box is its `size`, the content
//! box that size less its `padding` and `border` edges, and the device-pixel
//! content box the content box times the document's device pixel ratio,
//! rounded to whole device pixels the way seldon's rounding tail rounds
//! (`crates/seldon/src/compute/mod.rs:1613`, `css_round_to_integer`: halves
//! round up).
//!
//! [`Document::observed_box_state`] sorts a target into one of three
//! [`BoxState`]s, walking up its box ancestors once:
//! - **Skipped**: an ancestor that has a box skips its contents (css-contain-2 §4,
//!   `content-visibility: hidden`, or `auto` while not relevant to the user). The update leaves the
//!   registration alone: no compare, no entry, and `lastReportedSizes` stays as it was. When the
//!   ancestor stops skipping and the target is laid out again, the next update compares its real
//!   size against the size it last reported, so a target that comes back at that size queues
//!   nothing. This is Chromium's: `ResizeObservation::ObservationSizeOutOfSync`
//!   (`third_party/blink/renderer/core/resize_observer/resize_observation.cc`) returns false for a
//!   target in a display-locked subtree (`DisplayLockUtilities::IsInLockedSubtreeCrossingFrames`),
//!   so web-core on Chrome fires no `layoutchange` for an item of a `content-visibility: auto` list
//!   while it is skipped. The skipping element itself is not skipped: its own box is laid out. A
//!   `display: none` ancestor, or one with no layout, above the skipping one means the skipping one
//!   has no box and skips nothing, and the target is not rendered.
//! - **Not rendered**: `0 × 0` in every box, with a zero `contentRect`. No live node, no style,
//!   `display: none` or `contents` on the target, a `display: none` ancestor or one with no layout,
//!   a box parent chain that does not reach the document element (detached), or a layout slot
//!   seldon hid. The target's own half of the test is the intersection geometry's
//!   (`Document::own_box` in `crates/terminus/src/visual/intersection/geometry.rs`).
//! - **Rendered**: the sizes of its rounded layout.
//!
//! # Observers, registrations and entries
//!
//! An observer is created with [`Document::create_resize_observer`] for one
//! handler and named by a [`ResizeObserverId`] that is never reissued. What
//! it observes is a list of registrations in observe order, each a target,
//! the [`ObservedBox`] it watches, and the spec's `lastReportedSizes`: the
//! size of that box the last entry reported. What it has to say is a queue
//! of [`ResizeObserverEntry`] records, each carrying the content rect and
//! all three box sizes, whichever box the registration watches.
//!
//! `lastReportedSizes` starts at `-1 × -1`, the spec's initial value (§3.1,
//! `ResizeObservation` constructor step 4, from the CSSWG resolution on
//! csswg-drafts issue 3664, 2022-08). No box has a negative size, so the
//! first update after `observe` reports every target once, a `0 × 0` or
//! unrendered one included; a skipped one is reported by the first update
//! after it stops being skipped. Chromium (`resize_observation.cc`,
//! `kInitialObservationSize` of indefinite sizes) and `WebKit` (bug 250836)
//! start from the same impossible size.
//!
//! # The update and the delivery
//!
//! [`Document::update_resize_observations`] is the spec's "gather active
//! observations" (§3.4.1) with "create and populate a `ResizeObserverEntry`"
//! (§3.4.4) folded in: for every observer in creation order and every
//! registration in observe order, the watched box's size is computed, and
//! where it differs from `lastReportedSizes` the size is remembered and one
//! entry queued. It invokes nothing. Setting `lastReportedSizes` here rather
//! than at broadcast (§3.4.5 step 2.3.3) is the same thing in this model,
//! since the entry is built here: a second update before the delivery
//! compares against what the first one queued, and an observer whose
//! delivery has not run by then holds one entry per update that moved a
//! target, in update order.
//!
//! [`Document::notify_resize_observers`] is §3.4.5's broadcast, minus the
//! depth it would return: each observer with entries queued, in creation
//! order, has its queue handed to its handler. The discipline is the
//! intersection primitive's: the list never leaves the document while a
//! handler runs, each observer is looked up again by id when its turn comes,
//! and its handler is out of it only while its own notification runs. A loop
//! run from inside a notification skips the observer whose handler is out
//! and leaves its entries queued for the next loop.
//!
//! # The stale bit
//!
//! The update runs only when a size could have moved. A **stale bit** is
//! set by every layout pass that ran (`Document::layout`'s
//! `run_layout_once`; a `layout()` that found nothing dirty runs no pass and
//! sets nothing), by every observe, and by the free of an observed target.
//! Only an update that ran clears it, and none runs before the document's
//! first completed layout, which leaves the bit set: a target read before any
//! pass has no box yet, and reporting that `0 × 0` would be an entry the
//! first layout immediately contradicts.
//!
//! This is narrower than the intersection primitive's bit, which every
//! render that built a frame and every scroll that moved an offset also
//! set. A scroll moves boxes and resizes none, and a frame built without a
//! layout pass (a repaint, an animation of a paint-only property) has the
//! same boxes as the last one, so neither can move an observation here.
//!
//! # Node lifetime
//!
//! `Document::free_node` tells the registry: observers whose handler is
//! [bound to](ResizeEventHandler::bound_to) the node are dropped, handler and
//! all, the node's registrations leave every observer, and queued entries
//! naming it are dropped, so every entry handed out names a live node. A
//! target that is unlinked but alive stays observed: the unlink dirties
//! layout, the pass that follows sets the stale bit, and the target, no
//! longer rendered, reports `0 × 0`.
//!
//! Skipping does not set the stale bit by itself: a box that starts or stops
//! skipping its contents changes the layout of its subtree, and that layout
//! pass sets it.
//!
//! # Not in the primitive
//!
//! - `takeRecords()` is not in the spec's IDL; [`Document::take_resize_records`] is an engine
//!   addition for a host that reads the queue without running the loop.
//! - SVG elements without a CSS box (the spec's bounding-box branch of §3.4.8), scrollbar gutters
//!   (no box here has a scrollbar that takes layout space) and vertical writing modes.
//!
//! # Cost
//!
//! A document with no observers pays one `is_empty` test per freed node and
//! per update, and one `bool` store per layout pass. An update that runs
//! costs, per registration, one walk up the target's box ancestors (the
//! [`BoxState`] test) and one size compare.

use std::num::NonZeroU32;

use euclid::default::{Point2D, Rect, Size2D};

use crate::layout::{DisplayMode, Layout, Size, StyleView, box_parent, skips_contents};
use crate::tree::document::{DOCUMENT_ELEMENT_NODE_ID, Document, NodeId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResizeObserverId(NonZeroU32);

impl ResizeObserverId {
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ObservedBox {
    #[default]
    ContentBox,
    BorderBox,
    DevicePixelContentBox,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizeObserverEntry {
    pub target: NodeId,
    pub content_rect: Rect<f32>,
    pub border_box_size: Size<f32>,
    pub content_box_size: Size<f32>,
    pub device_pixel_content_box_size: Size<f32>,
}

pub trait ResizeEventHandler<T> {
    fn notify(
        &mut self,
        document: &mut Document<T>,
        observer: ResizeObserverId,
        entries: Vec<ResizeObserverEntry>,
    );

    fn bound_to(&self) -> Option<NodeId> {
        None
    }
}

const INITIAL_REPORTED: Size<f32> = Size::new(-1.0, -1.0);

#[derive(Debug)]
struct Registration {
    target: NodeId,
    box_kind: ObservedBox,
    last_reported: Size<f32>,
}

pub struct ResizeObserver<T> {
    id: ResizeObserverId,
    targets: Vec<Registration>,
    queue: Vec<ResizeObserverEntry>,
    bound_to: Option<NodeId>,
    handler: Option<Box<dyn ResizeEventHandler<T>>>,
}

impl<T> ResizeObserver<T> {
    #[must_use]
    pub const fn id(&self) -> ResizeObserverId {
        self.id
    }

    #[must_use]
    pub fn observed_box(&self, target: NodeId) -> Option<ObservedBox> {
        self.targets
            .iter()
            .find(|registration| registration.target == target)
            .map(|registration| registration.box_kind)
    }

    #[must_use]
    pub fn queued(&self) -> &[ResizeObserverEntry] {
        &self.queue
    }

    #[must_use]
    pub const fn bound_to(&self) -> Option<NodeId> {
        self.bound_to
    }

    #[must_use]
    pub fn handler(&self) -> Option<&dyn ResizeEventHandler<T>> {
        self.handler.as_deref()
    }

    #[must_use]
    pub fn handler_mut(&mut self) -> Option<&mut dyn ResizeEventHandler<T>> {
        match &mut self.handler {
            Some(handler) => Some(&mut **handler),
            None => None,
        }
    }
}

impl<T> std::fmt::Debug for ResizeObserver<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResizeObserver")
            .field("id", &self.id)
            .field("targets", &self.targets)
            .field("queue", &self.queue)
            .field("bound_to", &self.bound_to)
            .field("notifying", &self.handler.is_none())
            .finish_non_exhaustive()
    }
}

pub(crate) struct ResizeObservers<T> {
    observers: Vec<ResizeObserver<T>>,
    next: NonZeroU32,
    stale: bool,
}

impl<T> Default for ResizeObservers<T> {
    fn default() -> Self {
        Self {
            observers: Vec::new(),
            next: NonZeroU32::MIN,
            stale: false,
        }
    }
}

impl<T> ResizeObservers<T> {
    pub(crate) fn is_empty(&self) -> bool {
        self.observers.is_empty()
    }

    pub(crate) fn note_layout(&mut self) {
        self.stale = true;
    }

    fn position(&self, id: ResizeObserverId) -> Option<usize> {
        self.observers
            .binary_search_by_key(&id, |observer| observer.id)
            .ok()
    }

    fn index(&self, id: ResizeObserverId) -> usize {
        self.position(id)
            .unwrap_or_else(|| panic!("{id:?} names no live resize observer"))
    }

    fn get_mut(&mut self, id: ResizeObserverId) -> &mut ResizeObserver<T> {
        let index = self.index(id);
        &mut self.observers[index]
    }

    pub(crate) fn forget_node(&mut self, node: NodeId) {
        self.observers
            .retain(|observer| observer.bound_to != Some(node));
        for observer in &mut self.observers {
            let observed = observer.targets.len();
            observer
                .targets
                .retain(|registration| registration.target != node);
            if observer.targets.len() != observed {
                self.stale = true;
            }
            observer.queue.retain(|entry| entry.target != node);
        }
    }
}

fn round_half_up(value: f32) -> f32 {
    let lower = value.floor();
    if value - lower < 0.5 {
        lower
    } else {
        lower + 1.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct BoxSizes {
    content_rect: Rect<f32>,
    border_box: Size<f32>,
    content_box: Size<f32>,
    device_pixel_content_box: Size<f32>,
}

impl BoxSizes {
    const UNRENDERED: Self = Self {
        content_rect: Rect::new(Point2D::new(0.0, 0.0), Size2D::new(0.0, 0.0)),
        border_box: Size::new(0.0, 0.0),
        content_box: Size::new(0.0, 0.0),
        device_pixel_content_box: Size::new(0.0, 0.0),
    };

    fn of(layout: &Layout, device_pixel_ratio: f32) -> Self {
        let Layout {
            size,
            border,
            padding,
            ..
        } = layout;
        let content_box = Size::new(
            size.width - padding.left - padding.right - border.left - border.right,
            size.height - padding.top - padding.bottom - border.top - border.bottom,
        );
        Self {
            content_rect: Rect::new(
                Point2D::new(padding.left, padding.top),
                Size2D::new(content_box.width, content_box.height),
            ),
            border_box: *size,
            content_box,
            device_pixel_content_box: content_box
                .map(|length| round_half_up(length * device_pixel_ratio)),
        }
    }

    const fn observed(&self, box_kind: ObservedBox) -> Size<f32> {
        match box_kind {
            ObservedBox::ContentBox => self.content_box,
            ObservedBox::BorderBox => self.border_box,
            ObservedBox::DevicePixelContentBox => self.device_pixel_content_box,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum BoxState<'a> {
    Rendered(&'a Layout),
    Skipped,
    NotRendered,
}

impl<T> Document<T> {
    pub(crate) fn observed_box_state(&self, id: NodeId) -> BoxState<'_> {
        let Some(node) = self.get(id) else {
            return BoxState::NotRendered;
        };
        let mut skipped = false;
        let mut boxed = true;
        let mut top = node;
        while let Some(ancestor) = box_parent(top) {
            let style = StyleView::of(ancestor);
            if style.display_mode() == DisplayMode::None
                || self.rounded_layout(ancestor.id()).is_none()
            {
                skipped = false;
                boxed = false;
            } else if skips_contents(ancestor, style.values()) {
                skipped = true;
            }
            top = ancestor;
        }
        if top.id() != DOCUMENT_ELEMENT_NODE_ID {
            return BoxState::NotRendered;
        }
        if skipped {
            return BoxState::Skipped;
        }
        if !boxed {
            return BoxState::NotRendered;
        }
        self.own_box(id)
            .map_or(BoxState::NotRendered, |(_, _, layout)| {
                BoxState::Rendered(layout)
            })
    }

    pub fn create_resize_observer(
        &mut self,
        handler: Box<dyn ResizeEventHandler<T>>,
    ) -> ResizeObserverId {
        let bound_to = handler.bound_to();
        if let Some(node) = bound_to {
            assert!(
                self.get(node).is_some(),
                "a resize observer's handler is bound to {node:?}, which must be a live node"
            );
        }
        let resizes = &mut self.resizes;
        let id = ResizeObserverId(resizes.next);
        resizes.next = resizes
            .next
            .checked_add(1)
            .expect("a document cannot create u32::MAX resize observers");
        resizes.observers.push(ResizeObserver {
            id,
            targets: Vec::new(),
            queue: Vec::new(),
            bound_to,
            handler: Some(handler),
        });
        id
    }

    #[must_use]
    pub fn resize_observer(&self, observer: ResizeObserverId) -> Option<&ResizeObserver<T>> {
        let index = self.resizes.position(observer)?;
        Some(&self.resizes.observers[index])
    }

    #[must_use]
    pub fn resize_observer_mut(
        &mut self,
        observer: ResizeObserverId,
    ) -> Option<&mut ResizeObserver<T>> {
        let index = self.resizes.position(observer)?;
        Some(&mut self.resizes.observers[index])
    }

    pub fn observe_resize(
        &mut self,
        observer: ResizeObserverId,
        target: NodeId,
        box_kind: ObservedBox,
    ) {
        assert!(
            self.get(target)
                .is_some_and(crate::tree::node::Node::is_element),
            "a resize observer's target {target:?} must be a live element"
        );
        let resizes = &mut self.resizes;
        let observer = resizes.get_mut(observer);
        if let Some(index) = observer
            .targets
            .iter()
            .position(|registration| registration.target == target)
        {
            if observer.targets[index].box_kind == box_kind {
                return;
            }
            observer.targets.remove(index);
        }
        observer.targets.push(Registration {
            target,
            box_kind,
            last_reported: INITIAL_REPORTED,
        });
        resizes.stale = true;
    }

    pub fn unobserve_resize(&mut self, observer: ResizeObserverId, target: NodeId) {
        self.resizes
            .get_mut(observer)
            .targets
            .retain(|registration| registration.target != target);
    }

    pub fn disconnect_resize_observer(&mut self, observer: ResizeObserverId) {
        let observer = self.resizes.get_mut(observer);
        observer.targets.clear();
        observer.queue.clear();
    }

    #[must_use]
    pub fn take_resize_records(&mut self, observer: ResizeObserverId) -> Vec<ResizeObserverEntry> {
        std::mem::take(&mut self.resizes.get_mut(observer).queue)
    }

    pub fn drop_resize_observer(&mut self, observer: ResizeObserverId) {
        let index = self.resizes.index(observer);
        self.resizes.observers.remove(index);
    }

    pub fn update_resize_observations(&mut self) -> bool {
        if self.resizes.observers.is_empty() || !self.resizes.stale || !self.has_laid_out() {
            return false;
        }
        self.resizes.stale = false;
        let device_pixel_ratio = self.device_pixel_ratio();
        let mut observers = std::mem::take(&mut self.resizes.observers);
        let mut queued = false;
        for observer in &mut observers {
            for registration in &mut observer.targets {
                let sizes = match self.observed_box_state(registration.target) {
                    BoxState::Rendered(layout) => BoxSizes::of(layout, device_pixel_ratio),
                    BoxState::NotRendered => BoxSizes::UNRENDERED,
                    BoxState::Skipped => continue,
                };
                let size = sizes.observed(registration.box_kind);
                if size == registration.last_reported {
                    continue;
                }
                registration.last_reported = size;
                observer.queue.push(ResizeObserverEntry {
                    target: registration.target,
                    content_rect: sizes.content_rect,
                    border_box_size: sizes.border_box,
                    content_box_size: sizes.content_box,
                    device_pixel_content_box_size: sizes.device_pixel_content_box,
                });
                queued = true;
            }
        }
        debug_assert!(
            self.resizes.observers.is_empty(),
            "nothing creates an observer during an update"
        );
        self.resizes.observers = observers;
        queued
    }

    #[must_use]
    pub fn has_pending_resize_notifications(&self) -> bool {
        self.resizes
            .observers
            .iter()
            .any(|observer| !observer.queue.is_empty())
    }

    pub fn notify_resize_observers(&mut self) {
        let notify_list: Vec<ResizeObserverId> = self
            .resizes
            .observers
            .iter()
            .filter(|observer| !observer.queue.is_empty())
            .map(|observer| observer.id)
            .collect();
        for id in notify_list {
            let Some(index) = self.resizes.position(id) else {
                continue;
            };
            let observer = &mut self.resizes.observers[index];
            if observer.queue.is_empty() {
                continue;
            }
            let Some(mut handler) = observer.handler.take() else {
                continue;
            };
            let entries = std::mem::take(&mut observer.queue);
            handler.notify(self, id, entries);
            if let Some(index) = self.resizes.position(id) {
                self.resizes.observers[index].handler = Some(handler);
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests;
