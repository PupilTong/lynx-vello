//! The `list` and `list-item` tags: a scroller whose cells are virtualized by
//! `content-visibility: auto` and placed by `list-type`.
//!
//! A `list` is a [`super::scroll_container`] in everything the axis rules say
//! — which is why the scrolling half of `x-list.css` lives here rather than
//! there, one module per tag — plus three things a `scroll-view` has no idea
//! about: it is a **size query container**, so a cell's size estimate can be
//! written against it; its cells **skip their contents** until the frame
//! reaches them; and its `list-type` picks the layout mode the cells are
//! placed by. All three are the UA sheet's whole contribution. No cell
//! recycling, no `scrollToPosition`, no threshold events, no
//! `update-list-info` consumer — `docs/tracking/components.md` rows 23-24
//! carry what is still missing.
//!
//! Everything here is translated from web-elements'
//! `lynx-stack/packages/web-platform/web-elements/src/elements/XList/x-list.css`,
//! with the tag renamed (`x-list` → `list`, `lynx-wrapper` → `wrapper`) and
//! the shadow-part machinery dropped: web-core puts the grid on a
//! `::part(content)` box inside the shadow tree because a custom element
//! cannot lay its own light-tree children out in two modes at once, while here
//! the authored `list` *is* the box, so every `::part(content)` declaration
//! lands on the tag itself.
//!
//! # Where this deliberately leaves web-core
//!
//! - **The span count defaults to `1`, not `0`** (`x-list.css:11`). Zero is not a workable CSS
//!   default: `repeat(0, 1fr)` is invalid, so the whole `grid-template-columns` declaration would
//!   be dropped and a `flow` list would fall back to a single implicit column anyway. web-core
//!   survives it because its own JavaScript reads the attribute and re-writes the property; here
//!   the sheet is the only reader. web-core's changelog records the same symptom from the other end
//!   — "list may only render only one column in `ReactLynx`" when `span-count` is unset
//!   (`web-elements/CHANGELOG.md:376-378`, lynx-stack PR #1280).
//! - **A horizontal `flow` or `waterfall` list resets `grid-template-columns: none`.** web-core's
//!   horizontal `flow` rule (`x-list.css:221-229`) sets the row template and leaves the column
//!   template standing, which in a browser makes an `n x n` grid rather than `n` rows; its
//!   horizontal `waterfall` rule (`:262-264`) only swaps a `flex-direction`, since that waterfall
//!   is JavaScript-positioned and never a grid at all. Here the reset is load-bearing rather than
//!   cosmetic: `grid-lanes` reads the *absence* of a column template as the statement that the
//!   block axis carries the tracks (css-grid-3 §2.3's initial `grid-auto-flow: normal` behaviour,
//!   `crates/hughie/src/compute/grid/lanes.rs:535-547`), so without it a horizontal waterfall would
//!   stack downwards.
//! - **`waterfall` is `display: grid-lanes`, not absolutely positioned cells.** web-core computes
//!   the staggered placement in JavaScript and writes `left`/`top` onto absolutely positioned cells
//!   (`XListWaterfall.ts`, `x-list.css:270-303`); this engine has the layout mode, and the
//!   2026-09-18 ruling stands that its W3C cursor tie-break is kept rather than switched to Lynx's
//!   always-lowest-index rule. The placement differences are itemised in
//!   `docs/tracking/deviations.md`.
//! - **A cell is a container like a `view`**: it takes `box-sizing` and the `defaultDisplayLinear`
//!   display from [`super::ua_sheet`]'s common block, where web-core pins `list-item { display:
//!   none }` and restores `display: flex` for a cell that is a list's child or grandchild through a
//!   `lynx-wrapper` (`x-list.css:63-64,81-84`). That is the same one-rule, one-switch decision
//!   `scroll-view` and `list` already record for their own display (`docs/tracking/deviations.md`,
//!   2026-08-21): the alternative is `!important`, which `docs/style-assumptions.md` §D.15 forbids
//!   in this sheet. Two consequences follow, both accepted: a `list-item` written outside any list
//!   is an ordinary container rather than an invisible one, and a cell nested in *any* depth of
//!   `wrapper` is placed, where web-core handles exactly one level.
//! - **`wrapper` is exempt from the non-cell suppression.** web-core writes `x-list >
//!   *:not(list-item) { display: none }` (`x-list.css:16-18`) and gets away with it because a
//!   `lynx-wrapper` is `display: contents` in the *browser's* author sheet at a specificity the
//!   rule does not reach in the same way. Here the two rules are in one origin and the suppression
//!   is the more specific of the two, so a wrapped cell would vanish; `ReactLynx` wraps list
//!   children routinely (`__CreateWrapperElement` mints the tag,
//!   `packages/bobcat-element/src/element-papi.ts:797-799`), so `wrapper` is named in the `:not()`
//!   rather than left to lose.
//!
//! `sticky-top="true"` sets `position: sticky`; its inset is left to author
//! styles. The remaining `sticky-top`/`sticky-bottom` rules
//! (`x-list.css:104-135`) are still missing, along with `item-snap` /
//! `paging-enabled` scroll snapping (`:137-151`), the scrollbar rules
//! (`:8,45-61`), and every `::part()` threshold observer behind
//! `scrolltoupper`/`scrolltolower` (`:153-193`).
//!
//! Gaps are the author's own `row-gap`/`column-gap`. web-core's
//! `--list-main-axis-gap`/`--list-cross-axis-gap` indirection exists because
//! its style transformer renames the Lynx-only `list-main-axis-gap` /
//! `list-cross-axis-gap` properties into custom properties; neither property
//! exists in this engine's grammar, pinned by `list_main_axis_gap_is_absent`
//! (`crates/dom/tests/grammar_layout.rs:342-351`), so there is nothing to
//! rename and the real gap properties are what an author writes.

use dom::{CustomElement, NodeId};

use super::{LynxDocument, parse_count};

/// The `list` and `list-item` policy, in `x-list.css`'s own order.
///
/// # Why a list is a size query container
///
/// `container-type: size` (`x-list.css:9`) is what makes
/// `contain-intrinsic-size: … var(--estimated-main-axis-size-px, 100cqh)`
/// mean anything: with no estimate supplied, a cell that has never been
/// rendered is one scrollport tall, which is the best guess a sheet can make
/// and the one web-core makes. css-contain-3 §2.1 makes a size query
/// container a *contained* box, so a list's own size never answers to its
/// cells — it must come from the author or from its parent, exactly as in a
/// browser, where `x-list` carries the same declaration
/// (`docs/style-assumptions.md` §19).
///
/// # Why a cell skips
///
/// `content-visibility: auto` plus `contain: layout paint` is web-core's
/// whole virtualization (`x-list.css:65-69`), and it is this engine's too:
/// relevance is the painter's encode window, determined inside
/// `Document::render`, and a cell no rendering update has reached yet skips
/// and is sized by its estimate. `recyclable="false"` opts a cell out
/// (`x-list.css:72-75`); web-core writes `contain: initial`, which is
/// `contain: none` spelled the way a browser's author-origin sheet has to
/// spell it.
///
/// # Cascade notes
///
/// `list > *:not(list-item):not(wrapper)` is specificity (0,0,3), so it
/// outranks the container block's own `display` (0,0,1) and hides a `view`,
/// an `image` or a `scroll-view` written directly inside a list. It does
/// **not** outrank [`super::text`]'s `display: -lynx-text !important`: a
/// `text` written as a list's direct child keeps its box where a browser
/// would hide it. That is the price of §D.15's one recorded important
/// exception and is not worth a second one — a `<text>` outside a
/// `<list-item>` is not a shape `ReactLynx` emits.
pub(super) const UA_RULES: &str = r#"
list {
  overflow-x: clip; overflow-y: scroll;
  flex-direction: column; linear-direction: column;
  contain: layout; container-type: size;
  --list-item-span-count: 1; --list-item-sticky-offset: 0px;
}
list[scroll-orientation="horizontal"] {
  overflow-x: scroll; overflow-y: clip;
  flex-direction: row; linear-direction: row;
}
list[enable-scroll="false"] { overflow-y: hidden; }
list[scroll-orientation="horizontal"][enable-scroll="false"] { overflow-x: hidden; }
list > *:not(list-item):not(wrapper) { display: none; }
list-item {
  content-visibility: auto; contain: layout paint;
  contain-intrinsic-size: none auto var(--estimated-main-axis-size-px, 100cqh);
  flex: 0 0 auto;
}
list[scroll-orientation="horizontal"] list-item {
  contain-intrinsic-size: auto var(--estimated-main-axis-size-px, 100cqw) none;
}
list-item[recyclable="false"] { content-visibility: visible; contain: none; }
list-item[sticky-top="true"] { position: sticky; }
list[list-type="flow"] {
  display: grid;
  grid-template-columns: repeat(var(--list-item-span-count), 1fr);
  grid-auto-rows: min-content;
  justify-items: stretch; align-items: start;
}
list[list-type="flow"][scroll-orientation="horizontal"] {
  grid-template-columns: none;
  grid-template-rows: repeat(var(--list-item-span-count), 1fr);
  grid-auto-flow: column; grid-auto-columns: min-content;
  justify-items: start; align-items: stretch;
}
list[list-type="flow"] list-item[full-span]:not([full-span="false"]) { grid-column: 1 / -1; }
list[list-type="flow"][scroll-orientation="horizontal"] list-item[full-span]:not([full-span="false"]) { grid-row: 1 / -1; }
list[list-type="waterfall"] {
  display: grid-lanes;
  grid-template-columns: repeat(var(--list-item-span-count), minmax(0, 1fr));
  flow-tolerance: 0;
}
list[list-type="waterfall"][scroll-orientation="horizontal"] {
  grid-template-columns: none;
  grid-template-rows: repeat(var(--list-item-span-count), minmax(0, 1fr));
}
list[list-type="waterfall"] list-item[full-span]:not([full-span="false"]) { grid-column: 1 / -1; }
list[list-type="waterfall"][scroll-orientation="horizontal"] list-item[full-span]:not([full-span="false"]) { grid-row: 1 / -1; }
"#;

const LIST_TAG: &str = "list";
const LIST_ITEM_TAG: &str = "list-item";

const SPAN_COUNT_ATTRIBUTE: &str = "span-count";
const COLUMN_COUNT_ATTRIBUTE: &str = "column-count";
const STICKY_OFFSET_ATTRIBUTE: &str = "sticky-offset";
const ESTIMATED_MAIN_AXIS_SIZE_ATTRIBUTE: &str = "estimated-main-axis-size-px";

const SPAN_COUNT_PROPERTY: &str = "--list-item-span-count";
const STICKY_OFFSET_PROPERTY: &str = "--list-item-sticky-offset";
const ESTIMATED_MAIN_AXIS_SIZE_PROPERTY: &str = "--estimated-main-axis-size-px";

/// Installs both components, the scroller's and the cell's — one per tag,
/// because the two tags observe different attributes, exactly as web-core's
/// `XListAttributes` and `ListItemAttributes` are mixed into `x-list` and
/// `x-list-item` separately. Must run before any element could carry either
/// tag, which is [`Document::define`](dom::Document::define)'s own
/// precondition.
pub(super) fn define(document: &mut LynxDocument) {
    document.define(LIST_TAG, Box::new(List));
    document.define(LIST_ITEM_TAG, Box::new(ListItem));
}

/// Reflects the scroller's lane count and sticky offset into the custom
/// properties [`UA_RULES`] resolves against.
///
/// - `span-count` and `column-count` are one hint in web-core too (`XListAttributes.ts:33-39`: the
///   two handlers share `_handlerCount`). It is narrowed to **positive integers** here:
///   `parseFloat` would let `2.5` or `0` through, and `repeat(2.5, 1fr)`/`repeat(0, 1fr)` are
///   invalid track lists that would drop the whole declaration and silently give the list one
///   implicit column. An unusable value clears the hint instead, which leaves the UA default of one
///   lane standing.
/// - `sticky-offset` (`XListAttributes.ts:26-31`) is mapped even though nothing reads
///   `--list-item-sticky-offset` yet: sticky positioning is the missing half, not the attribute.
///
/// None of the three names is observed anywhere but on a `list`, which is what
/// makes the hint's scope the tag's own: `--list-item-span-count` on some other
/// element inherits, but never reaches a list, because [`UA_RULES`] sets the
/// property on every `list` itself and that outranks inheritance. The
/// `--list-item-sticky-offset` a cell reads *is* inherited — from the list
/// the attribute was written on, as web-core's is.
struct List;

impl CustomElement<()> for List {
    fn observed_attributes(&self) -> Vec<String> {
        vec![
            SPAN_COUNT_ATTRIBUTE.to_owned(),
            COLUMN_COUNT_ATTRIBUTE.to_owned(),
            STICKY_OFFSET_ATTRIBUTE.to_owned(),
        ]
    }

    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        _old: Option<&str>,
        new: Option<&str>,
    ) {
        let (property, css) = match name {
            SPAN_COUNT_ATTRIBUTE | COLUMN_COUNT_ATTRIBUTE => {
                (SPAN_COUNT_PROPERTY, span_count_css(parse_count(new)))
            }
            STICKY_OFFSET_ATTRIBUTE => (STICKY_OFFSET_PROPERTY, pixels_css(parse_count(new))),
            other => {
                debug_assert!(false, "`list` does not observe `{other}`");
                return;
            }
        };
        document.set_presentational_hint(element, property, &css);
    }
}

/// Reflects a cell's own size estimate (`ListItemAttributes.ts:22-27`), which
/// is what [`UA_RULES`]' `contain-intrinsic-size` prefers over the `100cqh`
/// fallback a list's size query container supplies.
struct ListItem;

impl CustomElement<()> for ListItem {
    fn observed_attributes(&self) -> Vec<String> {
        vec![ESTIMATED_MAIN_AXIS_SIZE_ATTRIBUTE.to_owned()]
    }

    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        _old: Option<&str>,
        new: Option<&str>,
    ) {
        debug_assert_eq!(
            name, ESTIMATED_MAIN_AXIS_SIZE_ATTRIBUTE,
            "`list-item` observes `{ESTIMATED_MAIN_AXIS_SIZE_ATTRIBUTE}` alone"
        );
        document.set_presentational_hint(
            element,
            ESTIMATED_MAIN_AXIS_SIZE_PROPERTY,
            &pixels_css(parse_count(new)),
        );
    }
}

/// A lane count reflects as a bare integer; anything else clears the hint.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "parse_count bounds values to [0, u32::MAX] and the filter keeps whole positives"
)]
fn span_count_css(count: Option<f64>) -> String {
    count
        .filter(|count| *count >= 1.0 && count.fract() == 0.0)
        .map_or_else(String::new, |count| (count as u32).to_string())
}

/// A pixel length reflects with its unit; an unparsable or absent value
/// clears the hint, which is what puts the UA fallback back in charge.
fn pixels_css(length: Option<f64>) -> String {
    length.map_or_else(String::new, |length| format!("{length}px"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Ahem and explicit lane sizes have exact metrics.

    use dom::stylo::computed_values::{flex_direction, linear_direction};
    use dom::stylo::properties::PropertyId;
    use dom::stylo::values::computed::{Display, Overflow};
    use dom::{CustomElement, NodeId, Vector2D};

    use super::super::LynxDocument;
    use super::super::test_support::{child, display, document, element_under, overflow, style_of};
    use super::{List, ListItem};

    const AHEM: &[u8] = include_bytes!("../../../../hughie/tests/fixtures/Ahem.ttf");

    /// A 200x200 list under the page, with the font every cell's label is
    /// measured in already registered.
    ///
    /// The size is inline because a list is a size query container: its own
    /// box never answers to its cells, so a list with no declared size is a
    /// zero-height scroller in this engine exactly as `x-list` is in a
    /// browser.
    fn list_page(style: &str) -> (LynxDocument, NodeId) {
        let mut document = document();
        assert_eq!(document.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
        let list = child(&mut document, "list", style);
        (document, list)
    }

    fn cell(document: &mut LynxDocument, list: NodeId, style: &str) -> NodeId {
        element_under(document, list, "list-item", style)
    }

    /// The label whose shaped paragraph reports whether its cell skipped:
    /// `text_block_size` is `None` for a paragraph no layout ever reached.
    fn labelled_cell(document: &mut LynxDocument, list: NodeId) -> (NodeId, NodeId) {
        let cell = cell(document, list, "");
        let label = element_under(document, cell, "text", "");
        let run = element_under(document, label, "raw-text", "");
        document.set_attribute(run, "text", "x");
        (cell, label)
    }

    fn rect(document: &LynxDocument, element: NodeId) -> (f32, f32, f32, f32) {
        let layout = document.rounded_layout(element).expect("a live element");
        (
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        )
    }

    /// One computed longhand or custom property, as a string.
    fn value(document: &LynxDocument, element: NodeId, property: &str) -> String {
        let id = PropertyId::parse_enabled_for_all_content(property)
            .unwrap_or_else(|()| panic!("unknown property `{property}`"));
        let declaration = id
            .as_shorthand()
            .err()
            .unwrap_or_else(|| panic!("`{property}` is a shorthand"));
        style_of(document, element).computed_value_to_string(declaration)
    }

    /// Sets or removes an attribute the way the runtime does: the DOM write
    /// alone. An observed name raises the tag's component from inside that
    /// write, and the reaction is what reflects the hint.
    fn set_attribute(
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        value: Option<&str>,
    ) {
        if let Some(value) = value {
            document.set_attribute(element, name, value);
        } else {
            document.remove_attribute(element, name);
        }
    }

    // --- the scrolling half, moved out of `scroll_container` -------------

    #[test]
    fn a_list_scrolls_one_axis_and_clips_the_other() {
        let mut document = document();
        let vertical = child(&mut document, "list", "");
        let horizontal = child(&mut document, "list", "");
        document.set_attribute(horizontal, "scroll-orientation", "horizontal");
        document.layout();

        let style = style_of(&document, vertical);
        assert_eq!(
            (style.clone_overflow_x(), style.clone_overflow_y()),
            (Overflow::Hidden, Overflow::Scroll),
            "the clipped axis computes to `hidden` beside a scrolling one"
        );
        assert_eq!(style.clone_flex_direction(), flex_direction::T::Column);
        assert_eq!(style.clone_linear_direction(), linear_direction::T::Column);

        let style = style_of(&document, horizontal);
        assert_eq!(
            (style.clone_overflow_x(), style.clone_overflow_y()),
            (Overflow::Scroll, Overflow::Hidden)
        );
        assert_eq!(style.clone_flex_direction(), flex_direction::T::Row);
        assert_eq!(style.clone_linear_direction(), linear_direction::T::Row);
    }

    #[test]
    fn enable_scroll_false_leaves_a_list_only_script_can_move() {
        let mut document = document();
        let vertical = child(&mut document, "list", "");
        let horizontal = child(&mut document, "list", "");
        document.set_attribute(vertical, "enable-scroll", "false");
        document.set_attribute(horizontal, "scroll-orientation", "horizontal");
        document.set_attribute(horizontal, "enable-scroll", "false");
        document.layout();

        for list in [vertical, horizontal] {
            assert_eq!(
                overflow(&document, list),
                (Overflow::Hidden, Overflow::Hidden),
                "neither axis is `scroll` any more, so no drag reaches either"
            );
        }
    }

    #[test]
    fn a_list_lays_its_cells_out_along_the_axis_it_scrolls() {
        for horizontal in [false, true] {
            let (mut document, list) = list_page("width: 100px; height: 100px; border: 10px solid");
            if horizontal {
                document.set_attribute(list, "scroll-orientation", "horizontal");
            }
            let cells: Vec<_> = (0..2)
                .map(|_| cell(&mut document, list, "width: 30px; height: 30px"))
                .collect();
            document.layout();

            assert_eq!(
                rect(&document, list),
                (0.0, 0.0, 100.0, 100.0),
                "border-box sizing puts the border inside the declared size"
            );
            let offsets: Vec<_> = cells
                .iter()
                .map(|cell| {
                    let (x, y, width, height) = rect(&document, *cell);
                    assert_eq!((width, height), (30.0, 30.0));
                    (x, y)
                })
                .collect();
            let stacked = if horizontal {
                [(10.0, 10.0), (40.0, 10.0)]
            } else {
                [(10.0, 10.0), (10.0, 40.0)]
            };
            assert_eq!(offsets, stacked, "horizontal={horizontal}");
        }
    }

    // --- virtualization ---------------------------------------------------

    /// A 200x200 list of 40 labelled cells, each carrying an estimate that
    /// matches the height its label really occupies — Ahem's line box at
    /// `font-size: 20px` with `line-height: 20px` is exactly 20px tall — so a
    /// reveal never moves anything and every cell's geometry is known whether
    /// or not this render admitted it.
    const CELLS: usize = 40;
    const CELL_HEIGHT: f32 = 20.0;
    /// The content the 40 cells fill, and so the offset that scrolls the
    /// list past its own end and clamps to the bottom.
    const CELLS_EXTENT: f32 = 800.0;

    fn virtualized_list() -> (LynxDocument, NodeId, Vec<(NodeId, NodeId)>) {
        let (mut document, list) = list_page(
            "width: 200px; height: 200px; font-family: Ahem; font-size: 20px; line-height: 20px",
        );
        let cells = (0..CELLS)
            .map(|_| {
                let (cell, label) = labelled_cell(&mut document, list);
                set_attribute(
                    &mut document,
                    cell,
                    "estimated-main-axis-size-px",
                    Some("20"),
                );
                (cell, label)
            })
            .collect();
        (document, list, cells)
    }

    /// The whole point of the two rules: a cell the frame does not reach lays
    /// nothing out and stands at the size its estimate names, while a cell
    /// inside the window shapes its label.
    #[test]
    fn a_cell_past_the_window_skips_its_contents_and_stands_at_its_estimate() {
        let (mut document, list, cells) = virtualized_list();
        assert!(document.render(), "the first render commits a frame");

        let (head, head_label) = cells[0];
        assert!(
            document.text_block_size(head_label).is_some(),
            "the first cell is inside the window, so its label is shaped",
        );
        assert_eq!(rect(&document, head), (0.0, 0.0, 200.0, CELL_HEIGHT));

        let (tail, tail_label) = cells[CELLS - 1];
        assert!(
            document.text_block_size(tail_label).is_none(),
            "the last cell is far past the encode window, so nothing inside it laid out",
        );
        assert_eq!(
            rect(&document, tail),
            (0.0, CELLS_EXTENT - CELL_HEIGHT, 200.0, CELL_HEIGHT),
            "a skipped cell still has the box its estimate names",
        );

        // Scrolling to the end reveals the last cell in the same commit.
        document.scroll_to(list, Vector2D::new(0.0, CELLS_EXTENT));
        assert!(document.render(), "a scroll past the window owes a frame");
        assert!(
            document.text_block_size(tail_label).is_some(),
            "the reveal shapes the label the skip threw away",
        );
    }

    /// `recyclable="false"` is the opt-out, and it has to work from outside
    /// the window — that is the only place it is observable.
    #[test]
    fn a_non_recyclable_cell_never_skips() {
        let (mut document, _list, cells) = virtualized_list();
        let (tail, tail_label) = cells[CELLS - 1];
        document.set_attribute(tail, "recyclable", "false");
        assert!(document.render());

        assert_eq!(
            value(&document, tail, "content-visibility"),
            "visible",
            "the opt-out is a cascade fact, not a component one",
        );
        assert_eq!(value(&document, tail, "contain"), "none");
        assert!(
            document.text_block_size(tail_label).is_some(),
            "a non-recyclable cell lays its contents out wherever it stands",
        );
    }

    /// The `var()` fallback the whole estimate rests on: with no attribute a
    /// cell is one scrollport of the list it is in, and the attribute wins
    /// over it when there is one.
    #[test]
    fn the_cell_estimate_falls_back_to_one_scrollport_of_its_list() {
        let (mut document, list) = list_page("width: 300px; height: 200px");
        let bare = cell(&mut document, list, "");
        let estimated = cell(&mut document, list, "");
        set_attribute(
            &mut document,
            estimated,
            "estimated-main-axis-size-px",
            Some("42"),
        );
        document.layout();

        assert_eq!(
            value(&document, bare, "contain-intrinsic-height"),
            "auto 200px",
            "100cqh is the list's own content box, because the list is a size query container",
        );
        assert_eq!(
            value(&document, bare, "contain-intrinsic-width"),
            "none",
            "the cross axis has no estimate at all",
        );
        assert_eq!(
            value(&document, estimated, "contain-intrinsic-height"),
            "auto 42px",
            "the attribute outranks the fallback",
        );

        set_attribute(
            &mut document,
            estimated,
            "estimated-main-axis-size-px",
            None,
        );
        document.layout();
        assert_eq!(
            value(&document, estimated, "contain-intrinsic-height"),
            "auto 200px",
            "removing the attribute clears the hint and puts the fallback back",
        );
    }

    /// A horizontal list estimates along the inline axis instead, off
    /// `100cqw`.
    #[test]
    fn a_horizontal_lists_cell_estimates_along_the_inline_axis() {
        let (mut document, list) = list_page("width: 300px; height: 200px");
        document.set_attribute(list, "scroll-orientation", "horizontal");
        let bare = cell(&mut document, list, "");
        let estimated = cell(&mut document, list, "");
        set_attribute(
            &mut document,
            estimated,
            "estimated-main-axis-size-px",
            Some("42"),
        );
        document.layout();

        assert_eq!(
            value(&document, bare, "contain-intrinsic-width"),
            "auto 300px"
        );
        assert_eq!(value(&document, bare, "contain-intrinsic-height"), "none");
        assert_eq!(
            value(&document, estimated, "contain-intrinsic-width"),
            "auto 42px"
        );
    }

    // --- what generates a box inside a list -------------------------------

    #[test]
    fn only_a_cell_or_a_wrapper_generates_a_box_inside_a_list() {
        let (mut document, list) = list_page("width: 200px; height: 200px");
        let view = element_under(&mut document, list, "view", "");
        let image = element_under(&mut document, list, "image", "");
        let wrapper = element_under(&mut document, list, "wrapper", "");
        let nested = element_under(&mut document, wrapper, "list-item", "height: 30px");
        let plain = cell(&mut document, list, "height: 30px");
        document.layout();

        assert_eq!(display(&document, view), Display::None);
        assert_eq!(display(&document, image), Display::None);
        assert_eq!(
            display(&document, wrapper),
            Display::Contents,
            "a wrapper is exempt, or every ReactLynx cell would vanish with it",
        );
        assert_eq!(rect(&document, nested), (0.0, 0.0, 200.0, 30.0));
        assert_eq!(
            rect(&document, plain),
            (0.0, 30.0, 200.0, 30.0),
            "a hidden child takes no space, so the two cells are adjacent",
        );
    }

    // --- list-type placement ----------------------------------------------

    /// `flow` is CSS Grid over `--list-item-span-count` columns, and a
    /// full-span cell spans all of them.
    #[test]
    fn flow_places_cells_in_span_count_columns() {
        let (mut document, list) = list_page("width: 200px; height: 400px");
        document.set_attribute(list, "list-type", "flow");
        set_attribute(&mut document, list, "span-count", Some("2"));
        let cells: Vec<_> = (0..5)
            .map(|_| cell(&mut document, list, "height: 50px"))
            .collect();
        document.set_attribute(cells[2], "full-span", "true");
        set_attribute(
            &mut document,
            cells[2],
            "estimated-main-axis-size-px",
            Some("50"),
        );
        document.set_attribute(cells[3], "full-span", "false");
        assert!(document.render());

        assert_eq!(display(&document, list), Display::Grid);
        assert_eq!(rect(&document, cells[0]), (0.0, 0.0, 100.0, 50.0));
        assert_eq!(rect(&document, cells[1]), (100.0, 0.0, 100.0, 50.0));
        assert_eq!(
            rect(&document, cells[2]),
            (0.0, 50.0, 200.0, 50.0),
            "`full-span` spans every column",
        );
        assert_eq!(
            rect(&document, cells[3]),
            (0.0, 100.0, 100.0, 50.0),
            "`full-span=\"false\"` is the string ReactLynx sends for `false`, \
             so it must not be matched as a presence",
        );
        assert_eq!(rect(&document, cells[4]), (100.0, 100.0, 100.0, 50.0));
    }

    /// With no `span-count` a `flow` list is one column wide, because the UA
    /// default is one lane. web-core's `0` would be an invalid track list.
    #[test]
    fn the_span_count_default_is_one_lane() {
        let (mut document, list) = list_page("width: 200px; height: 400px");
        document.set_attribute(list, "list-type", "flow");
        let cells: Vec<_> = (0..2)
            .map(|_| cell(&mut document, list, "height: 50px"))
            .collect();
        assert!(document.render());

        assert_eq!(
            value(&document, list, "grid-template-columns"),
            "repeat(1, 1fr)",
            "the substituted count reaches the computed track list",
        );
        assert_eq!(rect(&document, cells[0]), (0.0, 0.0, 200.0, 50.0));
        assert_eq!(rect(&document, cells[1]), (0.0, 50.0, 200.0, 50.0));
    }

    /// `waterfall` is `display: grid-lanes` with `flow-tolerance: 0`, so each
    /// cell lands at the running end of the strictly shortest lane.
    #[test]
    fn waterfall_stacks_each_cell_into_the_shortest_lane() {
        let (mut document, list) = list_page("width: 200px; height: 400px");
        document.set_attribute(list, "list-type", "waterfall");
        set_attribute(&mut document, list, "span-count", Some("2"));
        let cells: Vec<_> = [30.0_f32, 50.0, 10.0, 25.0]
            .into_iter()
            .map(|height| cell(&mut document, list, &format!("height: {height}px")))
            .collect();
        assert!(document.render());

        assert_eq!(display(&document, list), Display::GridLanes);
        assert_eq!(rect(&document, cells[0]), (0.0, 0.0, 100.0, 30.0));
        assert_eq!(rect(&document, cells[1]), (100.0, 0.0, 100.0, 50.0));
        assert_eq!(
            rect(&document, cells[2]),
            (0.0, 30.0, 100.0, 10.0),
            "the first lane is 30 tall against the second's 50",
        );
        assert_eq!(
            rect(&document, cells[3]),
            (0.0, 40.0, 100.0, 25.0),
            "and still the shorter of the two at 40",
        );
    }

    /// A full-span waterfall cell is a definite span, which hughie's lanes
    /// honour: it starts below the longest lane and leaves all of them level.
    #[test]
    fn a_full_span_waterfall_cell_levels_every_lane() {
        let (mut document, list) = list_page("width: 200px; height: 400px");
        document.set_attribute(list, "list-type", "waterfall");
        set_attribute(&mut document, list, "span-count", Some("2"));
        let cells: Vec<_> = [30.0_f32, 50.0, 20.0, 15.0]
            .into_iter()
            .map(|height| cell(&mut document, list, &format!("height: {height}px")))
            .collect();
        document.set_attribute(cells[2], "full-span", "true");
        assert!(document.render());

        assert_eq!(rect(&document, cells[0]), (0.0, 0.0, 100.0, 30.0));
        assert_eq!(rect(&document, cells[1]), (100.0, 0.0, 100.0, 50.0));
        assert_eq!(rect(&document, cells[2]), (0.0, 50.0, 200.0, 20.0));
        assert_eq!(rect(&document, cells[3]), (0.0, 70.0, 100.0, 15.0));
    }

    /// The horizontal variants put the tracks on the block axis, which for
    /// `grid-lanes` only happens when the column template is `none` — the one
    /// place this sheet has to correct `x-list.css` rather than translate it.
    #[test]
    fn a_horizontal_list_type_puts_its_tracks_on_the_block_axis() {
        for list_type in ["flow", "waterfall"] {
            let (mut document, list) = list_page("width: 400px; height: 200px");
            document.set_attribute(list, "list-type", list_type);
            document.set_attribute(list, "scroll-orientation", "horizontal");
            set_attribute(&mut document, list, "span-count", Some("2"));
            let cells: Vec<_> = (0..4)
                .map(|_| cell(&mut document, list, "width: 40px"))
                .collect();
            assert!(document.render());

            assert_eq!(
                value(&document, list, "grid-template-columns"),
                "none",
                "{list_type}: otherwise grid-lanes would carry the tracks inline",
            );
            let rows = if list_type == "flow" {
                "repeat(2, 1fr)"
            } else {
                "repeat(2, minmax(0px, 1fr))"
            };
            assert_eq!(
                value(&document, list, "grid-template-rows"),
                rows,
                "{list_type}"
            );
            let first = rect(&document, cells[0]);
            let second = rect(&document, cells[1]);
            assert_eq!(
                first.0, 0.0,
                "{list_type}: the first cell starts at the left"
            );
            assert_eq!(
                second.0, 0.0,
                "{list_type}: the second is in the second row"
            );
            assert!(second.1 > 0.0, "{list_type}: rows, not columns");
            assert_eq!(
                rect(&document, cells[2]).0,
                40.0,
                "{list_type}: the third cell is one column along",
            );
            assert_eq!(rect(&document, cells[3]).0, 40.0, "{list_type}");
        }
    }

    // --- attribute mapping ------------------------------------------------

    #[test]
    fn span_count_and_column_count_both_map_to_the_lane_count() {
        for name in ["span-count", "column-count"] {
            let (mut document, list) = list_page("width: 200px; height: 400px");
            document.set_attribute(list, "list-type", "flow");
            for (written, expected) in [
                ("3", "3"),
                ("2.0", "2"),
                // `parseFloat` prefixes and a leading sign, like every other
                // Lynx numeric attribute.
                ("+4px", "4"),
                // Neither a fraction nor zero nor a negative can be a track
                // count, so each clears the hint and the UA default stands.
                ("2.5", "1"),
                ("0", "1"),
                ("-1", "1"),
                ("", "1"),
                ("auto", "1"),
            ] {
                set_attribute(&mut document, list, name, Some(written));
                document.layout();
                assert_eq!(
                    value(&document, list, "grid-template-columns"),
                    format!("repeat({expected}, 1fr)"),
                    "{name}=\"{written}\"",
                );
            }

            set_attribute(&mut document, list, name, None);
            document.layout();
            assert_eq!(
                value(&document, list, "grid-template-columns"),
                "repeat(1, 1fr)",
                "{name}: removal clears the hint",
            );
        }
    }

    #[test]
    fn sticky_offset_maps_to_a_pixel_length() {
        let (mut document, list) = list_page("width: 200px; height: 400px");
        let probe = cell(&mut document, list, "");

        set_attribute(&mut document, list, "sticky-offset", Some("12"));
        document.layout();
        assert_eq!(
            value(&document, probe, "--list-item-sticky-offset"),
            "12px",
            "the offset is written on the list and inherits to its cells",
        );

        set_attribute(&mut document, list, "sticky-offset", Some("nope"));
        document.layout();
        assert_eq!(
            value(&document, probe, "--list-item-sticky-offset"),
            "0px",
            "an unusable value clears the hint and leaves the UA default",
        );

        set_attribute(&mut document, list, "sticky-offset", Some("7.5"));
        document.layout();
        assert_eq!(
            value(&document, probe, "--list-item-sticky-offset"),
            "7.5px"
        );

        set_attribute(&mut document, list, "sticky-offset", None);
        document.layout();
        assert_eq!(value(&document, probe, "--list-item-sticky-offset"), "0px");
    }

    #[test]
    fn the_cell_estimate_attribute_maps_to_a_pixel_length() {
        let (mut document, list) = list_page("width: 200px; height: 200px");
        let cell = cell(&mut document, list, "");

        for (written, expected) in [
            ("120", "auto 120px"),
            ("120.5", "auto 120.5px"),
            ("1e2", "auto 100px"),
            // `parse_count` rejects a negative, and a negative intrinsic size
            // would be invalid anyway, so the fallback stands.
            ("-1", "auto 200px"),
            ("", "auto 200px"),
        ] {
            set_attribute(
                &mut document,
                cell,
                "estimated-main-axis-size-px",
                Some(written),
            );
            document.layout();
            assert_eq!(
                value(&document, cell, "contain-intrinsic-height"),
                expected,
                "estimated-main-axis-size-px=\"{written}\"",
            );
        }
    }

    /// The two components' contracts, which the tests above exercise through
    /// layout: each name belongs to exactly one of the two tags, and a name
    /// written on the other tag — or a name neither observes — reaches no
    /// declaration at all.
    #[test]
    fn the_list_tags_observe_their_own_attributes_and_nothing_else() {
        assert_eq!(
            CustomElement::<()>::observed_attributes(&List),
            vec![
                "span-count".to_owned(),
                "column-count".to_owned(),
                "sticky-offset".to_owned(),
            ],
        );
        assert_eq!(
            CustomElement::<()>::observed_attributes(&ListItem),
            vec!["estimated-main-axis-size-px".to_owned()],
        );

        let (mut document, list) = list_page("width: 200px; height: 200px");
        document.set_attribute(list, "list-type", "flow");
        let cell = cell(&mut document, list, "");

        // Each tag's own names, written on the other one.
        set_attribute(
            &mut document,
            list,
            "estimated-main-axis-size-px",
            Some("120"),
        );
        for name in ["span-count", "column-count", "sticky-offset"] {
            set_attribute(&mut document, cell, name, Some("2"));
        }
        // And a name neither observes: `list-type` is a selector rule, whose
        // rules name the `list` tag, so it selects nothing on a cell.
        let untouched = self::cell(&mut document, list, "");
        set_attribute(&mut document, cell, "list-type", Some("waterfall"));
        document.layout();

        assert_eq!(
            value(&document, cell, "contain-intrinsic-height"),
            "auto 200px",
            "a cell estimate written on the list is not the cell's own, and \
             the scrollport fallback stands",
        );
        assert_eq!(
            value(&document, list, "grid-template-columns"),
            "repeat(1, 1fr)",
            "a lane count written on a cell never reaches the list",
        );
        assert_eq!(
            value(&document, cell, "--list-item-sticky-offset"),
            "0px",
            "and a sticky offset written on a cell writes nothing, so the \
             list's inherited default stands",
        );
        assert_eq!(
            display(&document, cell),
            display(&document, untouched),
            "`list-type` selects a layout mode on a list, and nothing on a cell",
        );
    }
}
