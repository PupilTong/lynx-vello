//! CSS Positioned Layout §3.4 constraints over committed box geometry.
//!
//! Layout stays in normal flow. The frame and geometry queries both sample
//! these constraints against current scroll offsets; neither mutates layout.

use euclid::default::Vector2D;
use hughie::geometry::Edges;
use hughie::style::PositionProperty;
use hughie::tree::Layout;
use stylo::values::computed::{Inset, Length, Margin};

use crate::NodeId;
use crate::layout::box_parent;
use crate::tree::document::Document;
use crate::tree::node::Node;

/// One axis of the sticky view rectangle and its containing-block limit.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct StickyAxis {
    normal_start: f32,
    size: f32,
    min_offset: f32,
    max_offset: f32,
    inset_start: Option<f32>,
    inset_end: Option<f32>,
    scrollport_size: f32,
    /// Whether the physical low edge is the logical end edge.
    end_is_start: bool,
}

impl StickyAxis {
    /// The box's own displacement, excluding the displacement inherited from
    /// sticky ancestors between its containing block and this scrollport.
    pub(crate) fn offset(self, scroll: f32, ancestor_shift: f32) -> f32 {
        if self.inset_start.is_none() && self.inset_end.is_none() {
            return 0.0;
        }
        let mut start = self.inset_start.unwrap_or(0.0);
        let mut end = self.inset_end.unwrap_or(0.0);
        let deficit = (self.size - (self.scrollport_size - start - end)).max(0.0);
        if self.end_is_start {
            start -= deficit;
        } else {
            end -= deficit;
        }
        let normal = self.normal_start + ancestor_shift - scroll;
        let mut shift: f32 = 0.0;
        if self.inset_start.is_some() {
            shift = shift.max(start - normal);
        }
        if self.inset_end.is_some() {
            shift = shift.min(self.scrollport_size - end - self.size - normal);
        }
        let (min, max) = self.offset_bounds();
        shift.clamp(min, max)
    }

    /// Conservative travel bounds, also used to retain ink that can become
    /// visible later without rebuilding a frame.
    pub(crate) fn offset_bounds(self) -> (f32, f32) {
        (
            if self.inset_end.is_some() {
                self.min_offset.min(0.0)
            } else {
                0.0
            },
            if self.inset_start.is_some() {
                self.max_offset.max(0.0)
            } else {
                0.0
            },
        )
    }
}

fn inset(value: &Inset, basis: f32) -> Option<f32> {
    match value {
        Inset::Auto => None,
        Inset::LengthPercentage(value) => Some(value.resolve(Length::new(basis)).px()),
        Inset::AnchorFunction(_)
        | Inset::AnchorSizeFunction(_)
        | Inset::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor insets are pref-dead under the lynx feature")
        }
    }
}

fn origin<T>(document: &Document<T>, id: NodeId) -> Vector2D<f32> {
    let mut current = document.get(id);
    let mut result = Vector2D::zero();
    while let Some(node) = current {
        if let Some(layout) = document.rounded_layout(node.id()) {
            result += Vector2D::new(layout.location.x, layout.location.y);
        }
        current = box_parent(node);
    }
    result
}

fn scrollport_axis<T>(document: &Document<T>, scroller: Option<NodeId>, axis: usize) -> (f32, f32) {
    let Some(scroller) = scroller else {
        let viewport = document.viewport_size();
        return (
            0.0,
            if axis == 0 {
                viewport.width
            } else {
                viewport.height
            },
        );
    };
    let layout = document
        .rounded_layout(scroller)
        .expect("scroll node has layout");
    let origin = origin(document, scroller);
    if axis == 0 {
        (
            origin.x + layout.border.left,
            layout.size.width - layout.border.left - layout.border.right,
        )
    } else {
        (
            origin.y + layout.border.top,
            layout.size.height - layout.border.top - layout.border.bottom,
        )
    }
}

fn containing_block<T>(
    document: &Document<T>,
    parent: Option<&Node<T>>,
    layout: &Layout,
) -> Edges<f32> {
    if let Some(bounds) = layout.containing_block.as_deref() {
        return *bounds;
    }
    let Some((parent, layout)) = parent.and_then(|node| {
        document
            .rounded_layout(node.id())
            .map(|layout| (node, layout))
    }) else {
        let viewport = document.viewport_size();
        return Edges {
            left: 0.0,
            right: viewport.width,
            top: 0.0,
            bottom: viewport.height,
        };
    };
    let scrollable = document.is_scroll_container(parent.id());
    Edges {
        left: layout.border.left + layout.padding.left,
        top: layout.border.top + layout.padding.top,
        right: if scrollable {
            layout
                .content_size
                .width
                .max(layout.size.width - layout.border.right)
        } else {
            layout.size.width - layout.border.right
        } - layout.padding.right,
        bottom: if scrollable {
            layout
                .content_size
                .height
                .max(layout.size.height - layout.border.bottom)
        } else {
            layout.size.height - layout.border.bottom
        } - layout.padding.bottom,
    }
}

/// Resolve the CSS constraints once when the frame is built. A missing
/// scroll node selects the viewport; each axis may select a different node.
pub(crate) fn axes<T>(
    document: &Document<T>,
    id: NodeId,
    scroll_x: Option<NodeId>,
    scroll_y: Option<NodeId>,
) -> [StickyAxis; 2] {
    let node = document.get(id).expect("a sticky box is live");
    let layout = document
        .rounded_layout(id)
        .expect("a sticky box has layout");
    let style = node
        .layout_computed_style()
        .expect("a sticky box has style");
    let position = style.get_position();
    let margin_style = style.get_margin();
    // WPT position-sticky-margins-002/003: an auto margin contributes zero
    // to sticky limits, even if flex/grid distributed free space into it.
    let used_margin = |margin: &Margin, resolved| {
        if matches!(margin, Margin::Auto) {
            0.0
        } else {
            resolved
        }
    };
    let margin = Edges {
        left: used_margin(&margin_style.margin_left, layout.margin.left),
        right: used_margin(&margin_style.margin_right, layout.margin.right),
        top: used_margin(&margin_style.margin_top, layout.margin.top),
        bottom: used_margin(&margin_style.margin_bottom, layout.margin.bottom),
    };
    let parent = box_parent(node);
    let containing = containing_block(document, parent, layout);
    let writing = parent
        .and_then(|parent| parent.layout_computed_style())
        .unwrap_or(style)
        .writing_mode;
    let reversed = [
        if writing.is_vertical() {
            !writing.is_vertical_lr()
        } else {
            !writing.is_bidi_ltr()
        },
        writing.is_vertical() && !writing.is_inline_tb(),
    ];
    let absolute = origin(document, id);
    std::array::from_fn(|axis| {
        let (normal, size, margin_start, margin_end, cb_start, cb_end, low, high, scroller) =
            if axis == 0 {
                (
                    layout.location.x,
                    layout.size.width,
                    margin.left,
                    margin.right,
                    containing.left,
                    containing.right,
                    &position.left,
                    &position.right,
                    scroll_x,
                )
            } else {
                (
                    layout.location.y,
                    layout.size.height,
                    margin.top,
                    margin.bottom,
                    containing.top,
                    containing.bottom,
                    &position.top,
                    &position.bottom,
                    scroll_y,
                )
            };
        let (scrollport_start, scrollport_size) = scrollport_axis(document, scroller, axis);
        // css-position-3's position box reduces a margin to the distance
        // between that margin edge and the containing-block edge when that
        // distance is smaller. Keep signed values, including negative margins.
        let start_margin = margin_start.min(normal - margin_start - cb_start);
        let end_margin = margin_end.min(cb_end - normal - size - margin_end);
        StickyAxis {
            normal_start: if axis == 0 { absolute.x } else { absolute.y } - scrollport_start,
            size,
            min_offset: cb_start + start_margin - normal,
            max_offset: cb_end - end_margin - normal - size,
            inset_start: inset(low, scrollport_size),
            inset_end: inset(high, scrollport_size),
            scrollport_size,
            end_is_start: reversed[axis],
        }
    })
}

/// Samples one box's own shift for the untransformed geometry API. Memoizing
/// ancestor samples keeps a chain of nested sticky boxes linear in samples.
pub(crate) fn live_offset<T>(
    document: &Document<T>,
    id: NodeId,
    sampled: &mut Vec<(NodeId, Vector2D<f32>)>,
) -> Vector2D<f32> {
    if let Some((_, offset)) = sampled.iter().find(|(node, _)| *node == id) {
        return *offset;
    }
    let mut scroll_nodes = [None; 2];
    let mut current = document.scroll_parent(id);
    while let Some(ancestor) = current {
        if let Some(style) = document
            .get(ancestor)
            .and_then(|node| node.layout_computed_style())
        {
            if scroll_nodes[0].is_none() && style.clone_overflow_x().is_scrollable() {
                scroll_nodes[0] = Some(ancestor);
            }
            if scroll_nodes[1].is_none() && style.clone_overflow_y().is_scrollable() {
                scroll_nodes[1] = Some(ancestor);
            }
        }
        current = document.scroll_parent(ancestor);
    }
    let mut inherited = Vector2D::zero();
    let mut inside = [true; 2];
    current = document.scroll_parent(id);
    while let Some(ancestor) = current {
        for axis in 0..2 {
            inside[axis] &= scroll_nodes[axis] != Some(ancestor);
        }
        if !inside[0] && !inside[1] {
            break;
        }
        if document
            .get(ancestor)
            .and_then(|node| node.layout_computed_style())
            .is_some_and(|style| style.clone_position() == PositionProperty::Sticky)
        {
            let offset = live_offset(document, ancestor, sampled);
            if inside[0] {
                inherited.x += offset.x;
            }
            if inside[1] {
                inherited.y += offset.y;
            }
        }
        current = document.scroll_parent(ancestor);
    }
    let [x, y] = axes(document, id, scroll_nodes[0], scroll_nodes[1]);
    let offset = Vector2D::new(
        x.offset(
            scroll_nodes[0].map_or(0.0, |node| document.scroll_offset(node).x),
            inherited.x,
        ),
        y.offset(
            scroll_nodes[1].map_or(0.0, |node| document.scroll_offset(node).y),
            inherited.y,
        ),
    );
    sampled.push((id, offset));
    offset
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::StickyAxis;

    #[test]
    fn oversized_sticky_reduces_the_logical_end_inset() {
        let ltr = StickyAxis {
            normal_start: 0.0,
            size: 200.0,
            min_offset: -500.0,
            max_offset: 500.0,
            inset_start: Some(20.0),
            inset_end: Some(10.0),
            scrollport_size: 100.0,
            end_is_start: false,
        };
        let rtl = StickyAxis {
            end_is_start: true,
            ..ltr
        };
        // The 200px box cannot fit between 20px and 90px. LTR preserves the
        // left inset; RTL preserves the right inset, allowing the opposite
        // edge to overflow rather than forcing the box back and forth.
        assert_eq!(ltr.offset(0.0, 0.0), 20.0);
        assert_eq!(rtl.offset(0.0, 0.0), -110.0);
        assert_eq!(ltr.offset(50.0, 0.0), 70.0);
        assert_eq!(rtl.offset(50.0, 0.0), -60.0);
    }

    #[test]
    fn auto_inset_stays_unconstrained_when_the_view_rectangle_is_enlarged() {
        let axis = StickyAxis {
            normal_start: 40.0,
            size: 200.0,
            min_offset: -500.0,
            max_offset: 500.0,
            inset_start: Some(20.0),
            inset_end: None,
            scrollport_size: 100.0,
            end_is_start: false,
        };
        assert_eq!(axis.offset(0.0, 0.0), 0.0);
        assert_eq!(axis.offset(100.0, 0.0), 80.0);
        assert_eq!(axis.offset(100.0, 80.0), 0.0);
    }
}
