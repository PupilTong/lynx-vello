//! Atomic inline-level sizing and placement over the existing inner layout
//! algorithms.
//!
//! An `inline-flex`, `inline-grid`, `inline-linear`, or `inline-relative` box
//! participates in its parent's inline formatting context as one indivisible
//! item. Its inside is still laid out by the ordinary algorithm selected by
//! the host. This adapter owns only the outside contract: intrinsic probes,
//! shrink-to-fit sizing against the containing block, and conversion from a
//! line builder's margin-box position to Hughie's durable border-box layout.

use stylo::values::computed::PositionProperty;

use super::util::{
    relative_offset, resolve_border, resolve_insets, resolve_item_geometry, resolve_padding,
};
use crate::geometry::{Edges, Point, Size};
use crate::style::CoreStyle;
use crate::tree::{
    AvailableSpace, Layout, LayoutInput, LayoutOutput, LayoutTree, RequestedAxis, SizingMode,
};

/// The measured outside geometry of one atomic inline-level box.
///
/// Values are resolved against the inline formatting context's containing
/// block, not against the space left on the current line. The latter only
/// decides whether this already-sized item stays on that line or wraps.
#[derive(Debug, Clone, Copy)]
pub struct AtomicInlineMetrics<N> {
    node: N,
    parent_size: Size<Option<f32>>,
    border_box_size: Size<f32>,
    margin_box_size: Size<f32>,
    first_baselines: Point<Option<f32>>,
    margin: Edges<f32>,
    padding: Edges<f32>,
    border: Edges<f32>,
}

/// Resolve the content-box origin relative to a box's border-box origin.
///
/// This is the coordinate offset an inline-content host adds before feeding
/// Parley's content-local item positions back to Hughie. As required by CSS,
/// padding percentages use the containing block's inline size.
#[must_use]
pub fn content_box_origin(style: &impl CoreStyle, parent_inline_basis: Option<f32>) -> Point<f32> {
    let border = resolve_border(&style.border());
    let padding = resolve_padding(style.padding(), parent_inline_basis);
    Point::new(border.left + padding.left, border.top + padding.top)
}

/// Resolve a box's padding-box origin and size in border-box coordinates.
#[must_use]
pub fn padding_box_geometry(
    style: &impl CoreStyle,
    border_box_size: Size<f32>,
) -> (Point<f32>, Size<f32>) {
    let border = resolve_border(&style.border());
    (
        Point::new(border.left, border.top),
        Size::new(
            (border_box_size.width - border.horizontal_sum()).max(0.0),
            (border_box_size.height - border.vertical_sum()).max(0.0),
        ),
    )
}

impl<N: Copy> AtomicInlineMetrics<N> {
    /// The node whose inner formatting algorithm was measured.
    #[must_use]
    pub const fn node(self) -> N {
        self.node
    }

    /// The used border-box size selected by the fit-content calculation.
    #[must_use]
    pub const fn border_box_size(self) -> Size<f32> {
        self.border_box_size
    }

    /// The indivisible size contributed to the containing line.
    #[must_use]
    pub const fn margin_box_size(self) -> Size<f32> {
        self.margin_box_size
    }

    /// The first baselines measured from the margin-box origin.
    ///
    /// An absent baseline is synthesized by the caller from the appropriate
    /// margin-box edge.
    #[must_use]
    pub const fn first_baselines(self) -> Point<Option<f32>> {
        self.first_baselines
    }
}

/// Measure an atomic inline box using the CSS fit-content formula.
///
/// `parent_size` is the inline formatting context's containing-block size.
/// `available_space.width` must likewise describe that containing block; it
/// must not be replaced with the remainder of a partially occupied line.
/// Hughie performs min-content and max-content probes first, chooses the used
/// width, then performs the final height-for-width probe. No durable geometry
/// is written by this function.
pub fn measure_atomic_inline<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    parent_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) -> AtomicInlineMetrics<T::NodeId> {
    let geometry = resolve_item_geometry(&tree.style(node), parent_size);
    let intrinsic = |state: &mut T::State, inline_space| {
        let input = LayoutInput::measure(
            Size::NONE,
            parent_size,
            Size::new(inline_space, available_space.height),
            RequestedAxis::Horizontal,
        );
        tree.compute_layout(state, node, input).size.width
    };

    let min_content = intrinsic(state, AvailableSpace::MinContent);
    let max_content = intrinsic(state, AvailableSpace::MaxContent).max(min_content);
    let available_border_width = match available_space.width {
        AvailableSpace::Definite(width) => (width - geometry.margin.horizontal_sum()).max(0.0),
        AvailableSpace::MinContent => min_content,
        AvailableSpace::MaxContent => max_content,
    };
    let used_width = max_content.min(min_content.max(available_border_width));

    // Once fit-content selects a width it is a definite input to the inner
    // algorithm. Size styles still apply on the other axis, so an authored
    // height, min/max-height, and aspect-ratio remain effective.
    let mut final_input = LayoutInput::measure(
        Size::new(Some(used_width), None),
        parent_size,
        Size::new(AvailableSpace::Definite(used_width), available_space.height),
        RequestedAxis::Both,
    );
    final_input.definite_dimensions.width = true;
    let output = tree.compute_layout(state, node, final_input);
    let border_box_size = output.size;
    let margin_box_size = Size::new(
        (border_box_size.width + geometry.margin.horizontal_sum()).max(0.0),
        (border_box_size.height + geometry.margin.vertical_sum()).max(0.0),
    );
    let first_baselines = Point::new(
        output
            .first_baselines
            .x
            .map(|baseline| geometry.margin.left + baseline),
        output
            .first_baselines
            .y
            .map(|baseline| geometry.margin.top + baseline),
    );

    AtomicInlineMetrics {
        node,
        parent_size,
        border_box_size,
        margin_box_size,
        first_baselines,
        margin: geometry.margin,
        padding: geometry.padding,
        border: geometry.border,
    }
}

/// Commit a previously measured atomic inline box at a margin-box origin.
///
/// The existing inner layout algorithm is recursively committed at the used
/// border-box size. The durable [`Layout::location`] is the border-box origin,
/// as everywhere else in Hughie; margins and relative positioning are applied
/// while converting from the line builder's coordinate system.
pub fn commit_atomic_inline<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    metrics: AtomicInlineMetrics<T::NodeId>,
    margin_box_origin: Point<f32>,
    order: u32,
) -> LayoutOutput {
    let size = metrics.border_box_size;
    let mut input = LayoutInput::commit(
        size.map(Some),
        metrics.parent_size,
        size.map(AvailableSpace::Definite),
    );
    // The fit-content pass has already applied the box's preferred/min/max
    // size styles. Commit the selected result without resolving them a second
    // time against a potentially different constraint.
    input.sizing_mode = SizingMode::IgnoreSizeStyles;
    let output = tree.compute_layout(state, metrics.node, input);

    let style = tree.style(metrics.node);
    let offset = if style.position() == PositionProperty::Relative {
        relative_offset(
            resolve_insets(style.inset(), metrics.parent_size),
            style.direction(),
        )
    } else {
        Point::ZERO
    };
    let mut layout = Layout::with_order(order);
    layout.location = Point::new(
        margin_box_origin.x + metrics.margin.left + offset.x,
        margin_box_origin.y + metrics.margin.top + offset.y,
    );
    layout.size = output.size;
    layout.content_size = output.content_size;
    layout.margin = metrics.margin;
    layout.padding = metrics.padding;
    layout.border = metrics.border;
    tree.set_unrounded_layout(state, metrics.node, layout);

    // A conforming inner algorithm returns the probed size here. Retaining the
    // measured content size is unnecessary, but compare in debug builds so a
    // host dispatch bug cannot silently make line breaking disagree with the
    // committed box.
    debug_assert_eq!(output.size, metrics.border_box_size);
    output
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests {
    use core::cell::RefCell;

    use stylo::values::computed::{Display, Inset, Length, LengthPercentage, Margin, Percentage};

    use super::*;
    use crate::style::CoreStyle;
    use crate::tree::{LayoutGoal, LayoutSlot};

    fn length(value: f32) -> LengthPercentage {
        LengthPercentage::new_length(Length::new(value))
    }

    #[derive(Debug)]
    struct TestStyle {
        margin: Edges<Margin>,
        position: PositionProperty,
        inset: Edges<Inset>,
    }

    impl Default for TestStyle {
        fn default() -> Self {
            Self {
                margin: Edges {
                    left: Margin::LengthPercentage(length(3.0)),
                    right: Margin::LengthPercentage(length(5.0)),
                    top: Margin::LengthPercentage(length(7.0)),
                    bottom: Margin::LengthPercentage(length(11.0)),
                },
                position: PositionProperty::Static,
                inset: Edges::uniform(Inset::Auto),
            }
        }
    }

    impl CoreStyle for TestStyle {
        fn display(&self) -> Display {
            Display::Flex
        }

        fn position(&self) -> PositionProperty {
            self.position
        }

        fn inset(&self) -> Edges<&Inset> {
            self.inset.as_ref()
        }

        fn margin(&self) -> Edges<&Margin> {
            self.margin.as_ref()
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct TestNode;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Call {
        goal: LayoutGoal,
        known: Size<Option<f32>>,
        available: Size<AvailableSpace>,
        sizing_mode: SizingMode,
    }

    #[derive(Debug, Default)]
    struct TestTree {
        style: TestStyle,
        calls: RefCell<Vec<Call>>,
    }

    impl LayoutTree for TestTree {
        type NodeId = TestNode;
        type State = LayoutSlot;
        type Style<'tree> = &'tree TestStyle;
        type ChildIter<'tree> = core::iter::Empty<TestNode>;

        fn children(&self, _node: TestNode) -> Self::ChildIter<'_> {
            core::iter::empty()
        }

        fn style(&self, _node: TestNode) -> Self::Style<'_> {
            &self.style
        }

        fn layout<'state>(
            &self,
            state: &'state Self::State,
            _node: TestNode,
        ) -> &'state LayoutSlot {
            state
        }

        fn layout_mut<'state>(
            &self,
            state: &'state mut Self::State,
            _node: TestNode,
        ) -> &'state mut LayoutSlot {
            state
        }

        fn compute_layout(
            &self,
            _state: &mut Self::State,
            _node: TestNode,
            input: LayoutInput,
        ) -> LayoutOutput {
            self.calls.borrow_mut().push(Call {
                goal: input.goal,
                known: input.known_dimensions,
                available: input.available_space,
                sizing_mode: input.sizing_mode,
            });
            let size = match input.goal {
                LayoutGoal::Measure(RequestedAxis::Horizontal) => match input.available_space.width
                {
                    AvailableSpace::MinContent => Size::new(20.0, 0.0),
                    AvailableSpace::MaxContent => Size::new(100.0, 0.0),
                    AvailableSpace::Definite(_) => unreachable!(),
                },
                LayoutGoal::Measure(RequestedAxis::Both) => {
                    Size::new(input.known_dimensions.width.unwrap(), 30.0)
                }
                LayoutGoal::Measure(RequestedAxis::Vertical) => unreachable!(),
                LayoutGoal::Commit => input.known_dimensions.unwrap_or(Size::ZERO),
            };
            LayoutOutput::new(size, size).with_first_baselines(Point::new(None, Some(18.0)))
        }
    }

    #[test]
    fn fit_content_uses_the_containing_block_then_probes_height_for_width() {
        let tree = TestTree::default();
        let mut state = LayoutSlot::default();
        let metrics = measure_atomic_inline(
            &tree,
            &mut state,
            TestNode,
            Size::new(Some(200.0), None),
            Size::new(AvailableSpace::Definite(58.0), AvailableSpace::MaxContent),
        );

        // 58 containing-block pixels minus 8 horizontal margin pixels.
        assert_eq!(metrics.border_box_size(), Size::new(50.0, 30.0));
        assert_eq!(metrics.margin_box_size(), Size::new(58.0, 48.0));
        assert_eq!(metrics.first_baselines().y, Some(25.0));
        let calls = tree.calls.borrow();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].available.width, AvailableSpace::MinContent);
        assert_eq!(calls[1].available.width, AvailableSpace::MaxContent);
        assert_eq!(calls[2].known.width, Some(50.0));
    }

    #[test]
    fn commit_converts_margin_origin_and_commits_the_selected_inner_size() {
        let mut tree = TestTree::default();
        tree.style.position = PositionProperty::Relative;
        tree.style.inset.left = Inset::LengthPercentage(length(2.0));
        tree.style.inset.top =
            Inset::LengthPercentage(LengthPercentage::new_percent(Percentage(0.1)));
        let mut state = LayoutSlot::default();
        let metrics = measure_atomic_inline(
            &tree,
            &mut state,
            TestNode,
            Size::new(Some(200.0), Some(100.0)),
            Size::new(AvailableSpace::Definite(58.0), AvailableSpace::MaxContent),
        );
        let output = commit_atomic_inline(&tree, &mut state, metrics, Point::new(40.0, 60.0), 9);

        assert_eq!(output.size, Size::new(50.0, 30.0));
        assert_eq!(state.unrounded.order, 9);
        assert_eq!(state.unrounded.location, Point::new(45.0, 77.0));
        assert_eq!(
            state.unrounded.margin,
            tree.style.margin.map(|value| match value {
                Margin::LengthPercentage(value) => value.resolve(Length::new(200.0)).px(),
                Margin::Auto => 0.0,
                _ => unreachable!(),
            })
        );
        assert_eq!(
            tree.calls.borrow().last().unwrap().sizing_mode,
            SizingMode::IgnoreSizeStyles
        );
    }
}
