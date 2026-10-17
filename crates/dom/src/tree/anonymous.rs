//! Anonymous block boxes around loose text.
//!
//! Text outside a paragraph is laid out the way css-flexbox-1 §4 and
//! css-grid-1 §6 lay out the text children of a flex or grid container — the
//! rule css-grid-3's `grid-lanes` items follow, and the one the Lynx `linear`
//! and `relative` modes take too, having none of their own: each contiguous
//! sequence of it is wrapped in an anonymous block container, which is then
//! one item like any element child, and a sequence holding nothing but white
//! space renders nothing at all. The block container is the internal
//! `(block, flow)` display, which lays its text out as a CSS inline
//! formatting context ([`crate::layout::inline_flow`]) — not as a
//! `display: -lynx-text` paragraph, whose Lynx semantics a CSS anonymous box
//! does not have.
//!
//! # Sources and sequences
//!
//! Text reaches an item container from two kinds of node, its **sources**:
//!
//! - a text node;
//! - a `display: contents` element whose `content` list replaces its children — how a carrier
//!   element supplies its text as generated content rather than as a child.
//!
//! A **sequence** is a run of sources with no box between them. Adjacency is
//! the box tree's: a `display: contents` element is transparent, so the
//! sources inside it join the sequence around it; a `display: none` element
//! generates nothing and so ends nothing; every element that generates a box
//! — in flow or out of it — ends the sequence before it. Whether a sequence
//! renders is css-flexbox-1 §4's test: whether any source holds a character
//! other than white space (space, tab, line feed, carriage return).
//!
//! # The anonymous node
//!
//! Each rendering sequence is boxed by a node in the arenas, as a
//! `::backdrop` is ([`crate::tree::top_layer`]): an element node no embedder
//! created, payload [`PayloadSlot::Anonymous`], never in any child list, so no
//! style traversal, selector match or DOM walk reaches it, and no script
//! handle names it. Two links make it a box, and neither is mirrored:
//!
//! - Its `parent` leads to the container: the container itself, or the shadow root of a container
//!   that hosts one — the parent whose flat-tree parent is the container either way. Every walk up
//!   from the box — layout invalidation, `box_parent`, the containing-block lookups — reaches the
//!   container, though the container does not list it.
//! - Its `children` are the sources, in tree order. The inline formatting context's content walk
//!   reads them as any block container's children, though each source's parent is still its DOM
//!   parent — which is what keeps a run's style the style of the element the text is in
//!   (`TextRunView`).
//!
//! Its style is the one CSS 2.2 §9.2.1.1 gives an anonymous box: every
//! inherited property from the container, the initial value of every other
//! property, and the block container display. It is built rather than
//! cascaded ([`Document::anonymous_block_style`]): no rule can select an
//! anonymous box.
//!
//! # Layout children
//!
//! Layout reads a node's children through `LayoutTree::children`, and a node
//! whose flat children include a source answers an override instead: the
//! same children with every source removed and each box inserted where its
//! first source was. The override sits on the node whose flat children hold
//! the source — the container, or a `display: contents` element inside it —
//! so a transparent box is still walked exactly as before, and a box lands at
//! the level of its first source, which the flattening walk then places at
//! the sequence's position. No source reaches an item algorithm, so a text
//! node never lays out as an empty item that takes a `gap` and a grid cell.
//!
//! # When the boxes are re-derived
//!
//! After the style flush and after each animation tick: the derivation reads
//! `display` and `content`, which only a flush computes, and the box style
//! inherits from the container's. Mutations record a node, and
//! [`Document::rebuild_anonymous_boxes`] re-derives the boxes of every item
//! container the records resolve to:
//!
//! - a node's child list or slot assignment changed (the node);
//! - a source's text data changed (the text node);
//! - an element was styled for the first or the last time, or its `display` or `content` changed
//!   (the element and its flat parent: the text on both sides of it may group differently);
//! - an element that is a container, an overridden node or a source was restyled at all (the
//!   element: the box style may have moved with it).
//!
//! Every styled item container with a source has a derivation, so while no
//! container has one, only a mutation that can give a container its first
//! source records:
//!
//! - a text node, or a styled `display: contents` element, inserted under a node that may hold
//!   loose text — not a styled paragraph, replaced element or `display: none` box;
//! - an element styled for the first time, or with a new `display` or `content`, that holds loose
//!   text itself, or that is a `display: contents` element passing text or generated content up to
//!   such a node.
//!
//! A removal records nothing then: it can neither split nor end a sequence.
//!
//! A record resolves to the nearest item container at or above it, through
//! box-less ancestors, when the boxes are re-derived rather than when it is
//! made, because only then is every style it reads current. A derivation
//! reuses the container's boxes in order, so a box keeps its node, and its
//! cached layout where nothing it holds moved.
//!
//! # Hit testing
//!
//! A hit on an anonymous box targets its container
//! ([`Document::hit_target`]): an event's path starts at an element the
//! embedder knows. A paragraph is hit as a whole here — a `-lynx-text`
//! paragraph's hit targets its element, not the run under the point — so
//! the container is the element that owns every run the box shows.
//!
//! # Cost
//!
//! A document whose text is all in paragraphs records nothing: per mutation
//! it pays an emptiness test and, for an inserted text node or a box-shape
//! restyle, a look at one style, plus for a restyled item container one pass
//! over its flat children. Every document pays one flag test per
//! `LayoutTree::children` call (a node's `may_override_children`; only a node
//! that holds loose text looks its override up) and one emptiness test per
//! freed node. Once a container has sources, the document pays one push per
//! child-list change and box-shape restyle, and per edit of a source; after a
//! flush that recorded anything, one walk up per record and, for each
//! container the records resolve to, one pass over its flat children — which
//! ends there unless one is a text node or a `display: contents` element.

use std::sync::LazyLock;

use rustc_hash::FxHashMap;
use stylo::LocalName;
use stylo::properties::{ComputedValues, StyleBuilder};
use stylo::servo::restyle_damage::ServoRestyleDamage;
use stylo::servo_arc::Arc;
use stylo::values::computed::ContentItem;

use crate::layout::paragraph::generated_content;
use crate::layout::{DisplayMode, block_flow_display, display_mode};
use crate::style::damage::StyleDamage;
use crate::tree::document::{Document, NodeId, PayloadSlot, TreeArenas};
use crate::tree::node::{Node, StyleRefresh};

/// The local name an anonymous box's node carries. No selector can name it:
/// a type selector cannot start with `::`.
static ANONYMOUS_BOX: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("::anonymous-box"));

/// The anonymous boxes, keyed three ways for their three readers.
///
/// An overridden node and a source each record the container that owns the
/// entry: a node moved from one container to another is claimed by the one
/// derived second, and the other must not take the entry back when it is
/// derived after it.
#[derive(Debug, Default)]
pub(crate) struct AnonymousBoxes {
    /// What each item container with sources generated.
    containers: FxHashMap<NodeId, Generated>,
    /// The children `LayoutTree::children` answers in place of a node's flat
    /// children.
    layout_children: FxHashMap<NodeId, Override>,
    /// Each source, and the box its sequence is in — `None` for a sequence
    /// that renders nothing.
    sources: FxHashMap<NodeId, Source>,
    /// Records not yet resolved: a node, and whether what moved can change
    /// the size of a box in its container.
    pending: Vec<(NodeId, bool)>,
}

/// One container's derivation.
#[derive(Debug, Default)]
struct Generated {
    /// The container's style the boxes' style was built from, and that box
    /// style: a derivation whose container still has that style `Arc` reuses
    /// it rather than building and comparing another.
    style: Option<(Arc<ComputedValues>, Arc<ComputedValues>)>,
    /// Its boxes, in tree order.
    boxes: Vec<NodeId>,
    /// The nodes whose layout children it overrides.
    overridden: Vec<NodeId>,
    /// Its sources, rendering or not.
    sources: Vec<NodeId>,
}

/// Overrides as a list: each overridden node and the children layout reads
/// for it.
type Overrides = Vec<(NodeId, Box<[NodeId]>)>;

/// A derivation taken out of the tables, with the overrides it still owned.
#[derive(Debug, Default)]
struct Taken {
    generated: Generated,
    overrides: Overrides,
}

#[derive(Debug)]
struct Override {
    container: NodeId,
    children: Box<[NodeId]>,
}

#[derive(Debug, Clone, Copy)]
struct Source {
    container: NodeId,
    anonymous: Option<NodeId>,
}

impl AnonymousBoxes {
    /// The children layout reads for `node` in place of its flat children:
    /// `Some` exactly when its flat children hold a source.
    #[inline]
    pub(crate) fn layout_children(&self, node: NodeId) -> Option<&[NodeId]> {
        if self.layout_children.is_empty() {
            return None;
        }
        self.layout_children
            .get(&node)
            .map(|entry| &*entry.children)
    }

    /// The anonymous box whose paragraph holds `source`'s text.
    #[inline]
    pub(crate) fn box_of(&self, source: NodeId) -> Option<NodeId> {
        if self.sources.is_empty() {
            return None;
        }
        self.sources.get(&source)?.anonymous
    }

    /// Whether no container has sources. The other two tables are only ever
    /// filled by a container's derivation, so this answers for all three.
    #[inline]
    fn is_empty(&self) -> bool {
        self.containers.is_empty()
    }

    fn involves(&self, node: NodeId) -> bool {
        self.containers.contains_key(&node)
            || self.layout_children.contains_key(&node)
            || self.sources.contains_key(&node)
    }

    /// Takes `container`'s derivation out, along with the overrides and
    /// sources it still owns.
    fn take(&mut self, container: NodeId) -> Option<Taken> {
        let generated = self.containers.remove(&container)?;
        let mut overrides = Vec::with_capacity(generated.overridden.len());
        for &node in &generated.overridden {
            if self
                .layout_children
                .get(&node)
                .is_some_and(|entry| entry.container == container)
                && let Some(entry) = self.layout_children.remove(&node)
            {
                overrides.push((node, entry.children));
            }
        }
        for source in &generated.sources {
            if self
                .sources
                .get(source)
                .is_some_and(|entry| entry.container == container)
            {
                self.sources.remove(source);
            }
        }
        Some(Taken {
            generated,
            overrides,
        })
    }
}

/// One sequence of a container's sources.
struct Sequence {
    sources: Vec<NodeId>,
    renders: bool,
}

/// The item container whose loose text `node`'s children are: `node` itself
/// when it lays its children out as items, else the one its box-less
/// ancestors lead to. `None` where that text renders as something else or
/// not at all — a paragraph's content, a replaced element's ignored
/// children, a subtree with no style.
fn item_container<T>(tree: &TreeArenas<T>, node: NodeId) -> Option<NodeId> {
    let mut current = tree.get(node)?;
    loop {
        if current.is_element() {
            let style = current.layout_computed_style()?;
            match display_mode(*style.get_display()) {
                DisplayMode::Contents => {}
                DisplayMode::Flex
                | DisplayMode::Grid
                | DisplayMode::GridLanes
                | DisplayMode::Linear
                | DisplayMode::Relative => {
                    return (!current.is_replaced()).then(|| current.id());
                }
                DisplayMode::None
                | DisplayMode::Text
                | DisplayMode::BlockFlow
                | DisplayMode::Leaf => return None,
            }
        } else if !current.is_text_node() && !current.is_shadow_root() {
            return None;
        }
        current = current.flat_parent()?;
    }
}

/// Whether text whose parent is `node` can be loose text: `node` is not a
/// styled element — its display is unknown yet, or it is a shadow root whose
/// host decides — or it is an item container or `display: contents`. Text in
/// a paragraph, in a replaced element or under a box that renders nothing
/// never is.
fn may_hold_loose_text<T>(node: &Node<T>) -> bool {
    if !node.is_element() {
        return true;
    }
    let Some(style) = node.layout_computed_style() else {
        return true;
    };
    !node.is_replaced()
        && matches!(
            display_mode(*style.get_display()),
            DisplayMode::Contents
                | DisplayMode::Flex
                | DisplayMode::Grid
                | DisplayMode::GridLanes
                | DisplayMode::Linear
                | DisplayMode::Relative
        )
}

/// Whether `element`, just styled for the first time or with a new
/// `display` or `content`, can have given an item container its first
/// source while no container had any: it holds loose text itself — a text
/// child, or a `display: contents` child that may hold some — or it is a
/// `display: contents` element with such children or generated content,
/// under a node that may hold loose text. Its record resolves to the
/// container either way.
fn brings_loose_text<T>(tree: &TreeArenas<T>, element: NodeId) -> bool {
    let Some(node) = tree.get(element) else {
        return false;
    };
    let Some(style) = node.layout_computed_style() else {
        return false;
    };
    // The children's styles are read live: the harvest that calls this
    // refreshes an element's layout snapshot before its children's.
    let holds_text = || {
        node.flat_children().iter().any(|&child| {
            let child = tree.at(child);
            child.is_text_node()
                || (child.is_element()
                    && child.borrow_computed_style().is_none_or(|data| {
                        data.styles
                            .primary
                            .as_ref()
                            .is_none_or(|style| style.get_display().is_contents())
                    }))
        })
    };
    if style.get_display().is_contents() {
        node.flat_parent().is_some_and(may_hold_loose_text)
            && (generated_content(node).is_some() || holds_text())
    } else {
        may_hold_loose_text(node) && holds_text()
    }
}

/// Whether `text` holds a character css-flexbox-1 §4 does not count as white
/// space.
fn has_rendered_character(text: &str) -> bool {
    text.chars()
        .any(|character| !matches!(character, ' ' | '\t' | '\n' | '\r'))
}

/// `container`'s sources, split into sequences, in tree order.
fn sequences<T>(tree: &TreeArenas<T>, container: NodeId) -> Vec<Sequence> {
    // Text reaches the container only as a text node or through a
    // `display: contents` child, so a container with neither — every
    // container a page of plain element boxes has — answers here.
    let may_hold_text = tree.live(container).flat_children().iter().any(|&child| {
        let child = tree.at(child);
        child.is_text_node()
            || child
                .layout_computed_style()
                .is_some_and(|style| style.get_display().is_contents())
    });
    if !may_hold_text {
        return Vec::new();
    }
    let mut sequences = Vec::new();
    let mut open: Option<Sequence> = None;
    let mut stack: Vec<NodeId> = tree
        .live(container)
        .flat_children()
        .iter()
        .rev()
        .copied()
        .collect();
    while let Some(id) = stack.pop() {
        let node = tree.at(id);
        let renders = if node.is_text_node() {
            node.text().is_some_and(has_rendered_character)
        } else if node.is_element() {
            let Some(style) = node.layout_computed_style() else {
                continue;
            };
            match display_mode(*style.get_display()) {
                DisplayMode::None => continue,
                DisplayMode::Contents => {
                    let Some((_, items)) = generated_content(node) else {
                        stack.extend(node.flat_children().iter().rev().copied());
                        continue;
                    };
                    items.iter().any(|item| {
                        matches!(item, ContentItem::String(text) if has_rendered_character(text))
                    })
                }
                _ => {
                    sequences.extend(open.take());
                    continue;
                }
            }
        } else {
            continue;
        };
        let sequence = open.get_or_insert_with(|| Sequence {
            sources: Vec::new(),
            renders: false,
        });
        sequence.sources.push(id);
        sequence.renders |= renders;
    }
    sequences.extend(open.take());
    sequences
}

impl<T> Document<T> {
    /// Records that the loose text of the item container `node` is in —
    /// `node` itself when it is one — may group differently: `node`'s child
    /// list or slot assignment changed, or, for a text node, its data.
    pub(crate) fn note_loose_text_change(&mut self, node: NodeId) {
        self.arenas_mut().anonymous_mut().pending.push((node, true));
    }

    /// The record for `child` inserted under `parent` (module doc, "When the
    /// boxes are re-derived").
    pub(crate) fn note_child_inserted(&mut self, parent: NodeId, child: NodeId) {
        let tree = self.arenas();
        let records = !tree.anonymous().is_empty() || {
            let child = tree.at(child);
            (child.is_text_node()
                || child
                    .layout_computed_style()
                    .is_some_and(|style| style.get_display().is_contents()))
                && may_hold_loose_text(tree.at(parent))
        };
        if records {
            self.note_loose_text_change(parent);
        }
    }

    /// The record for a child removed from `parent`: none while no container
    /// has sources, since a removal can then neither split nor end one.
    pub(crate) fn note_child_removed(&mut self, parent: NodeId) {
        if !self.arenas().anonymous().is_empty() {
            self.note_loose_text_change(parent);
        }
    }

    /// The record for a text node whose data changed: only a source's edit
    /// can make its sequence start or stop rendering.
    pub(crate) fn note_text_data_change(&mut self, text: NodeId) {
        if self.arenas().anonymous().sources.contains_key(&text) {
            self.note_loose_text_change(text);
        }
    }

    /// The harvest's record for an element it visited (module doc, "When the
    /// boxes are re-derived"). `relayout` is whether its damage reaches
    /// layout. Called whatever the damage: a first or a last style carries
    /// none.
    pub(crate) fn note_box_restyle(
        &mut self,
        element: NodeId,
        refresh: StyleRefresh,
        relayout: bool,
    ) {
        if !refresh.changed && !relayout {
            return;
        }
        if refresh.box_tree_changed {
            if self.arenas().anonymous().is_empty() {
                if brings_loose_text(self.arenas(), element) {
                    self.note_loose_text_change(element);
                }
                return;
            }
            let parent = self.get(element).and_then(Node::flat_parent_id);
            let pending = &mut self.arenas_mut().anonymous_mut().pending;
            pending.push((element, true));
            pending.extend(parent.map(|parent| (parent, true)));
        } else {
            let anonymous = self.arenas().anonymous();
            if !anonymous.is_empty() && anonymous.involves(element) {
                self.arenas_mut()
                    .anonymous_mut()
                    .pending
                    .push((element, relayout));
            }
        }
    }

    /// Drops what `id` was to the anonymous boxes — called by `free_node` for
    /// every node it frees. A container's boxes are freed with it; an
    /// overridden node's or a source's entry goes, and its container, which
    /// the unlink before the free recorded, drops it from its derivation the
    /// next time it is derived.
    pub(crate) fn forget_anonymous_boxes(&mut self, id: NodeId) {
        if self.arenas().anonymous().is_empty() {
            return;
        }
        let anonymous = self.arenas_mut().anonymous_mut();
        anonymous.layout_children.remove(&id);
        anonymous.sources.remove(&id);
        if let Some(Taken {
            generated,
            overrides,
        }) = anonymous.take(id)
        {
            self.clear_override_flags(&overrides, &[]);
            for anonymous_box in generated.boxes {
                self.free_node(anonymous_box);
            }
        }
    }

    /// Takes `container`'s derivation away, freeing its boxes, and answers
    /// whether it had generated anything layout read.
    fn release_container(&mut self, container: NodeId) -> bool {
        let Some(Taken {
            generated,
            overrides,
        }) = self.arenas_mut().anonymous_mut().take(container)
        else {
            return false;
        };
        self.clear_override_flags(&overrides, &[]);
        let generated_any = !generated.boxes.is_empty() || !generated.overridden.is_empty();
        for anonymous_box in generated.boxes {
            self.free_node(anonymous_box);
        }
        if generated_any && self.contains_node(container) {
            self.invalidate_layout(container);
            self.note_visual_mutation();
        }
        generated_any
    }

    /// Clears the override flag of every node `taken` overrode that `kept`
    /// does not list. A node whose free is in progress is skipped.
    fn clear_override_flags(&mut self, taken: &Overrides, kept: &[NodeId]) {
        for (holder, _) in taken {
            if !kept.contains(holder)
                && let Some(node) = self.arenas_mut().get_mut(*holder)
            {
                node.may_override_children = false;
            }
        }
    }

    fn allocate_anonymous_box(&mut self) -> NodeId {
        self.allocate_node(PayloadSlot::Anonymous, |owner, id| {
            Node::new_element(owner, id, ANONYMOUS_BOX.clone())
        })
    }

    /// CSS 2.2 §9.2.1.1's style for an anonymous block box whose parent box
    /// has style `parent`: its inherited properties, every other property's
    /// initial value, and the block container display.
    pub(crate) fn anonymous_block_style(&self, parent: &ComputedValues) -> Arc<ComputedValues> {
        let stylist = self.style_engine().stylist();
        let mut builder =
            StyleBuilder::for_inheritance(stylist.device(), Some(stylist), Some(parent), None);
        builder.mutate_box().set_display(block_flow_display());
        builder.build()
    }

    /// Test/benchmark-only: how many anonymous boxes `container`'s last
    /// derivation gave its loose text.
    #[cfg(feature = "layout-test-utils")]
    #[doc(hidden)]
    pub fn anonymous_box_count_for_testing(&self, container: NodeId) -> usize {
        self.arenas()
            .anonymous()
            .containers
            .get(&container)
            .map_or(0, |generated| generated.boxes.len())
    }

    /// The element a hit on `node` targets: an anonymous box's container,
    /// `node` itself otherwise.
    pub(crate) fn hit_target(&self, node: NodeId) -> NodeId {
        self.arenas().anonymous_container(node).unwrap_or(node)
    }
}

impl<T: Sync> Document<T> {
    /// Re-derives the anonymous boxes of every item container a record since
    /// the last call resolves to, and answers whether a box's size may have
    /// moved. One test when nothing was recorded.
    pub(crate) fn rebuild_anonymous_boxes(&mut self) -> bool {
        if self.arenas().anonymous().pending.is_empty() {
            return false;
        }
        let pending = std::mem::take(&mut self.arenas_mut().anonymous_mut().pending);
        let mut containers: Vec<(NodeId, bool)> = Vec::new();
        let mut released: Vec<NodeId> = Vec::new();
        {
            let tree = self.arenas();
            for (node, relayout) in pending {
                let container = item_container(tree, node);
                if let Some(container) = container {
                    containers.push((container, relayout));
                }
                // A container that stopped being one: its text belongs to
                // another container now, or renders as something else.
                if container != Some(node) && tree.anonymous().containers.contains_key(&node) {
                    released.push(node);
                }
            }
        }
        // Sorted by handle so the boxes a pass allocates — and so their
        // handles — do not depend on the order the records came in.
        containers.sort_unstable_by_key(|&(container, _)| container.to_bits());
        containers.dedup_by(|later, kept| {
            let same = later.0 == kept.0;
            if same {
                kept.1 |= later.1;
            }
            same
        });
        released.sort_unstable_by_key(|container| container.to_bits());
        released.dedup();
        let mut relayout = false;
        for container in released {
            relayout |= self.release_container(container);
        }
        for (container, moved) in containers {
            relayout |= self.derive_anonymous_boxes(container, moved);
        }
        relayout
    }

    /// Re-derives one container's boxes and overrides, and answers whether
    /// its layout was invalidated. `moved` is whether a record that resolved
    /// to it can have changed a box's size, which re-derivation alone cannot
    /// see: a run's style is its element's, not the box's.
    fn derive_anonymous_boxes(&mut self, container: NodeId, moved: bool) -> bool {
        let sequences = sequences(self.arenas(), container);
        if sequences.is_empty() {
            return self.release_container(container);
        }
        let Taken {
            generated: previous,
            overrides: previous_overrides,
        } = self
            .arenas_mut()
            .anonymous_mut()
            .take(container)
            .unwrap_or_default();
        let placed = self.place_anonymous_boxes(
            container,
            &sequences,
            &previous.boxes,
            previous.style.as_ref(),
            moved,
        );
        let (sources, overrides) = overrides(self.arenas(), &sequences, &placed.owners);

        // Any override that appeared, went or changed moves what layout
        // reads; the ones the previous derivation owned were taken out with
        // it above.
        let relayout = placed.relayout
            || previous_overrides.len() != overrides.len()
            || overrides.iter().any(|(holder, children)| {
                !previous_overrides
                    .iter()
                    .any(|(previous, old)| previous == holder && old == children)
            });
        let overridden: Vec<NodeId> = overrides.iter().map(|(holder, _)| *holder).collect();
        self.clear_override_flags(&previous_overrides, &overridden);
        for &holder in &overridden {
            self.live_node_mut(holder).may_override_children = true;
        }
        let anonymous = self.arenas_mut().anonymous_mut();
        for (holder, children) in overrides {
            anonymous.layout_children.insert(
                holder,
                Override {
                    container,
                    children,
                },
            );
        }
        for (sequence, owner) in sequences.iter().zip(&placed.owners) {
            for &source in &sequence.sources {
                anonymous.sources.insert(
                    source,
                    Source {
                        container,
                        anonymous: *owner,
                    },
                );
            }
        }
        anonymous.containers.insert(
            container,
            Generated {
                style: Some(placed.style),
                boxes: placed.boxes,
                overridden,
                sources,
            },
        );

        if relayout {
            self.invalidate_layout(container);
        }
        if relayout || placed.restyled {
            self.note_visual_mutation();
        }
        relayout
    }

    /// Gives each rendering sequence a box — reusing `previous` in order,
    /// allocating past its end, freeing what is left of it — with its links
    /// and style set, and clears the cached layout of every box whose
    /// content or style can have moved its size.
    fn place_anonymous_boxes(
        &mut self,
        container: NodeId,
        sequences: &[Sequence],
        previous: &[NodeId],
        previous_style: Option<&(Arc<ComputedValues>, Arc<ComputedValues>)>,
        moved: bool,
    ) -> Placed {
        let container_style = Arc::clone(
            self.live(container)
                .layout_computed_style_arc()
                .expect("an item container has a style"),
        );
        let style = match previous_style {
            Some((parent, style)) if Arc::ptr_eq(parent, &container_style) => Arc::clone(style),
            _ => self.anonymous_block_style(&container_style),
        };
        // The parent whose flat-tree parent is the container (module doc).
        let parent = {
            let node = self.live(container);
            node.shadow_root_slot()
                .unwrap_or_else(|| self.live_slot(container))
        };
        let mut placed = Placed {
            style: (container_style, Arc::clone(&style)),
            boxes: Vec::new(),
            owners: Vec::with_capacity(sequences.len()),
            relayout: moved,
            restyled: false,
        };
        for sequence in sequences {
            if !sequence.renders {
                placed.owners.push(None);
                continue;
            }
            let anonymous_box = if let Some(&reused) = previous.get(placed.boxes.len()) {
                reused
            } else {
                placed.relayout = true;
                self.allocate_anonymous_box()
            };
            let node = self.live_node_mut(anonymous_box);
            node.parent = Some(parent);
            let regrouped = node.children != sequence.sources;
            if regrouped {
                node.children.clone_from(&sequence.sources);
            }
            // `Some(relayout)` when the style moved, `None` when it did not.
            let damage = match node.layout_computed_style_arc() {
                None => Some(true),
                Some(old) if Arc::ptr_eq(old, &style) => None,
                Some(old) => StyleDamage::from_style_change(
                    ServoRestyleDamage::compute_style_difference::<&Node<T>>(old, &style).damage,
                    false,
                )
                .map(StyleDamage::needs_relayout),
            };
            if damage.is_some() {
                node.replace_pseudo_style(style.clone());
                placed.restyled = true;
            }
            if moved || regrouped || damage == Some(true) {
                placed.relayout = true;
                self.layout_state_mut().clear_box_cache(anonymous_box);
            }
            placed.owners.push(Some(anonymous_box));
            placed.boxes.push(anonymous_box);
        }
        if let Some(surplus) = previous.get(placed.boxes.len()..) {
            placed.relayout |= !surplus.is_empty();
            for &anonymous_box in surplus {
                self.free_node(anonymous_box);
            }
        }
        placed
    }
}

/// What [`Document::place_anonymous_boxes`] did.
struct Placed {
    /// The container's style and the box style built from it.
    style: (Arc<ComputedValues>, Arc<ComputedValues>),
    /// The boxes, in tree order.
    boxes: Vec<NodeId>,
    /// Each sequence's box, `None` for one that renders nothing; indexed like
    /// the sequences.
    owners: Vec<Option<NodeId>>,
    /// Whether a box appeared, went, or may have changed size.
    relayout: bool,
    /// Whether a box took a new style.
    restyled: bool,
}

/// Every source, in tree order, and the override of each node whose flat
/// children hold one: those children with every source removed and each box
/// where its first source was.
fn overrides<T>(
    tree: &TreeArenas<T>,
    sequences: &[Sequence],
    owners: &[Option<NodeId>],
) -> (Vec<NodeId>, Overrides) {
    let mut sources = Vec::new();
    // Each source, and the box it opens: `Some` only for a sequence's first.
    let mut opens: FxHashMap<NodeId, Option<NodeId>> = FxHashMap::default();
    let mut holders: Vec<NodeId> = Vec::new();
    for (sequence, owner) in sequences.iter().zip(owners) {
        for (index, &source) in sequence.sources.iter().enumerate() {
            sources.push(source);
            opens.insert(source, if index == 0 { *owner } else { None });
            let holder = tree
                .live(source)
                .flat_parent_id()
                .expect("a source is a flat child of the node that holds it");
            if !holders.contains(&holder) {
                holders.push(holder);
            }
        }
    }
    let overrides = holders
        .into_iter()
        .map(|holder| {
            let children = tree
                .live(holder)
                .flat_children()
                .iter()
                .filter_map(|child| match opens.get(child) {
                    Some(owner) => *owner,
                    None => Some(*child),
                })
                .collect();
            (holder, children)
        })
        .collect();
    (sources, overrides)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use crate::test_common::Doc;
    use crate::tree::document::NodeId;
    use crate::tree::node::Node;
    use crate::tree::shadow::ShadowRootMode;
    use crate::visual::PaintItemKind;
    use crate::{FontBlob, Point2D};

    const AHEM: &[u8] = include_bytes!("../../../hughie/tests/fixtures/Ahem.ttf");

    /// A 400×300 flex row page, 10px Ahem, and 20px square `view`s.
    fn doc(css: &str) -> Doc {
        let mut doc = Doc::with_css(&format!(
            "page {{ display: flex; width: 400px; height: 300px; align-items: flex-start;
                     font-family: Ahem; font-size: 10px; }}
             view {{ width: 20px; height: 20px; }}
             {css}"
        ));
        assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
        doc
    }

    fn text(doc: &mut Doc, parent: NodeId, data: &str) -> NodeId {
        let id = doc.dom.create_text_node(data, ());
        doc.dom.append_child(parent, id);
        id
    }

    fn rect(doc: &Doc, id: NodeId) -> (f32, f32, f32, f32) {
        let layout = doc.dom.rounded_layout(id).expect("live and laid out");
        (
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        )
    }

    /// The anonymous boxes `container` generates, in tree order.
    fn boxes(doc: &Doc, container: NodeId) -> Vec<NodeId> {
        doc.dom
            .arenas()
            .anonymous()
            .containers
            .get(&container)
            .map(|generated| generated.boxes.clone())
            .unwrap_or_default()
    }

    /// The sources one box lays out.
    fn sources(doc: &Doc, anonymous: NodeId) -> Vec<NodeId> {
        doc.dom
            .get(anonymous)
            .expect("a live box")
            .child_slots()
            .to_vec()
    }

    #[test]
    fn text_between_flex_items_is_one_item_and_takes_its_gaps() {
        let mut doc = doc("page { gap: 10px; }");
        let root = doc.root;
        let first = doc.el(root, "view");
        text(&mut doc, root, "hello");
        let last = doc.el(root, "view");
        doc.flush();

        let [anonymous] = boxes(&doc, root)[..] else {
            panic!("one box")
        };
        assert_eq!(rect(&doc, first), (0.0, 0.0, 20.0, 20.0));
        assert_eq!(rect(&doc, anonymous), (30.0, 0.0, 50.0, 10.0));
        assert_eq!(rect(&doc, last), (90.0, 0.0, 20.0, 20.0));
    }

    #[test]
    fn white_space_only_text_is_no_item() {
        let mut doc = doc("page { gap: 10px; }");
        let root = doc.root;
        doc.el(root, "view");
        text(&mut doc, root, "  \n\t ");
        let last = doc.el(root, "view");
        doc.flush();

        assert!(boxes(&doc, root).is_empty());
        assert_eq!(rect(&doc, last).0, 30.0, "one gap, not two");
    }

    #[test]
    fn a_text_sequence_takes_one_grid_cell_and_white_space_takes_none() {
        for (data, last_x) in [("hi", 100.0), (" ", 50.0)] {
            let mut doc = doc("page { display: grid; grid-template-columns: 50px 50px 50px; }");
            let root = doc.root;
            doc.el(root, "view");
            text(&mut doc, root, data);
            let last = doc.el(root, "view");
            doc.flush();
            assert_eq!(rect(&doc, last).0, last_x, "{data:?}");
        }
    }

    #[test]
    fn grid_lanes_place_a_text_item_in_a_lane() {
        let mut doc = doc("page { display: grid-lanes; grid-template-columns: 100px 100px; }");
        let root = doc.root;
        text(&mut doc, root, "hi");
        let last = doc.el(root, "view");
        doc.flush();

        let [anonymous] = boxes(&doc, root)[..] else {
            panic!("one box")
        };
        assert_eq!(rect(&doc, anonymous), (0.0, 0.0, 100.0, 10.0));
        assert_eq!(rect(&doc, last).0, 100.0);
    }

    #[test]
    fn linear_and_relative_containers_lay_their_text_out_too() {
        for display in ["linear", "relative"] {
            let mut doc = doc(&format!("page {{ display: {display}; }}"));
            let root = doc.root;
            text(&mut doc, root, "hello");
            doc.flush();
            let [anonymous] = boxes(&doc, root)[..] else {
                panic!("{display}: one box")
            };
            let (_, _, width, height) = rect(&doc, anonymous);
            assert!(
                width >= 50.0 && height == 10.0,
                "{display}: {width}×{height}"
            );
        }
    }

    #[test]
    fn contents_is_transparent_and_none_ends_no_sequence() {
        let mut doc = doc(".contents { display: contents; } .none { display: none; }");
        let root = doc.root;
        let ab = text(&mut doc, root, "ab");
        let contents = doc.el(root, "view.contents");
        let cd = text(&mut doc, contents, "cd");
        doc.el(root, "view.none");
        let ef = text(&mut doc, root, "ef");
        doc.flush();

        let [anonymous] = boxes(&doc, root)[..] else {
            panic!("one box")
        };
        assert_eq!(sources(&doc, anonymous), [ab, cd, ef]);
        assert_eq!(rect(&doc, anonymous).2, 60.0);
    }

    #[test]
    fn generated_content_on_a_contents_element_is_text() {
        let mut doc = doc("carrier { display: contents; content: attr(text); }");
        let root = doc.root;
        let carrier = doc.el(root, "carrier[text=xyz]");
        doc.flush();

        let [anonymous] = boxes(&doc, root)[..] else {
            panic!("one box")
        };
        assert_eq!(sources(&doc, anonymous), [carrier]);
        assert_eq!(rect(&doc, anonymous).2, 30.0);
    }

    #[test]
    fn every_box_in_flow_or_out_of_it_ends_a_sequence() {
        for between in ["view", "view.out"] {
            let mut doc = doc(".out { position: absolute; }");
            let root = doc.root;
            text(&mut doc, root, "ab");
            doc.el(root, between);
            text(&mut doc, root, "cd");
            doc.flush();
            assert_eq!(boxes(&doc, root).len(), 2, "{between}");
        }
    }

    #[test]
    fn an_inserted_box_splits_a_sequence_and_its_removal_joins_it() {
        let mut doc = doc("");
        let root = doc.root;
        let ab = text(&mut doc, root, "ab");
        let cd = text(&mut doc, root, "cd");
        doc.flush();
        let [joined] = boxes(&doc, root)[..] else {
            panic!("one box")
        };
        assert_eq!(rect(&doc, joined).2, 40.0);

        let view = doc.dom.create_element("view", ());
        doc.dom.insert_before(root, view, Some(cd));
        doc.flush();
        let [first, second] = boxes(&doc, root)[..] else {
            panic!("two boxes")
        };
        assert_eq!(first, joined, "a box keeps its node");
        assert_eq!(
            (sources(&doc, first), sources(&doc, second)),
            (vec![ab], vec![cd])
        );
        assert_eq!(rect(&doc, second).0, 40.0);

        doc.dom.drop_element(view);
        doc.flush();
        assert_eq!(boxes(&doc, root), [joined]);
        assert!(!doc.dom.contains_node(second), "the surplus box is freed");
        assert_eq!(rect(&doc, joined).2, 40.0);
    }

    #[test]
    fn a_text_edit_can_make_and_unmake_a_box() {
        let mut doc = doc("page { gap: 10px; }");
        let root = doc.root;
        let data = text(&mut doc, root, "abc");
        let last = doc.el(root, "view");
        doc.flush();
        assert_eq!(rect(&doc, last).0, 40.0);

        doc.dom.set_text_node_data(data, "   ");
        doc.flush();
        assert!(boxes(&doc, root).is_empty());
        assert_eq!(rect(&doc, last).0, 0.0);

        doc.dom.set_text_node_data(data, "abcde");
        doc.flush();
        assert_eq!(boxes(&doc, root).len(), 1);
        assert_eq!(rect(&doc, last).0, 60.0);
    }

    #[test]
    fn a_restyle_to_a_box_splits_the_sequence_around_it() {
        let mut doc = doc(".contents { display: contents; }");
        let root = doc.root;
        text(&mut doc, root, "ab");
        let span = doc.el(root, "view.contents");
        text(&mut doc, span, "cd");
        doc.flush();
        assert_eq!(boxes(&doc, root).len(), 1);
        assert!(boxes(&doc, span).is_empty());

        doc.remove_class(span, "contents");
        doc.dom.set_inline_style(span, "width: auto; height: auto");
        doc.flush();
        assert_eq!(boxes(&doc, root).len(), 1, "the text before the box");
        let [inner] = boxes(&doc, span)[..] else {
            panic!("the box lays its own text out")
        };
        assert_eq!(rect(&doc, inner).2, 20.0);
        assert_eq!(rect(&doc, span).0, 20.0);
    }

    #[test]
    fn a_container_that_becomes_a_paragraph_releases_its_boxes() {
        let mut doc = doc(".paragraph { display: -lynx-text; }");
        let root = doc.root;
        let div = doc.el(root, "div");
        text(&mut doc, div, "abc");
        doc.flush();
        let [anonymous] = boxes(&doc, div)[..] else {
            panic!("one box")
        };

        doc.add_class(div, "paragraph");
        doc.flush();
        assert!(boxes(&doc, div).is_empty());
        assert!(!doc.dom.contains_node(anonymous));
        assert_eq!(
            doc.dom.text_block_size(div).map(|size| size.width),
            Some(30.0)
        );
    }

    #[test]
    fn the_box_inherits_the_containers_font() {
        let mut doc = doc("");
        let root = doc.root;
        let div = doc.el(root, "div");
        text(&mut doc, div, "abc");
        doc.flush();
        let [anonymous] = boxes(&doc, div)[..] else {
            panic!("one box")
        };
        assert_eq!(rect(&doc, anonymous).2, 30.0);

        doc.set_inline(div, "font-size: 20px");
        doc.flush();
        assert_eq!(boxes(&doc, div), [anonymous]);
        assert_eq!(rect(&doc, anonymous), (0.0, 0.0, 60.0, 20.0));
    }

    #[test]
    fn a_repaint_only_restyle_of_the_container_keeps_the_box_laid_out() {
        let mut doc = doc(".red { color: red; }");
        let root = doc.root;
        let div = doc.el(root, "div");
        text(&mut doc, div, "abc");
        doc.flush();
        let [anonymous] = boxes(&doc, div)[..] else {
            panic!("one box")
        };

        doc.add_class(div, "red");
        doc.dom.flush_styles_with_damage_sink(&mut |_, _| {});
        assert_eq!(boxes(&doc, div), [anonymous]);
        assert_eq!(
            doc.dom.layout_cache_is_empty(anonymous),
            Some(false),
            "the box style moved in color only"
        );
    }

    #[test]
    fn a_container_built_detached_boxes_its_text_once_inserted() {
        let mut doc = doc("");
        let div = doc.dom.create_element("div", ());
        let data = doc.dom.create_text_node("abc", ());
        doc.dom.append_child(div, data);
        // A flush while `div` is detached: its record finds no style and is
        // dropped.
        doc.flush();
        assert!(boxes(&doc, div).is_empty());

        doc.dom.append_child(doc.root, div);
        doc.flush();
        let [anonymous] = boxes(&doc, div)[..] else {
            panic!("the first style records the container")
        };
        assert_eq!(rect(&doc, anonymous).2, 30.0);
    }

    #[test]
    fn a_container_restyled_while_hidden_shows_its_new_style() {
        let mut doc = doc(".hidden { display: none; }");
        let root = doc.root;
        let outer = doc.el(root, "view");
        let div = doc.el(outer, "div");
        text(&mut doc, div, "abc");
        doc.flush();
        assert_eq!(boxes(&doc, div).len(), 1);

        doc.add_class(outer, "hidden");
        doc.flush();
        doc.set_inline(div, "font-size: 20px");
        doc.flush();
        doc.remove_class(outer, "hidden");
        doc.flush();
        let [anonymous] = boxes(&doc, div)[..] else {
            panic!("one box")
        };
        assert_eq!(rect(&doc, anonymous).2, 60.0);
    }

    #[test]
    fn a_contents_holder_restyle_re_lays_the_box_out() {
        let mut doc = doc(".contents { display: contents; }");
        let root = doc.root;
        let div = doc.el(root, "div");
        let span = doc.el(div, "view.contents");
        text(&mut doc, span, "abc");
        doc.flush();
        let [anonymous] = boxes(&doc, div)[..] else {
            panic!("one box")
        };
        assert_eq!(rect(&doc, anonymous).2, 30.0);

        doc.set_inline(span, "font-size: 20px");
        doc.flush();
        assert_eq!(rect(&doc, anonymous).2, 60.0);
    }

    #[test]
    fn text_moved_between_containers_in_one_batch_belongs_to_the_new_one() {
        let mut doc = doc("");
        let root = doc.root;
        let from = doc.el(root, "div");
        let to = doc.el(root, "div");
        let moving = text(&mut doc, from, "abc");
        doc.flush();
        assert_eq!(boxes(&doc, from).len(), 1);

        doc.dom.append_child(to, moving);
        doc.flush();
        assert!(boxes(&doc, from).is_empty());
        let [anonymous] = boxes(&doc, to)[..] else {
            panic!("one box")
        };
        assert_eq!(sources(&doc, anonymous), [moving]);
        assert_eq!(doc.dom.arenas().anonymous().box_of(moving), Some(anonymous));
    }

    #[test]
    fn an_animated_container_font_reaches_the_box() {
        let mut doc = doc(
            "@keyframes grow { from { font-size: 10px; } to { font-size: 30px; } }
             .grow { animation: grow 1s linear; }",
        );
        let root = doc.root;
        let div = doc.el(root, "div.grow");
        text(&mut doc, div, "abc");
        doc.flush();
        doc.dom.advance_animations(0.0);
        doc.dom.advance_animations(0.5);
        doc.flush();
        let [anonymous] = boxes(&doc, div)[..] else {
            panic!("one box")
        };
        assert_eq!(rect(&doc, anonymous).2, 60.0, "20px at the midpoint");
    }

    #[test]
    fn a_hit_after_a_shadow_root_is_attached_still_targets_the_container() {
        let mut doc = doc("");
        let root = doc.root;
        let div = doc.el(root, "div");
        text(&mut doc, div, "hello");
        doc.flush();
        doc.dom.render();

        // No flush between the attach and the hit: the box still parents to
        // the host itself, whose flat children are the shadow root's now.
        doc.dom.attach_shadow(div, ShadowRootMode::Open);
        assert_eq!(
            doc.dom.elements_from_point(Point2D::new(5.0, 5.0)).first(),
            Some(&div)
        );
    }

    #[test]
    fn the_box_paints_its_text_and_a_hit_on_it_targets_the_container() {
        let mut doc = doc("");
        let root = doc.root;
        let div = doc.el(root, "div");
        let data = text(&mut doc, div, "hello");
        doc.flush();
        let [anonymous] = boxes(&doc, div)[..] else {
            panic!("one box")
        };

        let paint = doc.dom.build_paint_order();
        assert!(paint.items().iter().all(|item| item.node != data));
        assert!(
            paint
                .items()
                .iter()
                .any(|item| item.kind == PaintItemKind::TextRun { element: anonymous })
        );
        doc.dom.render();
        assert_eq!(
            doc.dom.elements_from_point(Point2D::new(5.0, 5.0)).first(),
            Some(&div)
        );
    }

    #[test]
    fn freeing_a_container_frees_its_boxes_and_its_entries() {
        let mut doc = doc("");
        let root = doc.root;
        let div = doc.el(root, "div");
        text(&mut doc, div, "abc");
        doc.flush();
        let [anonymous] = boxes(&doc, div)[..] else {
            panic!("one box")
        };

        doc.dom.drop_subtree(div);
        doc.flush();
        assert!(!doc.dom.contains_node(anonymous));
        assert!(doc.dom.arenas().anonymous().is_empty());
        assert!(doc.dom.arenas().anonymous().sources.is_empty());
        assert!(doc.dom.arenas().anonymous().layout_children.is_empty());
    }

    #[test]
    fn a_hosts_shadow_tree_text_is_boxed_under_the_host() {
        let mut doc = doc("");
        let root = doc.root;
        let host = doc.el(root, "div");
        let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);
        text(&mut doc, shadow, "hi");
        doc.flush();

        let [anonymous] = boxes(&doc, host)[..] else {
            panic!("one box")
        };
        assert_eq!(
            doc.dom.get(anonymous).and_then(Node::flat_parent_id),
            Some(host)
        );
        assert_eq!(rect(&doc, anonymous).2, 20.0);
    }

    /// The records waiting for the next derivation.
    fn pending(doc: &Doc) -> usize {
        doc.dom.arenas().anonymous().pending.len()
    }

    #[test]
    fn a_document_with_its_text_in_paragraphs_records_nothing() {
        let mut doc = doc("p { display: -lynx-text; }");
        let root = doc.root;
        let paragraph = doc.el(root, "p");
        text(&mut doc, paragraph, "abc");
        let view = doc.el(root, "view");
        doc.flush();
        assert!(boxes(&doc, root).is_empty());

        // An element inserted, text inserted into a paragraph and edited
        // there, and a removal: none can give a container a source.
        let more = text(&mut doc, paragraph, "def");
        doc.el(root, "view");
        doc.dom.set_text_node_data(more, "ghi");
        doc.dom.remove_element(view);
        assert_eq!(pending(&doc), 0);
        doc.flush();
        assert!(boxes(&doc, root).is_empty());
    }

    #[test]
    fn a_paragraph_restyled_to_a_container_boxes_its_text() {
        let mut doc = doc(".paragraph { display: -lynx-text; }");
        let root = doc.root;
        let div = doc.el(root, "div.paragraph");
        text(&mut doc, div, "abc");
        doc.flush();
        assert!(boxes(&doc, div).is_empty());

        doc.remove_class(div, "paragraph");
        doc.flush();
        let [anonymous] = boxes(&doc, div)[..] else {
            panic!("the restyle records the new container")
        };
        assert_eq!(rect(&doc, anonymous).2, 30.0);
    }

    #[test]
    fn a_styled_contents_element_moved_out_of_a_paragraph_brings_its_text() {
        let mut doc = doc("p { display: -lynx-text; } span { display: contents; }");
        let root = doc.root;
        let paragraph = doc.el(root, "p");
        let carrier = doc.el(paragraph, "span");
        let data = text(&mut doc, carrier, "abc");
        doc.flush();
        assert!(boxes(&doc, root).is_empty());

        doc.dom.remove_element(carrier);
        doc.dom.append_child(root, carrier);
        doc.flush();
        let [anonymous] = boxes(&doc, root)[..] else {
            panic!("the insertion of a styled contents element records")
        };
        assert_eq!(sources(&doc, anonymous), [data]);
    }
}
