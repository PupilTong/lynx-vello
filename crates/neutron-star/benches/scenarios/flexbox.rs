//! Flex workloads driven through w3c-dom's production layout host.

#![allow(clippy::cast_precision_loss)]

use neutron_star::geometry::Size;

use crate::support::LayoutFixture;

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
    ($name:literal, $build:ident) => {
        Scenario {
            name: $name,
            build: $build,
        }
    };
}

pub(super) const SCENARIOS: &[Scenario] = &[
    scenario!("flex_grow_row", build_flex_grow_row),
    scenario!("flex_wrap_gaps", build_flex_wrap_gaps),
    scenario!("flex_at_most_root", build_flex_at_most_root),
    scenario!("at_most_owner_matrix", build_at_most_owner_matrix),
    scenario!(
        "owner_direction_inheritance",
        build_owner_direction_inheritance
    ),
    scenario!(
        "flex_axis_alignment_matrix",
        build_flex_axis_alignment_matrix
    ),
    scenario!("flex_distribution_matrix", build_flex_distribution_matrix),
    scenario!(
        "flex_wrap_alignment_matrix",
        build_flex_wrap_alignment_matrix
    ),
    scenario!("flex_baseline_measured", build_flex_baseline_measured),
    scenario!(
        "baseline_propagation_matrix",
        build_baseline_propagation_matrix
    ),
    scenario!("measured_callback_matrix", build_measured_callback_matrix),
    scenario!("absolute_children", build_absolute_children),
    scenario!("nested_column_flex", build_nested_column_flex),
    scenario!("in_flow_order_matrix", build_in_flow_order_matrix),
    scenario!("full_value_spacing_matrix", build_full_value_spacing_matrix),
    scenario!("box_sizing_matrix", build_box_sizing_matrix),
    scenario!("fit_content_subtrees", build_fit_content_subtrees),
    scenario!("mixed_display_none", build_mixed_display_none),
];

pub(super) fn scenario_named(name: &str) -> Scenario {
    *SCENARIOS
        .iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("unknown Flex benchmark scenario {name}"))
}

fn flex_fixture(width: f32, height: f32, extra: &str) -> LayoutFixture {
    let style = format!("display:flex; width:{width}px; height:{height}px; {extra}");
    LayoutFixture::new(Size::new(width.max(1.0), height.max(1.0)), &style)
}

fn build_flex_grow_row(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut fixture = flex_fixture(count as f32, 10.0, "align-items:stretch");
    let root = fixture.root();
    for index in 0..count {
        let style = format!(
            "width:1px; height:10px; flex-basis:1px; flex-grow:{}",
            1 + index % 3
        );
        fixture.leaf(root, &style, Size::new(1.0, 10.0), None);
    }
    fixture.prepare()
}

fn build_flex_wrap_gaps(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let rows = count.div_ceil(16) as f32;
    let mut fixture = flex_fixture(
        320.0,
        rows * 12.0,
        "flex-wrap:wrap; gap:1px; align-content:flex-start; align-items:flex-start",
    );
    let root = fixture.root();
    for index in 0..count {
        let width = 16.0 + (index % 5) as f32;
        let height = 6.0 + (index % 3) as f32;
        let style = format!("width:{width}px; height:{height}px; flex:none");
        fixture.leaf(root, &style, Size::new(width, height), None);
    }
    fixture.prepare()
}

fn build_flex_at_most_root(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let limit = count as f32 * 4.0;
    let style = format!(
        "display:flex; width:auto; max-width:{limit}px; height:10px; align-items:flex-start"
    );
    let mut fixture = LayoutFixture::new(Size::new(limit * 1.25, 10.0), &style);
    let root = fixture.root();
    for index in 0..count {
        let basis = 1.0 + (index % 4) as f32;
        let style = format!("height:4px; flex:0 1 {basis}px");
        fixture.leaf(root, &style, Size::new(basis, 4.0), None);
    }
    fixture.prepare()
}

#[derive(Debug, Clone, Copy)]
enum MatrixKind {
    AtMost,
    Direction,
    Axis,
    Distribution,
    Wrap,
}

fn build_matrix(nodes: usize, kind: MatrixKind) -> BenchCase {
    let groups = nodes.max(4).div_ceil(4);
    let height = groups as f32 * 66.0;
    let mut fixture = flex_fixture(
        360.0,
        height,
        "flex-direction:column; align-items:flex-start",
    );
    let root = fixture.root();
    let directions = ["row", "row-reverse", "column", "column-reverse"];
    let justify = [
        "flex-start",
        "center",
        "flex-end",
        "space-between",
        "space-around",
        "space-evenly",
    ];
    let align = ["stretch", "flex-start", "center", "flex-end", "baseline"];
    let wraps = ["nowrap", "wrap", "wrap-reverse"];

    for index in 0..groups {
        let extra = match kind {
            MatrixKind::AtMost => format!(
                "width:fit-content(calc(55% + {}px)); max-width:330px; flex-wrap:wrap; gap:1px",
                index % 7
            ),
            MatrixKind::Direction => format!(
                "width:330px; direction:{}; flex-direction:{}",
                if index.is_multiple_of(2) {
                    "ltr"
                } else {
                    "rtl"
                },
                directions[index % directions.len()]
            ),
            MatrixKind::Axis => format!(
                "width:330px; flex-direction:{}; align-items:{}",
                directions[index % directions.len()],
                align[index % align.len()]
            ),
            MatrixKind::Distribution => format!(
                "width:330px; justify-content:{}; align-items:{}",
                justify[index % justify.len()],
                align[index % align.len()]
            ),
            MatrixKind::Wrap => format!(
                "width:110px; flex-wrap:{}; align-content:{}; gap:2px",
                wraps[index % wraps.len()],
                justify[index % justify.len()]
            ),
        };
        let style = format!("display:flex; height:64px; {extra}");
        let container = fixture.container(root, &style);
        for child_index in 0..3 {
            let width = 18.0 + child_index as f32 * 6.0;
            let height = 8.0 + ((index + child_index) % 4) as f32 * 2.0;
            let style = format!(
                "width:{width}px; height:{height}px; flex:{} 1 {width}px; align-self:{}",
                1 + child_index,
                align[(index + child_index) % align.len()]
            );
            fixture.leaf(
                container,
                &style,
                Size::new(width, height),
                Some(height - 2.0),
            );
        }
    }
    fixture.prepare()
}

fn build_at_most_owner_matrix(nodes: usize) -> BenchCase {
    build_matrix(nodes, MatrixKind::AtMost)
}

fn build_owner_direction_inheritance(nodes: usize) -> BenchCase {
    build_matrix(nodes, MatrixKind::Direction)
}

fn build_flex_axis_alignment_matrix(nodes: usize) -> BenchCase {
    build_matrix(nodes, MatrixKind::Axis)
}

fn build_flex_distribution_matrix(nodes: usize) -> BenchCase {
    build_matrix(nodes, MatrixKind::Distribution)
}

fn build_flex_wrap_alignment_matrix(nodes: usize) -> BenchCase {
    build_matrix(nodes, MatrixKind::Wrap)
}

fn build_flex_baseline_measured(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut fixture = flex_fixture(
        count as f32 * 12.0,
        32.0,
        "align-items:baseline; flex-wrap:wrap",
    );
    let root = fixture.root();
    for index in 0..count {
        let width = 8.0 + (index % 5) as f32;
        let height = 10.0 + (index % 7) as f32;
        fixture.leaf(
            root,
            "width:auto; height:auto",
            Size::new(width, height),
            Some(height - 2.0),
        );
    }
    fixture.prepare()
}

fn build_baseline_propagation_matrix(nodes: usize) -> BenchCase {
    let groups = nodes.max(3).div_ceil(3);
    let mut fixture = flex_fixture(
        groups as f32 * 36.0,
        36.0,
        "align-items:baseline; flex-wrap:wrap",
    );
    let root = fixture.root();
    for index in 0..groups {
        let direction = if index.is_multiple_of(2) {
            "row"
        } else {
            "column"
        };
        let nested = fixture.container(
            root,
            &format!(
                "display:flex; flex-direction:{direction}; align-items:baseline; width:34px; height:30px"
            ),
        );
        for child in 0..2 {
            let height = 10.0 + (index + child) as f32 % 8.0;
            fixture.leaf(
                nested,
                "width:auto; height:auto",
                Size::new(12.0, height),
                Some(height - 1.0),
            );
        }
    }
    fixture.prepare()
}

fn build_measured_callback_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut fixture = flex_fixture(360.0, count.div_ceil(12) as f32 * 18.0, "flex-wrap:wrap");
    let root = fixture.root();
    for index in 0..count {
        let width = 12.0 + (index % 11) as f32;
        let height = 6.0 + (index % 5) as f32;
        let style = match index % 4 {
            0 => "width:auto; height:auto",
            1 => "width:fit-content(28px); height:auto",
            2 => "min-width:10px; max-width:24px; height:auto",
            _ => "width:auto; height:auto; aspect-ratio:2",
        };
        fixture.leaf(root, style, Size::new(width, height), Some(height - 1.0));
    }
    fixture.prepare()
}

fn build_absolute_children(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut fixture = flex_fixture(
        640.0,
        480.0,
        "position:relative; flex-wrap:wrap; align-items:flex-start",
    );
    let root = fixture.root();
    for index in 0..count {
        let style = if index.is_multiple_of(2) {
            format!(
                "position:absolute; left:{}px; top:{}px; width:8px; height:6px",
                index % 80,
                index % 60
            )
        } else {
            "width:8px; height:6px".to_owned()
        };
        fixture.leaf(root, &style, Size::new(8.0, 6.0), None);
    }
    fixture.prepare()
}

fn build_nested_column_flex(nodes: usize) -> BenchCase {
    let depth = nodes.max(1);
    let mut fixture = flex_fixture(
        100.0,
        depth as f32 * 2.0 + 4.0,
        "flex-direction:column; align-items:flex-start",
    );
    let mut parent = fixture.root();
    for index in 0..depth {
        fixture.leaf(parent, "width:2px; height:2px", Size::new(2.0, 2.0), None);
        parent = fixture.container(
            parent,
            &format!(
                "display:flex; flex-direction:column; width:{}px; height:auto; align-items:flex-start",
                100 - index.min(99)
            ),
        );
    }
    fixture.prepare()
}

fn build_in_flow_order_matrix(nodes: usize) -> BenchCase {
    const ORDERS: [i32; 9] = [-4, -3, -2, -1, 0, 1, 2, 3, 4];

    let count = nodes.max(1);
    let mut fixture = flex_fixture(400.0, count.div_ceil(20) as f32 * 12.0, "flex-wrap:wrap");
    let root = fixture.root();
    for index in 0..count {
        let style = format!(
            "width:10px; height:10px; order:{}",
            ORDERS[index % ORDERS.len()]
        );
        fixture.leaf(root, &style, Size::new(10.0, 10.0), None);
    }
    fixture.prepare()
}

fn build_full_value_spacing_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut fixture = flex_fixture(
        640.0,
        count.div_ceil(12) as f32 * 24.0,
        "flex-wrap:wrap; gap:calc(1% + 1px); align-items:flex-start",
    );
    let root = fixture.root();
    for index in 0..count {
        let style = format!(
            "box-sizing:border-box; width:calc(5% + {}px); height:18px; margin:{}px; padding:2%; border:{}px solid black",
            index % 7,
            index % 3,
            index % 2
        );
        fixture.leaf(root, &style, Size::new(16.0, 10.0), None);
    }
    fixture.prepare()
}

fn build_box_sizing_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut fixture = flex_fixture(600.0, count.div_ceil(10) as f32 * 36.0, "flex-wrap:wrap");
    let root = fixture.root();
    for index in 0..count {
        let sizing = if index.is_multiple_of(2) {
            "border-box"
        } else {
            "content-box"
        };
        let style = format!(
            "box-sizing:{sizing}; width:50px; height:28px; padding:{}px; border:{}px solid black",
            1 + index % 4,
            index % 3
        );
        fixture.leaf(root, &style, Size::new(32.0, 12.0), None);
    }
    fixture.prepare()
}

fn build_fit_content_subtrees(nodes: usize) -> BenchCase {
    let groups = nodes.max(4).div_ceil(4);
    let mut fixture = flex_fixture(
        640.0,
        groups as f32 * 28.0,
        "flex-direction:column; align-items:flex-start",
    );
    let root = fixture.root();
    for index in 0..groups {
        let container = fixture.container(
            root,
            &format!(
                "display:flex; width:fit-content(calc(50% + {}px)); min-width:40px; max-width:600px; height:26px",
                index % 9
            ),
        );
        for child in 0..3 {
            let width = 12.0 + ((index + child) % 8) as f32;
            fixture.leaf(
                container,
                "width:auto; height:auto",
                Size::new(width, 12.0),
                None,
            );
        }
    }
    fixture.prepare()
}

fn build_mixed_display_none(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut fixture = flex_fixture(500.0, count.div_ceil(20) as f32 * 12.0, "flex-wrap:wrap");
    let root = fixture.root();
    for index in 0..count {
        let style = if index % 5 == 0 {
            "display:none; width:10px; height:10px"
        } else {
            "width:10px; height:10px"
        };
        let child = fixture.container(root, style);
        if index % 5 == 0 {
            fixture.leaf(child, "width:8px; height:8px", Size::new(8.0, 8.0), None);
        }
    }
    fixture.prepare()
}
