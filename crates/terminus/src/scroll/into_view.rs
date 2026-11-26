//! CSSOM-View's `scrollIntoView(options)`: §"scroll an element into view"
//! over the scroll containers above an element, each placed by
//! §"determine the scroll-into-view position"
//! (<https://drafts.csswg.org/cssom-view/#dom-element-scrollintoview>, the
//! editor's draft, with its `container` option).
//!
//! The scrolling boxes are the scroll containers on the element's
//! **containing-block** chain, innermost first — the boxes whose offsets move
//! the element, which is the chain [`Document::nearest_scroll_container`]
//! walks. Each one is scrolled to the position that aligns the element's
//! border box, grown by its `scroll-margin`, with the container's scrollport
//! inset by its `scroll-padding` (the snapport, css-scroll-snap-1 §4), and
//! each scroll is a script-facing request ([`Document::scroll_to_with`]), so
//! the painter carries it out.
//!
//! **Axes.** The block axis is y and the inline axis is x: the engine lays
//! out `horizontal-tb` alone. The fork compiles `writing-mode` as an
//! internal longhand no author can set (`style/computed.rs`'s module
//! documentation), and `seldon` assumes a vertical block axis wherever it
//! resolves a logical keyword (`compute/anchor.rs`, `compute/anchor_area.rs`:
//! "the only writing modes the fork has: `horizontal-tb`, `ltr` or `rtl`").
//! `direction` does exist, so the inline start edge (the draft's edge C) is
//! the scrolling box's right edge when its `direction` is `rtl`.
//!
//! **Geometry.** Positions are computed from the last completed layout and
//! nothing here flushes, like every scrolling method. The element's rect is
//! its laid-out border box ([`Document::rect_in_scroll_container`], as for
//! snap areas): transforms, sticky offsets and anchor scroll shifts do not
//! move it.

use euclid::default::Vector2D;
use stylo::properties::ComputedValues;
use stylo::values::computed::Length;

use super::ScrollBehavior;
use super::snap::scroll_padding;
use crate::tree::document::{Document, NodeId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollLogicalPosition {
    Start,
    Center,
    End,
    Nearest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollIntoViewContainer {
    All,
    Nearest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollIntoViewOptions {
    pub behavior: ScrollBehavior,
    pub block: ScrollLogicalPosition,
    pub inline: ScrollLogicalPosition,
    pub container: ScrollIntoViewContainer,
}

impl Default for ScrollIntoViewOptions {
    fn default() -> Self {
        Self {
            behavior: ScrollBehavior::Instant,
            block: ScrollLogicalPosition::Start,
            inline: ScrollLogicalPosition::Nearest,
            container: ScrollIntoViewContainer::All,
        }
    }
}

pub(crate) fn align(
    position: ScrollLogicalPosition,
    current: f32,
    area: (f32, f32),
    port: (f32, f32),
    reversed: bool,
) -> f32 {
    if reversed {
        let mirrored = align(
            position,
            -current,
            (-area.1, -area.0),
            (-port.1, -port.0),
            false,
        );
        return -mirrored;
    }
    let start = area.0 - port.0;
    let end = area.1 - port.1;
    match position {
        ScrollLogicalPosition::Start => start,
        ScrollLogicalPosition::End => end,
        ScrollLogicalPosition::Center => f32::midpoint(start, end),
        ScrollLogicalPosition::Nearest => {
            let start_outside = area.0 < port.0 + current;
            let end_outside = area.1 > port.1 + current;
            let fits = area.1 - area.0 <= port.1 - port.0;
            if start_outside && end_outside {
                current
            } else if (start_outside && fits) || (end_outside && !fits) {
                start
            } else if end_outside || start_outside {
                end
            } else {
                current
            }
        }
    }
}

fn snapport(values: &ComputedValues, width: f32, height: f32) -> ((f32, f32), (f32, f32)) {
    (
        (
            scroll_padding(values.get_scroll_padding_left(), width),
            width - scroll_padding(values.get_scroll_padding_right(), width),
        ),
        (
            scroll_padding(values.get_scroll_padding_top(), height),
            height - scroll_padding(values.get_scroll_padding_bottom(), height),
        ),
    )
}

impl<T> Document<T> {
    pub fn scroll_into_view(&mut self, target: NodeId, options: ScrollIntoViewOptions) -> bool {
        if self.bounding_client_rect(target).is_none() {
            return false;
        }
        let mut found = false;
        let mut inner_offsets = Vector2D::zero();
        let mut current = self.nearest_scroll_container(target);
        while let Some(container) = current {
            found = true;
            let Some(scroll_box) = self.scroll_box(container) else {
                break;
            };
            let mut offset = scroll_box.offset;
            if let Some(position) = self.scroll_into_view_position(
                target,
                container,
                options.block,
                options.inline,
                inner_offsets,
            ) {
                let position = scroll_box.admit(position);
                if position != scroll_box.offset || self.pending_scroll_request(container).is_some()
                {
                    offset = self.scroll_to_with(container, position, options.behavior);
                }
            }
            if options.container == ScrollIntoViewContainer::Nearest {
                break;
            }
            inner_offsets += offset;
            current = self.nearest_scroll_container(container);
        }
        found
    }

    pub(crate) fn scroll_into_view_position(
        &self,
        target: NodeId,
        container: NodeId,
        block: ScrollLogicalPosition,
        inline: ScrollLogicalPosition,
        inner_offsets: Vector2D<f32>,
    ) -> Option<Vector2D<f32>> {
        let scroll_box = self.scroll_box(container)?;
        let style = self.get(container)?.layout_computed_style()?;
        let port = scroll_box.scrollport;
        let (snapport_x, snapport_y) = snapport(style, port.width, port.height);
        let rtl = !style.writing_mode.is_bidi_ltr();
        let rect = self
            .rect_in_scroll_container(target, container)?
            .translate(-inner_offsets);
        let values = self.get(target)?.layout_computed_style()?;
        let margin = |length: Length| length.px();
        let area_x = (
            rect.min_x() - margin(*values.get_scroll_margin_left()),
            rect.max_x() + margin(*values.get_scroll_margin_right()),
        );
        let area_y = (
            rect.min_y() - margin(*values.get_scroll_margin_top()),
            rect.max_y() + margin(*values.get_scroll_margin_bottom()),
        );
        Some(Vector2D::new(
            align(inline, scroll_box.offset.x, area_x, snapport_x, rtl),
            align(block, scroll_box.offset.y, area_y, snapport_y, false),
        ))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp, reason = "the expectations are exact offsets")]
mod tests {
    use super::ScrollLogicalPosition::{Center, End, Nearest, Start};
    use super::*;
    use crate::style::engine::StylesheetOrigin;
    use crate::tree::document::tests::device;

    #[test]
    fn align_places_each_position_against_the_snapport() {
        let area = (200.0, 250.0);
        assert_eq!(align(Start, 0.0, area, (0.0, 100.0), false), 200.0);
        assert_eq!(align(End, 0.0, area, (0.0, 100.0), false), 150.0);
        assert_eq!(align(Center, 0.0, area, (0.0, 100.0), false), 175.0);
        assert_eq!(align(Start, 0.0, area, (10.0, 90.0), false), 190.0);
        assert_eq!(align(End, 0.0, area, (10.0, 90.0), false), 160.0);
    }

    #[test]
    fn nearest_leaves_a_visible_or_covering_element_alone_and_aligns_one_edge() {
        let port = (0.0, 100.0);
        assert_eq!(align(Nearest, 0.0, (20.0, 80.0), port, false), 0.0);
        assert_eq!(
            align(Nearest, 0.0, (-20.0, 180.0), port, false),
            0.0,
            "covers both edges"
        );
        assert_eq!(
            align(Nearest, 0.0, (300.0, 350.0), port, false),
            250.0,
            "shorter, past the end: the end edges align"
        );
        assert_eq!(
            align(Nearest, 400.0, (300.0, 350.0), port, false),
            300.0,
            "shorter, before the start: the start edges align"
        );
        assert_eq!(
            align(Nearest, 0.0, (300.0, 400.0), port, false),
            300.0,
            "exactly the box's size: either edge, the same offset"
        );
        assert_eq!(
            align(Nearest, 0.0, (300.0, 450.0), port, false),
            300.0,
            "taller, past the end: its start edge aligns, showing its start"
        );
        assert_eq!(
            align(Nearest, 500.0, (300.0, 450.0), port, false),
            350.0,
            "taller, before the start: its end edge aligns"
        );
        assert_eq!(
            align(Nearest, 0.0, (300.0, 350.0), (10.0, 90.0), false),
            260.0,
            "against the snapport"
        );
    }

    #[test]
    fn a_reversed_axis_starts_at_its_higher_edge() {
        let area = (200.0, 250.0);
        assert_eq!(align(Start, 0.0, area, (0.0, 100.0), true), 150.0);
        assert_eq!(align(End, 0.0, area, (0.0, 100.0), true), 200.0);
        assert_eq!(align(Center, 0.0, area, (0.0, 100.0), true), 175.0);
        assert_eq!(align(Nearest, 0.0, area, (0.0, 100.0), true), 150.0);
        assert_eq!(
            align(Start, 0.0, area, (0.0, 90.0), true),
            160.0,
            "the end-side padding is the start edge's"
        );
    }

    fn column(count: usize, css: &str) -> (Document<()>, NodeId, Vec<NodeId>) {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            &format!(
                "page {{ display: flex; width: 800px; height: 600px; }}
                 .scroller {{ display: flex; flex-direction: column; overflow: scroll;
                              width: 100px; height: 100px; flex-shrink: 0; }}
                 .item {{ flex-shrink: 0; width: 100px; height: 50px; }}
                 {css}"
            ),
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let scroller = document.create_element("view", ());
        document.add_class(scroller, "scroller");
        document.append_child(root, scroller);
        let items = (0..count)
            .map(|_| {
                let item = document.create_element("view", ());
                document.add_class(item, "item");
                document.append_child(scroller, item);
                item
            })
            .collect();
        document.commit();
        (document, scroller, items)
    }

    fn block(block: ScrollLogicalPosition) -> ScrollIntoViewOptions {
        ScrollIntoViewOptions {
            block,
            ..ScrollIntoViewOptions::default()
        }
    }

    #[test]
    fn the_defaults_are_the_drafts_with_behavior_resolved_to_instant() {
        assert_eq!(
            ScrollIntoViewOptions::default(),
            ScrollIntoViewOptions {
                behavior: ScrollBehavior::Instant,
                block: Start,
                inline: Nearest,
                container: ScrollIntoViewContainer::All,
            }
        );
    }

    #[test]
    fn each_block_position_on_a_vertical_scroller() {
        for (position, expected) in [
            (Start, 200.0),
            (End, 150.0),
            (Center, 175.0),
            (Nearest, 150.0),
        ] {
            let (mut document, scroller, items) = column(10, "");
            assert!(document.scroll_into_view(items[4], block(position)));
            assert_eq!(
                document.scroll_offset(scroller),
                Vector2D::new(0.0, expected),
                "{position:?}"
            );
        }
        let (mut document, scroller, items) = column(10, "");
        document.scroll_to(scroller, Vector2D::new(0.0, 180.0));
        assert!(document.scroll_into_view(items[4], block(Nearest)));
        assert_eq!(
            document.scroll_offset(scroller),
            Vector2D::new(0.0, 180.0),
            "a visible item moves nothing"
        );
        assert!(document.scroll_into_view(items[9], block(Start)));
        assert_eq!(
            document.scroll_offset(scroller),
            Vector2D::new(0.0, 400.0),
            "clamped to the range"
        );
    }

    #[test]
    fn inline_nearest_leaves_a_visible_element_alone_and_aligns_the_nearer_edge() {
        let (mut document, scroller, items) = column(
            3,
            ".scroller { flex-direction: row; } .item { width: 60px; height: 100px; }",
        );
        assert!(document.scroll_into_view(items[0], ScrollIntoViewOptions::default()));
        assert_eq!(document.scroll_offset(scroller), Vector2D::zero());
        assert!(document.scroll_into_view(items[1], ScrollIntoViewOptions::default()));
        assert_eq!(
            document.scroll_offset(scroller),
            Vector2D::new(20.0, 0.0),
            "item 1 (60..120) ends past the port: its end edge aligns"
        );
        document.scroll_to(scroller, Vector2D::new(80.0, 0.0));
        assert!(document.scroll_into_view(items[0], ScrollIntoViewOptions::default()));
        assert_eq!(
            document.scroll_offset(scroller),
            Vector2D::zero(),
            "item 0 starts before the port: its start edge aligns"
        );
    }

    #[test]
    fn scroll_margin_and_scroll_padding_apply() {
        let (mut document, scroller, items) =
            column(10, ".scroller { scroll-padding: 5px 0 15px; }");
        document.set_inline_style(items[4], "scroll-margin: 10px 0 20px");
        document.commit();
        assert!(document.scroll_into_view(items[4], block(Start)));
        assert_eq!(
            document.scroll_offset(scroller),
            Vector2D::new(0.0, 185.0),
            "200 less the 10px margin, less the 5px padding"
        );
        assert!(document.scroll_into_view(items[4], block(End)));
        assert_eq!(
            document.scroll_offset(scroller),
            Vector2D::new(0.0, 185.0),
            "250 plus the 20px margin, less the 85px snapport end"
        );
    }

    fn nested() -> (Document<()>, NodeId, NodeId, NodeId) {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .scroller { display: flex; flex-direction: column; overflow: scroll;
                         width: 100px; height: 100px; flex-shrink: 0; }
             .spacer { flex-shrink: 0; height: 300px; }
             .target { flex-shrink: 0; height: 50px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let element = |document: &mut Document<()>, parent: NodeId, class: &str| {
            let element = document.create_element("view", ());
            document.add_class(element, class);
            document.append_child(parent, element);
            element
        };
        let outer = element(&mut document, root, "scroller");
        element(&mut document, outer, "spacer");
        let inner = element(&mut document, outer, "scroller");
        element(&mut document, outer, "spacer");
        element(&mut document, inner, "spacer");
        let target = element(&mut document, inner, "target");
        element(&mut document, inner, "spacer");
        document.commit();
        (document, outer, inner, target)
    }

    #[test]
    fn container_nearest_scrolls_the_inner_scroller_alone() {
        let (mut document, outer, inner, target) = nested();
        let options = ScrollIntoViewOptions {
            container: ScrollIntoViewContainer::Nearest,
            ..ScrollIntoViewOptions::default()
        };
        assert!(document.scroll_into_view(target, options));
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 300.0));
        assert_eq!(document.scroll_offset(outer), Vector2D::zero());
        assert_eq!(document.pending_scroll_request(outer), None);
    }

    #[test]
    fn container_all_scrolls_both_and_the_outer_counts_the_inners_new_offset() {
        let (mut document, outer, inner, target) = nested();
        assert!(document.scroll_into_view(target, ScrollIntoViewOptions::default()));
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 300.0));
        assert_eq!(
            document.scroll_offset(outer),
            Vector2D::new(0.0, 300.0),
            "the target is at 300 + 300 unscrolled, less the inner's 300"
        );

        let (mut document, outer, inner, target) = nested();
        let smooth = ScrollIntoViewOptions {
            behavior: ScrollBehavior::Smooth,
            ..ScrollIntoViewOptions::default()
        };
        assert!(document.scroll_into_view(target, smooth));
        assert_eq!(document.scroll_offset(inner), Vector2D::zero());
        assert_eq!(document.scroll_offset(outer), Vector2D::zero());
        let frame = document.commit();
        let request = |id: NodeId| {
            let index = frame.slot_of(id).expect("a slot");
            frame.scroll_slots()[index as usize]
                .request
                .map(|request| request.target)
        };
        assert_eq!(request(inner), Some(Vector2D::new(0.0, 300.0)));
        assert_eq!(request(outer), Some(Vector2D::new(0.0, 300.0)));
    }

    #[test]
    fn an_instant_scroll_moves_the_document_and_a_smooth_one_only_requests() {
        let (mut document, scroller, items) = column(10, "");
        assert!(document.scroll_into_view(items[4], ScrollIntoViewOptions::default()));
        assert_eq!(document.scroll_offset(scroller), Vector2D::new(0.0, 200.0));
        assert!(document.pending_scroll_request(scroller).is_some());

        let (mut document, scroller, items) = column(10, "");
        let smooth = ScrollIntoViewOptions {
            behavior: ScrollBehavior::Smooth,
            ..ScrollIntoViewOptions::default()
        };
        assert!(document.scroll_into_view(items[4], smooth));
        assert_eq!(document.scroll_offset(scroller), Vector2D::zero());
        assert!(document.pending_scroll_request(scroller).is_some());
        assert!(document.needs_render(), "the request travels in a frame");
    }

    #[test]
    fn an_equal_position_records_no_request_unless_one_is_pending() {
        let (mut document, scroller, items) = column(10, "");
        assert!(!document.needs_render());
        assert!(document.scroll_into_view(items[0], ScrollIntoViewOptions::default()));
        assert_eq!(document.pending_scroll_request(scroller), None);
        assert!(!document.needs_render(), "nothing to carry");

        document.scroll_to_with(scroller, Vector2D::new(0.0, 100.0), ScrollBehavior::Smooth);
        let smooth = document.pending_scroll_request(scroller).expect("recorded");
        assert_eq!(document.scroll_offset(scroller), Vector2D::zero());
        assert!(document.scroll_into_view(items[0], ScrollIntoViewOptions::default()));
        let replaced = document.pending_scroll_request(scroller).expect("recorded");
        assert!(replaced > smooth, "the ongoing smooth scroll is replaced");
        let frame = document.commit();
        let index = frame.slot_of(scroller).expect("a slot");
        let request = frame.scroll_slots()[index as usize]
            .request
            .expect("carried");
        assert_eq!(
            (request.target, request.behavior),
            (Vector2D::zero(), ScrollBehavior::Instant)
        );
    }

    #[test]
    fn a_circular_axis_admits_a_position_past_either_end_of_the_range() {
        for (item, expected) in [(4, 175.0), (0, 225.0), (2, 75.0)] {
            let (mut document, scroller, items) =
                column(5, ".scroller { overscroll-behavior-y: circular; }");
            let scroll_box = document.scroll_box(scroller).expect("a scroll box");
            assert_eq!(scroll_box.wrap_period(), (None, Some(250.0)));
            assert!(document.scroll_into_view(items[item], block(Center)));
            assert_eq!(
                document.scroll_offset(scroller),
                Vector2D::new(0.0, expected),
                "item {item}"
            );
            let frame = document.commit();
            let index = frame.slot_of(scroller).expect("a slot");
            let request = frame.scroll_slots()[index as usize]
                .request
                .expect("carried");
            assert_eq!(request.target, Vector2D::new(0.0, expected), "item {item}");
        }
    }

    #[test]
    fn nothing_to_scroll_answers_false() {
        let (mut document, scroller, items) = column(3, ".loose { width: 10px; height: 10px; }");
        let root = document.document_element().id();
        let loose = document.create_element("view", ());
        document.add_class(loose, "loose");
        document.append_child(root, loose);
        document.commit();
        assert!(!document.scroll_into_view(loose, ScrollIntoViewOptions::default()));
        assert!(
            !document.scroll_into_view(root, ScrollIntoViewOptions::default()),
            "the root has nothing above it"
        );
        assert!(
            !document.scroll_into_view(scroller, ScrollIntoViewOptions::default()),
            "a scroller is not its own scrolling box"
        );

        document.set_inline_style(items[2], "display: none");
        document.commit();
        assert!(
            !document.scroll_into_view(items[2], ScrollIntoViewOptions::default()),
            "no box"
        );
        assert_eq!(document.pending_scroll_request(scroller), None);
    }
}
