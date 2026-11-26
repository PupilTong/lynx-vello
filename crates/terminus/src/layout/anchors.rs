//! css-anchor-position-1's host half: which element an anchor query names,
//! where that element is, and the per-box state the module keeps between
//! passes.
//!
//! `seldon` resolves every anchor function, `position-area`,
//! `anchor-center` and the fallback loop; it asks this module three things
//! through [`LayoutTree`](seldon::tree::LayoutTree) —
//! [`target anchor element`](target_anchor) and its rectangle
//! (`anchor_rect`), the [`default anchor`](default_anchor) (§2.4), and whether
//! a named anchor [scrolls with](scrolls_with_default) the default one (§3.3)
//! — and hands back one [`AnchorOutcome`] per committed anchor-positioned
//! box. css-position-4's scrollable containing block is not asked: the
//! scroll container's own algorithm computes it and hands it to its absolute
//! pass. The trait impl lives in `host.rs`; everything it
//! calls is here.
//!
//! # Storage, and why each piece exists
//!
//! Two places, split by who reads them:
//!
//! - [`AnchorRegistry`], in [`TreeArenas`]: the **name index** (who declares which `anchor-name`,
//!   who declares an `anchor-scope`), a **generation** per name (bumped when an element declaring
//!   it is restyled or freed; a document-wide epoch for `anchor-scope` changes and relevance flips)
//!   and each element's **position options** (§6.1, cascaded at the flush harvest, with the
//!   animation-free base style §6.5.1 compares). It is style-derived and maintained by the harvest,
//!   and it lives beside the tree rather than in [`DocumentLayoutState`] because
//!   `LayoutTree::position_option_style` has no state parameter: the option style has to be lent
//!   out of the tree arenas, exactly like the base style. Entries exist only for elements that
//!   declare an anchor name, an anchor scope or `position-try-fallbacks`, so it scales with
//!   declared names, never with the page.
//! - [`AnchoredBox`], one entry per anchor-positioned box in [`DocumentLayoutState::anchored`]: the
//!   last [`AnchorOutcome`], the anchor queries its last committing pass read ([`AnchorRead`], each
//!   with the generation it was answered at), its [`RememberedScroll`] (§3.3) and its last
//!   successful position option (§6.5.1.1). A box gets an entry the first time `seldon` reports it
//!   and loses it when it stops being anchor-positioned, stops generating a box, or is freed. The
//!   painter (scroll compensation, `position-visibility`) reads the outcome and the remembered
//!   offsets from here.
//! - [`AnchorBuckets`], in [`DocumentLayoutState::anchor_buckets`]: each scoped name's definers
//!   partitioned by their nearest `anchor-scope`, rebuilt when the name's generation moves, so a
//!   lookup under `anchor-scope` examines only the definers that can be in scope.
//!
//! One transient buffer, [`DocumentLayoutState::anchor_pending`], carries the
//! reads of the box being laid out until `seldon` reports its outcome; it is
//! empty between passes.
//!
//! # The settle loop
//!
//! Every containing block lays its absolutely positioned boxes out after its
//! in-flow content, in tree order — the ones escaping static wrappers into it
//! included (`layout::hoisted`) — so the layout order places every acceptable
//! anchor before the box that reads it. A reader's relayout root is never
//! deeper than its anchor's: the reader's containing block lies inside the
//! reader's own `contain: size layout` root, and §2.3 makes the anchor a
//! descendant of that containing block. So a run reads stale geometry only
//! where the reader was served from the cache while an anchor under a deeper
//! root moved: an anchor deeper in a sibling's subtree relaid in place, or
//! behind a `contain: strict` boundary that stopped the dirty walk. After
//! every run [`Document::settle_anchors`] re-checks each read — the
//! rectangle of the target found, when the name's registry generation is
//! unchanged and the target keeps its box; the whole lookup otherwise — and
//! invalidates the boxes whose answers moved.
//!
//! Each such reader costs one more run, and within one `layout()` each
//! anchor-positioned box earns that run at most once — the bound `WebKit` puts
//! on its layout-dependency loop (`LayoutDependencyUpdateContext`'s
//! `invalidatedAnchorPositioned` set). The runs are therefore bounded by the
//! number of distinct boxes whose reads went stale, with no constant: every
//! run that is owed was earned by a box not invalidated before. §2.3's
//! dependencies are acyclic, but a box can still go stale twice in one
//! `layout()`: when it reads two anchors whose moves surface in different
//! runs, the ends of two chains with different numbers of cache-served hops.
//! Such a repeat is invalidated like any other move and earns no run, so
//! the document stays dirty and the next `layout()` or `render()` lays the
//! box out — one commit late, never left on an unverified read. A page
//! without anchor-positioned boxes pays one `is_empty` test.
//!
//! # What is modelled of §2.3
//!
//! Exact: loosely matched tree-scoped names (a reference matches a name
//! declared in its own tree or a shadow-including ancestor tree), strictly
//! matched `anchor-scope` in both directions (the anchor's nearest scope
//! must contain the query box; the query box's nearest scope must contain
//! the anchor), "nearest ancestor, else last in tree order", and
//! acceptability through the containing-block chain. Approximated: "tree
//! order" is the flat tree's; the top layer exists (`tree::top_layer`) but
//! §2.3's top-layer clause is not implemented — an element's place in the
//! layer does not enter acceptability, only its containing block does; the
//! initial containing block and the viewport are the same
//! containing block (so a `fixed` box can anchor to anything in flow); the
//! skipped-contents clause is subsumed by the committed-layout check (an
//! element in skipped contents has none, and a positioned box in the same
//! skipped contents is not laid out either).

use std::cell::RefCell;

use euclid::default::Vector2D;
use rustc_hash::{FxHashMap, FxHashSet};
use seldon::geometry::{Point, Rect, Size};
use seldon::style::{DashedIdent, PhysicalAxis, PositionProperty, TreeScoped};
use seldon::tree::AnchorOutcome;
use smallvec::SmallVec;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;
use stylo::values::specified::position::PositionAnchorKeyword;
use stylo_atoms::Atom;

use super::style::{
    box_parent, establishes_absolute_containing_block, establishes_fixed_containing_block,
    generates_no_box,
};
use crate::tree::document::{DocumentLayoutState, NodeId, NodeSlot, TreeArenas};
use crate::tree::node::Node;

#[derive(Default)]
pub(crate) struct AnchorRegistry {
    definers: FxHashMap<Atom, SmallVec<[NodeId; 2]>>,
    defined: FxHashMap<NodeId, SmallVec<[Atom; 2]>>,
    scopers: FxHashSet<NodeId>,
    options: FxHashMap<NodeId, PositionOptions>,
    generations: FxHashMap<Atom, u64>,
    epoch: u64,
}

pub(crate) struct PositionOptions {
    pub(crate) base: Arc<ComputedValues>,
    pub(crate) styles: Box<[Arc<ComputedValues>]>,
}

impl AnchorRegistry {
    pub(crate) fn definers(&self, name: &Atom) -> &[NodeId] {
        self.definers.get(name).map_or(&[], |list| list.as_slice())
    }

    pub(crate) fn has_scopers(&self) -> bool {
        !self.scopers.is_empty()
    }

    pub(crate) fn is_scoper(&self, id: NodeId) -> bool {
        self.scopers.contains(&id)
    }

    pub(crate) fn options(&self, id: NodeId) -> Option<&PositionOptions> {
        self.options.get(&id)
    }

    pub(crate) fn has_fallbacks(&self, id: NodeId) -> bool {
        self.options(id)
            .is_some_and(|options| !options.styles.is_empty())
    }

    fn option_holders(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.options.keys().copied()
    }

    pub(crate) fn knows(&self, id: NodeId) -> bool {
        !self.is_empty()
            && (self.defined.contains_key(&id)
                || self.scopers.contains(&id)
                || self.options.contains_key(&id))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.defined.is_empty() && self.scopers.is_empty() && self.options.is_empty()
    }

    #[inline]
    pub(crate) fn generation(&self, name: &Atom) -> u64 {
        self.epoch
            .wrapping_add(self.generations.get(name).copied().unwrap_or(0))
    }

    pub(crate) fn note_relevance_flip(&mut self) {
        if !self.is_empty() {
            self.epoch = self.epoch.wrapping_add(1);
        }
    }

    fn bump(&mut self, name: &Atom) {
        let generation = self.generations.entry(name.clone()).or_default();
        *generation = generation.wrapping_add(1);
    }

    pub(crate) fn restyled(&mut self, id: NodeId, style: Option<&ComputedValues>) {
        self.undefine(id);
        let Some(style) = style else {
            return;
        };
        let box_style = style.get_box();
        let mut names: SmallVec<[Atom; 2]> = SmallVec::new();
        for name in box_style.anchor_name.value.0.iter() {
            if !names.contains(&name.0) {
                names.push(name.0.clone());
            }
        }
        for name in &names {
            self.definers.entry(name.clone()).or_default().push(id);
            self.bump(name);
        }
        if !names.is_empty() {
            self.defined.insert(id, names);
        }
        if !box_style.anchor_scope.is_none() {
            self.scopers.insert(id);
            self.epoch = self.epoch.wrapping_add(1);
        }
    }

    fn undefine(&mut self, id: NodeId) {
        if let Some(names) = self.defined.remove(&id) {
            for name in names {
                self.bump(&name);
                if let Some(list) = self.definers.get_mut(&name) {
                    list.retain(|definer| *definer != id);
                    if list.is_empty() {
                        self.definers.remove(&name);
                    }
                }
            }
        }
        if self.scopers.remove(&id) {
            self.epoch = self.epoch.wrapping_add(1);
        }
    }

    pub(crate) fn forget(&mut self, id: NodeId) {
        if self.is_empty() {
            return;
        }
        self.undefine(id);
        self.options.remove(&id);
    }
}

pub(crate) fn declares_anchor_state(style: &ComputedValues) -> bool {
    let box_style = style.get_box();
    !box_style.anchor_name.value.0.is_empty()
        || !box_style.anchor_scope.is_none()
        || !style.get_position().position_try_fallbacks.value.is_none()
}

#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent per-box flags with different writers, not a state machine"
)]
pub(crate) struct AnchoredBox {
    pub(crate) outcome: AnchorOutcome,
    pub(crate) reads: SmallVec<[AnchorRead; 2]>,
    pub(crate) remembered: Option<RememberedScroll>,
    pub(crate) last_successful: Option<usize>,
    pub(crate) fallback_sensitive: bool,
    pub(crate) redetermine: bool,
    pub(crate) fits_scrolled: bool,
    pub(crate) reported: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AnchorQuery {
    Default,
    Named(TreeScoped<DashedIdent>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AnchorRead {
    pub(crate) option: usize,
    pub(crate) query: AnchorQuery,
    pub(crate) target: Option<NodeId>,
    pub(crate) rect: Option<Rect<f32>>,
    pub(crate) generation: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RememberedScroll {
    pub(crate) option: usize,
    pub(crate) anchors: SmallVec<[(NodeId, Vector2D<f32>); 2]>,
    pub(crate) default: Option<(NodeId, Vector2D<f32>)>,
}

impl RememberedScroll {
    pub(crate) fn displacement_of(&self, anchor: NodeId) -> Option<Vector2D<f32>> {
        self.anchors
            .iter()
            .find(|(id, _)| *id == anchor)
            .map(|&(_, displacement)| displacement)
    }
}

pub(crate) type PendingReads = RefCell<Vec<(NodeSlot, AnchorRead)>>;

pub(crate) type AnchorBuckets = RefCell<FxHashMap<Atom, (u64, ScopeBuckets)>>;

type ScopeBuckets = FxHashMap<Option<NodeId>, SmallVec<[NodeId; 2]>>;

#[cfg(test)]
std::thread_local! {
    static CANDIDATES_EXAMINED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn candidates_examined_during(pass: impl FnOnce()) -> usize {
    CANDIDATES_EXAMINED.with(|count| count.set(0));
    pass();
    CANDIDATES_EXAMINED.with(std::cell::Cell::get)
}

pub(crate) fn containing_block_generator<T>(node: &Node<T>) -> Option<&Node<T>> {
    if node.arenas().top_layer().places_against_viewport(node.id()) {
        return None;
    }
    let Some(style) = node.layout_computed_style() else {
        return box_parent(node);
    };
    match *style.get_box().get_position() {
        PositionProperty::Absolute => positioned_containing_block(node, false),
        PositionProperty::Fixed => positioned_containing_block(node, true),
        PositionProperty::Static | PositionProperty::Relative | PositionProperty::Sticky => {
            box_parent(node)
        }
    }
}

fn positioned_containing_block<T>(node: &Node<T>, fixed: bool) -> Option<&Node<T>> {
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
            return Some(ancestor);
        }
        if !top_layer.is_empty() && top_layer.contains(ancestor.id()) {
            return None;
        }
        current = box_parent(ancestor);
    }
    None
}

fn is_absolutely_positioned(style: &ComputedValues) -> bool {
    matches!(
        *style.get_box().get_position(),
        PositionProperty::Absolute | PositionProperty::Fixed
    )
}

pub(super) type Chain = SmallVec<[NodeId; 16]>;

pub(super) fn connected_chain<T>(node: &Node<T>) -> Option<Chain> {
    let mut chain = Chain::new();
    let mut current = Some(node);
    let mut last = node;
    while let Some(step) = current {
        chain.push(step.id());
        last = step;
        current = step.flat_parent();
    }
    last.is_document().then_some(chain)
}

fn chain_contains_ancestor(chain: &Chain, ancestor: NodeId) -> bool {
    chain.iter().skip(1).any(|&id| id == ancestor)
}

pub(super) fn precedes<T>(tree: &TreeArenas<T>, first: &[NodeId], second: &[NodeId]) -> bool {
    let mut a = first.iter().rev();
    let mut b = second.iter().rev();
    let mut parent = None;
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) if x == y => parent = Some(*x),
            (Some(x), Some(y)) => {
                let parent = parent.expect("both chains end at the document node");
                let children = tree.at(parent).flat_children();
                let index = |id: &NodeId| children.iter().position(|child| child == id);
                return index(x) < index(y);
            }
            (None, Some(_)) => return true,
            (Some(_) | None, None) => return false,
        }
    }
}

fn loosely_matches<T>(
    tree: &TreeArenas<T>,
    declared: Option<NodeId>,
    reference: Option<NodeId>,
) -> bool {
    let mut current = reference;
    loop {
        if current == declared {
            return true;
        }
        let Some(root) = current else {
            return false;
        };
        current = tree
            .get(root)
            .and_then(Node::shadow_host_id)
            .and_then(|host| tree.get(host))
            .and_then(Node::containing_shadow_root)
            .map(Node::id);
    }
}

pub(crate) struct Query<'a, T> {
    pub(crate) node: &'a Node<T>,
    chain: Chain,
    containing_block: Option<NodeId>,
}

impl<'a, T> Query<'a, T> {
    pub(crate) fn of(node: &'a Node<T>) -> Option<Self> {
        Some(Self {
            node,
            chain: connected_chain(node)?,
            containing_block: containing_block_generator(node).map(Node::id),
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Liveness<'a> {
    Committed(&'a DocumentLayoutState),
    Relevance { unskipped: NodeId },
}

pub(crate) fn target_anchor<'t, T>(
    tree: &'t TreeArenas<T>,
    query: &Query<'_, T>,
    name: &TreeScoped<DashedIdent>,
    liveness: Liveness<'_>,
) -> Option<&'t Node<T>> {
    let atom = &name.value.0;
    let registry = tree.anchors();
    let all = registry.definers(atom);
    if all.is_empty() {
        return None;
    }
    let scoped = match liveness {
        Liveness::Committed(state) if registry.has_scopers() && all.len() > 1 => {
            Some(scoped_candidates(tree, state, query, atom))
        }
        _ => None,
    };
    let candidates = scoped.as_deref().unwrap_or(all);
    let reference_tree = query.node.scoped_name_tree(name.scope);
    let mut nearest_ancestor: Option<(usize, &Node<T>)> = None;
    let mut last: Option<(&Node<T>, Chain)> = None;
    for &candidate in candidates {
        #[cfg(test)]
        CANDIDATES_EXAMINED.with(|count| count.set(count.get() + 1));
        let node = tree.at(candidate);
        if node.id() == query.node.id() {
            continue;
        }
        let Some(style) = node.layout_computed_style() else {
            continue;
        };
        let anchor_name = &style.get_box().anchor_name;
        debug_assert!(
            anchor_name.value.0.iter().any(|own| own.0 == *atom),
            "the registry lists an element under a name it declares"
        );
        let declared_tree = node.scoped_name_tree(anchor_name.scope);
        if !loosely_matches(tree, declared_tree, reference_tree) {
            continue;
        }
        if generates_no_box(style) || style.get_display().is_none() {
            continue;
        }
        let Some(chain) = connected_chain(node) else {
            continue;
        };
        if !has_box(tree, node, &chain, liveness) {
            continue;
        }
        if registry.has_scopers()
            && !in_scope(tree, &chain, query, atom, declared_tree, reference_tree)
        {
            continue;
        }
        if !acceptable(tree, node, &chain, query) {
            continue;
        }
        if chain_contains_ancestor(&query.chain, node.id()) {
            let depth = chain.len();
            if nearest_ancestor.is_none_or(|(best, _)| depth > best) {
                nearest_ancestor = Some((depth, node));
            }
        } else if last
            .as_ref()
            .is_none_or(|(_, best)| precedes(tree, best, &chain))
        {
            last = Some((node, chain));
        }
    }
    nearest_ancestor
        .map(|(_, node)| node)
        .or_else(|| last.map(|(node, _)| node))
}

fn scoped_candidates<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    query: &Query<'_, T>,
    name: &Atom,
) -> SmallVec<[NodeId; 4]> {
    let registry = tree.anchors();
    let generation = registry.generation(name);
    let mut buckets = state.anchor_buckets.borrow_mut();
    let entry = buckets
        .entry(name.clone())
        .or_insert_with(|| (generation.wrapping_add(1), ScopeBuckets::default()));
    if entry.0 != generation {
        entry.1.clear();
        for &definer in registry.definers(name) {
            let node = tree.at(definer);
            let declared_tree = node
                .layout_computed_style()
                .map(|style| node.scoped_name_tree(style.get_box().anchor_name.scope));
            let nearest = declared_tree.and_then(|declared_tree| {
                let mut current = Some(node);
                while let Some(step) = current {
                    if scopes(tree, step.id(), name, declared_tree) {
                        return Some(step.id());
                    }
                    current = step.flat_parent();
                }
                None
            });
            entry.1.entry(nearest).or_default().push(definer);
        }
        entry.0 = generation;
    }
    let mut candidates = SmallVec::new();
    if let Some(unscoped) = entry.1.get(&None) {
        candidates.extend_from_slice(unscoped);
    }
    for &ancestor in query.chain.iter().skip(1) {
        if let Some(bucket) = entry.1.get(&Some(ancestor)) {
            candidates.extend_from_slice(bucket);
        }
    }
    candidates
}

fn has_box<T>(tree: &TreeArenas<T>, node: &Node<T>, chain: &Chain, liveness: Liveness<'_>) -> bool {
    match liveness {
        Liveness::Committed(state) => {
            state
                .get(node.id())
                .is_some_and(|entry| !entry.slot.is_hidden())
                && chain
                    .iter()
                    .skip(1)
                    .all(|&id| state.get(id).is_none_or(|entry| !entry.slot.is_hidden()))
        }
        Liveness::Relevance { unskipped } => {
            for &id in chain.iter().skip(1) {
                if id == unskipped {
                    return true;
                }
                let Some(ancestor) = tree.get(id) else {
                    return false;
                };
                let Some(style) = ancestor.layout_computed_style() else {
                    continue;
                };
                if style.get_display().is_none() || super::skips_contents(ancestor, style) {
                    return false;
                }
            }
            true
        }
    }
}

fn in_scope<T>(
    tree: &TreeArenas<T>,
    candidate: &Chain,
    query: &Query<'_, T>,
    name: &Atom,
    declared_tree: Option<NodeId>,
    reference_tree: Option<NodeId>,
) -> bool {
    let nearest_scope = |chain: &Chain, own: bool, name_tree: Option<NodeId>| {
        chain
            .iter()
            .skip(usize::from(!own))
            .copied()
            .find(|&id| scopes(tree, id, name, name_tree))
    };
    if let Some(scope) = nearest_scope(candidate, true, declared_tree)
        && !chain_contains_ancestor(&query.chain, scope)
    {
        return false;
    }
    if let Some(scope) = nearest_scope(&query.chain, false, reference_tree)
        && !candidate.contains(&scope)
    {
        return false;
    }
    true
}

fn scopes<T>(tree: &TreeArenas<T>, id: NodeId, name: &Atom, name_tree: Option<NodeId>) -> bool {
    if !tree.anchors().is_scoper(id) {
        return false;
    }
    let node = tree.at(id);
    let Some(style) = node.layout_computed_style() else {
        return false;
    };
    let scope = &style.get_box().anchor_scope;
    (scope.value.is_all() || scope.value.iter().any(|own| own == name))
        && node.scoped_name_tree(scope.scope) == name_tree
}

fn acceptable<T>(
    tree: &TreeArenas<T>,
    candidate: &Node<T>,
    chain: &Chain,
    query: &Query<'_, T>,
) -> bool {
    let mut current = candidate;
    let mut at = 0;
    loop {
        if current.id() == query.node.id() {
            return false;
        }
        let generator = containing_block_generator(current);
        if generator.map(Node::id) == query.containing_block {
            let Some(style) = current.layout_computed_style() else {
                return false;
            };
            if !is_absolutely_positioned(style) {
                return true;
            }
            return precedes(tree, &chain[at..], &query.chain);
        }
        let Some(generator) = generator else {
            return false;
        };
        at += chain[at..]
            .iter()
            .position(|&id| id == generator.id())
            .expect("a containing block's generator is a flat-tree ancestor");
        current = generator;
    }
}

pub(crate) fn default_anchor<'t, T>(
    tree: &'t TreeArenas<T>,
    query: &Query<'_, T>,
    style: &ComputedValues,
    liveness: Liveness<'_>,
) -> Option<&'t Node<T>> {
    let position_anchor = &style.get_position().position_anchor;
    match &position_anchor.value {
        PositionAnchorKeyword::Ident(name) => {
            let name = TreeScoped {
                value: name.clone(),
                scope: position_anchor.scope,
            };
            target_anchor(tree, query, &name, liveness)
        }
        PositionAnchorKeyword::MatchParent => {
            let parent = query
                .node
                .flat_parent()
                .filter(|parent| parent.is_element())?;
            let parent_base = parent
                .layout_computed_style()
                .filter(|style| is_absolutely_positioned(style))?;
            let parent_style = match liveness {
                Liveness::Committed(state) => state
                    .anchored
                    .get(&parent.id())
                    .map(|entry| entry.outcome)
                    .filter(|outcome| outcome.chosen > 0)
                    .and_then(|outcome| option_style(tree, parent, outcome.chosen))
                    .unwrap_or(parent_base),
                Liveness::Relevance { .. } => parent_base,
            };
            let parent_query = Query::of(parent)?;
            let anchor = default_anchor(tree, &parent_query, parent_style, liveness)?;
            let chain = connected_chain(anchor)?;
            acceptable(tree, anchor, &chain, query).then_some(anchor)
        }
        PositionAnchorKeyword::Normal
        | PositionAnchorKeyword::None
        | PositionAnchorKeyword::Auto => None,
    }
}

pub(crate) fn may_have_default_anchor<T>(tree: &TreeArenas<T>, node: &Node<T>) -> bool {
    let names = |style: &ComputedValues| {
        !matches!(
            style.get_position().position_anchor.value,
            PositionAnchorKeyword::Normal
                | PositionAnchorKeyword::None
                | PositionAnchorKeyword::Auto
        )
    };
    node.layout_computed_style().is_some_and(names)
        || tree
            .anchors()
            .options(node.id())
            .is_some_and(|options| options.styles.iter().any(|style| names(style)))
}

fn unrounded_origin<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    node: NodeId,
) -> Point<f32> {
    let mut origin = Point::ZERO;
    let mut current = Some(node);
    while let Some(id) = current {
        if let Some(entry) = state.get(id) {
            let location = entry.slot.unrounded.location;
            origin = Point::new(origin.x + location.x, origin.y + location.y);
        }
        current = tree.at(id).flat_parent_slot();
    }
    origin
}

fn layout_rect<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    anchor: NodeId,
    containing_block: Option<NodeId>,
) -> Rect<f32> {
    let origin = unrounded_origin(tree, state, anchor);
    let frame = containing_block.map_or(Point::ZERO, |block| {
        let block_origin = unrounded_origin(tree, state, block);
        let border = state
            .get(block)
            .map_or(seldon::geometry::Edges::uniform(0.0), |entry| {
                entry.slot.unrounded.border
            });
        Point::new(block_origin.x + border.left, block_origin.y + border.top)
    });
    let size = state
        .get(anchor)
        .map_or(Size::ZERO, |entry| entry.slot.unrounded.size);
    Rect::new(Point::new(origin.x - frame.x, origin.y - frame.y), size)
}

fn scroll_ancestors<T>(
    anchor: &Node<T>,
    containing_block: Option<NodeId>,
) -> SmallVec<[NodeId; 4]> {
    let mut found = SmallVec::new();
    let mut current = containing_block_generator(anchor);
    while let Some(node) = current {
        if Some(node.id()) == containing_block {
            break;
        }
        if node
            .layout_computed_style()
            .is_some_and(crate::scroll::is_scroll_container)
        {
            found.push(node.id());
        }
        current = containing_block_generator(node);
    }
    found
}

fn stored_displacement<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    anchor: NodeId,
    containing_block: Option<NodeId>,
) -> Vector2D<f32> {
    scroll_ancestors(tree.at(anchor), containing_block)
        .iter()
        .fold(Vector2D::zero(), |sum, &scroller| {
            let offset = tree
                .get(scroller)
                .and_then(Node::layout_computed_style)
                .zip(state.get(scroller))
                .and_then(|(style, entry)| {
                    let offset = state
                        .scroll_offsets
                        .get(&scroller)
                        .copied()
                        .unwrap_or_default();
                    crate::scroll::resolve(style, &entry.slot.unrounded, offset)
                })
                .map_or_else(Vector2D::zero, |scroll_box| scroll_box.offset);
            sum - offset
        })
}

fn anchor_rect_of<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    query: &Query<'_, T>,
    option: usize,
    target: NodeId,
) -> Rect<f32> {
    let rect = layout_rect(tree, state, target, query.containing_block);
    let remembered = state
        .anchored
        .get(&query.node.id())
        .filter(|entry| !entry.redetermine)
        .and_then(|entry| entry.remembered.as_ref())
        .filter(|remembered| remembered.option == option)
        .and_then(|remembered| remembered.displacement_of(target));
    let displacement = remembered
        .unwrap_or_else(|| stored_displacement(tree, state, target, query.containing_block));
    Rect::new(
        Point::new(
            rect.origin.x + displacement.x,
            rect.origin.y + displacement.y,
        ),
        rect.size,
    )
}

fn nearest_scroller<T>(node: &Node<T>, axis: PhysicalAxis) -> Option<NodeId> {
    let mut current = containing_block_generator(node);
    while let Some(ancestor) = current {
        if let Some(style) = ancestor.layout_computed_style() {
            let overflow = match axis {
                PhysicalAxis::Horizontal => *style.get_overflow_x(),
                PhysicalAxis::Vertical => *style.get_overflow_y(),
            };
            if overflow.is_scrollable() {
                return Some(ancestor.id());
            }
        }
        current = containing_block_generator(ancestor);
    }
    None
}

pub(crate) fn scrolls_with_default<T>(
    named: &Node<T>,
    default: &Node<T>,
    axis: PhysicalAxis,
) -> bool {
    nearest_scroller(named, axis) == nearest_scroller(default, axis)
}

pub(crate) fn fits_shifted(outcome: &AnchorOutcome, shift: Vector2D<f32>) -> bool {
    const SLACK: f32 = 1.0 / 64.0;
    if shift == Vector2D::zero() {
        return !outcome.overflows;
    }
    let (imcb, margin_box, carried) = (outcome.imcb, outcome.margin_box, outcome.carried_edges);
    let moved = |carried: bool, edge: f32, by: f32| if carried { edge + by } else { edge };
    let left = moved(carried.left, imcb.origin.x, shift.x);
    let top = moved(carried.top, imcb.origin.y, shift.y);
    let right = moved(carried.right, imcb.origin.x + imcb.size.width, shift.x);
    let bottom = moved(carried.bottom, imcb.origin.y + imcb.size.height, shift.y);
    let (x, y) = (margin_box.origin.x + shift.x, margin_box.origin.y + shift.y);
    x >= left - SLACK
        && y >= top - SLACK
        && x + margin_box.size.width <= right + SLACK
        && y + margin_box.size.height <= bottom + SLACK
}

pub(crate) fn mask(compensates: Size<bool>, v: Vector2D<f32>) -> Vector2D<f32> {
    Vector2D::new(
        if compensates.width { v.x } else { 0.0 },
        if compensates.height { v.y } else { 0.0 },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrollAdjust {
    Scroll,
    Sticky,
}

pub(crate) fn scroll_adjustment_ancestors<T>(
    document: &crate::tree::document::Document<T>,
    anchor: NodeId,
    containing_block: Option<NodeId>,
) -> impl Iterator<Item = (NodeId, ScrollAdjust)> + '_ {
    std::iter::successors(document.get(anchor), |&step| {
        containing_block_generator(step)
    })
    .take_while(move |step| Some(step.id()) != containing_block)
    .flat_map(move |step| {
        let style = step.layout_computed_style();
        let scroll = step.id() != anchor && style.is_some_and(crate::scroll::is_scroll_container);
        let sticky =
            style.is_some_and(|style| *style.get_box().get_position() == PositionProperty::Sticky);
        [
            (scroll, ScrollAdjust::Scroll),
            (sticky, ScrollAdjust::Sticky),
        ]
        .into_iter()
        .filter(|&(applies, _)| applies)
        .map(move |(_, adjust)| (step.id(), adjust))
    })
}

pub(crate) fn option_style<'t, T>(
    tree: &'t TreeArenas<T>,
    node: &'t Node<T>,
    option: usize,
) -> Option<&'t ComputedValues> {
    if option == 0 {
        return node.layout_computed_style();
    }
    tree.anchors()
        .options(node.id())
        .and_then(|options| options.styles.get(option - 1))
        .map(|style| &**style)
}

pub(crate) fn answer<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    anchored: &Query<'_, T>,
    option: usize,
    query: &AnchorQuery,
) -> (Option<NodeId>, Option<Rect<f32>>) {
    let liveness = Liveness::Committed(state);
    let target = match query {
        AnchorQuery::Default => option_style(tree, anchored.node, option)
            .and_then(|style| default_anchor(tree, anchored, style, liveness)),
        AnchorQuery::Named(name) => target_anchor(tree, anchored, name, liveness),
    };
    let Some(target) = target else {
        return (None, None);
    };
    (
        Some(target.id()),
        Some(anchor_rect_of(tree, state, anchored, option, target.id())),
    )
}

pub(crate) fn lookup_generation<T>(
    tree: &TreeArenas<T>,
    node: &Node<T>,
    option: usize,
    query: &AnchorQuery,
) -> Option<u64> {
    match query {
        AnchorQuery::Named(name) => Some(tree.anchors().generation(&name.value.0)),
        AnchorQuery::Default => match &option_style(tree, node, option)?
            .get_position()
            .position_anchor
            .value
        {
            PositionAnchorKeyword::Ident(name) => Some(tree.anchors().generation(&name.0)),
            PositionAnchorKeyword::MatchParent
            | PositionAnchorKeyword::Normal
            | PositionAnchorKeyword::None
            | PositionAnchorKeyword::Auto => None,
        },
    }
}

fn read_moved<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    anchored: &Query<'_, T>,
    read: &AnchorRead,
) -> bool {
    if let (Some(generation), Some(target)) = (read.generation, read.target)
        && lookup_generation(tree, anchored.node, read.option, &read.query) == Some(generation)
        && let Some(node) = tree.get(target)
        && connected_chain(node)
            .is_some_and(|chain| has_box(tree, node, &chain, Liveness::Committed(state)))
    {
        return Some(anchor_rect_of(tree, state, anchored, read.option, target)) != read.rect;
    }
    answer(tree, state, anchored, read.option, &read.query) != (read.target, read.rect)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestyleSource {
    Flush,
    Animation,
}

fn fallback_sensitive_difference(old: &ComputedValues, new: &ComputedValues) -> bool {
    if std::ptr::eq(old, new) {
        return false;
    }
    let no_box = |style: &ComputedValues| {
        let display = *style.get_display();
        display.is_none() || display.is_contents()
    };
    *old.get_box().get_position() != *new.get_box().get_position()
        || no_box(old) != no_box(new)
        || accepted_properties_differ(old, new)
        || {
            let (a, b) = (old.get_position(), new.get_position());
            a.position_try_fallbacks != b.position_try_fallbacks
                || a.position_try_order != b.position_try_order
        }
}

fn accepted_properties_differ(old: &ComputedValues, new: &ComputedValues) -> bool {
    let (a, b) = (old.get_position(), new.get_position());
    let position = !std::ptr::eq(a, b)
        && (a.top != b.top
            || a.right != b.right
            || a.bottom != b.bottom
            || a.left != b.left
            || a.width != b.width
            || a.height != b.height
            || a.min_width != b.min_width
            || a.min_height != b.min_height
            || a.max_width != b.max_width
            || a.max_height != b.max_height
            || a.justify_self != b.justify_self
            || a.align_self != b.align_self
            || a.position_anchor != b.position_anchor
            || a.position_area != b.position_area);
    let (a, b) = (old.get_margin(), new.get_margin());
    position
        || (!std::ptr::eq(a, b)
            && (a.margin_top != b.margin_top
                || a.margin_right != b.margin_right
                || a.margin_bottom != b.margin_bottom
                || a.margin_left != b.margin_left))
}

fn options_differ(old: &[Arc<ComputedValues>], new: &[Arc<ComputedValues>]) -> bool {
    old.len() != new.len()
        || old
            .iter()
            .zip(new)
            .any(|(old, new)| accepted_properties_differ(old, new))
}

impl<T: Sync> crate::tree::document::Document<T> {
    pub(crate) fn refresh_anchor_state(
        &mut self,
        restyled: &[NodeId],
        source: RestyleSource,
    ) -> bool {
        let changed_names = match source {
            RestyleSource::Flush => self.style_engine_mut().take_changed_position_try_names(),
            RestyleSource::Animation => FxHashSet::default(),
        };
        if restyled.is_empty() && changed_names.is_empty() {
            return false;
        }
        for &id in restyled {
            let style = self.get(id).and_then(Node::computed_style);
            self.arenas_mut()
                .anchors_mut()
                .restyled(id, style.as_deref());
        }
        let mut recascade: Vec<NodeId> = restyled
            .iter()
            .copied()
            .filter(|&id| {
                self.get(id)
                    .and_then(Node::layout_computed_style)
                    .is_some_and(|style| {
                        !style.get_position().position_try_fallbacks.value.is_none()
                    })
                    || self.arenas().anchors().options(id).is_some()
            })
            .collect();
        if !changed_names.is_empty() {
            let names_changed_rule = |options: &PositionOptions| {
                options
                    .base
                    .get_position()
                    .position_try_fallbacks
                    .value
                    .0
                    .iter()
                    .any(|item| match item {
                        stylo::values::specified::position::PositionTryFallbacksItem::IdentAndOrTactic(named) => {
                            changed_names.contains(&named.ident.0)
                        }
                        stylo::values::specified::position::PositionTryFallbacksItem::PositionArea(_) => false,
                    })
            };
            let registry = self.arenas().anchors();
            let holders: Vec<NodeId> = registry
                .option_holders()
                .filter(|&id| registry.options(id).is_some_and(names_changed_rule))
                .filter(|id| !recascade.contains(id))
                .collect();
            recascade.extend(holders);
        }
        let mut invalidated = false;
        for id in recascade {
            let options = self.cascade_position_options(id, source);
            let old = self.arenas_mut().anchors_mut().options.remove(&id);
            let (sensitive, options_moved) = match (&old, &options) {
                (None, None) => (false, false),
                (Some(_), None) | (None, Some(_)) => (true, true),
                (Some(old), Some(new)) => {
                    let moved = options_differ(&old.styles, &new.styles);
                    (
                        moved || fallback_sensitive_difference(&old.base, &new.base),
                        moved,
                    )
                }
            };
            if let Some(options) = options {
                self.arenas_mut().anchors_mut().options.insert(id, options);
            }
            if sensitive
                && source == RestyleSource::Flush
                && let Some(entry) = self.layout_state_mut().anchored.get_mut(&id)
                && entry.last_successful.is_some()
            {
                entry.fallback_sensitive = true;
            }
            if options_moved {
                self.invalidate_layout(id);
                invalidated = true;
            }
        }
        invalidated
    }

    fn cascade_position_options(
        &self,
        id: NodeId,
        source: RestyleSource,
    ) -> Option<PositionOptions> {
        let node = self.get(id)?;
        let style = node.computed_style()?;
        let fallbacks = &style.get_position().position_try_fallbacks;
        if fallbacks.value.is_none() {
            return None;
        }
        let engine = self.style_engine();
        let guards = engine.rule_guards();
        let guards = guards.guards();
        let styles = fallbacks
            .value
            .0
            .iter()
            .filter_map(|item| {
                engine
                    .stylist()
                    .resolve_position_try(&style, &guards, fallbacks.scope, node, item)
            })
            .collect();
        let kept = match source {
            RestyleSource::Animation => self
                .arenas()
                .anchors()
                .options(id)
                .map(|options| options.base.clone()),
            RestyleSource::Flush => None,
        };
        let base =
            kept.unwrap_or_else(|| without_animations(engine.stylist(), &guards, node, &style));
        Some(PositionOptions { base, styles })
    }
}

fn without_animations<T: Sync>(
    stylist: &stylo::stylist::Stylist,
    guards: &stylo::shared_lock::StylesheetGuards<'_>,
    node: &Node<T>,
    style: &Arc<ComputedValues>,
) -> Arc<ComputedValues> {
    let Some(rules) = style.rules.as_ref() else {
        return style.clone();
    };
    if !rules.has_animation_or_transition_rules() {
        return style.clone();
    }
    let mut inputs = stylo::context::CascadeInputs::new_from_style(style);
    inputs.rules = Some(stylist.rule_tree().remove_animation_rules(rules));
    inputs.visited_rules = None;
    let parent = node.flat_parent().filter(|parent| parent.is_element());
    let parent_style = parent.and_then(Node::computed_style);
    let mut layout_parent = parent;
    while let Some(candidate) = layout_parent {
        if candidate
            .computed_style()
            .is_none_or(|style| !style.get_display().is_contents())
        {
            break;
        }
        layout_parent = candidate.flat_parent().filter(|parent| parent.is_element());
    }
    let layout_parent_style = layout_parent.and_then(Node::computed_style);
    stylist.cascade_style_and_visited(
        Some(node),
        None,
        &inputs,
        guards,
        parent_style.as_deref(),
        layout_parent_style.as_deref(),
        stylo::properties::FirstLineReparenting::No,
        &stylo::values::specified::position::PositionTryFallbacksTryTactic::default(),
        None,
        &mut stylo::rule_cache::RuleCacheConditions::default(),
        &mut stylo::context::TreeCountingCaches::default(),
    )
}

impl<T> crate::tree::document::Document<T> {
    #[must_use]
    pub(crate) fn anchor_outcome(&self, id: NodeId) -> Option<&AnchorOutcome> {
        Some(&self.layout_state().anchored.get(&id)?.outcome)
    }

    #[must_use]
    pub(crate) fn remembered_scroll(&self, id: NodeId) -> Option<&RememberedScroll> {
        self.layout_state().anchored.get(&id)?.remembered.as_ref()
    }

    #[must_use]
    pub(crate) fn anchor_containing_block(&self, id: NodeId) -> Option<NodeId> {
        containing_block_generator(self.get(id)?).map(Node::id)
    }

    #[must_use]
    fn anchor_displacement(
        &self,
        anchor: NodeId,
        containing_block: Option<NodeId>,
    ) -> Vector2D<f32> {
        let mut sampled = Vec::new();
        scroll_adjustment_ancestors(self, anchor, containing_block).fold(
            Vector2D::zero(),
            |displacement, (id, adjust)| match adjust {
                ScrollAdjust::Scroll => displacement - self.scroll_offset(id),
                ScrollAdjust::Sticky => {
                    displacement + crate::visual::sticky::live_offset(self, id, &mut sampled)
                }
            },
        )
    }

    #[must_use]
    pub(crate) fn applied_position_option(&self, id: NodeId) -> Option<&ComputedValues> {
        let chosen = self.anchor_outcome(id)?.chosen;
        if chosen == 0 {
            return None;
        }
        option_style(self.arenas(), self.get(id)?, chosen)
    }

    pub(crate) fn settle_anchors(&mut self, invalidated: &mut FxHashSet<NodeId>) -> bool {
        let state = self.layout_state_mut();
        state.anchor_pending.get_mut().clear();
        if state.anchored.is_empty() {
            return false;
        }
        self.retire_anchored_boxes();
        let reported: SmallVec<[NodeId; 8]> = self
            .layout_state_mut()
            .anchored
            .iter_mut()
            .filter_map(|(&id, entry)| std::mem::take(&mut entry.reported).then_some(id))
            .collect();
        for id in reported {
            self.record_remembered_scroll(id);
        }
        let moved: Vec<NodeId> = {
            let (tree, state) = self.visual_parts();
            state
                .anchored
                .iter()
                .filter(|&(&id, entry)| {
                    let Some(query) = tree.get(id).and_then(Query::of) else {
                        return false;
                    };
                    entry
                        .reads
                        .iter()
                        .any(|read| read_moved(tree, state, &query, read))
                })
                .map(|(&id, _)| id)
                .collect()
        };
        let mut fresh = false;
        for id in moved {
            self.invalidate_layout(id);
            fresh |= invalidated.insert(id);
        }
        fresh
    }

    fn retire_anchored_boxes(&mut self) {
        let retired: Vec<NodeId> = {
            let (tree, state) = self.visual_parts();
            state
                .anchored
                .keys()
                .copied()
                .filter(|&id| {
                    let node = tree.at(id);
                    if state.get(id).is_none_or(|entry| entry.slot.is_hidden()) {
                        return true;
                    }
                    let Some(style) = node.layout_computed_style() else {
                        return true;
                    };
                    !is_absolutely_positioned(style)
                        || (!tree.anchors().has_fallbacks(id)
                            && !seldon::compute::uses_anchor_positioning(&super::StyleView::of(
                                node,
                            )))
                })
                .collect()
        };
        let state = self.layout_state_mut();
        for id in retired {
            state.anchored.remove(&id);
        }
    }

    fn record_remembered_scroll(&mut self, id: NodeId) {
        let Some(entry) = self.layout_state().anchored.get(&id) else {
            return;
        };
        let outcome = entry.outcome;
        let chosen = outcome.chosen;
        let recalculate = entry.redetermine
            || entry
                .remembered
                .as_ref()
                .is_none_or(|remembered| remembered.option != chosen);
        let previous = if recalculate {
            None
        } else {
            entry.remembered.clone()
        };
        let targets: SmallVec<[NodeId; 2]> = entry
            .reads
            .iter()
            .filter(|read| read.option == chosen)
            .filter_map(|read| read.target)
            .collect();
        let default = {
            let (tree, state) = self.visual_parts();
            let node = tree.at(id);
            Query::of(node).and_then(|query| {
                let style = option_style(tree, node, chosen)?;
                default_anchor(tree, &query, style, Liveness::Committed(state)).map(Node::id)
            })
        };
        let containing_block = self.anchor_containing_block(id);
        let mut remembered = previous.unwrap_or_else(|| RememberedScroll {
            option: chosen,
            ..RememberedScroll::default()
        });
        let remember = |remembered: &mut RememberedScroll, target: NodeId| {
            remembered.displacement_of(target).unwrap_or_else(|| {
                let displacement = self.anchor_displacement(target, containing_block);
                remembered.anchors.push((target, displacement));
                displacement
            })
        };
        for target in targets {
            remember(&mut remembered, target);
        }
        let default = default.map(|default| (default, remember(&mut remembered, default)));
        remembered.default = default;
        if let Some(entry) = self.layout_state_mut().anchored.get_mut(&id) {
            entry.remembered = Some(remembered);
            entry.redetermine = false;
            entry.fits_scrolled = !outcome.overflows;
        }
    }

    #[must_use]
    pub(crate) fn default_scroll_shift(
        &self,
        id: NodeId,
        ratio: Option<f32>,
    ) -> Option<Vector2D<f32>> {
        let entry = self.layout_state().anchored.get(&id)?;
        let (default, then) = entry.remembered.as_ref()?.default?;
        let now = self.anchor_displacement(default, self.anchor_containing_block(id));
        let shift = match ratio {
            Some(ratio) => {
                crate::paint::compose::snap_offset(now, ratio)
                    - crate::paint::compose::snap_offset(then, ratio)
            }
            None => now - then,
        };
        Some(mask(entry.outcome.compensates, shift))
    }

    pub fn redetermine_scrolled_fallbacks(&mut self) -> bool {
        if self.layout_state().anchored.is_empty() {
            return false;
        }
        let candidates: SmallVec<[(NodeId, AnchorOutcome); 4]> = {
            let (tree, state) = self.visual_parts();
            state
                .anchored
                .iter()
                .filter(|&(&id, entry)| !entry.redetermine && tree.anchors().has_fallbacks(id))
                .map(|(&id, entry)| (id, entry.outcome))
                .collect()
        };
        let mut due: SmallVec<[NodeId; 2]> = SmallVec::new();
        let ratio = self.device_pixel_ratio();
        for (id, outcome) in candidates {
            let Some(shift) = self.default_scroll_shift(id, Some(ratio)) else {
                continue;
            };
            let fits = fits_shifted(&outcome, shift);
            let entry = self
                .layout_state_mut()
                .anchored
                .get_mut(&id)
                .expect("a candidate keeps its entry");
            if std::mem::replace(&mut entry.fits_scrolled, fits) && !fits {
                entry.redetermine = true;
                due.push(id);
            }
        }
        for &id in &due {
            self.invalidate_layout(id);
        }
        !due.is_empty()
    }

    pub(crate) fn forget_fallback_sensitive_options(&mut self) -> bool {
        let state = self.layout_state_mut();
        if state.anchored.is_empty() {
            return false;
        }
        let mut forgotten = Vec::new();
        for (&id, entry) in &mut state.anchored {
            if std::mem::take(&mut entry.fallback_sensitive)
                && entry.last_successful.take().is_some()
            {
                forgotten.push(id);
            }
        }
        for &id in &forgotten {
            self.invalidate_layout(id);
        }
        !forgotten.is_empty()
    }

    pub(crate) fn record_last_successful_options(&mut self) {
        let (tree, state) = self.layout_parts();
        for (&id, entry) in &mut state.anchored {
            entry.last_successful = tree
                .anchors()
                .has_fallbacks(id)
                .then_some(entry.outcome.chosen);
        }
    }

    #[must_use]
    pub(crate) fn anchor_keeps_relevant(&self, element: NodeId) -> bool {
        let (tree, state) = self.visual_parts();
        if state.anchored.is_empty() {
            return false;
        }
        let liveness = Liveness::Relevance { unskipped: element };
        state.anchored.iter().any(|(&id, entry)| {
            let node = tree.at(id);
            if state.get(id).is_none_or(|slot| slot.slot.is_hidden()) {
                return false;
            }
            let Some(query) = Query::of(node) else {
                return false;
            };
            if query.containing_block.is_some_and(|block| {
                block == element
                    || tree
                        .get(block)
                        .and_then(connected_chain)
                        .is_some_and(|chain| chain_contains_ancestor(&chain, element))
            }) {
                return false;
            }
            entry.reads.iter().any(|read| {
                let target = match &read.query {
                    AnchorQuery::Named(name) => target_anchor(tree, &query, name, liveness),
                    AnchorQuery::Default => option_style(tree, node, read.option)
                        .and_then(|style| default_anchor(tree, &query, style, liveness)),
                };
                target
                    .and_then(connected_chain)
                    .is_some_and(|chain| chain_contains_ancestor(&chain, element))
            })
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use euclid::default::Vector2D;

    use crate::layout::host::layout_runs_during;
    use crate::test_common::Doc;

    const BASE: &str = "
        page { display: flex; flex-direction: column; width: 800px; height: 600px; }
        view { display: flex; flex-direction: column; flex-shrink: 0; }";

    fn doc(css: &str) -> Doc {
        Doc::with_css(&format!("{BASE}\n{css}"))
    }

    #[test]
    fn a_page_without_anchor_positioning_keeps_every_table_empty() {
        let mut doc = doc(".abs { position: absolute; top: 3px; }");
        let root = doc.root;
        let cb = doc.el(root, "view");
        doc.set_inline(cb, "position: relative");
        doc.el(cb, "view.abs");
        let runs = layout_runs_during(|| doc.dom.layout());
        assert_eq!(runs, 1, "no settle run for a page without anchors");
        assert!(doc.dom.arenas().anchors().is_empty());
        assert!(doc.dom.layout_state().anchored.is_empty());
    }

    #[test]
    fn the_name_index_follows_restyles_and_frees() {
        let mut doc = doc(".a { anchor-name: --a, --b; } .s { anchor-scope: all; }");
        let root = doc.root;
        let anchor = doc.el(root, "view.a.s");
        doc.flush();
        let name = |doc: &Doc, name: &str| {
            doc.dom
                .arenas()
                .anchors()
                .definers(&stylo_atoms::Atom::from(name))
                .to_vec()
        };
        assert_eq!(name(&doc, "--a"), vec![anchor]);
        assert_eq!(name(&doc, "--b"), vec![anchor]);
        assert!(doc.dom.arenas().anchors().is_scoper(anchor));

        doc.remove_class(anchor, "a");
        doc.flush();
        assert_eq!(name(&doc, "--a"), []);
        assert!(doc.dom.arenas().anchors().knows(anchor), "still a scoper");

        doc.dom.remove_element(anchor);
        doc.flush();
        assert!(doc.dom.arenas().anchors().knows(anchor));
        doc.dom.drop_element(anchor);
        assert!(doc.dom.arenas().anchors().is_empty());
    }

    #[test]
    fn the_default_anchor_is_remembered_at_its_scroll_offset() {
        let mut doc = doc(
            ".scroller { overflow: scroll; width: 200px; height: 100px; }
             .filler { height: 500px; }
             .anchor { anchor-name: --a; width: 40px; height: 30px; margin-top: 50px; }
             .anchored { position: absolute; position-anchor: --a;
                         position-area: bottom center; width: 10px; height: 10px; }",
        );
        let root = doc.root;
        let cb = doc.el(root, "view");
        doc.set_inline(cb, "position: relative; width: 400px; height: 400px");
        let scroller = doc.el(cb, "view.scroller");
        let content = doc.el(scroller, "view");
        let anchor = doc.el(content, "view.anchor");
        doc.el(content, "view.filler");
        let anchored = doc.el(cb, "view.anchored");
        doc.flush();
        let remembered = doc.dom.remembered_scroll(anchored).expect("recorded");
        assert_eq!(remembered.default, Some((anchor, Vector2D::zero())));
        assert_eq!(remembered.displacement_of(anchor), Some(Vector2D::zero()));
        let outcome = doc.dom.anchor_outcome(anchored).expect("reported");
        assert!(!outcome.default_anchor_missing);
        assert!(outcome.compensates.width && outcome.compensates.height);
        assert_eq!(doc.dom.anchor_containing_block(anchored), Some(cb));

        doc.dom.scroll_to(scroller, Vector2D::new(0.0, 30.0));
        assert_eq!(
            doc.dom.anchor_displacement(anchor, Some(cb)),
            Vector2D::new(0.0, -30.0)
        );
        doc.set_inline(anchored, "display: none");
        doc.flush();
        assert!(doc.dom.remembered_scroll(anchored).is_none());
        doc.set_inline(anchored, "");
        doc.flush();
        let remembered = doc.dom.remembered_scroll(anchored).expect("recorded");
        assert_eq!(
            remembered.displacement_of(anchor),
            Some(Vector2D::new(0.0, -30.0))
        );
    }

    #[test]
    fn a_scrollable_containing_block_settles_in_the_run_that_reads_it() {
        let mut doc = doc(
            ".scroller { overflow: hidden; position: relative; width: 80px; height: 80px; }
             .filler { width: 180px; height: 180px; }
             .anchor { anchor-name: --a; }
             .target { position: absolute; position-anchor: --a; top: 0px; left: 0px;
                       right: 0px; bottom: 0px; }",
        );
        let root = doc.root;
        let scroller = doc.el(root, "view.scroller");
        let filler = doc.el(scroller, "view.filler");
        doc.el(filler, "view.anchor");
        let target = doc.el(scroller, "view.target");
        let runs = layout_runs_during(|| doc.dom.layout());
        assert_eq!(runs, 1);
        assert_eq!(
            doc.dom.rounded_layout(target).expect("laid out").size.width,
            180.0
        );
        doc.set_inline(target, "opacity: 0.5");
        let runs = layout_runs_during(|| doc.dom.layout());
        assert!(runs <= 1, "{runs}");
    }

    #[test]
    fn a_box_does_not_grow_its_own_scrollable_containing_block() {
        let mut doc = doc(
            ".cb { position: relative; overflow: hidden; width: 100px; height: 100px; }
             .anchor { anchor-name: --a; width: 20px; height: 20px; }
             .box { position: absolute; position-anchor: --a; left: 0px; right: -50px;
                    top: 0px; height: 10px; }",
        );
        let root = doc.root;
        let cb = doc.el(root, "view.cb");
        doc.el(cb, "view.anchor");
        let target = doc.el(cb, "view.box");
        for pass in 0..4 {
            let runs = layout_runs_during(|| doc.dom.layout());
            assert_eq!(runs, 1, "pass {pass}");
            assert_eq!(
                doc.dom.rounded_layout(target).expect("laid out").size.width,
                150.0,
                "pass {pass}"
            );
            doc.set_inline(target, &format!("top: {}px", pass + 1));
        }
    }

    #[test]
    fn an_anchor_chain_through_escaping_boxes_takes_one_run() {
        let mut doc = doc(".cb { position: relative; width: 400px; height: 400px; }
             .x1 { anchor-name: --x1; width: 10px; height: 10px; }
             .link { position: absolute; top: 0px; width: 10px; height: 10px; }
             .y1 { anchor-name: --y1; left: anchor(--x1 right); }
             .x2 { anchor-name: --x2; left: anchor(--y1 right); }
             .y2 { anchor-name: --y2; left: anchor(--x2 right); }
             .x3 { anchor-name: --x3; left: anchor(--y2 right); }
             .y3 { left: anchor(--x3 right); }");
        let root = doc.root;
        let cb = doc.el(root, "view.cb");
        doc.el(cb, "view.x1");
        doc.el(cb, "view.link.y1");
        let wrapper = doc.el(cb, "view");
        doc.el(wrapper, "view.link.x2");
        doc.el(cb, "view.link.y2");
        let deeper = doc.el(cb, "view");
        let inner = doc.el(deeper, "view");
        doc.el(inner, "view.link.x3");
        let y3 = doc.el(cb, "view.link.y3");
        let runs = layout_runs_during(|| doc.dom.layout());
        assert_eq!(runs, 1);
        assert_eq!(
            doc.dom.rounded_layout(y3).expect("laid out").location.x,
            50.0
        );
    }

    #[test]
    fn each_stale_hop_of_an_anchor_chain_costs_one_run() {
        use std::fmt::Write as _;

        let assert_tops = |doc: &Doc, hops: &[crate::tree::document::NodeId], first: f32| {
            let mut expected = first;
            for (k, &hop) in hops.iter().enumerate() {
                let top = doc.dom.rounded_layout(hop).expect("laid out").location.y;
                assert_eq!(top, expected, "hop {}", k + 1);
                expected += 10.0;
            }
        };
        for stale in [2, 8] {
            let mut css = String::from(
                ".ring { contain: strict; width: 300px; height: 300px; }
                 .hop0 { anchor-name: --hop0; width: 40px; height: 30px; }",
            );
            for k in 1..=stale {
                write!(
                    css,
                    ".hop{k} {{ position: absolute; anchor-name: --hop{k};
                               top: anchor(--hop{} bottom); left: 0px;
                               width: 10px; height: 10px; }}",
                    k - 1
                )
                .expect("a String takes any write");
            }
            let mut doc = doc(&css);
            let root = doc.root;
            let mut rings = vec![doc.el(root, "view.ring")];
            for _ in 0..stale {
                let outer = *rings.last().expect("one ring");
                rings.push(doc.el(outer, "view.ring"));
            }
            rings.reverse();
            let anchor = doc.el(rings[0], "view.hop0");
            let hops: Vec<_> = (1..=stale)
                .map(|k| doc.el(rings[k], &format!("view.hop{k}")))
                .collect();
            let runs = layout_runs_during(|| doc.dom.layout());
            assert_eq!(runs, 1, "{stale}: the first layout reads in order");
            assert_tops(&doc, &hops, 30.0);

            doc.set_inline(anchor, "height: 70px");
            let runs = layout_runs_during(|| doc.dom.layout());
            assert_eq!(runs, 1 + stale, "{stale}: one run per stale hop");
            assert_tops(&doc, &hops, 70.0);
            let viewport = doc.dom.device().viewport_size();
            let viewport = seldon::geometry::Size::new(viewport.width, viewport.height);
            let scale = doc.dom.device().device_pixel_ratio().get();
            assert!(
                !doc.dom.layout_needs_pass(viewport, scale),
                "{stale}: settled within the call"
            );
        }
    }

    #[test]
    fn a_box_stale_twice_in_one_layout_waits_for_the_next() {
        let mut doc = doc(".ring { contain: strict; width: 300px; height: 300px; }
             .outer { contain: strict; width: 300px; height: 700px; }
             .a0 { anchor-name: --a0; width: 40px; height: 30px; }
             .a1 { anchor-name: --a1; width: 40px; height: 30px; }
             .x1 { position: absolute; anchor-name: --x1; top: anchor(--a0 bottom); left: 0px;
                   width: 10px; height: 10px; }
             .x2 { position: absolute; anchor-name: --x2; top: anchor(--x1 bottom); left: 0px;
                   width: 10px; height: 10px; }
             .b { position: absolute; top: anchor(--x2 bottom); left: anchor(--a1 right);
                  width: 10px; height: 10px; }");
        let root = doc.root;
        let outer = doc.el(root, "view.outer");
        let upper = doc.el(outer, "view.ring");
        let side = doc.el(outer, "view.ring");
        let b = doc.el(outer, "view.b");
        let middle = doc.el(upper, "view.ring");
        doc.el(upper, "view.x2");
        let inner = doc.el(middle, "view.ring");
        doc.el(middle, "view.x1");
        let a0 = doc.el(inner, "view.a0");
        let a1 = doc.el(side, "view.a1");
        let at = |doc: &Doc| {
            let at = doc.dom.rounded_layout(b).expect("laid out").location;
            (at.x, at.y)
        };
        assert_eq!(layout_runs_during(|| doc.dom.layout()), 1);
        assert_eq!(at(&doc), (40.0, 50.0));
        let viewport = doc.dom.device().viewport_size();
        let viewport = seldon::geometry::Size::new(viewport.width, viewport.height);
        let scale = doc.dom.device().device_pixel_ratio().get();

        doc.set_inline(a0, "height: 70px");
        doc.set_inline(a1, "width: 90px");
        assert_eq!(layout_runs_during(|| doc.dom.layout()), 3);
        assert_eq!(
            at(&doc),
            (90.0, 50.0),
            "the second read is not laid out yet"
        );
        assert!(
            doc.dom.layout_needs_pass(viewport, scale),
            "the repeat left the document dirty"
        );
        assert_eq!(layout_runs_during(|| doc.dom.layout()), 1);
        assert_eq!(at(&doc), (90.0, 90.0));
        assert!(!doc.dom.layout_needs_pass(viewport, scale));
    }

    #[test]
    fn scoped_lookups_examine_one_candidate_per_box() {
        let examined = |items: usize| {
            let mut doc = doc(
                ".li { position: relative; anchor-scope: --a; width: 100px; height: 20px; }
                 .anchor { anchor-name: --a; width: 10px; height: 10px; }
                 .box { position: absolute; top: anchor(--a bottom); left: 0px;
                        width: 5px; height: 5px; }",
            );
            let root = doc.root;
            let list = doc.el(root, "view");
            let mut boxes = Vec::new();
            for _ in 0..items {
                let li = doc.el(list, "view.li");
                doc.el(li, "view.anchor");
                boxes.push(doc.el(li, "view.box"));
            }
            let first = super::candidates_examined_during(|| doc.dom.layout());
            for &target in &boxes {
                assert_eq!(
                    doc.dom.rounded_layout(target).expect("laid out").location.y,
                    10.0
                );
            }
            doc.set_inline(list, "padding-top: 1px");
            let again = super::candidates_examined_during(|| doc.dom.layout());
            (first, again)
        };
        for items in [8, 32] {
            let (first, again) = examined(items);
            assert!(first <= 2 * items, "{items} items: {first} candidates");
            assert!(again <= 2 * items, "{items} items: {again} candidates");
        }
    }

    #[test]
    fn the_fallback_base_ignores_animations() {
        let mut doc = doc(".cb { position: relative; width: 200px; height: 200px; }
             .box { position: absolute; left: 0px; top: 10px; width: 20px; height: 20px;
                    position-try-fallbacks: --other; animation: drift 10s linear; }
             @position-try --other { top: 0px; }
             @keyframes drift { from { top: 80px; } to { top: 80px; } }");
        let root = doc.root;
        let cb = doc.el(root, "view.cb");
        let target = doc.el(cb, "view.box");
        doc.flush();
        doc.dom.advance_animations(0.0);
        doc.dom.advance_animations(1.0);
        doc.dom.render();
        let top = |doc: &Doc| {
            let options = doc.dom.arenas().anchors().options(target).expect("options");
            format!("{:?}", options.base.get_position().top)
        };
        assert!(top(&doc).contains("10.0"), "{}", top(&doc));
        assert_eq!(
            doc.dom.rounded_layout(target).expect("laid out").location.y,
            80.0,
            "the animation still lays the box out"
        );
        doc.set_inline(target, "opacity: 0.5");
        doc.flush();
        assert!(top(&doc).contains("10.0"), "{}", top(&doc));
        assert!(
            !doc.dom.layout_state().anchored[&target].fallback_sensitive,
            "a restyle that moved only the animation is not fallback-sensitive"
        );
    }

    #[test]
    fn a_new_containing_block_is_fallback_sensitive() {
        let mut doc = doc(".cb { position: relative; width: 200px; height: 200px; }
             .box { position: absolute; left: 0px; top: 500px; width: 20px; height: 20px;
                    position-try-fallbacks: --up; }
             @position-try --up { top: 0px; }");
        let root = doc.root;
        let cb = doc.el(root, "view.cb");
        let wrapper = doc.el(cb, "view");
        let target = doc.el(wrapper, "view.box");
        doc.flush();
        doc.dom.render();
        assert_eq!(
            doc.dom.layout_state().anchored[&target].last_successful,
            Some(1)
        );
        doc.set_inline(wrapper, "transform: translateX(0px)");
        doc.flush();
        assert!(doc.dom.layout_state().anchored[&target].fallback_sensitive);
    }

    #[test]
    fn unknown_fallbacks_retire_like_none() {
        let mut doc = doc(".cb { position: relative; width: 200px; height: 200px; }
             .anchor { anchor-name: --a; width: 10px; height: 10px; }
             .box { position: absolute; left: 0px; top: anchor(--a bottom); width: 20px;
                    height: 20px; position-try-fallbacks: --missing; }");
        let root = doc.root;
        let cb = doc.el(root, "view.cb");
        doc.el(cb, "view.anchor");
        let target = doc.el(cb, "view.box");
        doc.dom.layout();
        assert!(doc.dom.anchor_outcome(target).is_some());
        doc.set_inline(target, "top: 5px");
        doc.dom.layout();
        assert!(doc.dom.anchor_outcome(target).is_none());
    }

    #[test]
    fn match_parent_follows_the_parents_chosen_option() {
        let mut doc = doc(".case { position: relative; width: 120px; height: 100px;
                     transform: translateX(0px); }
             .a1 { position: absolute; anchor-name: --a1; left: 0px; top: 0px;
                   width: 10px; height: 10px; }
             .a2 { position: absolute; anchor-name: --a2; left: 20px; top: 50px;
                   width: 30px; height: 20px; }
             .parent { position: fixed; position-anchor: --a1; left: 0px; top: 200px;
                       width: 10px; height: 10px; position-try-fallbacks: --two; }
             @position-try --two { position-anchor: --a2; top: 0px; }
             .probe { position: fixed; position-anchor: match-parent;
                      left: anchor(right, 1px); top: anchor(bottom, 1px);
                      width: 5px; height: 5px; }");
        let root = doc.root;
        let case = doc.el(root, "view.case");
        doc.el(case, "view.a1");
        doc.el(case, "view.a2");
        let parent = doc.el(case, "view.parent");
        let probe = doc.el(parent, "view.probe");
        let runs = layout_runs_during(|| doc.dom.layout());
        assert_eq!(doc.dom.anchor_outcome(parent).expect("reported").chosen, 1);
        assert_eq!(runs, 1, "the parent reports before its escaping child runs");
        let rect = doc.dom.bounding_client_rect(probe).expect("laid out");
        let frame = doc.dom.bounding_client_rect(case).expect("laid out");
        assert_eq!(
            (
                rect.origin.x - frame.origin.x,
                rect.origin.y - frame.origin.y
            ),
            (50.0, 70.0)
        );
    }
}
