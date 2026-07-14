//! Immutable DOM formatting-tree projection for neutron-star.
//!
//! [`DomLayoutSource`] borrows an [`Arena`] for one layout epoch. It owns only
//! dense formatting-tree metadata and strong references to computed styles;
//! DOM topology and Text character data stay in the arena. This keeps the
//! immutable source physically separate from the runtime's mutable layout,
//! measurement, and cache session.

use core::iter::FusedIterator;
use core::num::NonZeroU64;
use core::{fmt, slice};

use neutron_star::geometry::{Edges, Line, Point, Size};
use neutron_star::style::{
    AlignContent, AlignItems, AlignSelf, BoxGenerationMode, BoxSizing, CalcHandle, CoreStyle,
    Dimension, Direction, FlexContainerStyle, FlexDirection, FlexItemStyle, FlexWrap, GridAutoFlow,
    GridContainerStyle, GridItemStyle, GridPlacement, GridTemplateComponent,
    GridTemplateRepetition, JustifyContent, JustifyItems, JustifySelf, LengthPercentage,
    LengthPercentageAuto, LinearContainerStyle, LinearCrossGravity, LinearGravity, LinearItemStyle,
    LinearLayoutGravity, LinearOrientation, MaxTrackSizingFunction, MinTrackSizingFunction,
    Overflow, Position, RelativeCenter, RelativeContainerStyle, RelativeItemStyle,
    RelativeReference, TextAlign, TextContainerStyle, TextRun, TrackSizingFunction, Visibility,
    WhiteSpace, WordBreak,
};
use neutron_star::tree::{
    FlexSource, GridSource, LayoutSource, LinearSource, NodeId as LayoutNodeId, RelativeSource,
    TraverseTree,
};
use rustc_hash::{FxHashMap, FxHashSet};
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;

use super::style::{
    ComputedGridRepetition, ComputedGridTemplateTracks, ComputedGridTracks, ComputedLayoutStyle,
    ComputedTextRunStyle, LayoutDisplay, resolve_calc,
};
use crate::{Arena, ElementId, Node, NodeId as DomNodeId};

/// Embedder-neutral formatting role for an Element.
///
/// Standard `display` still comes from the element's computed style. These
/// roles cover host concepts that CSS cannot infer, without teaching
/// `stylo-dom` any embedder tag names or widget vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LayoutNodeRole {
    /// An ordinary element whose computed `display` decides its box.
    #[default]
    Normal,
    /// A host fragment: do not create a box and promote its formatting
    /// children. Standard `display: contents` is handled identically even
    /// when this role is `Normal`.
    Contents,
    /// Replaced/custom content measured by the host as a leaf.
    Replaced,
    /// A host character-data carrier. The carrier creates no box; its real
    /// DOM Text descendants join the surrounding anonymous text item.
    TextCarrier,
}

/// Neutral layout policy supplied by an arena Element's external payload.
///
/// The default methods describe an ordinary CSS element. An embedder can
/// implement this on its own payload type to identify fragments, replaced
/// content, or transparent Text carriers.
pub trait LayoutNodePolicy {
    /// The host-only formatting role of this Element.
    fn layout_node_role(&self) -> LayoutNodeRole {
        LayoutNodeRole::Normal
    }

    /// Whether newline characters in Text below this carrier are forced line
    /// breaks independently of the container's computed `white-space`.
    fn preserve_newlines(&self) -> bool {
        false
    }
}

impl LayoutNodePolicy for () {}

/// Host dispatch for one formatting-tree node.
///
/// `display: none` is represented by [`CoreStyle::box_generation_mode`] and
/// must be handled before consulting this value. An anonymous text item is a
/// leaf measured through [`DomLayoutSource::text_runs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DomLayoutDisplay {
    /// CSS Flexbox container.
    Flex,
    /// CSS Grid container.
    Grid,
    /// Lynx Linear container.
    Linear,
    /// Lynx Relative container.
    Relative,
    /// An ordinary flow box whose algorithm belongs to the host.
    Flow,
    /// Replaced/custom leaf content.
    Leaf,
    /// Generated anonymous text item.
    AnonymousText,
}

/// Failure to create a DOM layout epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomLayoutSourceError {
    /// The supplied root does not identify a live Element.
    StaleRoot(ElementId),
    /// The root has not completed style computation.
    MissingRootStyle(ElementId),
    /// A visible descendant has not completed style computation.
    MissingStyle(ElementId),
    /// A root cannot itself be flattened into a containing formatting tree.
    FlattenedRoot(ElementId),
}

impl fmt::Display for DomLayoutSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::StaleRoot(root) => {
                write!(f, "DOM layout root {root:?} is stale or not an Element")
            }
            Self::MissingRootStyle(root) => {
                write!(
                    f,
                    "DOM layout root {root:?} has no computed style; flush styles first"
                )
            }
            Self::MissingStyle(element) => write!(
                f,
                "DOM layout element {element:?} has no computed style; flush styles first"
            ),
            Self::FlattenedRoot(root) => write!(
                f,
                "DOM layout root {root:?} is display: contents or a transparent host node"
            ),
        }
    }
}

impl std::error::Error for DomLayoutSourceError {}

#[derive(Debug, Clone, Copy)]
struct InitialStyle;

impl CoreStyle for InitialStyle {}
impl FlexContainerStyle for InitialStyle {}
impl FlexItemStyle for InitialStyle {}
impl GridItemStyle for InitialStyle {}
impl LinearContainerStyle for InitialStyle {}
impl LinearItemStyle for InitialStyle {}
impl RelativeContainerStyle for InitialStyle {}
impl RelativeItemStyle for InitialStyle {}
impl TextContainerStyle for InitialStyle {}

/// Style view used for actual boxes and anonymous text items.
///
/// Anonymous items receive protocol initial box/item values. Only inherited
/// values relevant to text (`visibility`, `direction`, and
/// [`TextContainerStyle`]) delegate to their formatting parent. This is what
/// prevents a parent's flex/grid item properties from leaking into the
/// generated anonymous item.
#[derive(Debug, Clone, Copy)]
pub enum DomLayoutStyle<'a> {
    /// A box backed by an Element's computed style.
    Computed(ComputedLayoutStyle<'a>),
    /// A generated text item inheriting from its formatting parent.
    Anonymous(ComputedLayoutStyle<'a>),
}

impl<'a> DomLayoutStyle<'a> {
    fn computed(self) -> ComputedLayoutStyle<'a> {
        match self {
            Self::Computed(style) | Self::Anonymous(style) => style,
        }
    }

    fn actual(self) -> Option<ComputedLayoutStyle<'a>> {
        match self {
            Self::Computed(style) => Some(style),
            Self::Anonymous(_) => None,
        }
    }
}

impl CoreStyle for DomLayoutStyle<'_> {
    fn box_generation_mode(&self) -> BoxGenerationMode {
        match *self {
            Self::Computed(style) => style.box_generation_mode(),
            Self::Anonymous(_) => BoxGenerationMode::Normal,
        }
    }

    fn visibility(&self) -> Visibility {
        CoreStyle::visibility(&self.computed())
    }

    fn position(&self) -> Position {
        self.actual().map_or_else(
            || InitialStyle.position(),
            |style| CoreStyle::position(&style),
        )
    }

    fn inset(&self) -> Edges<LengthPercentageAuto> {
        self.actual()
            .map_or_else(|| InitialStyle.inset(), |style| CoreStyle::inset(&style))
    }

    fn size(&self) -> Size<Dimension> {
        self.actual()
            .map_or_else(|| InitialStyle.size(), |style| CoreStyle::size(&style))
    }

    fn min_size(&self) -> Size<Dimension> {
        self.actual().map_or_else(
            || InitialStyle.min_size(),
            |style| CoreStyle::min_size(&style),
        )
    }

    fn max_size(&self) -> Size<Dimension> {
        self.actual().map_or_else(
            || InitialStyle.max_size(),
            |style| CoreStyle::max_size(&style),
        )
    }

    fn aspect_ratio(&self) -> Option<f32> {
        self.actual().map_or_else(
            || InitialStyle.aspect_ratio(),
            |style| CoreStyle::aspect_ratio(&style),
        )
    }

    fn margin(&self) -> Edges<LengthPercentageAuto> {
        self.actual()
            .map_or_else(|| InitialStyle.margin(), |style| CoreStyle::margin(&style))
    }

    fn padding(&self) -> Edges<LengthPercentage> {
        self.actual().map_or_else(
            || InitialStyle.padding(),
            |style| CoreStyle::padding(&style),
        )
    }

    fn border(&self) -> Edges<LengthPercentage> {
        self.actual()
            .map_or_else(|| InitialStyle.border(), |style| CoreStyle::border(&style))
    }

    fn overflow(&self) -> Point<Overflow> {
        self.actual().map_or_else(
            || InitialStyle.overflow(),
            |style| CoreStyle::overflow(&style),
        )
    }

    fn scrollbar_width(&self) -> f32 {
        self.actual().map_or_else(
            || InitialStyle.scrollbar_width(),
            |style| CoreStyle::scrollbar_width(&style),
        )
    }

    fn box_sizing(&self) -> BoxSizing {
        self.actual().map_or_else(
            || InitialStyle.box_sizing(),
            |style| CoreStyle::box_sizing(&style),
        )
    }

    fn direction(&self) -> Direction {
        CoreStyle::direction(&self.computed())
    }
}

impl FlexContainerStyle for DomLayoutStyle<'_> {
    fn flex_direction(&self) -> FlexDirection {
        self.actual().map_or_else(
            || InitialStyle.flex_direction(),
            |style| FlexContainerStyle::flex_direction(&style),
        )
    }

    fn flex_wrap(&self) -> FlexWrap {
        self.actual().map_or_else(
            || InitialStyle.flex_wrap(),
            |style| FlexContainerStyle::flex_wrap(&style),
        )
    }

    fn gap(&self) -> Size<LengthPercentage> {
        self.actual().map_or_else(
            || FlexContainerStyle::gap(&InitialStyle),
            |style| FlexContainerStyle::gap(&style),
        )
    }

    fn align_content(&self) -> Option<AlignContent> {
        self.actual().map_or_else(
            || InitialStyle.align_content(),
            |style| FlexContainerStyle::align_content(&style),
        )
    }

    fn align_items(&self) -> Option<AlignItems> {
        self.actual().map_or_else(
            || FlexContainerStyle::align_items(&InitialStyle),
            |style| FlexContainerStyle::align_items(&style),
        )
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        self.actual().map_or_else(
            || FlexContainerStyle::justify_content(&InitialStyle),
            |style| FlexContainerStyle::justify_content(&style),
        )
    }
}

impl FlexItemStyle for DomLayoutStyle<'_> {
    fn flex_basis(&self) -> Dimension {
        self.actual().map_or_else(
            || InitialStyle.flex_basis(),
            |style| FlexItemStyle::flex_basis(&style),
        )
    }

    fn flex_grow(&self) -> f32 {
        self.actual().map_or_else(
            || InitialStyle.flex_grow(),
            |style| FlexItemStyle::flex_grow(&style),
        )
    }

    fn flex_shrink(&self) -> f32 {
        self.actual().map_or_else(
            || InitialStyle.flex_shrink(),
            |style| FlexItemStyle::flex_shrink(&style),
        )
    }

    fn align_self(&self) -> Option<AlignSelf> {
        self.actual().map_or_else(
            || FlexItemStyle::align_self(&InitialStyle),
            |style| FlexItemStyle::align_self(&style),
        )
    }

    fn order(&self) -> i32 {
        self.actual().map_or_else(
            || FlexItemStyle::order(&InitialStyle),
            |style| FlexItemStyle::order(&style),
        )
    }
}

/// Optional wrapper around a computed-style iterator.
///
/// Anonymous container views use the empty branch because generated text
/// items cannot themselves establish Grid containers.
#[derive(Debug, Clone)]
pub enum OptionalStyleIter<I> {
    /// Iterate the computed sequence.
    Some(I),
    /// The protocol's empty/initial sequence.
    Empty,
}

impl<I: Iterator> Iterator for OptionalStyleIter<I> {
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Some(iter) => iter.next(),
            Self::Empty => None,
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Some(iter) => iter.size_hint(),
            Self::Empty => (0, Some(0)),
        }
    }
}

impl<I: DoubleEndedIterator> DoubleEndedIterator for OptionalStyleIter<I> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match self {
            Self::Some(iter) => iter.next_back(),
            Self::Empty => None,
        }
    }
}

impl<I: ExactSizeIterator> ExactSizeIterator for OptionalStyleIter<I> {}
impl<I: FusedIterator> FusedIterator for OptionalStyleIter<I> {}

impl GridContainerStyle for DomLayoutStyle<'_> {
    type Repetition<'a>
        = ComputedGridRepetition<'a>
    where
        Self: 'a;
    type TemplateTracks<'a>
        = OptionalStyleIter<ComputedGridTemplateTracks<'a>>
    where
        Self: 'a;
    type AutoTracks<'a>
        = OptionalStyleIter<ComputedGridTracks<'a>>
    where
        Self: 'a;

    fn grid_template_rows(&self) -> Self::TemplateTracks<'_> {
        match self {
            Self::Computed(style) => {
                OptionalStyleIter::Some(GridContainerStyle::grid_template_rows(style))
            }
            Self::Anonymous(_) => OptionalStyleIter::Empty,
        }
    }

    fn grid_template_columns(&self) -> Self::TemplateTracks<'_> {
        match self {
            Self::Computed(style) => {
                OptionalStyleIter::Some(GridContainerStyle::grid_template_columns(style))
            }
            Self::Anonymous(_) => OptionalStyleIter::Empty,
        }
    }

    fn grid_auto_rows(&self) -> Self::AutoTracks<'_> {
        match self {
            Self::Computed(style) => {
                OptionalStyleIter::Some(GridContainerStyle::grid_auto_rows(style))
            }
            Self::Anonymous(_) => OptionalStyleIter::Empty,
        }
    }

    fn grid_auto_columns(&self) -> Self::AutoTracks<'_> {
        match self {
            Self::Computed(style) => {
                OptionalStyleIter::Some(GridContainerStyle::grid_auto_columns(style))
            }
            Self::Anonymous(_) => OptionalStyleIter::Empty,
        }
    }

    fn grid_auto_flow(&self) -> GridAutoFlow {
        self.actual().map_or_else(GridAutoFlow::default, |style| {
            GridContainerStyle::grid_auto_flow(&style)
        })
    }

    fn gap(&self) -> Size<LengthPercentage> {
        self.actual().map_or_else(
            || Size::new(LengthPercentage::ZERO, LengthPercentage::ZERO),
            |style| GridContainerStyle::gap(&style),
        )
    }

    fn align_content(&self) -> Option<AlignContent> {
        self.actual()
            .and_then(|style| GridContainerStyle::align_content(&style))
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        self.actual()
            .and_then(|style| GridContainerStyle::justify_content(&style))
    }

    fn align_items(&self) -> Option<AlignItems> {
        self.actual()
            .and_then(|style| GridContainerStyle::align_items(&style))
    }

    fn justify_items(&self) -> Option<JustifyItems> {
        self.actual()
            .and_then(|style| GridContainerStyle::justify_items(&style))
    }
}

impl GridItemStyle for DomLayoutStyle<'_> {
    fn grid_row(&self) -> Line<GridPlacement> {
        self.actual().map_or_else(
            || InitialStyle.grid_row(),
            |style| GridItemStyle::grid_row(&style),
        )
    }

    fn grid_column(&self) -> Line<GridPlacement> {
        self.actual().map_or_else(
            || InitialStyle.grid_column(),
            |style| GridItemStyle::grid_column(&style),
        )
    }

    fn align_self(&self) -> Option<AlignSelf> {
        self.actual().map_or_else(
            || GridItemStyle::align_self(&InitialStyle),
            |style| GridItemStyle::align_self(&style),
        )
    }

    fn justify_self(&self) -> Option<JustifySelf> {
        self.actual().map_or_else(
            || InitialStyle.justify_self(),
            |style| GridItemStyle::justify_self(&style),
        )
    }

    fn order(&self) -> i32 {
        self.actual().map_or_else(
            || GridItemStyle::order(&InitialStyle),
            |style| GridItemStyle::order(&style),
        )
    }
}

impl LinearContainerStyle for DomLayoutStyle<'_> {
    fn linear_orientation(&self) -> LinearOrientation {
        self.actual().map_or_else(
            || InitialStyle.linear_orientation(),
            |style| LinearContainerStyle::linear_orientation(&style),
        )
    }

    fn linear_gravity(&self) -> LinearGravity {
        self.actual().map_or_else(
            || InitialStyle.linear_gravity(),
            |style| LinearContainerStyle::linear_gravity(&style),
        )
    }

    fn linear_cross_gravity(&self) -> LinearCrossGravity {
        self.actual().map_or_else(
            || InitialStyle.linear_cross_gravity(),
            |style| LinearContainerStyle::linear_cross_gravity(&style),
        )
    }

    fn linear_weight_sum(&self) -> f32 {
        self.actual().map_or_else(
            || InitialStyle.linear_weight_sum(),
            |style| LinearContainerStyle::linear_weight_sum(&style),
        )
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        self.actual().map_or_else(
            || LinearContainerStyle::justify_content(&InitialStyle),
            |style| LinearContainerStyle::justify_content(&style),
        )
    }

    fn align_items(&self) -> Option<AlignItems> {
        self.actual().map_or_else(
            || LinearContainerStyle::align_items(&InitialStyle),
            |style| LinearContainerStyle::align_items(&style),
        )
    }
}

impl LinearItemStyle for DomLayoutStyle<'_> {
    fn linear_layout_gravity(&self) -> LinearLayoutGravity {
        self.actual().map_or_else(
            || InitialStyle.linear_layout_gravity(),
            |style| LinearItemStyle::linear_layout_gravity(&style),
        )
    }

    fn linear_weight(&self) -> f32 {
        self.actual().map_or_else(
            || InitialStyle.linear_weight(),
            |style| LinearItemStyle::linear_weight(&style),
        )
    }

    fn align_self(&self) -> Option<AlignSelf> {
        self.actual().map_or_else(
            || LinearItemStyle::align_self(&InitialStyle),
            |style| LinearItemStyle::align_self(&style),
        )
    }

    fn order(&self) -> i32 {
        self.actual().map_or_else(
            || LinearItemStyle::order(&InitialStyle),
            |style| LinearItemStyle::order(&style),
        )
    }
}

impl RelativeContainerStyle for DomLayoutStyle<'_> {
    fn relative_layout_once(&self) -> bool {
        self.actual().map_or_else(
            || InitialStyle.relative_layout_once(),
            |style| RelativeContainerStyle::relative_layout_once(&style),
        )
    }
}

impl RelativeItemStyle for DomLayoutStyle<'_> {
    fn relative_id(&self) -> RelativeReference {
        self.actual().map_or_else(
            || InitialStyle.relative_id(),
            |style| RelativeItemStyle::relative_id(&style),
        )
    }

    fn relative_align(&self) -> Edges<RelativeReference> {
        self.actual().map_or_else(
            || InitialStyle.relative_align(),
            |style| RelativeItemStyle::relative_align(&style),
        )
    }

    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        self.actual().map_or_else(
            || InitialStyle.relative_adjacent(),
            |style| RelativeItemStyle::relative_adjacent(&style),
        )
    }

    fn relative_center(&self) -> RelativeCenter {
        self.actual().map_or_else(
            || InitialStyle.relative_center(),
            |style| RelativeItemStyle::relative_center(&style),
        )
    }

    fn order(&self) -> i32 {
        self.actual().map_or_else(
            || RelativeItemStyle::order(&InitialStyle),
            |style| RelativeItemStyle::order(&style),
        )
    }
}

impl TextContainerStyle for DomLayoutStyle<'_> {
    fn text_align(&self) -> TextAlign {
        TextContainerStyle::text_align(&self.computed())
    }

    fn white_space(&self) -> WhiteSpace {
        TextContainerStyle::white_space(&self.computed())
    }

    fn word_break(&self) -> WordBreak {
        TextContainerStyle::word_break(&self.computed())
    }

    fn text_indent(&self) -> LengthPercentage {
        TextContainerStyle::text_indent(&self.computed())
    }
}

#[derive(Debug)]
struct TextRunRecord {
    text: DomNodeId,
    style: ComputedTextRunStyle,
    preserve_newlines: bool,
}

#[derive(Debug)]
enum FormattingNodeKind {
    Element {
        element: ElementId,
        role: LayoutNodeRole,
        computed: Arc<ComputedValues>,
    },
    AnonymousText {
        container: ElementId,
        contributors: Box<[DomNodeId]>,
        runs: Box<[TextRunRecord]>,
    },
}

#[derive(Debug)]
struct FormattingNode {
    id: LayoutNodeId,
    kind: FormattingNodeKind,
    children: Box<[LayoutNodeId]>,
}

impl FormattingNode {
    fn element(&self) -> Option<ElementId> {
        match self.kind {
            FormattingNodeKind::Element { element, .. } => Some(element),
            FormattingNodeKind::AnonymousText { .. } => None,
        }
    }

    fn computed(&self) -> Option<&Arc<ComputedValues>> {
        match &self.kind {
            FormattingNodeKind::Element { computed, .. } => Some(computed),
            FormattingNodeKind::AnonymousText { .. } => None,
        }
    }
}

#[derive(Debug)]
struct PendingText {
    text: DomNodeId,
    style: Arc<ComputedValues>,
    preserve_newlines: bool,
}

#[derive(Debug)]
enum FormattingChild {
    Box(ElementId),
    Text(PendingText),
}

/// Borrowed Text-run iterator for one generated anonymous item.
pub struct DomTextRuns<'a, T> {
    arena: &'a Arena<T>,
    inner: slice::Iter<'a, TextRunRecord>,
}

impl<T> Clone for DomTextRuns<'_, T> {
    fn clone(&self) -> Self {
        Self {
            arena: self.arena,
            inner: self.inner.clone(),
        }
    }
}

impl<T> fmt::Debug for DomTextRuns<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DomTextRuns")
            .field("remaining", &self.inner.len())
            .finish()
    }
}

impl<'a, T> Iterator for DomTextRuns<'a, T> {
    type Item = TextRun<'a, ComputedTextRunStyle>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|run| TextRun {
            text: self
                .arena
                .text(run.text)
                .expect("layout epoch retains every Text contributor")
                .data(),
            style: &run.style,
            preserve_newlines: run.preserve_newlines,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<T> DoubleEndedIterator for DomTextRuns<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next_back().map(|run| TextRun {
            text: self
                .arena
                .text(run.text)
                .expect("layout epoch retains every Text contributor")
                .data(),
            style: &run.style,
            preserve_newlines: run.preserve_newlines,
        })
    }
}

impl<T> ExactSizeIterator for DomTextRuns<'_, T> {}
impl<T> FusedIterator for DomTextRuns<'_, T> {}

/// Immutable neutron-star source projected directly from a styled DOM arena.
///
/// Layout ids are source-local dense indices. Actual Elements and generated
/// anonymous text items share that one id space; [`layout_node`](Self::layout_node)
/// and [`element_id`](Self::element_id) provide explicit identity mapping
/// instead of encoding DOM ids into neutron-star's opaque handle.
pub struct DomLayoutSource<'arena, T> {
    arena: &'arena Arena<T>,
    arena_identity: NonZeroU64,
    root_element: ElementId,
    root_node: LayoutNodeId,
    revision: u64,
    nodes: Vec<FormattingNode>,
    by_dom: FxHashMap<DomNodeId, LayoutNodeId>,
    calc_handles: FxHashSet<u64>,
}

impl<T> fmt::Debug for DomLayoutSource<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DomLayoutSource")
            .field("root_element", &self.root_element)
            .field("root_node", &self.root_node)
            .field("revision", &self.revision)
            .field("node_count", &self.nodes.len())
            .finish_non_exhaustive()
    }
}

impl<'arena, T: LayoutNodePolicy> DomLayoutSource<'arena, T> {
    /// Project the formatting subtree rooted at `root`.
    ///
    /// Callers finish Stylo's flush before constructing an epoch. The source
    /// then keeps computed-style allocations alive while borrowing topology
    /// and character data from `arena`.
    pub fn new(arena: &'arena Arena<T>, root: ElementId) -> Result<Self, DomLayoutSourceError> {
        let root_element = arena
            .get(root)
            .ok_or(DomLayoutSourceError::StaleRoot(root))?;
        let root_computed = root_element
            .computed_style()
            .ok_or(DomLayoutSourceError::MissingRootStyle(root))?;
        let root_display = ComputedLayoutStyle::new(&root_computed).layout_display();
        if root_display == LayoutDisplay::Contents
            || matches!(
                root_element.ext.layout_node_role(),
                LayoutNodeRole::Contents | LayoutNodeRole::TextCarrier
            )
        {
            return Err(DomLayoutSourceError::FlattenedRoot(root));
        }

        let mut source = Self {
            arena,
            arena_identity: arena.layout_identity(),
            root_element: root,
            root_node: LayoutNodeId::new(0),
            revision: arena.layout_revision(),
            nodes: Vec::new(),
            by_dom: FxHashMap::default(),
            calc_handles: FxHashSet::default(),
        };
        source.root_node = source.push_element(root)?;
        source.collect_calc_handles();
        Ok(source)
    }

    /// DOM mutation/style revision captured by this immutable epoch.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) const fn arena_identity(&self) -> NonZeroU64 {
        self.arena_identity
    }

    /// Root Element of this epoch.
    #[must_use]
    pub const fn root_element(&self) -> ElementId {
        self.root_element
    }

    /// Root neutron-star node of this epoch.
    #[must_use]
    pub const fn root_node(&self) -> LayoutNodeId {
        self.root_node
    }

    /// All formatting node ids in deterministic depth-first construction
    /// order. The ids are dense from zero through `len - 1`.
    #[must_use]
    pub fn node_ids(
        &self,
    ) -> impl Clone + DoubleEndedIterator<Item = LayoutNodeId> + ExactSizeIterator + '_ {
        self.nodes.iter().map(|node| node.id)
    }

    /// Actual Element↔layout-node mappings in formatting-tree order.
    #[must_use]
    pub fn mappings(
        &self,
    ) -> impl Clone + DoubleEndedIterator<Item = (ElementId, LayoutNodeId)> + '_ {
        self.nodes
            .iter()
            .filter_map(|node| node.element().map(|element| (element, node.id)))
    }

    /// Actual layout-node↔Element mappings in formatting-tree order.
    #[must_use]
    pub fn node_mappings(
        &self,
    ) -> impl Clone + DoubleEndedIterator<Item = (LayoutNodeId, ElementId)> + '_ {
        self.mappings().map(|(element, node)| (node, element))
    }

    /// Resolve a DOM Element or Text contributor to its formatting node.
    ///
    /// Elements flattened by `display: contents`, `Contents`, or
    /// `TextCarrier` return `None`; their Text descendants map to the
    /// generated anonymous item that consumes them.
    #[must_use]
    pub fn layout_node(&self, dom: DomNodeId) -> Option<LayoutNodeId> {
        self.by_dom.get(&dom).copied()
    }

    /// Alias specialized for callers holding an Element id.
    #[must_use]
    pub fn element_node(&self, element: ElementId) -> Option<LayoutNodeId> {
        self.layout_node(element)
            .filter(|&node| self.element_id(node).is_some())
    }

    /// Resolve an actual layout box back to its Element.
    /// Anonymous text items return `None`.
    #[must_use]
    pub fn element_id(&self, node: LayoutNodeId) -> Option<ElementId> {
        self.node(node).element()
    }

    /// First anonymous text item generated directly for `element`.
    ///
    /// This is a convenience for hosts that retain paragraph artifacts by
    /// their containing Element. It returns `None` when the Element is not a
    /// layout box or has no non-whitespace direct formatting text.
    #[must_use]
    pub fn anonymous_text_child(&self, element: ElementId) -> Option<LayoutNodeId> {
        self.anonymous_text_children(element).next()
    }

    /// Anonymous text items generated directly for `element`, in formatting
    /// order. A box child can separate two otherwise-adjacent Text sequences,
    /// so callers that need complete output must not assume there is only one.
    #[must_use]
    pub fn anonymous_text_children(
        &self,
        element: ElementId,
    ) -> impl Clone + DoubleEndedIterator<Item = LayoutNodeId> + '_ {
        let children = self
            .element_node(element)
            .map_or(&[][..], |parent| self.node(parent).children.as_ref());
        children.iter().copied().filter(|&child| {
            matches!(
                self.node(child).kind,
                FormattingNodeKind::AnonymousText { .. }
            )
        })
    }

    /// DOM Text nodes contributing to an anonymous text item.
    ///
    /// Actual Element nodes return an empty iterator.
    #[must_use]
    pub fn text_contributors(
        &self,
        node: LayoutNodeId,
    ) -> impl Clone + DoubleEndedIterator<Item = DomNodeId> + ExactSizeIterator + '_ {
        let contributors = match &self.node(node).kind {
            FormattingNodeKind::AnonymousText { contributors, .. } => contributors.as_ref(),
            FormattingNodeKind::Element { .. } => &[],
        };
        contributors.iter().copied()
    }

    /// Host display dispatch for `node`.
    ///
    /// `display: none` descendants are absent from this source. A root whose
    /// own computed display is `none` reports [`DomLayoutDisplay::Flow`], but
    /// its [`CoreStyle::box_generation_mode`] remains authoritative.
    #[must_use]
    pub fn display(&self, node: LayoutNodeId) -> DomLayoutDisplay {
        match &self.node(node).kind {
            FormattingNodeKind::AnonymousText { .. } => DomLayoutDisplay::AnonymousText,
            FormattingNodeKind::Element {
                role: LayoutNodeRole::Replaced,
                ..
            } => DomLayoutDisplay::Leaf,
            FormattingNodeKind::Element { computed, .. } => {
                match ComputedLayoutStyle::new(computed).layout_display() {
                    LayoutDisplay::Flex => DomLayoutDisplay::Flex,
                    LayoutDisplay::Grid => DomLayoutDisplay::Grid,
                    LayoutDisplay::Linear => DomLayoutDisplay::Linear,
                    LayoutDisplay::Relative => DomLayoutDisplay::Relative,
                    LayoutDisplay::None | LayoutDisplay::Contents | LayoutDisplay::Flow => {
                        DomLayoutDisplay::Flow
                    }
                }
            }
        }
    }

    /// Paragraph style inherited by a generated anonymous text item from its
    /// formatting parent.
    ///
    /// # Panics
    ///
    /// Panics when `node` is not an anonymous text item.
    #[must_use]
    pub fn text_container_style(&self, node: LayoutNodeId) -> DomLayoutStyle<'_> {
        let FormattingNodeKind::AnonymousText { container, .. } = self.node(node).kind else {
            panic!("layout node {node:?} is not an anonymous text item");
        };
        // Resolve through the parent's formatting node so the returned view
        // borrows the source-owned style allocation.
        let parent = self
            .element_node(container)
            .expect("anonymous text formatting parent generates a box");
        let computed = self
            .node(parent)
            .computed()
            .expect("visible formatting parent retains computed style");
        DomLayoutStyle::Anonymous(ComputedLayoutStyle::new(computed))
    }

    /// Borrow the literal DOM Text and computed shaping runs for an anonymous
    /// item without copying character data.
    ///
    /// # Panics
    ///
    /// Panics when `node` is not an anonymous text item.
    #[must_use]
    pub fn text_runs(&self, node: LayoutNodeId) -> DomTextRuns<'_, T> {
        let FormattingNodeKind::AnonymousText { runs, .. } = &self.node(node).kind else {
            panic!("layout node {node:?} is not an anonymous text item");
        };
        DomTextRuns {
            arena: self.arena,
            inner: runs.iter(),
        }
    }

    /// Source-owned computed style for an actual visible node.
    #[must_use]
    pub fn computed_style(&self, node: LayoutNodeId) -> Option<&Arc<ComputedValues>> {
        self.node(node).computed()
    }

    fn node(&self, node: LayoutNodeId) -> &FormattingNode {
        let index = usize::try_from(node.get()).expect("layout node id exceeds usize");
        self.nodes
            .get(index)
            .filter(|entry| entry.id == node)
            .unwrap_or_else(|| panic!("NodeId {node:?} is not part of this DOM layout epoch"))
    }

    fn style(&self, node: LayoutNodeId) -> DomLayoutStyle<'_> {
        match &self.node(node).kind {
            FormattingNodeKind::Element { computed, .. } => {
                DomLayoutStyle::Computed(ComputedLayoutStyle::new(computed))
            }
            FormattingNodeKind::AnonymousText { container, .. } => {
                let parent = self
                    .element_node(*container)
                    .expect("anonymous formatting parent must generate a box");
                let computed = self
                    .node(parent)
                    .computed()
                    .expect("anonymous formatting parent must be visible and styled");
                DomLayoutStyle::Anonymous(ComputedLayoutStyle::new(computed))
            }
        }
    }

    fn push_element(&mut self, element: ElementId) -> Result<LayoutNodeId, DomLayoutSourceError> {
        let dom_element = self
            .arena
            .get(element)
            .expect("projected Element must stay live for the borrowed epoch");
        let computed = dom_element
            .computed_style()
            .ok_or(DomLayoutSourceError::MissingStyle(element))?;
        let display = ComputedLayoutStyle::new(&computed).layout_display();
        let hides_descendants = display == LayoutDisplay::None;
        let role = dom_element.ext.layout_node_role();

        let node = self.reserve_node(FormattingNodeKind::Element {
            element,
            role,
            computed,
        });
        self.by_dom.insert(element, node);

        let children = if hides_descendants || role == LayoutNodeRole::Replaced {
            Vec::new()
        } else {
            self.formatting_children(element)?
        };
        let index = usize::try_from(node.get()).expect("layout node id exceeds usize");
        self.nodes[index].children = children.into_boxed_slice();
        Ok(node)
    }

    fn reserve_node(&mut self, kind: FormattingNodeKind) -> LayoutNodeId {
        let id = LayoutNodeId::new(
            u64::try_from(self.nodes.len()).expect("DOM formatting tree exceeds u64::MAX nodes"),
        );
        self.nodes.push(FormattingNode {
            id,
            kind,
            children: Box::default(),
        });
        id
    }

    fn formatting_children(
        &mut self,
        parent: ElementId,
    ) -> Result<Vec<LayoutNodeId>, DomLayoutSourceError> {
        let mut stream = Vec::new();
        self.collect_formatting_children(parent, false, &mut stream)?;

        let mut result = Vec::new();
        let mut pending_text = Vec::new();
        for child in stream {
            match child {
                FormattingChild::Text(text) => pending_text.push(text),
                FormattingChild::Box(element) => {
                    self.flush_anonymous_text(parent, &mut pending_text, &mut result);
                    result.push(self.push_element(element)?);
                }
            }
        }
        self.flush_anonymous_text(parent, &mut pending_text, &mut result);
        Ok(result)
    }

    fn collect_formatting_children(
        &self,
        parent: ElementId,
        preserve_newlines: bool,
        output: &mut Vec<FormattingChild>,
    ) -> Result<(), DomLayoutSourceError> {
        let parent_element = self
            .arena
            .get(parent)
            .expect("formatting parent must remain live");
        for child in parent_element.children.iter().copied() {
            match self
                .arena
                .get_node(child)
                .expect("DOM child must remain live during borrowed epoch")
            {
                Node::Text(text) => {
                    if text.data().is_empty() {
                        continue;
                    }
                    let style = parent_element
                        .computed_style()
                        .ok_or(DomLayoutSourceError::MissingStyle(parent))?;
                    output.push(FormattingChild::Text(PendingText {
                        text: child,
                        style,
                        preserve_newlines,
                    }));
                }
                Node::Element(element) => {
                    let role = element.ext.layout_node_role();
                    let computed = element
                        .computed_style()
                        .ok_or(DomLayoutSourceError::MissingStyle(child))?;
                    let display = ComputedLayoutStyle::new(&computed).layout_display();
                    // `display:none` suppresses every Element subtree,
                    // including host-transparent roles. TextCarrier and
                    // Contents affect box generation only after standard CSS
                    // suppression has been honored.
                    if display == LayoutDisplay::None {
                        continue;
                    }

                    let flattened = display == LayoutDisplay::Contents
                        || matches!(role, LayoutNodeRole::Contents | LayoutNodeRole::TextCarrier);
                    if flattened {
                        let preserve = preserve_newlines
                            || (role == LayoutNodeRole::TextCarrier
                                && element.ext.preserve_newlines());
                        self.collect_formatting_children(child, preserve, output)?;
                    } else {
                        output.push(FormattingChild::Box(child));
                    }
                }
            }
        }
        Ok(())
    }

    fn flush_anonymous_text(
        &mut self,
        container: ElementId,
        pending: &mut Vec<PendingText>,
        output: &mut Vec<LayoutNodeId>,
    ) {
        if pending.is_empty() {
            return;
        }
        if is_fully_collapsible_whitespace(self.arena, pending) {
            pending.clear();
            return;
        }

        let contributors = pending
            .iter()
            .map(|run| run.text)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let runs = pending
            .drain(..)
            .map(|run| TextRunRecord {
                text: run.text,
                style: ComputedTextRunStyle::new(run.style),
                preserve_newlines: run.preserve_newlines,
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let contributor_ids = contributors.clone();
        let node = self.reserve_node(FormattingNodeKind::AnonymousText {
            container,
            contributors,
            runs,
        });
        for text in contributor_ids {
            self.by_dom.insert(text, node);
        }
        output.push(node);
    }

    fn collect_calc_handles(&mut self) {
        let computed = self
            .nodes
            .iter()
            .filter_map(FormattingNode::computed)
            .cloned()
            .collect::<Vec<_>>();
        for computed in computed {
            collect_style_calc_handles(ComputedLayoutStyle::new(&computed), &mut self.calc_handles);
        }
    }
}

fn is_fully_collapsible_whitespace<T>(arena: &Arena<T>, pending: &[PendingText]) -> bool {
    pending.iter().all(|run| {
        arena
            .text(run.text)
            .expect("Text contributor must remain live")
            .data()
            .chars()
            .all(|character| match character {
                '\u{0009}' | '\u{000D}' | ' ' => true,
                '\u{000A}' | '\u{000C}' => !run.preserve_newlines,
                _ => false,
            })
    })
}

impl<T: LayoutNodePolicy> TraverseTree for DomLayoutSource<'_, T> {
    type ChildIter<'a>
        = core::iter::Copied<slice::Iter<'a, LayoutNodeId>>
    where
        Self: 'a;

    #[inline]
    fn child_ids(&self, parent: LayoutNodeId) -> Self::ChildIter<'_> {
        self.node(parent).children.iter().copied()
    }

    #[inline]
    fn child_count(&self, parent: LayoutNodeId) -> usize {
        self.node(parent).children.len()
    }

    #[inline]
    fn child_id(&self, parent: LayoutNodeId, index: usize) -> LayoutNodeId {
        self.node(parent).children[index]
    }
}

impl<T: LayoutNodePolicy> LayoutSource for DomLayoutSource<'_, T> {
    type CoreStyle<'a>
        = DomLayoutStyle<'a>
    where
        Self: 'a;

    #[inline]
    fn core_style(&self, node: LayoutNodeId) -> Self::CoreStyle<'_> {
        self.style(node)
    }

    #[inline]
    fn resolve_calc(&self, calc: CalcHandle, basis: f32) -> f32 {
        resolve_calc(calc, basis, &self.calc_handles)
    }
}

impl<T: LayoutNodePolicy> FlexSource for DomLayoutSource<'_, T> {
    type ContainerStyle<'a>
        = DomLayoutStyle<'a>
    where
        Self: 'a;
    type ItemStyle<'a>
        = DomLayoutStyle<'a>
    where
        Self: 'a;

    #[inline]
    fn flex_container_style(&self, container: LayoutNodeId) -> Self::ContainerStyle<'_> {
        self.style(container)
    }

    #[inline]
    fn flex_item_style(&self, item: LayoutNodeId) -> Self::ItemStyle<'_> {
        self.style(item)
    }
}

impl<T: LayoutNodePolicy> GridSource for DomLayoutSource<'_, T> {
    type ContainerStyle<'a>
        = DomLayoutStyle<'a>
    where
        Self: 'a;
    type ItemStyle<'a>
        = DomLayoutStyle<'a>
    where
        Self: 'a;

    #[inline]
    fn grid_container_style(&self, container: LayoutNodeId) -> Self::ContainerStyle<'_> {
        self.style(container)
    }

    #[inline]
    fn grid_item_style(&self, item: LayoutNodeId) -> Self::ItemStyle<'_> {
        self.style(item)
    }
}

impl<T: LayoutNodePolicy> LinearSource for DomLayoutSource<'_, T> {
    type ContainerStyle<'a>
        = DomLayoutStyle<'a>
    where
        Self: 'a;
    type ItemStyle<'a>
        = DomLayoutStyle<'a>
    where
        Self: 'a;

    #[inline]
    fn linear_container_style(&self, container: LayoutNodeId) -> Self::ContainerStyle<'_> {
        self.style(container)
    }

    #[inline]
    fn linear_item_style(&self, item: LayoutNodeId) -> Self::ItemStyle<'_> {
        self.style(item)
    }
}

impl<T: LayoutNodePolicy> RelativeSource for DomLayoutSource<'_, T> {
    type ContainerStyle<'a>
        = DomLayoutStyle<'a>
    where
        Self: 'a;
    type ItemStyle<'a>
        = DomLayoutStyle<'a>
    where
        Self: 'a;

    #[inline]
    fn relative_container_style(&self, container: LayoutNodeId) -> Self::ContainerStyle<'_> {
        self.style(container)
    }

    #[inline]
    fn relative_item_style(&self, item: LayoutNodeId) -> Self::ItemStyle<'_> {
        self.style(item)
    }
}

fn collect_style_calc_handles(style: ComputedLayoutStyle<'_>, handles: &mut FxHashSet<u64>) {
    let inset = style.inset();
    for value in [inset.left, inset.right, inset.top, inset.bottom] {
        collect_length_percentage_auto(value, handles);
    }

    for size in [style.size(), style.min_size(), style.max_size()] {
        collect_dimension(size.width, handles);
        collect_dimension(size.height, handles);
    }

    let margin = style.margin();
    for value in [margin.left, margin.right, margin.top, margin.bottom] {
        collect_length_percentage_auto(value, handles);
    }
    let padding = style.padding();
    for value in [padding.left, padding.right, padding.top, padding.bottom] {
        collect_length_percentage(value, handles);
    }
    let border = style.border();
    for value in [border.left, border.right, border.top, border.bottom] {
        collect_length_percentage(value, handles);
    }

    let flex_gap = FlexContainerStyle::gap(&style);
    collect_length_percentage(flex_gap.width, handles);
    collect_length_percentage(flex_gap.height, handles);
    collect_dimension(FlexItemStyle::flex_basis(&style), handles);

    let grid_gap = GridContainerStyle::gap(&style);
    collect_length_percentage(grid_gap.width, handles);
    collect_length_percentage(grid_gap.height, handles);
    for component in GridContainerStyle::grid_template_rows(&style) {
        collect_grid_template_component(component, handles);
    }
    for component in GridContainerStyle::grid_template_columns(&style) {
        collect_grid_template_component(component, handles);
    }
    for track in GridContainerStyle::grid_auto_rows(&style) {
        collect_track(track, handles);
    }
    for track in GridContainerStyle::grid_auto_columns(&style) {
        collect_track(track, handles);
    }

    collect_length_percentage(TextContainerStyle::text_indent(&style), handles);
}

fn collect_grid_template_component<R: GridTemplateRepetition>(
    component: GridTemplateComponent<R>,
    handles: &mut FxHashSet<u64>,
) {
    match component {
        GridTemplateComponent::Single(track) => collect_track(track, handles),
        GridTemplateComponent::Repeat(repetition) => {
            for track in repetition.tracks() {
                collect_track(track, handles);
            }
        }
    }
}

fn collect_track(track: TrackSizingFunction, handles: &mut FxHashSet<u64>) {
    if let MinTrackSizingFunction::Fixed(value) = track.min {
        collect_length_percentage(value, handles);
    }
    match track.max {
        MaxTrackSizingFunction::Fixed(value) | MaxTrackSizingFunction::FitContent(value) => {
            collect_length_percentage(value, handles);
        }
        MaxTrackSizingFunction::MinContent
        | MaxTrackSizingFunction::MaxContent
        | MaxTrackSizingFunction::Auto
        | MaxTrackSizingFunction::Fr(_) => {}
    }
}

fn collect_dimension(value: Dimension, handles: &mut FxHashSet<u64>) {
    match value {
        Dimension::Calc(handle) => {
            handles.insert(handle.raw());
        }
        Dimension::FitContent(value) => collect_length_percentage(value, handles),
        Dimension::Length(_)
        | Dimension::Percent(_)
        | Dimension::Auto
        | Dimension::MinContent
        | Dimension::MaxContent => {}
    }
}

fn collect_length_percentage_auto(value: LengthPercentageAuto, handles: &mut FxHashSet<u64>) {
    if let LengthPercentageAuto::Calc(handle) = value {
        handles.insert(handle.raw());
    }
}

fn collect_length_percentage(value: LengthPercentage, handles: &mut FxHashSet<u64>) {
    if let LengthPercentage::Calc(handle) = value {
        handles.insert(handle.raw());
    }
}

#[cfg(test)]
mod tests {
    use euclid::{Scale, Size2D};
    use neutron_star::style::{FlexItemStyle, GridItemStyle, TextRunStyle, WhiteSpace, WordBreak};
    use neutron_star::tree::{FlexSource, GridSource, LinearSource, RelativeSource, TraverseTree};
    use stylo::context::QuirksMode;
    use stylo::device::Device;
    use stylo::device::servo::FontMetricsProvider;
    use stylo::font_metrics::FontMetrics;
    use stylo::media_queries::MediaType;
    use stylo::properties::ComputedValues;
    use stylo::properties::style_structs::Font;
    use stylo::queries::values::PrefersColorScheme;
    use stylo::servo::media_features::PointerCapabilities;
    use stylo::values::computed::font::GenericFontFamily;
    use stylo::values::computed::{CSSPixelLength, Length};
    use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};
    use stylo_traits::{CSSPixel, DevicePixel};

    use super::{DomLayoutDisplay, DomLayoutSource, LayoutNodePolicy, LayoutNodeRole};
    use crate::{Arena, Element, ElementId, ExternalState, StyleEngine, StylesheetOrigin};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct TestExt {
        role: LayoutNodeRole,
        preserve_newlines: bool,
    }

    impl TestExt {
        const NORMAL: Self = Self {
            role: LayoutNodeRole::Normal,
            preserve_newlines: false,
        };

        const CONTENTS: Self = Self {
            role: LayoutNodeRole::Contents,
            preserve_newlines: false,
        };

        const REPLACED: Self = Self {
            role: LayoutNodeRole::Replaced,
            preserve_newlines: false,
        };

        const CARRIER: Self = Self {
            role: LayoutNodeRole::TextCarrier,
            preserve_newlines: true,
        };
    }

    impl ExternalState for TestExt {}

    impl LayoutNodePolicy for TestExt {
        fn layout_node_role(&self) -> LayoutNodeRole {
            self.role
        }

        fn preserve_newlines(&self) -> bool {
            self.preserve_newlines
        }
    }

    #[derive(Debug)]
    struct TestFontMetrics;

    impl FontMetricsProvider for TestFontMetrics {
        fn query_font_metrics(
            &self,
            _vertical: bool,
            _font: &Font,
            base_size: CSSPixelLength,
            _flags: QueryFontMetricsFlags,
        ) -> FontMetrics {
            FontMetrics {
                ascent: Length::new(base_size.px()),
                ..FontMetrics::default()
            }
        }

        fn base_size_for_generic(&self, _generic: GenericFontFamily) -> Length {
            Length::new(FONT_MEDIUM_PX)
        }
    }

    fn device() -> Device {
        Device::new(
            MediaType::screen(),
            QuirksMode::NoQuirks,
            Size2D::<f32, CSSPixel>::new(800.0, 600.0),
            Size2D::<f32, DevicePixel>::new(800.0, 600.0),
            Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
            Box::new(TestFontMetrics),
            ComputedValues::initial_values_with_font_override(Font::initial_values()),
            PrefersColorScheme::Light,
            PointerCapabilities::empty(),
            PointerCapabilities::empty(),
        )
    }

    fn styled_arena(css: &str) -> (StyleEngine, Arena<TestExt>, ElementId) {
        let mut engine = StyleEngine::new(device());
        engine.add_stylesheet_str(css, StylesheetOrigin::Author);
        let mut arena = engine.new_arena();
        let root = arena.insert(Element::new("root", TestExt::NORMAL));
        (engine, arena, root)
    }

    fn append_element(
        arena: &mut Arena<TestExt>,
        parent: ElementId,
        tag: &str,
        ext: TestExt,
    ) -> ElementId {
        let child = arena.insert(Element::new(tag, ext));
        let index = arena.children_len(parent);
        arena.attach_at(parent, child, index);
        child
    }

    fn append_text(arena: &mut Arena<TestExt>, parent: ElementId, text: &str) -> crate::NodeId {
        let child = arena.insert_text(text);
        let index = arena.children_len(parent);
        arena.attach_at(parent, child, index);
        child
    }

    fn assert_all_sources<T>()
    where
        T: FlexSource + GridSource + LinearSource + RelativeSource,
    {
    }

    fn assert_all_nodes_reachable(source: &DomLayoutSource<'_, TestExt>) {
        let mut reachable = Vec::new();
        let mut stack = vec![source.root_node()];
        while let Some(node) = stack.pop() {
            reachable.push(node);
            stack.extend(source.child_ids(node));
        }
        reachable.sort_unstable_by_key(|node| node.get());
        assert_eq!(reachable, source.node_ids().collect::<Vec<_>>());
    }

    #[test]
    fn flex_and_grid_group_adjacent_text_into_anonymous_items() {
        let css = "root { display:flex } grid { display:grid }";
        let (engine, mut arena, root) = styled_arena(css);
        let first = append_text(&mut arena, root, "hello");
        let second = append_text(&mut arena, root, " world");
        let grid = append_element(&mut arena, root, "grid", TestExt::NORMAL);
        let grid_first = append_text(&mut arena, grid, "grid");
        let grid_second = append_text(&mut arena, grid, " text");
        let tail = append_text(&mut arena, root, "tail");
        engine.flush_tree(&mut arena, root);

        let source = DomLayoutSource::new(&arena, root).unwrap();
        let root_text = source.anonymous_text_child(root).unwrap();
        assert_eq!(source.display(root_text), DomLayoutDisplay::AnonymousText);
        assert_eq!(source.layout_node(first), Some(root_text));
        assert_eq!(source.layout_node(second), Some(root_text));
        assert_eq!(
            source.text_contributors(root_text).collect::<Vec<_>>(),
            [first, second]
        );
        assert_eq!(
            source
                .text_runs(root_text)
                .map(|run| run.text)
                .collect::<String>(),
            "hello world"
        );

        let grid_text = source.anonymous_text_child(grid).unwrap();
        assert_eq!(source.layout_node(grid_first), Some(grid_text));
        assert_eq!(source.layout_node(grid_second), Some(grid_text));
        assert_eq!(
            source.display(source.element_node(grid).unwrap()),
            DomLayoutDisplay::Grid
        );
        let root_items = source.anonymous_text_children(root).collect::<Vec<_>>();
        assert_eq!(root_items.len(), 2);
        assert_eq!(source.layout_node(tail), Some(root_items[1]));
        assert_all_nodes_reachable(&source);
    }

    #[test]
    fn collapsible_whitespace_only_runs_do_not_generate_items() {
        let (engine, mut arena, root) = styled_arena("root { display:flex }");
        let spaces = append_text(&mut arena, root, " \t\r\n ");
        let box_child = append_element(&mut arena, root, "box", TestExt::NORMAL);
        append_text(&mut arena, root, "\u{000c}");
        engine.flush_tree(&mut arena, root);

        let source = DomLayoutSource::new(&arena, root).unwrap();
        assert_eq!(source.child_count(source.root_node()), 1);
        assert_eq!(
            source.element_id(source.child_id(source.root_node(), 0)),
            Some(box_child)
        );
        assert_eq!(source.layout_node(spaces), None);
    }

    #[test]
    fn raw_text_whitespace_omission_matches_normalization() {
        let (engine, mut arena, root) = styled_arena("root { display:flex }");
        let carrier = append_element(&mut arena, root, "carrier", TestExt::CARRIER);
        let carriage_return = append_text(&mut arena, carrier, "\r");
        engine.flush_tree(&mut arena, root);

        let source = DomLayoutSource::new(&arena, root).unwrap();
        assert_eq!(source.child_count(source.root_node()), 0);
        assert_eq!(source.layout_node(carriage_return), None);

        let (engine, mut arena, root) = styled_arena("root { display:flex }");
        let carrier = append_element(&mut arena, root, "carrier", TestExt::CARRIER);
        let form_feed = append_text(&mut arena, carrier, "\u{000C}");
        engine.flush_tree(&mut arena, root);

        let source = DomLayoutSource::new(&arena, root).unwrap();
        let anonymous = source.anonymous_text_child(root).unwrap();
        assert_eq!(source.child_count(source.root_node()), 1);
        assert_eq!(source.layout_node(form_feed), Some(anonymous));
        let run = source.text_runs(anonymous).next().unwrap();
        assert_eq!(run.text, "\u{000C}");
        assert!(run.preserve_newlines);
    }

    #[test]
    fn display_none_elements_do_not_split_contiguous_text_runs() {
        let css = "root { display:flex } hidden { display:none }";
        let (engine, mut arena, root) = styled_arena(css);
        let before = append_text(&mut arena, root, "before");
        let hidden = append_element(&mut arena, root, "hidden", TestExt::NORMAL);
        append_text(&mut arena, hidden, "suppressed");
        let after = append_text(&mut arena, root, "after");
        engine.flush_tree(&mut arena, root);

        let source = DomLayoutSource::new(&arena, root).unwrap();
        let anonymous = source.anonymous_text_child(root).unwrap();
        assert_eq!(source.child_count(source.root_node()), 1);
        assert_eq!(source.layout_node(before), Some(anonymous));
        assert_eq!(source.layout_node(after), Some(anonymous));
        assert_eq!(
            source.text_contributors(anonymous).collect::<Vec<_>>(),
            [before, after]
        );
        assert_eq!(
            source
                .text_runs(anonymous)
                .map(|run| run.text)
                .collect::<String>(),
            "beforeafter"
        );

        assert!(source.element_node(hidden).is_none());
    }

    #[test]
    fn contents_flattening_preserves_run_style_boundaries() {
        let css = "root { display:flex; font-size:10px }\
                   contents { display:contents; font-size:20px; \
                              white-space:nowrap; word-break:break-all }\
                   carrier { font-size:30px }";
        let (engine, mut arena, root) = styled_arena(css);
        let direct = append_text(&mut arena, root, "a");
        let contents = append_element(&mut arena, root, "contents", TestExt::CONTENTS);
        let nested = append_text(&mut arena, contents, "b");
        let carrier = append_element(&mut arena, root, "carrier", TestExt::CARRIER);
        let carrier_text = append_text(&mut arena, carrier, "\nc");
        engine.flush_tree(&mut arena, root);

        let source = DomLayoutSource::new(&arena, root).unwrap();
        let anonymous = source.anonymous_text_child(root).unwrap();
        assert_eq!(source.child_count(source.root_node()), 1);
        assert!(source.element_node(contents).is_none());
        assert!(source.element_node(carrier).is_none());
        assert_eq!(source.layout_node(direct), Some(anonymous));
        assert_eq!(source.layout_node(nested), Some(anonymous));
        assert_eq!(source.layout_node(carrier_text), Some(anonymous));

        let runs = source.text_runs(anonymous).collect::<Vec<_>>();
        assert_eq!(
            runs.iter().map(|run| run.text).collect::<Vec<_>>(),
            ["a", "b", "\nc"]
        );
        assert_eq!(
            runs.iter()
                .map(|run| TextRunStyle::font_size(run.style))
                .collect::<Vec<_>>(),
            [10.0, 20.0, 30.0]
        );
        assert_eq!(
            TextRunStyle::white_space(runs[0].style),
            Some(WhiteSpace::Normal)
        );
        assert_eq!(
            TextRunStyle::word_break(runs[0].style),
            Some(WordBreak::Normal)
        );
        assert_eq!(
            TextRunStyle::white_space(runs[1].style),
            Some(WhiteSpace::NoWrap)
        );
        assert_eq!(
            TextRunStyle::word_break(runs[1].style),
            Some(WordBreak::BreakAll)
        );
        assert_eq!(
            TextRunStyle::white_space(runs[2].style),
            Some(WhiteSpace::Normal)
        );
        assert_eq!(
            TextRunStyle::word_break(runs[2].style),
            Some(WordBreak::Normal)
        );
        assert!(!runs[0].preserve_newlines);
        assert!(!runs[1].preserve_newlines);
        assert!(runs[2].preserve_newlines);
    }

    #[test]
    fn text_nodes_have_no_box_or_computed_style_of_their_own() {
        let (engine, mut arena, root) = styled_arena("root { display:flex; flex-grow:7 }");
        let text = append_text(&mut arena, root, "content");
        engine.flush_tree(&mut arena, root);

        let source = DomLayoutSource::new(&arena, root).unwrap();
        let anonymous = source.layout_node(text).unwrap();
        assert_eq!(source.element_id(anonymous), None);
        assert!(source.computed_style(anonymous).is_none());
        let style = source.flex_item_style(anonymous);
        assert!(
            style.flex_grow().abs() < f32::EPSILON,
            "parent item properties must not leak"
        );
        assert_eq!(GridItemStyle::order(&style), 0);
    }

    #[test]
    fn dispatches_all_algorithms_leaf_flow_and_anonymous_text() {
        let css = "root { display:flex } flex {display:flex} grid {display:grid}\
                   linear {display:linear} relative {display:relative}\
                   flow {display:block}";
        let (engine, mut arena, root) = styled_arena(css);
        let flex = append_element(&mut arena, root, "flex", TestExt::NORMAL);
        let grid = append_element(&mut arena, root, "grid", TestExt::NORMAL);
        let linear = append_element(&mut arena, root, "linear", TestExt::NORMAL);
        let relative = append_element(&mut arena, root, "relative", TestExt::NORMAL);
        let flow = append_element(&mut arena, root, "flow", TestExt::NORMAL);
        let leaf = append_element(&mut arena, root, "leaf", TestExt::REPLACED);
        append_text(&mut arena, root, "text");
        engine.flush_tree(&mut arena, root);

        let source = DomLayoutSource::new(&arena, root).unwrap();
        let display = |element| source.display(source.element_node(element).unwrap());
        assert_eq!(display(flex), DomLayoutDisplay::Flex);
        assert_eq!(display(grid), DomLayoutDisplay::Grid);
        assert_eq!(display(linear), DomLayoutDisplay::Linear);
        assert_eq!(display(relative), DomLayoutDisplay::Relative);
        assert_eq!(display(flow), DomLayoutDisplay::Flow);
        assert_eq!(display(leaf), DomLayoutDisplay::Leaf);
        assert_eq!(
            source.display(source.anonymous_text_child(root).unwrap()),
            DomLayoutDisplay::AnonymousText
        );
        assert_all_sources::<DomLayoutSource<'_, TestExt>>();
    }

    #[test]
    fn display_none_subtrees_are_absent_from_the_formatting_tree() {
        let css = "root { display:flex } hidden {display:none} child {display:grid}";
        let (engine, mut arena, root) = styled_arena(css);
        let hidden = append_element(&mut arena, root, "hidden", TestExt::NORMAL);
        let child = append_element(&mut arena, hidden, "child", TestExt::NORMAL);
        append_text(&mut arena, child, "not formatted");
        engine.flush_tree(&mut arena, root);

        let source = DomLayoutSource::new(&arena, root).unwrap();
        assert!(source.element_node(hidden).is_none());
        assert!(source.element_node(child).is_none());
        assert_eq!(source.node_ids().count(), 1);
        assert_all_nodes_reachable(&source);
    }

    #[test]
    fn display_none_suppresses_text_carrier_content() {
        let css = "root { display:flex } carrier { display:none }";
        let (engine, mut arena, root) = styled_arena(css);
        let before = append_text(&mut arena, root, "before");
        let carrier = append_element(&mut arena, root, "carrier", TestExt::CARRIER);
        let suppressed = append_text(&mut arena, carrier, "suppressed");
        let after = append_text(&mut arena, root, "after");
        engine.flush_tree(&mut arena, root);

        let source = DomLayoutSource::new(&arena, root).unwrap();
        let anonymous = source.anonymous_text_child(root).unwrap();
        assert_eq!(
            source.text_contributors(anonymous).collect::<Vec<_>>(),
            [before, after]
        );
        assert_eq!(source.layout_node(suppressed), None);
        assert!(source.element_node(carrier).is_none());
    }
}
