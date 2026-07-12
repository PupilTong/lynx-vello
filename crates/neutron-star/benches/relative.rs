//! Starlight relative-layout throughput benchmarks over a cache-backed host.
//!
//! Fixture and graph construction happen in divan's input generator, outside
//! the timed region. The measured closure covers layout, dependency solving,
//! recursive child dispatch, and host-owned cache traffic.

use divan::counter::ItemsCount;
use neutron_star::cache::Cache;
use neutron_star::compute::{
    FnLeafMeasurer, LeafMetrics, compute_cached_layout, compute_leaf_layout,
    compute_relative_layout, hide_subtree,
};
use neutron_star::prelude::*;
use neutron_star::style::{
    BoxGenerationMode, CalcHandle, Dimension, Position, RelativeCenter, RelativeContainerStyle,
    RelativeItemStyle, RelativeReference,
};

fn main() {
    divan::main();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Display {
    Relative,
    Leaf,
}

#[derive(Debug, Clone, Copy)]
struct BenchStyle {
    size: Size<Dimension>,
    position: Position,
    relative_id: RelativeReference,
    align: Edges<RelativeReference>,
    adjacent: Edges<RelativeReference>,
    center: RelativeCenter,
    layout_once: bool,
}

impl Default for BenchStyle {
    fn default() -> Self {
        Self {
            size: Size::new(Dimension::Auto, Dimension::Auto),
            position: Position::Relative,
            relative_id: RelativeReference::NONE,
            align: Edges::uniform(RelativeReference::NONE),
            adjacent: Edges::uniform(RelativeReference::NONE),
            center: RelativeCenter::None,
            layout_once: false,
        }
    }
}

impl CoreStyle for BenchStyle {
    #[inline]
    fn size(&self) -> Size<Dimension> {
        self.size
    }

    #[inline]
    fn position(&self) -> Position {
        self.position
    }
}

impl RelativeContainerStyle for BenchStyle {
    #[inline]
    fn relative_layout_once(&self) -> bool {
        self.layout_once
    }
}

impl RelativeItemStyle for BenchStyle {
    #[inline]
    fn relative_id(&self) -> RelativeReference {
        self.relative_id
    }

    #[inline]
    fn relative_align(&self) -> Edges<RelativeReference> {
        self.align
    }

    #[inline]
    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        self.adjacent
    }

    #[inline]
    fn relative_center(&self) -> RelativeCenter {
        self.center
    }
}

#[derive(Debug)]
struct SourceNode {
    display: Display,
    style: BenchStyle,
    children: Vec<NodeId>,
    intrinsic: Size<f32>,
}

#[derive(Debug, Default)]
struct Source {
    nodes: Vec<SourceNode>,
}

impl Source {
    #[inline]
    fn node(&self, node: NodeId) -> &SourceNode {
        &self.nodes[usize::from(node)]
    }
}

#[derive(Debug, Default)]
struct SessionNode {
    cache: Cache,
    layout: Layout,
    static_position: Point<f32>,
}

#[derive(Debug, Default)]
struct Session {
    nodes: Vec<SessionNode>,
}

#[derive(Debug, Default)]
struct Tree {
    source: Source,
    session: Session,
}

impl Tree {
    fn push(&mut self, node: SourceNode) -> NodeId {
        let id = NodeId::from(self.source.nodes.len());
        self.source.nodes.push(node);
        self.session.nodes.push(SessionNode::default());
        id
    }

    fn leaf(&mut self, style: BenchStyle, intrinsic: Size<f32>) -> NodeId {
        self.push(SourceNode {
            display: Display::Leaf,
            style,
            children: Vec::new(),
            intrinsic,
        })
    }

    fn relative(&mut self, style: BenchStyle, children: Vec<NodeId>) -> NodeId {
        self.push(SourceNode {
            display: Display::Relative,
            style,
            children,
            intrinsic: Size::ZERO,
        })
    }
}

impl TraverseTree for Source {
    type ChildIter<'a> = std::iter::Copied<std::slice::Iter<'a, NodeId>>;

    #[inline]
    fn child_ids(&self, parent: NodeId) -> Self::ChildIter<'_> {
        self.node(parent).children.iter().copied()
    }

    #[inline]
    fn child_count(&self, parent: NodeId) -> usize {
        self.node(parent).children.len()
    }

    #[inline]
    fn child_id(&self, parent: NodeId, index: usize) -> NodeId {
        self.node(parent).children[index]
    }
}

impl LayoutSource for Source {
    type CoreStyle<'a> = &'a BenchStyle;

    #[inline]
    fn core_style(&self, node: NodeId) -> Self::CoreStyle<'_> {
        &self.node(node).style
    }

    fn resolve_calc(&self, _calc: CalcHandle, _basis: f32) -> f32 {
        unreachable!("relative benchmarks contain no calc() values")
    }
}

impl RelativeSource for Source {
    type ContainerStyle<'a> = &'a BenchStyle;
    type ItemStyle<'a> = &'a BenchStyle;

    #[inline]
    fn relative_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.node(container).style
    }

    #[inline]
    fn relative_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.node(item).style
    }
}

impl LayoutState for Session {
    #[inline]
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        self.nodes[usize::from(node)].layout = *layout;
    }

    #[inline]
    fn set_static_position(&mut self, child: NodeId, static_position: Point<f32>) {
        self.nodes[usize::from(child)].static_position = static_position;
    }
}

impl CacheState for Session {
    #[inline]
    fn cache_get(&self, node: NodeId, input: LayoutInput) -> Option<LayoutOutput> {
        self.nodes[usize::from(node)].cache.get(input)
    }

    #[inline]
    fn cache_store(&mut self, node: NodeId, input: LayoutInput, output: LayoutOutput) {
        self.nodes[usize::from(node)].cache.store(input, output);
    }

    #[inline]
    fn cache_clear(&mut self, node: NodeId) {
        self.nodes[usize::from(node)].cache.clear();
    }
}

impl LayoutSession<Source> for Session {
    #[inline]
    fn compute_child_layout(
        &mut self,
        source: &Source,
        child: NodeId,
        input: LayoutInput,
    ) -> LayoutOutput {
        let node = source.node(child);
        if node.style.box_generation_mode() == BoxGenerationMode::None {
            hide_subtree(source, self, child);
            return LayoutOutput::HIDDEN;
        }

        compute_cached_layout(self, child, input, |session, child, input| {
            match node.display {
                Display::Relative => compute_relative_layout(source, session, child, input),
                Display::Leaf => {
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
                    compute_leaf_layout(
                        input,
                        &node.style,
                        |_calc, _basis| {
                            unreachable!("relative benchmarks contain no calc() values")
                        },
                        &mut measurer,
                    )
                }
            }
        })
    }
}

#[derive(Debug)]
struct Fixture {
    tree: Tree,
    root: NodeId,
    probes: [NodeId; 3],
    viewport: Size<f32>,
    item_count: usize,
}

type GeometrySample = (LayoutOutput, Layout, Layout, Layout);

impl Fixture {
    #[inline]
    fn run(&mut self) -> GeometrySample {
        let known = self.viewport.map(Some);
        let available = self.viewport.map(AvailableSpace::Definite);
        let output = self.tree.session.compute_child_layout(
            &self.tree.source,
            self.root,
            LayoutInput::perform_layout(known, known, available),
        );
        (
            output,
            self.tree.session.nodes[usize::from(self.probes[0])].layout,
            self.tree.session.nodes[usize::from(self.probes[1])].layout,
            self.tree.session.nodes[usize::from(self.probes[2])].layout,
        )
    }

    #[inline]
    fn run_auto_width(&mut self) -> GeometrySample {
        let known = Size::new(None, Some(self.viewport.height));
        let parent_size = self.viewport.map(Some);
        let available = self.viewport.map(AvailableSpace::Definite);
        let output = self.tree.session.compute_child_layout(
            &self.tree.source,
            self.root,
            LayoutInput::perform_layout(known, parent_size, available),
        );
        (
            output,
            self.tree.session.nodes[usize::from(self.probes[0])].layout,
            self.tree.session.nodes[usize::from(self.probes[1])].layout,
            self.tree.session.nodes[usize::from(self.probes[2])].layout,
        )
    }

    #[inline]
    fn clear_root_cache(&mut self) {
        self.tree.session.cache_clear(self.root);
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
    RelativeReference::new(i32::try_from(value).expect("benchmark ids fit i32"))
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
            size: Size::new(Dimension::Length(4.0), Dimension::Length(4.0)),
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
                style.align.left = RelativeReference::PARENT;
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
            layout_once,
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
                size: Size::new(Dimension::Length(4.0), Dimension::Length(4.0)),
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

fn bench_graph(
    bencher: divan::Bencher<'_, '_>,
    item_count: usize,
    layout_once: bool,
    kind: GraphKind,
) {
    bencher
        .with_inputs(|| graph_fixture(item_count, layout_once, kind))
        .input_counter(|fixture| ItemsCount::new(fixture.item_count))
        .bench_local_refs(|fixture| divan::black_box(fixture.run()));
}

#[divan::bench(args = [256, 4_096])]
fn independent_two_pass_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(bencher, item_count, false, GraphKind::Independent);
}

#[divan::bench(args = [256, 4_096])]
fn independent_two_pass_wrap_width_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bencher
        .with_inputs(|| graph_fixture(item_count, false, GraphKind::Independent))
        .input_counter(|fixture| ItemsCount::new(fixture.item_count))
        .bench_local_refs(|fixture| divan::black_box(fixture.run_auto_width()));
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
        .with_inputs(nested_fixture)
        .input_counter(|fixture| ItemsCount::new(fixture.item_count))
        .bench_local_refs(Fixture::run);
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
        .input_counter(|fixture| ItemsCount::new(fixture.item_count))
        .bench_local_refs(Fixture::run);
}

#[divan::bench]
fn nested_relative_root_cache_hit(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| {
            let mut fixture = nested_fixture();
            let _ = fixture.run();
            fixture
        })
        .input_counter(|fixture| ItemsCount::new(fixture.item_count))
        .bench_local_refs(Fixture::run);
}
