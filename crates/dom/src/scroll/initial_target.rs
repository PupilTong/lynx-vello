//! css-scroll-snap-2 §3.1 `scroll-initial-target`: an element with `nearest`
//! sets the initial scroll position of its nearest scroll container.
//!
//! The paint build records every `nearest` element beside the scroll slot it
//! lives in ([`PaintOrder::initial_targets`]); after the build, the document
//! picks each container's target — the first in tree order when several
//! compete (§3.1.2) — and, when it is not the one the container last
//! honoured, scrolls to it as `scrollIntoView` would with `block: start`
//! and `inline: nearest` (§3.1.1), against the snapport (`scroll-padding`)
//! and the target's `scroll-margin`. The frame is then rebuilt so the
//! commit publishes the offset it settled on.
//!
//! A container honours each new target once, when it first becomes the
//! target: on the container's first layout, and again for a target that
//! arrives later (§3.1.4 says a user agent *should* still scroll then). The
//! "unless the user is no longer interested" escape is not modelled — the
//! document cannot tell a user scroll from a programmatic one — and a target
//! that stays the target never re-scrolls, so a user who scrolls away is not
//! dragged back on the next commit. The position snaps like any programmatic
//! scroll: the runtime's at-rest rule settles it on a snapping container.

use euclid::default::Vector2D;
use stylo::values::computed::Length;

use super::snap::scroll_padding;
use crate::NodeId;
use crate::tree::document::Document;
use crate::visual::PaintOrder;

/// One `scroll-initial-target: nearest` element the build found, with the
/// scroll slot it lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InitialTarget {
    pub(crate) chain: u32,
    pub(crate) node: NodeId,
}

/// The offset `scrollIntoView` chooses on one axis for `inline: nearest`
/// (CSSOM-View §"scroll an element into view"): unchanged when the area is
/// already within the visible range or covers all of it, else aligned to
/// the nearer edge.
fn nearest_edge(current: f32, area: (f32, f32), port: (f32, f32)) -> f32 {
    let (area_start, area_end) = area;
    let (visible_start, visible_end) = (port.0 + current, port.1 + current);
    if (area_start >= visible_start && area_end <= visible_end)
        || (area_start <= visible_start && area_end >= visible_end)
    {
        current
    } else if area_start < visible_start {
        area_start - port.0
    } else {
        area_end - port.1
    }
}

impl<T> Document<T> {
    /// Honours the initial scroll targets the build found; returns whether
    /// any container moved, in which case the frame must be rebuilt.
    pub(crate) fn scroll_to_initial_targets(&mut self, frame: &PaintOrder) -> bool {
        let targets = frame.initial_targets();
        if targets.is_empty() {
            return false;
        }
        let mut moved = false;
        let mut chains_done: Vec<u32> = Vec::new();
        for target in targets {
            if chains_done.contains(&target.chain) {
                continue;
            }
            chains_done.push(target.chain);
            let container = frame.slots()[target.chain as usize].node;
            let candidates = targets
                .iter()
                .filter(|other| other.chain == target.chain)
                .map(|other| other.node);
            let Some(chosen) = self.first_in_tree_order(container, candidates) else {
                continue;
            };
            if self.honoured_initial_target(container) == Some(chosen) {
                continue;
            }
            self.set_honoured_initial_target(container, Some(chosen));
            if let Some(position) = self.scroll_into_view_position(chosen, container) {
                let before = self.scroll_offset(container);
                moved |= self.scroll_to(container, position) != before;
            }
        }
        moved
    }

    /// Of `candidates` under `container`, the first in tree order — the
    /// only one when there is one, which is the common case and costs no
    /// walk.
    fn first_in_tree_order(
        &self,
        container: NodeId,
        candidates: impl Iterator<Item = NodeId>,
    ) -> Option<NodeId> {
        let mut candidates: Vec<NodeId> = candidates.collect();
        if candidates.len() <= 1 {
            return candidates.pop();
        }
        let mut pending: Vec<NodeId> = self
            .get(container)?
            .child_ids()
            .iter()
            .rev()
            .copied()
            .collect();
        while let Some(id) = pending.pop() {
            if candidates.contains(&id) {
                return Some(id);
            }
            if let Some(node) = self.get(id) {
                pending.extend(node.child_ids().iter().rev());
            }
        }
        None
    }

    /// Where `container` scrolls to bring `target` into view with
    /// `block: start`, `inline: nearest`, or `None` when either has no box.
    fn scroll_into_view_position(
        &self,
        target: NodeId,
        container: NodeId,
    ) -> Option<Vector2D<f32>> {
        let scroll_box = self.scroll_box(container)?;
        let style = self.get(container)?.layout_computed_style()?;
        let port = scroll_box.scrollport;
        let snapport_x = (
            scroll_padding(style.clone_scroll_padding_left(), port.width),
            port.width - scroll_padding(style.clone_scroll_padding_right(), port.width),
        );
        let snapport_y = (
            scroll_padding(style.clone_scroll_padding_top(), port.height),
            port.height - scroll_padding(style.clone_scroll_padding_bottom(), port.height),
        );
        let rect = self.rect_in_scroll_container(target, container)?;
        let values = self.get(target)?.layout_computed_style()?;
        let margin = |length: Length| length.px();
        let area_x = (
            rect.min_x() - margin(values.clone_scroll_margin_left()),
            rect.max_x() + margin(values.clone_scroll_margin_right()),
        );
        let area_y = (
            rect.min_y() - margin(values.clone_scroll_margin_top()),
            rect.max_y() + margin(values.clone_scroll_margin_bottom()),
        );
        Some(Vector2D::new(
            nearest_edge(scroll_box.offset.x, area_x, snapport_x),
            area_y.0 - snapport_y.0,
        ))
    }

    fn honoured_initial_target(&self, container: NodeId) -> Option<NodeId> {
        self.layout_state()
            .initial_targets
            .iter()
            .find(|(held_by, _)| *held_by == container)
            .map(|(_, target)| *target)
    }

    fn set_honoured_initial_target(&mut self, container: NodeId, target: Option<NodeId>) {
        // Containers freed since drop out here, on the rare write, rather
        // than on every release: a `NodeId` is never reissued, so a stale
        // entry is dead weight and never a wrong answer.
        let mut table = std::mem::take(&mut self.layout_state_mut().initial_targets);
        table.retain(|(held_by, _)| *held_by != container && self.slot(*held_by).is_some());
        if let Some(target) = target {
            table.push((container, target));
        }
        self.layout_state_mut().initial_targets = table;
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp, reason = "the expectations are exact offsets")]
mod tests {
    use super::*;
    use crate::StylesheetOrigin;
    use crate::tree::document::tests::device;

    #[test]
    fn nearest_edge_leaves_a_visible_area_alone_and_aligns_the_nearer_edge() {
        assert_eq!(nearest_edge(0.0, (20.0, 80.0), (0.0, 100.0)), 0.0);
        assert_eq!(
            nearest_edge(0.0, (-20.0, 180.0), (0.0, 100.0)),
            0.0,
            "covers the port"
        );
        assert_eq!(
            nearest_edge(0.0, (300.0, 400.0), (0.0, 100.0)),
            300.0,
            "to the right: end"
        );
        assert_eq!(
            nearest_edge(350.0, (300.0, 400.0), (0.0, 100.0)),
            300.0,
            "to the left: start"
        );
        assert_eq!(
            nearest_edge(0.0, (300.0, 400.0), (10.0, 90.0)),
            310.0,
            "against the snapport"
        );
    }

    fn paged(page_css: &str, count: usize) -> (Document<()>, NodeId, Vec<NodeId>) {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            &format!(
                "page {{ display: flex; width: 800px; height: 600px; }}
                 .scroller {{ display: flex; flex-direction: column; overflow: scroll;
                              width: 100px; height: 100px; }}
                 .page {{ flex-shrink: 0; width: 100px; height: 100px; {page_css} }}"
            ),
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let scroller = document.create_element("view", ());
        document.add_class(scroller, "scroller");
        document.append_child(root, scroller);
        let pages = (0..count)
            .map(|_| {
                let page = document.create_element("view", ());
                document.add_class(page, "page");
                document.append_child(scroller, page);
                page
            })
            .collect();
        (document, scroller, pages)
    }

    #[test]
    fn the_first_commit_starts_at_the_target_and_a_later_target_is_honoured_once() {
        let (mut document, scroller, pages) = paged("", 4);
        document.set_inline_style(pages[2], "scroll-initial-target: nearest");
        let frame = document.commit();
        assert_eq!(document.scroll_offset(scroller), Vector2D::new(0.0, 200.0));
        let slot = frame
            .scroll_slots()
            .iter()
            .find(|slot| slot.node == scroller)
            .expect("the scroller has a slot");
        assert_eq!(
            slot.offset.y, 200.0,
            "the published frame carries the position"
        );

        // The user (or anyone) scrolls away; the same target does not drag
        // the container back on the next commit.
        document.scroll_to(scroller, Vector2D::new(0.0, 50.0));
        document.set_inline_style(pages[0], "height: 120px");
        document.commit();
        assert_eq!(document.scroll_offset(scroller), Vector2D::new(0.0, 50.0));

        // A target arriving later is honoured, once: page 3 now starts at
        // 120 + 100 + 100.
        document.set_inline_style(pages[2], "");
        document.set_inline_style(pages[3], "scroll-initial-target: nearest");
        document.commit();
        assert_eq!(document.scroll_offset(scroller), Vector2D::new(0.0, 320.0));
    }

    #[test]
    fn the_first_in_tree_order_wins_and_padding_and_margin_apply() {
        let (mut document, scroller, pages) = paged("", 4);
        document.set_inline_style(pages[3], "scroll-initial-target: nearest");
        document.set_inline_style(
            pages[1],
            "scroll-initial-target: nearest; scroll-margin-top: 10px",
        );
        document.add_stylesheet(
            ".scroller { scroll-padding-top: 5px; }",
            StylesheetOrigin::Author,
        );
        document.commit();
        assert_eq!(
            document.scroll_offset(scroller),
            Vector2D::new(0.0, 85.0),
            "page 1 (100) less its 10px margin, less the 5px snapport inset",
        );
    }

    #[test]
    fn the_inline_axis_aligns_the_nearer_edge_only_when_needed() {
        let (mut document, scroller, pages) = paged("", 3);
        document.add_stylesheet(
            ".scroller { flex-direction: row; } .page { width: 150px; }",
            StylesheetOrigin::Author,
        );
        document.set_inline_style(pages[2], "scroll-initial-target: nearest");
        document.commit();
        assert_eq!(
            document.scroll_offset(scroller),
            Vector2D::new(350.0, 0.0),
            "the third page (300..450) is to the right, so its end edge aligns",
        );
    }
}
