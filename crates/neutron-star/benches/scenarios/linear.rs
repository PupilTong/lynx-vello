//! `display: linear` workloads driven through w3c-dom's production host.

#![allow(clippy::cast_precision_loss)]

use neutron_star::geometry::Size;

use crate::support::{LayoutFixture, LeafContent};

#[derive(Clone, Copy)]
pub(super) struct Scenario {
    pub(super) name: &'static str,
    pub(super) build: fn(usize) -> BenchCase,
}

impl std::fmt::Debug for Scenario {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Scenario")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

pub(super) type BenchCase = LayoutFixture;

macro_rules! scenario {
    ($function:ident, $build:ident) => {
        Scenario {
            name: stringify!($function),
            build: $build,
        }
    };
}

macro_rules! for_each_linear_scenario {
    ($callback:ident) => {
        $callback! {
            fixed_stack, build_fixed_stack;
            ordered_stack, build_ordered_stack;
            weighted_distribution, build_weighted_distribution;
            weighted_freeze, build_weighted_freeze;
            weighted_freeze_with_text, build_weighted_freeze_with_text;
            measured_stretch, build_measured_stretch;
            mixed_hidden_absolute, build_mixed_hidden_absolute;
            mixed_hidden_absolute_with_text, build_mixed_hidden_absolute_with_text;
            intrinsic_pure_length, build_intrinsic_pure_length;
            intrinsic_sparse_percentage, build_intrinsic_sparse_percentage;
            intrinsic_dense_percentage, build_intrinsic_dense_percentage;
            intrinsic_dense_padding_percentage, build_intrinsic_dense_padding_percentage;
            intrinsic_dense_padding_percentage_with_text, build_intrinsic_dense_padding_percentage_with_text;
            intrinsic_percentage_size_only, build_intrinsic_percentage_size_only;
            intrinsic_percentage_min_max_only, build_intrinsic_percentage_min_max_only;
            intrinsic_percentage_min_max_only_with_text, build_intrinsic_percentage_min_max_only_with_text;
            intrinsic_relative_inset_only, build_intrinsic_relative_inset_only;
            linear_gravity_matrix, build_linear_gravity_matrix;
            linear_layout_gravity_matrix, build_linear_layout_gravity_matrix;
            linear_cross_gravity_matrix, build_linear_cross_gravity_matrix;
            linear_cross_gravity_matrix_with_text, build_linear_cross_gravity_matrix_with_text;
        }
    };
}
#[allow(
    unused_imports,
    reason = "the benchmark registry expands the declaration list twice"
)]
pub(super) use for_each_linear_scenario;

macro_rules! declare_scenarios {
    ($( $function:ident, $build:ident; )*) => {
        pub(super) const SCENARIOS: &[Scenario] = &[
            $(scenario!($function, $build),)*
        ];
    };
}

for_each_linear_scenario!(declare_scenarios);

pub(super) fn scenario_named(name: &str) -> Scenario {
    *SCENARIOS
        .iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("unknown Linear benchmark scenario {name}"))
}

fn linear_fixture(width: f32, height: f32, extra: &str) -> LayoutFixture {
    let style = format!("display:linear; width:{width}px; height:{height}px; {extra}");
    LayoutFixture::new(Size::new(width.max(1.0), height.max(1.0)), &style)
}

fn build_fixed_stack(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut fixture = linear_fixture(320.0, count as f32 * 2.0, "");
    let root = fixture.root();
    for index in 0..count {
        let width = 10.0 + (index % 5) as f32;
        fixture.leaf(
            root,
            &format!("width:{width}px; height:2px"),
            Size::new(width, 2.0),
            None,
        );
    }
    fixture.prepare()
}

fn build_ordered_stack(nodes: usize) -> BenchCase {
    const ORDERS: [i32; 11] = [-5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5];

    let count = nodes.max(1);
    let mut fixture = linear_fixture(320.0, count as f32 * 2.0, "");
    let root = fixture.root();
    for index in 0..count {
        fixture.leaf(
            root,
            &format!(
                "width:12px; height:2px; order:{}",
                ORDERS[index % ORDERS.len()]
            ),
            Size::new(12.0, 2.0),
            None,
        );
    }
    fixture.prepare()
}

fn build_weighted_distribution(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut fixture = linear_fixture(240.0, count as f32 * 4.0, "linear-weight-sum:0");
    let root = fixture.root();
    for index in 0..count {
        fixture.leaf(
            root,
            &format!("width:20px; height:0; linear-weight:{}", 1 + index % 4),
            Size::new(20.0, 2.0),
            None,
        );
    }
    fixture.prepare()
}

fn build_weighted_freeze_with_content(nodes: usize, content: LeafContent) -> BenchCase {
    let count = nodes.max(1);
    let mut fixture = linear_fixture(240.0, count as f32 * 3.0, "linear-weight-sum:0");
    let root = fixture.root();
    for index in 0..count {
        let clamp = match index % 3 {
            0 => "min-height:4px",
            1 => "max-height:2px",
            _ => "min-height:1px; max-height:5px",
        };
        fixture.leaf_with_content(
            root,
            &format!(
                "width:20px; height:0; linear-weight:{}; {clamp}",
                1 + index % 5
            ),
            Size::new(20.0, 2.0),
            None,
            content,
            index,
        );
    }
    fixture.prepare()
}

fn build_weighted_freeze(nodes: usize) -> BenchCase {
    build_weighted_freeze_with_content(nodes, LeafContent::Synthetic)
}

fn build_weighted_freeze_with_text(nodes: usize) -> BenchCase {
    build_weighted_freeze_with_content(nodes, LeafContent::Text)
}

fn build_measured_stretch(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut fixture = linear_fixture(
        count as f32 * 10.0,
        40.0,
        "linear-direction:row; align-items:stretch",
    );
    let root = fixture.root();
    for index in 0..count {
        let width = 6.0 + (index % 7) as f32;
        let height = 8.0 + (index % 5) as f32;
        fixture.leaf(
            root,
            "width:auto; height:auto",
            Size::new(width, height),
            Some(height - 1.0),
        );
    }
    fixture.prepare()
}

fn build_mixed_hidden_absolute_with_content(nodes: usize, content: LeafContent) -> BenchCase {
    let count = nodes.max(1);
    let mut fixture = linear_fixture(640.0, 480.0, "position:relative");
    let root = fixture.root();
    for index in 0..count {
        let style = match index % 5 {
            0 => "display:none; width:8px; height:4px".to_owned(),
            1 => format!(
                "position:absolute; left:{}px; top:{}px; width:8px; height:4px",
                index % 80,
                index % 60
            ),
            _ => format!(
                "width:8px; height:4px; order:{}; margin-top:{}px",
                index % 7,
                index % 3
            ),
        };
        fixture.leaf_with_content(root, &style, Size::new(8.0, 4.0), None, content, index);
    }
    fixture.prepare()
}

fn build_mixed_hidden_absolute(nodes: usize) -> BenchCase {
    build_mixed_hidden_absolute_with_content(nodes, LeafContent::Synthetic)
}

fn build_mixed_hidden_absolute_with_text(nodes: usize) -> BenchCase {
    build_mixed_hidden_absolute_with_content(nodes, LeafContent::Text)
}

#[derive(Debug, Clone, Copy)]
enum IntrinsicKind {
    PureLength,
    SparsePercentage,
    DensePercentage,
    DensePaddingPercentage,
    PercentageSizeOnly,
    PercentageMinMaxOnly,
    RelativeInsetOnly,
}

fn build_intrinsic(nodes: usize, kind: IntrinsicKind, content: LeafContent) -> BenchCase {
    let count = nodes.max(1);
    let style = "display:linear; linear-direction:row; width:auto; max-width:4096px; height:16px; align-items:flex-start";
    let mut fixture = LayoutFixture::new(Size::new(4096.0, 16.0), style);
    let root = fixture.root();
    for index in 0..count {
        let uses_percentage = match kind {
            IntrinsicKind::PureLength => false,
            IntrinsicKind::SparsePercentage => index % 16 == 0,
            _ => true,
        };
        let style = match kind {
            IntrinsicKind::DensePaddingPercentage => {
                "width:12px; height:8px; padding-left:5%; padding-right:3%".to_owned()
            }
            IntrinsicKind::PercentageSizeOnly => "width:8%; height:8px".to_owned(),
            IntrinsicKind::PercentageMinMaxOnly => {
                "width:auto; min-width:2%; max-width:8%; height:8px".to_owned()
            }
            IntrinsicKind::RelativeInsetOnly => {
                "position:relative; left:5%; width:12px; height:8px".to_owned()
            }
            _ if uses_percentage => "width:6%; height:8px".to_owned(),
            _ => format!("width:{}px; height:8px", 8 + index % 7),
        };
        fixture.leaf_with_content(root, &style, Size::new(12.0, 8.0), None, content, index);
    }
    fixture.prepare()
}

fn build_intrinsic_pure_length(nodes: usize) -> BenchCase {
    build_intrinsic(nodes, IntrinsicKind::PureLength, LeafContent::Synthetic)
}

fn build_intrinsic_sparse_percentage(nodes: usize) -> BenchCase {
    build_intrinsic(
        nodes,
        IntrinsicKind::SparsePercentage,
        LeafContent::Synthetic,
    )
}

fn build_intrinsic_dense_percentage(nodes: usize) -> BenchCase {
    build_intrinsic(
        nodes,
        IntrinsicKind::DensePercentage,
        LeafContent::Synthetic,
    )
}

fn build_intrinsic_dense_padding_percentage(nodes: usize) -> BenchCase {
    build_intrinsic(
        nodes,
        IntrinsicKind::DensePaddingPercentage,
        LeafContent::Synthetic,
    )
}

fn build_intrinsic_dense_padding_percentage_with_text(nodes: usize) -> BenchCase {
    build_intrinsic(
        nodes,
        IntrinsicKind::DensePaddingPercentage,
        LeafContent::Text,
    )
}

fn build_intrinsic_percentage_size_only(nodes: usize) -> BenchCase {
    build_intrinsic(
        nodes,
        IntrinsicKind::PercentageSizeOnly,
        LeafContent::Synthetic,
    )
}

fn build_intrinsic_percentage_min_max_only(nodes: usize) -> BenchCase {
    build_intrinsic(
        nodes,
        IntrinsicKind::PercentageMinMaxOnly,
        LeafContent::Synthetic,
    )
}

fn build_intrinsic_percentage_min_max_only_with_text(nodes: usize) -> BenchCase {
    build_intrinsic(
        nodes,
        IntrinsicKind::PercentageMinMaxOnly,
        LeafContent::Text,
    )
}

fn build_intrinsic_relative_inset_only(nodes: usize) -> BenchCase {
    build_intrinsic(
        nodes,
        IntrinsicKind::RelativeInsetOnly,
        LeafContent::Synthetic,
    )
}

#[derive(Debug, Clone, Copy)]
enum GravityKind {
    Main,
    Item,
    Cross,
}

fn build_gravity_matrix(nodes: usize, kind: GravityKind, content: LeafContent) -> BenchCase {
    let groups = nodes.max(4).div_ceil(4);
    let mut fixture = linear_fixture(
        360.0,
        groups as f32 * 74.0,
        "linear-direction:column; align-items:flex-start",
    );
    let root = fixture.root();
    let directions = ["row", "row-reverse", "column", "column-reverse"];
    let main = ["flex-start", "center", "flex-end", "space-between"];
    let cross = ["flex-start", "center", "flex-end", "stretch"];
    for index in 0..groups {
        let alignment = match kind {
            GravityKind::Main => format!("justify-content:{}", main[index % main.len()]),
            GravityKind::Cross => format!("align-items:{}", cross[index % cross.len()]),
            GravityKind::Item => "align-items:flex-start".to_owned(),
        };
        let extra = format!(
            "display:linear; linear-direction:{}; width:340px; height:72px; {alignment}",
            directions[index % directions.len()]
        );
        let container = fixture.container(root, &extra);
        for child in 0..3 {
            let align_self = if matches!(kind, GravityKind::Item) {
                cross[(index + child) % cross.len()]
            } else {
                "auto"
            };
            fixture.leaf_with_content(
                container,
                &format!(
                    "width:{}px; height:{}px; align-self:{align_self}; linear-weight:{}",
                    20 + child * 4,
                    10 + child * 3,
                    child
                ),
                Size::new((20 + child * 4) as f32, (10 + child * 3) as f32),
                None,
                content,
                index * 3 + child,
            );
        }
    }
    fixture.prepare()
}

fn build_linear_gravity_matrix(nodes: usize) -> BenchCase {
    build_gravity_matrix(nodes, GravityKind::Main, LeafContent::Synthetic)
}

fn build_linear_layout_gravity_matrix(nodes: usize) -> BenchCase {
    build_gravity_matrix(nodes, GravityKind::Item, LeafContent::Synthetic)
}

fn build_linear_cross_gravity_matrix(nodes: usize) -> BenchCase {
    build_gravity_matrix(nodes, GravityKind::Cross, LeafContent::Synthetic)
}

fn build_linear_cross_gravity_matrix_with_text(nodes: usize) -> BenchCase {
    build_gravity_matrix(nodes, GravityKind::Cross, LeafContent::Text)
}
