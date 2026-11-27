//! The devtools node highlight: the content box, padding area, border area
//! and margin area of one element's box, each filled with its own colour,
//! recorded by the paint walk after every page item.

use crate::kurbo::{Affine, Rect};
use crate::paint::shape::{BoxShape, ring_path_into};
use crate::paint::walker::WalkSink;
use crate::paint::{BoxFragment, PathScratch, convert};
use crate::peniko::{Color, Fill};
use crate::tree::document::{Document, NodeId};
use crate::visual::{CornerRadii, PaintItemKind, PaintOrder};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeHighlight {
    pub node: NodeId,
    pub content: Color,
    pub padding: Color,
    pub border: Color,
    pub margin: Color,
}

impl<T> Document<T> {
    pub fn set_node_highlight(&mut self, highlight: Option<NodeHighlight>) {
        if self.node_highlight != highlight {
            self.node_highlight = highlight;
            self.note_visual_mutation();
        }
    }
}

pub(super) fn paint<T>(
    sink: &mut WalkSink<'_>,
    paths: &mut PathScratch,
    document: &Document<T>,
    frame: &PaintOrder,
    scale: Affine,
    highlight: &NodeHighlight,
) {
    let Some(item) = frame
        .items()
        .iter()
        .find(|item| item.node == highlight.node && matches!(item.kind, PaintItemKind::ElementBox))
    else {
        return;
    };
    let (Some(layout), Some(local)) = (
        document.rounded_layout(item.node),
        convert::item_affine(&frame.matrix(item.transform), item.size),
    ) else {
        return;
    };
    let fragment = BoxFragment::new(scale * local, item.size, CornerRadii::ZERO, layout);
    let margin = layout.margin.map(|width| f64::from(width.max(0.0)));
    let border_box = fragment.border_box;
    let margin_box = Rect::new(
        border_box.x0 - margin.left,
        border_box.y0 - margin.top,
        border_box.x1 + margin.right,
        border_box.y1 + margin.bottom,
    );
    let scene = sink.scene_for(item.space);
    if highlight.content.components[3] > 0.0 {
        scene.fill(
            Fill::NonZero,
            fragment.transform,
            highlight.content,
            None,
            &fragment.content_box,
        );
    }
    for (outer, inner, color) in [
        (
            fragment.padding_box,
            fragment.content_box,
            highlight.padding,
        ),
        (border_box, fragment.padding_box, highlight.border),
        (margin_box, border_box, highlight.margin),
    ] {
        if color.components[3] > 0.0 && outer != inner {
            ring_path_into(
                &mut paths.ring,
                &BoxShape::Rect(outer),
                &BoxShape::Rect(inner),
            );
            scene.fill(Fill::EvenOdd, fragment.transform, color, None, &paths.ring);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::queries::values::PrefersColorScheme;

    use super::NodeHighlight;
    use crate::kurbo::{Affine, BezPath, Rect};
    use crate::paint::compose::ComposeOp;
    use crate::paint::convert;
    use crate::paint::display_list::{DisplayList, Draw, Item, Paint, Shape};
    use crate::paint::shape::{BoxShape, ring_path_into};
    use crate::peniko::{Color, Fill};
    use crate::test_common::{Doc, device_with};
    use crate::tree::document::NodeId;
    use crate::visual::{CommittedFrame, PaintItem, PaintItemKind, SpaceKind};

    const CONTENT: Color = Color::from_rgba8(0, 0, 255, 128);
    const PADDING: Color = Color::from_rgba8(0, 255, 0, 128);
    const BORDER: Color = Color::from_rgba8(255, 255, 0, 128);
    const MARGIN: Color = Color::from_rgba8(255, 128, 0, 128);

    const CSS: &str = "page { display: flex; position: relative; width: 400px; height: 300px; }
        .box { position: absolute; left: 20px; top: 30px; width: 100px; height: 60px;
               box-sizing: border-box; padding: 5px 6px 7px 8px;
               border: 2px solid black; margin: 3px 4px 9px 10px;
               transform: translate(1px, 2px); background-color: red; }
        .over { position: absolute; z-index: 5; left: 0px; top: 0px;
                width: 400px; height: 300px; background-color: white; }
        .scroller { position: absolute; left: 0px; top: 0px; width: 200px; height: 100px;
                    display: flex; flex-direction: column; overflow: scroll; }
        .tall { flex-shrink: 0; width: 50px; height: 500px; }
        .hidden { display: none; }";

    fn page() -> Doc {
        let mut doc = Doc::with_device(device_with(400.0, 300.0, 2.0, PrefersColorScheme::Light));
        doc.add_css(CSS);
        doc
    }

    fn highlight(node: NodeId) -> NodeHighlight {
        NodeHighlight {
            node,
            content: CONTENT,
            padding: PADDING,
            border: BORDER,
            margin: MARGIN,
        }
    }

    fn element_box(frame: &CommittedFrame, node: NodeId) -> &PaintItem {
        frame
            .order
            .items()
            .iter()
            .find(|item| item.node == node && matches!(item.kind, PaintItemKind::ElementBox))
            .expect("the element has a box item")
    }

    fn last_fragment(frame: &CommittedFrame) -> (Option<u32>, &DisplayList) {
        let Some(&ComposeOp::Fragment { index, space }) = frame.presentation.program.last() else {
            panic!("the program ends with a fragment");
        };
        (space, &frame.presentation.fragments[index as usize])
    }

    fn solid_fills(list: &DisplayList) -> Vec<(&Draw, Color)> {
        list.items()
            .iter()
            .filter_map(|item| match item {
                Item::Fill(draw) => match draw.paint {
                    Paint::Solid(color) => Some((draw, color)),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn highlight_fills(frame: &CommittedFrame) -> usize {
        frame
            .presentation
            .fragments
            .iter()
            .flat_map(solid_fills)
            .filter(|(_, color)| [CONTENT, PADDING, BORDER, MARGIN].contains(color))
            .count()
    }

    fn ring(outer: Rect, inner: Rect) -> BezPath {
        let mut path = BezPath::new();
        ring_path_into(&mut path, &BoxShape::Rect(outer), &BoxShape::Rect(inner));
        path
    }

    fn assert_same_output(left: &CommittedFrame, right: &CommittedFrame) {
        let (a, b) = (&left.presentation, &right.presentation);
        assert_eq!(a.fragments.len(), b.fragments.len());
        for (left, right) in a.fragments.iter().zip(&b.fragments) {
            crate::paint::equivalence::assert_scenes_identical(left, right);
        }
        assert_eq!(a.program.len(), b.program.len());
        for (left, right) in a.program.iter().zip(&b.program) {
            assert_eq!(std::mem::discriminant(left), std::mem::discriminant(right));
            assert_eq!(left.space(&a.filter_groups), right.space(&b.filter_groups));
        }
        assert_eq!(left.paints_content, right.paints_content);
    }

    #[test]
    fn the_four_areas_are_the_last_fills_of_the_box_space_under_the_box_transform() {
        let mut doc = page();
        let target = doc.el(doc.root, "view.box");
        doc.el(doc.root, "view.over");
        doc.dom.set_node_highlight(Some(highlight(target)));
        let frame = doc.dom.commit();

        let item = element_box(&frame, target);
        let (space, list) = last_fragment(&frame);
        assert_eq!(space, item.space);
        assert!(matches!(list.items().last(), Some(Item::Fill(_))));
        let fills = solid_fills(list);
        let [.., content, padding, border, margin] = fills.as_slice() else {
            panic!("four fills: {fills:?}");
        };
        let local = convert::item_affine(&frame.order.matrix(item.transform), item.size)
            .expect("an invertible box matrix");
        assert_eq!(
            local.translation(),
            crate::kurbo::Vec2::new(31.0, 35.0),
            "the offset, margin-left and margin-top, and the translate",
        );
        let transform = Affine::scale(2.0) * local;

        let content_box = Rect::new(10.0, 7.0, 92.0, 51.0);
        let padding_box = Rect::new(2.0, 2.0, 98.0, 58.0);
        let border_box = Rect::new(0.0, 0.0, 100.0, 60.0);
        let margin_box = Rect::new(-10.0, -3.0, 104.0, 69.0);
        assert_eq!(content.1, CONTENT);
        assert_eq!(content.0.rule, Fill::NonZero);
        assert!(matches!(content.0.shape, Shape::Rect(rect) if rect == content_box));
        for ((draw, color), expected, outer, inner) in [
            (padding, PADDING, padding_box, content_box),
            (border, BORDER, border_box, padding_box),
            (margin, MARGIN, margin_box, border_box),
        ] {
            assert_eq!(*color, expected);
            assert_eq!(draw.rule, Fill::EvenOdd);
            let Shape::Path { start, end } = draw.shape else {
                panic!("a ring path: {draw:?}");
            };
            assert_eq!(list.path(start, end), ring(outer, inner).elements());
        }
        for (draw, _) in [content, padding, border, margin] {
            assert_eq!(list.transform(draw.transform), transform);
        }
    }

    #[test]
    fn a_box_inside_a_scroll_container_is_highlighted_in_its_content_space() {
        let mut doc = page();
        let scroller = doc.el(doc.root, "view.scroller");
        let target = doc.el(scroller, "view.tall");
        doc.dom.set_node_highlight(Some(highlight(target)));
        let frame = doc.dom.commit();

        let item = element_box(&frame, target);
        let (space, list) = last_fragment(&frame);
        assert_eq!(space, item.space);
        let space = space.expect("not the root space");
        let SpaceKind::Scroll(slot) = frame.order.spaces()[space as usize].kind else {
            panic!("a scroll space");
        };
        assert_eq!(frame.scroll_slots()[slot as usize].node, scroller);
        assert_eq!(solid_fills(list).len(), 1, "the content box alone has area");
    }

    #[test]
    fn a_transparent_area_records_no_fill() {
        let mut doc = page();
        let target = doc.el(doc.root, "view.box");
        doc.dom.set_node_highlight(Some(NodeHighlight {
            border: Color::TRANSPARENT,
            ..highlight(target)
        }));
        let frame = doc.dom.commit();
        let (_, list) = last_fragment(&frame);
        let colors: Vec<Color> = solid_fills(list).iter().map(|(_, color)| *color).collect();
        assert_eq!(colors[colors.len() - 3..], [CONTENT, PADDING, MARGIN]);
        assert_eq!(highlight_fills(&frame), 3);
    }

    #[test]
    fn setting_changing_and_removing_need_a_render_and_the_same_value_does_not() {
        let mut doc = page();
        let target = doc.el(doc.root, "view.box");
        let plain = doc.dom.commit();

        doc.dom.set_node_highlight(Some(highlight(target)));
        assert!(doc.dom.needs_render());
        let highlighted = doc.dom.commit();
        assert_eq!(highlight_fills(&highlighted), 4);
        assert_eq!(highlighted.paints_content, plain.paints_content);

        doc.dom.set_node_highlight(Some(highlight(target)));
        assert!(!doc.dom.needs_render());

        doc.dom.set_node_highlight(Some(NodeHighlight {
            margin: CONTENT,
            ..highlight(target)
        }));
        assert!(doc.dom.needs_render());
        doc.dom.commit();

        doc.dom.set_node_highlight(None);
        assert!(doc.dom.needs_render());
        let removed = doc.dom.commit();
        assert_eq!(highlight_fills(&removed), 0);
        assert_same_output(&plain, &removed);

        doc.dom.set_node_highlight(None);
        assert!(!doc.dom.needs_render());
    }

    #[test]
    fn a_stale_id_a_text_node_and_an_undisplayed_element_draw_nothing() {
        let mut doc = page();
        doc.el(doc.root, "view.box");
        let text = doc.dom.create_text_node("text", ());
        doc.dom.append_child(doc.root, text);
        let hidden = doc.el(doc.root, "view.box.hidden");
        let stale = doc.el(doc.root, "view.box");
        doc.dom.remove_element(stale);
        doc.dom.drop_element(stale);
        let plain = doc.dom.commit();

        for node in [stale, text, hidden] {
            doc.dom.set_node_highlight(Some(highlight(node)));
            let frame = doc.dom.commit();
            assert_eq!(highlight_fills(&frame), 0);
            assert_same_output(&plain, &frame);
        }
    }

    #[test]
    fn a_point_in_the_highlight_hits_the_page_under_it() {
        let mut doc = page();
        let target = doc.el(doc.root, "view.box");
        doc.dom.set_node_highlight(Some(highlight(target)));
        let frame = doc.dom.commit();
        assert_eq!(highlight_fills(&frame), 4);
        assert_eq!(
            doc.dom
                .elements_from_point(euclid::default::Point2D::new(22.0, 35.0))
                .first(),
            Some(&doc.root),
            "a point in the margin area",
        );
    }
}
