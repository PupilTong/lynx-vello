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
//!   whatever its ancestors (§3.1): the position lowering ([`crate::layout::resolve_position`])
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
//! rules of every origin take part, and the UA-only `-servo-top-layer:
//! auto` makes Stylo's adjuster apply §3.1's computed-value fixups to it.
//! Animations and transitions on `::backdrop` itself are not run: the lazy
//! cascade has no animation declarations.
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
