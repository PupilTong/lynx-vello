//! A block container: one element, its text runs laid out as a CSS inline
//! formatting context.
//!
//! The internal `(block, flow)` display ([`DisplayMode::BlockFlow`]) is the
//! display of the anonymous block box CSS wraps around a contiguous sequence
//! of loose text in a flex, grid, grid-lanes, linear or relative container
//! (CSS 2.2 §9.2.1.1, css-flexbox-1 §4, css-grid-1 §6). Its content is text
//! runs only — the `lynx` grammar has no `inline` or `inline-block`, so no
//! inline-level box exists to sit among them — laid out by
//! [`hughie::text::inline`].
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
//! | anything else | not content: it generates no box here and is hidden |
//!
//! The last row includes `display: none`, out-of-flow boxes and every
//! box-generating element. The anonymous block producer never puts one inside
//! the box — it wraps text runs only — so hiding is the whole answer, not a
//! layout this box owes them.
//!
//! The runs reach the paragraph through the same pieces the Lynx block uses
//! (`crate::layout::paragraph`); what differs is the walk, the parameters —
//! CSS properties only — and that no atom is ever placed.

use std::hash::Hasher;

use hughie::style::CoreStyle;
use hughie::text::inline::{InlineFlow, InlineFlowStyle, InlineRun};
use hughie::text::paragraph::{BreakStyle, OverflowWrap, RunStyle};
use hughie::tree::LayoutTree;
use stylo::properties::ComputedValues;

use crate::layout::paragraph::{
    Collected, OwnedItem, TextStore, collect_generated, context_and_store, hash_flow,
    run_white_space,
};
use crate::layout::style::{DisplayMode, StyleView, TextRunView, display_mode};
use crate::tree::document::{DocumentLayoutState, NodeId, NodeSlot, TreeArenas};

/// One block container's inline formatting context, plus the table that maps
/// it back to the DOM.
pub(crate) struct InlineFlowStore {
    pub(crate) flow: InlineFlow,
    /// Run index → the node whose style the run carries: the paint identity
    /// `InlineFlow::source_of` names.
    pub(crate) source_ids: Vec<NodeId>,
    /// What the flow was built from, compared rather than hashed for style,
    /// as the Lynx block's store does: a missed rebuild would paint glyphs
    /// shaped at an old font.
    fingerprint: u64,
    style: InlineFlowStyle,
    runs: Vec<(RunStyle, BreakStyle)>,
    /// How many times this element's flow has been shaped from scratch.
    pub(crate) rebuilds: u32,
}

impl InlineFlowStore {
    /// The flow, if a commit has produced one.
    pub(crate) fn committed(&self) -> Option<&InlineFlow> {
        self.flow.has_committed().then_some(&self.flow)
    }
}

/// The block's text runs, in order.
///
/// Iterative, like the Lynx block's walk, because `contents` nesting depth is
/// author-controlled.
fn collect_flow<T>(tree: &TreeArenas<T>, element: NodeSlot) -> Vec<Collected> {
    let mut collected = Vec::new();
    if collect_generated(tree, element, &mut collected) {
        return collected;
    }
    let mut stack: Vec<NodeSlot> = tree.children(element).collect();
    stack.reverse();
    while let Some(slot) = stack.pop() {
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
        if node.is_element()
            && display_mode(StyleView::of(node).display()) == DisplayMode::Contents
            && !collect_generated(tree, slot, &mut collected)
        {
            let first = stack.len();
            stack.extend(tree.children(slot));
            stack[first..].reverse();
        }
    }
    collected
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

/// Rebuilds `element`'s flow when its runs or the values they are shaped
/// with moved; otherwise hands it the block's current parameters.
fn refresh<T>(tree: &TreeArenas<T>, state: &mut DocumentLayoutState, element: NodeSlot) {
    let collected = collect_flow(tree, element);
    let stamp = {
        let mut hasher = rustc_hash::FxHasher::default();
        hash_flow(&collected, &mut hasher);
        hasher.finish()
    };
    let style = flow_style(tree, element);
    let runs: Vec<(RunStyle, BreakStyle)> = collected
        .iter()
        .filter_map(|entry| match &entry.item {
            OwnedItem::Run { style, .. } => {
                let breaking = tree
                    .at(entry.source)
                    .layout_computed_style()
                    .map_or_else(BreakStyle::default, breaking_of);
                Some((style.clone(), breaking))
            }
            OwnedItem::Atom { .. } => None,
        })
        .collect();

    // The strut is shaped from too — its first available font is measured
    // at construction — so a strut change is a new flow, like a run change.
    let needs_build = state
        .text_store(element)
        .and_then(TextStore::as_flow)
        .is_none_or(|store| {
            store.fingerprint != stamp || store.runs != runs || store.style.strut != style.strut
        });

    if needs_build {
        let inline_runs: Vec<InlineRun<'_>> = collected
            .iter()
            .zip(&runs)
            .filter_map(|(entry, (run_style, breaking))| match &entry.item {
                OwnedItem::Run {
                    text, white_space, ..
                } => Some(InlineRun {
                    text,
                    style: run_style,
                    white_space: *white_space,
                    breaking: *breaking,
                }),
                OwnedItem::Atom { .. } => None,
            })
            .collect();
        let source_ids = collected
            .iter()
            .map(|entry| tree.at(entry.source).id())
            .collect();
        let (context, slot) = state.text_store_parts(element);
        let rebuilds = slot.as_deref().map_or(0, |store| store.rebuilds() + 1);
        let flow = InlineFlow::new(context, style.clone(), &inline_runs);
        *slot = Some(Box::new(TextStore::Flow(InlineFlowStore {
            flow,
            source_ids,
            fingerprint: stamp,
            style,
            runs,
            rebuilds,
        })));
    } else if let Some((_, store)) = context_and_store(state, element)
        && let Some(store) = store.as_flow_mut()
    {
        // Same runs, possibly a different alignment or indent keyword.
        store.flow.set_style(style.clone());
        store.style = style;
    }
}

/// Lays `element` out as a block container whose content is one inline
/// formatting context.
///
/// The box model is hughie's leaf wrapper, as for the Lynx block; inside it
/// the shared measure closure probes or commits the flow. A commit hides every
/// child: none of them generates a box (module doc).
pub(crate) fn compute_inline_flow_layout<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    input: hughie::tree::LayoutInput,
) -> hughie::tree::LayoutOutput {
    use hughie::compute::compute_text_block_layout as compute_block_box;

    refresh(tree, state, element);
    let basis = input.known_dimensions.width.or(input.parent_size.width);
    let view = StyleView::of(tree.at(element));
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
        assert_eq!((line.height, line.baseline, line.shift), (20.0, 16.0, 8.0));
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
}
