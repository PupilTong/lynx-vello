//! Containment-bounded incremental relayout benchmarks over the shared
//! cache-embedding [`TestTree`] host.

#[path = "../tests/support/mod.rs"]
mod support;

use hughie::compute::{compute_boundary_relayout, compute_root_layout};
use hughie::invalidate::invalidate_for_relayout;
use hughie::prelude::*;
use stylo::computed_values::flex_direction;
use stylo::values::computed::Contain;
use support::{TestId, TestStyle, TestTree, basis_px, contain_intrinsic_px, nn, size_px};

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

/// A deep flex tree with an optional strict-containment boundary.
struct Fixture {
    tree: TestTree,
    root: TestId,
    dirty_leaf: TestId,
    boundary: Option<TestId>,
    ancestors: Vec<TestId>,
    viewport: Size<f32>,
    wide: bool,
}

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
    ancestors.push(root);
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

    fn warm(self) -> Self {
        let available = self.available();
        self.tree.compute_root_layout(self.root, available);
        self
    }

    fn dirty_the_leaf(&mut self) {
        self.wide = !self.wide;
        let width = if self.wide { 24.0 } else { 16.0 };
        self.tree.source_node_mut(self.dirty_leaf).style = dirty_leaf_style(width);
    }

    fn run_boundary_stopped(&mut self) -> LayoutOutput {
        self.dirty_the_leaf();
        let committed = self.boundary.map(|b| {
            self.tree
                .committed_input(b)
                .expect("warmed boundary committed")
        });
        let re_root = self.tree.with_layout_state(false, |tree, state| {
            invalidate_for_relayout(
                tree,
                state,
                self.tree.node(self.dirty_leaf),
                self.ancestors.iter().map(|&id| self.tree.node(id)),
            )
        });
        if let Some(input) = committed {
            self.tree.with_layout_state(true, |tree, state| {
                compute_boundary_relayout(tree, state, re_root, input)
            })
        } else {
            let available = self.available();
            self.tree.with_layout_state(true, |tree, state| {
                compute_root_layout(tree, state, re_root, available);
            });
            self.tree.layout(self.root).into_output()
        }
    }

    fn run_whole_path(&mut self) -> LayoutOutput {
        self.dirty_the_leaf();
        self.tree.clear_layout_cache(self.dirty_leaf);
        for &ancestor in &self.ancestors {
            self.tree.clear_layout_cache(ancestor);
        }
        let available = self.available();
        self.tree.compute_root_layout(self.root, available);
        self.tree.layout(self.root).into_output()
    }

    fn run_cold(&mut self) -> LayoutOutput {
        self.dirty_the_leaf();
        for id in 0..self.tree.nodes.len() {
            self.tree.clear_layout_cache(id);
        }
        let available = self.available();
        self.tree.compute_root_layout(self.root, available);
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

#[divan::bench]
fn uncontained_boundary_stopped_control(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| fixture(false).warm())
        .bench_local_refs(Fixture::run_boundary_stopped);
}

/// A virtualized list between two frames: one row on screen has a dirty leaf,
/// and every row below the window skips its contents.
///
/// The list itself is the re-layout root — a row is no boundary, its height
/// follows its content — so the pass asks all `LIST_ROWS` rows for their box.
/// A skipped row answers from its cache like any other row, which is what
/// `skipped_rows` and `plain_rows_control` are here to keep true: the two
/// should cost the same, where a skipped row that re-resolved its own box
/// model per pass made the skipping list the more expensive of the two.
const LIST_ROWS: usize = 512;
const LIST_VISIBLE_ROWS: usize = 8;

fn list_available() -> Size<AvailableSpace> {
    Size::new(
        AvailableSpace::Definite(320.0),
        AvailableSpace::Definite(192.0),
    )
}

struct RowList {
    tree: TestTree,
    list: TestId,
    dirty_row: TestId,
    dirty_leaf: TestId,
    wide: bool,
}

fn row_list(skipping: bool) -> RowList {
    let mut tree = TestTree::default();
    let mut rows = Vec::with_capacity(LIST_ROWS);
    let mut dirty = None;
    for row in 0..LIST_ROWS {
        let leaf = tree.push_leaf(dirty_leaf_style(16.0), Size::new(16.0, 12.0), None);
        let mut style = TestStyle {
            size: Size::new(size_px(320.0), size_px(24.0)),
            flex_shrink: nn(0.0),
            ..TestStyle::default()
        };
        if skipping && row >= LIST_VISIBLE_ROWS {
            style.skips_contents = true;
            style.containment = Contain::STRICT;
            style.contain_intrinsic_width = contain_intrinsic_px(320.0);
            style.contain_intrinsic_height = contain_intrinsic_px(24.0);
        }
        let row_id = tree.push_flex(style, vec![leaf]);
        if row == LIST_VISIBLE_ROWS / 2 {
            dirty = Some((row_id, leaf));
        }
        rows.push(row_id);
    }
    let list = tree.push_flex(
        TestStyle {
            size: Size::new(size_px(320.0), size_px(192.0)),
            ..column_flex()
        },
        rows,
    );
    tree.enable_cache();
    tree.compute_root_layout(list, list_available());
    let (dirty_row, dirty_leaf) = dirty.expect("a visible row holds the dirty leaf");
    RowList {
        tree,
        list,
        dirty_row,
        dirty_leaf,
        wide: false,
    }
}

impl RowList {
    fn relayout(&mut self) -> Layout {
        self.wide = !self.wide;
        let width = if self.wide { 24.0 } else { 16.0 };
        self.tree.source_node_mut(self.dirty_leaf).style = dirty_leaf_style(width);
        // What the host's boundary-stopped walk would clear: the leaf, its
        // row, and the list the row's height reaches. Every other row keeps
        // the cache it was committed with.
        for id in [self.dirty_leaf, self.dirty_row, self.list] {
            self.tree.clear_layout_cache(id);
        }
        self.tree.compute_root_layout(self.list, list_available());
        self.tree.layout(self.list)
    }
}

#[divan::bench]
fn skipped_rows(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| row_list(true))
        .bench_local_refs(RowList::relayout);
}

#[divan::bench]
fn plain_rows_control(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| row_list(false))
        .bench_local_refs(RowList::relayout);
}
