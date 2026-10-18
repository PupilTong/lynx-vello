//! A block container: one element, its inline-level content laid out as a
//! CSS inline formatting context.
//!
//! The `(block, flow)` display ([`DisplayMode::BlockFlow`]) is a block
//! container: `display: block`, or an `inline` element no inline formatting
//! context around it took in. css-display-3 §2.7 blockifies an `inline`
//! child of a flex, grid, grid-lanes or linear container (the fork's
//! `Display::is_item_container`). A relative container is not one of them,
//! so an `inline` child of one keeps `inline`, is no inline box (its box
//! parent is not a block container), and lays its own content out here.
//! The `lynx` grammar parses `block` and `inline` only in the engine's own
//! sheets; the producers are the anonymous block box CSS wraps around loose
//! text in a flex, grid, grid-lanes, linear or relative container (CSS 2.2
//! §9.2.1.1, css-flexbox-1 §4, css-grid-1 §6) and the HTML content the engine
//! builds. Its content is laid out by [`hughie::text::inline`].
//!
//! # The content walk
//!
//! By computed display, over the block's children in flat tree order:
//!
//! | node | role |
//! |---|---|
//! | text node | a run, carrying its flat parent's style |
//! | `contents` with a string `content` list | one run of the generated text, in its own style |
//! | `contents` without one | walked through: its children are the block's |
//! | `inline`, not replaced | an inline box ([`is_inline_box`]): its string `content` list, else its children, between its two boundaries |
//! | anything else | not content: hidden |
//!
//! The last row covers `none`, an out-of-flow box, an inline-level replaced
//! element and every block-level box — `block`, `flex`, `grid`,
//! `grid-lanes`, `linear`, `relative`, a Lynx text element. None of them is
//! laid out in the context: the block hides each, as it did when its content
//! was text runs only.
//!
//! The runs reach the paragraph through the same pieces the Lynx block uses
//! (`crate::layout::paragraph`). What differs is the walk, the parameters —
//! CSS properties only — and the inline boxes.
//!
//! # Inline boxes outside layout
//!
//! An inline box generates no layout box: its slot is hidden. Its geometry is the
//! paragraph's fragments ([`hughie::text::inline::InlineFragment`]), which
//! the painter draws its background and border from, the hit test answers
//! it from, and `Document::bounding_client_rect` unions. The paint-order
//! build emits no record of its own for it ([`is_inline_box`]), and it clips
//! nothing: `overflow` and paint containment do not apply to a non-atomic
//! inline box (css-overflow-3, css-contain-2 §3.3).

use std::hash::Hasher;

use hughie::geometry::Edges;
use hughie::style::CoreStyle;
use hughie::text::inline::{
    InlineBoxOpen, InlineEdges, InlineFlow, InlineFlowItem, InlineFlowStyle, InlineRun,
};
use hughie::text::paragraph::{BreakStyle, Direction, OverflowWrap, RunStyle};
use hughie::tree::LayoutTree;
use stylo::properties::ComputedValues;

use crate::layout::paragraph::{
    Collected, OwnedItem, TextStore, collect_generated, context_and_store, hash_flow,
    run_white_space,
};
use crate::layout::style::{DisplayMode, StyleView, TextRunView, box_parent};
use crate::tree::document::{DocumentLayoutState, NodeId, NodeSlot, TreeArenas};
use crate::tree::node::Node;

/// One inline box's used border and padding, physical, for the painter.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InlineBoxInsets {
    /// The box's `Open` item.
    pub(crate) item: u32,
    pub(crate) border: Edges<f32>,
    pub(crate) padding: Edges<f32>,
}

/// What one item was built from, compared rather than hashed: a missed
/// rebuild would paint glyphs shaped at an old font, or an edge at an old
/// width.
#[derive(Debug, Clone, PartialEq)]
enum ItemKey {
    Run(RunStyle, BreakStyle),
    Open(RunStyle, BreakStyle, Direction, InlineEdges),
    Close,
}

/// One block container's inline formatting context, plus the table that maps
/// it back to the DOM.
pub(crate) struct InlineFlowStore {
    pub(crate) flow: InlineFlow,
    /// Item index → the node behind it: a run's element (the paint identity
    /// `InlineFlow::source_of` names), an inline box for both its boundaries.
    pub(crate) source_ids: Vec<NodeId>,
    /// Each inline box's border and padding, in open order.
    pub(crate) insets: Vec<InlineBoxInsets>,
    /// What the flow was built from, compared rather than hashed for style,
    /// as the Lynx block's store does.
    fingerprint: u64,
    style: InlineFlowStyle,
    keys: Vec<ItemKey>,
    /// How many times this element's flow has been shaped from scratch.
    pub(crate) rebuilds: u32,
}

impl InlineFlowStore {
    /// The flow, if a commit has produced one.
    pub(crate) fn committed(&self) -> Option<&InlineFlow> {
        self.flow.has_committed().then_some(&self.flow)
    }
}

/// Whether `node` is a non-atomic inline box of the inline formatting context
/// around it (CSS 2.2 §9.2.2): an element whose display is `inline`, which is
/// not replaced and no Lynx text element, and whose box parent lays its
/// content out in line boxes — a block container, or an inline box itself.
///
/// The same element under a relative container or inside a Lynx paragraph is
/// no inline box: it lays its own content out, as a block container or as an
/// atom of that paragraph.
pub(crate) fn is_inline_box<T>(node: &Node<T>) -> bool {
    use hughie::style::Display;
    if !node.is_element() || node.is_replaced() || node.is_lynx_text() {
        return false;
    }
    let Some(style) = node.layout_computed_style() else {
        return false;
    };
    *style.get_display() == Display::Inline
        && box_parent(node).is_some_and(|parent| {
            !parent.is_replaced()
                && StyleView::try_of(parent)
                    .is_some_and(|view| view.display_mode() == DisplayMode::BlockFlow)
        })
}

/// What one child of the content walk is.
enum Role {
    Hidden,
    Contents,
    InlineBox,
}

fn role_of<T>(node: &Node<T>, view: &StyleView<'_, T>) -> Role {
    if crate::layout::text_block::out_of_flow(view.position()) {
        return Role::Hidden;
    }
    match view.display_mode() {
        DisplayMode::Contents => Role::Contents,
        DisplayMode::BlockFlow
            if *view.values().get_display() == hughie::style::Display::Inline
                && !node.is_replaced() =>
        {
            Role::InlineBox
        }
        // `none`, and every box that is no inline box: no atomic inline box
        // is laid out in this context.
        _ => Role::Hidden,
    }
}

/// One step of the content walk: a node to visit, or the end of the inline
/// box a node opened.
enum Visit {
    Node(NodeSlot),
    Close(NodeSlot),
}

/// The block's content, in order: runs and inline box boundaries.
///
/// Iterative, like the Lynx block's walk, because nesting depth is
/// author-controlled. `basis` is what an inline box's percentage margin and
/// padding resolve against.
fn collect_flow<T>(tree: &TreeArenas<T>, element: NodeSlot, basis: Option<f32>) -> Vec<Collected> {
    let mut collected = Vec::new();
    if collect_generated(tree, element, &mut collected) {
        return collected;
    }
    let mut stack: Vec<Visit> = tree.children(element).map(Visit::Node).collect();
    stack.reverse();
    while let Some(visit) = stack.pop() {
        let slot = match visit {
            Visit::Node(slot) => slot,
            Visit::Close(slot) => {
                collected.push(Collected {
                    item: OwnedItem::Close,
                    source: slot,
                });
                continue;
            }
        };
        let node = tree.at(slot);
        if node.is_text_node() {
            let Some(text) = node.text() else { continue };
            if text.is_empty() {
                continue;
            }
            collected.push(Collected {
                item: OwnedItem::Run {
                    text: text.to_owned(),
                    style: RunStyle::from_run_style(&TextRunView::of(node)),
                    white_space: run_white_space(node),
                },
                // The element the run's style came from: its flat parent.
                source: node
                    .flat_parent()
                    .map_or(slot, |parent| tree.slot(parent.id()).unwrap_or(slot)),
            });
            continue;
        }
        if !node.is_element() {
            continue;
        }
        let view = StyleView::of(node);
        match role_of(node, &view) {
            Role::Hidden => {}
            Role::Contents => {
                if !collect_generated(tree, slot, &mut collected) {
                    let first = stack.len();
                    stack.extend(tree.children(slot).map(Visit::Node));
                    stack[first..].reverse();
                }
            }
            Role::InlineBox => {
                collected.push(Collected {
                    item: open_of(&view, basis),
                    source: slot,
                });
                // A string `content` list is the box's content, as `br`'s
                // `"\A"` is; otherwise its children are.
                if collect_generated(tree, slot, &mut collected) {
                    collected.push(Collected {
                        item: OwnedItem::Close,
                        source: slot,
                    });
                } else {
                    stack.push(Visit::Close(slot));
                    let first = stack.len();
                    stack.extend(tree.children(slot).map(Visit::Node));
                    stack[first..].reverse();
                }
            }
        }
    }
    collected
}

/// An inline box's start: its font, line breaking, direction, and its used
/// edges with percentages resolved against `basis` (CSS 2.2 §8.3, §8.4: the
/// containing block's width). `auto` margins are zero on an inline box
/// (§10.3.1).
fn open_of<T>(view: &StyleView<'_, T>, basis: Option<f32>) -> OwnedItem {
    let (margin, border, padding) = used_insets(view, basis);
    let direction: Direction = view.direction().into();
    let (start, end) = match direction {
        Direction::Ltr => (
            (margin.left, border.left + padding.left),
            (border.right + padding.right, margin.right),
        ),
        Direction::Rtl => (
            (margin.right, border.right + padding.right),
            (border.left + padding.left, margin.left),
        ),
    };
    OwnedItem::Open {
        style: RunStyle::from_run_style(&TextRunView::from_values(view.values())),
        breaking: breaking_of(view.values()),
        direction,
        edges: InlineEdges {
            start_margin: start.0,
            start_inset: start.1,
            end_inset: end.0,
            end_margin: end.1,
            top_inset: border.top + padding.top,
            bottom_inset: border.bottom + padding.bottom,
        },
    }
}

/// An inline box's used margin, border and padding.
fn used_insets<T>(
    view: &StyleView<'_, T>,
    basis: Option<f32>,
) -> (Edges<f32>, Edges<f32>, Edges<f32>) {
    (
        hughie::compute::used_margins(view, basis),
        hughie::compute::used_border(view),
        hughie::compute::used_padding(view, basis),
    )
}

/// A run's line-breaking values, from the computed style it carries.
fn breaking_of(style: &ComputedValues) -> BreakStyle {
    let text = style.get_inherited_text();
    BreakStyle {
        word_break: (*text.get_word_break()).into(),
        text_wrap: (*text.get_text_wrap_mode()).into(),
        // `overflow-wrap` is not compiled into the `lynx` engine at all
        // (`vendor/stylo/style/tests/lynx_disabled_properties.rs`), so every
        // element computes its initial value: a word wider than its line
        // overflows it.
        overflow_wrap: OverflowWrap::Normal,
    }
}

/// The block's own parameters, from its computed style.
fn flow_style<T>(tree: &TreeArenas<T>, element: NodeSlot) -> InlineFlowStyle {
    let view = StyleView::of(tree.at(element));
    InlineFlowStyle::from_container_style(&view, &TextRunView::from_values(view.values()))
}

/// Rebuilds `element`'s flow when its content or the values it is shaped
/// with moved, otherwise hands it the block's current parameters.
fn refresh<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    basis: Option<f32>,
) {
    let collected = collect_flow(tree, element, basis);
    let stamp = {
        let mut hasher = rustc_hash::FxHasher::default();
        hash_flow(&collected, &mut hasher);
        hasher.finish()
    };
    let style = flow_style(tree, element);
    let keys = keys_of(tree, &collected);

    // The strut is parley's style 0 and the direction its bidi paragraph
    // level, both read when the flow is shaped, so a change to either is a
    // new flow, like an item change.
    let needs_build = state
        .text_store(element)
        .and_then(TextStore::as_flow)
        .is_none_or(|store| {
            store.fingerprint != stamp
                || store.keys != keys
                || store.style.strut != style.strut
                || store.style.direction != style.direction
        });

    if needs_build {
        let items = items_of(&collected, &keys);
        let source_ids = collected
            .iter()
            .map(|entry| tree.at(entry.source).id())
            .collect();
        let insets = collected
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry.item, OwnedItem::Open { .. }))
            .map(|(index, entry)| {
                let view = StyleView::of(tree.at(entry.source));
                let (_, border, padding) = used_insets(&view, basis);
                InlineBoxInsets {
                    item: u32::try_from(index).expect("item count fits u32"),
                    border,
                    padding,
                }
            })
            .collect();
        let (context, slot) = state.text_store_parts(element);
        let rebuilds = slot.as_deref().map_or(0, |store| store.rebuilds() + 1);
        let flow = InlineFlow::new(context, style.clone(), &items);
        *slot = Some(Box::new(TextStore::Flow(InlineFlowStore {
            flow,
            source_ids,
            insets,
            fingerprint: stamp,
            style,
            keys,
            rebuilds,
        })));
    } else if let Some((_, store)) = context_and_store(state, element)
        && let Some(store) = store.as_flow_mut()
    {
        // Same content, possibly a different alignment or indent keyword.
        store.flow.set_style(style.clone());
        store.style = style;
    }
}

/// What each collected item was built from.
fn keys_of<T>(tree: &TreeArenas<T>, collected: &[Collected]) -> Vec<ItemKey> {
    collected
        .iter()
        .map(|entry| match &entry.item {
            OwnedItem::Run { style, .. } => {
                let breaking = tree
                    .at(entry.source)
                    .layout_computed_style()
                    .map_or_else(BreakStyle::default, breaking_of);
                ItemKey::Run(style.clone(), breaking)
            }
            OwnedItem::Open {
                style,
                breaking,
                direction,
                edges,
            } => ItemKey::Open(style.clone(), *breaking, *direction, *edges),
            OwnedItem::Close => ItemKey::Close,
            OwnedItem::Atom { .. } => unreachable!("the content walk collects no atom"),
        })
        .collect()
}

/// Borrows the collected items, with the run line breaking `keys` read, as
/// the flow's input vocabulary.
fn items_of<'a>(collected: &'a [Collected], keys: &'a [ItemKey]) -> Vec<InlineFlowItem<'a>> {
    collected
        .iter()
        .zip(keys)
        .map(|(entry, key)| match (&entry.item, key) {
            (
                OwnedItem::Run {
                    text, white_space, ..
                },
                ItemKey::Run(run_style, breaking),
            ) => InlineFlowItem::Run(InlineRun {
                text,
                style: run_style,
                white_space: *white_space,
                breaking: *breaking,
            }),
            (
                OwnedItem::Open {
                    style,
                    breaking,
                    direction,
                    edges,
                },
                _,
            ) => InlineFlowItem::Open(InlineBoxOpen {
                style,
                breaking: *breaking,
                direction: *direction,
                edges: *edges,
            }),
            _ => InlineFlowItem::Close,
        })
        .collect()
}

/// What an inline box's percentage margins and padding resolve against: the
/// block's content-box width, as far as this call knows it — its own width
/// when the input fixes one, else the width its parent offers — less its own
/// horizontal border and padding.
///
/// The edges are shaping inputs (`hughie::text::inline`'s edge clusters), so a box
/// with a percentage edge re-shapes the paragraph when this moves.
fn percentage_basis<T>(view: &StyleView<'_, T>, input: &hughie::tree::LayoutInput) -> Option<f32> {
    let width = input.known_dimensions.width.or(input.parent_size.width)?;
    let border = hughie::compute::used_border(view);
    let padding = hughie::compute::used_padding(view, input.parent_size.width);
    Some((width - border.horizontal_sum() - padding.horizontal_sum()).max(0.0))
}

/// Lays `element` out as a block container whose content is one inline
/// formatting context.
///
/// Refresh the flow, then probe or commit it inside hughie's leaf box
/// wrapper, and on a commit hide every child: no node of the content
/// generates a box of its own.
pub(crate) fn compute_inline_flow_layout<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    input: hughie::tree::LayoutInput,
) -> hughie::tree::LayoutOutput {
    use hughie::compute::compute_text_block_layout as compute_block_box;

    let view = StyleView::of(tree.at(element));
    let percentages = percentage_basis(&view, &input);
    refresh(tree, state, element, percentages);

    let basis = input.known_dimensions.width.or(input.parent_size.width);
    let output = compute_block_box(input, &view, |measure_input| {
        crate::layout::paragraph::measure(tree, state, element, basis, measure_input)
    });
    if input.goal.commits() {
        let children: Vec<NodeSlot> = tree.children(element).collect();
        for child in children {
            hughie::compute::hide_subtree(tree, state, child);
        }
    } else {
        state.note_probed_text(element);
    }
    output
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use hughie::geometry::Size;

    use crate::tree::document::{Document, NodeId};
    use crate::visual::PaintItemKind;
    use crate::{FontBlob, Point2D, StylesheetOrigin};

    const AHEM: &[u8] = include_bytes!("../../../hughie/tests/fixtures/Ahem.ttf");

    impl<T: Sync> Document<T> {
        /// Gives `id` the style of an anonymous block box under its flat
        /// parent: every inherited property from the parent, every other one
        /// initial (CSS 2.2 §9.2.1.1), and the internal block container
        /// display.
        ///
        /// The producer's own style ([`Document::anonymous_block_style`]) on
        /// an ordinary element, so these tests reach the inline formatting
        /// context without the loose-text grouping in front of it
        /// (`tree::anonymous`). It writes through `replace_pseudo_style`, the
        /// writer for engine-built styles, so it survives every flush that
        /// does not restyle `id` — none of these tests restyles it.
        pub(crate) fn make_anonymous_block(&mut self, id: NodeId) {
            let style = {
                let parent = self
                    .get(id)
                    .and_then(crate::tree::node::Node::flat_parent)
                    .and_then(crate::tree::node::Node::layout_computed_style)
                    .expect("an anonymous block's parent is styled");
                self.anonymous_block_style(parent)
            };
            self.arenas_mut()
                .get_mut(id)
                .expect("a live element")
                .replace_pseudo_style(style);
            // The record the style harvest leaves for a changed `display`,
            // which this write bypasses: loose text the element boxed as a
            // flex container is its paragraph's content now.
            self.note_loose_text_change(id);
            self.invalidate_layout(id);
        }
    }

    /// A page of `css` with one anonymous block holding `text`, laid out.
    fn page(css: &str, text: &str) -> (Document<()>, NodeId, NodeId) {
        let mut doc = Document::new(crate::tree::document::tests::device(), "page", ());
        doc.register_fonts(FontBlob::from_static(AHEM));
        doc.add_stylesheet(css, StylesheetOrigin::Author);
        let block = doc.create_element("anonymous", ());
        doc.append_child(doc.document_element().id(), block);
        let node = doc.create_text_node(text, ());
        doc.append_child(block, node);
        doc.layout();
        doc.make_anonymous_block(block);
        doc.layout();
        (doc, block, node)
    }

    fn size(doc: &Document<()>, id: NodeId) -> Size<f32> {
        let layout = doc.rounded_layout(id).expect("laid out");
        Size::new(layout.size.width, layout.size.height)
    }

    const COLUMN: &str = "page { display: flex; flex-direction: column; align-items: flex-start;
                                 width: 60px; height: 200px;
                                 font-family: Ahem; font-size: 10px; }";

    #[test]
    fn an_anonymous_block_lays_its_text_out_as_one_inline_formatting_context() {
        let (doc, block, _) = page(COLUMN, "aaaa bbbb");
        let flow = doc.inline_flow(block).expect("a committed flow");
        assert_eq!(flow.line_geometry().len(), 2);
        // Not stretched, so its width is the fit-content size: the 60px
        // offered, not the 40px widest line a Lynx `<text>` would take.
        assert_eq!(size(&doc, block), Size::new(60.0, 20.0));
    }

    #[test]
    fn the_strut_is_the_blocks_inherited_font() {
        // The empty text node `page` adds is no run.
        let (mut doc, block, _) = page(
            "page { display: flex; align-items: flex-start; width: 200px; height: 200px;
                    font-family: Ahem; font-size: 20px; }
             small { display: contents; font-size: 10px; }",
            "",
        );
        let small = doc.create_element("small", ());
        doc.append_child(block, small);
        let run = doc.create_text_node("aa", ());
        doc.append_child(small, run);
        doc.layout();
        // 10px glyphs on a 20px line box, on the block's 20px baseline.
        let flow = doc.inline_flow(block).expect("a committed flow");
        let [line] = flow.line_geometry() else {
            panic!("one line expected")
        };
        assert_eq!((line.height, line.baseline), (20.0, 16.0));
        assert_eq!(size(&doc, block), Size::new(20.0, 20.0));
    }

    #[test]
    fn the_block_paints_one_text_record_and_hits_as_itself() {
        let (mut doc, block, text) = page(COLUMN, "aaaa bbbb");
        let paint = doc.build_paint_order();
        assert!(
            paint.items().iter().all(|item| item.node != text),
            "a text node paints no item of its own"
        );
        let item = paint
            .items()
            .iter()
            .find(|item| item.kind == PaintItemKind::TextRun { element: block })
            .expect("the block paints one paragraph item");
        assert_eq!(item.size.width, 60.0);
        doc.render();
        assert_eq!(
            doc.elements_from_point(Point2D::new(5.0, 5.0)).first(),
            Some(&block)
        );
    }

    #[test]
    fn a_text_change_re_lays_the_block_out() {
        let (mut doc, block, text) = page(COLUMN, "aaaa bbbb");
        doc.set_text_node_data(text, "aaaa bbbb cccc");
        doc.layout();
        let flow = doc.inline_flow(block).expect("a committed flow");
        assert_eq!(flow.line_geometry().len(), 3);
        assert_eq!(size(&doc, block), Size::new(60.0, 30.0));
        assert_eq!(doc.text_block_rebuilds(block), Some(1));
    }

    #[test]
    fn whitespace_only_text_makes_no_line_box() {
        let (doc, block, _) = page(COLUMN, "  \n  ");
        assert!(
            doc.inline_flow(block)
                .expect("a flow")
                .line_geometry()
                .is_empty()
        );
        assert_eq!(size(&doc, block).height, 0.0);
    }

    #[test]
    fn contents_scopes_are_walked_and_box_generating_children_hidden() {
        let (mut doc, block, _) = page(
            &format!(
                "{COLUMN} span {{ display: contents; }} label {{ display: contents; content: 'cc'; }}
                 view {{ display: flex; width: 30px; height: 30px; }}"
            ),
            "aa",
        );
        let span = doc.create_element("span", ());
        doc.append_child(block, span);
        let inner = doc.create_text_node(" bb", ());
        doc.append_child(span, inner);
        let label = doc.create_element("label", ());
        doc.append_child(block, label);
        let replaced = doc.create_text_node("never shown", ());
        doc.append_child(label, replaced);
        let view = doc.create_element("view", ());
        doc.append_child(block, view);
        doc.layout();
        // "aa bbcc": the span's text joins the run, the label's generated
        // text replaces its child, the view is no content.
        assert_eq!(doc.inline_flow(block).expect("a flow").size().width, 60.0);
        assert_eq!(size(&doc, view), Size::new(0.0, 0.0));
    }

    /// A page whose UA sheet writes flow layout, as an engine sheet does: `p`
    /// a block container, `span` and `a` inline boxes, `view` a box that is
    /// no inline box. The Ahem page is `width` wide; `css` adds author rules.
    fn flow_page(width: f32, css: &str) -> (Document<()>, NodeId) {
        let mut doc = Document::new(crate::tree::document::tests::device(), "page", ());
        doc.register_fonts(FontBlob::from_static(AHEM));
        doc.add_stylesheet(
            &format!(
                "page {{ display: flex; flex-direction: column; align-items: flex-start;
                         width: {width}px; height: 300px; font-family: Ahem; font-size: 10px; }}
                 p {{ display: block; }}
                 span, a {{ display: inline; }}
                 view {{ display: flex; width: 20px; height: 20px; }}"
            ),
            StylesheetOrigin::UserAgent,
        );
        doc.add_stylesheet(css, StylesheetOrigin::Author);
        let p = doc.create_element("p", ());
        doc.append_child(doc.document_element().id(), p);
        (doc, p)
    }

    fn text(doc: &mut Document<()>, parent: NodeId, data: &str) -> NodeId {
        let node = doc.create_text_node(data, ());
        doc.append_child(parent, node);
        node
    }

    fn element(doc: &mut Document<()>, parent: NodeId, tag: &str) -> NodeId {
        let node = doc.create_element(tag, ());
        doc.append_child(parent, node);
        node
    }

    /// `(left, top, width, height)` of every fragment of `item`'s box.
    fn fragment_rects(doc: &Document<()>, p: NodeId, item: u32) -> Vec<(f32, f32, f32, f32)> {
        doc.inline_flow(p)
            .expect("a committed flow")
            .fragments()
            .iter()
            .filter(|fragment| fragment.item == item)
            .map(|fragment| {
                (
                    fragment.rect.origin.x,
                    fragment.rect.origin.y,
                    fragment.rect.size.width,
                    fragment.rect.size.height,
                )
            })
            .collect()
    }

    #[test]
    fn a_block_container_lays_out_inline_boxes() {
        // aa <span>bb</span> cc <view>: the span's start edge is a 2px
        // border and 5px of padding, its end edge 5px of padding; the view is
        // no inline box, and no content.
        let (mut doc, p) = flow_page(
            300.0,
            "span { padding: 3px 5px; border-left: 2px solid red; }",
        );
        text(&mut doc, p, "aa ");
        let span = element(&mut doc, p, "span");
        text(&mut doc, span, "bb");
        text(&mut doc, p, " cc");
        let view = element(&mut doc, p, "view");
        doc.layout();

        let flow = doc.inline_flow(p).expect("a committed flow");
        let [line] = flow.line_geometry() else {
            panic!("one line expected")
        };
        // The span's vertical padding moves nothing.
        assert_eq!((line.height, line.baseline), (10.0, 8.0));
        // aa␣ 0–30, start edge 30–37, bb 37–57, end edge 57–62, ␣cc 62–92;
        // the content area 8px above the baseline and 2px below, grown by
        // the 3px of vertical padding.
        assert_eq!(fragment_rects(&doc, p, 1), [(30.0, -3.0, 32.0, 16.0)]);
        assert_eq!(size(&doc, p), Size::new(92.0, 10.0));
        assert_eq!(size(&doc, view), Size::new(0.0, 0.0));
        // An inline box generates no layout box of its own.
        assert_eq!(size(&doc, span), Size::new(0.0, 0.0));
    }

    #[test]
    fn a_wrapped_box_has_one_fragment_per_line_and_a_union_for_a_rect() {
        let (mut doc, p) = flow_page(70.0, "span { padding: 0 10px; }");
        text(&mut doc, p, "aa ");
        let span = element(&mut doc, p, "span");
        text(&mut doc, span, "bb cc");
        text(&mut doc, p, " dd");
        doc.layout();
        // "aa ", the start edge and "bb " fill the 70px line; "cc", the end
        // edge and " dd" the next.
        assert_eq!(
            fragment_rects(&doc, p, 1),
            [(30.0, 0.0, 30.0, 10.0), (0.0, 10.0, 30.0, 10.0)]
        );
        let flow = doc.inline_flow(p).expect("a flow");
        let fragments: Vec<_> = flow.fragments().iter().map(|f| (f.first, f.last)).collect();
        assert_eq!(fragments, [(true, false), (false, true)]);
        // CSSOM-View: the union of the fragments' border boxes.
        doc.render();
        let rect = doc.bounding_client_rect(span).expect("a rect");
        assert_eq!(
            (
                rect.origin.x,
                rect.origin.y,
                rect.size.width,
                rect.size.height
            ),
            (0.0, 0.0, 60.0, 20.0)
        );
    }

    #[test]
    fn a_point_on_a_fragment_s_padding_hits_its_inline_box() {
        let (mut doc, p) = flow_page(300.0, "span { padding: 4px 10px; }");
        text(&mut doc, p, "aa ");
        let link = element(&mut doc, p, "a");
        let span = element(&mut doc, link, "span");
        text(&mut doc, span, "bb");
        text(&mut doc, p, " cc");
        doc.layout();
        doc.render();
        // The span's start padding: 30–40, the link holding it.
        let hits = doc.elements_from_point(Point2D::new(35.0, 5.0));
        assert_eq!(hits[..3], [span, link, p]);
        // Its top padding reaches 4px above the content area, past the top
        // of the line box and of the paragraph's own box: it is still the
        // span's (CSS 2.2 §10.6.1 lets it overflow the line).
        assert_eq!(
            doc.elements_from_point(Point2D::new(45.0, -2.0)).first(),
            Some(&span)
        );
        assert_eq!(
            doc.elements_from_point(Point2D::new(45.0, -5.0)).first(),
            None,
            "past the padding nothing of the page is there"
        );
        // Plain text past the span is the paragraph.
        assert_eq!(
            doc.elements_from_point(Point2D::new(85.0, 5.0)).first(),
            Some(&p)
        );
        let paint = doc.build_paint_order();
        assert!(
            paint.items().iter().all(|item| item.node != span
                || matches!(item.kind, PaintItemKind::InlineFragment { .. })),
            "an inline box paints no record of its own"
        );
    }

    #[test]
    fn the_geometry_apis_answer_for_a_block_container() {
        let (mut doc, p) = flow_page(300.0, "span { padding: 0 5px; }");
        let plain = text(&mut doc, p, "aa");
        let span = element(&mut doc, p, "span");
        let inner = text(&mut doc, span, "bb");
        text(&mut doc, p, "cc");
        let view = element(&mut doc, p, "view");
        assert_eq!(doc.paragraph_of(span), None, "no pass has run");
        doc.layout();

        for node in [p, plain, span, inner] {
            assert_eq!(doc.paragraph_of(node), Some(p), "{node:?}");
        }
        assert_eq!(doc.paragraph_of(view), None, "no content of the flow");
        assert_eq!(doc.inline_source_range(span), Some(2..4));
        assert_eq!(doc.inline_source_range(p), None, "the paragraph itself");
        // aa 0–20, start edge 20–25, bb 25–45, end edge 45–50, cc 50–70.
        let boxes = doc.text_range_boxes(p, 2, 4).expect("in range");
        assert_eq!(
            boxes,
            [crate::Rect::new(
                Point2D::new(25.0, 0.0),
                crate::Size2D::new(20.0, 10.0)
            )]
        );
        assert_eq!(doc.text_range_boxes(p, 0, 7), None, "past the source");
        assert_eq!(doc.text_index_at_point(p, Point2D::new(30.0, 5.0)), Some(2));
        assert_eq!(doc.text_index_at_point(p, Point2D::new(22.0, 5.0)), None);
        assert_eq!(doc.text_index_at_point(p, Point2D::new(55.0, 5.0)), Some(4));
        // An inline box is no paragraph of its own.
        assert_eq!(doc.text_range_boxes(span, 0, 1), None);
    }

    #[test]
    fn percentage_edges_resolve_against_the_containing_block_and_re_shape_on_change() {
        let (mut doc, p) = flow_page(300.0, "p { width: 200px; } span { padding-left: 5%; }");
        text(&mut doc, p, "aa");
        let span = element(&mut doc, p, "span");
        text(&mut doc, span, "bb");
        doc.layout();
        // 5% of the 200px content box.
        assert_eq!(fragment_rects(&doc, p, 1), [(20.0, 0.0, 30.0, 10.0)]);
        doc.set_inline_style(p, "width: 100px;");
        doc.layout();
        assert_eq!(fragment_rects(&doc, p, 1), [(20.0, 0.0, 25.0, 10.0)]);
        // A padding change is a new edge: the flow is shaped again.
        let before = doc.text_block_rebuilds(p).expect("a flow");
        doc.set_inline_style(span, "padding-left: 1px;");
        doc.layout();
        assert_eq!(fragment_rects(&doc, p, 1), [(20.0, 0.0, 21.0, 10.0)]);
        assert_eq!(doc.text_block_rebuilds(p), Some(before + 1));
    }

    #[test]
    fn a_text_change_inside_an_inline_box_re_lays_its_paragraph_out() {
        let (mut doc, p) = flow_page(300.0, "");
        text(&mut doc, p, "aa ");
        let span = element(&mut doc, p, "span");
        let inner = text(&mut doc, span, "bb");
        doc.layout();
        assert_eq!(size(&doc, p).width, 50.0);
        doc.set_text_node_data(inner, "bbbb");
        doc.layout();
        assert_eq!(size(&doc, p).width, 70.0);
        // An inline box is never the paragraph's establishing element, even
        // where `display: inline` would be a block container elsewhere.
        assert!(super::is_inline_box(doc.get(span).expect("live")));
        assert!(!super::is_inline_box(doc.get(p).expect("live")));
    }

    #[test]
    fn an_inline_element_under_a_relative_container_lays_its_own_content_out() {
        // A relative container is no item container (the fork's
        // `Display::is_item_container`), so an `inline` child keeps `inline`.
        // Its box parent is no block container, so it is no inline box: it
        // lays its own content out, inline boxes included, as one.
        let (mut doc, _) = flow_page(
            300.0,
            "rel { display: relative; width: 200px; height: 100px; } a { padding: 0 5px; }",
        );
        let root = doc.document_element().id();
        let rel = element(&mut doc, root, "rel");
        let span = element(&mut doc, rel, "span");
        text(&mut doc, span, "aa ");
        let link = element(&mut doc, span, "a");
        text(&mut doc, link, "bb");
        doc.layout();
        assert_eq!(
            *doc.get(span)
                .expect("live")
                .layout_computed_style()
                .expect("styled")
                .get_display(),
            hughie::style::Display::Inline
        );
        assert!(!super::is_inline_box(doc.get(span).expect("live")));
        assert!(super::is_inline_box(doc.get(link).expect("live")));
        // Its own paragraph, with the link's fragment in it: "aa " then the
        // 5px start padding.
        assert_eq!(doc.paragraph_of(link), Some(span));
        assert_eq!(fragment_rects(&doc, span, 1), [(30.0, 0.0, 30.0, 10.0)]);
        assert_eq!(doc.text_block_size(span), Some(Size::new(60.0, 10.0)));
        let placed = doc.rounded_layout(span).expect("laid out");
        assert_eq!(
            (placed.location.x, placed.location.y, placed.size.height),
            (0.0, 0.0, 10.0)
        );
    }

    #[test]
    fn an_inline_box_s_generated_content_is_its_content() {
        // `br` as the engine writes it: an inline box whose `content` is a
        // preserved newline. Only an engine sheet writes `inline`.
        let (mut doc, p) = flow_page(
            300.0,
            r"br { content: '\A'; white-space-collapse: preserve-breaks; }",
        );
        doc.add_stylesheet("br { display: inline; }", StylesheetOrigin::UserAgent);
        text(&mut doc, p, "aa");
        element(&mut doc, p, "br");
        text(&mut doc, p, "bb");
        doc.layout();
        let flow = doc.inline_flow(p).expect("a flow");
        assert_eq!(flow.line_geometry().len(), 2);
        assert_eq!(size(&doc, p), Size::new(20.0, 20.0));
    }

    /// A 400px page of 10px Ahem: every glyph, the space included, advances
    /// 10px, and the block is as wide as its widest line.
    const WIDE: &str = "page { display: flex; flex-direction: column; align-items: flex-start;
                               width: 400px; height: 200px;
                               font-family: Ahem; font-size: 10px; }";

    fn flow_size(doc: &Document<()>, block: NodeId) -> Size<f32> {
        doc.inline_flow(block).expect("a flow").size()
    }

    #[test]
    fn a_run_takes_its_white_space_from_computed_style() {
        // `white-space-collapse` inherits into the anonymous block, so its
        // run reads the page's value.
        for (css, width) in [
            ("", 10.0),
            ("page { white-space-collapse: preserve; }", 50.0),
            ("page { white-space: pre; }", 50.0),
        ] {
            let (doc, block, _) = page(&format!("{WIDE} {css}"), "    x");
            assert_eq!(flow_size(&doc, block).width, width, "{css:?}");
        }
    }

    #[test]
    fn a_preserved_tab_lands_on_column_eight_and_each_break_restarts_the_columns() {
        // `abc` ends at 30 and the tab runs to the stop at 80; after the
        // break the tab starts at the edge and runs to 80 again.
        let (doc, block, _) = page(
            &format!("{WIDE} page {{ white-space: pre; }}"),
            "abc\tx\n\ty",
        );
        assert_eq!(flow_size(&doc, block), Size::new(90.0, 20.0));
    }

    #[test]
    fn preserve_breaks_still_collapses_the_spaces_around_a_break() {
        let (doc, block, _) = page(
            &format!("{WIDE} page {{ white-space-collapse: preserve-breaks; }}"),
            "  a\n  b",
        );
        assert_eq!(flow_size(&doc, block), Size::new(10.0, 20.0));
    }

    #[test]
    fn a_contents_scope_s_white_space_reaches_its_runs_alone() {
        let (mut doc, block, _) = page(
            &format!(
                "{WIDE} span {{ display: contents; }}
                 .pre {{ white-space-collapse: preserve; }}"
            ),
            "a  ",
        );
        let span = doc.create_element("span", ());
        doc.add_class(span, "pre");
        doc.append_child(block, span);
        let inner = doc.create_text_node("  b", ());
        doc.append_child(span, inner);
        doc.layout();
        // `a`, the collapsed pair as one space, then the two preserved
        // spaces: the collapsible space is kept by the preserved one after it.
        assert_eq!(flow_size(&doc, block).width, 50.0);

        // Turning the mode off rebuilds the flow: `a b`.
        doc.remove_class(span, "pre");
        doc.layout();
        assert_eq!(flow_size(&doc, block).width, 30.0);
    }

    #[test]
    fn break_spaces_keeps_its_spaces_and_the_unparsed_keywords_collapse() {
        // `break-spaces` keeps every space, as `preserve` does: `x` follows
        // four of them.
        let (doc, block, _) = page(
            &format!("{WIDE} page {{ white-space-collapse: break-spaces; }}"),
            "    x",
        );
        assert_eq!(flow_size(&doc, block).width, 50.0);
        // `preserve-spaces` and `discard` do not parse in the fork, leaving
        // the initial `collapse`.
        for keyword in ["preserve-spaces", "discard"] {
            let (doc, block, _) = page(
                &format!("{WIDE} page {{ white-space-collapse: {keyword}; }}"),
                "    x",
            );
            assert_eq!(flow_size(&doc, block).width, 10.0, "{keyword}");
        }
    }
}
