//! The document's **top layer** (css-position-4 §3) and the `::backdrop`
//! box each of its elements generates (§3.2).
//!
//! The layer is an ordered set of elements, generic rather than
//! dialog-specific: the embedder decides what enters it and when
//! ([`Document::add_to_top_layer`], [`Document::remove_from_top_layer`]),
//! and marks each entry with whether it **blocks the document** — HTML's
//! modal dialog, the topmost of which makes everything painted before its
//! `::backdrop` inert to hit testing. This crate owns what membership does
//! to rendering:
//!
//! - **Containing block.** A top-layer element's containing block is the initial containing block,
//!   whatever its ancestors (§3.1): the position lowering (`layout::style::resolve_position`)
//!   answers `fixed` for it, so its parent records only a static position, and
//!   [`crate::layout::anchors::containing_block_generator`] answers the initial containing block,
//!   so the rounding tail places it against the viewport with a static position of zero. The
//!   containing-block walk of its own descendants stops at it, so a `fixed` descendant escapes to
//!   the viewport rather than to a transformed ancestor outside the layer.
//! - **Paint order.** The paint-order build skips a top-layer element where its parent's collection
//!   meets it, and after the root stacking context builds each entry in layer order as a stacking
//!   context of its own — identity world, no clip chain, no space, no group layer — its
//!   `::backdrop` first ([`crate::visual`]'s build).
//! - **Inertness.** The build records the index of the first item of the topmost blocking entry as
//!   the frame's inert floor; hit testing walks reverse paint order and stops there.
//! - **Relevance.** A top-layer element is relevant to the user (css-contain-2 §4), beside the
//!   geometric test.
//!
//! # The `::backdrop` node
//!
//! Each entry owns a real element node in the arenas, created by
//! [`Document::add_to_top_layer`] and freed with the entry. It is never
//! linked under a parent, so no style traversal, selector query, child
//! list or flat-tree walk reaches it, and no script handle names it: its
//! payload slot is [`PayloadSlot::Backdrop`], which is also what marks it.
//! What makes it a box is that the layout and paint tails name it from this
//! table. A paint item for it carries
//! [`PaintItemKind::Backdrop`](crate::visual::PaintItemKind::Backdrop), which
//! hit testing reports as the originating element — a pointer on the
//! backdrop targets the dialog, as in a browser.
//!
//! Its style is Stylo's lazy pseudo-element cascade
//! (`Stylist::lazily_compute_pseudo_element_style`) with the originating
//! element's style as parent, recomputed for every entry after each style
//! flush and animation tick that ran (and after an entry was added),
//! compared with the previous one through Stylo's own damage computation,
//! and stored only when it differs — so a flush that changed nothing the
//! backdrop reads costs one cascade per entry and no layout. `::backdrop`
//! rules of every origin take part. Animations and transitions on
//! `::backdrop` itself are not run: the lazy cascade has no animation
//! declarations.
//!
//! # §3.1's computed-value fixups
//!
//! Stylo applies them (`StyleAdjuster::adjust_for_top_layer`: a position
//! other than `absolute`/`fixed` computes to `absolute`, `display: contents`
//! to its block equivalent) to a style whose `-servo-top-layer` is `auto`.
//! The fork's `lynx` build admits that longhand in a UA-origin sheet only
//! (`LYNX_UA_LONGHANDS` in `properties/data.py`), so the fixups run for an
//! element exactly when an embedder UA rule declares it — bobcat-core's
//! `dialog:modal` and `::backdrop` do. Membership is not derived from that
//! declaration and does not require it: the embedder decides what enters the
//! layer, and an element can be in it with no such rule. So membership stays
//! this crate's truth where it reads it — the position lowering answers
//! `fixed`, and a top-layer element establishes the containing block of its
//! absolutely positioned descendants whatever its computed position — and for
//! an element the UA rule reaches that only agrees with the computed value.
//! Without the rule, a top-layer element whose `display` computes to
//! `contents` generates no box and so does not render. With it, the fixup
//! blockifies `contents` to `flex`, the `lynx` grammar's initial display,
//! where a browser computes `block`.
//!
//! # Removal
//!
//! HTML's removing steps take an element out of the top layer when it
//! leaves the document. The one path every removal and move takes is
//! `Document::unlink_from_parent`; after it unlinks a node it drops the
//! entries whose element is no longer connected — a walk over the entries,
//! which are few, never over the removed subtree. Removal is immediate:
//! there is no `overlay` property and so no pending removal.
//!
//! # Cost
//!
//! A document with an empty top layer pays one `is_empty` test per lowered
//! position, per containing-block lookup, per collected child in the paint
//! build, per unlink, per flush and per layout run. No per-node field
//! exists for any of it.

use stylo::selector_parser::PseudoElement;
use stylo::servo::restyle_damage::ServoRestyleDamage;
use stylo::shared_lock::StylesheetGuards;
use stylo::stylist::RuleInclusion;
use stylo::values::computed::Content;

use crate::layout::{DisplayMode, display_mode};
use crate::style::damage::StyleDamage;
use crate::tree::document::{Document, DocumentLayoutState, NodeId, PayloadSlot, TreeArenas};
use crate::tree::node::Node;

/// One element of the top layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopLayerEntry {
    element: NodeId,
    /// The detached node that carries this entry's `::backdrop` style and
    /// box. Never handed out.
    pub(crate) backdrop: NodeId,
    blocks_document: bool,
}

impl TopLayerEntry {
    /// The element in the top layer.
    #[must_use]
    pub const fn element(&self) -> NodeId {
        self.element
    }

    /// Whether this entry blocks the document (HTML's modal dialog): while
    /// it is the topmost such entry, nothing painted before its
    /// `::backdrop` is hit-testable.
    #[must_use]
    pub const fn blocks_document(&self) -> bool {
        self.blocks_document
    }
}

/// The ordered set, bottom first.
#[derive(Debug, Default)]
pub(crate) struct TopLayer {
    entries: Vec<TopLayerEntry>,
    /// An entry was added since the backdrops were last cascaded, so the
    /// next flush cascades them even if no element needs restyling.
    restyle_pending: bool,
}

impl TopLayer {
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn entries(&self) -> &[TopLayerEntry] {
        &self.entries
    }

    pub(crate) fn contains(&self, element: NodeId) -> bool {
        self.entries.iter().any(|entry| entry.element == element)
    }

    /// The originating element of `backdrop`, when it is a backdrop node.
    pub(crate) fn origin_of(&self, backdrop: NodeId) -> Option<NodeId> {
        self.entries
            .iter()
            .find(|entry| entry.backdrop == backdrop)
            .map(|entry| entry.element)
    }

    /// Whether `node` generates a box whose containing block is the initial
    /// containing block by membership: a top-layer element, or a backdrop.
    #[inline]
    pub(crate) fn places_against_viewport(&self, node: NodeId) -> bool {
        !self.is_empty()
            && self
                .entries
                .iter()
                .any(|entry| entry.element == node || entry.backdrop == node)
    }
}

/// Whether a top-layer element generates a box this frame: it has a style,
/// its own `display` is neither `none` nor `contents`, and its last layout
/// did not hide it — the mark `display: none` and skipping ancestors leave
/// on every box under them, so no ancestor style is read here.
pub(crate) fn is_rendered<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    element: NodeId,
) -> bool {
    let Some(style) = tree.get(element).and_then(Node::layout_computed_style) else {
        return false;
    };
    if matches!(
        display_mode(*style.get_display()),
        DisplayMode::None | DisplayMode::Contents
    ) {
        return false;
    }
    state
        .get(element)
        .is_some_and(|slot| !slot.slot.is_hidden())
}

/// Whether an entry's `::backdrop` generates a box, given that its element
/// does: it has been cascaded, and neither its `display` nor its `content`
/// computes to `none`.
pub(crate) fn backdrop_generates_box<T>(tree: &TreeArenas<T>, backdrop: NodeId) -> bool {
    tree.get(backdrop)
        .and_then(Node::layout_computed_style)
        .is_some_and(|style| {
            display_mode(*style.get_display()) != DisplayMode::None
                && !matches!(style.get_counters().content, Content::None)
        })
}

impl<T> Document<T> {
    /// css-position-4 §3.3 "add an element to the top layer": `element`
    /// becomes the topmost entry — moved there if it was already in the
    /// layer — with a fresh `::backdrop`. `blocks_document` is HTML's modal
    /// flag.
    ///
    /// The caller checks that `element` is a connected element (HTML's
    /// `showModal()` throws otherwise); this asserts it.
    pub fn add_to_top_layer(&mut self, element: NodeId, blocks_document: bool) {
        assert!(
            self.get(element).is_some_and(Node::is_element),
            "Document::add_to_top_layer: not a live element"
        );
        debug_assert!(
            self.is_connected(element),
            "Document::add_to_top_layer: the element must be connected"
        );
        if self.arenas().top_layer().contains(element) {
            self.remove_from_top_layer(element);
        }
        let backdrop = self.allocate_node(PayloadSlot::Backdrop, |owner, id| {
            Node::new_element(owner, id, stylo::LocalName::from("::backdrop"))
        });
        let layer = self.arenas_mut().top_layer_mut();
        layer.entries.push(TopLayerEntry {
            element,
            backdrop,
            blocks_document,
        });
        layer.restyle_pending = true;
        self.note_top_layer_change(element);
    }

    /// Removes `element` from the top layer immediately and frees its
    /// `::backdrop`. A no-op when it is not in the layer.
    pub fn remove_from_top_layer(&mut self, element: NodeId) {
        let layer = self.arenas_mut().top_layer_mut();
        let Some(index) = layer
            .entries
            .iter()
            .position(|entry| entry.element == element)
        else {
            return;
        };
        let entry = layer.entries.remove(index);
        self.free_node(entry.backdrop);
        self.note_top_layer_change(element);
    }

    /// Whether `node` is in the top layer.
    #[must_use]
    pub fn in_top_layer(&self, node: NodeId) -> bool {
        self.arenas().top_layer().contains(node)
    }

    /// Whether `node` is in the top layer with the blocks-the-document flag.
    #[must_use]
    pub fn blocks_document(&self, node: NodeId) -> bool {
        self.arenas()
            .top_layer()
            .entries
            .iter()
            .any(|entry| entry.element == node && entry.blocks_document)
    }

    /// The top layer, bottom first.
    pub fn top_layer(&self) -> impl Iterator<Item = &TopLayerEntry> {
        self.arenas().top_layer().entries.iter()
    }

    /// The originating element of a `::backdrop` node, or `None` when `node`
    /// is not one.
    #[must_use]
    pub fn backdrop_origin(&self, node: NodeId) -> Option<NodeId> {
        self.arenas().top_layer().origin_of(node)
    }

    /// HTML's removing steps for the top layer: drops every entry whose
    /// element is no longer connected. The unlink path calls it after every
    /// removal, so the walk is over the entries, not over what was removed.
    pub(crate) fn drop_disconnected_top_layer_entries(&mut self) {
        if self.arenas().top_layer().is_empty() {
            return;
        }
        let disconnected: smallvec::SmallVec<[NodeId; 2]> = self
            .arenas()
            .top_layer()
            .entries
            .iter()
            .map(|entry| entry.element)
            .filter(|&element| !self.is_connected(element))
            .collect();
        for element in disconnected {
            self.remove_from_top_layer(element);
        }
    }

    /// What a membership flip invalidates. Membership is not in the
    /// element's style, so the harvest cannot notice it: the element's
    /// position lowers differently (its parent records a static position
    /// now, or lays it out again), and its `fixed` descendants' containing
    /// block may have moved with the walk that stops at it.
    fn note_top_layer_change(&mut self, element: NodeId) {
        self.invalidate_layout(element);
        self.invalidate_containing_block(element);
        self.note_visual_mutation();
    }
}

/// What one backdrop restyle changed.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct BackdropRestyle {
    pub(crate) restyled: usize,
    pub(crate) relayout: bool,
}

impl<T: Sync> Document<T> {
    /// Whether the next flush owes the backdrops a cascade even if no
    /// element needs restyling.
    pub(crate) fn backdrops_need_restyle(&self) -> bool {
        self.arenas().top_layer().restyle_pending
    }

    /// Cascades every entry's `::backdrop` against its element's current
    /// style and stores the ones that changed, invalidating what their
    /// damage says. An element with no style (under `display: none`, out of
    /// the flat tree) keeps its backdrop's last style; it renders nothing,
    /// so nothing reads it.
    pub(crate) fn restyle_backdrops(&mut self) -> BackdropRestyle {
        let mut outcome = BackdropRestyle::default();
        if self.arenas().top_layer().is_empty() {
            return outcome;
        }
        self.arenas_mut().top_layer_mut().restyle_pending = false;
        for index in 0..self.arenas().top_layer().entries.len() {
            let entry = self.arenas().top_layer().entries[index];
            let Some(style) = self.cascade_backdrop(entry) else {
                continue;
            };
            // A first style lays the box out for the first time.
            let relayout = {
                let backdrop = self.arenas().live(entry.backdrop);
                match backdrop.layout_computed_style() {
                    None => true,
                    Some(old) => {
                        let difference =
                            ServoRestyleDamage::compute_style_difference::<&Node<T>>(old, &style);
                        let Some(damage) = StyleDamage::from_style_change(difference.damage, false)
                        else {
                            continue;
                        };
                        damage.needs_relayout()
                    }
                }
            };
            outcome.restyled += 1;
            outcome.relayout |= relayout;
            self.arenas_mut()
                .get_mut(entry.backdrop)
                .expect("an entry's backdrop lives as long as the entry")
                .replace_pseudo_style(style);
            if relayout {
                // The tail lays the backdrop out on every run; a cleared
                // cache is what makes it compute again rather than answer
                // from the old style. Nothing above it exists to clear.
                self.layout_state_mut().clear_box_cache(entry.backdrop);
                self.mark_layout_dirty(false);
            }
            self.note_visual_mutation();
        }
        outcome
    }

    /// The lazy `::backdrop` cascade for one entry, with its element's
    /// primary style as parent.
    fn cascade_backdrop(
        &self,
        entry: TopLayerEntry,
    ) -> Option<stylo::servo_arc::Arc<stylo::properties::ComputedValues>> {
        let element = self.arenas().get(entry.element)?;
        let primary = element.layout_computed_style()?;
        let engine = self.style_engine();
        let guard = engine.shared_lock().read();
        let guards = StylesheetGuards::same(&guard);
        engine.stylist().lazily_compute_pseudo_element_style(
            &guards,
            element,
            &PseudoElement::Backdrop,
            RuleInclusion::All,
            primary,
            false,
            None,
        )
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use crate::NodeId;
    use crate::test_common::Doc;

    /// The parts of bobcat-core's UA sheet these tests read.
    const UA: &str = "
        dialog { display: flex; position: absolute; }
        dialog.modal { position: fixed; }
        ::backdrop { display: flex; position: fixed; inset: 0; }
        dialog::backdrop { background-color: rgba(0, 0, 0, 0.1); }";

    fn page(css: &str) -> (Doc, NodeId) {
        let mut doc = Doc::with_css(&format!(
            "page {{ display: flex; width: 800px; height: 600px; }} {css}"
        ));
        doc.add_ua_css(UA);
        let dialog = doc.el(doc.root, "dialog.modal");
        (doc, dialog)
    }

    fn backdrop_of(doc: &Doc, element: NodeId) -> NodeId {
        doc.dom
            .arenas()
            .top_layer()
            .entries()
            .iter()
            .find(|entry| entry.element() == element)
            .expect("in the top layer")
            .backdrop
    }

    fn rect(doc: &Doc, id: NodeId) -> (f32, f32, f32, f32) {
        let layout = doc.dom.rounded_layout(id).expect("laid out");
        (
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        )
    }

    #[test]
    fn the_ua_default_and_an_author_rule_both_reach_the_backdrop() {
        let (mut doc, dialog) =
            page("dialog::backdrop { border-top-width: 7px; border-top-style: solid; }");
        doc.dom.add_to_top_layer(dialog, true);
        doc.flush();
        let backdrop = backdrop_of(&doc, dialog);
        assert_eq!(doc.value(backdrop, "position"), "fixed", "UA origin");
        assert_eq!(doc.value(backdrop, "top"), "0px", "UA origin");
        assert_eq!(
            doc.value(backdrop, "background-color"),
            "rgba(0, 0, 0, 0.1)"
        );
        assert_eq!(
            doc.value(backdrop, "border-top-width"),
            "7px",
            "author origin"
        );

        doc.add_css("dialog::backdrop { background-color: rgb(1, 2, 3); top: 10px; }");
        doc.flush();
        assert_eq!(doc.value(backdrop, "background-color"), "rgb(1, 2, 3)");
        assert_eq!(doc.value(backdrop, "top"), "10px", "author over UA");
    }

    #[test]
    fn the_backdrop_inherits_from_its_element_and_follows_its_restyle() {
        let (mut doc, dialog) = page(
            "dialog { color: rgb(10, 20, 30); font-size: 21px; }
             dialog.warm { color: rgb(200, 0, 0); }",
        );
        doc.dom.add_to_top_layer(dialog, true);
        doc.flush();
        let backdrop = backdrop_of(&doc, dialog);
        assert_eq!(doc.value(backdrop, "color"), "rgb(10, 20, 30)");
        assert_eq!(doc.value(backdrop, "font-size"), "21px");
        doc.add_class(dialog, "warm");
        doc.flush();
        assert_eq!(doc.value(backdrop, "color"), "rgb(200, 0, 0)");
    }

    #[test]
    fn a_modal_state_rule_on_the_backdrop_restyles_with_the_state() {
        let (mut doc, dialog) =
            page("dialog:modal::backdrop { background-color: rgb(0, 0, 255); }");
        doc.dom.add_to_top_layer(dialog, true);
        doc.flush();
        let backdrop = backdrop_of(&doc, dialog);
        assert_eq!(
            doc.value(backdrop, "background-color"),
            "rgba(0, 0, 0, 0.1)"
        );
        doc.dom
            .add_element_state(dialog, crate::ElementState::MODAL);
        doc.flush();
        assert_eq!(doc.value(backdrop, "background-color"), "rgb(0, 0, 255)");
    }

    #[test]
    fn the_backdrop_covers_the_viewport_and_follows_insets_and_resizes() {
        let (mut doc, dialog) = page(".modal { width: 100px; height: 50px; }");
        doc.dom.add_to_top_layer(dialog, true);
        doc.flush();
        let backdrop = backdrop_of(&doc, dialog);
        assert_eq!(rect(&doc, backdrop), (0.0, 0.0, 800.0, 600.0));

        doc.add_css("dialog::backdrop { inset: 10px; }");
        doc.flush();
        assert_eq!(rect(&doc, backdrop), (10.0, 10.0, 780.0, 580.0));

        doc.dom.set_viewport(400.0, 300.0);
        doc.flush();
        assert_eq!(rect(&doc, backdrop), (10.0, 10.0, 380.0, 280.0));
    }

    #[test]
    fn an_element_that_does_not_render_paints_no_backdrop() {
        let (mut doc, dialog) = page(".gone { display: none; }");
        doc.dom.add_to_top_layer(dialog, true);
        let paint = doc.dom.build_paint_order();
        assert!(
            paint
                .items()
                .iter()
                .any(|item| matches!(item.kind, crate::visual::PaintItemKind::Backdrop { .. }))
        );
        let wrapper = doc.el(doc.root, "view.gone");
        doc.dom.append_child(wrapper, dialog);
        // A move leaves the top layer (HTML's removing steps run).
        assert!(!doc.dom.in_top_layer(dialog));
        doc.dom.add_to_top_layer(dialog, true);
        let paint = doc.dom.build_paint_order();
        assert!(
            paint
                .items()
                .iter()
                .all(|item| item.node != dialog && item.node != backdrop_of(&doc, dialog)),
            "nothing under a `display: none` ancestor paints"
        );
        assert_eq!(
            paint.inert_floor(),
            Some(paint.items().len()),
            "still blocks"
        );
    }

    #[test]
    fn content_none_suppresses_the_backdrop_box() {
        let (mut doc, dialog) = page("dialog::backdrop { content: none; }");
        doc.dom.add_to_top_layer(dialog, true);
        let paint = doc.dom.build_paint_order();
        assert!(
            paint
                .items()
                .iter()
                .all(|item| !matches!(item.kind, crate::visual::PaintItemKind::Backdrop { .. }))
        );
    }

    #[test]
    fn an_empty_top_layer_cascades_no_backdrop() {
        let (mut doc, _) = page("");
        doc.flush();
        assert!(!doc.dom.backdrops_need_restyle());
        assert_eq!(doc.dom.restyle_backdrops().restyled, 0);
    }
}
