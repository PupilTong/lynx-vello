//! Mutable box-layout and text-artifact state for one DOM layout consumer.
//!
//! The immutable [`DomLayoutSource`] borrows DOM topology, character data,
//! and computed styles from an [`Arena`](crate::Arena). This module owns the
//! disjoint mutable half of neutron-star's source/session protocol. A session
//! retains caches and Parley artifacts while the source revision is stable
//! and conservatively clears them whenever the DOM layout epoch changes.

use core::marker::PhantomData;
use core::num::NonZeroU64;

use neutron_star::cache::Cache;
use neutron_star::compute::{
    FnLeafMeasurer, LeafMetrics, compute_cached_layout, compute_flexbox_layout,
    compute_grid_layout, compute_leaf_layout, compute_linear_layout, compute_relative_layout,
    compute_root_layout, hide_subtree, round_layout,
};
use neutron_star::geometry::{Point, Size};
use neutron_star::style::{BoxGenerationMode, CoreStyle};
use neutron_star::text::{ArtifactSlots, TextContext, TextLayout, TextMeasurer};
use neutron_star::tree::{
    AvailableSpace, CacheState, Layout, LayoutInput, LayoutOutput, LayoutSession, LayoutSource,
    LayoutState, NodeId as LayoutNodeId, RoundState,
};
use rustc_hash::FxHashMap;

use super::{DomLayoutDisplay, DomLayoutSource, LayoutNodePolicy};
use crate::{ElementId, NodeId};

#[derive(Debug, Default)]
struct NodeLayoutState {
    cache: Cache,
    unrounded: Layout,
    has_unrounded_layout: bool,
    final_layout: Layout,
    has_final_layout: bool,
    static_position: Option<Point<f32>>,
    text: ArtifactSlots,
}

/// Mutable layout, cache, and retained-text state for a DOM embedding.
///
/// The type is parameterized by the same embedder-neutral [`LayoutNodePolicy`]
/// payload as [`DomLayoutSource`], but it stores no DOM nodes or computed
/// styles. Rebuild a source after style or tree mutation and pass it to
/// [`Self::commit`]; the captured DOM revision drives conservative cache
/// invalidation.
///
/// # Flow fallback
///
/// Flex and Grid dispatch to their CSS algorithms. `display: linear` and
/// `display: relative` dispatch to their Lynx-specific algorithms. Ordinary
/// CSS flow currently dispatches to neutron-star's single-axis Linear
/// algorithm only as a temporary host fallback. It does **not** claim W3C CSS
/// Block or Inline Layout conformance.
///
/// CSS absolute-position containing-block discovery is also pending. The
/// current computed-style view can only use neutron-star's parent-contained
/// absolute variant, so this session must not be treated as complete CSS
/// Positioned Layout conformance.
#[derive(Debug)]
pub struct DomLayoutSession<T: LayoutNodePolicy> {
    text: TextContext,
    nodes: FxHashMap<LayoutNodeId, NodeLayoutState>,
    prepared_epoch: Option<(NonZeroU64, u64, ElementId)>,
    policy: PhantomData<fn() -> T>,
}

impl<T: LayoutNodePolicy> DomLayoutSession<T> {
    /// Create a session backed by the platform system-font collection.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a session with an initially empty font collection.
    ///
    /// This is useful for deterministic embedders that register every font
    /// explicitly through [`Self::register_fonts`].
    #[must_use]
    pub fn without_system_fonts() -> Self {
        Self {
            text: TextContext::without_system_fonts(),
            nodes: FxHashMap::default(),
            prepared_epoch: None,
            policy: PhantomData,
        }
    }

    /// Commit and device-pixel-round one immutable DOM formatting epoch.
    ///
    /// The returned layout is the final rounded layout of the source root.
    /// Individual Element and Text results can subsequently be queried with
    /// [`Self::final_layout`] and [`Self::committed_text_layout`].
    ///
    /// # Panics
    ///
    /// Panics if a [`DomLayoutSource`] constructed by this crate violates its
    /// invariant that [`DomLayoutSource::root_node`] is included in
    /// [`DomLayoutSource::node_ids`].
    #[must_use]
    pub fn commit(
        &mut self,
        source: &DomLayoutSource<'_, T>,
        available_space: Size<AvailableSpace>,
        device_pixel_ratio: f32,
    ) -> Layout {
        self.prepare(source);
        compute_root_layout(source, self, source.root_node(), available_space);
        round_layout(source, self, source.root_node(), device_pixel_ratio);
        self.nodes
            .get(&source.root_node())
            .expect("the prepared source always contains its root")
            .final_layout
    }

    /// Register all readable font faces in `bytes` with the retained Parley
    /// context.
    ///
    /// Successfully registering any face invalidates box-measurement caches,
    /// retained text artifacts, and output queries until the next commit,
    /// because font selection and metrics may change. The return value is the
    /// number of registered faces.
    pub fn register_fonts(&mut self, bytes: &[u8]) -> usize {
        let registered = self.text.register_fonts(bytes);
        if registered != 0 {
            self.invalidate_all();
            self.prepared_epoch = None;
        }
        registered
    }

    /// Return the final rounded layout for a real DOM Element or Text node.
    ///
    /// Text nodes participating in the same anonymous text item resolve to
    /// that item's shared layout. Flattened Elements and nodes outside the
    /// most recently committed source epoch return `None`.
    #[must_use]
    pub fn final_layout(&self, source: &DomLayoutSource<'_, T>, node: NodeId) -> Option<Layout> {
        let layout_node = source.layout_node(node)?;
        self.formatting_layout(source, layout_node)
    }

    /// Return the final rounded layout for an actual or generated formatting
    /// node, including an anonymous text item returned by
    /// [`DomLayoutSource::anonymous_text_children`].
    #[must_use]
    pub fn formatting_layout(
        &self,
        source: &DomLayoutSource<'_, T>,
        node: LayoutNodeId,
    ) -> Option<Layout> {
        self.is_prepared_for(source).then_some(())?;
        let state = self.nodes.get(&node)?;
        (source.core_style(node).box_generation_mode() != BoxGenerationMode::None).then_some(())?;
        state.has_final_layout.then_some(state.final_layout)
    }

    /// Return the retained committed paragraph layout for a real DOM Text
    /// node.
    ///
    /// Every Text contributor to one anonymous item resolves to the same
    /// artifact. Elements, non-text layout boxes, omitted whitespace, and
    /// nodes outside the most recently committed source epoch return `None`.
    #[must_use]
    pub fn committed_text_layout(
        &self,
        source: &DomLayoutSource<'_, T>,
        node: NodeId,
    ) -> Option<&TextLayout> {
        let layout_node = source.layout_node(node)?;
        self.formatting_text_layout(source, layout_node)
    }

    /// Return the retained paragraph artifact for a generated anonymous text
    /// formatting node. Actual box nodes and stale epochs return `None`.
    #[must_use]
    pub fn formatting_text_layout(
        &self,
        source: &DomLayoutSource<'_, T>,
        node: LayoutNodeId,
    ) -> Option<&TextLayout> {
        self.is_prepared_for(source).then_some(())?;
        self.nodes.get(&node)?.text.committed()
    }

    fn epoch(source: &DomLayoutSource<'_, T>) -> (NonZeroU64, u64, ElementId) {
        (
            source.arena_identity(),
            source.revision(),
            source.root_element(),
        )
    }

    fn is_prepared_for(&self, source: &DomLayoutSource<'_, T>) -> bool {
        self.prepared_epoch == Some(Self::epoch(source))
    }

    fn prepare(&mut self, source: &DomLayoutSource<'_, T>) {
        let epoch = Self::epoch(source);
        if self.prepared_epoch != Some(epoch) {
            self.nodes.clear();
        }
        for node in source.node_ids() {
            self.nodes.entry(node).or_default();
        }
        self.prepared_epoch = Some(epoch);
    }

    fn invalidate_all(&mut self) {
        for state in self.nodes.values_mut() {
            state.cache.clear();
            state.text.invalidate();
        }
    }

    fn state_mut(&mut self, node: LayoutNodeId) -> &mut NodeLayoutState {
        self.nodes
            .get_mut(&node)
            .expect("layout source nodes are prepared before recursion")
    }

    fn compute_text(
        &mut self,
        source: &DomLayoutSource<'_, T>,
        node: LayoutNodeId,
        input: LayoutInput,
    ) -> LayoutOutput {
        let style = source.text_container_style(node);
        let runs = source.text_runs(node);
        let Self { text, nodes, .. } = self;
        let artifacts = &mut nodes
            .get_mut(&node)
            .expect("text layout nodes are prepared before recursion")
            .text;
        let mut measurer = TextMeasurer::new(text, artifacts, &style, runs, |handle, basis| {
            source.resolve_calc(handle, basis)
        });
        compute_leaf_layout(
            input,
            &style,
            |handle, basis| source.resolve_calc(handle, basis),
            &mut measurer,
        )
    }

    fn compute_empty_leaf(
        source: &DomLayoutSource<'_, T>,
        node: LayoutNodeId,
        input: LayoutInput,
    ) -> LayoutOutput {
        let style = source.core_style(node);
        let mut measurer = FnLeafMeasurer::new(|_| LeafMetrics::default());
        compute_leaf_layout(
            input,
            &style,
            |handle, basis| source.resolve_calc(handle, basis),
            &mut measurer,
        )
    }
}

impl<T: LayoutNodePolicy> Default for DomLayoutSession<T> {
    fn default() -> Self {
        Self {
            text: TextContext::default(),
            nodes: FxHashMap::default(),
            prepared_epoch: None,
            policy: PhantomData,
        }
    }
}

impl<'arena, T: LayoutNodePolicy> LayoutSession<DomLayoutSource<'arena, T>>
    for DomLayoutSession<T>
{
    fn compute_child_layout(
        &mut self,
        source: &DomLayoutSource<'arena, T>,
        child: LayoutNodeId,
        input: LayoutInput,
    ) -> LayoutOutput {
        if source.core_style(child).box_generation_mode() == BoxGenerationMode::None {
            hide_subtree(source, self, child);
            return LayoutOutput::HIDDEN;
        }
        let display = source.display(child);

        compute_cached_layout(self, child, input, |session, child, input| match display {
            DomLayoutDisplay::AnonymousText => session.compute_text(source, child, input),
            DomLayoutDisplay::Flex => compute_flexbox_layout(source, session, child, input),
            DomLayoutDisplay::Grid => compute_grid_layout(source, session, child, input),
            DomLayoutDisplay::Relative => compute_relative_layout(source, session, child, input),
            // Linear is a supported Lynx algorithm. Flow reaches the same
            // call only as a temporary fallback: neutron-star does not yet
            // implement CSS Block/Inline Layout.
            DomLayoutDisplay::Linear | DomLayoutDisplay::Flow => {
                compute_linear_layout(source, session, child, input)
            }
            DomLayoutDisplay::Leaf => Self::compute_empty_leaf(source, child, input),
        })
    }
}

impl<T: LayoutNodePolicy> LayoutState for DomLayoutSession<T> {
    fn set_unrounded_layout(&mut self, node: LayoutNodeId, layout: &Layout) {
        let state = self.state_mut(node);
        state.unrounded = *layout;
        state.has_unrounded_layout = true;
    }

    fn set_static_position(&mut self, child: LayoutNodeId, static_position: Point<f32>) {
        self.state_mut(child).static_position = Some(static_position);
    }
}

impl<T: LayoutNodePolicy> CacheState for DomLayoutSession<T> {
    fn cache_get(&self, node: LayoutNodeId, input: LayoutInput) -> Option<LayoutOutput> {
        self.nodes.get(&node)?.cache.get(input)
    }

    fn cache_store(&mut self, node: LayoutNodeId, input: LayoutInput, output: LayoutOutput) {
        self.state_mut(node).cache.store(input, output);
    }

    fn cache_clear(&mut self, node: LayoutNodeId) {
        let state = self.state_mut(node);
        state.cache.clear();
        state.text.invalidate();
    }
}

impl<T: LayoutNodePolicy> RoundState for DomLayoutSession<T> {
    fn unrounded_layout(&self, node: LayoutNodeId) -> Layout {
        self.nodes
            .get(&node)
            .expect("rounding only visits prepared layout nodes")
            .unrounded
    }

    fn set_final_layout(&mut self, node: LayoutNodeId, layout: &Layout) {
        let state = self.state_mut(node);
        state.final_layout = *layout;
        state.has_final_layout = state.has_unrounded_layout;
    }
}
