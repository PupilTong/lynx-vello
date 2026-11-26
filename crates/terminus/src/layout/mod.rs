//! Box layout over the document tree — the concrete [`seldon`] host.

pub(crate) mod anchors;
pub(crate) mod committed_box;
mod hoisted;
mod host;
pub(crate) mod inline_flow;
pub(crate) mod paragraph;
pub(crate) mod relevance;
mod style;
pub(crate) mod text_block;

use std::sync::LazyLock;

use euclid::default::{Point2D, Rect, Size2D, Vector2D};
use rustc_hash::FxHashSet;
#[cfg(feature = "layout-test-utils")]
use seldon::compute::LeafMetrics;
pub use seldon::compute::NaturalSize;
pub(crate) use seldon::geometry::Edges;
pub use seldon::geometry::Size;
use seldon::invalidate::is_relayout_boundary;
pub(crate) use seldon::style::TextBrush;
use seldon::style::{CoreStyle, PositionProperty};
use seldon::text::{FontBlob, TextContext};
pub use seldon::tree::Layout;
use seldon::tree::LayoutSlot;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;

pub(crate) use self::style::{
    DisplayMode, StyleView, box_parent, converts_tail_color, display_mode,
    establishes_absolute_containing_block, establishes_fixed_containing_block, generates_no_box,
    paragraph_limits_changed, reads_percentage_basis, skips_contents,
};
use crate::clock::ClockInstant;
use crate::render::image::ImageStatus;
use crate::render::picture::PictureKey;
use crate::tree::document::{DOCUMENT_ELEMENT_NODE_ID, Document, NodeLayoutState, RelayoutKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParagraphKind {
    Lynx,
    InlineFormattingContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextLineReport {
    pub start: u32,
    pub end: u32,
    pub ellipsis_count: u32,
}

pub(crate) static ANONYMOUS_STYLE: LazyLock<Arc<ComputedValues>> = LazyLock::new(|| {
    use stylo::properties::style_structs::Font;
    ComputedValues::initial_values_with_font_override(Font::initial_values())
});

impl<T: Sync> Document<T> {
    pub fn layout(&mut self) {
        let anchors = self.select_scroll_anchors();
        let mut resized = Vec::new();
        for pass in 0..committed_box::CONTAINER_PASSES {
            self.layout_pass(&mut resized);
            let cap = pass + 1 == committed_box::CONTAINER_PASSES;
            let timelines = self.resolve_timelines();
            let containers = self.recascade_resized_containers(&mut resized, cap);
            if !timelines && !containers {
                break;
            }
        }
        if !anchors.is_empty() {
            self.adjust_scroll_anchors(anchors);
        }
        self.clamp_scroll_offsets_after_layout();
        self.refresh_editing_after_layout();
    }

    fn layout_pass(&mut self, resized: &mut Vec<crate::tree::document::NodeId>) {
        let timed = self.commit_timer.marking();
        let style_start = timed.then(ClockInstant::now);
        self.flush_styles_with_damage_sink(&mut |_, _| {});
        if let Some(start) = style_start {
            self.commit_timer.mark_style(start, ClockInstant::now());
        }

        let viewport_size = self.device().viewport_size();
        let viewport = Size::new(viewport_size.width, viewport_size.height);
        let scale = self.device().device_pixel_ratio().get();

        if !self.layout_needs_pass(viewport, scale) {
            return;
        }

        let layout_start = timed.then(ClockInstant::now);
        self.run_layout_once(viewport, scale, resized);
        let state = self.layout_state_mut();
        if !state.anchored.is_empty() || !state.anchor_pending.get_mut().is_empty() {
            self.settle_anchor_runs(viewport, scale, resized);
        }
        if let Some(start) = layout_start {
            self.commit_timer.mark_layout(start, ClockInstant::now());
        }
    }

    #[inline]
    fn run_layout_once(
        &mut self,
        viewport: Size<f32>,
        scale: f32,
        resized: &mut Vec<crate::tree::document::NodeId>,
    ) {
        let full = self.layout_requires_full_pass(viewport, scale);
        let rescale = self.layout_inputs_changed(viewport, scale);
        let (bound, elements) = (self.arenas().slot_bound(), self.arenas().live_elements());
        self.layout_state_mut().ensure_covers(bound, elements);
        host::run_layout(self, viewport, scale, full, rescale, resized);
        self.arenas_mut().publish_committed_boxes(resized);
        self.clear_relayout_roots();
        self.mark_layout_complete(viewport, scale);
        self.resizes.note_layout();
    }

    #[cold]
    #[inline(never)]
    fn settle_anchor_runs(
        &mut self,
        viewport: Size<f32>,
        scale: f32,
        resized: &mut Vec<crate::tree::document::NodeId>,
    ) {
        let mut invalidated = FxHashSet::default();
        while self.settle_anchors(&mut invalidated) {
            self.run_layout_once(viewport, scale, resized);
        }
    }

    fn recascade_resized_containers(
        &mut self,
        resized: &mut Vec<crate::tree::document::NodeId>,
        cap: bool,
    ) -> bool {
        if resized.is_empty() {
            return false;
        }
        if !self.arenas().uses_container_units() {
            resized.clear();
            return false;
        }
        let mut marked = false;
        for id in resized.drain(..) {
            if self
                .get(id)
                .is_some_and(crate::tree::node::Node::is_element)
            {
                if cap {
                    self.mark_subtree_recascade(id);
                    marked = true;
                } else {
                    marked |= self.mark_container_units_users(id);
                }
            }
        }
        marked
    }
}

#[must_use]
pub(crate) fn natural_size(width: u32, height: u32) -> NaturalSize {
    #[expect(
        clippy::cast_precision_loss,
        reason = "an intrinsic size large enough to lose precision here is far past anything \
                  layout can present, and rounds harmlessly. It is deliberately not bounded: \
                  the texture limit applies to the decoded bitmap, not to this."
    )]
    NaturalSize::from_size(seldon::geometry::Size::new(width as f32, height as f32))
}

impl<T> Document<T> {
    pub fn set_natural_size(
        &mut self,
        id: crate::tree::document::NodeId,
        natural_size: NaturalSize,
    ) {
        let changed = {
            let node = self
                .arenas_mut()
                .get_mut(id)
                .expect("stale NodeId passed to Document::set_natural_size");
            assert!(
                node.is_element(),
                "non-element NodeId passed to Document::set_natural_size"
            );
            node.set_natural_size(natural_size)
        };
        if changed {
            self.invalidate_layout(id);
        }
    }

    #[must_use]
    pub(crate) fn natural_size(&self, id: crate::tree::document::NodeId) -> NaturalSize {
        self.get(id)
            .map_or(NaturalSize::NONE, crate::tree::node::Node::natural_size)
    }

    pub fn set_picture(
        &mut self,
        id: crate::tree::document::NodeId,
        picture: Option<(PictureKey, Option<(u32, u32)>)>,
    ) {
        let value = picture.map(|(picture, natural)| {
            (
                picture,
                natural.map_or(NaturalSize::NONE, |(width, height)| {
                    natural_size(width, height)
                }),
            )
        });
        let (was_replaced, natural_before, key_changed) = {
            let node = self
                .get(id)
                .expect("stale NodeId passed to Document::set_picture");
            assert!(
                node.is_element(),
                "non-element NodeId passed to Document::set_picture"
            );
            let held = node.picture();
            let key_changed = value.as_ref().map(|(picture, _)| picture) != held;
            let unchanged = match &value {
                Some((_, natural)) => !key_changed && *natural == node.natural_size(),
                None => held.is_none(),
            };
            if unchanged {
                return;
            }
            (node.is_replaced(), node.natural_size(), key_changed)
        };
        let value = value.map(|(picture, natural)| match picture {
            PictureKey::Url(url) if key_changed => {
                (PictureKey::Url(self.images.bind(&url)), natural)
            }
            PictureKey::Url(_) => (
                self.get(id)
                    .and_then(crate::tree::node::Node::picture)
                    .cloned()
                    .expect("an unchanged key is the one the box holds"),
                natural,
            ),
            picture @ PictureKey::Id(_) => (picture, natural),
        });
        let previous = self
            .arenas_mut()
            .get_mut(id)
            .expect("the node was live a moment ago")
            .set_picture(value);
        if key_changed && let Some(PictureKey::Url(previous)) = previous {
            self.images.unbind(&previous);
            self.forget_animated_image(&previous);
        }
        let node = self.get(id).expect("the node was live a moment ago");
        let (is_replaced, natural_after) = (node.is_replaced(), node.natural_size());
        if natural_before != natural_after && is_replaced {
            self.invalidate_layout(id);
        }
        if was_replaced == is_replaced {
            self.note_visual_mutation();
        } else {
            self.invalidate_layout(id);
        }
    }

    #[must_use]
    pub fn picture(&self, id: crate::tree::document::NodeId) -> Option<&PictureKey> {
        self.get(id)?.picture()
    }

    pub fn request_image(&mut self, url: &str) -> ImageStatus {
        self.images.request(url).1
    }

    #[must_use]
    pub fn image_status(&self, url: &str) -> ImageStatus {
        self.images.status(url)
    }

    pub fn insert_image_document(
        &mut self,
        bytes: &[u8],
        kind: crate::render::image::DocumentKind,
    ) -> std::sync::Arc<str> {
        self.images.insert_document(bytes, kind)
    }

    #[doc(hidden)]
    #[must_use]
    pub fn knows_image_source(&self, source: &str) -> bool {
        self.images.knows(source)
    }

    #[cfg(feature = "layout-test-utils")]
    #[doc(hidden)]
    pub fn set_leaf_metrics_for_testing(
        &mut self,
        id: crate::tree::document::NodeId,
        size: Size<f32>,
        first_baseline: Option<f32>,
    ) {
        let node = self
            .arenas_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::set_leaf_metrics_for_testing");
        assert!(
            node.is_element(),
            "non-element NodeId passed to Document::set_leaf_metrics_for_testing"
        );
        node.set_test_leaf_metrics(LeafMetrics::new(size).with_first_baseline(first_baseline));
        self.invalidate_layout(id);
    }

    pub fn adopt_text_context(&mut self, context: TextContext) {
        self.layout_state_mut().text_context = Some(Box::new(context));
        self.invalidate_layout_all();
    }

    pub fn text_context_mut(&mut self) -> &mut TextContext {
        self.layout_state_mut()
            .text_context
            .get_or_insert_with(|| Box::new(TextContext::new()))
    }

    pub fn register_fonts(&mut self, data: FontBlob) -> usize {
        let context = self
            .layout_state_mut()
            .text_context
            .get_or_insert_with(|| Box::new(TextContext::new()));
        let registered = context.register_fonts(data);
        if registered != 0 {
            self.invalidate_layout_all();
        }
        registered
    }

    pub fn take_font_face_requests(&mut self) -> Vec<crate::style::engine::FontFaceRequest> {
        self.style_engine_mut().take_font_face_requests()
    }

    pub fn register_font_face(&mut self, family: &str, data: FontBlob) -> usize {
        let context = self
            .layout_state_mut()
            .text_context
            .get_or_insert_with(|| Box::new(TextContext::new()));
        let registered = context.register_font_face(family, data);
        if registered != 0 {
            self.invalidate_layout_all();
        }
        registered
    }

    pub fn set_default_font_family(&mut self, family: &str) -> bool {
        let context = self
            .layout_state_mut()
            .text_context
            .get_or_insert_with(|| Box::new(TextContext::new()));
        let configured = context.set_default_font_family(family);
        if configured {
            self.invalidate_layout_all();
        }
        configured
    }

    #[must_use]
    pub fn rounded_layout(&self, id: crate::tree::document::NodeId) -> Option<&Layout> {
        let slot = self.slot(id)?;
        Some(&self.layout_state().get(slot)?.slot.rounded)
    }

    #[must_use]
    pub fn bounding_client_rect(&self, id: crate::tree::document::NodeId) -> Option<Rect<f32>> {
        let node = self.get(id)?;
        if inline_flow::is_inline_box(node) {
            return self.inline_box_client_rect(id);
        }
        let style = StyleView::try_of(node)?;
        if matches!(
            style.display_mode(),
            DisplayMode::None | DisplayMode::Contents
        ) {
            return None;
        }
        let layout = self.rounded_layout(id)?;
        let size = Size2D::new(layout.size.width, layout.size.height);
        let mut origin = Point2D::new(layout.location.x, layout.location.y);
        let mut sticky_offsets = Vec::new();
        if *style.values().get_box().get_position() == PositionProperty::Sticky {
            origin += crate::visual::sticky::live_offset(self, id, &mut sticky_offsets);
        }
        let anchored = !self.layout_state().anchored.is_empty();
        let shift = |id| {
            if anchored {
                self.default_scroll_shift(id, None).unwrap_or_default()
            } else {
                Vector2D::zero()
            }
        };
        origin += shift(id);
        let mut chain = ContainingBlockChain::new(self, node, style.values());
        for step in &mut chain {
            origin += Vector2D::new(step.layout.location.x, step.layout.location.y);
            if step.on_chain {
                if *step.style.values().get_box().get_position() == PositionProperty::Sticky {
                    origin +=
                        crate::visual::sticky::live_offset(self, step.id, &mut sticky_offsets);
                }
                if self.is_scroll_container(step.id) {
                    origin -= self.scroll_offset(step.id);
                }
                origin += shift(step.id);
            }
        }
        (chain.end() == Some(ChainEnd::DocumentElement)).then(|| Rect::new(origin, size))
    }

    #[must_use]
    pub fn generates_rendered_box(&self, id: crate::tree::document::NodeId) -> bool {
        let Some(node) = self.get(id) else {
            return false;
        };
        if inline_flow::is_inline_box(node) {
            return self.inline_box_client_rect(id).is_some();
        }
        let Some(style) = StyleView::try_of(node) else {
            return false;
        };
        if matches!(
            style.display_mode(),
            DisplayMode::None | DisplayMode::Contents
        ) || self.rounded_layout(id).is_none()
        {
            return false;
        }
        let mut chain = ContainingBlockChain::new(self, node, style.values());
        for _step in &mut chain {}
        chain.end() == Some(ChainEnd::DocumentElement)
    }

    fn inline_box_client_rect(&self, id: crate::tree::document::NodeId) -> Option<Rect<f32>> {
        let paragraph = self.paragraph_of(id)?;
        let painted = self.painted_paragraph(paragraph)?;
        let mut union: Option<Rect<f32>> = None;
        for fragment in painted.fragments() {
            if painted.sources.get(fragment.item as usize) != Some(&id) {
                continue;
            }
            let rect = Rect::new(
                Point2D::new(fragment.rect.origin.x, fragment.rect.origin.y),
                Size2D::new(fragment.rect.size.width, fragment.rect.size.height),
            );
            union = Some(union.map_or(rect, |seen| seen.union(&rect)));
        }
        let union = union?;
        let block = self.bounding_client_rect(paragraph)?;
        let inset = self.rounded_layout(paragraph)?;
        let content = Vector2D::new(
            inset.border.left + inset.padding.left,
            inset.border.top + inset.padding.top,
        );
        Some(union.translate(block.origin.to_vector() + content))
    }

    #[must_use]
    pub fn text_block_size(
        &self,
        id: crate::tree::document::NodeId,
    ) -> Option<seldon::geometry::Size<f32>> {
        self.committed_paragraph(id)?.committed_size()
    }

    #[must_use]
    pub fn paragraph_kind(&self, id: crate::tree::document::NodeId) -> Option<ParagraphKind> {
        Some(match self.committed_paragraph(id)? {
            paragraph::TextStore::Block(_) => ParagraphKind::Lynx,
            paragraph::TextStore::Flow(_) => ParagraphKind::InlineFormattingContext,
        })
    }

    #[must_use]
    pub fn paragraph_line_count(&self, id: crate::tree::document::NodeId) -> Option<usize> {
        Some(self.committed_paragraph(id)?.painted()?.line_count)
    }

    #[must_use]
    pub fn text_lines(&self, id: crate::tree::document::NodeId) -> Option<Vec<TextLineReport>> {
        let block = self.committed_paragraph(id)?.as_block()?.committed()?;
        Some(
            block
                .lines()
                .iter()
                .map(|line| TextLineReport {
                    start: line.source_start,
                    end: line.source_end,
                    ellipsis_count: line.ellipsis_count,
                })
                .collect(),
        )
    }

    pub fn set_text_layout_reporting(&mut self, id: crate::tree::document::NodeId, on: bool) {
        let node = self
            .get(id)
            .expect("stale NodeId passed to Document::set_text_layout_reporting");
        assert!(
            node.is_lynx_text(),
            "non-Lynx-text NodeId passed to Document::set_text_layout_reporting"
        );
        let state = self.layout_state_mut();
        if on {
            state.text_layout_reporters.insert(id);
        } else if state.text_layout_reporters.remove(&id) && !state.text_layout_reports.is_empty() {
            state.text_layout_reports.retain(|node| *node != id);
        }
    }

    #[must_use]
    pub fn take_text_layout_reports(&mut self) -> Vec<crate::tree::document::NodeId> {
        let pending = &mut self.layout_state_mut().text_layout_reports;
        if pending.is_empty() {
            return Vec::new();
        }
        let mut reports = std::mem::take(pending);
        if reports.len() > 1 {
            let mut seen =
                FxHashSet::with_capacity_and_hasher(reports.len(), rustc_hash::FxBuildHasher);
            reports.retain(|id| seen.insert(*id));
        }
        reports
    }

    #[must_use]
    pub fn text_range_boxes(
        &self,
        id: crate::tree::document::NodeId,
        start: u32,
        end: u32,
    ) -> Option<Vec<Rect<f32>>> {
        let boxes = self.committed_paragraph(id)?.range_boxes(start, end)?;
        Some(
            boxes
                .into_iter()
                .map(|rect| {
                    Rect::new(
                        Point2D::new(rect.origin.x, rect.origin.y),
                        Size2D::new(rect.size.width, rect.size.height),
                    )
                })
                .collect(),
        )
    }

    #[must_use]
    pub fn text_index_at_point(
        &self,
        paragraph: crate::tree::document::NodeId,
        point: Point2D<f32>,
    ) -> Option<usize> {
        let unit = self
            .committed_paragraph(paragraph)?
            .index_at_point(seldon::geometry::Point::new(point.x, point.y))?;
        usize::try_from(unit).ok()
    }

    #[must_use]
    pub fn inline_source_range(
        &self,
        element: crate::tree::document::NodeId,
    ) -> Option<core::ops::Range<usize>> {
        let slot = self.slot(element)?;
        let units = paragraph::inline_source_range(self.arenas(), self.layout_state(), slot)?;
        Some(usize::try_from(units.start).ok()?..usize::try_from(units.end).ok()?)
    }

    #[must_use]
    pub fn paragraph_of(
        &self,
        node: crate::tree::document::NodeId,
    ) -> Option<crate::tree::document::NodeId> {
        let slot = self.slot(node)?;
        paragraph::paragraph_of(self.arenas(), self.layout_state(), slot)
    }

    pub(crate) fn committed_paragraph(
        &self,
        id: crate::tree::document::NodeId,
    ) -> Option<&paragraph::TextStore> {
        let node = self.get(id)?;
        if inline_flow::is_inline_box(node) {
            return None;
        }
        let slot = self.slot(id)?;
        self.layout_state()
            .get(slot)?
            .text
            .as_deref()
            .filter(|store| store.has_committed())
    }

    pub(crate) fn paragraph_source_text(&self, paragraph: crate::link::NodeId) -> Option<String> {
        let store = self.committed_paragraph(paragraph)?;
        let slot = self.slot(paragraph)?;
        Some(match store {
            paragraph::TextStore::Block(_) => text_block::source_text(self.arenas(), slot),
            paragraph::TextStore::Flow(_) => inline_flow::source_text(self.arenas(), slot),
        })
    }

    #[must_use]
    pub fn text_content_offset(&self, paragraph: crate::link::NodeId, unit: u32) -> Option<u32> {
        let view = self.committed_paragraph(paragraph)?.editable()?;
        (unit <= view.map.source_len()).then(|| unit - view.map.boxes_before(unit))
    }

    #[cfg(test)]
    pub(crate) fn text_block(
        &self,
        id: crate::tree::document::NodeId,
    ) -> Option<&seldon::text::block::TextBlock> {
        let slot = self.slot(id)?;
        self.layout_state()
            .get(slot)?
            .text
            .as_deref()?
            .as_block()?
            .committed()
    }

    #[must_use]
    pub(crate) fn painted_paragraph(
        &self,
        id: crate::tree::document::NodeId,
    ) -> Option<paragraph::PaintedParagraph<'_>> {
        let slot = self.slot(id)?;
        self.layout_state().get(slot)?.text.as_deref()?.painted()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn inline_flow(
        &self,
        id: crate::tree::document::NodeId,
    ) -> Option<&seldon::text::inline::InlineFlow> {
        let slot = self.slot(id)?;
        self.layout_state()
            .get(slot)?
            .text
            .as_deref()?
            .as_flow()?
            .committed()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn text_block_rebuilds(&self, id: crate::tree::document::NodeId) -> Option<u32> {
        let slot = self.slot(id)?;
        Some(self.layout_state().get(slot)?.text.as_deref()?.rebuilds())
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn text_block_is_probe_dirty(&self, id: crate::tree::document::NodeId) -> bool {
        self.slot(id)
            .and_then(|slot| self.layout_state().get(slot))
            .and_then(|state| state.text.as_deref())
            .is_some_and(paragraph::TextStore::is_probe_dirty)
    }

    #[must_use]
    pub(crate) fn paint_style(&self, id: crate::tree::document::NodeId) -> Option<&ComputedValues> {
        self.get(id)?.layout_computed_style()
    }

    #[must_use]
    pub(crate) fn sticky_containing_block(
        &self,
        id: crate::tree::document::NodeId,
    ) -> Option<Edges<f32>> {
        let slot = self.slot(id)?;
        self.layout_state()
            .sticky_containing_blocks
            .iter()
            .find(|entry| entry.node == slot)
            .and_then(|entry| entry.rounded)
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn layout_cache_is_empty(&self, id: crate::tree::document::NodeId) -> Option<bool> {
        let slot = self.slot(id)?;
        Some(
            self.layout_state()
                .get(slot)
                .is_none_or(|state| state.slot.layout_cache_is_empty()),
        )
    }

    pub(crate) fn invalidate_layout(&mut self, id: crate::tree::document::NodeId) {
        let id = if let Some(paragraph) = self.paragraph_container(id) {
            if paragraph != id
                && let Some(slot) = self.slot(id)
            {
                self.layout_state_mut().clear_box_cache(slot);
            }
            paragraph
        } else {
            id
        };
        let (pending, reached_root) = {
            let (tree, state) = self.layout_parts();
            let slot = tree
                .slot(id)
                .expect("stale NodeId passed to Document::invalidate_layout");
            let start = tree.at(slot);
            state.clear_box_cache(slot);

            let mut pending = None;
            let mut reached_root = true;
            let mut top = id;
            let mut current = start.flat_parent_id();
            while let Some(node_id) = current {
                top = node_id;
                let node_slot = tree.live_slot(node_id);
                let node = tree.at(node_slot);
                let style_view = node.is_element().then(|| StyleView::of(node));
                if style_view
                    .as_ref()
                    .is_some_and(|style| style.display().is_contents())
                    || inline_flow::is_inline_box(node)
                {
                    state.clear_box_cache(node_slot);
                    current = node.flat_parent_id();
                    continue;
                }
                let node_state = state.get(node_slot).map(|state| &state.slot);
                if node_state.is_none_or(LayoutSlot::layout_cache_is_empty)
                    && node.flat_parent_id().is_some()
                {
                    reached_root = false;
                    break;
                }
                let skips_contents = style_view.as_ref().is_some_and(CoreStyle::skips_contents);
                let scheduled = if style_view.as_ref().is_some_and(is_relayout_boundary) {
                    node_state
                        .and_then(LayoutSlot::committed_input)
                        .map(|input| (node_id, input, RelayoutKind::Boundary))
                } else if node.is_element()
                    && node_id != crate::tree::document::DOCUMENT_ELEMENT_NODE_ID
                {
                    node_state.and_then(LayoutSlot::committed_independent).map(
                        |(input, previous)| (node_id, input, RelayoutKind::InPlace { previous }),
                    )
                } else {
                    None
                };
                state.clear_box_cache(node_slot);
                if let Some(entry) = scheduled {
                    pending = Some(entry);
                    reached_root = false;
                    break;
                }
                if skips_contents {
                    reached_root = false;
                    break;
                }
                current = node.flat_parent_id();
            }
            reached_root &= top == crate::tree::document::DOCUMENT_NODE_ID;
            (pending, reached_root)
        };
        self.mark_layout_dirty(reached_root);
        if let Some((root_id, committed_input, kind)) = pending {
            self.record_relayout_root(root_id, committed_input, kind);
        }
    }

    pub(crate) fn invalidate_containing_block(
        &mut self,
        id: crate::tree::document::NodeId,
    ) -> bool {
        let mut positioned = Vec::new();
        let mut stack = vec![(id, false)];
        while let Some((current, absolute_held)) = stack.pop() {
            let Some(node) = self.get(current) else {
                continue;
            };
            for &slot in node.flat_children() {
                let child = self.arenas().at(slot);
                let Some(style) = child.layout_computed_style() else {
                    continue;
                };
                if display_mode(child, *style.get_display()) == DisplayMode::None {
                    continue;
                }
                match *style.get_box().get_position() {
                    PositionProperty::Fixed => positioned.push(child.id()),
                    PositionProperty::Absolute if !absolute_held => positioned.push(child.id()),
                    _ => {}
                }
                if !establishes_fixed_containing_block(child, style) {
                    stack.push((
                        child.id(),
                        absolute_held || establishes_absolute_containing_block(child, style),
                    ));
                }
            }
        }
        let anchored = &mut self.layout_state_mut().anchored;
        for descendant in &positioned {
            if let Some(entry) = anchored.get_mut(descendant)
                && entry.last_successful.is_some()
            {
                entry.fallback_sensitive = true;
            }
        }
        for &descendant in &positioned {
            self.invalidate_layout(descendant);
        }
        !positioned.is_empty()
    }

    fn paragraph_container(
        &self,
        id: crate::tree::document::NodeId,
    ) -> Option<crate::tree::document::NodeId> {
        if let Some(anonymous) = self.arenas().anonymous().box_of(id) {
            return Some(anonymous);
        }
        let start = self.get(id)?;
        let own = start
            .layout_computed_style()
            .map(|style| display_mode(start, *style.get_display()));
        if own == Some(DisplayMode::BlockFlow) && !inline_flow::is_inline_box(start) {
            return Some(id);
        }
        let mut paragraph = (own == Some(DisplayMode::Text)).then_some(id);
        let mut current = start.flat_parent();
        while let Some(node) = current {
            let Some(style) = node.layout_computed_style() else {
                break;
            };
            match display_mode(node, *style.get_display()) {
                DisplayMode::Text => paragraph = Some(node.id()),
                DisplayMode::Contents => {}
                DisplayMode::BlockFlow if inline_flow::is_inline_box(node) => {}
                DisplayMode::BlockFlow => return Some(paragraph.unwrap_or_else(|| node.id())),
                _ => break,
            }
            current = node.flat_parent();
        }
        paragraph
    }

    pub(crate) fn invalidate_layout_all(&mut self) {
        for NodeLayoutState { slot, text } in self.layout_data_mut() {
            slot.clear_layout_cache();
            *text = None;
        }
        self.clear_relayout_roots();
        self.mark_layout_dirty(true);
    }

    #[cfg(feature = "layout-test-utils")]
    #[doc(hidden)]
    pub fn invalidate_layout_for_testing(&mut self, id: crate::tree::document::NodeId) {
        self.invalidate_layout(id);
    }
}

pub(crate) struct ContainingBlockChain<'a, T> {
    document: &'a Document<T>,
    current: &'a crate::tree::node::Node<T>,
    top: crate::tree::document::NodeId,
    escape: PositionProperty,
    escaped: bool,
    end: Option<ChainEnd>,
}

pub(crate) struct ChainStep<'a, T> {
    pub(crate) node: &'a crate::tree::node::Node<T>,
    pub(crate) id: crate::tree::document::NodeId,
    pub(crate) style: StyleView<'a, T>,
    pub(crate) layout: &'a Layout,
    pub(crate) on_chain: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChainEnd {
    DocumentElement,
    Detached,
    Hidden,
}

impl<'a, T> ContainingBlockChain<'a, T> {
    pub(crate) fn new(
        document: &'a Document<T>,
        node: &'a crate::tree::node::Node<T>,
        style: &ComputedValues,
    ) -> Self {
        let id = node.id();
        Self {
            document,
            current: node,
            top: id,
            escape: *style.get_box().get_position(),
            escaped: document.arenas().top_layer().places_against_viewport(id),
            end: None,
        }
    }

    pub(crate) fn end(&self) -> Option<ChainEnd> {
        self.end
    }
}

impl<'a, T> Iterator for ContainingBlockChain<'a, T> {
    type Item = ChainStep<'a, T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.end.is_some() {
            return None;
        }
        let Some(ancestor) = box_parent(self.current) else {
            self.end = Some(if self.top == DOCUMENT_ELEMENT_NODE_ID {
                ChainEnd::DocumentElement
            } else {
                ChainEnd::Detached
            });
            return None;
        };
        let style = StyleView::of(ancestor);
        if style.display_mode() == DisplayMode::None {
            self.end = Some(ChainEnd::Hidden);
            return None;
        }
        let id = ancestor.id();
        let Some(layout) = self.document.rounded_layout(id) else {
            self.end = Some(ChainEnd::Hidden);
            return None;
        };
        let on_chain = !self.escaped
            && match self.escape {
                PositionProperty::Absolute => {
                    establishes_absolute_containing_block(ancestor, style.values())
                }
                PositionProperty::Fixed => {
                    establishes_fixed_containing_block(ancestor, style.values())
                }
                PositionProperty::Static
                | PositionProperty::Relative
                | PositionProperty::Sticky => true,
            };
        if on_chain {
            self.escape = *style.values().get_box().get_position();
        }
        let top_layer = self.document.arenas().top_layer();
        self.escaped |= top_layer.places_against_viewport(id);
        self.current = ancestor;
        self.top = id;
        Some(ChainStep {
            node: ancestor,
            id,
            style,
            layout,
            on_chain,
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::mem::size_of;

    use seldon::tree::{LayoutInput, LayoutOutput, LayoutSlot};
    use stylo::invalidation::element::restyle_hints::RestyleHint;

    use super::*;
    use crate::style::engine::StylesheetOrigin;
    use crate::tree::document::DOCUMENT_NODE_ID;

    #[test]
    fn layout_state_size_probe() {
        const PRE_SPLIT_NODE_SIZE: usize = 368;
        const PRE_SPLIT_ATOMIC_LAYOUT_DATA_SIZE: usize = 456;
        const PRE_SPLIT_ATOMIC_LAYOUT_RESULTS_SIZE: usize = 160;
        let current = (
            size_of::<crate::tree::node::Node<()>>(),
            size_of::<LayoutSlot>(),
            size_of::<NodeLayoutState>(),
        );
        eprintln!(
            "current: node={} layout_slot={} node_layout_state={}; \
             pre-static-split baseline: node={} atomic_layout_data={} \
             atomic_layout_results={}",
            current.0,
            current.1,
            current.2,
            PRE_SPLIT_NODE_SIZE,
            PRE_SPLIT_ATOMIC_LAYOUT_DATA_SIZE,
            PRE_SPLIT_ATOMIC_LAYOUT_RESULTS_SIZE,
        );
        #[cfg(target_pointer_width = "64")]
        assert_eq!(
            current,
            (if cfg!(debug_assertions) { 232 } else { 224 }, 336, 344),
            "Node, LayoutSlot and NodeLayoutState sizes changed",
        );
    }

    fn query_container_document(item: &str) -> (Document<()>, crate::tree::document::NodeId) {
        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        document.add_stylesheet(
            &format!(
                "page {{ display: flex; align-items: flex-start;
                         width: 400px; height: 300px; }}
                 .query {{ display: flex; container-type: size;
                           width: 200px; height: 100px; }}
                 .item {{ display: flex; {item} }}"
            ),
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let query = document.create_element("view", ());
        document.add_class(query, "query");
        document.append_child(root, query);
        let item = document.create_element("view", ());
        document.add_class(item, "item");
        document.append_child(query, item);
        (document, item)
    }

    #[test]
    fn a_query_container_without_container_units_never_recascades() {
        let (mut document, item) = query_container_document("width: 10px; height: 10px;");
        document.layout();

        assert!(!document.arenas().uses_container_units());
        assert!(
            !document.document_element().needs_style_flush(),
            "nothing under the container reads its size"
        );
        assert_eq!(document.rounded_layout(item).unwrap().size.width, 10.0);
    }

    #[test]
    fn a_settled_query_container_leaves_no_pass_owing() {
        let (mut document, item) = query_container_document("width: 25cqw; height: 50cqh;");
        document.layout();

        assert!(document.arenas().uses_container_units());
        assert_eq!(document.rounded_layout(item).unwrap().size.width, 50.0);
        assert_eq!(document.rounded_layout(item).unwrap().size.height, 50.0);
        assert!(
            !document.document_element().needs_style_flush(),
            "the loop ran to a fixed point inside the one call"
        );
        assert_eq!(
            document.layout_cache_is_empty(item),
            Some(false),
            "and left the boxes it laid out valid"
        );
    }

    fn query_document(
        rules: &str,
    ) -> (
        Document<()>,
        crate::tree::document::NodeId,
        crate::tree::document::NodeId,
    ) {
        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        document.add_stylesheet(
            &format!(
                "page {{ display: flex; align-items: flex-start;
                         width: 400px; height: 300px; }}
                 .item {{ display: flex; flex-shrink: 0; }}
                 {rules}"
            ),
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let query = document.create_element("view", ());
        document.add_class(query, "query");
        document.append_child(root, query);
        let item = document.create_element("view", ());
        document.add_class(item, "item");
        document.append_child(query, item);
        (document, query, item)
    }

    fn laid_out_size(document: &Document<()>, id: crate::tree::document::NodeId) -> (f32, f32) {
        let size = document
            .rounded_layout(id)
            .expect("the box was laid out")
            .size;
        (size.width, size.height)
    }

    #[test]
    fn one_run_settles_a_size_query_container() {
        for (case, rules, size, runs) in [
            (
                "the nearest size query container",
                ".query { display: flex; container-type: size;
                          width: 200px; height: 100px; }
                 .item { width: 50cqw; height: 50cqh; }",
                (100.0, 50.0),
                1,
            ),
            (
                "the container's content box, padding and borders excluded",
                ".query { display: flex; container-type: size; box-sizing: content-box;
                          width: 200px; height: 100px;
                          padding: 10px; border: 5px solid black; }
                 .item { width: 100cqw; height: 100cqh; }",
                (200.0, 100.0),
                1,
            ),
            (
                "an inline-size container, which the interleave declines",
                ".query { display: flex; container-type: inline-size;
                          width: 200px; height: 100px; }
                 .item { width: 50cqw; height: 50cqh; }",
                (100.0, 300.0),
                2,
            ),
        ] {
            let (mut document, _query, item) = query_document(rules);
            assert_eq!(
                host::layout_runs_during(|| document.layout()),
                runs,
                "{case}: layout runs",
            );
            assert_eq!(laid_out_size(&document, item), size, "{case}: the reader");
            assert!(
                !document.document_element().needs_style_flush(),
                "{case}: and nothing is left owing",
            );
        }
    }

    #[test]
    fn a_container_that_moves_settles_in_one_run() {
        let (mut document, query, item) = query_document(
            ".query { display: flex; container-type: size; width: 200px; height: 100px; }
             .item { width: 50cqw; height: 50cqh; }",
        );
        document.layout();
        assert_eq!(laid_out_size(&document, item), (100.0, 50.0));

        document.set_inline_style(query, "width: 100px");
        assert_eq!(host::layout_runs_during(|| document.layout()), 1);
        assert_eq!(laid_out_size(&document, item), (50.0, 50.0));

        document.set_inline_style(query, "container-type: normal");
        assert_eq!(host::layout_runs_during(|| document.layout()), 1);
        assert_eq!(
            laid_out_size(&document, item),
            (400.0, 300.0),
            "half the 800x600 viewport in each axis",
        );
    }

    #[test]
    fn nested_containers_settle_in_one_run() {
        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; align-items: flex-start;
                    width: 400px; height: 300px; }
             .outer { display: flex; container-type: size;
                      width: 400px; height: 200px; }
             .inner { display: flex; container-type: size;
                      width: 50cqw; height: 50cqh; }
             .item { display: flex; width: 50cqw; height: 50cqh; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let outer = document.create_element("view", ());
        document.add_class(outer, "outer");
        document.append_child(root, outer);
        let inner = document.create_element("view", ());
        document.add_class(inner, "inner");
        document.append_child(outer, inner);
        let item = document.create_element("view", ());
        document.add_class(item, "item");
        document.append_child(inner, item);

        assert_eq!(host::layout_runs_during(|| document.layout()), 1);
        assert_eq!(
            laid_out_size(&document, inner),
            (200.0, 100.0),
            "half the outer container in each axis",
        );
        assert_eq!(
            laid_out_size(&document, item),
            (100.0, 50.0),
            "and half the inner one",
        );
        assert!(!document.document_element().needs_style_flush());
    }

    #[test]
    fn a_published_size_no_run_produced_is_corrected_by_the_next_one() {
        use committed_box::{CommittedBox, ContainerSize, RememberedSize};

        let (mut document, query, item) = query_document(
            ".query { display: flex; container-type: size; width: 200px; height: 100px; }
             .item { width: 50cqw; height: 50cqh; }",
        );
        document.layout();
        assert_eq!(laid_out_size(&document, item), (100.0, 50.0));

        document.arenas().note_committed_box(
            query,
            CommittedBox {
                remembered: RememberedSize::default(),
                container: ContainerSize {
                    width: Some(50.0),
                    height: Some(50.0),
                },
            },
        );
        let mut resized = Vec::new();
        document.arenas_mut().publish_committed_boxes(&mut resized);
        assert_eq!(resized, vec![query], "the staged size is a move");
        assert!(document.recascade_resized_containers(&mut resized, false));
        assert_eq!(laid_out_size(&document, item), (100.0, 50.0));

        assert_eq!(host::layout_runs_during(|| document.layout()), 1);
        assert_eq!(
            laid_out_size(&document, item),
            (100.0, 50.0),
            "the reader is back on the container's real content box",
        );
        assert!(!document.document_element().needs_style_flush());
    }

    const AHEM: &[u8] = include_bytes!("../../../seldon/tests/fixtures/Ahem.ttf");

    #[test]
    fn a_lynx_text_element_is_a_paragraph_whatever_its_display_computes_to() {
        use stylo::values::computed::Display;

        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        crate::test_common::define_lynx_text(&mut document, "x-para");
        document.add_stylesheet(
            "page { display: flex; align-items: flex-start;
                    width: 400px; height: 300px; font-family: Ahem; font-size: 10px; }
             x-para { display: grid; }",
            StylesheetOrigin::Author,
        );
        assert_eq!(document.register_fonts(FontBlob::from_static(AHEM)), 1);
        let root = document.document_element().id();
        let paragraph = document.create_element("x-para", ());
        document.append_child(root, paragraph);
        let run = document.create_text_node("ab", ());
        document.append_child(paragraph, run);
        let display = |document: &Document<()>| {
            document
                .get(paragraph)
                .and_then(crate::tree::node::Node::layout_computed_style)
                .map(|style| *style.get_display())
        };

        document.layout();
        assert_eq!(display(&document), Some(Display::Grid));
        assert_eq!(
            document.text_block_size(paragraph),
            Some(Size::new(20.0, 10.0)),
            "two Ahem glyphs of 10px, laid out as a Lynx paragraph"
        );

        document.set_inline_style(paragraph, "display: none");
        document.layout();
        assert_eq!(document.bounding_client_rect(paragraph), None);

        document.set_inline_style(paragraph, "display: contents");
        document.layout();
        assert_eq!(display(&document), Some(Display::Contents));
        assert_eq!(document.bounding_client_rect(paragraph), None);
    }

    #[test]
    fn a_text_reader_under_a_container_is_shaped_once_at_the_container_size() {
        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        crate::test_common::define_lynx_text(&mut document, "text");
        document.add_stylesheet(
            "page { display: flex; align-items: flex-start;
                    width: 400px; height: 300px; font-family: Ahem; }
             .query { display: flex; container-type: size;
                      width: 200px; height: 100px; }
             .label { font-size: 10cqw; }",
            StylesheetOrigin::Author,
        );
        assert_eq!(document.register_fonts(FontBlob::from_static(AHEM)), 1);
        let root = document.document_element().id();
        let query = document.create_element("view", ());
        document.add_class(query, "query");
        document.append_child(root, query);
        let label = document.create_element("text", ());
        document.add_class(label, "label");
        document.append_child(query, label);
        let run = document.create_text_node("hello", ());
        document.append_child(label, run);

        assert_eq!(host::layout_runs_during(|| document.layout()), 1);
        assert_eq!(
            document
                .text_block_size(label)
                .expect("a committed paragraph"),
            Size::new(100.0, 20.0),
            "10cqw of the 200px container is a 20px font, and Ahem is one em \
             per glyph",
        );
        assert_eq!(
            document.text_block_rebuilds(label),
            Some(1),
            "the paragraph was never shaped at the viewport fallback",
        );
    }

    #[test]
    fn a_text_query_container_is_settled_by_the_loop() {
        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        crate::test_common::define_lynx_text(&mut document, "text");
        document.add_stylesheet(
            "page { display: flex; align-items: flex-start;
                    width: 400px; height: 300px; font-family: Ahem; font-size: 16px; }
             .query { container-type: size; width: 200px; line-height: 20px; }
             .item { display: flex; width: 50cqw; height: 5px; }",
            StylesheetOrigin::Author,
        );
        assert_eq!(document.register_fonts(FontBlob::from_static(AHEM)), 1);
        let root = document.document_element().id();
        let query = document.create_element("text", ());
        document.add_class(query, "query");
        document.append_child(root, query);
        let run = document.create_text_node("hi", ());
        document.append_child(query, run);
        let item = document.create_element("view", ());
        document.add_class(item, "item");
        document.append_child(query, item);

        assert_eq!(
            host::layout_runs_during(|| document.layout()),
            2,
            "the first run publishes the paragraph's content box, the second \
             lays out what re-resolved against it",
        );
        assert_eq!(
            document
                .rounded_layout(item)
                .expect("an atomic inline")
                .size
                .width,
            100.0,
            "50cqw of the container's 200px content box",
        );
        assert!(!document.document_element().needs_style_flush());
    }

    const PLAIN_LEAVES: usize = 8;

    fn mixed_container_document(
        container_type: &str,
    ) -> (
        Document<()>,
        [crate::tree::document::NodeId; 2],
        crate::tree::document::NodeId,
        Vec<crate::tree::document::NodeId>,
    ) {
        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        document.add_stylesheet(
            &format!(
                "page {{ display: flex; flex-direction: column;
                         width: 400px; height: 300px; }}
                 .query {{ display: flex; container-type: {container_type};
                           width: 200px; height: 100px; }}
                 .plain {{ display: flex; width: 10px; height: 2px; }}
                 .reader {{ display: flex; width: 25cqw; height: 2px; }}"
            ),
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let mut containers = Vec::with_capacity(2);
        let mut plain = Vec::with_capacity(2 * PLAIN_LEAVES);
        let mut reader = None;
        for index in 0..2 {
            let query = document.create_element("view", ());
            document.add_class(query, "query");
            document.append_child(root, query);
            if index == 0 {
                let leaf = document.create_element("view", ());
                document.add_class(leaf, "reader");
                document.append_child(query, leaf);
                reader = Some(leaf);
            }
            for _ in 0..PLAIN_LEAVES {
                let leaf = document.create_element("view", ());
                document.add_class(leaf, "plain");
                document.append_child(query, leaf);
                plain.push(leaf);
            }
            containers.push(query);
        }
        (
            document,
            [containers[0], containers[1]],
            reader.expect("the first container holds the reader"),
            plain,
        )
    }

    fn restyle_hint(document: &Document<()>, id: crate::tree::document::NodeId) -> RestyleHint {
        document
            .live(id)
            .borrow_computed_style()
            .expect("the flush styled every element on the page")
            .hint
    }

    #[test]
    fn a_resized_container_marks_only_its_container_unit_readers() {
        let (mut document, [reading, plain_only], reader, plain) = mixed_container_document("size");

        assert_eq!(
            host::layout_runs_during(|| document.layout()),
            1,
            "both containers were settled inside the one run",
        );
        assert_eq!(document.rounded_layout(reader).unwrap().size.width, 50.0);
        assert_eq!(document.rounded_layout(plain[0]).unwrap().size.width, 10.0);
        assert!(
            !document.document_element().needs_style_flush(),
            "and the run left no restyle owing"
        );

        assert!(
            document.mark_container_units_users(reading),
            "the page has a container-unit reader under this container"
        );
        assert_eq!(
            restyle_hint(&document, reader).bits(),
            RestyleHint::RECASCADE_SELF.bits(),
            "the reader cascades again and is not matched again",
        );
        for (index, &leaf) in plain.iter().enumerate() {
            assert!(
                restyle_hint(&document, leaf).is_empty(),
                "plain leaf {index} reads nothing of its container",
            );
            assert!(
                !document.live(leaf).has_dirty_descendants(),
                "plain leaf {index} has no marked descendant either",
            );
        }
        assert!(
            document.live(reading).has_dirty_descendants(),
            "the traversal descends into the container that holds the reader"
        );
        assert!(
            !document.live(plain_only).has_dirty_descendants(),
            "and not into the one that holds none"
        );
        assert!(
            !document.mark_container_units_users(plain_only),
            "a subtree with no reader marks nothing, so it owes no pass"
        );
    }

    #[test]
    fn the_cap_iteration_leaves_a_mark_an_animation_tick_cannot_erase() {
        let (mut document, [reading, _], reader, _) = mixed_container_document("inline-size");

        let mut resized = Vec::new();
        document.layout_pass(&mut resized);
        assert!(
            document.recascade_resized_containers(&mut resized, true),
            "a resized container still owes a recascade at the cap"
        );

        let mut container_hint = restyle_hint(&document, reading);
        assert_eq!(
            container_hint.bits(),
            (RestyleHint::RESTYLE_SELF | RestyleHint::RECASCADE_DESCENDANTS).bits(),
            "the cap marks the container's whole subtree, not the readers under it",
        );
        assert!(
            restyle_hint(&document, reader).is_empty(),
            "which is why the reader itself is left unmarked here"
        );
        container_hint.remove_animation_hints();
        assert!(
            !container_hint.is_empty(),
            "and the mark is one an animation-only traversal leaves standing"
        );

        let mut in_loop = RestyleHint::RECASCADE_SELF;
        in_loop.remove_animation_hints();
        assert!(
            in_loop.is_empty(),
            "while the in-loop spelling is not — it is read before any tick can run"
        );
    }

    #[test]
    fn an_inherited_container_unit_reaches_an_unmarked_grandchild() {
        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; align-items: flex-start;
                    width: 400px; height: 300px; }
             .query { display: flex; container-type: size;
                      width: 200px; height: 100px; }
             .mid { display: flex; font-size: 10cqw; }
             .leaf { display: flex; width: 2em; height: 2px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let query = document.create_element("view", ());
        document.add_class(query, "query");
        document.append_child(root, query);
        let mid = document.create_element("view", ());
        document.add_class(mid, "mid");
        document.append_child(query, mid);
        let leaf = document.create_element("view", ());
        document.add_class(leaf, "leaf");
        document.append_child(mid, leaf);

        document.layout();

        assert_eq!(document.rounded_layout(leaf).unwrap().size.width, 40.0);
        assert!(
            !document
                .live(leaf)
                .computed_style()
                .expect("the leaf is styled")
                .flags
                .contains(stylo::computed_value_flags::ComputedValueFlags::USES_CONTAINER_UNITS),
            "the leaf resolves no container unit of its own, so nothing marks it"
        );

        document.set_inline_style(query, "width: 300px");
        document.layout();

        assert_eq!(document.rounded_layout(leaf).unwrap().size.width, 60.0);
    }

    fn label_document_parts(
        text: &str,
        width: f32,
    ) -> (
        Document<()>,
        crate::tree::document::NodeId,
        crate::tree::document::NodeId,
    ) {
        const AHEM: &[u8] = include_bytes!("../../../seldon/tests/fixtures/Ahem.ttf");

        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        crate::test_common::define_lynx_text(&mut document, "text");
        document.add_stylesheet(
            &format!(
                "page {{ display: flex; width: {width}px; height: 100px;
                         align-items: flex-start; font-family: Ahem; font-size: 16px; }}"
            ),
            StylesheetOrigin::Author,
        );
        assert_eq!(document.register_fonts(FontBlob::from_static(AHEM)), 1);
        let root = document.document_element().id();
        let label = document.create_element("text", ());
        document.add_class(label, "label");
        document.append_child(root, label);
        let run = document.create_text_node(text, ());
        document.append_child(label, run);
        (document, label, run)
    }

    #[test]
    fn a_masked_text_shows_one_mask_per_character_and_follows_the_property() {
        let (mut document, label, _run) = label_document_parts("a b\n\u{1F600}", 200.0);
        document.layout();
        let plain = document.text_block(label).expect("committed");
        assert_eq!(plain.offset_map().display_text(), "a b\n\u{1F600}");
        assert_eq!(plain.lines().len(), 2);

        for (security, mask) in [("disc", '\u{2022}'), ("square", '\u{25A0}')] {
            document.set_inline_style(label, &format!("-webkit-text-security: {security}"));
            document.layout();
            let block = document.text_block(label).expect("committed");
            let map = block.offset_map();
            assert_eq!(map.display_text(), mask.to_string().repeat(5), "{security}");
            assert_eq!(map.source_len(), 6);
            assert_eq!(map.unit_to_byte(4), Some(12));
            assert_eq!(map.unit_to_byte(5), Some(12));
            assert_eq!(block.lines().len(), 1, "{security}");
        }
        document.set_inline_style(label, "-webkit-text-security: disc");
        document.layout();
        assert_eq!(
            document.text_block_size(label).map(|size| size.width),
            Some(80.0)
        );

        let rebuilds = document.text_block_rebuilds(label).expect("a store");
        document.set_inline_style(label, "");
        document.layout();
        let block = document.text_block(label).expect("committed");
        assert_eq!(block.offset_map().display_text(), "a b\n\u{1F600}");
        assert_eq!(block.lines().len(), 2);
        assert_eq!(document.text_block_rebuilds(label), Some(rebuilds + 1));
    }

    #[test]
    fn one_pass_breaks_a_text_node_once_per_distinct_constraint() {
        for (case, text, width, lines, breaks) in [
            ("one word", "hello", 200.0, 1, 2),
            ("one line, two words", "hello world", 200.0, 1, 3),
            ("wrapped", "hello world", 100.0, 2, 4),
        ] {
            let (mut document, label, run) = label_document_parts(text, width);
            document.layout();

            let _ = run;
            let block = document
                .text_block(label)
                .expect("the pass committed a paragraph");
            assert_eq!(block.lines().len(), lines, "{case}: lines");
            assert!(
                block.break_count() <= breaks,
                "{case}: {} breaks exceeds the {breaks} distinct constraints a pass asks in",
                block.break_count()
            );
        }
    }

    #[test]
    fn a_probe_that_never_commits_is_handed_back_to_its_committed_break() {
        use seldon::tree::{AvailableSpace, LayoutInput, LayoutTree, RequestedAxis};

        let (mut document, label, _run) = label_document_parts("hello world", 200.0);
        document.layout();
        let committed = document.text_block(label).expect("committed").size();
        let lines = document.text_block(label).expect("committed").lines().len();

        let slot = document.live_slot(label);
        let (tree, state) = document.layout_parts();
        tree.compute_layout(
            state,
            slot,
            LayoutInput::measure(
                Size::NONE,
                Size::NONE,
                Size::new(AvailableSpace::Definite(37.0), AvailableSpace::MaxContent),
                RequestedAxis::Horizontal,
            ),
        );

        assert!(
            document.text_block_is_probe_dirty(label),
            "a probe at an unseen constraint moves the retained line breaks",
        );

        document.layout_state_mut().restore_probed_text();

        assert!(!document.text_block_is_probe_dirty(label));
        let restored = document.text_block(label).expect("committed");
        assert_eq!(restored.size(), committed);
        assert_eq!(restored.lines().len(), lines);
    }

    #[test]
    fn changing_paragraph_limits_rebreaks_without_reshaping() {
        let (mut document, label, _run) = label_document_parts("hello world", 100.0);
        document.add_stylesheet(
            r#"@property --lynx-text-maxline { syntax: "<integer>"; inherits: false; initial-value: 0; }
               @property --lynx-text-maxlength { syntax: "<integer>"; inherits: false; initial-value: -1; }"#,
            StylesheetOrigin::UserAgent,
        );
        document.layout();
        let before = document
            .text_block_rebuilds(label)
            .expect("shaped paragraph");
        assert_eq!(
            document.text_block(label).expect("paragraph").lines().len(),
            2
        );

        for (max_lines, max_chars, lines) in
            [(Some("1"), None, 1), (None, Some("2"), 1), (None, None, 2)]
        {
            for (name, value) in [
                ("--lynx-text-maxline", max_lines),
                ("--lynx-text-maxlength", max_chars),
            ] {
                document.set_inline_style_property(label, name, value.unwrap_or(""));
            }
            document.layout();
            let block = document.text_block(label).expect("committed paragraph");
            assert_eq!(block.lines().len(), lines, "{max_lines:?}, {max_chars:?}");
            assert_eq!(document.text_block_rebuilds(label), Some(before));
            assert!(!document.text_block_is_probe_dirty(label));

            for (name, value) in [
                ("--lynx-text-maxline", max_lines),
                ("--lynx-text-maxlength", max_chars),
            ] {
                document.set_inline_style_property(label, name, value.unwrap_or(""));
            }
            assert_eq!(
                document.layout_cache_is_empty(label),
                Some(false),
                "no-op update"
            );
        }
    }

    #[test]
    fn a_text_block_s_min_content_counts_its_indent() {
        let (mut document, label, _run) = label_document_parts("a b", 5.0);
        document.add_stylesheet(
            ".label { font-size: 10px; text-indent: 15px; }",
            StylesheetOrigin::Author,
        );
        document.layout();
        assert_eq!(document.rounded_layout(label).unwrap().size.width, 25.0);
    }

    #[test]
    fn cascaded_paragraph_limits_update_ancestor_and_following_sibling_geometry() {
        for (property, initial, limit) in [
            ("--lynx-text-maxline", "0", "1"),
            ("--lynx-text-maxlength", "-1", "2"),
        ] {
            let (mut document, label, _) = label_document_parts("hello world", 100.0);
            document.add_stylesheet(
                &format!(
                    "@property {property} {{ syntax: '<integer>'; inherits: false; initial-value: {initial}; }}"
                ),
                StylesheetOrigin::UserAgent,
            );
            document.add_stylesheet(
                &format!(
                    "page {{ flex-direction: column; line-height: 20px; }}
                     .label {{ width: 100px; }}
                     .expanded {{ {property}: initial !important; }}"
                ),
                StylesheetOrigin::Author,
            );
            let root = document.document_element().id();
            let wrapper = document.create_element("view", ());
            document.set_inline_style(
                wrapper,
                "display: flex; flex-direction: column; width: 100px; flex-shrink: 0",
            );
            document.append_child(root, wrapper);
            document.append_child(wrapper, label);
            let following = document.create_element("view", ());
            document.set_inline_style(following, "display: flex; width: 1px; height: 7px");
            document.append_child(root, following);
            document.layout();
            let rebuilds = document.text_block_rebuilds(label);

            for (stage, height) in [(0, 20.0), (1, 40.0), (2, 20.0), (3, 40.0)] {
                match stage {
                    0 => document.set_inline_style_property(label, property, limit),
                    1 => document.add_class(label, "expanded"),
                    2 => document.remove_class(label, "expanded"),
                    _ => document.set_inline_style_property(label, property, ""),
                }
                document.layout();
                assert_eq!(
                    document.rounded_layout(label).unwrap().size.height,
                    height,
                    "{property}, stage {stage}: paragraph height"
                );
                assert_eq!(
                    document.rounded_layout(wrapper).unwrap().size.height,
                    height,
                    "{property}, stage {stage}: auto-sized ancestor"
                );
                assert_eq!(
                    document.rounded_layout(following).unwrap().location.y,
                    height,
                    "{property}, stage {stage}: following sibling position"
                );
                assert_eq!(document.text_block_rebuilds(label), rebuilds);
            }
        }
    }

    #[test]
    fn interpolated_integer_paragraph_limits_relayout_without_reshaping() {
        use seldon::style::LynxTextStyle;

        for (property, initial, limit, samples) in [
            (
                "--lynx-text-maxline",
                "0",
                "4",
                [(2.5, 1, 1, 20.0), (5.0, 2, 2, 40.0), (7.5, 3, 2, 40.0)],
            ),
            (
                "--lynx-text-maxlength",
                "-1",
                "7",
                [(2.5, 1, 1, 20.0), (5.0, 3, 1, 20.0), (7.5, 5, 1, 20.0)],
            ),
        ] {
            let (mut document, label, _) = label_document_parts("hello world", 100.0);
            document.add_stylesheet(
                &format!(
                    "@property {property} {{ syntax: '<integer>'; inherits: false; initial-value: {initial}; }}"
                ),
                StylesheetOrigin::UserAgent,
            );
            document.add_stylesheet(
                &format!(
                    "@keyframes limit {{ from {{ {property}: {initial}; }} to {{ {property}: {limit}; }} }}
                     .label {{ line-height: 20px; animation: limit 10s linear; }}"
                ),
                StylesheetOrigin::Author,
            );
            document.layout();
            document.advance_animations(0.0);
            document.layout();
            let rebuilds = document.text_block_rebuilds(label);
            assert_eq!(document.text_block(label).unwrap().lines().len(), 2);

            for (index, (time, count, lines, height)) in samples.into_iter().enumerate() {
                let tick = document.advance_animations(time);
                assert!(tick.relayout, "{property}: limit changed at t={time}");
                document.layout();
                let style = StyleView::of(document.get(label).unwrap());
                let effective = if property == "--lynx-text-maxline" {
                    style.text_maxline().map(core::num::NonZeroU32::get)
                } else {
                    style.text_maxlength()
                };
                assert_eq!(
                    effective,
                    Some(count),
                    "{property}: integer sample at t={time}"
                );
                assert_eq!(document.text_block(label).unwrap().lines().len(), lines);
                assert_eq!(document.rounded_layout(label).unwrap().size.height, height);
                assert_eq!(document.text_block_rebuilds(label), rebuilds);
                assert!(!document.text_block_is_probe_dirty(label));

                if index == 0 {
                    let same_integer = document.advance_animations(3.0);
                    assert!(
                        !same_integer.relayout,
                        "{property}: a later sample rounding to the same integer keeps its layout"
                    );
                }
            }

            let ended = document.advance_animations(10.5);
            assert!(
                ended.relayout,
                "{property}: finishing restores the initial limit"
            );
            document.layout();
            let style = StyleView::of(document.get(label).unwrap());
            assert_eq!(style.text_maxline(), None);
            assert_eq!(style.text_maxlength(), None);
            assert_eq!(document.text_block(label).unwrap().lines().len(), 2);
            assert_eq!(document.rounded_layout(label).unwrap().size.height, 40.0);
            assert_eq!(document.text_block_rebuilds(label), rebuilds);
        }
    }

    #[test]
    fn a_relayout_keeps_shaped_text_unless_the_shaping_inputs_moved() {
        for (case, declaration, survives) in [
            ("width", "width: 150px", true),
            ("padding", "padding: 4px", true),
            ("text-align", "text-align: center", true),
            ("text-indent", "text-indent: 8px", true),
            ("font-size", "font-size: 24px", false),
            ("font-weight", "font-weight: 700", false),
            ("letter-spacing", "letter-spacing: 2px", false),
            ("word-break", "word-break: break-all", false),
        ] {
            let (mut document, label, run) = label_document_parts("hello world", 200.0);
            document.layout();
            assert!(document.text_block(label).is_some());
            let before = document.text_block_rebuilds(label).expect("built");

            document.set_inline_style(label, declaration);
            document.flush_styles_with_damage_sink(&mut |_, _| {});
            document.layout();

            let after = document.text_block_rebuilds(label).expect("built");
            assert_eq!(
                after == before,
                survives,
                "{case}: shaped text should {} the restyle",
                if survives { "survive" } else { "not survive" },
            );
            assert!(
                document.layout_cache_is_empty(run).expect("live text node"),
                "{case}: a relayout always drops the text child's box cache",
            );
        }
    }

    #[test]
    fn a_repaint_only_restyle_never_reaches_the_text_eviction_path() {
        let (mut document, label, run) = label_document_parts("hello world", 200.0);
        document.layout();

        document.set_inline_style(label, "color: rgb(255, 0, 0)");
        document.flush_styles_with_damage_sink(&mut |_, _| {});

        assert!(
            document.text_block(label).is_some(),
            "a repaint-only change leaves the shaped paragraph alone"
        );
        let _ = run;
        assert!(
            !document.layout_cache_is_empty(label).expect("live element"),
            "a repaint-only change leaves even the box cache alone",
        );
    }

    #[test]
    fn a_re_measured_text_node_ends_the_pass_on_its_committed_break() {
        let (mut document, label, run) = label_document_parts("hello world", 200.0);
        document.layout();
        let first = document.text_block(label).expect("committed").size();

        document.invalidate_layout(run);
        document.layout();

        assert!(!document.text_block_is_probe_dirty(label));
        let committed = document
            .text_block(label)
            .expect("committed after re-measure");
        assert_eq!(committed.size(), first);
        assert_eq!(committed.lines().len(), 1);
    }

    #[test]
    fn internal_natural_size_update_invalidates_the_dirty_spine() {
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        let image = document.create_element("image", ());
        document.append_child(root, image);

        let input = LayoutInput::default();
        for id in [DOCUMENT_NODE_ID, root, image] {
            let slot = document.live_slot(id);
            document
                .layout_state_mut()
                .at_mut(slot)
                .slot
                .store_cached_layout(input, LayoutOutput::default());
        }

        let natural_size = NaturalSize::from_size(Size::new(40.0, 20.0));
        document.set_natural_size(image, natural_size);

        assert_eq!(document.get(image).unwrap().natural_size(), natural_size);
        for id in [DOCUMENT_NODE_ID, root, image] {
            assert_eq!(document.layout_cache_is_empty(id), Some(true));
        }
    }

    #[test]
    fn the_picture_that_makes_an_element_replaced_invalidates_layout() {
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        let image = document.create_element("image", ());
        document.append_child(root, image);

        let prime = |document: &mut Document<()>| {
            let input = LayoutInput::default();
            for id in [DOCUMENT_NODE_ID, root, image] {
                let slot = document.live_slot(id);
                document
                    .layout_state_mut()
                    .at_mut(slot)
                    .slot
                    .store_cached_layout(input, LayoutOutput::default());
            }
        };

        prime(&mut document);
        document.set_picture(image, Some((url("app:///a.png"), None)));
        for id in [DOCUMENT_NODE_ID, root, image] {
            assert_eq!(
                document.layout_cache_is_empty(id),
                Some(true),
                "the first picture flips the node to replaced, which layout reads",
            );
        }

        prime(&mut document);
        document.render();
        document.set_picture(image, Some((url("app:///b.png"), None)));
        assert!(
            document.needs_render(),
            "a new picture invalidates the retained frame"
        );
        for id in [DOCUMENT_NODE_ID, root, image] {
            assert_eq!(
                document.layout_cache_is_empty(id),
                Some(false),
                "swapping one image for another changes no layout input",
            );
        }
    }

    #[test]
    fn taking_a_picture_off_an_element_that_has_none_leaves_it_alone() {
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        let view = document.create_element("view", ());
        let sized = document.create_element("view", ());
        document.append_child(root, view);
        document.append_child(root, sized);
        document.set_natural_size(sized, natural_size(4, 4));
        document.layout();
        document.render();

        document.set_picture(view, None);
        document.set_picture(sized, None);

        assert!(!document.get(view).expect("live element").is_replaced());
        assert!(document.get(sized).expect("live element").is_replaced());
        assert_eq!(document.natural_size(sized), natural_size(4, 4));
        assert!(!document.needs_render(), "nothing changed");
    }

    const SRC: &str = "app:///a.png";
    const OTHER_SRC: &str = "app:///b.png";

    fn url(source: &str) -> PictureKey {
        PictureKey::Url(std::sync::Arc::from(source))
    }

    fn image_document() -> (Document<()>, crate::tree::document::NodeId) {
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        let image = document.create_element("image", ());
        document.append_child(root, image);
        (document, image)
    }

    fn loaded(source: &str, width: u32, height: u32) -> crate::render::image::ImageEvent {
        crate::render::image::ImageEvent::Loaded {
            source: std::sync::Arc::from(source),
            width,
            height,
            animated: false,
        }
    }

    fn failed(source: &str) -> crate::render::image::ImageEvent {
        crate::render::image::ImageEvent::Failed {
            source: std::sync::Arc::from(source),
        }
    }

    fn document_report(source: &str, svg: &str) -> crate::render::image::ImageEvent {
        crate::render::image::ImageEvent::parse_document(
            std::sync::Arc::from(source),
            svg.as_bytes(),
            crate::render::image::DocumentKind::Svg,
        )
    }

    fn settled(
        document: &mut Document<()>,
        events: &[crate::render::image::ImageEvent],
    ) -> Vec<(String, ImageStatus)> {
        document
            .apply_image_events(events)
            .into_iter()
            .map(|(source, status)| (source.to_string(), status))
            .collect()
    }

    #[test]
    fn a_parsed_document_settles_and_a_malformed_one_fails() {
        let (mut document, _) = image_document();
        assert_eq!(document.request_image(SRC), ImageStatus::Pending);
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="12"/>"#;

        assert_eq!(
            settled(&mut document, &[document_report(SRC, svg)]),
            [(
                SRC.to_owned(),
                ImageStatus::Vector {
                    width: 24,
                    height: 12
                }
            )]
        );
        assert!(
            settled(&mut document, &[document_report(SRC, svg)]).is_empty(),
            "one URL has one content, so the repeat is a no-op"
        );

        assert_eq!(
            settled(
                &mut document,
                &[document_report(
                    OTHER_SRC,
                    "<svg xmlns='http://www.w3.org/2000/svg'><rect></svg>",
                )]
            ),
            [(OTHER_SRC.to_owned(), ImageStatus::Failed)]
        );
        assert!(
            settled(&mut document, &[document_report(OTHER_SRC, svg)]).is_empty(),
            "a failure is terminal: well-formed bytes arriving later change nothing"
        );
        assert_eq!(document.image_status(OTHER_SRC), ImageStatus::Failed);
    }

    #[test]
    fn a_request_asks_once_and_answers_what_is_known() {
        let (mut document, _) = image_document();
        assert_eq!(document.image_status(SRC), ImageStatus::Pending);
        assert!(
            document.take_wanted_images().is_empty(),
            "a read asks nothing"
        );

        assert_eq!(document.request_image(SRC), ImageStatus::Pending);
        assert_eq!(document.request_image(SRC), ImageStatus::Pending);
        assert_eq!(
            document.take_wanted_images(),
            vec![std::sync::Arc::<str>::from(SRC)],
            "asked once however often requested"
        );

        let _ = settled(&mut document, &[loaded(SRC, 40, 20), failed(OTHER_SRC)]);
        assert_eq!(
            document.request_image(SRC),
            ImageStatus::Bitmap {
                width: 40,
                height: 20,
                animated: false,
            }
        );
        assert_eq!(document.request_image(OTHER_SRC), ImageStatus::Failed);
        assert_eq!(document.take_wanted_images(), []);
    }

    #[test]
    fn a_changed_natural_size_invalidates_layout() {
        let (mut document, image) = image_document();
        let root = document.document_element().id();
        document.set_picture(image, Some((url(SRC), Some((40, 20)))));
        assert_eq!(document.natural_size(image), natural_size(40, 20));
        document.layout();
        document.render();
        document.set_picture(image, Some((url(SRC), Some((40, 20)))));
        assert!(
            !document.needs_render(),
            "the same picture at the same size"
        );

        let input = LayoutInput::default();
        for id in [DOCUMENT_NODE_ID, root, image] {
            let slot = document.live_slot(id);
            document
                .layout_state_mut()
                .at_mut(slot)
                .slot
                .store_cached_layout(input, LayoutOutput::default());
        }
        document.set_picture(image, Some((url(OTHER_SRC), None)));
        assert_eq!(document.natural_size(image), NaturalSize::NONE);
        for id in [DOCUMENT_NODE_ID, root, image] {
            assert_eq!(
                document.layout_cache_is_empty(id),
                Some(true),
                "the element lost the size it was laid out at",
            );
        }
    }

    #[test]
    fn taking_the_picture_off_an_element_unreplaces_it() {
        let (mut document, image) = image_document();
        document.set_picture(image, Some((url(SRC), Some((40, 20)))));
        assert!(document.get(image).expect("live element").is_replaced());
        assert_eq!(document.picture(image), Some(&url(SRC)));

        document.set_picture(image, None);
        assert!(!document.get(image).expect("live element").is_replaced());
        assert_eq!(document.natural_size(image), NaturalSize::NONE);
        assert_eq!(document.picture(image), None);
    }

    #[test]
    fn presenting_a_url_asks_for_it() {
        let (mut document, image) = image_document();
        document.set_picture(image, Some((url(SRC), None)));
        assert_eq!(
            document.take_wanted_images(),
            vec![std::sync::Arc::<str>::from(SRC)]
        );
    }

    fn is_content_independent(doc: &Document<()>, id: crate::tree::document::NodeId) -> bool {
        let slot = doc.live_slot(id);
        doc.layout_state()
            .get(slot)
            .is_some_and(|state| state.slot.committed_independent().is_some())
    }

    fn linear_page() -> Document<()> {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            "page, view { overflow: hidden; box-sizing: border-box; }
             page { display: flex; flex-direction: column; width: 100%; height: 100%; }
             .list { display: linear; linear-direction: column;
                     flex-grow: 1; flex-basis: 0px; }
             .row { display: linear; linear-direction: row; height: 56px; }
             .weighted { display: linear; linear-weight: 1; }
             .measured { display: linear; }
             .ratio { display: linear; width: 40px; aspect-ratio: 2; }
             .percent-row { display: linear; linear-direction: row; height: 50%; }",
            StylesheetOrigin::Author,
        );
        doc
    }

    fn child_of(
        doc: &mut Document<()>,
        parent: crate::tree::document::NodeId,
        class: &str,
    ) -> crate::tree::document::NodeId {
        let child = doc.create_element("view", ());
        doc.add_class(child, class);
        doc.append_child(parent, child);
        child
    }

    #[test]
    fn linear_imposes_content_independent_inputs_on_pinned_and_weighted_children() {
        let mut doc = linear_page();
        let root = doc.document_element().id();
        let list = child_of(&mut doc, root, "list");
        let row = child_of(&mut doc, list, "row");
        let weighted = child_of(&mut doc, row, "weighted");
        let measured = child_of(&mut doc, row, "measured");
        let ratio = child_of(&mut doc, row, "ratio");
        let percent = child_of(&mut doc, list, "percent-row");

        doc.layout();

        assert!(is_content_independent(&doc, row));
        assert!(is_content_independent(&doc, weighted));
        assert!(is_content_independent(&doc, ratio));
        assert!(is_content_independent(&doc, percent));
        assert!(
            !is_content_independent(&doc, measured),
            "a content-measured main size cannot license an in-place relayout",
        );
    }

    #[test]
    fn grid_relative_and_out_of_flow_children_carry_their_own_independence() {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            "page, view { overflow: hidden; box-sizing: border-box; }
             page { display: flex; flex-direction: column; width: 300px; height: 300px; }
             .grid { display: grid; grid-template-columns: 100px 1fr;
                     grid-template-rows: 60px; width: 300px; height: 60px; }
             .pinned { width: 40px; }
             .relative { display: relative; width: 200px; height: 100px; }
             .sized { width: 30px; height: 30px; }
             .abs { position: absolute; left: 4px; top: 4px; width: 20px; height: 20px; }",
            StylesheetOrigin::Author,
        );
        let root = doc.document_element().id();
        let grid = child_of(&mut doc, root, "grid");
        let fixed_cell = child_of(&mut doc, grid, "");
        let pinned_cell = child_of(&mut doc, grid, "pinned");
        let relative = child_of(&mut doc, root, "relative");
        let anchored = child_of(&mut doc, relative, "sized");
        let measured = child_of(&mut doc, relative, "");
        let out_of_flow = child_of(&mut doc, relative, "abs");

        doc.layout();

        assert!(is_content_independent(&doc, fixed_cell));
        assert!(is_content_independent(&doc, pinned_cell));
        assert!(is_content_independent(&doc, anchored));
        assert!(
            is_content_independent(&doc, out_of_flow),
            "an out-of-flow box is sized from its containing block and its own \
             style, never from a measurement of itself",
        );
        assert!(
            !is_content_independent(&doc, measured),
            "a relative child with no imposed size is whatever it measures",
        );
    }

    #[test]
    fn a_grid_item_in_a_flexible_track_follows_its_own_content_without_a_pinned_size() {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            "page, view { overflow: hidden; box-sizing: border-box; }
             page { display: flex; flex-direction: column; width: 300px; height: 300px; }
             .grid { display: grid; grid-template-columns: 1fr;
                     grid-template-rows: auto; width: 300px; height: 60px; }",
            StylesheetOrigin::Author,
        );
        let root = doc.document_element().id();
        let grid = child_of(&mut doc, root, "grid");
        let cell = child_of(&mut doc, grid, "");
        doc.layout();

        assert!(
            !is_content_independent(&doc, cell),
            "an auto row sizes itself from this item, so the item's own area \
             moves with its content",
        );
    }

    #[test]
    fn a_linear_subtree_mutation_parks_instead_of_reaching_the_root() {
        let mut doc = linear_page();
        let root = doc.document_element().id();
        let list = child_of(&mut doc, root, "list");
        let row = child_of(&mut doc, list, "row");
        let cell = child_of(&mut doc, row, "weighted");
        let leaf = child_of(&mut doc, cell, "measured");
        doc.layout();

        let viewport = Size::new(800.0, 600.0);
        doc.invalidate_layout(leaf);
        assert!(
            !doc.layout_requires_full_pass(viewport, 1.0),
            "a mutation under a content-independent linear ancestor stays incremental",
        );
    }

    #[test]
    fn a_parked_boundary_under_another_parked_boundary_is_rounded_on_its_own() {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            "page { display: flex; width: 300px; height: 100px; }
             .outer { display: flex; contain: strict; width: 200px; height: 80px; }
             .wrapper { display: contents; }
             .inner { display: flex; contain: strict; width: 100px; height: 40px; }
             .leaf { width: 10px; height: 10px; }
             .top { position: fixed; top: 10px; left: 10px; width: 20px; height: 20px; }
             .hidden { display: none; }",
            StylesheetOrigin::Author,
        );
        let root = doc.document_element().id();

        let outer = doc.create_element("view", ());
        doc.add_class(outer, "outer");
        doc.append_child(root, outer);
        let wrapper = doc.create_element("view", ());
        doc.add_class(wrapper, "wrapper");
        doc.append_child(outer, wrapper);
        let inner = doc.create_element("view", ());
        doc.add_class(inner, "inner");
        doc.append_child(wrapper, inner);
        let top = doc.create_element("view", ());
        doc.set_classes(top, "top hidden");
        doc.append_child(inner, top);
        doc.add_to_top_layer(top, false);
        let leaf = doc.create_element("view", ());
        doc.add_class(leaf, "leaf");
        doc.append_child(outer, leaf);
        doc.layout();
        assert!(
            doc.bounding_client_rect(top).is_none(),
            "hidden until shown"
        );

        doc.set_classes(top, "top");
        doc.set_inline_style(leaf, "height: 20px");
        doc.flush_styles_with_damage_sink(&mut |_, _| {});
        assert!(!doc.layout_requires_full_pass(Size::new(800.0, 600.0), 1.0));
        assert_eq!(doc.relayout_roots().len(), 2, "both boundaries park");
        doc.layout();

        let placed = doc
            .bounding_client_rect(top)
            .expect("the top-layer box is placed against the viewport");
        assert_eq!(
            (
                placed.origin.x,
                placed.origin.y,
                placed.size.width,
                placed.size.height
            ),
            (10.0, 10.0, 20.0, 20.0)
        );
        let leaf_rect = doc.bounding_client_rect(leaf).expect("the leaf has a box");
        assert_eq!(
            (leaf_rect.size.width, leaf_rect.size.height),
            (10.0, 20.0),
            "the outer boundary's change"
        );
    }

    #[test]
    fn a_parked_in_place_root_under_a_wrapper_is_rounded_in_a_whole_tree_run() {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            "page { display: flex; flex-direction: column; width: 300px; height: 300px; }
             .wrapper { display: contents; }
             .inner { display: flex; flex: none; overflow: hidden; width: 100px; height: 40px; }
             .leaf { width: 10px; height: 10px; }
             .top { position: fixed; top: 10px; left: 10px; width: 20px; height: 20px; }
             .hidden { display: none; }",
            StylesheetOrigin::Author,
        );
        let root = doc.document_element().id();
        let wrapper = doc.create_element("view", ());
        doc.add_class(wrapper, "wrapper");
        doc.append_child(root, wrapper);
        let inner = doc.create_element("view", ());
        doc.add_class(inner, "inner");
        doc.append_child(wrapper, inner);
        let top = doc.create_element("view", ());
        doc.set_classes(top, "top hidden");
        doc.append_child(inner, top);
        doc.add_to_top_layer(top, false);
        let leaf = doc.create_element("view", ());
        doc.add_class(leaf, "leaf");
        doc.append_child(root, leaf);
        doc.layout();
        assert!(
            doc.bounding_client_rect(top).is_none(),
            "hidden until shown"
        );

        doc.set_classes(top, "top");
        let added = doc.create_element("view", ());
        doc.add_class(added, "leaf");
        doc.append_child(root, added);
        doc.flush_styles_with_damage_sink(&mut |_, _| {});
        assert!(
            doc.layout_requires_full_pass(Size::new(800.0, 600.0), 1.0),
            "the appended child reached the root"
        );
        assert_eq!(doc.relayout_roots().len(), 1, "the in-place root parks");
        doc.layout();

        let placed = doc
            .bounding_client_rect(top)
            .expect("the top-layer box is placed against the viewport");
        assert_eq!(
            (
                placed.origin.x,
                placed.origin.y,
                placed.size.width,
                placed.size.height
            ),
            (10.0, 10.0, 20.0, 20.0)
        );
    }

    #[test]
    fn an_empty_linear_box_that_gains_children_lays_them_out_under_its_definite_size() {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            "page { display: linear; linear-direction: column; width: 300px; height: 300px; }
             .box { display: linear; linear-direction: row; overflow-x: scroll; overflow-y: clip; contain: content; width: 100px; height: 40px; }
             .item { width: 50%; height: 100%; contain: content; }",
            StylesheetOrigin::Author,
        );
        let root = doc.document_element().id();
        let container = doc.create_element("view", ());
        doc.add_class(container, "box");
        doc.append_child(root, container);
        doc.layout();

        let item = doc.create_element("view", ());
        doc.add_class(item, "item");
        doc.append_child(container, item);
        doc.flush_styles_with_damage_sink(&mut |_, _| {});
        assert_eq!(doc.relayout_roots().len(), 1, "the box relays out in place");
        doc.layout();

        let rect = doc.bounding_client_rect(item).expect("the item has a box");
        assert_eq!((rect.size.width, rect.size.height), (50.0, 40.0));
    }

    #[test]
    fn a_parked_boundary_under_a_warm_box_is_relaid_in_a_whole_tree_run() {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            "page { display: flex; flex-direction: column; width: 300px; height: 300px; }
             .box { display: flex; flex: none; width: 200px; height: 80px; }
             .wrapper { display: contents; }
             .inner { display: flex; flex: none; contain: strict; width: 100px; height: 40px; }
             .leaf { width: 10px; height: 10px; }",
            StylesheetOrigin::Author,
        );
        let root = doc.document_element().id();
        let outer = doc.create_element("view", ());
        doc.add_class(outer, "box");
        doc.append_child(root, outer);
        let wrapper = doc.create_element("view", ());
        doc.add_class(wrapper, "wrapper");
        doc.append_child(outer, wrapper);
        let inner = doc.create_element("view", ());
        doc.add_class(inner, "inner");
        doc.append_child(wrapper, inner);
        let child = doc.create_element("view", ());
        doc.add_class(child, "leaf");
        doc.append_child(inner, child);
        let leaf = doc.create_element("view", ());
        doc.add_class(leaf, "leaf");
        doc.append_child(root, leaf);
        doc.layout();

        doc.set_inline_style(child, "height: 20px");
        let added = doc.create_element("view", ());
        doc.add_class(added, "leaf");
        doc.append_child(root, added);
        doc.flush_styles_with_damage_sink(&mut |_, _| {});
        assert!(
            doc.layout_requires_full_pass(Size::new(800.0, 600.0), 1.0),
            "the appended child reached the root"
        );
        assert_eq!(doc.relayout_roots().len(), 1, "the boundary parks");
        doc.layout();

        let rect = doc
            .bounding_client_rect(child)
            .expect("the child has a box");
        assert_eq!(
            (
                rect.origin.x,
                rect.origin.y,
                rect.size.width,
                rect.size.height
            ),
            (0.0, 0.0, 10.0, 20.0)
        );
    }

    #[test]
    fn only_a_root_reaching_invalidation_forces_a_full_pass() {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            "page { display: flex; width: 300px; height: 100px; }
             .box { display: flex; contain: strict; width: 80px; height: 40px; }
             .skip { display: flex; content-visibility: hidden;
                     contain-intrinsic-size: 40px 30px; width: 40px; height: 30px; }
             .leaf { width: 10px; height: 10px; }",
            StylesheetOrigin::Author,
        );
        let root = doc.document_element().id();

        let boundary = doc.create_element("view", ());
        doc.add_class(boundary, "box");
        doc.append_child(root, boundary);
        let c1 = doc.create_element("view", ());
        doc.add_class(c1, "leaf");
        doc.append_child(boundary, c1);
        let c2 = doc.create_element("view", ());
        doc.add_class(c2, "leaf");
        doc.append_child(boundary, c2);

        let plain = doc.create_element("view", ());
        doc.add_class(plain, "leaf");
        doc.append_child(root, plain);

        let skip = doc.create_element("view", ());
        doc.add_class(skip, "skip");
        doc.append_child(root, skip);
        let hidden_child = doc.create_element("view", ());
        doc.add_class(hidden_child, "leaf");
        doc.append_child(skip, hidden_child);

        doc.layout();

        let viewport = Size::new(800.0, 600.0);
        let scale = 1.0;
        assert!(
            !doc.layout_needs_pass(viewport, scale),
            "an unchanged frame after layout needs no pass at all",
        );

        doc.invalidate_layout(hidden_child);
        assert!(
            !doc.layout_requires_full_pass(viewport, scale),
            "a skipped-contents mutation must not force a whole-tree pass",
        );

        doc.invalidate_layout(c1);
        doc.invalidate_layout(c2);
        assert!(
            !doc.layout_requires_full_pass(viewport, scale),
            "a second mutation under one parked boundary must stay incremental",
        );

        doc.invalidate_layout(plain);
        assert!(
            doc.layout_requires_full_pass(viewport, scale),
            "a root-reaching mutation forces a whole-tree pass",
        );
    }

    #[test]
    fn a_mutation_under_a_skipped_box_parks_the_box_itself() {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            "page { display: flex; width: 300px; height: 100px; }
             .skip { display: flex; content-visibility: hidden;
                     contain-intrinsic-size: 40px 30px; width: 40px; height: 30px; }
             .leaf { width: 10px; height: 10px; }",
            StylesheetOrigin::Author,
        );
        let root = doc.document_element().id();
        let skip = child_of(&mut doc, root, "skip");
        let hidden_child = child_of(&mut doc, skip, "leaf");
        doc.layout();

        doc.invalidate_layout(hidden_child);

        let roots = doc.relayout_roots();
        assert_eq!(roots.len(), 1, "one parked root: {roots:?}");
        assert_eq!(roots[0].node_id, skip);
        assert!(
            matches!(roots[0].kind, RelayoutKind::Boundary),
            "a skipped box's committed input reproduces its output exactly, \
             so the re-run is final: {:?}",
            roots[0].kind,
        );
        assert!(
            doc.slot(skip)
                .and_then(|slot| doc.layout_state().get(slot))
                .is_none_or(|state| state.slot.layout_cache_is_empty()),
            "recording a parked root clears the cache it captured the input from",
        );
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Skipping {
        Hidden,
        Auto,
    }

    fn skipping_row_list(
        rows: usize,
        skipping: Skipping,
    ) -> (
        Document<()>,
        Vec<crate::tree::document::NodeId>,
        crate::tree::document::NodeId,
    ) {
        const VISIBLE_ROWS: usize = 2;
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        let (row_rule, skipped_rule) = match skipping {
            Skipping::Auto => ("content-visibility: auto;", ""),
            Skipping::Hidden => ("", ".skipped { content-visibility: hidden; }"),
        };
        doc.add_stylesheet(
            &format!(
                "page {{ display: flex; flex-direction: column; width: 200px; height: 600px;
                         align-items: flex-start; }}
                 .list {{ display: flex; flex-direction: column; width: 200px; height: 40px;
                          overflow: hidden; align-items: flex-start; }}
                 .row {{ display: flex; width: 200px; flex-shrink: 0; {row_rule}
                         contain-intrinsic-size: 200px 20px; }}
                 .cell {{ width: 20px; height: 20px; }}
                 {skipped_rule}"
            ),
            StylesheetOrigin::Author,
        );
        let root = doc.document_element().id();
        let list = child_of(&mut doc, root, "list");
        let mut row_ids = Vec::with_capacity(rows);
        let mut first_cell = None;
        for row in 0..rows {
            let element = child_of(&mut doc, list, "row");
            if skipping == Skipping::Hidden && row >= VISIBLE_ROWS {
                doc.add_class(element, "skipped");
            }
            let cell = child_of(&mut doc, element, "cell");
            first_cell.get_or_insert(cell);
            row_ids.push(element);
        }
        (doc, row_ids, first_cell.expect("the list has rows"))
    }

    #[test]
    fn a_mutation_beside_skipped_rows_re_resolves_no_skipped_box() {
        for skipping in [Skipping::Hidden, Skipping::Auto] {
            let mut counts = Vec::new();
            for rows in [8_usize, 64] {
                let (mut doc, row_ids, cell) = skipping_row_list(rows, skipping);
                let settle = |doc: &mut Document<()>| match skipping {
                    Skipping::Auto => {
                        doc.render();
                    }
                    Skipping::Hidden => doc.layout(),
                };
                settle(&mut doc);
                let rows_that_skip = row_ids
                    .iter()
                    .filter(|&&row| {
                        let node = doc.get(row).expect("the row is live");
                        skips_contents(node, node.layout_computed_style().expect("laid out"))
                    })
                    .count();
                doc.set_inline_style_property(cell, "width", "30px");
                let resolutions = super::host::skipped_size_resolutions_during(|| {
                    settle(&mut doc);
                });
                counts.push((rows_that_skip, resolutions));
            }
            let [(few, small_page), (many, large_page)] = counts[..] else {
                unreachable!("one measurement per row count")
            };
            assert!(
                many > few * 4,
                "{skipping:?}: {few} then {many} rows skip — the two pages have to \
                 differ in how much there is to re-resolve for the counts to mean \
                 anything",
            );
            assert_eq!(
                (small_page, large_page),
                (0, 0),
                "{skipping:?}: every skipped row's box came back from its cache, \
                 whatever the page's row count",
            );
        }
    }
}
