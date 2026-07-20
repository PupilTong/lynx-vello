//! Starlight relative-layout throughput benchmarks over a cache-backed host.
//!
//! Fixture and graph construction happen in divan's input generator, outside
//! the timed region. The measured closure covers layout, dependency solving,
//! recursive child dispatch, and host-owned cache traffic.

use std::cell::{Cell, RefCell};

use divan::counter::ItemsCount;
use neutron_star::cache::Cache;
use neutron_star::compute::{
    FnLeafMeasurer, LeafMetrics, compute_cached_layout, compute_leaf_layout,
    compute_relative_layout, hide_subtree,
};
use neutron_star::prelude::*;
use stylo::computed_values::{relative_center, relative_layout_once};
use stylo::values::computed::lynx_layout::{RelativeAlign, RelativeReference};
use stylo::values::computed::{
    Display, Length, LengthPercentage, PositionProperty, Size as StyleSize,
};
use stylo::values::generics::NonNegative;

/// `relative-id` / `relative-align` / `relative-*-of` share the fork's `i32`
/// sentinel encoding: `-1` = none, `0` = parent (align only), `>0` = sibling.
const NO_REFERENCE: RelativeReference = -1;
const PARENT_REFERENCE: RelativeAlign = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchDisplay {
    Relative,
    Leaf,
}

#[derive(Debug, Clone)]
struct BenchStyle {
    size: Size<StyleSize>,
    position: PositionProperty,
    relative_id: RelativeReference,
    align: Edges<RelativeAlign>,
    adjacent: Edges<RelativeReference>,
    center: relative_center::T,
    layout_once: relative_layout_once::T,
}

impl Default for BenchStyle {
    fn default() -> Self {
        Self {
            size: Size::new(StyleSize::Auto, StyleSize::Auto),
            position: PositionProperty::Relative,
            relative_id: NO_REFERENCE,
            align: Edges::uniform(NO_REFERENCE),
            adjacent: Edges::uniform(NO_REFERENCE),
            center: relative_center::T::None,
            layout_once: relative_layout_once::T::False,
        }
    }
}

impl CoreStyle for BenchStyle {
    #[inline]
    fn display(&self) -> Display {
        Display::LynxRelative
    }

    #[inline]
    fn size(&self) -> Size<&StyleSize> {
        self.size.as_ref()
    }

    #[inline]
    fn position(&self) -> PositionProperty {
        self.position
    }
}

impl RelativeContainerStyle for BenchStyle {
    #[inline]
    fn relative_layout_once(&self) -> relative_layout_once::T {
        self.layout_once
    }
}

impl RelativeItemStyle for BenchStyle {
    #[inline]
    fn relative_id(&self) -> RelativeReference {
        self.relative_id
    }

    #[inline]
    fn relative_align(&self) -> Edges<RelativeAlign> {
        self.align
    }

    #[inline]
    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        self.adjacent
    }

    #[inline]
    fn relative_center(&self) -> relative_center::T {
        self.center
    }
}

#[derive(Debug)]
struct SourceNode {
    display: BenchDisplay,
    style: BenchStyle,
    children: Vec<usize>,
    intrinsic: Size<f32>,
}

/// Per-node mutable layout slots, written through [`BenchRef`] handles.
/// Layout is single-threaded, so `Cell`/`RefCell` interior mutability is the
/// whole synchronization story.
#[derive(Debug, Default)]
struct SessionNode {
    cache: RefCell<Cache>,
    layout: Cell<Layout>,
    static_position: Cell<Point<f32>>,
}

/// The one host tree: source-shaped immutable node data plus a parallel
/// `Vec` of interior-mutable session slots, keeping memory layout comparable
/// with the pre-handle two-store host.
#[derive(Debug, Default)]
struct Tree {
    nodes: Vec<SourceNode>,
    session: Vec<SessionNode>,
}

impl Tree {
    fn push(&mut self, node: SourceNode) -> usize {
        let id = self.nodes.len();
        self.nodes.push(node);
        self.session.push(SessionNode::default());
        id
    }

    fn leaf(&mut self, style: BenchStyle, intrinsic: Size<f32>) -> usize {
        self.push(SourceNode {
            display: BenchDisplay::Leaf,
            style,
            children: Vec::new(),
            intrinsic,
        })
    }

    fn relative(&mut self, style: BenchStyle, children: Vec<usize>) -> usize {
        self.push(SourceNode {
            display: BenchDisplay::Relative,
            style,
            children,
            intrinsic: Size::ZERO,
        })
    }

    /// Resolves a builder-returned index to a borrowed node handle.
    #[inline]
    fn node(&self, index: usize) -> BenchRef<'_> {
        BenchRef { tree: self, index }
    }

    #[inline]
    fn layout(&self, index: usize) -> Layout {
        self.session[index].layout.get()
    }
}

/// The `Copy` node handle: a borrow of the tree plus a node index.
#[derive(Clone, Copy)]
struct BenchRef<'t> {
    tree: &'t Tree,
    index: usize,
}

impl std::fmt::Debug for BenchRef<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("BenchRef")
            .field(&self.index)
            .finish()
    }
}

impl<'t> BenchRef<'t> {
    #[inline]
    fn source(self) -> &'t SourceNode {
        &self.tree.nodes[self.index]
    }

    #[inline]
    fn slots(self) -> &'t SessionNode {
        &self.tree.session[self.index]
    }
}

struct BenchChildren<'t> {
    tree: &'t Tree,
    ids: std::slice::Iter<'t, usize>,
}

impl<'t> Iterator for BenchChildren<'t> {
    type Item = BenchRef<'t>;

    fn next(&mut self) -> Option<BenchRef<'t>> {
        let index = *self.ids.next()?;
        Some(BenchRef {
            tree: self.tree,
            index,
        })
    }
}

impl<'t> LayoutNode for BenchRef<'t> {
    type Style = &'t BenchStyle;
    type ChildIter = BenchChildren<'t>;

    #[inline]
    fn children(self) -> BenchChildren<'t> {
        BenchChildren {
            tree: self.tree,
            ids: self.source().children.iter(),
        }
    }

    #[inline]
    fn child_count(self) -> usize {
        self.source().children.len()
    }

    #[inline]
    fn style(self) -> &'t BenchStyle {
        &self.source().style
    }

    fn compute_child_layout(self, input: LayoutInput) -> LayoutOutput {
        let node = self.source();
        if node.style.display().is_none() {
            hide_subtree(self);
            return LayoutOutput::HIDDEN;
        }

        compute_cached_layout(self, input, |handle, input| match node.display {
            BenchDisplay::Relative => compute_relative_layout(handle, input),
            BenchDisplay::Leaf => {
                let intrinsic = node.intrinsic;
                let mut measurer = FnLeafMeasurer::new(move |measure_input| {
                    LeafMetrics::new(Size::new(
                        measure_input
                            .known_dimensions
                            .width
                            .unwrap_or(intrinsic.width),
                        measure_input
                            .known_dimensions
                            .height
                            .unwrap_or(intrinsic.height),
                    ))
                });
                compute_leaf_layout(input, &node.style, &mut measurer)
            }
        })
    }

    #[inline]
    fn set_unrounded_layout(self, layout: &Layout) {
        self.slots().layout.set(*layout);
    }

    #[inline]
    fn unrounded_layout(self) -> Layout {
        self.slots().layout.get()
    }

    fn set_final_layout(self, _layout: &Layout) {
        unreachable!("relative benchmarks do not run the rounding pass")
    }

    #[inline]
    fn set_static_position(self, static_position: Point<f32>) {
        self.slots().static_position.set(static_position);
    }

    #[inline]
    fn cache_get(self, input: LayoutInput) -> Option<LayoutOutput> {
        self.slots().cache.borrow().get(input)
    }

    #[inline]
    fn cache_store(self, input: LayoutInput, output: LayoutOutput) {
        self.slots().cache.borrow_mut().store(input, output);
    }

    #[inline]
    fn cache_clear(self) {
        self.slots().cache.borrow_mut().clear();
    }
}

#[derive(Debug)]
struct Fixture {
    tree: Tree,
    root: usize,
    probes: [usize; 3],
    viewport: Size<f32>,
    item_count: usize,
}

type GeometrySample = (LayoutOutput, Layout, Layout, Layout);

impl Fixture {
    #[inline]
    fn run(&mut self) -> GeometrySample {
        let known = self.viewport.map(Some);
        let available = self.viewport.map(AvailableSpace::Definite);
        let output = self
            .tree
            .node(self.root)
            .compute_child_layout(LayoutInput::perform_layout(known, known, available));
        (
            output,
            self.tree.layout(self.probes[0]),
            self.tree.layout(self.probes[1]),
            self.tree.layout(self.probes[2]),
        )
    }

    #[inline]
    fn run_auto_width(&mut self) -> GeometrySample {
        let known = Size::new(None, Some(self.viewport.height));
        let parent_size = self.viewport.map(Some);
        let available = self.viewport.map(AvailableSpace::Definite);
        let output = self
            .tree
            .node(self.root)
            .compute_child_layout(LayoutInput::perform_layout(known, parent_size, available));
        (
            output,
            self.tree.layout(self.probes[0]),
            self.tree.layout(self.probes[1]),
            self.tree.layout(self.probes[2]),
        )
    }

    #[inline]
    fn clear_root_cache(&mut self) {
        self.tree.node(self.root).cache_clear();
    }
}

#[derive(Debug, Clone, Copy)]
enum GraphKind {
    Independent,
    ReverseChain,
    DisjointCycles,
    DuplicateIds,
}

#[inline]
fn reference(value: usize) -> RelativeReference {
    i32::try_from(value).expect("benchmark ids fit i32")
}

#[inline]
fn once(flag: bool) -> relative_layout_once::T {
    if flag {
        relative_layout_once::T::True
    } else {
        relative_layout_once::T::False
    }
}

#[inline]
fn px_size(value: f32) -> StyleSize {
    StyleSize::LengthPercentage(NonNegative(LengthPercentage::new_length(Length::new(
        value,
    ))))
}

#[inline]
fn small_count(value: usize) -> f32 {
    f32::from(u16::try_from(value).expect("benchmark item counts fit u16"))
}

fn graph_fixture(item_count: usize, layout_once: bool, kind: GraphKind) -> Fixture {
    let mut tree = Tree::default();
    let mut children = Vec::with_capacity(item_count);
    for index in 0..item_count {
        let relative_id = match kind {
            GraphKind::DuplicateIds => reference(index % 32 + 1),
            GraphKind::Independent | GraphKind::ReverseChain | GraphKind::DisjointCycles => {
                reference(index + 1)
            }
        };
        let mut style = BenchStyle {
            size: Size::new(px_size(4.0), px_size(4.0)),
            relative_id,
            ..BenchStyle::default()
        };
        match kind {
            GraphKind::ReverseChain if index > 0 => {
                style.adjacent.right = reference(index);
            }
            GraphKind::DisjointCycles => {
                let partner = if index % 2 == 0 { index + 2 } else { index };
                if partner <= item_count {
                    style.adjacent.right = reference(partner);
                }
            }
            GraphKind::DuplicateIds => {
                style.align.left = PARENT_REFERENCE;
                if index >= 32 {
                    style.adjacent.bottom = reference(index % 32 + 1);
                }
            }
            GraphKind::Independent | GraphKind::ReverseChain => {}
        }
        children.push(tree.leaf(style, Size::new(4.0, 4.0)));
    }
    if matches!(kind, GraphKind::ReverseChain) {
        children.reverse();
    }
    let probes = [
        children[0],
        children[item_count / 2],
        children[item_count - 1],
    ];
    let root = tree.relative(
        BenchStyle {
            layout_once: once(layout_once),
            ..BenchStyle::default()
        },
        children,
    );
    Fixture {
        tree,
        root,
        probes,
        viewport: Size::new(small_count(item_count) * 4.0, 256.0),
        item_count,
    }
}

fn nested_fixture() -> Fixture {
    const CONTAINERS: usize = 32;
    const ITEMS: usize = 32;
    let mut tree = Tree::default();
    let mut containers = Vec::with_capacity(CONTAINERS);
    let mut all_leaves = Vec::with_capacity(CONTAINERS * ITEMS);
    for _ in 0..CONTAINERS {
        let mut children = Vec::with_capacity(ITEMS);
        for index in 0..ITEMS {
            let mut style = BenchStyle {
                size: Size::new(px_size(4.0), px_size(4.0)),
                relative_id: reference(index + 1),
                ..BenchStyle::default()
            };
            if index > 0 {
                style.adjacent.right = reference(index);
            }
            let leaf = tree.leaf(style, Size::new(4.0, 4.0));
            children.push(leaf);
            all_leaves.push(leaf);
        }
        containers.push(tree.relative(BenchStyle::default(), children));
    }
    let root = tree.relative(BenchStyle::default(), containers);
    let probes = [
        all_leaves[0],
        all_leaves[all_leaves.len() / 2],
        all_leaves[all_leaves.len() - 1],
    ];
    Fixture {
        tree,
        root,
        probes,
        viewport: Size::new(1024.0, 768.0),
        item_count: CONTAINERS * ITEMS,
    }
}

const SMALL_GRAPH_BATCH: usize = 64;
const LARGE_GRAPH_BATCH: usize = 4;
const NESTED_COLD_BATCH: usize = 8;
const WARM_DESCENDANTS_BATCH: usize = 1_024;
const ROOT_CACHE_HIT_BATCH: usize = 131_072;

const fn graph_batch_size(item_count: usize) -> usize {
    if item_count <= 256 {
        SMALL_GRAPH_BATCH
    } else {
        LARGE_GRAPH_BATCH
    }
}

fn bench_graph(
    bencher: divan::Bencher<'_, '_>,
    item_count: usize,
    layout_once: bool,
    kind: GraphKind,
) {
    let batch_size = graph_batch_size(item_count);
    bencher
        .with_inputs(|| {
            (0..batch_size)
                .map(|_| graph_fixture(item_count, layout_once, kind))
                .collect::<Vec<_>>()
        })
        .input_counter(|fixtures: &Vec<Fixture>| {
            ItemsCount::new(
                fixtures
                    .iter()
                    .map(|fixture| fixture.item_count)
                    .sum::<usize>(),
            )
        })
        .bench_local_values(|mut fixtures| {
            for fixture in &mut fixtures {
                divan::black_box(fixture.run());
            }
            fixtures
        });
}

#[divan::bench(args = [256, 4_096])]
fn independent_two_pass_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(bencher, item_count, false, GraphKind::Independent);
}

#[divan::bench(args = [256, 4_096])]
fn independent_two_pass_wrap_width_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    let batch_size = graph_batch_size(item_count);
    bencher
        .with_inputs(|| {
            (0..batch_size)
                .map(|_| graph_fixture(item_count, false, GraphKind::Independent))
                .collect::<Vec<_>>()
        })
        .input_counter(|fixtures: &Vec<Fixture>| {
            ItemsCount::new(
                fixtures
                    .iter()
                    .map(|fixture| fixture.item_count)
                    .sum::<usize>(),
            )
        })
        .bench_local_values(|mut fixtures| {
            for fixture in &mut fixtures {
                divan::black_box(fixture.run_auto_width());
            }
            fixtures
        });
}

#[divan::bench(args = [256, 4_096])]
fn reverse_chain_two_pass_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(bencher, item_count, false, GraphKind::ReverseChain);
}

#[divan::bench(args = [256, 4_096])]
fn reverse_chain_one_pass_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(bencher, item_count, true, GraphKind::ReverseChain);
}

#[divan::bench(args = [256, 4_096])]
fn disjoint_cycles_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(bencher, item_count, true, GraphKind::DisjointCycles);
}

#[divan::bench(args = [256, 4_096])]
fn duplicate_ids_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(bencher, item_count, false, GraphKind::DuplicateIds);
}

#[divan::bench]
fn nested_relative_cold(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| {
            (0..NESTED_COLD_BATCH)
                .map(|_| nested_fixture())
                .collect::<Vec<_>>()
        })
        .input_counter(|fixtures: &Vec<Fixture>| {
            ItemsCount::new(
                fixtures
                    .iter()
                    .map(|fixture| fixture.item_count)
                    .sum::<usize>(),
            )
        })
        .bench_local_values(|mut fixtures| {
            for fixture in &mut fixtures {
                divan::black_box(fixture.run());
            }
            fixtures
        });
}

#[divan::bench]
fn nested_relative_warm_descendants(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| {
            let mut fixture = nested_fixture();
            let _ = fixture.run();
            fixture.clear_root_cache();
            fixture
        })
        .input_counter(|fixture| ItemsCount::new(fixture.item_count * WARM_DESCENDANTS_BATCH))
        .bench_local_refs(|fixture| {
            for _ in 0..WARM_DESCENDANTS_BATCH {
                divan::black_box(fixture.run());
                // Re-establish the intended warm-descendants/root-dirty state
                // for the next logical operation in this sample.
                fixture.clear_root_cache();
            }
        });
}

#[divan::bench]
fn nested_relative_root_cache_hit(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| {
            let mut fixture = nested_fixture();
            let _ = fixture.run();
            fixture
        })
        .input_counter(|fixture| ItemsCount::new(fixture.item_count * ROOT_CACHE_HIT_BATCH))
        .bench_local_refs(|fixture| {
            for _ in 0..ROOT_CACHE_HIT_BATCH {
                // Cache hits do not mutate the fixture. Obscure the input on
                // every lookup so the optimizer cannot hoist the query out of
                // this batching loop.
                divan::black_box(divan::black_box(&mut *fixture).run());
            }
        });
}
