//! The paint snapshot: one frame, resolved off the document and owned.
//!
//! [`PaintOrder`] answers *where* things paint; it names nodes, and a
//! renderer holding one still has to reach back into the document for every
//! style, layout, and paragraph it draws. [`Frame`] closes that seam: it
//! pairs the order with the resolved payload each item needs, so the renderer
//! never borrows the document at all.
//!
//! That buys three things:
//!
//! 1. **It is `Send + Sync + 'static`.** A frame can be handed to a paint thread while the DOM
//!    thread runs the next script tick, style flush, and layout. Nothing here borrows from
//!    `Document`, and every payload type (`servo_arc::Arc<ComputedValues>`, `Arc<TextLayout>`,
//!    plain `Copy` geometry) is already shared-thread-safe.
//! 2. **It cannot go stale.** [`PaintOrder`] has to fail closed against post-build mutation
//!    ([`PaintOrder::assert_visually_fresh`]) precisely because it resolves against live state. A
//!    `Frame` resolved its state at build time, so there is no live state to disagree with and no
//!    freshness assert on the paint path.
//! 3. **It keeps the tree walks on the side that owns the tree.** Three paint inputs are not
//!    per-node lookups but *traversals* — the css-text-decor-3 §2 decorating-box chain, the
//!    `background-clip: text` glyph silhouette, and a gradient-valued `color`'s positioning area.
//!    All three are resolved here, where the tree is, and arrive at the renderer as flat data.
//!
//! The price is one `Arc` bump per painted item and layer, plus a `Vec` of
//! payloads per frame. Against a full scene rebuild that is noise, and it is
//! what makes the thread split possible.
//!
//! Hit testing comes along: its body never read the document — it is geometry
//! over this snapshot, and the `&Document` it used to take bought only the
//! staleness assert a `Frame` cannot need. [`Frame::hit_test`] therefore runs
//! wherever the frame does. Resolving a hit into DOM *dispatch* is still the
//! document's business; finding out what is under a point is not.

use std::sync::Arc as StdArc;

use euclid::default::Rect;
use smallvec::SmallVec;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;

use super::{PaintItemKind, PaintOrder};
use crate::layout::{Edges, Layout, TextLayout};
use crate::tree::document::{Document, NodeId};
use crate::{Point2D, Size2D, Vector2D};

/// One frame, ready to paint with no document in hand.
///
/// Items and layers are parallel to [`PaintOrder::items`] and
/// [`PaintOrder::layers`] — index `i` of [`Self::item_paint`] is the payload
/// for index `i` of [`Self::items`].
#[derive(Debug)]
pub struct Frame {
    order: PaintOrder,
    items: Vec<ItemPaint>,
    layers: Vec<LayerPaint>,
    viewport: Size2D<f32>,
    device_pixel_ratio: f32,
}

impl Frame {
    /// Back-to-front paint items.
    #[must_use]
    pub(crate) fn items(&self) -> &[super::PaintItem] {
        self.order.items()
    }

    /// Clip arena referenced by `PaintItem::clip` and `ClipNode::parent`.
    #[must_use]
    pub(crate) fn clips(&self) -> &[super::ClipNode] {
        self.order.clips()
    }

    /// Render layers, in preorder.
    #[must_use]
    pub(crate) fn layers(&self) -> &[super::RenderLayer] {
        self.order.layers()
    }

    /// Scroll arena, in preorder — what a renderer scrolls ahead of the
    /// document with. See [`ScrollNode`](super::ScrollNode).
    #[must_use]
    pub fn scrolls(&self) -> &[super::ScrollNode] {
        self.order.scrolls()
    }

    /// [`PaintOrder::node_removal_epoch`](super::PaintOrder::node_removal_epoch)
    /// for this frame — what a message carrying one of its `NodeId`s across a
    /// thread boundary must be pinned to.
    #[must_use]
    pub const fn node_removal_epoch(&self) -> u64 {
        self.order.node_removal_epoch()
    }

    /// The scroll container a gesture at `point` belongs to — see
    /// [`PaintOrder::scroll_target`](super::PaintOrder::scroll_target).
    #[must_use]
    pub fn scroll_target(&self, point: Point2D<f32>, offsets: &[Vector2D<f32>]) -> Option<usize> {
        self.order.scroll_target(point, offsets)
    }

    /// The viewport-space translation from this frame's baked offsets to
    /// `offsets` — see
    /// [`PaintOrder::scroll_correction`](super::PaintOrder::scroll_correction).
    #[must_use]
    pub(crate) fn scroll_correction(
        &self,
        scroll: Option<usize>,
        offsets: &[Vector2D<f32>],
    ) -> Vector2D<f32> {
        self.order.scroll_correction(scroll, offsets)
    }

    /// The topmost element under `point`, as the frame stands at `offsets`.
    ///
    /// Hit testing needs no document — see
    /// [`PaintOrder::hit_test_at`](super::PaintOrder::hit_test_at) — so a
    /// renderer holding a frame resolves its own targets rather than asking
    /// the thread that owns the DOM.
    #[must_use]
    pub fn hit_test(&self, point: Point2D<f32>, offsets: &[Vector2D<f32>]) -> Option<NodeId> {
        self.order.hit_test_at(point, offsets).map(|(node, _)| node)
    }

    /// Resolved paint payload for [`Self::items`]`[index]`.
    ///
    /// # Panics
    ///
    /// Panics on an out-of-range index — the two lists are built together and
    /// a renderer indexing past one has already lost the frame's shape.
    #[must_use]
    pub(crate) fn item_paint(&self, index: usize) -> &ItemPaint {
        &self.items[index]
    }

    /// Resolved paint payload for [`Self::layers`]`[index]`.
    ///
    /// # Panics
    ///
    /// Panics on an out-of-range index, as [`Self::item_paint`] does.
    #[must_use]
    pub(crate) fn layer_paint(&self, index: usize) -> &LayerPaint {
        &self.layers[index]
    }

    /// Viewport size in CSS px, as the document's `Device` reported it when
    /// the frame was built.
    #[must_use]
    pub const fn viewport(&self) -> Size2D<f32> {
        self.viewport
    }

    /// The device pixel ratio the layouts in this frame were rounded to — the
    /// one CSS px → device px scale the renderer applies as a root transform.
    #[must_use]
    pub const fn device_pixel_ratio(&self) -> f32 {
        self.device_pixel_ratio
    }

    /// The document visual epoch this frame represents.
    #[must_use]
    pub(crate) const fn visual_epoch(&self) -> u64 {
        self.order.visual_epoch()
    }
}

/// What one paint item draws, resolved.
#[derive(Debug)]
pub(crate) struct ItemPaint {
    /// The style the item paints under: an element's own, or for a text run
    /// its styled parent element's.
    ///
    /// `None` for a node the completed traversal left unstyled — the
    /// renderer's signal to skip the item, matching what a `paint_style`
    /// miss used to mean.
    pub(crate) style: Option<Arc<ComputedValues>>,
    pub(crate) content: ItemContent,
}

/// The kind-specific half of [`ItemPaint`].
#[derive(Debug)]
pub(crate) enum ItemContent {
    /// The node had no layout slot to paint from.
    Absent,
    Element(ElementPaint),
    Text(TextPaint),
}

/// An element box's resolved content.
#[derive(Debug)]
pub(crate) struct ElementPaint {
    pub(crate) metrics: BoxMetrics,
    /// Intrinsic dimensions for replaced content; [`NaturalSize::NONE`]
    /// otherwise.
    pub(crate) natural_size: crate::layout::NaturalSize,
    /// Glyph silhouette for `background-clip: text`: the element's descendant
    /// paragraphs with their offsets from its border-box origin. `None` when
    /// no background layer asks for the clip; `Some(empty)` when it does and
    /// the element has no visible text, which clips the layer away entirely.
    pub(crate) text_clip: Option<Vec<TextClipRun>>,
}

/// A text run's resolved content.
#[derive(Debug)]
pub(crate) struct TextPaint {
    /// The committed, shaped, line-broken paragraph.
    pub(crate) layout: StdArc<TextLayout>,
    /// The css-text-decor-3 §2 decorating-box chain that propagates lines
    /// into this run, nearest ancestor first. Each entry draws in its **own**
    /// style and color, so the styles travel rather than a flattened result.
    pub(crate) decorations: SmallVec<[Arc<ComputedValues>; 2]>,
    /// Positioning area for a gradient-valued `color`, in the run's local
    /// space. `None` for solid text, which must not pay for the lookup.
    pub(crate) gradient_box: Option<Rect<f32>>,
}

/// One paragraph in an element's `background-clip: text` silhouette.
#[derive(Debug)]
pub(crate) struct TextClipRun {
    /// Offset from the clipping element's border-box origin.
    pub(crate) offset: Vector2D<f32>,
    pub(crate) layout: StdArc<TextLayout>,
}

/// The parts of a rounded [`Layout`] a painter needs to derive the padding
/// and content boxes from a border box.
///
/// `Layout` is deliberately non-`Clone` (it is the arena's durable record);
/// these are its `Copy` fields, lifted out rather than the whole value
/// duplicated.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BoxMetrics {
    pub(crate) border: Edges<f32>,
    pub(crate) padding: Edges<f32>,
}

impl BoxMetrics {
    fn of(layout: &Layout) -> Self {
        Self {
            border: layout.border,
            padding: layout.padding,
        }
    }
}

/// A render layer's resolved payload: the effect parameters come from the
/// establishing element's style, and `clip-path`/`mask` geometry needs its
/// box metrics.
#[derive(Debug)]
pub(crate) struct LayerPaint {
    pub(crate) style: Option<Arc<ComputedValues>>,
    /// `None` when the establishing element has no layout slot, which
    /// suppresses the geometry-derived effects but not the group itself.
    pub(crate) metrics: Option<BoxMetrics>,
}

impl<T: Sync> Document<T> {
    /// Resolves an already-built [`PaintOrder`] against this document.
    ///
    /// # Panics
    ///
    /// Panics if `order` was not built from this document's current visual
    /// state — resolving a stale order would mix one frame's geometry with
    /// another's styles.
    pub(crate) fn resolve_frame(&self, order: PaintOrder) -> Frame {
        order.assert_visually_fresh(self);
        let items = order
            .items()
            .iter()
            .map(|item| self.resolve_item(item))
            .collect();
        let layers = order
            .layers()
            .iter()
            .map(|layer| LayerPaint {
                style: self.shared_paint_style(layer.node),
                metrics: self.rounded_layout(layer.node).map(BoxMetrics::of),
            })
            .collect();
        let viewport = self.device().viewport_size();
        Frame {
            order,
            items,
            layers,
            viewport: Size2D::new(viewport.width, viewport.height),
            device_pixel_ratio: self.device().device_pixel_ratio().get(),
        }
    }

    fn resolve_item(&self, item: &super::PaintItem) -> ItemPaint {
        match item.kind {
            PaintItemKind::ElementBox => {
                let style = self.shared_paint_style(item.node);
                let content =
                    self.rounded_layout(item.node)
                        .map_or(ItemContent::Absent, |layout| {
                            ItemContent::Element(ElementPaint {
                                metrics: BoxMetrics::of(layout),
                                natural_size: self.natural_size(item.node),
                                text_clip: style
                                    .as_deref()
                                    .is_some_and(clips_background_to_text)
                                    .then(|| self.text_clip_runs(item.node)),
                            })
                        });
                ItemPaint { style, content }
            }
            PaintItemKind::TextRun { element } => {
                let style = self.shared_paint_style(element);
                let content =
                    self.shared_text_layout(item.node)
                        .map_or(ItemContent::Absent, |layout| {
                            ItemContent::Text(TextPaint {
                                layout,
                                decorations: self.decorating_boxes(element),
                                gradient_box: style
                                    .as_deref()
                                    .is_some_and(has_gradient_color)
                                    .then(|| self.color_gradient_box(item, element))
                                    .flatten(),
                            })
                        });
                ItemPaint { style, content }
            }
        }
    }

    /// The post-flush computed style as a shared handle. Mirrors
    /// [`Self::paint_style`]'s answer; it bumps the `Arc` because the value
    /// has to outlive this borrow.
    fn shared_paint_style(&self, id: NodeId) -> Option<Arc<ComputedValues>> {
        self.get(id)?.computed_style()
    }

    fn shared_text_layout(&self, id: NodeId) -> Option<StdArc<TextLayout>> {
        self.layout_state()
            .nodes
            .get(id)?
            .text
            .as_deref()?
            .committed_shared()
    }

    /// The decorating boxes whose lines propagate into text under `element`
    /// — css-text-decor-3 §2: `text-decoration-line` is *not* inherited;
    /// each ancestor box that draws a line is a decorating box whose lines
    /// reach all in-flow descendant text, drawn in that box's own style and
    /// color.
    ///
    /// Collected nearest-first. Propagation from ancestors stops at an
    /// out-of-flow (absolutely positioned) box, which per spec does not
    /// receive them — that box's own decorations still apply. Boxless
    /// (`display: contents`) ancestors count as decorating boxes, matching
    /// browser rendering of a decorated `display: contents` span.
    fn decorating_boxes(&self, element: NodeId) -> SmallVec<[Arc<ComputedValues>; 2]> {
        use stylo::computed_values::position::T as Position;
        let mut out = SmallVec::new();
        let mut current = Some(element);
        while let Some(id) = current {
            let Some(node) = self.get(id) else { break };
            if !node.is_element() {
                break;
            }
            let Some(style) = self.paint_style(id) else {
                break;
            };
            if draws_decoration_line(style) {
                out.push(
                    self.shared_paint_style(id)
                        .expect("the style was just borrowed"),
                );
            }
            if matches!(
                style.get_box().position,
                Position::Absolute | Position::Fixed
            ) {
                break;
            }
            current = node.parent_id();
        }
        out
    }

    /// The positioning area for a gradient-valued `color` (Lynx's
    /// text-gradient sugar), in the text run's local space.
    ///
    /// The gradient anchors to the styled *element*, not to the run: the same
    /// gradient authored as `background-image` would resolve against the
    /// element's padding box (`background-origin: padding-box` is the initial
    /// value), and anchoring per run would restart the ramp on every line of
    /// a wrapped paragraph.
    ///
    /// A text run's `element` is its DOM parent and its `Layout.location` is
    /// relative to that same box, so the padding-box origin in run-local
    /// space is `border_origin - location`. When the parent is boxless
    /// (`display: contents`, whose layout slot the host zeroes) there is no
    /// box to anchor to and the run's own box stands in — signalled here as
    /// `None`, which the renderer resolves against the item size it already
    /// has.
    fn color_gradient_box(&self, item: &super::PaintItem, element: NodeId) -> Option<Rect<f32>> {
        let element_layout = self.rounded_layout(element)?;
        let run_layout = self.rounded_layout(item.node)?;
        let size = element_layout.size;
        let border = element_layout.border;
        let width = (size.width - border.right).max(border.left) - border.left;
        let height = (size.height - border.bottom).max(border.top) - border.top;
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        Some(Rect::new(
            super::Point2D::new(
                border.left - run_layout.location.x,
                border.top - run_layout.location.y,
            ),
            Size2D::new(width, height),
        ))
    }

    /// The committed paragraphs under `element`, with offsets accumulated
    /// from its border-box origin — the glyph source a
    /// `background-clip: text` layer is clipped to.
    ///
    /// `Layout.location` is box-parent-relative and the layout host zeroes
    /// boxless (`display: contents`) elements' slots, so unconditional
    /// accumulation composes correctly; `display: none` subtrees contribute
    /// nothing because their text never commits a layout. Recorded v1 limits
    /// carried over from the renderer: descendant `transform`s are ignored
    /// (the silhouette uses layout positions only) and `visibility: hidden`
    /// text is skipped via its parent's style.
    fn text_clip_runs(&self, element: NodeId) -> Vec<TextClipRun> {
        let mut runs = Vec::new();
        self.collect_text_clip_runs(element, Vector2D::zero(), &mut runs);
        runs
    }

    fn collect_text_clip_runs(
        &self,
        node: NodeId,
        offset: Vector2D<f32>,
        runs: &mut Vec<TextClipRun>,
    ) {
        let Some(node_ref) = self.get(node) else {
            return;
        };
        for child in node_ref.children() {
            if child.is_text_node() {
                let visible = self.paint_style(node).is_none_or(|style| {
                    matches!(
                        style.clone_visibility(),
                        stylo::computed_values::visibility::T::Visible
                    )
                });
                if !visible {
                    continue;
                }
                if let (Some(layout), Some(text)) = (
                    self.rounded_layout(child.id()),
                    self.shared_text_layout(child.id()),
                ) {
                    runs.push(TextClipRun {
                        offset: offset + Vector2D::new(layout.location.x, layout.location.y),
                        layout: text,
                    });
                }
            } else if child.is_element() {
                let child_offset = self.rounded_layout(child.id()).map_or(offset, |layout| {
                    offset + Vector2D::new(layout.location.x, layout.location.y)
                });
                self.collect_text_clip_runs(child.id(), child_offset, runs);
            }
        }
    }
}

/// Whether any background layer clips to the element's descendant glyphs.
/// A rare property, which is why the silhouette walk is gated on it.
fn clips_background_to_text(style: &ComputedValues) -> bool {
    use stylo::values::computed::BackgroundClip;
    style
        .get_background()
        .background_clip
        .0
        .iter()
        .any(|clip| matches!(clip, BackgroundClip::Text))
}

/// Whether `color` is a gradient (Lynx's text-gradient sugar) rather than a
/// solid — nearly all text is solid and must not pay for the positioning-area
/// lookups.
fn has_gradient_color(style: &ComputedValues) -> bool {
    use stylo::values::computed::ColorPropertyValue;
    matches!(
        style.get_inherited_text().color,
        ColorPropertyValue::Gradient(..)
    )
}

/// Whether this box is a *decorating box* — it draws at least one
/// decoration line in a style that renders. The `lynx` fork compiles
/// `text-decoration-line` without an `OVERLINE` bit (Lynx's
/// `TextDecorationType` has none), so only these two exist.
fn draws_decoration_line(style: &ComputedValues) -> bool {
    use stylo::computed_values::text_decoration_style::T as TextDecorationStyle;
    use stylo::values::computed::TextDecorationLine;
    let text = style.get_text();
    let line = text.text_decoration_line;
    (line.contains(TextDecorationLine::UNDERLINE)
        || line.contains(TextDecorationLine::LINE_THROUGH))
        && !matches!(text.text_decoration_style, TextDecorationStyle::MozNone)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::Frame;

    /// The load-bearing property of this module. A `Frame` that stops being
    /// `Send` cannot reach a paint thread, and the failure would otherwise
    /// surface as an inscrutable error in whichever crate tries to send one.
    #[test]
    fn a_frame_can_cross_a_thread_boundary() {
        const fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<Frame>();
    }
}

/// Renders [`Frame`]s, and scrolls them, without a document.
///
/// [`Document::render`] is the in-process path: the document paints into a
/// scene it retains and lends back. This is the same painter driven from a
/// snapshot instead — which is what makes painting on another thread
/// possible, because a `Frame` owns everything the walk reads and a
/// `Ref<'_, Scene>` borrowed from a document does not cross threads.
///
/// It owns the frame, the scroll offsets, and the painter together, because
/// on the thread that holds them those are one job: an event that scrolls
/// must repaint, and repainting must use the offsets that event produced.
/// Splitting them would leave a host to sequence three objects correctly on
/// every gesture.
///
/// The encapsulation is unchanged from the in-process path: the paint order,
/// its items, clips, and layers stay crate-internal either way. What an
/// embedder gets is a scene, exactly as [`Document::scene`] gives one.
///
/// Decoded images live here rather than on the frame: pixels belong with the
/// renderer that uploads them, while the natural sizes layout needs stay on
/// the document.
#[derive(Default)]
pub struct FrameRenderer {
    painter: crate::paint::painter::Painter,
    frame: Option<Frame>,
    scroller: crate::scroll::frame_scroller::FrameScroller,
}

impl std::fmt::Debug for FrameRenderer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FrameRenderer")
            .field("has_frame", &self.frame.is_some())
            .finish_non_exhaustive()
    }
}

impl FrameRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or updates the decoded images this renderer paints from.
    pub const fn images_mut(&mut self) -> &mut crate::ImageStore {
        self.painter.images_mut()
    }

    /// Adopts a newly published frame, carrying forward any scroll this side
    /// has done that the document has not confirmed yet.
    ///
    /// `confirmed_scroll` is the highest [`ScrollUpdate::seq`] the document
    /// had applied when it built `frame`.
    pub fn adopt(&mut self, frame: Frame, confirmed_scroll: u64) {
        self.scroller.adopt(&frame, confirmed_scroll);
        self.frame = Some(frame);
    }

    /// Whether a frame has been adopted yet.
    #[must_use]
    pub const fn has_frame(&self) -> bool {
        self.frame.is_some()
    }

    /// Routes one input event against the frame in hand.
    ///
    /// A gesture that resolves to a scroll container is answered here and
    /// now: the offsets move, and [`Self::render`] will paint them. Nothing
    /// waits on the document — which is the point, because that round trip is
    /// a restyle, a relayout, and a republish.
    ///
    /// The caller still forwards the event to the document for *targeting*,
    /// with [`InputEvent::default_prevented`] set to
    /// [`FrameInput::owns_gesture`], and applies the returned
    /// [`ScrollUpdate`]s with
    /// [`Document::scroll_to`](Document::scroll_to). Returns an empty
    /// response when no frame has been adopted.
    pub fn handle_input(&mut self, event: &crate::input::InputEvent) -> crate::FrameInput {
        let Some(frame) = self.frame.as_ref() else {
            return crate::FrameInput::default();
        };
        self.scroller.handle_input(frame, event)
    }

    /// Paints the frame in hand at this renderer's current scroll offsets.
    ///
    /// Returns `None` until a frame has been adopted.
    pub fn render(&mut self) -> Option<&crate::vello::Scene> {
        let frame = self.frame.as_ref()?;
        self.painter
            .paint_scrolled(frame, self.scroller.paint_offsets(frame));
        Some(self.painter.scene())
    }

    /// The scene built by the last render.
    #[must_use]
    pub const fn scene(&self) -> &crate::vello::Scene {
        self.painter.scene()
    }
}
