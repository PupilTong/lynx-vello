//! Box layout over the document tree — the concrete [`hughie`] host.

pub(crate) mod committed_box;
mod host;
pub(crate) mod relevance;
mod style;
pub(crate) mod text_block;

use std::sync::LazyLock;

use euclid::default::{Point2D, Rect, Size2D, Vector2D};
#[cfg(feature = "layout-test-utils")]
use hughie::compute::LeafMetrics;
pub use hughie::compute::NaturalSize;
pub(crate) use hughie::geometry::Edges;
#[cfg(feature = "layout-test-utils")]
use hughie::geometry::Point;
pub use hughie::geometry::Size;
use hughie::invalidate::is_relayout_boundary;
pub(crate) use hughie::style::TextBrush;
use hughie::style::{CoreStyle, PositionProperty};
use hughie::text::{FontBlob, TextContext};
pub use hughie::tree::Layout;
use hughie::tree::LayoutSlot;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;

pub(crate) use self::style::{
    DisplayMode, StyleView, box_parent, converts_tail_color, display_mode,
    establishes_absolute_containing_block, establishes_fixed_containing_block, generates_no_box,
    paragraph_limits_changed, shaping_inputs_changed, skips_contents,
};
use crate::render::image::{ImageOutcome, ImageRole};
use crate::tree::document::{DOCUMENT_ELEMENT_NODE_ID, Document, NodeLayoutState, RelayoutKind};

pub(crate) static ANONYMOUS_STYLE: LazyLock<Arc<ComputedValues>> = LazyLock::new(|| {
    use stylo::properties::style_structs::Font;
    ComputedValues::initial_values_with_font_override(Font::initial_values())
});

impl<T: Sync> Document<T> {
    /// Flushes style and lays the document out — up to
    /// [`CONTAINER_PASSES`](committed_box::CONTAINER_PASSES) times, because a
    /// css-contain-3 size query container's size is a *cascade* input that
    /// only layout can produce.
    ///
    /// Gecko does the same thing after its reflow, in
    /// `UpdateContainerQueryStyles`. A pass that moved no query container's
    /// content box — every pass of every page that has none — is the whole
    /// call, so the loop costs a page that does not use the feature one
    /// `is_empty` test.
    ///
    /// **The loop is the fallback, not the path.** A `container-type: size`
    /// box's size is contained in both axes, so the layout run settles it
    /// inside itself: it publishes the size at the entry of that box's own
    /// committing run and restyles the readers under it before laying its
    /// contents out at all (`layout::host::run_layout`, and
    /// [`crate::layout::committed_box`] for why the size is knowable there).
    /// What reaches this loop is what that cannot predict — an
    /// `inline-size` container, whose block axis answers to its contents, and
    /// a `display: -lynx-text` one, whose algorithm implements no size
    /// containment — plus the safety net of a prediction that disagreed with
    /// its algorithm.
    ///
    /// Which iteration this is decides how the marks are spelled, so the loop
    /// counts. A mark made *inside* the loop is read by the very next
    /// `layout_pass`'s flush, in this call, with nothing in between — so it
    /// can use the cheap `RECASCADE_SELF`, which no animation tick is there to
    /// strip. A mark made on the last permitted iteration is laid out by
    /// nobody here: it waits for whatever flush comes next, which an animation
    /// tick can precede, so that one is spelled the way
    /// [`Document::mark_subtree_recascade`] spells it.
    pub fn layout(&mut self) {
        let mut resized = Vec::new();
        for pass in 0..committed_box::CONTAINER_PASSES {
            self.layout_pass(&mut resized);
            let cap = pass + 1 == committed_box::CONTAINER_PASSES;
            if !self.recascade_resized_containers(&mut resized, cap) {
                break;
            }
        }
    }

    /// One flush and one layout, publishing the query-container sizes the
    /// pass committed into `resized`.
    fn layout_pass(&mut self, resized: &mut Vec<crate::NodeId>) {
        self.flush_styles_with_damage_sink(&mut |_, _| {});

        let viewport_size = self.device().viewport_size();
        let viewport = Size::new(viewport_size.width, viewport_size.height);
        let scale = self.device().device_pixel_ratio().get();

        if !self.layout_needs_pass(viewport, scale) {
            return;
        }

        let full = self.layout_requires_full_pass(viewport, scale);
        let rescale = self.layout_inputs_changed(viewport, scale);
        let bound = self.arenas().slot_bound();
        self.layout_state_mut().ensure_covers(bound);
        host::run_layout(self, viewport, scale, full, rescale, resized);
        // Publishing is what makes the pass's sizes readable by the style
        // traversal, which is parallel, and by the pass after this one; see
        // [`crate::layout::committed_box`]. The run publishes on its own
        // whenever it interleaved a restyle into itself, so what is left here
        // is what the *last* boxes it laid out recorded.
        self.arenas_mut().publish_committed_boxes(resized);
        self.clear_relayout_roots();
        self.mark_layout_complete(viewport, scale);
    }

    /// Marks the container-unit users under every query container the pass
    /// resized, and answers whether another pass is owed.
    ///
    /// Two gates, either of which ends the call, and a third inside the mark.
    /// The list is empty unless a size query container's content box actually
    /// moved, and the flag is false unless some style this document ever
    /// cascaded resolved a `cqw`/`cqh` — a page with query containers and no
    /// container units has nothing to re-resolve when one of them resizes.
    /// Then `mark_container_units_users` visits a resized container's subtree
    /// and marks only the elements whose own style resolved one, so a
    /// container that moved with no such element under it owes no pass
    /// either.
    ///
    /// `cap` says this is the last permitted iteration, and it changes the
    /// mark rather than suppressing it. Everywhere else the mark is consumed
    /// by the next `layout_pass`'s flush, inside the same `layout()` call and
    /// with no animation tick reachable in between, which is what licenses the
    /// rematch-free `RECASCADE_SELF` the targeted walk writes. Here the marks
    /// are made and simply not laid out: they wait for whatever flush comes
    /// next, and `RestyleHint::remove_animation_hints` deletes
    /// `RECASCADE_SELF` outright, so an animation tick in the gap would erase
    /// them. `mark_subtree_recascade`'s `RESTYLE_SELF | RECASCADE_DESCENDANTS`
    /// survives one, at the cost of being the coarse whole-subtree mark and of
    /// recascading the container itself, both of which this path accepts: it
    /// is reached only by a page whose containers are still moving after
    /// `CONTAINER_PASSES` layouts, and the document is one commit behind at
    /// worst either way.
    fn recascade_resized_containers(
        &mut self,
        resized: &mut Vec<crate::NodeId>,
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
            // A container freed since the run that measured it resolves to
            // nothing: `NodeId` carries the generation its key was at.
            if self.get(id).is_some_and(crate::Node::is_element) {
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

/// The intrinsic size a completed image load reports, as layout wants it.
///
/// One conversion for the two places a load can reach layout — the report
/// itself, and a source set on a node that had already loaded.
#[must_use]
pub(crate) fn natural_size(width: u32, height: u32) -> NaturalSize {
    #[expect(
        clippy::cast_precision_loss,
        reason = "an intrinsic size large enough to lose precision here is far past anything \
                  layout can present, and rounds harmlessly. It is deliberately not bounded: \
                  the atlas limit applies to the decoded bitmap, not to this."
    )]
    NaturalSize::from_size(hughie::geometry::Size::new(width as f32, height as f32))
}

impl<T> Document<T> {
    /// Updates intrinsic dimensions and invalidates affected layout.
    pub fn set_natural_size(&mut self, id: crate::NodeId, natural_size: NaturalSize) {
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
    pub(crate) fn natural_size(&self, id: crate::NodeId) -> NaturalSize {
        self.get(id)
            .map_or(NaturalSize::NONE, crate::Node::natural_size)
    }

    /// Sets one of this replaced element's two sources — the URL the paint
    /// walk presents to the host's resource system through
    /// [`FrameImages`](crate::FrameImages) — with `role` saying which.
    ///
    /// The two are independent sources on one element, requested
    /// concurrently: [`ImageRole::Placeholder`] is not a fallback the element
    /// reaches for when [`ImageRole::Source`] fails, it is what the box draws
    /// for as long as the other has nothing to draw — and a source that loads
    /// suppresses it for good, even if the placeholder's own pixels arrive
    /// later. Handing a role the value it already holds does nothing at all:
    /// the call returns before it binds, asks, or invalidates anything.
    ///
    /// Binding is what asks the host for the source, so the answer may already
    /// be in: a second mount of a URL this document has already seen settled
    /// is reported by nothing else ever again, and the [`ImageOutcome`] this
    /// returns is the only place its caller can learn what it got. `None` for
    /// a source still loading, whose outcome
    /// [`Self::apply_image_events`](Self::apply_image_events) will carry, and
    /// for a call that changed nothing.
    ///
    /// The element's natural size is this document's, not the caller's: it is
    /// recomputed here from whichever source the element now draws, so it
    /// always describes the bitmap `object-fit` is resolved against. Changing
    /// a source of an element that is already replaced therefore invalidates
    /// only the scene unless that size moved, but the call that *makes* an
    /// element replaced — or the one that takes its last source off it — is a
    /// layout change on its own: being replaced forces `DisplayMode::Leaf`,
    /// which sizes the box from its natural size and hides every child. Either
    /// role alone is enough to make an element replaced.
    pub fn set_image_source(
        &mut self,
        id: crate::NodeId,
        role: ImageRole,
        value: Option<&str>,
    ) -> Option<ImageOutcome> {
        let (changed, was_replaced, previous) = {
            let node = self
                .arenas_mut()
                .get_mut(id)
                .expect("stale NodeId passed to Document::set_image_source");
            assert!(
                node.is_element(),
                "non-element NodeId passed to Document::set_image_source"
            );
            let was_replaced = node.is_replaced();
            let previous = node.image_source(role).map(str::to_owned);
            (node.set_image_source(role, value), was_replaced, previous)
        };
        if !changed {
            return None;
        }
        // The registry has to know which node presents which source, or a
        // completed load has nobody to hand its intrinsic size to.
        if let Some(previous) = previous {
            self.images.unbind_node(&previous, id, role);
        }
        let mut outcome = None;
        if let Some(value) = value {
            self.images.bind_node(value, id, role);
            // Only the element's own source has an outcome to report: a
            // placeholder is an interim picture the page did not ask about, so
            // neither its load nor its failure is an event. `ImageOutcome` has
            // no variant naming one, and this is where that stays true.
            if role == ImageRole::Source {
                outcome = self.images.outcome_for(value, id);
            }
        }
        self.note_replaced_change(id, was_replaced);
        outcome
    }

    /// Settles a source change: the natural size the element's new sources
    /// give it, and the invalidation that change is worth.
    fn note_replaced_change(&mut self, id: crate::NodeId, was_replaced: bool) {
        self.refresh_natural_size(id);
        if was_replaced == self.get(id).is_some_and(crate::Node::is_replaced) {
            self.note_visual_mutation();
        } else {
            self.invalidate_layout(id);
        }
    }

    /// Puts on `id` the natural size of the bitmap it now presents — its own
    /// source's while that has pixels, its placeholder's until then, none at
    /// all when neither has any.
    ///
    /// Every change that can move that choice ends here, which is what keeps
    /// the natural size describing the bitmap actually drawn: `object-fit`
    /// resolves one against the other at paint, and a size left over from a
    /// departed source would fit the wrong picture.
    pub(crate) fn refresh_natural_size(&mut self, id: crate::NodeId) {
        let natural = {
            let Some(node) = self.get(id) else {
                return;
            };
            // A node that is not replaced has no natural size to hold, and
            // `set_natural_size` would make it replaced to give it one.
            if !node.is_replaced() {
                return;
            }
            self.images
                .presented_dimensions(
                    node.image_source(ImageRole::Source),
                    node.image_source(ImageRole::Placeholder),
                )
                .map_or(NaturalSize::NONE, |(width, height)| {
                    natural_size(width, height)
                })
        };
        self.set_natural_size(id, natural);
    }

    /// Both sources of a replaced element, in the order the paint walk
    /// prefers them.
    #[must_use]
    pub(crate) fn image_sources(&self, id: crate::NodeId) -> (Option<&str>, Option<&str>) {
        self.get(id).map_or((None, None), |node| {
            (
                node.image_source(ImageRole::Source),
                node.image_source(ImageRole::Placeholder),
            )
        })
    }

    #[cfg(feature = "layout-test-utils")]
    #[doc(hidden)]
    pub fn set_leaf_metrics_for_testing(
        &mut self,
        id: crate::NodeId,
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
        node.set_test_leaf_metrics(
            LeafMetrics::new(size).with_first_baselines(Point::new(None, first_baseline)),
        );
        self.invalidate_layout(id);
    }

    /// Takes over a text context an embedder built before this document
    /// existed.
    ///
    /// The alternative to [`Self::register_fonts`] and
    /// [`Self::set_default_font_family`], for a caller that has to know
    /// whether the fonts and the default family are usable *before* it is
    /// willing to build a document at all: it validates them against a
    /// context of its own and hands the result over here. Replaces whatever
    /// context this document had, so a document that has already laid
    /// anything out has to be invalidated whole.
    pub fn adopt_text_context(&mut self, context: TextContext) {
        self.layout_state_mut().text_context = Some(Box::new(context));
        self.invalidate_layout_all();
    }

    /// Registers an owned font resource without copying its byte payload.
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

    /// Every `@font-face` rule this document has not reported before.
    ///
    /// The document names the declared faces; the embedder fetches their
    /// sources — no IO happens in `dom` — and hands each blob back through
    /// [`Self::register_font_face`]. Each rule is reported exactly once,
    /// whatever later sheets do to the cascade.
    ///
    /// See [`FontFaceRequest`](crate::FontFaceRequest) for what is and is not
    /// carried out of a rule.
    pub fn take_font_face_requests(&mut self) -> Vec<crate::FontFaceRequest> {
        self.style_engine_mut().take_font_face_requests()
    }

    /// Registers a loaded `@font-face` source under its declared family.
    ///
    /// The answer to [`Self::take_font_face_requests`]: `data` is filed under
    /// `family` rather than under the name inside the font file, so runs
    /// naming the declared family shape with it. Returns how many faces the
    /// blob contributed, like [`Self::register_fonts`].
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

    /// Selects a registered family as the embedder-provided platform default.
    ///
    /// This maps CSS `system-ui`, `sans-serif`, and `serif` to `family` ahead
    /// of any platform fallbacks. Returns `false` when the family is unknown.
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
    pub fn rounded_layout(&self, id: crate::NodeId) -> Option<&Layout> {
        let slot = self.slot(id)?;
        Some(&self.layout_state().get(slot)?.slot.rounded)
    }

    /// The element's border box in viewport CSS pixels, as of the last
    /// completed layout pass.
    ///
    /// This is Lynx's own engine-side conversion
    /// (`platform_event_target_helper.cc`), not the web's
    /// `getBoundingClientRect`, and it differs from it in two ways.
    ///
    /// **No transforms.** `transform`, `offset-path` and any animation of
    /// them are ignored entirely: the rect is the untransformed border box
    /// the layout pass produced. Native sums each ancestor's `Left()`/`Top()`
    /// with an explicit `TODO: add transform support`, and Android excludes
    /// transforms unless asked for them; only iOS folds them in.
    ///
    /// **No flush.** Nothing here runs style, layout or paint — it reads
    /// what the last [`Document::layout`] left behind, so a caller that
    /// mutated the tree since sees the pre-mutation geometry until it lays
    /// out again.
    ///
    /// Ancestor scroll offsets *are* applied, for every scroll container on
    /// this box's containing-block chain (an out-of-flow box does not move
    /// with scrollers between it and its containing block, exactly as it
    /// does not in the paint order). The box's own scroll offset never
    /// applies: scrolling a container does not move the container.
    /// Sticky offsets are sampled from these same live scroll positions,
    /// including those inherited from sticky containing-block ancestors.
    ///
    /// `None` when the element has no box at all: `display: none` or
    /// `display: contents`, a node that is not a styled element, a node no
    /// pass has laid out, or one detached from the document tree.
    ///
    /// Coordinates are device-pixel-snapped, like [`Self::rounded_layout`].
    #[must_use]
    pub fn bounding_client_rect(&self, id: crate::NodeId) -> Option<Rect<f32>> {
        let node = self.get(id)?;
        let style = StyleView::try_of(node)?;
        if matches!(
            display_mode(style.display()),
            DisplayMode::None | DisplayMode::Contents
        ) {
            return None;
        }
        let layout = self.rounded_layout(id)?;
        let size = Size2D::new(layout.size.width, layout.size.height);
        let mut origin = Point2D::new(layout.location.x, layout.location.y);
        let mut sticky_offsets = Vec::new();
        if style.values().clone_position() == PositionProperty::Sticky {
            origin += crate::visual::sticky::live_offset(self, id, &mut sticky_offsets);
        }
        // The position the *escaping* box was keyed on, which decides which
        // ancestor is its containing block — and so which scroll offsets
        // move it. It is the computed value, not hughie's parent-lowered
        // one, so it matches what the paint walk keys its flow contexts on.
        let mut escape = style.values().clone_position();
        let mut current = node;
        let mut top = id;
        // `box_parent` skips `display: contents` ancestors, which hold a
        // zero layout and contribute no offset, and stops above the document
        // element — the node the paint order itself is rooted at.
        while let Some(ancestor) = box_parent(current) {
            let ancestor_style = StyleView::of(ancestor);
            if display_mode(ancestor_style.display()) == DisplayMode::None {
                // Nothing under a `display: none` box was laid out; whatever
                // geometry the node still carries is from before it was
                // hidden.
                return None;
            }
            let ancestor_id = ancestor.id();
            let ancestor_layout = self.rounded_layout(ancestor_id)?;
            origin += Vector2D::new(ancestor_layout.location.x, ancestor_layout.location.y);
            let on_chain = match escape {
                PositionProperty::Absolute => {
                    establishes_absolute_containing_block(ancestor, ancestor_style.values())
                }
                PositionProperty::Fixed => {
                    establishes_fixed_containing_block(ancestor, ancestor_style.values())
                }
                PositionProperty::Static
                | PositionProperty::Relative
                | PositionProperty::Sticky => true,
            };
            if on_chain {
                if ancestor_style.values().clone_position() == PositionProperty::Sticky {
                    origin +=
                        crate::visual::sticky::live_offset(self, ancestor_id, &mut sticky_offsets);
                }
                if self.is_scroll_container(ancestor_id) {
                    origin -= self.scroll_offset(ancestor_id);
                }
                escape = ancestor_style.values().clone_position();
            }
            current = ancestor;
            top = ancestor_id;
        }
        // A subtree detached from the document answers nothing, the way a
        // disconnected element does on the web: its coordinates would be
        // relative to a root that is not the viewport.
        (top == DOCUMENT_ELEMENT_NODE_ID).then(|| Rect::new(origin, size))
    }

    /// The measured size of the paragraph `id` establishes.
    ///
    /// The block's own ink, not its box: a stretched text element is wider
    /// than the text in it. This is the size Lynx's `layout` event reports,
    /// and the only way an embedder can ask what a run actually measured now
    /// that a text node has no box of its own.
    #[must_use]
    pub fn text_block_size(&self, id: crate::NodeId) -> Option<hughie::geometry::Size<f32>> {
        Some(self.text_block(id)?.size())
    }

    /// The nodes behind `id`'s paragraph items, indexed the way
    /// [`SourceItem::Content`](hughie::text::block::SourceItem) indexes them.
    ///
    /// How a glyph run finds the element whose style painted it.
    #[must_use]
    pub(crate) fn text_block_sources(&self, id: crate::NodeId) -> Option<&[crate::NodeId]> {
        let slot = self.slot(id)?;
        Some(&self.layout_state().get(slot)?.text.as_deref()?.source_ids)
    }

    /// The nodes behind `id`'s custom truncation content, indexed the way
    /// [`SourceItem::Truncation`](hughie::text::block::SourceItem) indexes
    /// them — so the marker paints in its own subtree's colours rather than
    /// the establishing element's.
    #[must_use]
    pub(crate) fn text_block_truncation_sources(
        &self,
        id: crate::NodeId,
    ) -> Option<&[crate::NodeId]> {
        let slot = self.slot(id)?;
        Some(
            &self
                .layout_state()
                .get(slot)?
                .text
                .as_deref()?
                .truncation_source_ids,
        )
    }

    /// The paragraph `id` established, if a commit has produced one.
    ///
    /// `None` keeps every reader fail-closed: a block no commit laid out is
    /// one that does not paint, rather than a panic on the paint thread.
    #[must_use]
    pub(crate) fn text_block(&self, id: crate::NodeId) -> Option<&hughie::text::block::TextBlock> {
        let slot = self.slot(id)?;
        self.layout_state().get(slot)?.text.as_deref()?.committed()
    }

    /// How many times `id`'s paragraph has been shaped from scratch.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn text_block_rebuilds(&self, id: crate::NodeId) -> Option<u32> {
        let slot = self.slot(id)?;
        Some(self.layout_state().get(slot)?.text.as_deref()?.rebuilds)
    }

    /// Whether a probe left `id`'s paragraph off its committed break.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn text_block_is_probe_dirty(&self, id: crate::NodeId) -> bool {
        self.slot(id)
            .and_then(|slot| self.layout_state().get(slot))
            .and_then(|state| state.text.as_deref())
            .is_some_and(|store| store.block.is_probe_dirty())
    }

    #[must_use]
    pub(crate) fn paint_style(&self, id: crate::NodeId) -> Option<&ComputedValues> {
        self.get(id)?.layout_computed_style()
    }

    /// The device-snapped grid area a sticky grid item's insets resolve
    /// against, in its box parent's border-box coordinates, when the last
    /// layout recorded one; `None` for every other box.
    #[must_use]
    pub(crate) fn sticky_containing_block(&self, id: crate::NodeId) -> Option<Edges<f32>> {
        let slot = self.slot(id)?;
        self.layout_state()
            .sticky_containing_blocks
            .iter()
            .find(|entry| entry.node == slot)
            .and_then(|entry| entry.rounded)
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn layout_cache_is_empty(&self, id: crate::NodeId) -> Option<bool> {
        let slot = self.slot(id)?;
        Some(
            self.layout_state()
                .get(slot)
                .is_none_or(|state| state.slot.layout_cache_is_empty()),
        )
    }

    /// Drops the cached box measurements of `id` and of every ancestor whose
    /// geometry could move because of it.
    ///
    /// **Box caches only.** A shaped paragraph is not geometry: it dies when
    /// the text or the style it was shaped from changes, which is a different
    /// question and has its own entry points
    /// ([`Document::invalidate_text_children`] and
    /// [`TreeArenas::clear_text_artifact`]). Clearing it here would be free
    /// today — a text node is a leaf, and no element holds an artifact — but
    /// it stops being free the moment a paragraph is owned by the element
    /// that establishes it: every descendant mutation and every ancestor this
    /// walk touches would re-shape a paragraph whose text never changed.
    pub(crate) fn invalidate_layout(&mut self, id: crate::NodeId) {
        // Paragraph content (including generated runs) shares one cache. A
        // transparent scope has none, so starting the usual walk there would
        // stop early and leave its enclosing paragraph measured at the old text.
        let id = if let Some(paragraph) = self.paragraph_container(id) {
            // An atomic inline child still owns a measured box. Clear it as
            // well, before asking the paragraph to measure its children again.
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
            let (tree, state, _) = self.layout_parts();
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
                {
                    // A contents element never owns a layout cache. Its
                    // children participate in the nearest box ancestor, so
                    // an empty cache here cannot prove that ancestor was
                    // already invalidated or provide an in-place boundary.
                    state.clear_box_cache(node_slot);
                    current = node.flat_parent_id();
                    continue;
                }
                let node_state = state.get(node_slot).map(|state| &state.slot);
                if node_state.is_none_or(LayoutSlot::layout_cache_is_empty)
                    && node.flat_parent_id().is_some()
                {
                    // Nothing above needs clearing: either an earlier
                    // invalidation already walked past here (whatever it
                    // recorded still stands), the node sits parked for a
                    // committed-input relayout (recording clears its cache),
                    // or it was never laid out (detached or hidden). The
                    // parentless document node is exempt — it never holds a
                    // cache, and stopping there must still count as reaching
                    // the root.
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
                    // A committed input the parent imposed independently of
                    // this subtree's content survives the mutation, so the
                    // subtree can relayout in place under it; run_layout
                    // verifies the output stayed identical before trusting it.
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
                    // A skipped box is a relayout boundary, so the arm above
                    // parks it whenever it has a committed input to re-run
                    // under. Reaching here means it has none — its size was
                    // only ever probed, or the cache just cleared holds no
                    // commit — and then there is nothing to re-run and
                    // nothing above it to tell: its contents are not laid out
                    // and do not paint until it stops skipping, which
                    // invalidates it on its own.
                    reached_root = false;
                    break;
                }
                current = node.flat_parent_id();
            }
            // Running off the top of a detached subtree is not reaching the
            // root: nothing above it was ever laid out, and the caches of the
            // document it will join are untouched. Counting it would make the
            // next pass a whole-tree pass that ignores the relayout roots the
            // eventual insertion records, and that pass stops at the root's
            // still-valid cache, leaving the inserted subtree unlaid.
            reached_root &= top == crate::tree::document::DOCUMENT_NODE_ID;
            (pending, reached_root)
        };
        self.mark_layout_dirty(reached_root);
        if let Some((root_id, committed_input, kind)) = pending {
            self.record_relayout_root(root_id, committed_input, kind);
        }
    }

    /// `id` gained or lost the containing block of its absolute and fixed
    /// descendants: its transform animation started or ended (a current one
    /// acts as `will-change: transform`), or a restyle flipped what
    /// [`establishes_fixed_containing_block`] or
    /// [`establishes_absolute_containing_block`] read (the harvests compare
    /// the old and new computed style). Answers whether any such descendant
    /// was invalidated.
    ///
    /// Every positioned descendant that could name `id` lays out again:
    /// rounding only re-hoists a box under a subtree some write reached. That
    /// holds even where its style establishes the block now, since the same
    /// flush can have changed that style too and the relayout damage it
    /// produced reaches `id` alone. `id` itself lays out again only through a
    /// child whose lowered position flipped, whose upward walk clears it; a
    /// deeper box is hoisted on both sides of the flip. The walk stops below
    /// an element that establishes the fixed containing block itself, which
    /// the absolute one implies; it runs only for an element whose role
    /// flipped, never per frame.
    pub(crate) fn invalidate_containing_block(&mut self, id: crate::NodeId) -> bool {
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
                if display_mode(style.clone_display()) == DisplayMode::None {
                    continue;
                }
                match style.clone_position() {
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
        for &descendant in &positioned {
            self.invalidate_layout(descendant);
        }
        !positioned.is_empty()
    }

    /// Generated runs have no box cache of their own. Reach the paragraph
    /// through transparent/nested scopes before using normal layout invalidation.
    fn paragraph_container(&self, id: crate::NodeId) -> Option<crate::NodeId> {
        use stylo::values::computed::Display;
        let start = self.get(id)?;
        // Its own display may just have become none. The old run still
        // belongs to the surrounding paragraph and must be removed from it.
        let mut paragraph = start
            .layout_computed_style()
            .filter(|style| style.clone_display() == Display::LynxText)
            .map(|_| id);
        let mut current = start.flat_parent();
        while let Some(node) = current {
            let Some(style) = node.layout_computed_style() else {
                break;
            };
            match style.clone_display() {
                Display::LynxText => paragraph = Some(node.id()),
                Display::Contents => {}
                _ => break,
            }
            current = node.flat_parent();
        }
        paragraph
    }

    pub(crate) fn invalidate_layout_all(&mut self) {
        for (
            _,
            NodeLayoutState {
                slot,
                text,
                scroll_offset: _,
            },
        ) in self.layout_data_mut()
        {
            slot.clear_layout_cache();
            *text = None;
        }
        self.clear_relayout_roots();
        self.mark_layout_dirty(true);
    }

    /// Test/benchmark-only cache invalidation hook for protocol fixtures that
    /// mutate synthetic leaf data outside the production DOM mutation API.
    #[cfg(feature = "layout-test-utils")]
    #[doc(hidden)]
    pub fn invalidate_layout_for_testing(&mut self, id: crate::NodeId) {
        self.invalidate_layout(id);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::mem::size_of;

    use hughie::tree::{LayoutInput, LayoutOutput, LayoutSlot};
    use stylo::invalidation::element::restyle_hints::RestyleHint;

    use super::*;
    use crate::StylesheetOrigin;
    use crate::tree::document::DOCUMENT_NODE_ID;

    #[test]
    fn layout_state_size_probe() {
        const PRE_SPLIT_NODE_SIZE: usize = 368;
        const PRE_SPLIT_ATOMIC_LAYOUT_DATA_SIZE: usize = 456;
        const PRE_SPLIT_ATOMIC_LAYOUT_RESULTS_SIZE: usize = 160;
        // The paragraph is one `Option<Box<_>>` on the node's layout state, so
        // the store's own size never entered this budget — only the pointer
        // does. The retired measurement path's `TextLayoutStore` used to be
        // measured here beside it, which said nothing the pointer did not.
        let current = (
            size_of::<crate::Node<()>>(),
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
            (if cfg!(debug_assertions) { 232 } else { 224 }, 336, 352),
            "Node, LayoutSlot and NodeLayoutState sizes changed",
        );
    }

    /// A document with one size query container holding one child, and
    /// whatever `item` declaration the case wants on it.
    fn query_container_document(item: &str) -> (Document<()>, crate::NodeId) {
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

    /// The loop's first gate: a page whose styles never resolved a `cqw` or a
    /// `cqh` has nothing to re-resolve when a query container's size moves,
    /// so the container's *first* recorded size — which is always a move,
    /// from nothing to something — costs it no second pass and leaves no
    /// restyle behind.
    #[test]
    #[expect(clippy::float_cmp, reason = "rounded boxes have exact pixel geometry")]
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

    /// The second gate: once the units are in play the first pass does
    /// recascade, and the pass after it settles — the container's size did
    /// not move again, so the changed list is empty and the call ends.
    #[test]
    #[expect(clippy::float_cmp, reason = "rounded boxes have exact pixel geometry")]
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

    /// One document per shape: `page > .query > .item`, with the case's own
    /// rules for the two.
    fn query_document(rules: &str) -> (Document<()>, crate::NodeId, crate::NodeId) {
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

    fn laid_out_size(document: &Document<()>, id: crate::NodeId) -> (f32, f32) {
        let size = document
            .rounded_layout(id)
            .expect("the box was laid out")
            .size;
        (size.width, size.height)
    }

    /// The interleave, shape by shape: a `container-type: size` box publishes
    /// the size it is about to take *before* its contents are laid out, so one
    /// layout run settles the page — the units resolve against the container
    /// on the only pass there is.
    ///
    /// The `inline-size` shape is the one the interleave declines, and it is
    /// here to show what declining costs: its block axis answers to its
    /// contents, so there is nothing to publish before them and the loop in
    /// [`Document::layout`] settles it in a second run, exactly as it did
    /// before the interleave existed.
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

    /// A container that moves on a settled page settles in one run too —
    /// including the move that stops it being a container at all, which the
    /// interleave never sees: losing `container-type` is a change to the
    /// *container's own* style, so Stylo's own invalidation has already put
    /// the units back on the viewport fallback in the flush that precedes the
    /// run.
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

    /// Nested containers settle in one run as well, one nesting level per
    /// iteration of the run's own settling loop: the outer container's
    /// contents — the inner container among them — are laid out only once the
    /// outer's size is published, and the inner's only once its own is.
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

    /// The interleave is a prediction, and the record the algorithm leaves
    /// behind is what is believed. A published size no run ever produced —
    /// staged here behind the run's back, which is what a prediction that
    /// disagreed with its algorithm would amount to — is therefore not
    /// durable: the next committing run of that container finds it, publishes
    /// what the box actually takes, and the readers re-resolve from that.
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

    /// The Ahem face every text measurement here is deterministic because of:
    /// one em per glyph, so a run's width is its length times its font size.
    const AHEM: &[u8] = include_bytes!("../../../hughie/tests/fixtures/Ahem.ttf");

    /// A paragraph whose *font size* is a container unit is the case the
    /// interleave has to reach past style into the text pipeline for: the
    /// shaped run is an artifact of the style, not of the geometry, and a
    /// paragraph shaped against the viewport fallback would have to be thrown
    /// away and shaped again.
    ///
    /// One run, one shaping. The restyle the run interleaves into itself goes
    /// through the same damage harvest a flush does, so the text artifact is
    /// evicted before the paragraph is ever built — rather than after it was
    /// built at the wrong size.
    #[test]
    fn a_text_reader_under_a_container_is_shaped_once_at_the_container_size() {
        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; align-items: flex-start;
                    width: 400px; height: 300px; font-family: Ahem; }
             .query { display: flex; container-type: size;
                      width: 200px; height: 100px; }
             .label { display: -lynx-text; font-size: 10cqw; }",
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

    /// The loop is still the fallback, and a `display: -lynx-text` query
    /// container is what still needs it: the text block algorithm implements
    /// no size containment, so its content box is not a function of its own
    /// style and the interleave declines to predict one. The container's size
    /// is published after the run that laid it out, and the units under it
    /// re-resolve in the run after that — the pre-interleave path, intact.
    #[test]
    #[expect(clippy::float_cmp, reason = "Ahem runs have exact pixel geometry")]
    fn a_text_query_container_is_settled_by_the_loop() {
        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; align-items: flex-start;
                    width: 400px; height: 300px; font-family: Ahem; font-size: 16px; }
             .query { display: -lynx-text; container-type: size;
                      width: 200px; line-height: 20px; }
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

    /// Plain leaves per container in [`mixed_container_document`] — enough
    /// that marking them would be visible, few enough to name each in a
    /// failure.
    const PLAIN_LEAVES: usize = 8;

    /// Two query containers of `container_type` on one page: the first holds
    /// one `cqw` leaf among [`PLAIN_LEAVES`] px ones, the second holds px
    /// leaves only.
    ///
    /// Both containers record a size on the first pass, so both reach the
    /// recascade — the second is the container that must mark nothing.
    fn mixed_container_document(
        container_type: &str,
    ) -> (
        Document<()>,
        [crate::NodeId; 2],
        crate::NodeId,
        Vec<crate::NodeId>,
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

    fn restyle_hint(document: &Document<()>, id: crate::NodeId) -> RestyleHint {
        document
            .live(id)
            .borrow_computed_style()
            .expect("the flush styled every element on the page")
            .hint
    }

    /// What a resized container's restyle costs: one mark per element that
    /// actually resolved a container unit, and nothing for the rest of the
    /// subtree.
    ///
    /// The mark is made *inside* the layout run now, which consumes it before
    /// it returns, so it is replayed here on the settled document — the same
    /// call the run makes, against a page whose readers are already at their
    /// final size.
    #[test]
    #[expect(clippy::float_cmp, reason = "rounded boxes have exact pixel geometry")]
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

    /// The cap iteration spells its marks differently, because they are the
    /// only ones that outlive the `layout()` call that made them.
    ///
    /// Everywhere else in the loop the next `layout_pass` flushes immediately,
    /// so a bare `RECASCADE_SELF` is safe and cheap — and, as the second half
    /// of this test shows by running Stylo's own eraser over it, it is exactly
    /// what an animation tick would take away if one could run first. On the
    /// last permitted iteration the marks wait for a later flush, so the
    /// container gets the whole-subtree spelling that survives that tick.
    #[test]
    fn the_cap_iteration_leaves_a_mark_an_animation_tick_cannot_erase() {
        // An `inline-size` container is the shape the interleave declines —
        // its block axis answers to its contents, so there is no size to
        // publish before them — which is what leaves the loop to settle it and
        // makes the two halves drivable apart here.
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

    /// The marks stop at the elements that resolved a unit because Stylo
    /// carries the rest: a `font-size: 10cqw` child is the only element under
    /// the container whose style carries `USES_CONTAINER_UNITS`, and its own
    /// grandchild's `2em` width follows the container anyway — the child
    /// cascade requirement propagates `RECASCADE_SELF` down when an inherited
    /// value moves.
    #[test]
    #[expect(clippy::float_cmp, reason = "rounded boxes have exact pixel geometry")]
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

        // 10cqw of the 200px container is a 20px font, and the leaf is 2em of
        // that — reached in the one call, from a first pass that resolved the
        // font against the 800px viewport fallback.
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

    /// Builds the shape a Lynx label actually has: an auto-sized `text`
    /// element holding one text node, inside a definite-width flex row.
    fn label_document_parts(
        text: &str,
        width: f32,
    ) -> (Document<()>, crate::NodeId, crate::NodeId) {
        const AHEM: &[u8] = include_bytes!("../../../hughie/tests/fixtures/Ahem.ttf");

        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        document.add_stylesheet(
            &format!(
                "page {{ display: flex; width: {width}px; height: 100px;
                         align-items: flex-start; font-family: Ahem; font-size: 16px; }}
                 .label {{ display: -lynx-text; }}"
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

    /// The number this whole path exists to hold down.
    ///
    /// Nine measurements reach a flex text child in one pass — max-content,
    /// min-content, and its used width, each asked once per enclosing flex
    /// level, plus the commit — because the box cache keys on the available
    /// height a text node's answer does not depend on. What survives is one
    /// line break per *distinct* constraint, which is the floor. Before the
    /// break and constraint memos all nine broke, and the first probe also
    /// deep-cloned the shaped layout.
    #[test]
    fn one_pass_breaks_a_text_node_once_per_distinct_constraint() {
        for (case, text, width, lines, breaks) in [
            // min-content == max-content == used width: two distinct widths.
            ("one word", "hello", 200.0, 1, 2),
            // min-content (one word) < used width == max-content: three.
            ("one line, two words", "hello world", 200.0, 1, 3),
            // min-content, max-content as a width, the used width, and
            // unconstrained: four, one break each.
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

    /// The case the restore queue exists for: a pass that measures a text node
    /// and never commits it.
    ///
    /// It is reachable whenever the box cache answers a node's `Commit` but
    /// not the `Measure` that preceded it — a single-axis probe can never be
    /// served from the committed slot, while the commit input the parent
    /// re-imposes always is. Driving the host directly is the deterministic
    /// way to produce it; what matters is that the layout does not end the
    /// pass painting the probe's line breaks.
    #[test]
    fn a_probe_that_never_commits_is_handed_back_to_its_committed_break() {
        use hughie::tree::{AvailableSpace, LayoutInput, LayoutTree, RequestedAxis};

        let (mut document, label, _run) = label_document_parts("hello world", 200.0);
        document.layout();
        let committed = document.text_block(label).expect("committed").size();
        let lines = document.text_block(label).expect("committed").lines().len();

        let slot = document.live_slot(label);
        let (tree, state, _) = document.layout_parts();
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

    /// Paragraph limits evict box and break measurements, preserving the
    /// natural shaped layout even across intrinsic-width probes.
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
    #[expect(clippy::float_cmp, reason = "rounded boxes have exact pixel geometry")]
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
    #[expect(
        clippy::float_cmp,
        reason = "explicit line heights have exact pixel metrics"
    )]
    fn interpolated_integer_paragraph_limits_relayout_without_reshaping() {
        use hughie::style::TextContainerStyle;

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
            // The flush above creates the animation; the tick that follows is
            // the frame it starts on, which puts its origin at zero.
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

    /// A relayout-damaged element keeps its shaped glyphs unless the restyle
    /// moved something Parley shapes from.
    #[test]
    fn a_relayout_keeps_shaped_text_unless_the_shaping_inputs_moved() {
        for (case, declaration, survives) in [
            // Geometry: a different `Font`/`InheritedText` is never even
            // allocated, so level one answers.
            ("width", "width: 150px", true),
            ("padding", "padding: 4px", true),
            // Same struct as `letter-spacing`, but not a shaping input — only
            // the field comparison can tell these apart.
            ("text-align", "text-align: center", true),
            ("text-indent", "text-indent: 8px", true),
            // Shaping inputs.
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

        // Force the whole spine to re-measure, then check the paragraph did
        // not end the pass holding an intrinsic-sizing probe's break.
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

    /// Becoming replaced is a layout change, not just a paint change:
    /// `is_replaced` forces `DisplayMode::Leaf`, which sizes the box from its
    /// natural size and hides every child. A source that arrives before any
    /// natural size — the ordinary order, because the source comes from an
    /// attribute and the size only from a completed load — must therefore
    /// invalidate layout, or the document keeps laying the node out as a
    /// container while painting it as an image.
    #[test]
    fn the_source_that_makes_an_element_replaced_invalidates_layout() {
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
        document.set_image_source(image, ImageRole::Source, Some("app:///a.png"));
        for id in [DOCUMENT_NODE_ID, root, image] {
            assert_eq!(
                document.layout_cache_is_empty(id),
                Some(true),
                "the first source flips the node to replaced, which layout reads",
            );
        }

        // A later source only changes which pixels the same replaced box
        // draws, so the retained boxes survive and only the scene is rebuilt.
        prime(&mut document);
        document.render();
        document.set_image_source(image, ImageRole::Source, Some("app:///b.png"));
        assert!(
            document.needs_render(),
            "a new source invalidates the retained frame"
        );
        for id in [DOCUMENT_NODE_ID, root, image] {
            assert_eq!(
                document.layout_cache_is_empty(id),
                Some(false),
                "swapping one image for another changes no layout input",
            );
        }
    }

    /// Clearing a source an element never had must stay a no-op. Writing
    /// `Replaced` for it would turn an ordinary container into a childless
    /// zero-sized leaf, which is not what "there is no image here" means.
    #[test]
    fn clearing_a_source_never_set_leaves_the_element_alone() {
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        let view = document.create_element("view", ());
        document.append_child(root, view);
        document.layout();
        document.render();

        document.set_image_source(view, ImageRole::Source, None);

        assert!(!document.get(view).expect("live element").is_replaced());
        assert!(!document.needs_render(), "nothing changed");
    }

    // The two sources of a replaced element, and the one bitmap they decide
    // between. `SRC` is the picture the element is for; `PLACEHOLDER` is what
    // it draws until `SRC` has pixels.
    const SRC: &str = "app:///a.png";
    const OTHER_SRC: &str = "app:///b.png";
    const PLACEHOLDER: &str = "app:///p.png";

    fn image_document() -> (Document<()>, crate::NodeId) {
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        let image = document.create_element("image", ());
        document.append_child(root, image);
        (document, image)
    }

    fn loaded(source: &str, width: u32, height: u32) -> crate::ImageEvent {
        crate::ImageEvent::Loaded {
            source: std::sync::Arc::from(source),
            width,
            height,
        }
    }

    fn failed(source: &str) -> crate::ImageEvent {
        crate::ImageEvent::Failed {
            source: std::sync::Arc::from(source),
        }
    }

    /// Both sources are asked for the moment they are written, and neither
    /// waits on the other: a placeholder is not what an element reaches for
    /// after its source failed, it is a second request made alongside it.
    #[test]
    fn both_of_an_elements_sources_are_asked_for_at_once() {
        let (mut document, image) = image_document();
        document.set_image_source(image, ImageRole::Source, Some(SRC));
        document.set_image_source(image, ImageRole::Placeholder, Some(PLACEHOLDER));

        let mut wanted = document.take_wanted_images();
        wanted.sort();
        assert_eq!(
            wanted,
            vec![
                std::sync::Arc::<str>::from(SRC),
                std::sync::Arc::<str>::from(PLACEHOLDER)
            ]
        );
        assert!(document.take_wanted_images().is_empty(), "each asked once");
    }

    /// The natural size is the drawn bitmap's, whichever source that is —
    /// `object-fit` fits one against the other at paint.
    #[test]
    fn a_loaded_source_suppresses_the_placeholder() {
        let (mut document, image) = image_document();
        document.set_image_source(image, ImageRole::Source, Some(SRC));
        document.set_image_source(image, ImageRole::Placeholder, Some(PLACEHOLDER));
        assert_eq!(
            document.natural_size(image),
            NaturalSize::NONE,
            "neither source has pixels yet, so there is no bitmap to describe"
        );

        document.apply_image_events(&[loaded(PLACEHOLDER, 4, 4)]);
        assert_eq!(document.natural_size(image), natural_size(4, 4));

        document.apply_image_events(&[loaded(SRC, 40, 20)]);
        assert_eq!(
            document.natural_size(image),
            natural_size(40, 20),
            "the element's own source takes over the moment it has pixels"
        );
    }

    /// And the suppression is permanent: a placeholder whose load lands after
    /// the source's changes nothing visible.
    #[test]
    fn a_placeholder_arriving_after_the_source_changes_nothing() {
        let (mut document, image) = image_document();
        document.set_image_source(image, ImageRole::Source, Some(SRC));
        document.set_image_source(image, ImageRole::Placeholder, Some(PLACEHOLDER));

        document.apply_image_events(&[loaded(SRC, 40, 20)]);
        document.apply_image_events(&[loaded(PLACEHOLDER, 4, 4)]);
        assert_eq!(document.natural_size(image), natural_size(40, 20));
    }

    /// A failed source is not a failed element: the placeholder it was
    /// covering stays drawn, and one that loads afterwards still appears.
    #[test]
    fn a_failed_source_leaves_the_placeholder_showing() {
        let (mut document, image) = image_document();
        document.set_image_source(image, ImageRole::Source, Some(SRC));
        document.set_image_source(image, ImageRole::Placeholder, Some(PLACEHOLDER));

        document.apply_image_events(&[failed(SRC)]);
        assert_eq!(document.natural_size(image), NaturalSize::NONE);

        document.apply_image_events(&[loaded(PLACEHOLDER, 4, 4)]);
        assert_eq!(document.natural_size(image), natural_size(4, 4));
    }

    /// A placeholder is a bitmap in the content box like any other, so an
    /// element that has only one is replaced content and sizes from it.
    #[test]
    fn a_placeholder_alone_makes_an_element_replaced() {
        let (mut document, image) = image_document();
        document.set_image_source(image, ImageRole::Placeholder, Some(PLACEHOLDER));

        assert!(document.get(image).expect("live element").is_replaced());
        document.apply_image_events(&[loaded(PLACEHOLDER, 4, 4)]);
        assert_eq!(document.natural_size(image), natural_size(4, 4));
    }

    /// Swapping one source for another blanks the element until the new one
    /// loads, rather than fitting the new bitmap to the departed one's size.
    #[test]
    fn swapping_the_source_drops_the_departed_bitmaps_size() {
        let (mut document, image) = image_document();
        document.set_image_source(image, ImageRole::Source, Some(SRC));
        document.apply_image_events(&[loaded(SRC, 40, 20)]);
        assert_eq!(document.natural_size(image), natural_size(40, 20));

        document.set_image_source(image, ImageRole::Source, Some(OTHER_SRC));
        assert_eq!(
            document.natural_size(image),
            NaturalSize::NONE,
            "a pending source draws nothing, and describes nothing"
        );

        // With a placeholder ready, that is what the blanked element shows.
        document.set_image_source(image, ImageRole::Placeholder, Some(PLACEHOLDER));
        document.apply_image_events(&[loaded(PLACEHOLDER, 4, 4)]);
        assert_eq!(document.natural_size(image), natural_size(4, 4));
    }

    /// The natural size sizes the box wherever containment does not remove
    /// it, so every change of it has to reach layout — the same invalidation
    /// `set_natural_size` does on its own.
    #[test]
    fn a_swapped_source_invalidates_layout_through_its_natural_size() {
        let (mut document, image) = image_document();
        let root = document.document_element().id();
        document.set_image_source(image, ImageRole::Source, Some(SRC));
        document.apply_image_events(&[loaded(SRC, 40, 20)]);

        let input = LayoutInput::default();
        for id in [DOCUMENT_NODE_ID, root, image] {
            let slot = document.live_slot(id);
            document
                .layout_state_mut()
                .at_mut(slot)
                .slot
                .store_cached_layout(input, LayoutOutput::default());
        }
        document.set_image_source(image, ImageRole::Source, Some(OTHER_SRC));

        for id in [DOCUMENT_NODE_ID, root, image] {
            assert_eq!(
                document.layout_cache_is_empty(id),
                Some(true),
                "the element lost the size it was laid out at",
            );
        }
    }

    /// A source that settled before this element bound to it is reported by
    /// nothing else ever again — one URL is reported once — so the bind is
    /// where its outcome and its size both arrive.
    #[test]
    fn binding_a_settled_source_reports_its_outcome_and_sizes_the_element() {
        let (mut document, first) = image_document();
        let root = document.document_element().id();
        assert_eq!(
            document.set_image_source(first, ImageRole::Source, Some(SRC)),
            None,
            "a pending source owes nothing yet"
        );
        document.apply_image_events(&[loaded(SRC, 40, 20), failed(OTHER_SRC)]);

        let second = document.create_element("image", ());
        document.append_child(root, second);
        assert_eq!(
            document.set_image_source(second, ImageRole::Source, Some(SRC)),
            Some(crate::ImageOutcome::Loaded {
                node: second,
                width: 40,
                height: 20
            })
        );
        assert_eq!(
            document.natural_size(second),
            natural_size(40, 20),
            "and it lays out in the commit that first draws it"
        );

        let third = document.create_element("image", ());
        document.append_child(root, third);
        assert_eq!(
            document.set_image_source(third, ImageRole::Source, Some(OTHER_SRC)),
            Some(crate::ImageOutcome::Failed { node: third })
        );
        assert_eq!(document.natural_size(third), NaturalSize::NONE);
    }

    /// Rewriting the source that is already there changes nothing, and
    /// reports nothing — a second `load` for a picture that never moved.
    #[test]
    fn rewriting_the_same_source_reports_nothing() {
        let (mut document, image) = image_document();
        document.set_image_source(image, ImageRole::Source, Some(SRC));
        document.apply_image_events(&[loaded(SRC, 40, 20)]);

        assert_eq!(
            document.set_image_source(image, ImageRole::Source, Some(SRC)),
            None
        );
    }

    /// Only the elements the source belongs to are reported: a placeholder is
    /// an interim picture the page did not ask about.
    #[test]
    fn a_report_names_only_the_elements_whose_own_source_settled() {
        let (mut document, owner) = image_document();
        let root = document.document_element().id();
        let borrower = document.create_element("image", ());
        document.append_child(root, borrower);
        document.set_image_source(owner, ImageRole::Source, Some(SRC));
        document.set_image_source(borrower, ImageRole::Placeholder, Some(SRC));

        assert_eq!(
            document.apply_image_events(&[loaded(SRC, 40, 20)]),
            vec![crate::ImageOutcome::Loaded {
                node: owner,
                width: 40,
                height: 20
            }]
        );
        assert_eq!(
            document.natural_size(borrower),
            natural_size(40, 20),
            "the placeholder still sizes the element drawing it"
        );

        let (mut document, image) = image_document();
        document.set_image_source(image, ImageRole::Placeholder, Some(PLACEHOLDER));
        assert!(
            document
                .apply_image_events(&[failed(PLACEHOLDER)])
                .is_empty(),
            "a placeholder failing is nobody's event either"
        );

        // Nor at the bind, which is the other place an outcome is produced:
        // the role decides, so a settled URL bound as a placeholder answers
        // with nothing.
        let second = document.create_element("image", ());
        document.append_child(document.document_element().id(), second);
        assert_eq!(
            document.set_image_source(second, ImageRole::Placeholder, Some(PLACEHOLDER)),
            None,
            "a settled placeholder is still nobody's event"
        );
    }

    /// A failure names its elements, where it used to name none: an `error`
    /// is owed to exactly the elements the source belongs to.
    #[test]
    fn a_failed_source_reports_its_elements() {
        let (mut document, image) = image_document();
        document.set_image_source(image, ImageRole::Source, Some(SRC));

        assert_eq!(
            document.apply_image_events(&[failed(SRC)]),
            vec![crate::ImageOutcome::Failed { node: image }]
        );
        assert!(
            document.apply_image_events(&[failed(SRC)]).is_empty(),
            "and a source reported twice moved nothing, so it owes nothing"
        );
    }

    /// Taking the last source off an element makes it an ordinary one again,
    /// natural size and all: there is no bitmap left for `DisplayMode::Leaf`
    /// to size the box from or to hide its children for.
    #[test]
    fn taking_the_last_source_off_an_element_unreplaces_it() {
        let (mut document, image) = image_document();
        document.set_image_source(image, ImageRole::Source, Some(SRC));
        document.set_image_source(image, ImageRole::Placeholder, Some(PLACEHOLDER));
        document.apply_image_events(&[loaded(SRC, 40, 20), loaded(PLACEHOLDER, 4, 4)]);

        document.set_image_source(image, ImageRole::Source, None);
        assert!(document.get(image).expect("live element").is_replaced());
        assert_eq!(
            document.natural_size(image),
            natural_size(4, 4),
            "the placeholder is all that is left, and it is what is drawn"
        );

        document.set_image_source(image, ImageRole::Placeholder, None);
        assert!(!document.get(image).expect("live element").is_replaced());
        assert_eq!(document.natural_size(image), NaturalSize::NONE);
    }

    /// The freed-node unbind covers both sources, or a load completing after
    /// the element was dropped reaches `set_natural_size`'s stale-id panic.
    #[test]
    fn freeing_an_element_unbinds_both_of_its_sources() {
        let (mut document, image) = image_document();
        document.set_image_source(image, ImageRole::Source, Some(SRC));
        document.set_image_source(image, ImageRole::Placeholder, Some(PLACEHOLDER));
        document.drop_element(image);

        assert!(
            document
                .apply_image_events(&[loaded(SRC, 40, 20), loaded(PLACEHOLDER, 4, 4)])
                .is_empty(),
            "a dropped element owes no events and has no size to set"
        );
    }

    /// Whether the node's committing parent proved its input survives any
    /// change confined to the node's own subtree — the license the in-place
    /// relayout path parks on.
    fn is_content_independent(doc: &Document<()>, id: crate::NodeId) -> bool {
        let slot = doc.live_slot(id);
        doc.layout_state()
            .get(slot)
            .is_some_and(|state| state.slot.committed_independent().is_some())
    }

    fn linear_page() -> Document<()> {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            // `overflow: hidden` is what the Lynx UA sheet gives every element,
            // and it is load-bearing here: it is what frees the flex list's
            // automatic minimum size from its content, without which the list
            // has no independent main axis for percentages to chain off.
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

    fn child_of(doc: &mut Document<()>, parent: crate::NodeId, class: &str) -> crate::NodeId {
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
        // A pinned main size and a stretched cross axis: both imposed. Linear
        // has no content-based automatic minimum, so `overflow` plays no part
        // here the way it does under flexbox.
        let row = child_of(&mut doc, list, "row");
        // Main size distributed from the row's own definite main size by
        // weight, cross size stretched to it.
        let weighted = child_of(&mut doc, row, "weighted");
        // Its main size is whatever its content measures.
        let measured = child_of(&mut doc, row, "measured");
        // A ratio ties the axes together: a definite width fixes the height.
        let ratio = child_of(&mut doc, row, "ratio");
        // A percentage main size resolves against the list's own main axis,
        // which the page imposes.
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
            // Lynx's UA `overflow: hidden` is again load-bearing: it is what
            // frees a grid item's automatic minimum from its content, which is
            // what lets a flexible track stay put under it.
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
        // Fixed column, fixed row: the area cannot move at all.
        let fixed_cell = child_of(&mut doc, grid, "");
        // Flexible column, but a pinned width keeps this item's contribution
        // out of the track's sizing.
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

    /// A skipped box is a relayout boundary — `SIZE` and `LAYOUT` both — and
    /// now that it holds a committed input it can be the root of one.
    ///
    /// Its re-run is the trivial size it already answered plus the hide sweep,
    /// so parking it costs one box model and one pass over its children, where
    /// the alternative is either reflowing from the root or leaving the box
    /// tree's change unanswered until the box stops skipping.
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

    /// How a row list skips: `content-visibility: hidden` on the rows the
    /// page keeps out, or `auto` on rows the render finds outside the encode
    /// window.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Skipping {
        Hidden,
        Auto,
    }

    /// A fixed-height scroller of `rows` rows, each holding one cell, where
    /// everything past the first `VISIBLE_ROWS` skips its contents.
    ///
    /// The scroller's own box never moves, so every row's committed input is
    /// the same input on every pass: a row's own cell changing width is the
    /// one thing dirty about the page.
    fn skipping_row_list(
        rows: usize,
        skipping: Skipping,
    ) -> (Document<()>, Vec<crate::NodeId>, crate::NodeId) {
        const VISIBLE_ROWS: usize = 2;
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        let (row_rule, skipped_rule) = match skipping {
            // `auto` is on every row; which of them skips is the render's
            // answer, and rows below the encode window are the ones it says
            // no to.
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

    /// The number this cache exists to hold down.
    ///
    /// A mutation beside the skipped rows re-runs the list, which asks every
    /// row for its box — and a skipped box's answer reads no child, so it is
    /// the same answer as last pass and comes back from the cache. Before it
    /// did, every skipped row on the page re-resolved its own box model on
    /// every pass, which is what made a long list of skipped rows more
    /// expensive to mutate beside than a list of ordinary ones.
    #[test]
    fn a_mutation_beside_skipped_rows_re_resolves_no_skipped_box() {
        for skipping in [Skipping::Hidden, Skipping::Auto] {
            let mut counts = Vec::new();
            for rows in [8_usize, 64] {
                let (mut doc, row_ids, cell) = skipping_row_list(rows, skipping);
                let settle = |doc: &mut Document<()>| match skipping {
                    // Relevance is the rendering update's answer, so an
                    // `auto` page has to render to have one at all.
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
