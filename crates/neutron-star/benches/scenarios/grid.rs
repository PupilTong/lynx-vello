//! Rust-only Grid lowerings of the 18 Grid-tagged benchmark scenarios from
//! PupilTong/lynx#25.

#![allow(dead_code, clippy::cast_precision_loss)]

use neutron_star::prelude::*;
use neutron_star::style::{
    AlignContent, AlignItems, BoxGenerationMode, Dimension, GridAutoFlow, GridLine, GridPlacement,
    JustifyItems, LengthPercentage, LengthPercentageAuto, MaxTrackSizingFunction,
    MinTrackSizingFunction, Position, TrackSizingFunction,
};

use crate::support::{TestStyle, TestTree, perform_layout};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lowering {
    Direct,
    GridSlice,
}

#[derive(Clone, Copy)]
pub(super) struct Scenario {
    pub(super) name: &'static str,
    pub(super) lowering: Lowering,
    pub(super) build: fn(usize) -> BenchCase,
}

#[derive(Debug)]
pub(super) struct BenchCase {
    pub(super) tree: TestTree,
    pub(super) root: NodeId,
    pub(super) known_dimensions: Size<Option<f32>>,
    pub(super) available_space: Size<AvailableSpace>,
}

impl BenchCase {
    pub(super) fn node_count(&self) -> usize {
        self.tree.source.nodes.len()
    }

    pub(super) fn run(&mut self) -> LayoutOutput {
        perform_layout(
            &mut self.tree,
            self.root,
            self.known_dimensions,
            self.available_space,
        )
    }
}

macro_rules! builder {
    ($function:ident, $name:literal) => {
        fn $function(size: usize) -> BenchCase {
            build_grid_scenario($name, size)
        }
    };
}

builder!(build_at_most_owner_matrix, "at_most_owner_matrix");
builder!(
    build_baseline_propagation_matrix,
    "baseline_propagation_matrix"
);
builder!(build_measured_callback_matrix, "measured_callback_matrix");
builder!(build_in_flow_order_matrix, "in_flow_order_matrix");
builder!(build_full_value_spacing_matrix, "full_value_spacing_matrix");
builder!(build_box_sizing_matrix, "box_sizing_matrix");
builder!(build_fit_content_subtrees, "fit_content_subtrees");
builder!(build_sticky_percent_insets, "sticky_percent_insets");
builder!(build_mixed_display_none, "mixed_display_none");
builder!(
    build_grid_out_of_flow_intrinsic,
    "grid_out_of_flow_intrinsic"
);
builder!(build_grid_out_of_flow_areas, "grid_out_of_flow_areas");
builder!(
    build_grid_item_alignment_matrix,
    "grid_item_alignment_matrix"
);
builder!(
    build_grid_content_alignment_matrix,
    "grid_content_alignment_matrix"
);
builder!(build_grid_auto_flow_matrix, "grid_auto_flow_matrix");
builder!(
    build_grid_auto_margin_alignment,
    "grid_auto_margin_alignment"
);
builder!(
    build_grid_minmax_intrinsic_tracks,
    "grid_minmax_intrinsic_tracks"
);
builder!(
    build_grid_auto_fit_content_max_tracks,
    "grid_auto_fit_content_max_tracks"
);
builder!(
    build_grid_indefinite_auto_fit_content_max_tracks,
    "grid_indefinite_auto_fit_content_max_tracks"
);

macro_rules! scenario {
    ($name:literal, $lowering:ident, $build:ident) => {
        Scenario {
            name: $name,
            lowering: Lowering::$lowering,
            build: $build,
        }
    };
}

pub(super) const SCENARIOS: &[Scenario] = &[
    scenario!(
        "at_most_owner_matrix",
        GridSlice,
        build_at_most_owner_matrix
    ),
    scenario!(
        "baseline_propagation_matrix",
        GridSlice,
        build_baseline_propagation_matrix
    ),
    scenario!(
        "measured_callback_matrix",
        GridSlice,
        build_measured_callback_matrix
    ),
    scenario!(
        "in_flow_order_matrix",
        GridSlice,
        build_in_flow_order_matrix
    ),
    scenario!(
        "full_value_spacing_matrix",
        GridSlice,
        build_full_value_spacing_matrix
    ),
    scenario!("box_sizing_matrix", GridSlice, build_box_sizing_matrix),
    scenario!(
        "fit_content_subtrees",
        GridSlice,
        build_fit_content_subtrees
    ),
    scenario!(
        "sticky_percent_insets",
        GridSlice,
        build_sticky_percent_insets
    ),
    scenario!("mixed_display_none", GridSlice, build_mixed_display_none),
    scenario!(
        "grid_out_of_flow_intrinsic",
        Direct,
        build_grid_out_of_flow_intrinsic
    ),
    scenario!(
        "grid_out_of_flow_areas",
        Direct,
        build_grid_out_of_flow_areas
    ),
    scenario!(
        "grid_item_alignment_matrix",
        Direct,
        build_grid_item_alignment_matrix
    ),
    scenario!(
        "grid_content_alignment_matrix",
        Direct,
        build_grid_content_alignment_matrix
    ),
    scenario!("grid_auto_flow_matrix", Direct, build_grid_auto_flow_matrix),
    scenario!(
        "grid_auto_margin_alignment",
        Direct,
        build_grid_auto_margin_alignment
    ),
    scenario!(
        "grid_minmax_intrinsic_tracks",
        Direct,
        build_grid_minmax_intrinsic_tracks
    ),
    scenario!(
        "grid_auto_fit_content_max_tracks",
        Direct,
        build_grid_auto_fit_content_max_tracks
    ),
    scenario!(
        "grid_indefinite_auto_fit_content_max_tracks",
        Direct,
        build_grid_indefinite_auto_fit_content_max_tracks
    ),
];

pub(super) fn scenario_named(name: &str) -> Scenario {
    *SCENARIOS
        .iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("unknown Grid benchmark scenario {name}"))
}

fn line(value: i16) -> GridPlacement {
    GridPlacement::Line(GridLine::new(value))
}

#[allow(clippy::too_many_lines)]
fn build_grid_scenario(name: &str, size: usize) -> BenchCase {
    let size = size.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(size);
    for index in 0..size {
        let hidden = name == "mixed_display_none" && index % 5 == 0;
        let out_of_flow = name.starts_with("grid_out_of_flow") && index % 7 == 0;
        let sticky = name == "sticky_percent_insets" && index % 7 == 0;
        let alignment = match index % 5 {
            0 => AlignItems::Start,
            1 => AlignItems::End,
            2 => AlignItems::Center,
            3 => AlignItems::Stretch,
            _ => AlignItems::Baseline,
        };
        let mut style = TestStyle {
            box_generation_mode: if hidden {
                BoxGenerationMode::None
            } else {
                BoxGenerationMode::Normal
            },
            position: if out_of_flow {
                Position::Absolute
            } else {
                Position::Relative
            },
            size: Size::new(Dimension::Auto, Dimension::Auto),
            min_size: Size::new(Dimension::Length(4.0), Dimension::Length(3.0)),
            max_size: Size::new(Dimension::Length(48.0), Dimension::Length(32.0)),
            align_self: Some(alignment),
            order: if name == "in_flow_order_matrix" {
                i32::try_from(index % 11).expect("modulo result fits i32") - 5
            } else {
                0
            },
            grid_column: if out_of_flow || name == "grid_out_of_flow_areas" {
                Line::new(
                    line(i16::try_from(index % 10 + 1).expect("modulo result fits i16")),
                    GridPlacement::Span(1),
                )
            } else if index % 13 == 0 {
                Line::new(GridPlacement::Auto, GridPlacement::Span(2))
            } else {
                Line::new(GridPlacement::Auto, GridPlacement::Auto)
            },
            grid_row: Line::new(GridPlacement::Auto, GridPlacement::Auto),
            justify_self: Some(match index % 4 {
                0 => AlignItems::Start,
                1 => AlignItems::End,
                2 => AlignItems::Center,
                _ => AlignItems::Stretch,
            }),
            ..TestStyle::default()
        };
        if name == "grid_auto_margin_alignment" {
            style.margin.left = if index.is_multiple_of(2) {
                LengthPercentageAuto::Auto
            } else {
                LengthPercentageAuto::ZERO
            };
            style.margin.right = if index.is_multiple_of(3) {
                LengthPercentageAuto::Auto
            } else {
                LengthPercentageAuto::ZERO
            };
        }
        if sticky {
            style.inset.left = LengthPercentageAuto::Percent(0.1);
            style.inset.top = LengthPercentageAuto::Percent(0.2);
        }
        let min_width = 5.0 + (index % 7) as f32;
        let max_width = min_width + 12.0 + (index % 9) as f32;
        children.push(tree.push_intrinsic_leaf(
            style,
            Size::new(min_width, 7.0),
            Size::new(max_width, 9.0 + (index % 4) as f32),
        ));
    }

    let intrinsic_track = TrackSizingFunction::minmax(
        MinTrackSizingFunction::MinContent,
        MaxTrackSizingFunction::MaxContent,
    );
    let fit_track = TrackSizingFunction::minmax(
        MinTrackSizingFunction::Auto,
        MaxTrackSizingFunction::FitContent(LengthPercentage::Length(52.0)),
    );
    let columns = if name.contains("minmax_intrinsic") {
        vec![intrinsic_track; 12]
    } else if name.contains("fit_content_max") {
        vec![fit_track; 12]
    } else {
        vec![TrackSizingFunction::fr(1.0); 12]
    };
    let auto_flow = if name == "grid_auto_flow_matrix" {
        GridAutoFlow::RowDense
    } else {
        GridAutoFlow::Row
    };
    let root = tree.push_grid(
        TestStyle {
            size: Size::new(Dimension::Length(640.0), Dimension::Length(480.0)),
            template_columns: columns,
            auto_rows: vec![TrackSizingFunction::AUTO],
            auto_flow,
            gap: Size::new(LengthPercentage::Length(2.0), LengthPercentage::Length(2.0)),
            align_content: Some(if name == "grid_content_alignment_matrix" {
                AlignContent::SpaceAround
            } else {
                AlignContent::Stretch
            }),
            align_items: Some(AlignItems::Stretch),
            justify_items: Some(JustifyItems::Stretch),
            ..TestStyle::default()
        },
        children,
    );
    let known_dimensions = if name == "at_most_owner_matrix"
        || name == "grid_indefinite_auto_fit_content_max_tracks"
    {
        Size::NONE
    } else {
        Size::new(Some(640.0), Some(480.0))
    };
    BenchCase {
        tree,
        root,
        known_dimensions,
        available_space: Size::new(
            AvailableSpace::Definite(640.0),
            AvailableSpace::Definite(480.0),
        ),
    }
}
