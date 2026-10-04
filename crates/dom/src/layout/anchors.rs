//! css-anchor-position-1's host half: which element an anchor query names,
//! where that element is, and the per-box state the module keeps between
//! passes.
//!
//! `hughie` resolves every anchor function, `position-area`,
//! `anchor-center` and the fallback loop; it asks this module four things
//! through [`LayoutTree`](hughie::tree::LayoutTree) —
//! [`target anchor element`](target_anchor) and its rectangle
//! (`anchor_rect`), the [`default anchor`](default_anchor) (§2.4), whether a
//! named anchor [scrolls with](scrolls_with_default) the default one (§3.3),
//! and the [scrollable containing block](scrollable_containing_block)
//! (css-position-4) — and hands back one [`AnchorOutcome`] per committed
//! anchor-positioned box. The trait impl lives in `host.rs`; everything it
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
//!   successful position option (§6.5.1.1). A box gets an entry the first time `hughie` reports it
//!   and loses it when it stops being anchor-positioned, stops generating a box, or is freed. The
//!   painter (scroll compensation, `position-visibility`) reads the outcome and the remembered
//!   offsets from here.
//! - [`AnchorBuckets`], in [`DocumentLayoutState::anchor_buckets`]: each scoped name's definers
//!   partitioned by their nearest `anchor-scope`, rebuilt when the name's generation moves, so a
//!   lookup under `anchor-scope` examines only the definers that can be in scope.
//!
//! One transient buffer, [`DocumentLayoutState::anchor_pending`], carries the
//! reads of the box being laid out until `hughie` reports its outcome; it is
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
//! order" is the flat tree's; the top layer does not exist (every box is in
//! one layer); the initial containing block and the viewport are the same
//! containing block (so a `fixed` box can anchor to anything in flow); the
//! skipped-contents clause is subsumed by the committed-layout check (an
//! element in skipped contents has none, and a positioned box in the same
//! skipped contents is not laid out either).

use std::cell::RefCell;

use euclid::default::Vector2D;
use hughie::geometry::{Point, Rect, Size};
use hughie::style::{DashedIdent, PhysicalAxis, PositionProperty, TreeScoped};
use hughie::tree::AnchorOutcome;
use rustc_hash::{FxHashMap, FxHashSet};
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

// ---------------------------------------------------------------------------
// The registry: name index and position options.

/// The style-derived half of anchor positioning, kept from the flush
/// harvest (see the module docs for why it lives in [`TreeArenas`]).
#[derive(Default)]
pub(crate) struct AnchorRegistry {
    /// Anchor name → the elements whose `anchor-name` lists it, in no
    /// particular order (the lookup orders candidates itself).
    definers: FxHashMap<Atom, SmallVec<[NodeId; 2]>>,
    /// Element → the names it is listed under in [`Self::definers`].
    defined: FxHashMap<NodeId, SmallVec<[Atom; 2]>>,
    /// The elements whose `anchor-scope` is not `none`.
    scopers: FxHashSet<NodeId>,
    /// The elements whose `position-try-fallbacks` is not `none`, with the
    /// option styles cascaded for them.
    options: FxHashMap<NodeId, PositionOptions>,
    /// Per anchor name, a counter bumped whenever an element declaring it
    /// (before or after) is restyled or freed: what a lookup's answer for
    /// that name can depend on besides the tree. See [`Self::generation`].
    generations: FxHashMap<Atom, u64>,
    /// Bumped by what decides every name's lookup at once: an `anchor-scope`
    /// appearing, changing or going, and a `content-visibility: auto`
    /// relevance flip, which gives or takes the boxes of a whole subtree.
    epoch: u64,
}

/// One element's position options list past its base style (§6.1).
pub(crate) struct PositionOptions {
    /// The base style these were cascaded over, kept so the next harvest
    /// can compare the accepted properties of the base it replaces
    /// (§6.5.1's fallback-sensitive changes) after the node itself has
    /// already swapped its style.
    pub(crate) base: Arc<ComputedValues>,
    /// Option `i + 1`'s style, in `position-try-fallbacks` order, with the
    /// entries naming no `@position-try` rule left out (§6.1: "has no
    /// effect").
    pub(crate) styles: Box<[Arc<ComputedValues>]>,
}

impl AnchorRegistry {
    /// The elements declaring `name`.
    pub(crate) fn definers(&self, name: &Atom) -> &[NodeId] {
        self.definers.get(name).map_or(&[], |list| list.as_slice())
    }

    /// Whether any element declares an `anchor-scope`.
    pub(crate) fn has_scopers(&self) -> bool {
        !self.scopers.is_empty()
    }

    pub(crate) fn is_scoper(&self, id: NodeId) -> bool {
        self.scopers.contains(&id)
    }

    /// `id`'s position options past its base style, if it has any.
    pub(crate) fn options(&self, id: NodeId) -> Option<&PositionOptions> {
        self.options.get(&id)
    }

    /// Whether `id` has a position option past its base style — what makes
    /// `hughie` run the fallback loop for it (an options list whose entries
    /// all name unknown rules is no list at all, §6.1).
    pub(crate) fn has_fallbacks(&self, id: NodeId) -> bool {
        self.options(id)
            .is_some_and(|options| !options.styles.is_empty())
    }

    /// Every element holding position options.
    pub(crate) fn option_holders(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.options.keys().copied()
    }

    pub(crate) fn set_options(&mut self, id: NodeId, options: Option<PositionOptions>) {
        match options {
            Some(options) => {
                self.options.insert(id, options);
            }
            None => {
                self.options.remove(&id);
            }
        }
    }

    /// Whether the registry holds anything for `id`, which is what decides
    /// whether a restyle of an element that declares nothing must still be
    /// looked at (it may have stopped declaring something).
    pub(crate) fn knows(&self, id: NodeId) -> bool {
        !self.is_empty()
            && (self.defined.contains_key(&id)
                || self.scopers.contains(&id)
                || self.options.contains_key(&id))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.defined.is_empty() && self.scopers.is_empty() && self.options.is_empty()
    }

    /// What a §2.3 lookup of `name` read from the registry, as a number that
    /// changes whenever that could: a lookup answered at one generation
    /// names the same target at the same generation, as long as that target
    /// still has a box (the settle loop's check, [`Document::settle_anchors`]).
    #[inline]
    pub(crate) fn generation(&self, name: &Atom) -> u64 {
        self.epoch
            .wrapping_add(self.generations.get(name).copied().unwrap_or(0))
    }

    /// A `content-visibility: auto` element's relevance flipped: its subtree
    /// gained or lost its boxes, which any lookup may have counted.
    pub(crate) fn note_relevance_flip(&mut self) {
        if !self.is_empty() {
            self.epoch = self.epoch.wrapping_add(1);
        }
    }

    fn bump(&mut self, name: &Atom) {
        let generation = self.generations.entry(name.clone()).or_default();
        *generation = generation.wrapping_add(1);
    }

    /// Re-reads the names and the scope `id` declares from its restyled
    /// `style` (`None` when it has none).
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

    /// Drops everything `id` declared: it was freed.
    pub(crate) fn forget(&mut self, id: NodeId) {
        if self.is_empty() {
            return;
        }
        self.undefine(id);
        self.options.remove(&id);
    }
}

/// Whether a computed style declares anything the registry indexes.
pub(crate) fn declares_anchor_state(style: &ComputedValues) -> bool {
    let box_style = style.get_box();
    !box_style.anchor_name.value.0.is_empty()
        || !box_style.anchor_scope.is_none()
        || !style.get_position().position_try_fallbacks.value.is_none()
}

// ---------------------------------------------------------------------------
// Per-box state.

/// Everything kept for one anchor-positioned box between passes; see the
/// module docs.
#[derive(Debug, Default)]
pub(crate) struct AnchoredBox {
    /// What the box's last committing absolute pass decided.
    pub(crate) outcome: Option<AnchorOutcome>,
    /// The anchor queries that pass read, with their answers, for the
    /// settle loop and for §2.5 anchor relevance.
    pub(crate) reads: SmallVec<[AnchorRead; 2]>,
    /// §3.3's remembered scroll offsets.
    pub(crate) remembered: Option<RememberedScroll>,
    /// §6.5.1.1: the option the box used at the last rendering update, which
    /// the next fallback determination starts from.
    pub(crate) last_successful: Option<usize>,
    /// §6.5.1: a fallback-sensitive change happened since the last
    /// rendering update, so the next one forgets [`Self::last_successful`]
    /// before it records again.
    pub(crate) fallback_sensitive: bool,
    /// §6.5 on scroll: the box, shifted by its default scroll shift, stopped
    /// fitting its inset-modified containing block since its last layout, so
    /// its next layout determines position fallback styles again, with every
    /// option — the current one included — reading its anchors at the
    /// current offsets ([`Document::redetermine_scrolled_fallbacks`]).
    pub(crate) redetermine: bool,
    /// Whether the box fit, shifted, at the last scroll that checked it: the
    /// state a re-determination is due on leaving. Reset from the outcome by
    /// every layout that reports the box.
    pub(crate) fits_scrolled: bool,
}

/// What an anchor query asked.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AnchorQuery {
    /// The default anchor element (§2.4).
    Default,
    /// A tree-scoped `<anchor-name>`.
    Named(TreeScoped<DashedIdent>),
    /// css-position-4's scrollable containing block.
    Scrollable,
}

/// One anchor query a box's layout read, and what it was told.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AnchorRead {
    /// The position option it was asked under.
    pub(crate) option: usize,
    pub(crate) query: AnchorQuery,
    /// The element that answered an anchor query.
    pub(crate) target: Option<NodeId>,
    /// The answer: the anchor's rectangle, or the scrollable containing
    /// block at the origin.
    pub(crate) rect: Option<Rect<f32>>,
    /// The registry generation of the name the lookup went through
    /// ([`AnchorRegistry::generation`]), or `None` when it went through no
    /// single name (`match-parent`, the scrollable containing block): the
    /// settle loop re-reads only the rectangle of a target found at the
    /// current generation.
    pub(crate) generation: Option<u64>,
}

/// §3.3's remembered scroll offsets of one anchor-positioned box, recorded
/// at its last anchor recalculation point.
///
/// Stored as *displacements*: for each anchor, the sum over its
/// scroll-adjustment ancestors — the scroll containers on its
/// containing-block chain up to, not including, the box's containing
/// block, and the sticky boxes on that chain including the anchor — of
/// each sticky box's shift minus each scroll container's scroll offset.
/// That is the vector by which scrolling moved the anchor's border box away
/// from where the unscrolled layout put it; the anchor's rectangle as layout
/// reads it is its layout rectangle translated by it.
///
/// The painter's default scroll shift (§3.3) is the default anchor's
/// *current* displacement ([`Document::anchor_displacement`]) minus
/// [`Self::default`]'s remembered one, on the axes the outcome compensates.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RememberedScroll {
    /// The position option the recalculation point was for: a determination
    /// that switches to another option recalculates (§3.3), trying one does
    /// not.
    pub(crate) option: usize,
    /// Each anchor the box referenced under that option, with its
    /// displacement.
    pub(crate) anchors: SmallVec<[(NodeId, Vector2D<f32>); 2]>,
    /// The default anchor element, which always has an entry in
    /// [`Self::anchors`] when present.
    pub(crate) default: Option<NodeId>,
}

impl RememberedScroll {
    pub(crate) fn displacement_of(&self, anchor: NodeId) -> Option<Vector2D<f32>> {
        self.anchors
            .iter()
            .find(|(id, _)| *id == anchor)
            .map(|&(_, displacement)| displacement)
    }
}

/// The pending reads of the boxes being laid out, until `hughie` reports
/// their outcome. Interior-mutable because the queries arrive through
/// `&DocumentLayoutState`.
pub(crate) type PendingReads = RefCell<Vec<(NodeSlot, AnchorRead)>>;

/// One name's definers partitioned by their nearest `anchor-scope` for it
/// (`None`: unscoped), at the registry generation they were sorted at.
///
/// §2.2 makes a candidate acceptable only when its nearest scope for the
/// name contains the query box, so a lookup needs only the partitions keyed
/// by the query box's scoping ancestors and the unscoped one: with N list
/// items each scoping one shared name over one anchor, a lookup sees one
/// candidate instead of N, and a layout's lookups are O(N) rather than
/// O(N²). Kept in [`DocumentLayoutState::anchor_buckets`] (interior-mutable:
/// lookups arrive with the state borrowed shared), one entry per scoped name
/// a lookup asked about, rebuilt when that name's generation moves.
pub(crate) type AnchorBuckets = RefCell<FxHashMap<Atom, (u64, ScopeBuckets)>>;

type ScopeBuckets = FxHashMap<Option<NodeId>, SmallVec<[NodeId; 2]>>;

#[cfg(test)]
std::thread_local! {
    /// How many candidates the §2.3 lookups on this thread have examined.
    static CANDIDATES_EXAMINED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Runs `pass` and answers how many §2.3 candidates its lookups examined.
#[cfg(test)]
pub(crate) fn candidates_examined_during(pass: impl FnOnce()) -> usize {
    CANDIDATES_EXAMINED.with(|count| count.set(0));
    pass();
    CANDIDATES_EXAMINED.with(std::cell::Cell::get)
}

// ---------------------------------------------------------------------------
// Tree walks.

/// The element generating `node`'s containing block (css-position-3 §2.1),
/// or `None` for the initial containing block — which this engine does not
/// tell from the viewport, the fixed containing block.
pub(crate) fn containing_block_generator<T>(node: &Node<T>) -> Option<&Node<T>> {
    let Some(style) = node.layout_computed_style() else {
        return box_parent(node);
    };
    match style.clone_position() {
        PositionProperty::Absolute => positioned_containing_block(node, false),
        PositionProperty::Fixed => positioned_containing_block(node, true),
        PositionProperty::Static | PositionProperty::Relative | PositionProperty::Sticky => {
            box_parent(node)
        }
    }
}

/// The nearest box ancestor establishing the containing block of an
/// absolutely (or, with `fixed`, fixed) positioned descendant.
pub(crate) fn positioned_containing_block<T>(node: &Node<T>, fixed: bool) -> Option<&Node<T>> {
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
        current = box_parent(ancestor);
    }
    None
}

fn is_absolutely_positioned(style: &ComputedValues) -> bool {
    matches!(
        style.clone_position(),
        PositionProperty::Absolute | PositionProperty::Fixed
    )
}

/// `node` and its flat-tree ancestors, nearest first, up to and including
/// the document node — or `None` when the chain does not end at one (a
/// detached subtree, which lays nothing out).
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

/// Whether the element `ancestor` is a proper flat-tree ancestor of the node
/// whose chain is `chain`.
fn chain_contains_ancestor(chain: &Chain, ancestor: NodeId) -> bool {
    chain.iter().skip(1).any(|&id| id == ancestor)
}

/// Whether the node with chain `first` comes before the one with chain
/// `second` in flat tree order (pre-order: an ancestor comes first).
pub(super) fn precedes<T>(tree: &TreeArenas<T>, first: &Chain, second: &Chain) -> bool {
    let mut a = first.iter().rev();
    let mut b = second.iter().rev();
    let mut parent = None;
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) if x == y => parent = Some(*x),
            (Some(x), Some(y)) => {
                let Some(parent) = parent else {
                    return false;
                };
                let children = tree.at(parent).flat_children();
                let index = |id: &NodeId| children.iter().position(|child| child == id);
                return index(x) < index(y);
            }
            // `first` is an ancestor of `second`, or they are the same node.
            (None, Some(_)) => return true,
            (Some(_) | None, None) => return false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tree-scoped names.

/// Whether a name declared in tree `declared` loosely matches a reference
/// from tree `reference` (css-scoping-1 §3.5): the same tree, or a
/// shadow-including ancestor of it. `None` is the document tree.
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

// ---------------------------------------------------------------------------
// §2.3: the target anchor element.

/// The query box a lookup is made for.
pub(crate) struct Query<'a, T> {
    pub(crate) node: &'a Node<T>,
    chain: Chain,
    /// The element generating its containing block.
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

/// How a lookup treats `content-visibility` and committed boxes.
#[derive(Clone, Copy)]
pub(crate) enum Liveness<'a> {
    /// A layout-time lookup: a candidate must hold a committed, unhidden
    /// box in `state`.
    Committed(&'a DocumentLayoutState),
    /// §2.5's relevance question: `unskipped` is taken to be relevant and
    /// its contents laid out, so a candidate inside it counts wherever
    /// `display` gives it a box.
    Relevance { unskipped: NodeId },
}

/// §2.3 for a `<dashed-ident>` `name`, cascaded for `query`'s element.
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
        let Some(node) = tree.get(candidate) else {
            continue;
        };
        if node.id() == query.node.id() {
            continue;
        }
        let Some(style) = node.layout_computed_style() else {
            continue;
        };
        // "el is an anchor element with an anchor name of anchor spec" and
        // "el's anchor name loosely matches anchor spec".
        let anchor_name = &style.get_box().anchor_name;
        if !anchor_name.value.0.iter().any(|own| own.0 == *atom) {
            continue;
        }
        let declared_tree = node.scoped_name_tree(anchor_name.scope);
        if !loosely_matches(tree, declared_tree, reference_tree) {
            continue;
        }
        // An anchor element generates a principal box (§2.1).
        if generates_no_box(style) || style.clone_display().is_none() {
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

/// The definers of `name` that can be in scope for `query` (§2.2): those
/// whose nearest scope for the name is unscoped or one of the query box's
/// scoping ancestors — a superset of the ones [`in_scope`] accepts, read
/// from the name's [`AnchorBuckets`] entry.
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
            let Some(node) = tree.get(definer) else {
                continue;
            };
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
        if registry.is_scoper(ancestor)
            && let Some(bucket) = entry.1.get(&Some(ancestor))
        {
            candidates.extend_from_slice(bucket);
        }
    }
    candidates
}

/// Whether `node` has a box the lookup may read.
fn has_box<T>(tree: &TreeArenas<T>, node: &Node<T>, chain: &Chain, liveness: Liveness<'_>) -> bool {
    match liveness {
        // Its own box, and no hidden ancestor: a child inserted under an
        // already hidden subtree is never visited by the hiding walk, so its
        // own mark alone does not say it is hidden.
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
            // Every ancestor up to `unskipped` gives it a box; above that
            // the element under determination is laid out, so is its chain.
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
                if style.clone_display().is_none() || super::skips_contents(ancestor, style) {
                    return false;
                }
            }
            true
        }
    }
}

/// `anchor-scope` (§2.2), both ways: the candidate's nearest scope for the
/// name must contain the query box, and the query box's nearest scope for
/// it must contain the candidate.
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
    // The candidate's own scope counts: "anchor names defined by this
    // element or its descendants … in scope only for this element's
    // descendants".
    if let Some(scope) = nearest_scope(candidate, true, declared_tree)
        && !chain_contains_ancestor(&query.chain, scope)
    {
        return false;
    }
    // "limits descendants to only match … anchor elements within this
    // subtree": the scope element itself is within it.
    if let Some(scope) = nearest_scope(&query.chain, false, reference_tree)
        && !candidate.contains(&scope)
    {
        return false;
    }
    true
}

/// Whether element `id`'s `anchor-scope` limits `name`, a name of tree
/// `name_tree` — strictly matched: only a scope cascaded from the same tree
/// counts, `all` included.
fn scopes<T>(tree: &TreeArenas<T>, id: NodeId, name: &Atom, name_tree: Option<NodeId>) -> bool {
    if !tree.anchors().is_scoper(id) {
        return false;
    }
    let Some(node) = tree.get(id) else {
        return false;
    };
    let Some(style) = node.layout_computed_style() else {
        return false;
    };
    let scope = &style.get_box().anchor_scope;
    (scope.value.is_all() || scope.value.iter().any(|own| own == name))
        && node.scoped_name_tree(scope.scope) == name_tree
}

/// §2.3's "laid out strictly before": `candidate`, or the element
/// generating its containing block, recursively, shares the query box's
/// original containing block and is in flow, or absolutely positioned and
/// earlier in flat tree order.
fn acceptable<T>(
    tree: &TreeArenas<T>,
    candidate: &Node<T>,
    chain: &Chain,
    query: &Query<'_, T>,
) -> bool {
    let mut current = candidate;
    let mut current_chain = None;
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
            let chain = match &current_chain {
                None => chain,
                Some(own) => own,
            };
            return precedes(tree, chain, &query.chain);
        }
        let Some(generator) = generator else {
            return false;
        };
        current = generator;
        let Some(chain) = connected_chain(current) else {
            return false;
        };
        current_chain = Some(chain);
    }
}

/// §2.4: the default anchor element of `query` under the style `style`
/// (its base style or one of its position options).
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
            // "Uses the same default anchor element as the parent … if any,
            // and if that would be an acceptable anchor element."
            // `position-anchor` applies to absolutely positioned boxes only,
            // so a parent that is not one has no default anchor to match.
            let parent = query
                .node
                .flat_parent()
                .filter(|parent| parent.is_element())?;
            let parent_base = parent
                .layout_computed_style()
                .filter(|style| is_absolutely_positioned(style))?;
            // The parent's default anchor under the option it was laid out
            // with: `position-anchor` is an accepted `@position-try`
            // property, so the option may name another one. A parent being
            // laid out right now has the last run's outcome; when that run
            // chose differently, the settle loop re-reads this (a
            // `match-parent` read goes through no single name, so it is
            // always looked up again).
            let parent_style = match liveness {
                Liveness::Committed(state) => state
                    .anchored
                    .get(&parent.id())
                    .and_then(|entry| entry.outcome)
                    .filter(|outcome| outcome.chosen > 0)
                    .and_then(|outcome| option_style(tree, parent, outcome.chosen))
                    .unwrap_or(parent_base),
                Liveness::Relevance { .. } => parent_base,
            };
            let parent_query = Query::of(parent)?;
            let anchor = default_anchor(tree, &parent_query, parent_style, liveness)?;
            let chain = connected_chain(anchor)?;
            (anchor.id() != query.node.id()
                && has_box(tree, anchor, &chain, liveness)
                && acceptable(tree, anchor, &chain, query))
            .then_some(anchor)
        }
        // `normal` is `none` without a `position-area` and `auto` with one,
        // and `auto` names the implicit anchor element, which no host
        // language here defines.
        PositionAnchorKeyword::Normal
        | PositionAnchorKeyword::None
        | PositionAnchorKeyword::Auto => None,
    }
}

// ---------------------------------------------------------------------------
// Geometry.

/// The origin of `node`'s border box in the document's unscrolled layout
/// coordinates, from the unrounded boxes of this pass.
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

/// `anchor`'s unrounded border box in the padding-box coordinates of
/// `containing_block` (the viewport's for `None`), unscrolled.
pub(crate) fn layout_rect<T>(
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
            .map_or(hughie::geometry::Edges::uniform(0.0), |entry| {
                entry.slot.unrounded.border
            });
        Point::new(block_origin.x + border.left, block_origin.y + border.top)
    });
    let size = state
        .get(anchor)
        .map_or(Size::ZERO, |entry| entry.slot.unrounded.size);
    Rect::new(Point::new(origin.x - frame.x, origin.y - frame.y), size)
}

/// Every scroll container on `anchor`'s containing-block chain, up to, not
/// including, `containing_block`: the scroll containers whose offset moves
/// the anchor relative to that containing block.
pub(crate) fn scroll_ancestors<T>(
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

/// The displacement scrolling applies to `anchor` relative to
/// `containing_block` as layout can see it: minus the sum of the stored
/// scroll offsets of its scroll ancestors. Sticky shifts need the rounded
/// geometry of a finished pass, so a recording made after the pass
/// ([`Document::anchor_displacement`]) adds them; see [`RememberedScroll`].
fn stored_displacement<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    anchor: NodeId,
    containing_block: Option<NodeId>,
) -> Vector2D<f32> {
    let Some(node) = tree.get(anchor) else {
        return Vector2D::zero();
    };
    scroll_ancestors(node, containing_block)
        .iter()
        .fold(Vector2D::zero(), |sum, &scroller| {
            // Clamped the way `Document::scroll_offset` clamps it, against
            // the scroller's last committed box, so a remembered offset
            // recorded after the run agrees with what the run read.
            let offset = tree
                .get(scroller)
                .and_then(Node::layout_computed_style)
                .zip(state.get(scroller))
                .and_then(|(style, entry)| {
                    crate::scroll::resolve(style, &entry.slot.unrounded, entry.scroll_offset)
                })
                .map_or_else(Vector2D::zero, |scroll_box| scroll_box.offset);
            sum - offset
        })
}

/// The answer `LayoutTree::anchor_rect` gives: the target's layout rectangle
/// in the containing block's padding-box coordinates, translated by the
/// query box's remembered displacement for it — or, before any recalculation
/// point recorded one (and for an option being *tried*, §6.5.2's
/// hypothetical recalculation point, which a scroll-driven re-determination
/// makes of every option), by the displacement the stored offsets give now.
pub(crate) fn anchor_rect_of<T>(
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

/// css-position-4's scrollable containing block of a box whose containing
/// block `containing_block` generates: that scroll container's in-flow
/// scrollable overflow area measured from its padding-box origin, as its
/// last committing run recorded it before laying its out-of-flow boxes out
/// (the current run's, when the box is being laid out by that container's
/// absolute pass). `None` when the generator is not a scroll container.
pub(crate) fn scrollable_containing_block<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    containing_block: Option<NodeId>,
) -> Option<Size<f32>> {
    let block = tree.get(containing_block?)?;
    if !block
        .layout_computed_style()
        .is_some_and(crate::scroll::is_scroll_container)
    {
        return None;
    }
    state.scrollable_containing_blocks.get(&block.id()).copied()
}

/// The nearest scroll container on `node`'s containing-block chain whose
/// `axis` is scrollable.
fn nearest_scroller<T>(node: &Node<T>, axis: PhysicalAxis) -> Option<NodeId> {
    let mut current = containing_block_generator(node);
    while let Some(ancestor) = current {
        if let Some(style) = ancestor.layout_computed_style() {
            let overflow = match axis {
                PhysicalAxis::Horizontal => style.clone_overflow_x(),
                PhysicalAxis::Vertical => style.clone_overflow_y(),
            };
            if overflow.is_scrollable() {
                return Some(ancestor.id());
            }
        }
        current = containing_block_generator(ancestor);
    }
    None
}

/// §3.3's last compensation condition: `named` and `default` have the same
/// nearest scroll container with `axis` scrollable.
pub(crate) fn scrolls_with_default<T>(
    named: &Node<T>,
    default: &Node<T>,
    axis: PhysicalAxis,
) -> bool {
    nearest_scroller(named, axis) == nearest_scroller(default, axis)
}

/// Whether a margin box moved by `shift` is inside the inset-modified
/// containing block moved by `shift` on its carried edges
/// ([`AnchorOutcome::carried_edges`], which `hughie` reports from the
/// insets and `position-area` lines it laid the box out with), with
/// `hughie`'s layout unit of slack — §6.5's fit test after "applying any
/// default scroll shift", without the layout that would re-derive the
/// anchor's edges. Only an `auto` inset's containing-block edge constrains
/// the shifted box, as in Blink (`CalculateNonOverflowingRangeInOneAxis`):
/// a carried edge keeps the relation it was laid out with.
pub(crate) fn fits_shifted(outcome: &AnchorOutcome, shift: Vector2D<f32>) -> bool {
    const SLACK: f32 = 1.0 / 64.0;
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

/// The option style `option` names for `node`: its base style for `0`.
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

/// Answers `query` for the box whose [`Query`] is `anchored`, under option
/// `option`, without recording anything: what `LayoutTree`'s anchor methods
/// return and what the settle loop re-asks.
pub(crate) fn answer<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    anchored: &Query<'_, T>,
    option: usize,
    query: &AnchorQuery,
) -> (Option<NodeId>, Option<Rect<f32>>) {
    let liveness = Liveness::Committed(state);
    let target = match query {
        AnchorQuery::Scrollable => {
            let size = scrollable_containing_block(tree, state, anchored.containing_block);
            return (None, size.map(|size| Rect::new(Point::ZERO, size)));
        }
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

/// The registry generation a lookup of `query` for `node` under `option`
/// goes through, when it goes through one name; see
/// [`AnchorRead::generation`].
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
        AnchorQuery::Scrollable => None,
    }
}

/// Whether `read`, a query the box `anchored` made in an earlier run, would
/// be answered differently now. A target found at the registry generation
/// still current is the target now too, as long as it keeps its box, so
/// only its rectangle is re-read; anything else is looked up again.
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

// ---------------------------------------------------------------------------
// The document's half: harvest, settle loop, rendering-update recording.

/// Which harvest re-read an element's anchor state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestyleSource {
    /// A style flush: every change counts toward §6.5.1.
    Flush,
    /// An animation tick. §6.5.1 considers "only changes to the computed
    /// base style … ignoring any declarations originating from the
    /// Transitions or Animations cascade origins", so a tick re-cascades
    /// the options (their cascade includes the animated values) but never
    /// makes a fallback-sensitive change.
    Animation,
}

/// Whether two styles of one element differ in a property §6.5.1 lists:
/// `position`, any `position-try` longhand, any accepted `@position-try`
/// property, or whether it generates a box.
fn fallback_sensitive_difference(old: &ComputedValues, new: &ComputedValues) -> bool {
    if std::ptr::eq(old, new) {
        return false;
    }
    let no_box = |style: &ComputedValues| {
        let display = style.clone_display();
        display.is_none() || display.is_contents()
    };
    old.clone_position() != new.clone_position()
        || no_box(old) != no_box(new)
        || accepted_properties_differ(old, new)
        || {
            let (a, b) = (old.get_position(), new.get_position());
            a.position_try_fallbacks != b.position_try_fallbacks
                || a.position_try_order != b.position_try_order
        }
}

/// Whether two styles differ in an accepted `@position-try` property
/// (§6.3): insets, margins, sizes and min/max sizes, `justify-self`,
/// `align-self`, `position-anchor`, `position-area`.
pub(crate) fn accepted_properties_differ(old: &ComputedValues, new: &ComputedValues) -> bool {
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

/// Whether option lists `old` and `new` differ in what layout reads.
fn options_differ(old: &[Arc<ComputedValues>], new: &[Arc<ComputedValues>]) -> bool {
    old.len() != new.len()
        || old
            .iter()
            .zip(new)
            .any(|(old, new)| accepted_properties_differ(old, new))
}

impl<T: Sync> crate::tree::document::Document<T> {
    /// Re-reads the anchor state of the elements a harvest restyled — their
    /// names, scopes and position options — and re-cascades the options of
    /// every element naming an `@position-try` rule a stylist flush changed.
    /// Answers whether it invalidated layout.
    ///
    /// Options are cascaded here, once per restyle of an element that has
    /// fallbacks, with the fork's `Stylist::resolve_position_try` (the
    /// Position Fallback Origin over the element's rules, then the try
    /// tactic), and kept until the next one: layout reads them as it reads
    /// the base style.
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
            // "Any of the @position-try rules referenced by it have been
            // added, removed, or mutated."
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
            self.arenas_mut().anchors_mut().set_options(id, options);
            if sensitive
                && source == RestyleSource::Flush
                && let Some(entry) = self.layout_state_mut().anchored.get_mut(&id)
                && entry.last_successful.is_some()
            {
                entry.fallback_sensitive = true;
            }
            // A changed option reaches layout through no damage of the base
            // style when only an `@position-try` rule moved.
            if options_moved && self.get(id).is_some() {
                self.invalidate_layout(id);
                invalidated = true;
            }
        }
        invalidated
    }

    /// `id`'s position options past its base style, cascaded now; `None`
    /// when its `position-try-fallbacks` is `none`.
    ///
    /// The options are cascaded over the element's current style, animated
    /// values included: the Position Fallback Origin sits below the
    /// Animations origin, so an animation still wins over an option. The
    /// stored [`PositionOptions::base`] is the other thing §6.5.1 compares —
    /// the base style *ignoring* the Animations and Transitions origins — so
    /// a flush recomputes it without them, and an animation tick, which can
    /// change nothing §6.5.1 counts, keeps the one it has.
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
        let guard = engine.shared_lock().read();
        let guards = stylo::shared_lock::StylesheetGuards::same(&guard);
        let _layout_thread = crate::style::flush::LayoutThreadStateGuard::enter();
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

/// `style`, `node`'s computed style, cascaded again without the rules of
/// the Animations, Transitions and SMIL override origins — §6.5.1's
/// "computed base style … ignoring any declarations originating from the
/// Transitions or Animations cascade origins". The style itself when it has
/// none of those rules, which is every style no animation touches; a
/// cascade from the stripped rule node otherwise, with the parents a
/// restyle would use (the flat tree parent, and the nearest one that
/// generates a box for the fixups).
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
            .is_none_or(|style| !style.clone_display().is_contents())
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
    /// The anchor-positioning outcome of `id`'s last committing layout, for
    /// the painter (scroll compensation, `position-visibility`) and readback.
    #[must_use]
    pub(crate) fn anchor_outcome(&self, id: NodeId) -> Option<&AnchorOutcome> {
        self.layout_state().anchored.get(&id)?.outcome.as_ref()
    }

    /// `id`'s remembered scroll offsets (§3.3), recorded at its last anchor
    /// recalculation point.
    #[must_use]
    pub(crate) fn remembered_scroll(&self, id: NodeId) -> Option<&RememberedScroll> {
        self.layout_state().anchored.get(&id)?.remembered.as_ref()
    }

    /// Every anchor-positioned box with its state, in no particular order.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the painter's scroll compensation and position-visibility read it"
        )
    )]
    pub(crate) fn anchored_boxes(&self) -> impl Iterator<Item = (NodeId, &AnchoredBox)> {
        self.layout_state()
            .anchored
            .iter()
            .map(|(&id, entry)| (id, entry))
    }

    /// The element generating `id`'s containing block — the frame its
    /// [`AnchorOutcome`] rectangles and its anchors' rectangles are in.
    #[must_use]
    pub(crate) fn anchor_containing_block(&self, id: NodeId) -> Option<NodeId> {
        containing_block_generator(self.get(id)?).map(Node::id)
    }

    /// The current displacement of `anchor` relative to `containing_block`
    /// by scrolling and sticky positioning — the quantity
    /// [`RememberedScroll`] remembers. Reads the stored scroll offsets and
    /// the rounded boxes of the last run.
    #[must_use]
    pub(crate) fn anchor_displacement(
        &self,
        anchor: NodeId,
        containing_block: Option<NodeId>,
    ) -> Vector2D<f32> {
        let Some(node) = self.get(anchor) else {
            return Vector2D::zero();
        };
        let mut displacement = Vector2D::zero();
        let mut sampled = Vec::new();
        let mut current = Some(node);
        while let Some(step) = current {
            if Some(step.id()) == containing_block {
                break;
            }
            if let Some(style) = step.layout_computed_style() {
                if step.id() != anchor && crate::scroll::is_scroll_container(style) {
                    displacement -= self.scroll_offset(step.id());
                }
                if style.clone_position() == PositionProperty::Sticky {
                    displacement +=
                        crate::visual::sticky::live_offset(self, step.id(), &mut sampled);
                }
            }
            current = containing_block_generator(step);
        }
        displacement
    }

    /// The position option style `id` was last laid out with, when that is
    /// not its base style: what computed-value readback reports for the
    /// accepted `@position-try` properties (§6.5: the chosen styles "affect
    /// computed values").
    #[must_use]
    pub(crate) fn applied_position_option(&self, id: NodeId) -> Option<&ComputedValues> {
        let chosen = self.anchor_outcome(id)?.chosen;
        if chosen == 0 {
            return None;
        }
        option_style(self.arenas(), self.get(id)?, chosen)
    }

    /// The settle step after one layout run: drops the state of boxes that
    /// stopped being anchor-positioned, records the remembered scroll offsets
    /// of boxes the run reached at an anchor recalculation point, and
    /// re-asks every query each box last read, invalidating the boxes whose
    /// answers moved.
    ///
    /// `invalidated` holds the boxes this `layout()` call has invalidated so
    /// far. Every moved box is invalidated, which marks the document dirty;
    /// the answer is whether one of them was not in the set yet, which is
    /// what owes another run. A box already in it is a repeat — its reads
    /// went stale twice in one `layout()` — and earns none: the document
    /// stays dirty, so the next `layout()` or `render()` lays it out again
    /// rather than leaving it settled on a stale read.
    pub(crate) fn settle_anchors(&mut self, invalidated: &mut FxHashSet<NodeId>) -> bool {
        let state = self.layout_state_mut();
        state.anchor_pending.get_mut().clear();
        if state.anchored.is_empty() {
            debug_assert!(state.anchor_reported.is_empty());
            return false;
        }
        let reported = std::mem::take(&mut state.anchor_reported);
        self.retire_anchored_boxes();
        for &id in &reported {
            self.record_remembered_scroll(id);
        }
        let mut reported = reported;
        reported.clear();
        self.layout_state_mut().anchor_reported = reported;
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

    /// Drops the entries of boxes that are gone, generate no box, or no
    /// longer use anything `hughie` reports an outcome for.
    fn retire_anchored_boxes(&mut self) {
        let retired: Vec<NodeId> = {
            let (tree, state) = self.visual_parts();
            state
                .anchored
                .keys()
                .copied()
                .filter(|&id| {
                    let Some(node) = tree.get(id) else {
                        return true;
                    };
                    if state.get(id).is_none_or(|entry| entry.slot.is_hidden()) {
                        return true;
                    }
                    let Some(style) = node.layout_computed_style() else {
                        return true;
                    };
                    // A box whose fallbacks all name unknown rules has one
                    // option, which `hughie` treats as none at all.
                    !is_absolutely_positioned(style)
                        || (!tree.anchors().has_fallbacks(id)
                            && !hughie::compute::uses_anchor_positioning(&super::StyleView::of(
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

    /// §3.3's anchor recalculation point, taken after the run that reached
    /// it: when `id` has no remembered offsets yet (it just began generating
    /// boxes) or its fallback determination switched options, remember the
    /// current displacement of every anchor its chosen option read and of
    /// its default anchor; otherwise only fill in anchors it had not
    /// referenced before, so no answer ever depends on the live offsets
    /// between two recalculation points.
    fn record_remembered_scroll(&mut self, id: NodeId) {
        let Some(entry) = self.layout_state().anchored.get(&id) else {
            return;
        };
        let Some(outcome) = entry.outcome else {
            return;
        };
        let chosen = outcome.chosen;
        // A scroll-driven re-determination laid every option out at the
        // current offsets, the one it kept included, so what it committed
        // is only consistent with offsets remembered now.
        let recalculate = entry.redetermine
            || entry
                .remembered
                .as_ref()
                .is_none_or(|remembered| remembered.option != chosen);
        let mut targets: SmallVec<[NodeId; 2]> = entry
            .reads
            .iter()
            .filter(|read| read.option == chosen && read.query != AnchorQuery::Scrollable)
            .filter_map(|read| read.target)
            .collect();
        let default = {
            let (tree, state) = self.visual_parts();
            tree.get(id).and_then(|node| {
                let query = Query::of(node)?;
                let style = option_style(tree, node, chosen)?;
                default_anchor(tree, &query, style, Liveness::Committed(state)).map(Node::id)
            })
        };
        if let Some(default) = default
            && !targets.contains(&default)
        {
            targets.push(default);
        }
        let containing_block = self.anchor_containing_block(id);
        let previous = if recalculate {
            None
        } else {
            self.layout_state()
                .anchored
                .get(&id)
                .and_then(|entry| entry.remembered.clone())
        };
        let mut remembered = previous.unwrap_or_else(|| RememberedScroll {
            option: chosen,
            ..RememberedScroll::default()
        });
        remembered.default = default;
        for target in targets {
            if remembered.displacement_of(target).is_none() {
                let displacement = self.anchor_displacement(target, containing_block);
                remembered.anchors.push((target, displacement));
            }
        }
        if let Some(entry) = self.layout_state_mut().anchored.get_mut(&id) {
            entry.remembered = Some(remembered);
            entry.redetermine = false;
            entry.fits_scrolled = !outcome.overflows;
        }
    }

    /// `id`'s default scroll shift (§3.3) at the stored offsets: the current
    /// displacement of its default anchor less the remembered one, on the
    /// axes it compensates in, in its containing block's layout space.
    ///
    /// With `ratio`, each displacement is snapped to that device pixel grid
    /// first — `snap(now) − snap(then)`, exactly what the frame's anchored
    /// node composes ([`crate::visual::anchored`]), so the main thread's
    /// fit test and the painter's agree, and a box at rest has a zero shift
    /// whatever fraction of a pixel its scrollers sit at. Without, the exact
    /// shift `now − then`.
    #[must_use]
    pub(crate) fn default_scroll_shift(
        &self,
        id: NodeId,
        ratio: Option<f32>,
    ) -> Option<Vector2D<f32>> {
        let entry = self.layout_state().anchored.get(&id)?;
        let outcome = entry.outcome?;
        let remembered = entry.remembered.as_ref()?;
        let default = remembered.default?;
        let then = remembered.displacement_of(default)?;
        let now = self.anchor_displacement(default, self.anchor_containing_block(id));
        let shift = match ratio {
            Some(ratio) => {
                crate::paint::compose::snap_offset(now, ratio)
                    - crate::paint::compose::snap_offset(then, ratio)
            }
            None => now - then,
        };
        Some(Vector2D::new(
            if outcome.compensates.width {
                shift.x
            } else {
                0.0
            },
            if outcome.compensates.height {
                shift.y
            } else {
                0.0
            },
        ))
    }

    /// css-anchor-position-1 §6.5 on scroll: "When a positioned box (after
    /// applying any default scroll shift) overflows its inset-modified
    /// containing block, and has more than one position option", it
    /// determines position fallback styles. A scroll moves no layout here —
    /// the painter shifts the box — so this asks, for every box with
    /// position options, whether the stored offsets' default scroll shift
    /// took it from fitting to overflowing since it was last checked, and
    /// invalidates the layout of each that did, flagged so its next layout
    /// determines again at the current offsets. Answers whether any was.
    ///
    /// The main thread asks after it adopts the painter's offsets and before
    /// every render, so there is at most one re-determination per adopted
    /// offset, and none while the box stays on one side of its edge. A box
    /// that overflowed in every option at its last layout is re-determined
    /// only once it has fit again and left again.
    pub fn redetermine_scrolled_fallbacks(&mut self) -> bool {
        if self.layout_state().anchored.is_empty() {
            return false;
        }
        let candidates: SmallVec<[NodeId; 4]> = {
            let (tree, state) = self.visual_parts();
            state
                .anchored
                .iter()
                .filter(|&(&id, entry)| {
                    !entry.redetermine
                        && entry.outcome.is_some()
                        && tree.anchors().has_fallbacks(id)
                })
                .map(|(&id, _)| id)
                .collect()
        };
        let mut due: SmallVec<[NodeId; 2]> = SmallVec::new();
        let ratio = self.device_pixel_ratio();
        for id in candidates {
            let Some(shift) = self.default_scroll_shift(id, Some(ratio)) else {
                continue;
            };
            let Some(outcome) = self.anchor_outcome(id).copied() else {
                continue;
            };
            let fits = if shift == Vector2D::zero() {
                !outcome.overflows
            } else {
                fits_shifted(&outcome, shift)
            };
            let Some(entry) = self.layout_state_mut().anchored.get_mut(&id) else {
                continue;
            };
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

    /// The first half of §6.5.1.1 at a rendering update: every box that
    /// made a fallback-sensitive change forgets its last successful option
    /// and is laid out again, so the determination the second half records
    /// starts from its base style. Answers whether any was.
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

    /// The second half of §6.5.1.1: every box with position options records
    /// the option it is now laid out with as its last successful one.
    pub(crate) fn record_last_successful_options(&mut self) {
        let (tree, state, _) = self.layout_parts();
        for (&id, entry) in &mut state.anchored {
            let has_options = tree.anchors().has_fallbacks(id);
            entry.last_successful = entry
                .outcome
                .filter(|_| has_options)
                .map(|outcome| outcome.chosen);
        }
    }

    /// css-anchor-position-1 §2.5: whether `element`, a
    /// `content-visibility: auto` box about to skip its contents, holds a
    /// target anchor element of a positioned box that is not skipped and
    /// whose containing block is outside it — which makes it relevant to
    /// the user. The anchor is looked up as if `element` were not skipping,
    /// since while it skips its descendants are acceptable to nobody.
    #[must_use]
    pub(crate) fn anchor_keeps_relevant(&self, element: NodeId) -> bool {
        let (tree, state) = self.visual_parts();
        if state.anchored.is_empty() {
            return false;
        }
        let liveness = Liveness::Relevance { unskipped: element };
        state.anchored.iter().any(|(&id, entry)| {
            let Some(node) = tree.get(id) else {
                return false;
            };
            if state.get(id).is_none_or(|slot| slot.slot.is_hidden()) {
                return false;
            }
            let Some(query) = Query::of(node) else {
                return false;
            };
            // "whose containing block is not el or a descendant of el".
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
                    AnchorQuery::Scrollable => None,
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
        assert_eq!(doc.dom.anchored_boxes().count(), 0);
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
        assert!(name(&doc, "--a").is_empty());
        assert!(doc.dom.arenas().anchors().knows(anchor), "still a scoper");

        // A detached element keeps its entries — the lookup skips anything
        // not connected — and loses them when it is freed.
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
        assert_eq!(remembered.default, Some(anchor));
        assert_eq!(remembered.displacement_of(anchor), Some(Vector2D::zero()));
        let outcome = doc.dom.anchor_outcome(anchored).expect("reported");
        assert!(outcome.default_anchor_resolved);
        assert!(outcome.compensates.width && outcome.compensates.height);
        assert_eq!(doc.dom.anchor_containing_block(anchored), Some(cb));

        doc.dom.scroll_to(scroller, Vector2D::new(0.0, 30.0));
        assert_eq!(
            doc.dom.anchor_displacement(anchor, Some(cb)),
            Vector2D::new(0.0, -30.0)
        );
        // A recalculation point: the box stops and starts generating one.
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
        // The scroller records its in-flow overflow before its absolute pass,
        // so the box laid out against it reads this run's area.
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

    /// The reviewer's page: a box laid out against a scrollable containing
    /// block that it overflows itself. The area ignores it, so the box does
    /// not grow it — before, the area grew by the box's own overhang on every
    /// pass (250, 350, 450 …).
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
            // Each pass lays the box out again, so its containing block's
            // absolute pass reads the area again.
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

    /// A chain through boxes that escape static wrappers is laid out in one
    /// run: every link reads a box its containing block placed before it.
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

    /// The settle loop's bound is per box, not a constant: a chain whose
    /// every hop is served from the cache when the anchor before it moves
    /// costs one run per stale hop, and `layout()` ends settled.
    ///
    /// Each reader `hop{k}` is the `contain: strict` boundary `ring{k}`'s
    /// absolutely positioned child and reads `--hop{k-1}`, which sits inside
    /// `ring{k-1}`, `ring{k}`'s in-flow child; `ring0` holds the in-flow
    /// anchor `--hop0`. Resizing that anchor dirties `ring0` only, and
    /// relaying `hop{k}` dirties `ring{k}` only (a strict boundary's size does
    /// not depend on its content), so every run moves exactly one more hop
    /// and leaves the next one served from the cache. Two stale hops is
    /// the three-hop chain `--hop0` → `hop1` → `hop2`; nine runs is more than
    /// the six the removed constant allowed.
    #[test]
    fn each_stale_hop_of_an_anchor_chain_costs_one_run() {
        use std::fmt::Write as _;

        // Every hop sits 10px below the bottom of the one before it.
        let assert_tops = |doc: &Doc, hops: &[crate::NodeId], first: f32| {
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
            let viewport = hughie::geometry::Size::new(viewport.width, viewport.height);
            let scale = doc.dom.device().device_pixel_ratio().get();
            assert!(
                !doc.dom.layout_needs_pass(viewport, scale),
                "{stale}: settled within the call"
            );
        }
    }

    /// The bound in action, with no hook: a box that reads two anchors whose
    /// moves surface in different runs goes stale twice in one `layout()`.
    /// `b` reads `--a1` (behind one boundary) and `--x2`, the end of the
    /// chain `--a0` → `x1` → `x2`, each link behind its own boundary.
    /// Resizing both anchors in one flush: the first run moves `a0` and
    /// `a1`; the second relays `b` (for `--a1`) and `x1`, while `x2` is still
    /// served from the cache; the third relays `x2`, which leaves `b` stale
    /// again. `b` was already invalidated in this call, so it earns no fourth
    /// run: it is invalidated, the document stays dirty, and the next
    /// `layout()` — with nothing else changed — places it.
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
        let viewport = hughie::geometry::Size::new(viewport.width, viewport.height);
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

    /// §2.2 + §2.3 cost: N list items scoping one shared name, each over one
    /// anchor and one box reading it. A lookup sees only the definers under
    /// the query box's scopes, and the settle loop re-reads a found target's
    /// rectangle without looking it up again, so a layout examines O(N)
    /// candidates — not N per box.
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
            // A relayout of the list's container re-runs every box.
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

    /// §6.5.1 compares the base style ignoring the Animations and
    /// Transitions origins: the stored base never carries an animated value,
    /// and a flush during the animation is not a fallback-sensitive change.
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

    /// §6.5.1: a change of containing block association is
    /// fallback-sensitive.
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

    /// A box whose fallbacks all name unknown rules has no options, so once
    /// it stops using anchor positioning nothing is reported for it and its
    /// state goes, rather than keeping a stale outcome for the painter.
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
        assert!(!doc.dom.layout_state().anchored.contains_key(&target));
    }

    /// `match-parent` matches the default anchor of the option its parent
    /// was laid out with, not its base style's.
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
