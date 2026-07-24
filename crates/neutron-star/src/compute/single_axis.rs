//! Algorithm-neutral machinery for main/cross-axis formatting contexts.

use super::util::Axis;
use crate::geometry::{Edges, Size};
use crate::tree::{
    AvailableSpace, LayoutInput, LayoutNode, LayoutOutput, RequestedAxis, SizingMode,
};

/// Physical main/cross mapping derived by each algorithm frontend.
#[derive(Debug, Clone, Copy)]
pub(super) struct FlowAxes<Base = ()> {
    pub(super) main: Axis,
    pub(super) cross: Axis,
    pub(super) main_reverse: bool,
    pub(super) cross_reverse: bool,
    pub(super) base: Base,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct BaseReversals {
    pub(super) main: bool,
    pub(super) cross: bool,
}

macro_rules! flow_edge {
    ($get:ident, $set:ident, $normal_get:ident, $reverse_get:ident, $normal_set:ident, $reverse_set:ident) => {
        #[inline]
        pub(super) fn $get<T>(edges: Edges<T>, axis: Axis, reverse: bool) -> T {
            if reverse {
                axis.$reverse_get(edges)
            } else {
                axis.$normal_get(edges)
            }
        }

        #[inline]
        pub(super) fn $set<T>(edges: &mut Edges<T>, axis: Axis, reverse: bool, value: T) {
            if reverse {
                axis.$reverse_set(edges, value);
            } else {
                axis.$normal_set(edges, value);
            }
        }
    };
}

flow_edge!(flow_start, set_flow_start, start, end, set_start, set_end);
flow_edge!(flow_end, set_flow_end, end, start, set_end, set_start);

#[inline]
pub(super) fn flow_to_physical(
    flow: f32,
    box_size: f32,
    container_size: f32,
    reverse: bool,
) -> f32 {
    if reverse {
        container_size - flow - box_size
    } else {
        flow
    }
}

/// Builds and performs one child measurement without changing the algorithm's
/// requested axis, sizing mode, or definiteness contract.
#[allow(clippy::too_many_arguments)]
pub(super) fn measure_child<N: LayoutNode>(
    node: N,
    known_dimensions: Size<Option<f32>>,
    definite_dimensions: Size<bool>,
    parent_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    sizing_mode: SizingMode,
    requested_axis: RequestedAxis,
) -> LayoutOutput {
    let mut input = LayoutInput::measure(
        known_dimensions,
        parent_size,
        available_space,
        requested_axis,
    );
    input.definite_dimensions = definite_dimensions;
    input.sizing_mode = sizing_mode;
    node.compute_layout(input)
}
