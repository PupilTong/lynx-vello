//! Containment-bounded incremental relayout benchmarks over the shared
//! cache-embedding [`TestTree`] host.
//!
//! The scenario is the perf payoff of `css-contain-2`: a deep, bushy flex tree
//! with a dirty leaf buried inside one subtree. When that subtree is
//! `contain: strict` it is a **relayout boundary**, so
//! [`invalidate_for_relayout`](neutron_star::invalidate::invalidate_for_relayout)
//! stops cache clearing at the boundary and layout re-runs only from there —
//! via [`compute_boundary_relayout`](neutron_star::compute::compute_boundary_relayout)
//! with the boundary's preserved committed input — versus clearing the whole
//! ancestor path and re-running from the document root, versus a fully cold
//! layout. A no-containment control makes the win falsifiable: without the
//! boundary, invalidation walks to the root and there is no saving.
//!
//! Fixture construction and cache warming happen in divan's input generator,
//! outside the timed region.

#[path = "../tests/support/mod.rs"]
mod support;

use neutron_star::compute::{compute_boundary_relayout, compute_root_layout};
use neutron_star::invalidate::invalidate_for_relayout;
use neutron_star::prelude::*;
use stylo::computed_values::flex_direction;
use stylo::values::computed::Contain;
use support::{TestId, TestStyle, TestTree, basis_px, contain_intrinsic_px, size_px};

fn main() {
    divan::main();
}

const SIBLINGS: usize = 8;
const DEPTH: usize = 6;

fn column_flex() -> TestStyle {
    TestStyle {
        flex_direction: flex_direction::T::Column,
        ..TestStyle::default()
    }
}

fn dirty_leaf_style(width: f32) -> TestStyle {
    TestStyle {
        size: Size::new(size_px(width), size_px(12.0)),
        flex_basis: basis_px(width),
        ..TestStyle::default()
    }
}

/// A deep, bushy flex tree whose dirty leaf lives inside one subtree that is
/// (optionally) a `contain: strict` relayout boundary.
struct Fixture {
    tree: TestTree,
    root: TestId,
    dirty_leaf: TestId,
    /// The relayout boundary (the dirty branch's `contain: strict` chain root),
    /// when the fixture is contained.
    boundary: Option<TestId>,
    /// Ancestor path of `dirty_leaf`, nearest first, up to `root` — the host
    /// supplies this to `invalidate_for_relayout` (the engine keeps no parent
    /// links).
    ancestors: Vec<TestId>,
    viewport: Size<f32>,
    wide: bool,
}

/// Builds one nested flex chain of `DEPTH` containers ending in a leaf,
/// recording the leaf's ancestor chain (nearest first).
fn build_chain(
    tree: &mut TestTree,
    boundary_style: Option<&TestStyle>,
) -> (TestId, TestId, Vec<TestId>) {
    let leaf = tree.push_leaf(dirty_leaf_style(16.0), Size::new(16.0, 12.0), None);
    let mut ancestors = Vec::with_capacity(DEPTH);
    let mut current = leaf;
    for level in 0..DEPTH {
        let style = if level == DEPTH - 1 {
            boundary_style.cloned().unwrap_or_else(column_flex)
        } else {
            column_flex()
        };
        current = tree.push_flex(style, vec![current]);
        ancestors.push(current);
    }
    // `current` is the chain root; its style is the (optional) boundary.
    (current, leaf, ancestors)
}

fn fixture(contained: bool) -> Fixture {
    let mut tree = TestTree::default();
    let mut branches = Vec::with_capacity(SIBLINGS);
    let mut dirty_leaf = None;
    let mut boundary = None;
    let mut ancestors = Vec::new();

    for branch in 0..SIBLINGS {
        let is_dirty_branch = branch == SIBLINGS / 2;
        let boundary_style = if is_dirty_branch && contained {
            Some(TestStyle {
                flex_direction: flex_direction::T::Column,
                containment: Contain::STRICT,
                contain_intrinsic_width: contain_intrinsic_px(16.0),
                contain_intrinsic_height: contain_intrinsic_px(72.0),
                ..TestStyle::default()
            })
        } else if is_dirty_branch {
            Some(column_flex())
        } else {
            None
        };
        let (chain_root, leaf, chain_ancestors) = build_chain(&mut tree, boundary_style.as_ref());
        if is_dirty_branch {
            dirty_leaf = Some(leaf);
            ancestors = chain_ancestors;
            if contained {
                boundary = Some(chain_root);
            }
        }
        branches.push(chain_root);
    }

    let root = tree.push_flex(TestStyle::default(), branches);
    // Append the document root to the dirty leaf's ancestor path.
    ancestors.push(root);
    // Enable the reference caches the invalidation workflow operates over.
    tree.enable_cache();

    Fixture {
        tree,
        root,
        dirty_leaf: dirty_leaf.expect("dirty branch has a leaf"),
        boundary,
        ancestors,
        viewport: Size::new(1_200.0, 800.0),
        wide: false,
    }
}

impl Fixture {
    fn available(&self) -> Size<AvailableSpace> {
        Size::new(
            AvailableSpace::Definite(self.viewport.width),
            AvailableSpace::Definite(self.viewport.height),
        )
    }

    /// Warms every cache with a full cold layout from the root.
    fn warm(self) -> Self {
        let available = self.available();
        compute_root_layout(self.tree.node(self.root), available);
        self
    }

    /// Perturbs the dirty leaf so a relayout is genuinely required.
    fn dirty_the_leaf(&mut self) {
        self.wide = !self.wide;
        let width = if self.wide { 24.0 } else { 16.0 };
        self.tree.source_node_mut(self.dirty_leaf).style = dirty_leaf_style(width);
    }

    /// Boundary-stopped incremental relayout: invalidate from the leaf,
    /// stopping at the nearest relayout boundary, and re-run from there with
    /// the boundary's preserved committed input.
    fn run_boundary_stopped(&mut self) -> LayoutOutput {
        self.dirty_the_leaf();
        // Capture the boundary's committed input before invalidation clears it.
        let committed = self.boundary.map(|b| {
            self.tree
                .committed_input(b)
                .expect("warmed boundary committed")
        });
        let re_root = invalidate_for_relayout(
            self.tree.node(self.dirty_leaf),
            self.ancestors.iter().map(|&id| self.tree.node(id)),
        );
        if let Some(input) = committed {
            compute_boundary_relayout(re_root, input)
        } else {
            // No boundary: re_root is the document root; re-run from there.
            let available = self.available();
            compute_root_layout(re_root, available);
            self.tree.layout(self.root).into_output()
        }
    }

    /// Whole-path invalidation: clear the leaf and every ancestor up to the
    /// root, then re-run from the root.
    fn run_whole_path(&mut self) -> LayoutOutput {
        self.dirty_the_leaf();
        self.tree.node(self.dirty_leaf).cache_clear();
        for &ancestor in &self.ancestors {
            self.tree.node(ancestor).cache_clear();
        }
        let available = self.available();
        compute_root_layout(self.tree.node(self.root), available);
        self.tree.layout(self.root).into_output()
    }

    /// Cold layout: clear every cache and lay out the whole tree from scratch.
    fn run_cold(&mut self) -> LayoutOutput {
        self.dirty_the_leaf();
        for id in 0..self.tree.nodes.len() {
            self.tree.node(id).cache_clear();
        }
        let available = self.available();
        compute_root_layout(self.tree.node(self.root), available);
        self.tree.layout(self.root).into_output()
    }
}

trait LayoutOutputExt {
    fn into_output(self) -> LayoutOutput;
}

impl LayoutOutputExt for Layout {
    #[inline]
    fn into_output(self) -> LayoutOutput {
        LayoutOutput::new(self.size, self.content_size)
    }
}

#[divan::bench]
fn contained_boundary_stopped(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| fixture(true).warm())
        .bench_local_refs(Fixture::run_boundary_stopped);
}

#[divan::bench]
fn contained_whole_path(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| fixture(true).warm())
        .bench_local_refs(Fixture::run_whole_path);
}

#[divan::bench]
fn contained_cold(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| fixture(true).warm())
        .bench_local_refs(Fixture::run_cold);
}

/// Falsifiability control: the same dirty-leaf incremental relayout without the
/// containment boundary. `invalidate_for_relayout` walks all the way to the
/// root, so there is no boundary-stopped saving to be had.
#[divan::bench]
fn uncontained_boundary_stopped_control(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| fixture(false).warm())
        .bench_local_refs(Fixture::run_boundary_stopped);
}
