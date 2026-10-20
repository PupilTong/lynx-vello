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
//! | `none` | not content: hidden |
//! | `absolute`, `fixed` | not content: an out-of-flow box, placed from where it is in the content |
//! | anything else | one atomic inline box, laid out by its own display |
//!
//! The last row covers an inline-level replaced element and every
//! block-level box — `block`, `flex`, `grid`, `grid-lanes`, `linear`,
//! `relative`, a Lynx text element. CSS 2.2 §9.2.1.1 splits an inline box
//! around a block-level box inside it; that is not implemented, and such a
//! box is an atomic inline here (`docs/tracking/deviations.md`).
//!
//! An out-of-flow box is no content: it takes nothing from the line and
//! white space collapses across it. Its static position (CSS 2.2 §10.3.7,
//! §10.6.4) is its place in the content, where the content after it begins
//! on its line (`InlineFlow::static_position`). The block lays it out when
//! the block, or an inline box of its content, is its containing block, and
//! otherwise records that position for the containing block above, which
//! lays it out (`layout::hoisted`); the shared commit pass does both for
//! either paragraph kind (`paragraph::place_and_hide`).
//!
//! The runs reach the paragraph through the same pieces the Lynx block uses
//! (`crate::layout::paragraph`), and an atom is placed as a Lynx atom is
//! (`crate::layout::text_block`): by its margin box, at the place the
//! paragraph gave it. What differs is the walk, the parameters — CSS
//! properties only — the inline boxes, and three things about an atom, each
//! CSS's where a Lynx paragraph follows native Lynx:
//!
//! - its width: an auto width shrinks to fit the width the line is laid out at (CSS 2.2 §10.3.9,
//!   [`size_atoms`]), where a Lynx atom is measured once at max-content;
//! - its `vertical-align`, read from its own style (`alignment-baseline` and `baseline-shift`, the
//!   longhands the fork parses `vertical-align` into), where a Lynx atom sits on the baseline;
//! - `position: relative`, which moves it from its place on the line (CSS 2.2 §9.4.3), where a Lynx
//!   paragraph ignores the insets.
//!
//! # Inline boxes outside layout
//!
//! An inline box generates no layout box: its slot is hidden, or, when an
//! atom or an out-of-flow box sits inside it, written empty at the block's
//! border-box origin so the rounding walk reaches that box (as a Lynx
//! paragraph's consumed scopes are). Its geometry is the
//! paragraph's fragments ([`hughie::text::inline::InlineFragment`]), which
//! the painter draws its background and border from, the hit test answers
//! it from, and `Document::bounding_client_rect` unions. The paint-order
//! build emits no record of its own for it ([`is_inline_box`]), and it clips
//! nothing: `overflow` and paint containment do not apply to a non-atomic
//! inline box (css-overflow-3, css-contain-2 §3.3).

use std::hash::Hasher;

use hughie::compute::LeafMeasureInput;
use hughie::geometry::{Edges, Point, Rect, Size};
use hughie::style::{CoreStyle, PositionProperty};
use hughie::text::block::PlacedBox;
use hughie::text::inline::{
    InlineAtom, InlineBoxOpen, InlineEdges, InlineFlow, InlineFlowItem, InlineFlowStyle, InlineRun,
};
use hughie::text::paragraph::{BreakStyle, Direction, OverflowWrap, RunStyle, VerticalAlign};
use hughie::tree::{AvailableSpace, LayoutTree};
use stylo::properties::ComputedValues;

use crate::layout::paragraph::{
    AtomOffset, BlockBox, Collected, OutOfFlowBox, OwnedItem, Placement, TextStore,
    collect_generated, context_and_store, hash_flow, place_and_hide, run_text_security,
    run_white_space,
};
use crate::layout::style::{DisplayMode, StyleView, TextRunView, box_parent};
use crate::layout::text_block::{ATOM_SPACE, Atom, margin_box, out_of_flow};
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
    Atom(VerticalAlign),
    Open(RunStyle, BreakStyle, Direction, InlineEdges),
    Close,
}

/// One block container's inline formatting context, plus the table that maps
/// it back to the DOM.
pub(crate) struct InlineFlowStore {
    pub(crate) flow: InlineFlow,
    /// Item index → the node behind it: a run's element (the paint identity
    /// `InlineFlow::source_of` names), an atom, an inline box for both its
    /// boundaries.
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
    Atom,
    /// An absolutely positioned or fixed box: no content, but laid out (or
    /// recorded for its containing block) from its place in the content.
    OutOfFlow,
}

fn role_of<T>(node: &Node<T>, view: &StyleView<'_, T>) -> Role {
    let display = view.display_mode();
    if out_of_flow(view.position()) {
        return match display {
            DisplayMode::None | DisplayMode::Contents => Role::Hidden,
            _ => Role::OutOfFlow,
        };
    }
    match display {
        DisplayMode::None => Role::Hidden,
        DisplayMode::Contents => Role::Contents,
        DisplayMode::BlockFlow
            if *view.values().get_display() == hughie::style::Display::Inline
                && !node.is_replaced() =>
        {
            Role::InlineBox
        }
        _ => Role::Atom,
    }
}

/// One step of the content walk: a node to visit, or the end of the inline
/// box a node opened.
enum Visit {
    Node(NodeSlot),
    Close(NodeSlot),
}

/// The block's content, in order: runs, atoms and inline box boundaries;
/// and every out-of-flow box among it, with the index of the item that
/// follows it.
///
/// Iterative, like the Lynx block's walk, because nesting depth is
/// author-controlled. `basis` is what an inline box's percentage margin and
/// padding resolve against.
fn collect_flow<T>(
    tree: &TreeArenas<T>,
    element: NodeSlot,
    basis: Option<f32>,
) -> (Vec<Collected>, Vec<(NodeSlot, u32)>) {
    let mut collected = Vec::new();
    let mut out_of_flow = Vec::new();
    if collect_generated(tree, element, &mut collected) {
        return (collected, out_of_flow);
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
                    text_security: run_text_security(node),
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
            Role::Atom => collected.push(Collected {
                item: OwnedItem::Atom {
                    vertical_align: vertical_align(view.values()),
                },
                source: slot,
            }),
            Role::OutOfFlow => out_of_flow.push((
                slot,
                u32::try_from(collected.len()).expect("item count fits u32"),
            )),
        }
    }
    (collected, out_of_flow)
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

/// An atom's `vertical-align`, from the two longhands the fork parses it
/// into (css-inline-3 §4: `alignment-baseline` and `baseline-shift`).
///
/// The vocabulary is CSS 2.2's, one value per box, so a shift keyword wins
/// over a non-`baseline` alignment: `text-top` and `super` together cannot be
/// written as `vertical-align` either. A length shift raises the box; a
/// percentage is one of the atom's own `line-height`, which
/// [`resolve_percent_alignments`] writes in as a length once the block's
/// text context is at hand.
fn vertical_align(style: &ComputedValues) -> VerticalAlign {
    use stylo::values::computed::{AlignmentBaseline, BaselineShift};
    use stylo::values::generics::box_::BaselineShiftKeyword;

    let values = style.get_box();
    match &values.baseline_shift {
        BaselineShift::Keyword(BaselineShiftKeyword::Sub) => VerticalAlign::Sub,
        BaselineShift::Keyword(BaselineShiftKeyword::Super) => VerticalAlign::Super,
        BaselineShift::Keyword(BaselineShiftKeyword::Top) => VerticalAlign::Top,
        BaselineShift::Keyword(BaselineShiftKeyword::Center) => VerticalAlign::Center,
        BaselineShift::Keyword(BaselineShiftKeyword::Bottom) => VerticalAlign::Bottom,
        BaselineShift::Length(shift) => match values.alignment_baseline {
            AlignmentBaseline::TextTop => VerticalAlign::TextTop,
            AlignmentBaseline::TextBottom => VerticalAlign::TextBottom,
            AlignmentBaseline::Middle => VerticalAlign::Middle,
            AlignmentBaseline::Baseline => match (shift.to_length(), shift.to_percentage()) {
                (Some(length), _) => VerticalAlign::Length(length.px()),
                (None, Some(percent)) => VerticalAlign::Percent(percent.0),
                // A calc() mixing both: its length part, the percentage
                // taken at zero.
                (None, None) => VerticalAlign::Length(
                    shift
                        .resolve(stylo::values::computed::Length::new(0.0))
                        .px(),
                ),
            },
        },
    }
}

/// Where an atom's baseline is, from the top of its border box: the one its
/// `baseline-source` names, `auto` taking the last for a box that lays its
/// content out in line boxes — what `inline-block` is (CSS 2.2 §10.8.1,
/// css-inline-3 `baseline-source`) — and the first for every other.
///
/// Such a box that is a scroll container has its bottom margin edge as its
/// baseline instead (§10.8.1, as css-align-3 §9.1 narrows "`overflow` other
/// than `visible`"): `None` here, which puts that edge on the line's
/// baseline.
fn atom_baseline<T>(node: &Node<T>, output: &hughie::tree::LayoutOutput) -> Option<f32> {
    use stylo::values::computed::BaselineSource;
    let view = StyleView::of(node);
    let line_boxes = matches!(
        view.display_mode(),
        DisplayMode::BlockFlow | DisplayMode::Text
    ) && !node.is_replaced();
    match view.values().get_box().baseline_source {
        BaselineSource::Last => output.last_baselines.y,
        BaselineSource::Auto if line_boxes => (!crate::scroll::is_scroll_container(view.values()))
            .then_some(output.last_baselines.y)
            .flatten(),
        BaselineSource::First | BaselineSource::Auto => output.first_baselines.y,
    }
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

/// The block's own parameters, from its computed style and, for an editing
/// host, its `contenteditable` attribute: a host's paragraph keeps one empty
/// line of its strut when it has no content, so the host is one line tall and
/// its caret has a line to sit on, as an HTML text control is never shorter
/// than one line.
///
/// An anonymous block box is never a host: a host that establishes no
/// paragraph of its own (a flex `view`) lays its loose text out in one, and
/// is left to `crate::editing`'s caret for a host with no paragraph.
fn flow_style<T>(tree: &TreeArenas<T>, element: NodeSlot) -> InlineFlowStyle {
    let node = tree.at(element);
    let view = StyleView::of(node);
    InlineFlowStyle {
        strut_line_when_empty: crate::editing::is_host_node(node),
        ..InlineFlowStyle::from_container_style(&view, &TextRunView::from_values(view.values()))
    }
}

/// What [`refresh`] found in the block's content.
struct Refreshed {
    /// Each atom, as `(node, item)`.
    atoms: Vec<(NodeSlot, u32)>,
    /// Each out-of-flow box, as `(node, the item after it)`.
    out_of_flow: Vec<(NodeSlot, u32)>,
}

/// Rebuilds `element`'s flow when its content or the values it is shaped
/// with moved, otherwise hands it the block's current parameters, and
/// reports its atoms and its out-of-flow boxes.
fn refresh<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    basis: Option<f32>,
) -> Refreshed {
    let (mut collected, out_of_flow) = collect_flow(tree, element, basis);
    resolve_percent_alignments(tree, state, element, &mut collected);
    let stamp = {
        let mut hasher = rustc_hash::FxHasher::default();
        hash_flow(&collected, &mut hasher);
        hasher.finish()
    };
    let style = flow_style(tree, element);
    let keys = keys_of(tree, &collected);
    let atoms: Vec<(NodeSlot, u32)> = collected
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry.item, OwnedItem::Atom { .. }))
        .map(|(index, entry)| {
            (
                entry.source,
                u32::try_from(index).expect("item count fits u32"),
            )
        })
        .collect();

    // The strut is parley's style 0 and the direction its bidi paragraph
    // level, and whether empty content keeps one line of the strut decides
    // whether it is shaped at all: all are read when the flow is shaped, so
    // a change to any is a new flow, like an item change.
    let needs_build = state
        .text_store(element)
        .and_then(TextStore::as_flow)
        .is_none_or(|store| {
            store.fingerprint != stamp
                || store.keys != keys
                || store.style.strut != style.strut
                || store.style.direction != style.direction
                || store.style.strut_line_when_empty != style.strut_line_when_empty
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
    Refreshed { atoms, out_of_flow }
}

/// Writes each atom's `vertical-align` percentage in as the length it is:
/// that fraction of the atom's own used `line-height` (CSS 2.2 §10.8.1),
/// which for `normal` takes the atom's font, and so the block's text context.
fn resolve_percent_alignments<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    collected: &mut [Collected],
) {
    for entry in collected {
        if let OwnedItem::Atom {
            vertical_align: VerticalAlign::Percent(fraction),
        } = entry.item
        {
            let style = RunStyle::from_run_style(&TextRunView::from_values(
                StyleView::of(tree.at(entry.source)).values(),
            ));
            let (context, _) = state.text_store_parts(element);
            let line_height = hughie::text::paragraph::used_line_height(context, &style);
            entry.item = OwnedItem::Atom {
                vertical_align: VerticalAlign::Length(fraction * line_height),
            };
        }
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
            OwnedItem::Atom { vertical_align } => ItemKey::Atom(*vertical_align),
            OwnedItem::Open {
                style,
                breaking,
                direction,
                edges,
            } => ItemKey::Open(style.clone(), *breaking, *direction, *edges),
            OwnedItem::Close => ItemKey::Close,
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
                    text,
                    white_space,
                    text_security,
                    ..
                },
                ItemKey::Run(run_style, breaking),
            ) => InlineFlowItem::Run(InlineRun {
                text,
                style: run_style,
                white_space: *white_space,
                text_security: *text_security,
                breaking: *breaking,
            }),
            (OwnedItem::Atom { vertical_align }, _) => InlineFlowItem::Atom(InlineAtom {
                vertical_align: *vertical_align,
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

/// The width a line is laid out at in one measurement — what an atom on it
/// shrinks to fit: the content width when the input fixes it, else the width
/// available to the content, zero at min-content; `None` at max-content.
fn line_width(measure_input: &LeafMeasureInput) -> Option<f32> {
    measure_input
        .known_dimensions
        .width
        .or(match measure_input.available_space.width {
            AvailableSpace::Definite(width) => Some(width),
            AvailableSpace::MinContent => Some(0.0),
            AvailableSpace::MaxContent => None,
        })
}

/// An atom's containing block in one measurement (CSS 2.2 §10.1: the block
/// container's content box), as far as it is definite: what the atom's
/// percentages resolve against.
fn containing_block(measure_input: &LeafMeasureInput) -> Size<Option<f32>> {
    Size::new(
        measure_input
            .known_dimensions
            .width
            .or(match measure_input.available_space.width {
                AvailableSpace::Definite(width) => Some(width),
                AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
            }),
        measure_input.known_dimensions.height,
    )
}

/// How an atom's own `width` sizes it on a line.
enum AtomWidth {
    /// `auto` (or `fit-content`) on a non-replaced box: shrink-to-fit, CSS
    /// 2.2 §10.3.9.
    ShrinkToFit,
    /// `stretch`: the line's whole width, less the margins.
    Fill,
    /// Anything else, a replaced element's `auto` included (§10.3.2): the
    /// box sizes itself, the line's width playing no part.
    Own,
}

fn atom_width<T>(node: &Node<T>, view: &StyleView<'_, T>) -> AtomWidth {
    use stylo::values::computed::Size as StyleSize;
    if node.is_replaced() {
        return AtomWidth::Own;
    }
    match view.size().width {
        StyleSize::Auto | StyleSize::FitContent => AtomWidth::ShrinkToFit,
        StyleSize::Stretch | StyleSize::WebkitFillAvailable => AtomWidth::Fill,
        _ => AtomWidth::Own,
    }
}

/// The shrink-to-fit width of `atom` (CSS 2.2 §10.3.9) for `available`, the
/// line's width less the atom's margins: `min(max(min-content, available),
/// max-content)`, both intrinsic widths measured — and kept in the atom's own
/// measurement cache — as border boxes with its `min-width` and `max-width`
/// applied, so the result already lies between them. The min-content width
/// is measured only when the max-content one does not fit.
fn shrink_to_fit<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    atom: NodeSlot,
    containing: Size<Option<f32>>,
    available: f32,
) -> f32 {
    let mut intrinsic = |space| {
        tree.compute_layout(
            state,
            atom,
            hughie::tree::LayoutInput::measure(
                Size::new(None, None),
                containing,
                Size::new(space, AvailableSpace::MaxContent),
                hughie::tree::RequestedAxis::Horizontal,
            ),
        )
        .size
        .width
    };
    let preferred = intrinsic(AvailableSpace::MaxContent);
    if preferred <= available {
        return preferred;
    }
    intrinsic(AvailableSpace::MinContent)
        .max(available)
        .min(preferred)
}

/// Lays every atom out for a line `line` wide and writes its margin box into
/// the flow: an auto width shrinks to fit the line (CSS 2.2 §10.3.9), any
/// other width is the atom's own. A commit commits each atom's subtree at
/// the size it is given; a probe measures it.
///
/// What the line's width is per measurement — the content width, the width
/// available, zero at min-content, unbounded at max-content — makes an
/// atom's contribution the CSS intrinsic one at the two extremes, so the
/// flow's min-content and max-content widths are CSS's too.
fn size_atoms<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    atoms: &[(NodeSlot, u32)],
    measure_input: &LeafMeasureInput,
) -> Vec<Atom> {
    let line = line_width(measure_input);
    let containing = containing_block(measure_input);
    let mut placed = Vec::with_capacity(atoms.len());
    for &(atom, item) in atoms {
        let view = tree.style(atom);
        let margin = hughie::compute::used_margins(&view, containing.width);
        let available = line.map(|line| (line - margin.horizontal_sum()).max(0.0));
        let width = match (atom_width(tree.at(atom), &view), available) {
            (AtomWidth::ShrinkToFit, Some(available)) => {
                Some(shrink_to_fit(tree, state, atom, containing, available))
            }
            (AtomWidth::Fill, Some(available)) => Some(available),
            // At max-content an auto width is the box's max-content width,
            // which it takes laid out against an unbounded line by itself.
            _ => None,
        };
        let known = Size::new(width, None);
        let space = Size::new(
            width.map_or(ATOM_SPACE, AvailableSpace::Definite),
            ATOM_SPACE,
        );
        let output = if measure_input.goal.commits() {
            hughie::compute::compute_inline_box_layout(tree, state, atom, known, containing, space)
        } else {
            tree.compute_layout(
                state,
                atom,
                hughie::tree::LayoutInput::measure(
                    known,
                    containing,
                    space,
                    hughie::tree::RequestedAxis::Both,
                ),
            )
        };
        if let Some((_, store)) = context_and_store(state, element)
            && let Some(store) = store.as_flow_mut()
        {
            // A baseline is measured from the border box's top edge, so it
            // moves down with the margin box's.
            let baseline = atom_baseline(tree.at(atom), &output)
                .map(|baseline| (baseline + margin.top).max(0.0));
            store
                .flow
                .set_atom_size(item, margin_box(output.size, margin), baseline);
        }
        placed.push(Atom {
            slot: atom,
            id: u64::from(item),
            margin,
        });
    }
    placed
}

/// Lays `element` out as a block container whose content is one inline
/// formatting context.
///
/// Refresh the flow, then probe or commit it inside hughie's leaf box
/// wrapper, sizing its atoms there first — inside, because an atom's size
/// depends on the width the wrapper lays the line out at — and on a commit
/// place the atoms and hide everything else the paragraph consumed. A Lynx
/// paragraph (`crate::layout::text_block`) measures its atoms once, at
/// max-content, outside the wrapper instead.
pub(crate) fn compute_inline_flow_layout<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    input: hughie::tree::LayoutInput,
) -> hughie::tree::LayoutOutput {
    use hughie::compute::compute_text_block_layout as compute_block_box;

    let view = StyleView::of(tree.at(element));
    let percentages = percentage_basis(&view, &input);
    let Refreshed { atoms, out_of_flow } = refresh(tree, state, element, percentages);

    let basis = input.known_dimensions.width.or(input.parent_size.width);
    let mut placed_atoms: Vec<Atom> = Vec::new();
    let mut atom_block = Size::new(None, None);
    let output = compute_block_box(input, &view, |measure_input| {
        if !atoms.is_empty() {
            placed_atoms = size_atoms(tree, state, element, &atoms, &measure_input);
            atom_block = containing_block(&measure_input);
        }
        crate::layout::paragraph::measure(tree, state, element, basis, measure_input)
    });
    if input.goal.commits() {
        let block = BlockBox::of(&view, output.size, input.parent_size.width);
        let committed = state
            .text_store(element)
            .and_then(TextStore::as_flow)
            .filter(|store| store.committed().is_some());
        let placements: Vec<PlacedBox> =
            committed.map_or_else(Vec::new, |store| store.flow.atoms().to_vec());
        let out_of_flow_boxes = committed.map_or_else(Vec::new, |store| {
            out_of_flow_boxes(tree, element, store, &out_of_flow, &block)
        });
        place_and_hide(
            tree,
            state,
            element,
            &Placement {
                atoms: &placed_atoms,
                placements: &placements,
                offset: AtomOffset::Relative(atom_block),
                out_of_flow: &out_of_flow_boxes,
            },
            &block,
        );
    } else {
        state.note_probed_text(element);
    }
    output
}

/// The out-of-flow boxes of the committed flow's content, each at its static
/// position: where the content after it begins on its line (CSS 2.2
/// §10.3.7, §10.6.4, `InlineFlow::static_position`), moved from the
/// paragraph's space to the block's border box.
///
/// An absolutely positioned box whose box parent is an inline box of the
/// paragraph has that box for its containing block (§10.1.4): the bounding
/// box of the padding boxes of its first and last fragments. Every other one
/// is the block's, or is hoisted past it.
///
/// A block-level box among inline content is an atom here rather than a box
/// that splits the line (`docs/tracking/deviations.md`), and its static
/// position is taken the same way, at its place on the line.
fn out_of_flow_boxes<T>(
    tree: &TreeArenas<T>,
    element: NodeSlot,
    store: &InlineFlowStore,
    found: &[(NodeSlot, u32)],
    block: &BlockBox,
) -> Vec<OutOfFlowBox> {
    let flow = &store.flow;
    let origin = block.content_origin();
    let rtl = |node: &Node<T>| StyleView::of(node).direction() == hughie::style::direction::T::Rtl;
    let block_rtl = rtl(tree.at(element));
    found
        .iter()
        .map(|&(slot, before)| {
            let at = flow.static_position(before);
            let static_position = Point::new(origin.x + at.x, origin.y + at.y);
            let node = tree.at(slot);
            let inline_parent = box_parent(node)
                .filter(|parent| parent.slot() != element && is_inline_box(parent))
                .filter(|_| StyleView::of(node).position() == PositionProperty::Absolute);
            let (containing, from_right) = match inline_parent {
                Some(parent) => (
                    Some(
                        inline_box_padding_box(store, parent.id(), rtl(parent)).map_or_else(
                            || Rect::new(static_position, Size::ZERO),
                            |rect| rect.translate(origin),
                        ),
                    ),
                    rtl(parent),
                ),
                None => (None, block_rtl),
            };
            OutOfFlowBox {
                slot,
                static_position,
                from_right,
                containing,
            }
        })
        .collect()
}

/// The containing block an inline box establishes for an absolutely
/// positioned descendant (CSS 2.2 §10.1.4): the bounding box around the
/// padding boxes of its first and its last fragment, in the paragraph's
/// space. `None` for a box with no fragment.
fn inline_box_padding_box(store: &InlineFlowStore, id: NodeId, rtl: bool) -> Option<Rect<f32>> {
    let insets = store
        .insets
        .iter()
        .find(|insets| store.source_ids.get(insets.item as usize) == Some(&id))?;
    let fragments = store.flow.fragments();
    let of_box = || {
        fragments
            .iter()
            .filter(|fragment| fragment.item == insets.item)
    };
    let first = of_box().find(|fragment| fragment.first)?;
    let last = of_box().rev().find(|fragment| fragment.last)?;
    // A fragment carries the box's border only on the sides it has: its
    // inline-start border on the first, its inline-end border on the last.
    let padding_box = |fragment: &hughie::text::inline::InlineFragment| {
        let border = insets.border;
        let (start, end) = (fragment.first, fragment.last);
        let (left, right) = if rtl { (end, start) } else { (start, end) };
        let left = if left { border.left } else { 0.0 };
        let right = if right { border.right } else { 0.0 };
        let rect = fragment.rect;
        (
            rect.origin.x + left,
            rect.origin.y + border.top,
            rect.origin.x + rect.size.width - right,
            rect.origin.y + rect.size.height - border.bottom,
        )
    };
    let (a, b) = (padding_box(first), padding_box(last));
    let (left, top) = (a.0.min(b.0), a.1.min(b.1));
    let (right, bottom) = (a.2.max(b.2), a.3.max(b.3));
    Some(Rect::new(
        Point::new(left, top),
        Size::new((right - left).max(0.0), (bottom - top).max(0.0)),
    ))
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
    fn contents_scopes_are_walked_and_box_generating_children_are_atoms() {
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
        // text replaces its child. The view is an atomic inline box, laid
        // out by its own display: it does not fit after "bbcc" on the 60px
        // line, so it starts the third line.
        let flow = doc.inline_flow(block).expect("a flow");
        assert_eq!(flow.size().width, 60.0);
        assert_eq!(flow.line_geometry().len(), 3);
        assert_eq!(size(&doc, view), Size::new(30.0, 30.0));
        let layout = doc.rounded_layout(view).expect("placed");
        assert_eq!((layout.location.x, layout.location.y), (0.0, 20.0));
    }

    /// A page whose UA sheet writes flow layout, as an engine sheet does: `p`
    /// a block container, `span` and `a` inline boxes, `view` an atom. The
    /// Ahem page is `width` wide; `css` adds author rules.
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
    fn a_block_container_lays_out_inline_boxes_and_atoms() {
        // aa <span>bb</span> cc <view>: the span's start edge is a 2px
        // border and 5px of padding, its end edge 5px of padding; the view
        // is an atom on the baseline.
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
        // The 20px atom sits on the baseline: 20px above it, the strut's 2px
        // below. The span's vertical padding moves nothing.
        assert_eq!((line.height, line.baseline), (22.0, 20.0));
        // aa␣ 0–30, start edge 30–37, bb 37–57, end edge 57–62, ␣cc 62–92;
        // the content area 8px above the baseline and 2px below, grown by
        // the 3px of vertical padding.
        assert_eq!(fragment_rects(&doc, p, 1), [(30.0, 9.0, 32.0, 16.0)]);
        assert_eq!(size(&doc, p), Size::new(112.0, 22.0));
        let placed = doc.rounded_layout(view).expect("placed");
        assert_eq!((placed.location.x, placed.location.y), (92.0, 0.0));
        assert_eq!(size(&doc, view), Size::new(20.0, 20.0));
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
    }

    #[test]
    fn the_geometry_apis_answer_for_a_block_container() {
        let (mut doc, p) = flow_page(300.0, "span { padding: 0 5px; }");
        let plain = text(&mut doc, p, "aa");
        let span = element(&mut doc, p, "span");
        let inner = text(&mut doc, span, "bb");
        text(&mut doc, p, "cc");
        let view = element(&mut doc, p, "view");
        let in_view = element(&mut doc, view, "view");
        assert_eq!(doc.paragraph_of(span), None, "no pass has run");
        doc.layout();

        for node in [p, plain, span, inner, view] {
            assert_eq!(doc.paragraph_of(node), Some(p), "{node:?}");
        }
        assert_eq!(doc.paragraph_of(in_view), None, "inside an atom");
        assert_eq!(doc.inline_source_range(span), Some(2..4));
        assert_eq!(doc.inline_source_range(view), Some(6..7));
        assert_eq!(doc.inline_source_range(p), None, "the paragraph itself");
        // aa 0–20, start edge 20–25, bb 25–45, end edge 45–50, cc 50–70,
        // the atom 70–90; the line is the atom's 22px.
        let boxes = doc.text_range_boxes(p, 2, 4).expect("in range");
        assert_eq!(
            boxes,
            [crate::Rect::new(
                Point2D::new(25.0, 0.0),
                crate::Size2D::new(20.0, 22.0)
            )]
        );
        let boxes = doc.text_range_boxes(p, 6, 7).expect("in range");
        assert_eq!(boxes[0].origin.x, 70.0);
        assert_eq!(boxes[0].size.width, 20.0);
        assert_eq!(doc.text_range_boxes(p, 0, 8), None, "past the source");
        assert_eq!(
            doc.text_index_at_point(p, Point2D::new(30.0, 15.0)),
            Some(2)
        );
        assert_eq!(doc.text_index_at_point(p, Point2D::new(22.0, 15.0)), None);
        assert_eq!(
            doc.text_index_at_point(p, Point2D::new(75.0, 15.0)),
            Some(6)
        );
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
    fn a_lynx_text_atom_lays_out_paints_and_follows_its_text() {
        let (mut doc, p) = flow_page(300.0, "");
        crate::test_common::define_lynx_text(&mut doc, "text");
        text(&mut doc, p, "aa");
        let label = element(&mut doc, p, "text");
        let run = text(&mut doc, label, "bb");
        text(&mut doc, p, "cc");
        doc.layout();
        // An atom of the block's line, laid out as the Lynx paragraph it is.
        let placed = doc.rounded_layout(label).expect("placed");
        assert_eq!(
            (placed.location.x, placed.location.y, placed.size.width),
            (20.0, 0.0, 20.0)
        );
        assert_eq!(size(&doc, p), Size::new(60.0, 10.0));
        // Its run belongs to its own paragraph, not to the block's.
        assert_eq!(doc.paragraph_of(run), Some(label));
        assert_eq!(doc.paragraph_of(label), Some(label));
        assert_eq!(
            doc.text_range_boxes(label, 0, 2).map(|boxes| boxes.len()),
            Some(1)
        );
        doc.render();
        assert_eq!(
            doc.elements_from_point(Point2D::new(25.0, 5.0)).first(),
            Some(&label)
        );

        // A text change re-lays the `<text>` out, and the block's line with
        // it: the upward invalidation reaches the block through the atom.
        let builds = doc.text_block_rebuilds(label).expect("a paragraph");
        doc.set_text_node_data(run, "bbbb");
        doc.layout();
        assert_eq!(doc.text_block_rebuilds(label), Some(builds + 1));
        assert_eq!(size(&doc, label).width, 40.0);
        assert_eq!(size(&doc, p), Size::new(80.0, 10.0));
        doc.render();
        assert_eq!(
            doc.elements_from_point(Point2D::new(55.0, 5.0)).first(),
            Some(&label)
        );

        // Inside an inline box the invalidation walks through the box, which
        // runs no layout of its own, to the block.
        let (mut doc, p) = flow_page(300.0, "");
        crate::test_common::define_lynx_text(&mut doc, "text");
        let span = element(&mut doc, p, "span");
        let label = element(&mut doc, span, "text");
        let run = text(&mut doc, label, "bb");
        doc.layout();
        assert_eq!(size(&doc, p).width, 20.0);
        doc.set_text_node_data(run, "bbbbb");
        doc.layout();
        assert_eq!(size(&doc, p).width, 50.0);
        assert_eq!(fragment_rects(&doc, p, 0), [(0.0, 0.0, 50.0, 10.0)]);
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

    /// `(x, y, width, height)` of `id`'s rounded box, from its parent's.
    fn placed_box(doc: &Document<()>, id: NodeId) -> (f32, f32, f32, f32) {
        let layout = doc.rounded_layout(id).expect("laid out");
        (
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        )
    }

    fn absolute(doc: &mut Document<()>, parent: NodeId, css: &str) -> NodeId {
        let node = element(doc, parent, "view");
        doc.set_inline_style(
            node,
            &format!("position: absolute; width: 5px; height: 5px; {css}"),
        );
        node
    }

    #[test]
    fn an_absolute_box_s_static_position_is_its_place_in_the_line() {
        // CSS 2.2 §10.3.7, §10.6.4. aa<abs> bbb<abs> at 50px: "aa " and
        // "bbb" on two lines. Before the space the box is on the first line,
        // where the space hangs; before "bbb", after the wrap, on the second;
        // at the end, after "bbb".
        let (mut doc, p) = flow_page(300.0, "p { position: relative; width: 50px; }");
        text(&mut doc, p, "aa");
        let before_space = absolute(&mut doc, p, "");
        text(&mut doc, p, " ");
        let after_wrap = absolute(&mut doc, p, "");
        text(&mut doc, p, "bbb");
        let at_end = absolute(&mut doc, p, "");
        doc.layout();
        assert_eq!(doc.inline_flow(p).expect("a flow").line_geometry().len(), 2);
        assert_eq!(placed_box(&doc, before_space), (20.0, 0.0, 5.0, 5.0));
        assert_eq!(placed_box(&doc, after_wrap), (0.0, 10.0, 5.0, 5.0));
        assert_eq!(placed_box(&doc, at_end), (30.0, 10.0, 5.0, 5.0));
        // The boxes take nothing from the line, and white space collapses
        // across them: the same lines as without them.
        assert_eq!(size(&doc, p), Size::new(50.0, 20.0));

        // Insets win over the static position, against the padding box.
        let (mut doc, p) = flow_page(300.0, "p { position: relative; padding: 3px; }");
        text(&mut doc, p, "aa");
        let inset = absolute(&mut doc, p, "right: 0; bottom: 0;");
        doc.layout();
        assert_eq!(placed_box(&doc, inset), (21.0, 11.0, 5.0, 5.0));
    }

    #[test]
    fn an_out_of_flow_box_whose_block_is_further_up_is_placed_from_its_place_in_the_line() {
        // The block is not positioned, so its absolute child's containing
        // block is the initial one and a fixed child's the viewport: the
        // block records their static positions, and they are laid out once,
        // from there, not hidden.
        let (mut doc, p) = flow_page(300.0, "");
        text(&mut doc, p, "aa");
        let escaping = absolute(&mut doc, p, "");
        text(&mut doc, p, "bb");
        let fixed = element(&mut doc, p, "view");
        doc.set_inline_style(fixed, "position: fixed; width: 5px; height: 5px;");
        doc.layout();
        assert_eq!(placed_box(&doc, escaping), (20.0, 0.0, 5.0, 5.0));
        assert_eq!(placed_box(&doc, fixed), (40.0, 0.0, 5.0, 5.0));
        assert_eq!(size(&doc, p), Size::new(40.0, 10.0));
    }

    #[test]
    fn an_absolute_box_in_an_inline_box_takes_the_box_s_padding_box() {
        // CSS 2.2 §10.1.4: a positioned inline box is the containing block of
        // its absolutely positioned descendants, the bounding box of its
        // first and last fragments' padding boxes. The span starts after
        // "aa" with a 2px border, so its padding box starts at 22.
        let (mut doc, p) = flow_page(
            300.0,
            "span { position: relative; border-left: 2px solid; padding: 0 3px; }",
        );
        text(&mut doc, p, "aa");
        let span = element(&mut doc, p, "span");
        text(&mut doc, span, "bb");
        let pinned = absolute(&mut doc, span, "left: 0; top: 0;");
        let flowing = absolute(&mut doc, span, "");
        doc.layout();
        assert_eq!(fragment_rects(&doc, p, 1), [(20.0, 0.0, 28.0, 10.0)]);
        // Its location is from the span's, which is the block's border box.
        assert_eq!(placed_box(&doc, pinned), (22.0, 0.0, 5.0, 5.0));
        // Without insets, its static position: after "bb", before the end
        // padding.
        assert_eq!(placed_box(&doc, flowing), (45.0, 0.0, 5.0, 5.0));

        // In an inline box that is not positioned, the containing block is
        // the positioned block: the box is hoisted to it from the inline
        // box, laid out once by the block's absolute pass, and not hidden
        // with the inline box's subtree.
        let (mut doc, p) = flow_page(300.0, "p { position: relative; }");
        text(&mut doc, p, "aa");
        let span = element(&mut doc, p, "span");
        text(&mut doc, span, "bb");
        let hoisted = absolute(&mut doc, span, "");
        let pinned = absolute(&mut doc, span, "right: 0; top: 1px;");
        doc.layout();
        assert_eq!(placed_box(&doc, hoisted), (40.0, 0.0, 5.0, 5.0));
        assert_eq!(placed_box(&doc, pinned), (35.0, 1.0, 5.0, 5.0));
    }

    #[test]
    fn a_box_hoisted_out_of_an_atom_is_laid_out_by_the_block() {
        // The atom's absolute child escapes it to the positioned block, whose
        // absolute pass lays it out against its padding box.
        let (mut doc, p) = flow_page(300.0, "p { position: relative; }");
        text(&mut doc, p, "aa");
        let atom = element(&mut doc, p, "view");
        let escaping = absolute(&mut doc, atom, "left: 1px; top: 2px;");
        doc.layout();
        assert_eq!(placed_box(&doc, atom).0, 20.0);
        assert_eq!(placed_box(&doc, escaping), (-19.0, 2.0, 5.0, 5.0));

        // Past the block, when the containing block is further up: the block
        // is a box like any other on the way.
        let (mut doc, p) = flow_page(300.0, "page { position: relative; padding: 4px; }");
        text(&mut doc, p, "aa");
        let atom = element(&mut doc, p, "view");
        let escaping = absolute(&mut doc, atom, "left: 1px; top: 2px;");
        doc.layout();
        assert_eq!(placed_box(&doc, p).0, 4.0);
        assert_eq!(placed_box(&doc, escaping), (-23.0, -2.0, 5.0, 5.0));
    }

    #[test]
    fn a_right_to_left_block_puts_the_static_position_on_the_right_edge() {
        // CSS 2.2 §10.3.7: in a right-to-left containing block `right` is set
        // to the static position. "aabb" fills 60..100 of the 100px line, and
        // the box between "aa" and "bb" ends at 80, where "bb" starts.
        let (mut doc, p) = flow_page(
            300.0,
            "p { position: relative; direction: rtl; width: 100px; }",
        );
        text(&mut doc, p, "aa");
        let between = absolute(&mut doc, p, "");
        text(&mut doc, p, "bb");
        doc.layout();
        assert_eq!(placed_box(&doc, between), (75.0, 0.0, 5.0, 5.0));
    }

    #[test]
    fn an_atom_takes_its_vertical_align() {
        let (mut doc, p) = flow_page(300.0, "view { vertical-align: 5px; }");
        text(&mut doc, p, "aa");
        let view = element(&mut doc, p, "view");
        doc.layout();
        let flow = doc.inline_flow(p).expect("a flow");
        // Raised 5px: the line's baseline drops from 20 to 25.
        let [line] = flow.line_geometry() else {
            panic!("one line expected")
        };
        assert_eq!((line.height, line.baseline), (27.0, 25.0));
        let placed = doc.rounded_layout(view).expect("placed");
        assert_eq!((placed.location.x, placed.location.y), (20.0, 0.0));
    }

    #[test]
    fn an_atom_s_vertical_align_percentage_is_of_its_own_line_height() {
        // CSS 2.2 §10.8.1: 50% of the atom's own 10px line-height raises it
        // 5px, not 20px, half the block's 40px. The strut reaches 17px below
        // the baseline (8px ascent, 2px descent, 15px half-leading each
        // side), the raised 20px atom 25px above it.
        let (mut doc, p) = flow_page(
            300.0,
            "p { line-height: 40px; } view { vertical-align: 50%; line-height: 10px; }",
        );
        text(&mut doc, p, "aa");
        element(&mut doc, p, "view");
        doc.layout();
        let [line] = doc.inline_flow(p).expect("a flow").line_geometry() else {
            panic!("one line expected")
        };
        assert_eq!((line.height, line.baseline), (42.0, 25.0));

        // `normal` is the atom's own font's: Ahem at 20px is 20px, so the
        // atom rises 10px.
        let (mut doc, p) = flow_page(
            300.0,
            "p { line-height: 40px; }
             view { vertical-align: 50%; line-height: normal; font-size: 20px; }",
        );
        text(&mut doc, p, "aa");
        element(&mut doc, p, "view");
        doc.layout();
        let [line] = doc.inline_flow(p).expect("a flow").line_geometry() else {
            panic!("one line expected")
        };
        assert_eq!((line.height, line.baseline), (47.0, 30.0));
    }

    #[test]
    fn an_atom_inside_an_inline_box_is_placed_and_its_carrier_clips_nothing() {
        // The Lynx UA sheet clips every element; an inline box has no box to
        // clip to, so the atom inside it must still paint and hit.
        let (mut doc, p) = flow_page(300.0, "span { overflow: clip; padding: 0 5px; }");
        text(&mut doc, p, "aa");
        let span = element(&mut doc, p, "span");
        let view = element(&mut doc, span, "view");
        doc.layout();
        let placed = doc.rounded_layout(view).expect("placed");
        assert_eq!((placed.location.x, placed.location.y), (25.0, 0.0));
        let carrier = doc.rounded_layout(span).expect("a carrier keeps a layout");
        assert_eq!((carrier.location.x, carrier.size.width), (0.0, 0.0));
        doc.render();
        assert_eq!(
            doc.elements_from_point(Point2D::new(30.0, 10.0)).first(),
            Some(&view)
        );
        assert_eq!(
            doc.elements_from_point(Point2D::new(22.0, 15.0)).first(),
            Some(&span),
            "the start padding"
        );
        let paint = doc.build_paint_order();
        assert!(
            paint.items().iter().all(|item| item.node != span
                || matches!(item.kind, PaintItemKind::InlineFragment { .. })),
            "an inline box paints no record of its own"
        );
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
    fn a_run_takes_its_text_security_from_computed_style() {
        // Inherited into the anonymous block: every character, the spaces
        // and the newline included, is one mask, and nothing collapses.
        let (doc, block, _) = page(
            &format!("{WIDE} page {{ -webkit-text-security: disc; }}"),
            "a  b\n",
        );
        let flow = doc.inline_flow(block).expect("a flow");
        assert_eq!(flow.offset_map().display_text(), "\u{2022}".repeat(5));
        assert_eq!(flow.line_geometry().len(), 1);
        assert_eq!(flow_size(&doc, block), Size::new(50.0, 10.0));
    }

    #[test]
    fn a_contents_scope_s_text_security_reaches_its_runs_alone_and_toggles() {
        let (mut doc, block, _) = page(
            &format!(
                "{WIDE} span {{ display: contents; }}
                 .masked {{ -webkit-text-security: circle; }}"
            ),
            "a  ",
        );
        let span = doc.create_element("span", ());
        doc.add_class(span, "masked");
        doc.append_child(block, span);
        let inner = doc.create_text_node("  b", ());
        doc.append_child(span, inner);
        doc.layout();
        // `a`, the collapsible pair as one space the first mask keeps, then
        // three masks.
        let flow = doc.inline_flow(block).expect("a flow");
        assert_eq!(
            flow.offset_map().display_text(),
            format!("a {}", "\u{25E6}".repeat(3))
        );

        // Turning the mask off rebuilds the flow: `a b`.
        doc.remove_class(span, "masked");
        doc.layout();
        let flow = doc.inline_flow(block).expect("a flow");
        assert_eq!(flow.offset_map().display_text(), "a b");
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

    /// `card` is an atom with an auto width: a flex container, its text an
    /// anonymous item that wraps at the width the card is given.
    const CARD: &str = "card { display: flex; }";

    fn card(doc: &mut Document<()>, parent: NodeId, class: &str, content: &str) -> NodeId {
        let node = element(doc, parent, "card");
        if !class.is_empty() {
            doc.set_classes(node, class);
        }
        text(doc, node, content);
        node
    }

    #[test]
    fn a_wide_auto_width_atom_shrinks_to_the_line_and_wraps_its_own_content() {
        // CSS 2.2 §10.3.9: min(max(min-content, available), max-content),
        // the available width the block's 100px content box. The card's
        // max-content width is 190px, so it takes the 100px and wraps its
        // text over two lines inside, on a line of its own after "x".
        let (mut doc, p) = flow_page(300.0, "p { width: 100px; }");
        doc.add_stylesheet(CARD, StylesheetOrigin::UserAgent);
        text(&mut doc, p, "x ");
        let wide = card(&mut doc, p, "", "aaaa bbbb cccc dddd");
        doc.layout();
        assert_eq!(size(&doc, wide), Size::new(100.0, 20.0));
        let placed = doc.rounded_layout(wide).expect("placed");
        assert_eq!((placed.location.x, placed.location.y), (0.0, 10.0));
        assert_eq!(size(&doc, p), Size::new(100.0, 30.0));
    }

    #[test]
    fn a_probe_at_another_width_hands_the_atoms_back_their_committed_sizes() {
        use hughie::tree::{AvailableSpace, LayoutInput, LayoutTree, RequestedAxis};

        // The card shrinks to the 100px line the page offers and wraps over
        // two lines.
        let (mut doc, p) = flow_page(100.0, "");
        doc.add_stylesheet(CARD, StylesheetOrigin::UserAgent);
        text(&mut doc, p, "x ");
        let wide = card(&mut doc, p, "", "aaaa bbbb cccc dddd");
        doc.layout();
        let flow = doc.inline_flow(p).expect("a flow");
        let atoms = flow.atoms().to_vec();
        let lines = flow.line_geometry().to_vec();
        let shaped_lines = flow.display().lines().count();
        assert_eq!(size(&doc, wide), Size::new(100.0, 20.0));

        // A probe at 45px, which a pass can end without committing again:
        // the card shrinks to 45px there, four lines tall, and the flow is
        // broken around it.
        let slot = doc.live_slot(p);
        let (tree, state) = doc.layout_parts();
        tree.compute_layout(
            state,
            slot,
            LayoutInput::measure(
                Size::NONE,
                Size::NONE,
                Size::new(AvailableSpace::Definite(45.0), AvailableSpace::MaxContent),
                RequestedAxis::Both,
            ),
        );
        assert!(doc.text_block_is_probe_dirty(p));

        // The end of the pass puts the committed sizes back before the
        // committed break, so paint reads the commit's lines and places.
        doc.layout_state_mut().restore_probed_text();
        assert!(!doc.text_block_is_probe_dirty(p));
        let flow = doc.inline_flow(p).expect("a flow");
        assert_eq!(flow.atoms(), atoms.as_slice());
        assert_eq!(flow.line_geometry(), lines.as_slice());
        assert_eq!(flow.display().lines().count(), shaped_lines);
        doc.render();
        assert_eq!(
            doc.elements_from_point(Point2D::new(95.0, 25.0)).first(),
            Some(&wide),
            "the card is hit where the commit put it, at its committed width"
        );
    }

    #[test]
    fn a_narrow_auto_width_atom_stays_at_its_max_content_width() {
        // Not the 100px the line offers: shrink-to-fit, not fill.
        let (mut doc, p) = flow_page(300.0, "p { width: 100px; }");
        doc.add_stylesheet(CARD, StylesheetOrigin::UserAgent);
        text(&mut doc, p, "x");
        let narrow = card(&mut doc, p, "", "ab");
        text(&mut doc, p, "y");
        doc.layout();
        assert_eq!(size(&doc, narrow), Size::new(20.0, 10.0));
        let placed = doc.rounded_layout(narrow).expect("placed");
        assert_eq!(placed.location.x, 10.0);
        // An atom no line holds keeps its min-content width and overflows.
        let (mut doc, p) = flow_page(300.0, "p { width: 30px; }");
        doc.add_stylesheet(CARD, StylesheetOrigin::UserAgent);
        let wide = card(&mut doc, p, "", "aaaa bbbb");
        doc.layout();
        assert_eq!(size(&doc, wide), Size::new(40.0, 20.0));
    }

    #[test]
    fn an_atom_s_percentage_and_min_max_widths_resolve_against_the_block() {
        let (mut doc, p) = flow_page(
            400.0,
            "p { width: 200px; }
             card.half { width: 50%; }
             card.capped { max-width: 60px; }
             card.floored { min-width: 80px; }",
        );
        doc.add_stylesheet(CARD, StylesheetOrigin::UserAgent);
        let half = card(&mut doc, p, "half", "ab");
        let capped = card(&mut doc, p, "capped", "aaaa bbbb cccc");
        let floored = card(&mut doc, p, "floored", "ab");
        doc.layout();
        // 50% of the 200px content box, whatever the content.
        assert_eq!(size(&doc, half).width, 100.0);
        // Shrink-to-fit inside `max-width`: three lines of one word each.
        assert_eq!(size(&doc, capped), Size::new(60.0, 30.0));
        // And never below `min-width`.
        assert_eq!(size(&doc, floored).width, 80.0);
    }

    #[test]
    fn a_relative_atom_keeps_its_place_on_the_line_and_moves_from_it() {
        let (mut doc, p) = flow_page(
            300.0,
            "p { width: 200px; }
             view.moved { position: relative; left: 5px; top: 3px; }
             view.percent { position: relative; left: 10%; right: 4px; }
             view.raised { position: relative; right: 4px; bottom: 2px; }",
        );
        // aa 0–20, moved 20–40, bb 40–60, percent 60–80, cc 80–100,
        // raised 100–120: the line is laid out as if none were relative.
        text(&mut doc, p, "aa");
        let moved = element(&mut doc, p, "view");
        doc.set_classes(moved, "moved");
        text(&mut doc, p, "bb");
        let percent = element(&mut doc, p, "view");
        doc.set_classes(percent, "percent");
        text(&mut doc, p, "cc");
        let raised = element(&mut doc, p, "view");
        doc.set_classes(raised, "raised");
        doc.layout();
        let at = |id| {
            let layout = doc.rounded_layout(id).expect("placed");
            (layout.location.x, layout.location.y)
        };
        assert_eq!(at(moved), (25.0, 3.0));
        // `left` wins over `right`, 10% of the 200px content box.
        assert_eq!(at(percent), (80.0, 0.0));
        assert_eq!(at(raised), (96.0, -2.0));
        assert_eq!(size(&doc, p), Size::new(200.0, 22.0));

        doc.render();
        let rect = doc.bounding_client_rect(moved).expect("a rect");
        assert_eq!((rect.origin.x, rect.origin.y), (25.0, 3.0));
        // Hit and painted where it moved to: its old left edge is the
        // paragraph's now, and its new right edge is the atom's.
        assert_eq!(
            doc.elements_from_point(Point2D::new(22.0, 5.0)).first(),
            Some(&p)
        );
        assert_eq!(
            doc.elements_from_point(Point2D::new(43.0, 5.0)).first(),
            Some(&moved)
        );
    }

    #[test]
    fn a_block_s_intrinsic_widths_take_its_atoms_intrinsic_widths() {
        // The page sizes the block to fit its content: max-content with room
        // for it, the atom unwrapped beside "x"; min-content without, the
        // atom at its own min-content width, a word.
        for (page, block, atom) in [
            (300.0, Size::new(100.0, 10.0), Size::new(90.0, 10.0)),
            (30.0, Size::new(40.0, 30.0), Size::new(40.0, 20.0)),
        ] {
            let (mut doc, p) = flow_page(page, "");
            doc.add_stylesheet(CARD, StylesheetOrigin::UserAgent);
            text(&mut doc, p, "x");
            let wide = card(&mut doc, p, "", "aaaa bbbb");
            doc.layout();
            assert_eq!(size(&doc, p), block, "page {page}");
            assert_eq!(size(&doc, wide), atom, "page {page}");
        }
    }
}
