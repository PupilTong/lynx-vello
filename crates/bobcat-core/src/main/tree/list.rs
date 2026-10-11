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
//! recycling, no threshold events, no `update-list-info` consumer —
//! `docs/tracking/components.md` rows 23-24 carry what is still missing.
//!
//! The component ([`List`]) adds four of the methods Lynx documents for the
//! tag (lynxjs.org/api/elements/built-in/list), which the runtime's
//! `callElementMethod` reaches through
//! [`dom::Document::invoke_element_method`]: `scrollToPosition`
//! ([`scroll_to_position`]), `scrollBy` (`scroll-view`'s own,
//! [`super::scroll_container::scroll_by`], on the list's axis),
//! `getScrollContainerInfo` ([`scroll_container_info`]) and
//! `getVisibleCells` ([`visible_cells`]). The list has no `scrollTo` of its
//! own, as web-core's `XList` has none, so the base set's CSSOM-View
//! `scrollTo` answers one ([`super::base_methods`]).
//!
//! The methods' references:
//!
//! - web-core `lynx-stack/packages/web-platform/web-elements/src/elements/XList/XList.ts`:
//!   `scrollToPosition` (`:70-116`), `autoScroll` (`:162-205`), `getScrollContainerInfo`
//!   (`:211-218`), `getVisibleCells` (`:220-250`). It has no `scrollBy`, so `HTMLElement.scrollBy`
//!   answers one.
//! - iOS `lynx/platform/darwin/ios/lynx/ui/list/container/LynxUIListContainer.mm`: `scrollBy`,
//!   `scrollToPosition` and `getVisibleCells` at `:1107-1215`, `visibleCellsInfo` at `:1704`; the
//!   older list's
//!   `lynx/platform/darwin/ios/lynx/ui/list/lynx_collection/LynxUICollection+Delegate.m` at
//!   `:323-480`.
//! - Android `lynx/platform/android/lynx_android/src/main/java/com/lynx/tasm/behavior/ui/list/`
//!   `container/UIListContainer.java:815-1042`: `getScrollInfo`, `scrollToPosition`, `scrollBy` and
//!   `getVisibleCells` (`:966-1002`).
//!
//! Each method reads the last completed layout and never flushes, as every
//! UI method does here. Not built, and so code 3: `autoScroll({rate, start,
//! autoStop})`, for the reason `scroll-view`'s is not
//! ([`super::scroll_container`]'s "Not built": a constant-rate scroll the
//! painter drives with a way to stop it does not exist), and `getScrollInfo`,
//! which only the natives answer on a list and the docs do not list, left
//! out as `setDragGesture` is (user ruling 2026-10-11).
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
//! `sticky-top="true"` sets `position: sticky`, `top` from the list's sticky
//! offset (default `0px`), and `z-index: 1` to paint above ordinary cells.
//! Horizontal sticky insets and `sticky-bottom` (`x-list.css:104-135`)
//! are still missing, along with `item-snap` /
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

use dom::scroll::{ScrollBehavior, ScrollBox};
use dom::stylo::values::computed::Display;
use dom::{CustomElement, MethodCall, MethodError, MethodOutcome, NodeId, Rect, Size2D, Vector2D};
use serde_json::{Map, Value};

use super::scroll_container::{
    along, clamp_to_range, object, offset_px, rect_in_scroller, scroll_by, with_axis,
};
use super::{InvalidParams, LynxDocument, element_child, element_children, is_truthy};
use crate::main::record::write_record_field;

/// The `list` and `list-item` policy, in `x-list.css`'s own order.
///
/// # Why a list is a size query container
///
/// `container-type: size` (`x-list.css:9`) is what makes
/// `contain-intrinsic-size: none auto 100cqh`
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
@property --list-item-span-count { syntax: "<integer>"; inherits: false; initial-value: 1; }
@property --list-item-sticky-offset { syntax: "<length>"; inherits: true; initial-value: 0px; }
list {
  overflow-x: clip; overflow-y: scroll;
  flex-direction: column; linear-direction: column;
  contain: layout; container-type: size;
  --list-item-span-count: attr(span-count number, attr(column-count number));
  --list-item-sticky-offset: attr(sticky-offset px, 0px);
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
  contain-intrinsic-size: none auto attr(estimated-main-axis-size-px px, 100cqh);
  flex: 0 0 auto;
}
list[scroll-orientation="horizontal"] list-item {
  contain-intrinsic-size: auto attr(estimated-main-axis-size-px px, 100cqw) none;
}
list-item[recyclable="false"] { content-visibility: visible; contain: none; }
list-item[sticky-top="true"] {
  position: sticky; top: max(0px, var(--list-item-sticky-offset)); z-index: 1;
}
list[list-type="flow"] {
  display: grid;
  grid-template-columns: repeat(max(1, var(--list-item-span-count)), 1fr);
  grid-auto-rows: min-content;
  justify-items: stretch; align-items: start;
}
list[list-type="flow"][scroll-orientation="horizontal"] {
  grid-template-columns: none;
  grid-template-rows: repeat(max(1, var(--list-item-span-count)), 1fr);
  grid-auto-flow: column; grid-auto-columns: min-content;
  justify-items: start; align-items: stretch;
}
list[list-type="flow"] list-item[full-span]:not([full-span="false"]) { grid-column: 1 / -1; }
list[list-type="flow"][scroll-orientation="horizontal"] list-item[full-span]:not([full-span="false"]) { grid-row: 1 / -1; }
list[list-type="waterfall"] {
  display: grid-lanes;
  grid-template-columns: repeat(max(1, var(--list-item-span-count)), minmax(0, 1fr));
  flow-tolerance: 0;
}
list[list-type="waterfall"][scroll-orientation="horizontal"] {
  grid-template-columns: none;
  grid-template-rows: repeat(max(1, var(--list-item-span-count)), minmax(0, 1fr));
}
list[list-type="waterfall"] list-item[full-span]:not([full-span="false"]) { grid-column: 1 / -1; }
list[list-type="waterfall"][scroll-orientation="horizontal"] list-item[full-span]:not([full-span="false"]) { grid-row: 1 / -1; }
"#;

/// The tag native registers and web-core defines as `x-list`.
pub(super) const LIST_TAG: &str = "list";

/// The cell tag: a list's element children of this tag are what
/// `getVisibleCells` reports and indexes.
const CELL_TAG: &str = "list-item";

/// Installs the `list` component. Must run before any element could carry
/// the tag, which is [`Document::define`](dom::Document::define)'s own
/// precondition.
pub(super) fn define(document: &mut LynxDocument) {
    document.define(LIST_TAG, Box::new(List));
}

/// The `list` component: `scrollToPosition`, `scrollBy`,
/// `getScrollContainerInfo` and `getVisibleCells`.
struct List;

impl CustomElement<()> for List {
    fn invoke(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        let answer = match call.name {
            "scrollToPosition" => scroll_to_position(document, element, call.params).map(|()| None),
            "scrollBy" => {
                let horizontal = is_horizontal(document, element);
                scroll_by(document, element, call.params, horizontal).map(|by| Some(by.record()))
            }
            "getScrollContainerInfo" => Ok(Some(scroll_container_info(document, element).record())),
            "getVisibleCells" => Ok(Some(cells_record(&visible_cells(document, element)))),
            _ => return MethodOutcome::NotFound,
        };
        match answer {
            Ok(None) => MethodOutcome::Done,
            Ok(Some(fields)) => MethodOutcome::Data(fields),
            Err(InvalidParams) => MethodOutcome::Failed(MethodError::InvalidParams),
        }
    }
}

/// Whether `list` scrolls x rather than y: `scroll-orientation="horizontal"`,
/// the one attribute [`UA_RULES`] select the horizontal rule by. A list has
/// no `scroll-x`.
fn is_horizontal(document: &LynxDocument, list: NodeId) -> bool {
    document
        .get(list)
        .is_some_and(|node| node.attribute("scroll-orientation") == Some("horizontal"))
}

/// `list`'s scroll geometry, or `None` when it has no box
/// ([`dom::Document::bounding_client_rect`] is `None`: `display: none`
/// itself or on an ancestor, or detached) or is no scroll container.
fn list_scroll_box(document: &LynxDocument, list: NodeId) -> Option<ScrollBox> {
    document.bounding_client_rect(list)?;
    document.scroll_box(list)
}

/// Whether `cell`, an element child of a list that has a box, generates one
/// itself: its computed `display` is neither `none` (which the UA sheet
/// gives every child that is no cell and no `wrapper`) nor `contents`.
fn generates_box(document: &LynxDocument, cell: NodeId) -> bool {
    document
        .get(cell)
        .and_then(dom::Node::computed_style)
        .is_some_and(|style| {
            let display = *style.get_display();
            display != Display::None && display != Display::Contents
        })
}

/// Where `scrollToPosition` puts the cell in the scrollport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlignTo {
    /// The cell's start edge at the scrollport's start edge.
    Top,
    /// The cell's centre at the scrollport's centre.
    Middle,
    /// The cell's end edge at the scrollport's end edge.
    Bottom,
}

/// `scrollToPosition({position, index?, itemKey?, offset = 0, alignTo =
/// "top", smooth = false})` on `list`, with `params` as the JSON text the
/// realm serialized.
///
/// - `params` must be a JSON object, else code 4.
/// - The cell: `itemKey`, when it is a string that equals the `item-key` attribute of one of the
///   list's element children, names the first such child, and `index` and `position` are then not
///   read, as both natives let a resolved `itemKey` win (iOS `LynxUIListContainer.mm:1107-1215`,
///   Android `UIListContainer.java:815-1042`); web-core ignores `itemKey`. Otherwise the number is
///   `index` when present, else `position`: the iOS list container reads `index` after `position`
///   as an alias, Android reads `index` first; here `index` is read first and `position` only when
///   there is no `index`. The number is truncated toward zero and names that element child of the
///   list ([`element_child`]), cell or not, as web-core's `children.item(position)` does.
/// - A missing number (with no `itemKey` match), a non-numeric one, a negative one or one past the
///   last element child is code 4 and moves nothing: this engine's index rule, which
///   `scroll-view`'s `scrollTo` and the pager's `selectTab` follow, and the iOS older list's answer
///   (`LynxUICollection+Delegate.m:323-480`). The iOS list container and Android answer code 8
///   (`OPERATION_ERROR`); web-core moves nothing and answers 0 (`XList.ts:89-107`); Android reads a
///   missing number as 0.
/// - `offset` is read as `scroll-view`'s `scrollTo` reads it ([`offset_px`]): a JSON number in CSS
///   px, or a string in `px`, `rpx` or `ppx` or with no unit; missing is 0, anything else is code
///   4. web-core `parseFloat`s a string and ignores its unit (`XList.ts:81-87`).
/// - `alignTo` is `"middle"` or `"bottom"` as both natives read them, and anything else, a missing
///   one included, is `"top"`. web-core ignores it. The target on the scrolling axis is the cell's
///   start in the list's scroll coordinates ([`rect_in_scroller`]); `middle` subtracts half of what
///   the scrollport has beyond the cell, `bottom` all of it. Then `offset` is added and the result
///   clamped to the scroll range. The cross axis keeps its offset, where web-core writes `offset`
///   to both axes (`XList.ts:84-87`).
/// - `smooth` is read with JavaScript truthiness, as web-core's `smooth ? 'smooth' : 'auto'` reads
///   it, and a missing one is `false` (Android's default). A smooth scroll is the painter's to
///   animate; an instant one moves the document at once ([`dom::Document::scroll_to_with`]). Either
///   way the call answers at once.
///
/// The params are read in full before any box is looked at. Then a list
/// with no box, or that is no scroll container, or a cell with no box (any
/// element child that is no cell and no `wrapper`, which the UA sheet hides,
/// or a `display: none` cell), moves nothing and answers code 0. web-core
/// reads such a cell's `offsetTop` as 0 and scrolls to `offset`; here
/// nothing moves.
pub(super) fn scroll_to_position(
    document: &mut LynxDocument,
    list: NodeId,
    params: &str,
) -> Result<(), InvalidParams> {
    let params = object(params)?;
    let offset = match params.get("offset") {
        None => 0.0,
        Some(offset) => offset_px(document, offset).ok_or(InvalidParams)?,
    };
    let cell = target_cell(document, list, &params)?;
    let align = match params.get("alignTo").and_then(Value::as_str) {
        Some("middle") => AlignTo::Middle,
        Some("bottom") => AlignTo::Bottom,
        _ => AlignTo::Top,
    };
    let behavior = if params.get("smooth").is_some_and(is_truthy) {
        ScrollBehavior::Smooth
    } else {
        ScrollBehavior::Instant
    };
    let horizontal = is_horizontal(document, list);
    let Some(scroll_box) = list_scroll_box(document, list) else {
        return Ok(());
    };
    if !generates_box(document, cell) {
        return Ok(());
    }
    let Some(rect) = rect_in_scroller(document, list, cell) else {
        return Ok(());
    };
    let start = along(rect.origin.to_vector(), horizontal);
    let slack = along(scroll_box.scrollport.to_vector(), horizontal)
        - along(rect.size.to_vector(), horizontal);
    let aligned = match align {
        AlignTo::Top => start,
        AlignTo::Middle => start - slack / 2.0,
        AlignTo::Bottom => start - slack,
    };
    let max = along(scroll_box.max_offset(), horizontal);
    let target = clamp_to_range(f64::from(aligned) + offset, max);
    document.scroll_to_with(
        list,
        with_axis(scroll_box.offset, horizontal, target),
        behavior,
    );
    Ok(())
}

/// The element child of `list` that `scrollToPosition`'s params name: the
/// first one whose `item-key` equals a string `itemKey`, else element child
/// number `index`, else number `position`, or code 4.
fn target_cell(
    document: &LynxDocument,
    list: NodeId,
    params: &Map<String, Value>,
) -> Result<NodeId, InvalidParams> {
    if let Some(key) = params.get("itemKey").and_then(Value::as_str) {
        let keyed = element_children(document, list).find(|child| {
            document
                .get(*child)
                .and_then(|node| node.attribute("item-key"))
                == Some(key)
        });
        if let Some(cell) = keyed {
            return Ok(cell);
        }
    }
    let number = params
        .get("index")
        .or_else(|| params.get("position"))
        .and_then(Value::as_f64)
        .ok_or(InvalidParams)?;
    element_child(document, list, number.trunc()).ok_or(InvalidParams)
}

/// What `getScrollContainerInfo` reports, in CSS px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrollContainerInfo {
    /// The scroll offset: `scrollLeft` and `scrollTop`.
    pub(super) offset: Vector2D<f32>,
    /// The scrolling area ([`ScrollBox::scroll_size`]): `scrollWidth` and
    /// `scrollHeight`.
    pub(super) scroll_size: Size2D<f32>,
}

impl ScrollContainerInfo {
    /// The data fields, in the order `element-papi.ts` reads them, which is
    /// web-core's: `scrollTop`, `scrollLeft`, `scrollHeight`, `scrollWidth`.
    fn record(self) -> String {
        let mut record = String::new();
        for value in [
            self.offset.y,
            self.offset.x,
            self.scroll_size.height,
            self.scroll_size.width,
        ] {
            write_record_field(&mut record, &value.to_string());
        }
        record
    }
}

/// `getScrollContainerInfo()` on `list`: web-core's
/// `{scrollTop, scrollLeft, scrollHeight, scrollWidth}` (`XList.ts:211-218`),
/// the CSSOM-View values of the list's scroll offset and scrolling area,
/// which is never smaller than the scrollport. Neither native has the
/// method. `params` is unread. A list with no box, or that is no scroll
/// container, answers zeros.
pub(super) fn scroll_container_info(document: &LynxDocument, list: NodeId) -> ScrollContainerInfo {
    list_scroll_box(document, list).map_or(
        ScrollContainerInfo {
            offset: Vector2D::zero(),
            scroll_size: Size2D::zero(),
        },
        |scroll_box| ScrollContainerInfo {
            offset: scroll_box.offset,
            scroll_size: scroll_box.scroll_size,
        },
    )
}

/// One cell `getVisibleCells` reports.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct VisibleCell {
    /// The cell's `id` attribute, empty when it carries none.
    pub(super) id: String,
    /// The cell's `item-key` attribute, empty when it carries none.
    pub(super) item_key: String,
    /// The cell's position among the list's `list-item` element children.
    pub(super) index: usize,
    /// The cell's border box relative to the list's padding-box origin less
    /// the scroll offset: where it stands in the scrollport.
    pub(super) rect: Rect<f32>,
}

/// `getVisibleCells()` on `list`: each `list-item` element child whose
/// border box overlaps the list's scrollport, in child order.
///
/// - A cell is a `list-item` element child of the list. A cell held in a `wrapper` is not reported,
///   and does not count toward `index`.
/// - "Overlaps" is a positive area in common: a cell whose edge only touches the scrollport's is
///   not reported. web-core's fallback test is inclusive of touching edges (`XList.ts:228-238`),
///   and its usual path reports every cell `content-visibility: auto` has found relevant, which is
///   a wider set than the scrollport; the natives report the cells their recycler holds in view.
/// - `index` is the cell's position among the list's `list-item` element children, counting cells
///   with no box and skipping children that are no cell, as web-core's `children.indexOf(cell)`
///   over the filtered list does.
/// - `left`, `top`, `right` and `bottom` are the border box in CSS px relative to the list's
///   padding-box origin less the scroll offset, which is the docs' "relative to the list" and what
///   iOS (`visibleCellsInfo`, `:1704`, each frame less the content offset) and Android
///   (`:966-1002`) report. web-core reports viewport coordinates (`getBoundingClientRect`).
/// - A cell whose contents `content-visibility: auto` skips still has a box, and is reported when
///   that box overlaps. A cell with no box is not.
///
/// A list with no box, or that is no scroll container, answers no cells.
/// `params` is unread.
pub(super) fn visible_cells(document: &LynxDocument, list: NodeId) -> Vec<VisibleCell> {
    let Some(scroll_box) = list_scroll_box(document, list) else {
        return Vec::new();
    };
    let scrollport = Rect::from_size(scroll_box.scrollport);
    element_children(document, list)
        .filter_map(|child| document.get(child))
        .filter(|node| node.tag_name() == Some(CELL_TAG))
        .enumerate()
        .filter_map(|(index, node)| {
            if !generates_box(document, node.id()) {
                return None;
            }
            let rect = rect_in_scroller(document, list, node.id())?.translate(-scroll_box.offset);
            rect.intersects(&scrollport).then(|| VisibleCell {
                id: node.attribute("id").unwrap_or_default().to_owned(),
                item_key: node.attribute("item-key").unwrap_or_default().to_owned(),
                index,
                rect,
            })
        })
        .collect()
}

/// The data fields of `getVisibleCells`, in the order `element-papi.ts`
/// reads them: the number of cells, then per cell `id`, `itemKey`, `index`,
/// `left`, `top`, `right`, `bottom`.
fn cells_record(cells: &[VisibleCell]) -> String {
    let mut record = String::new();
    write_record_field(&mut record, &cells.len().to_string());
    for cell in cells {
        write_record_field(&mut record, &cell.id);
        write_record_field(&mut record, &cell.item_key);
        write_record_field(&mut record, &cell.index.to_string());
        for value in [
            cell.rect.min_x(),
            cell.rect.min_y(),
            cell.rect.max_x(),
            cell.rect.max_y(),
        ] {
            write_record_field(&mut record, &value.to_string());
        }
    }
    record
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Ahem and explicit lane sizes have exact metrics.

    use dom::scroll::ScrollBehavior;
    use dom::stylo::computed_values::{flex_direction, linear_direction};
    use dom::stylo::properties::PropertyId;
    use dom::stylo::values::computed::{Display, Overflow};
    use dom::{MethodCall, MethodError, MethodOutcome, NodeId, Rect, Size2D, Vector2D};

    use super::super::LynxDocument;
    use super::super::test_support::{child, display, document, element_under, overflow, style_of};
    use super::{
        LIST_TAG, ScrollContainerInfo, VisibleCell, scroll_container_info, scroll_to_position,
        visible_cells,
    };

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
    /// alone. Stylo tracks the `attr()` dependency and recascades the rule.
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
            (*style.get_overflow_x(), *style.get_overflow_y()),
            (Overflow::Hidden, Overflow::Scroll),
            "the clipped axis computes to `hidden` beside a scrolling one"
        );
        assert_eq!(*style.get_flex_direction(), flex_direction::T::Column);
        assert_eq!(*style.get_linear_direction(), linear_direction::T::Column);

        let style = style_of(&document, horizontal);
        assert_eq!(
            (*style.get_overflow_x(), *style.get_overflow_y()),
            (Overflow::Scroll, Overflow::Hidden)
        );
        assert_eq!(*style.get_flex_direction(), flex_direction::T::Row);
        assert_eq!(*style.get_linear_direction(), linear_direction::T::Row);
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

    #[test]
    fn a_remembered_cell_size_takes_precedence_over_a_changed_estimate() {
        let (mut document, list, cells) = virtualized_list();
        let (head, label) = cells[0];
        document.render();
        assert_eq!(rect(&document, head).3, CELL_HEIGHT);

        document.scroll_to(list, Vector2D::new(0.0, CELLS_EXTENT));
        document.render();
        assert!(
            document.text_block_size(label).is_none(),
            "the head now skips"
        );
        document.set_attribute(head, "estimated-main-axis-size-px", "120");
        document.render();
        assert_eq!(
            rect(&document, head).3,
            CELL_HEIGHT,
            "a skipped cell retains the size it measured while visible",
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

    /// The intrinsic-size fallback: with no attribute a
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
            rect(&document, estimated).3,
            42.0,
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
            rect(&document, estimated).3,
            200.0,
            "removing the attribute restores the scrollport fallback",
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
        assert_eq!(rect(&document, estimated).2, 42.0);
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
                ("2.0", "1"),
                // A unit suffix is not a lane count.
                ("+4px", "1"),
                // Neither a fraction nor zero nor a negative can be a track
                // count, so the effective track count remains one.
                ("2.5", "1"),
                ("calc(2)", "1"),
                ("2 trailing", "1"),
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
                "{name}: removal restores the default",
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
            "an unusable value uses the UA default",
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
    fn span_count_precedes_column_count_independently_of_mutation_order() {
        let (mut document, list) = list_page("width: 200px; height: 400px");
        document.set_attribute(list, "list-type", "flow");
        for (name, input, expected) in [
            ("span-count", Some("2"), "2"),
            ("column-count", Some("3"), "2"),
            ("span-count", None, "3"),
            ("span-count", Some("4"), "4"),
            ("span-count", Some("bad"), "3"),
            ("span-count", Some("0"), "1"),
            ("column-count", None, "1"),
            ("span-count", None, "1"),
        ] {
            set_attribute(&mut document, list, name, input);
            document.layout();
            assert_eq!(
                value(&document, list, "grid-template-columns"),
                format!("repeat({expected}, 1fr)"),
                "{name}={input:?}",
            );
        }
    }

    #[test]
    fn sticky_offsets_require_numbers_and_ignore_negatives() {
        let (mut document, list) = list_page("width: 200px; height: 400px");
        let cell = cell(&mut document, list, "");
        document.set_attribute(cell, "sticky-top", "true");
        for (input, expected) in [
            (Some("12.5"), "12.5px"),
            (Some("-5"), "0px"),
            (Some("12px"), "0px"),
            (Some("12 trailing"), "0px"),
            (Some("calc(12)"), "0px"),
            (Some("7"), "7px"),
            (None, "0px"),
        ] {
            set_attribute(&mut document, list, "sticky-offset", input);
            document.layout();
            assert_eq!(value(&document, cell, "top"), expected, "{input:?}");
        }
    }

    #[test]
    fn cell_estimates_use_numbers_and_a_css_scrollport_fallback() {
        for horizontal in [false, true] {
            let (mut document, list) = list_page("width: 300px; height: 200px");
            if horizontal {
                document.set_attribute(list, "scroll-orientation", "horizontal");
            }
            let cell = cell(&mut document, list, "");
            let fallback: f32 = if horizontal { 300.0 } else { 200.0 };
            for (written, expected) in [
                (Some("120"), 120.0),
                (Some("120.5"), 120.5),
                (Some("1e2"), 100.0),
                (Some("0"), 0.0),
                (Some(""), fallback),
                (Some("12px"), fallback),
                (Some("calc(12)"), fallback),
                (Some("42"), 42.0),
                (None, fallback),
            ] {
                set_attribute(&mut document, cell, "estimated-main-axis-size-px", written);
                document.layout();
                let size = document.rounded_layout(cell).unwrap().size;
                assert_eq!(
                    if horizontal { size.width } else { size.height },
                    expected.round(),
                    "horizontal={horizontal}, estimate={written:?}",
                );
            }
        }
    }

    /// UA attribute rules only apply to the tag that owns the attribute.
    #[test]
    fn list_attribute_rules_are_scoped_to_their_tags() {
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
        // And a name neither numeric rule reads: `list-type` is a selector rule, whose
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

    // --- the component ------------------------------------------------------

    /// The cells of [`methods_list`], along the scrolling axis.
    const SIZES: [f32; 6] = [100.0, 80.0, 40.0, 80.0, 70.0, 100.0];

    /// A 200x200 list with a 10px border, so a 180px scrollport, of six cells
    /// [`SIZES`] long along the scrolling axis: they start at 0, 100, 180,
    /// 220, 300 and 370 in the list's scroll coordinates and end at 470, so
    /// the range is 290. `attribute` is written on the list before the
    /// layout.
    fn methods_list(attribute: Option<(&str, &str)>) -> (LynxDocument, NodeId, Vec<NodeId>) {
        let (mut document, list) = list_page("width: 200px; height: 200px; border: 10px solid");
        if let Some((name, value)) = attribute {
            document.set_attribute(list, name, value);
        }
        let horizontal = attribute == Some(("scroll-orientation", "horizontal"));
        let cells = SIZES
            .iter()
            .map(|size| {
                let style = if horizontal {
                    format!("width: {size}px")
                } else {
                    format!("height: {size}px")
                };
                cell(&mut document, list, &style)
            })
            .collect();
        document.layout();
        (document, list, cells)
    }

    fn invoke(
        document: &mut LynxDocument,
        list: NodeId,
        name: &str,
        params: &str,
    ) -> MethodOutcome {
        document.invoke_element_method(list, MethodCall { name, params })
    }

    /// The request the committed frame carries to the painter for `list`.
    fn carried(
        document: &mut LynxDocument,
        list: NodeId,
    ) -> Option<(Vector2D<f32>, ScrollBehavior)> {
        let frame = document.commit();
        let index = frame.slot_of(list).expect("the list has a slot");
        frame.scroll_slots()[index as usize]
            .request
            .map(|request| (request.target, request.behavior))
    }

    #[test]
    fn the_layout_the_method_tests_assume() {
        let (document, list, _) = methods_list(None);
        let scroll_box = document.scroll_box(list).expect("a scroll container");
        assert_eq!(scroll_box.scrollport, Size2D::new(180.0, 180.0));
        assert_eq!(scroll_box.max_offset(), Vector2D::new(0.0, 290.0));
        let (document, list, _) = methods_list(Some(("scroll-orientation", "horizontal")));
        let scroll_box = document.scroll_box(list).expect("a scroll container");
        assert_eq!(scroll_box.max_offset(), Vector2D::new(290.0, 0.0));
    }

    #[test]
    fn a_list_is_a_defined_element() {
        let (document, list, _) = methods_list(None);
        assert!(
            document
                .matches(list, ":defined")
                .expect("a valid selector")
        );
        assert_eq!(LIST_TAG, "list");
    }

    /// Cell 2 starts at 180 and is 40 long in a 180px scrollport: `top` puts
    /// its start at the scrollport's (180), `middle` its centre at the
    /// scrollport's (180 - 140 / 2), `bottom` its end at the scrollport's
    /// (180 - 140). Any other `alignTo` is `top`, and the target clamps.
    #[test]
    fn scroll_to_position_aligns_the_cell() {
        for (params, expected) in [
            (r#"{"position":2}"#, 180.0),
            (r#"{"position":2,"alignTo":"top"}"#, 180.0),
            (r#"{"position":2,"alignTo":"middle"}"#, 110.0),
            (r#"{"position":2,"alignTo":"bottom"}"#, 40.0),
            (r#"{"position":2,"alignTo":"Bottom"}"#, 180.0),
            (r#"{"position":2,"alignTo":2}"#, 180.0),
            (r#"{"position":3,"alignTo":"middle"}"#, 170.0),
            (r#"{"position":0,"alignTo":"bottom"}"#, 0.0),
            (r#"{"position":5}"#, 290.0),
        ] {
            let (mut document, list, _) = methods_list(None);
            assert_eq!(
                invoke(&mut document, list, "scrollToPosition", params),
                MethodOutcome::Done,
                "{params}"
            );
            assert_eq!(
                document.scroll_offset(list),
                Vector2D::new(0.0, expected),
                "{params}"
            );
        }
    }

    #[test]
    fn scroll_to_position_adds_the_offset_after_aligning() {
        for (params, expected) in [
            (r#"{"position":2,"offset":15}"#, 195.0),
            (r#"{"position":2,"offset":"15px"}"#, 195.0),
            (r#"{"position":2,"offset":"15"}"#, 195.0),
            (r#"{"position":2,"offset":-30,"alignTo":"bottom"}"#, 10.0),
            (r#"{"position":4,"offset":1000}"#, 290.0),
        ] {
            let (mut document, list, _) = methods_list(None);
            assert_eq!(
                scroll_to_position(&mut document, list, params),
                Ok(()),
                "{params}"
            );
            assert_eq!(
                document.scroll_offset(list),
                Vector2D::new(0.0, expected),
                "{params}"
            );
        }
        // `1rpx` is the viewport width over 750, 393px here.
        let (mut document, list, _) = methods_list(None);
        scroll_to_position(&mut document, list, r#"{"position":2,"offset":"10rpx"}"#)
            .expect("valid params");
        let offset = document.scroll_offset(list).y;
        assert!((offset - 185.24).abs() < 1e-3, "{offset}");
    }

    /// A string `itemKey` that names a cell wins over `index` and
    /// `position`; one that names none, or is no string, leaves them to
    /// name the cell.
    #[test]
    fn an_item_key_that_names_a_cell_wins() {
        for (params, expected) in [
            (r#"{"position":1,"itemKey":"k3"}"#, 220.0),
            (r#"{"index":1,"itemKey":"k3"}"#, 220.0),
            (r#"{"itemKey":"k3"}"#, 220.0),
            (r#"{"itemKey":"k3","position":"not read"}"#, 220.0),
            (r#"{"itemKey":"dup"}"#, 290.0),
            (r#"{"itemKey":"nope","position":1}"#, 100.0),
            (r#"{"itemKey":3,"position":1}"#, 100.0),
        ] {
            let (mut document, list, cells) = methods_list(None);
            document.set_attribute(cells[3], "item-key", "k3");
            document.set_attribute(cells[4], "item-key", "dup");
            document.set_attribute(cells[5], "item-key", "dup");
            assert_eq!(
                scroll_to_position(&mut document, list, params),
                Ok(()),
                "{params}"
            );
            assert_eq!(
                document.scroll_offset(list),
                Vector2D::new(0.0, expected),
                "the first `dup` starts at 300 and clamps to 290: {params}"
            );
        }
    }

    /// `index` is read first, and `position` only without one; the number is
    /// truncated toward zero.
    #[test]
    fn index_is_an_alias_read_before_position_and_truncated() {
        for (params, expected) in [
            (r#"{"index":3}"#, 220.0),
            (r#"{"index":3,"position":1}"#, 220.0),
            (r#"{"position":2.9}"#, 180.0),
            (r#"{"index":3.99}"#, 220.0),
            (r#"{"position":-0.5,"offset":5}"#, 5.0),
        ] {
            let (mut document, list, _) = methods_list(None);
            assert_eq!(
                scroll_to_position(&mut document, list, params),
                Ok(()),
                "{params}"
            );
            assert_eq!(
                document.scroll_offset(list),
                Vector2D::new(0.0, expected),
                "{params}"
            );
        }
    }

    #[test]
    fn scroll_to_position_refuses_bad_params_and_moves_nothing() {
        for params in [
            r#"{"position":6}"#,
            r#"{"position":-1}"#,
            r#"{"position":1e300}"#,
            r#"{"position":"1"}"#,
            r#"{"position":null}"#,
            r#"{"position":true}"#,
            r#"{"index":"3","position":1}"#,
            r#"{"index":-1,"position":1}"#,
            r#"{"itemKey":"nope"}"#,
            "{}",
            r#"{"offset":10}"#,
            r#"{"position":1,"offset":"abc"}"#,
            r#"{"position":1,"offset":"10vw"}"#,
            r#"{"position":1,"offset":null}"#,
            "null",
            "[1]",
            "1",
            "not json",
        ] {
            let (mut document, list, _) = methods_list(None);
            document.scroll_to(list, Vector2D::new(0.0, 30.0));
            assert_eq!(
                invoke(&mut document, list, "scrollToPosition", params),
                MethodOutcome::Failed(MethodError::InvalidParams),
                "{params}"
            );
            assert_eq!(
                document.scroll_offset(list),
                Vector2D::new(0.0, 30.0),
                "{params}"
            );
            assert_eq!(document.pending_scroll_request(list), None, "{params}");
        }
    }

    /// Instant by default; a truthy `smooth` is the painter's to animate, so
    /// only the request records the target.
    #[test]
    fn scroll_to_position_is_instant_unless_smooth_is_truthy() {
        for (smooth, behavior) in [
            ("false", ScrollBehavior::Instant),
            ("0", ScrollBehavior::Instant),
            ("true", ScrollBehavior::Smooth),
            ("\"yes\"", ScrollBehavior::Smooth),
        ] {
            let (mut document, list, _) = methods_list(None);
            let params = format!(r#"{{"position":2,"smooth":{smooth}}}"#);
            assert_eq!(
                scroll_to_position(&mut document, list, &params),
                Ok(()),
                "{params}"
            );
            let moved = if behavior == ScrollBehavior::Instant {
                180.0
            } else {
                0.0
            };
            assert_eq!(
                document.scroll_offset(list),
                Vector2D::new(0.0, moved),
                "{params}"
            );
            assert_eq!(
                carried(&mut document, list),
                Some((Vector2D::new(0.0, 180.0), behavior)),
                "{params}"
            );
        }
    }

    /// A horizontal list moves x, by the same arithmetic.
    #[test]
    fn a_horizontal_list_scrolls_to_a_position_along_x() {
        for (params, expected) in [
            (r#"{"position":2,"alignTo":"middle"}"#, 110.0),
            (r#"{"position":3,"offset":"10px"}"#, 230.0),
            (r#"{"position":5,"alignTo":"bottom"}"#, 290.0),
        ] {
            let (mut document, list, _) = methods_list(Some(("scroll-orientation", "horizontal")));
            assert_eq!(
                scroll_to_position(&mut document, list, params),
                Ok(()),
                "{params}"
            );
            assert_eq!(
                document.scroll_offset(list),
                Vector2D::new(expected, 0.0),
                "{params}"
            );
        }
    }

    /// `enable-scroll="false"` stops the user, not script: the axis is
    /// `overflow: hidden`, which a script scroll still moves.
    #[test]
    fn a_list_the_user_cannot_scroll_still_scrolls_from_script() {
        let (mut document, list, _) = methods_list(Some(("enable-scroll", "false")));
        assert_eq!(
            overflow(&document, list),
            (Overflow::Hidden, Overflow::Hidden)
        );
        assert_eq!(
            invoke(&mut document, list, "scrollToPosition", r#"{"position":2}"#),
            MethodOutcome::Done
        );
        assert_eq!(document.scroll_offset(list), Vector2D::new(0.0, 180.0));
        assert_eq!(
            invoke(&mut document, list, "scrollBy", r#"{"offset":50}"#),
            MethodOutcome::Data("1:02:502:501:0".to_owned())
        );
        assert_eq!(document.scroll_offset(list), Vector2D::new(0.0, 230.0));
    }

    /// A `display: none` list moves nothing and reports nothing, though its
    /// params are still read first.
    #[test]
    fn a_list_with_no_box_moves_nothing_and_reports_zeros() {
        let (mut document, list) = list_page("display: none; width: 200px; height: 200px");
        for _ in 0..3 {
            cell(&mut document, list, "height: 100px");
        }
        document.layout();
        assert_eq!(
            invoke(&mut document, list, "scrollToPosition", r#"{"position":2}"#),
            MethodOutcome::Done
        );
        assert_eq!(
            document.pending_scroll_request(list),
            None,
            "`scrollToPosition` records nothing"
        );
        assert_eq!(
            invoke(&mut document, list, "scrollToPosition", r#"{"position":3}"#),
            MethodOutcome::Failed(MethodError::InvalidParams),
            "the params are read before the box is looked at"
        );
        assert_eq!(
            invoke(&mut document, list, "scrollBy", r#"{"offset":10}"#),
            MethodOutcome::Data("1:01:02:102:10".to_owned()),
            "consumedX 0, consumedY 0, unconsumedX 10, unconsumedY 10"
        );
        assert_eq!(document.scroll_offset(list), Vector2D::zero());
        // `scrollBy` is `scroll-view`'s, which records its request to offset
        // 0 whether or not the scroller has a box; no frame carries it.
        let frame = document.commit();
        assert_eq!(frame.slot_of(list), None);
        assert_eq!(document.pending_scroll_request(list), None);
        assert_eq!(
            invoke(&mut document, list, "getScrollContainerInfo", "{}"),
            MethodOutcome::Data("1:01:01:01:0".to_owned())
        );
        assert_eq!(
            invoke(&mut document, list, "getVisibleCells", "{}"),
            MethodOutcome::Data("1:0".to_owned())
        );
    }

    /// An element child with no box, a non-cell the UA sheet hides or a
    /// `display: none` cell, is in range and moves nothing.
    #[test]
    fn a_child_with_no_box_moves_nothing() {
        let (mut document, list) = list_page("width: 200px; height: 200px");
        cell(&mut document, list, "height: 100px");
        element_under(&mut document, list, "view", "height: 100px");
        cell(&mut document, list, "display: none");
        cell(&mut document, list, "height: 300px");
        document.layout();
        document.scroll_to(list, Vector2D::new(0.0, 40.0));
        for position in [1, 2] {
            let params = format!(r#"{{"position":{position},"offset":5}}"#);
            assert_eq!(
                invoke(&mut document, list, "scrollToPosition", &params),
                MethodOutcome::Done,
                "{params}"
            );
            assert_eq!(
                document.scroll_offset(list),
                Vector2D::new(0.0, 40.0),
                "{params}"
            );
            assert_eq!(document.pending_scroll_request(list), None, "{params}");
        }
        assert_eq!(
            scroll_to_position(&mut document, list, r#"{"position":3}"#),
            Ok(())
        );
        assert_eq!(
            document.scroll_offset(list),
            Vector2D::new(0.0, 100.0),
            "the element children are counted, cell or not"
        );
    }

    /// `scrollBy` is `scroll-view`'s on the list's axis: the axis consumes
    /// what it moved, the cross axis nothing.
    #[test]
    fn scroll_by_answers_what_it_consumed_up_to_the_end() {
        let (mut document, list, _) = methods_list(None);
        assert_eq!(
            invoke(&mut document, list, "scrollBy", r#"{"offset":200}"#),
            MethodOutcome::Data("1:03:2003:2001:0".to_owned()),
            "consumedX 0, consumedY 200, unconsumedX 200, unconsumedY 0"
        );
        assert_eq!(
            invoke(&mut document, list, "scrollBy", r#"{"offset":200}"#),
            MethodOutcome::Data("1:02:903:2003:110".to_owned()),
            "the range ends at 290: consumedY 90, unconsumedY 110"
        );
        assert_eq!(document.scroll_offset(list), Vector2D::new(0.0, 290.0));
        assert_eq!(
            carried(&mut document, list),
            Some((Vector2D::new(0.0, 290.0), ScrollBehavior::Instant))
        );

        let (mut document, list, _) = methods_list(Some(("scroll-orientation", "horizontal")));
        assert_eq!(
            invoke(&mut document, list, "scrollBy", r#"{"offset":300}"#),
            MethodOutcome::Data("3:2901:02:103:300".to_owned()),
            "consumedX 290, consumedY 0, unconsumedX 10, unconsumedY 300"
        );
        assert_eq!(document.scroll_offset(list), Vector2D::new(290.0, 0.0));
    }

    #[test]
    fn scroll_by_refuses_params_without_a_numeric_offset() {
        for params in ["{}", r#"{"offset":"10"}"#, r#"{"offset":null}"#, "null"] {
            let (mut document, list, _) = methods_list(None);
            assert_eq!(
                invoke(&mut document, list, "scrollBy", params),
                MethodOutcome::Failed(MethodError::InvalidParams),
                "{params}"
            );
            assert_eq!(document.scroll_offset(list), Vector2D::zero(), "{params}");
            assert_eq!(document.pending_scroll_request(list), None, "{params}");
        }
    }

    #[test]
    fn get_scroll_container_info_answers_the_offset_and_the_scrolling_area() {
        let (mut document, list, _) = methods_list(None);
        assert_eq!(
            scroll_container_info(&document, list),
            ScrollContainerInfo {
                offset: Vector2D::zero(),
                scroll_size: Size2D::new(180.0, 470.0),
            }
        );
        document.scroll_to(list, Vector2D::new(0.0, 100.5));
        assert_eq!(
            invoke(&mut document, list, "getScrollContainerInfo", "not json"),
            MethodOutcome::Data("5:100.51:03:4703:180".to_owned()),
            "scrollTop, scrollLeft, scrollHeight, scrollWidth; params are not read"
        );

        let (mut document, list, _) = methods_list(Some(("scroll-orientation", "horizontal")));
        document.scroll_to(list, Vector2D::new(50.0, 0.0));
        assert_eq!(
            scroll_container_info(&document, list),
            ScrollContainerInfo {
                offset: Vector2D::new(50.0, 0.0),
                scroll_size: Size2D::new(470.0, 180.0),
            }
        );
    }

    /// [`methods_list`]'s cells, with a `view` after the first (hidden by the
    /// UA sheet, so no cell) and a `display: none` cell after the third,
    /// which no position moves. The first cell's `id` holds a comma and
    /// its `item-key` is `k0`; the second carries an empty `item-key` and
    /// no `id`.
    fn cells_list() -> (LynxDocument, NodeId) {
        let (mut document, list) = list_page("width: 200px; height: 200px; border: 10px solid");
        for (at, size) in SIZES.iter().enumerate() {
            let cell = cell(&mut document, list, &format!("height: {size}px"));
            match at {
                0 => {
                    document.set_attribute(cell, "id", "a,b");
                    document.set_attribute(cell, "item-key", "k0");
                    element_under(&mut document, list, "view", "height: 50px");
                }
                1 => document.set_attribute(cell, "item-key", ""),
                2 => {
                    self::cell(&mut document, list, "display: none");
                }
                _ => {}
            }
        }
        document.layout();
        (document, list)
    }

    fn visible(id: &str, item_key: &str, index: usize, rect: [f32; 4]) -> VisibleCell {
        let [x, y, width, height] = rect;
        VisibleCell {
            id: id.to_owned(),
            item_key: item_key.to_owned(),
            index,
            rect: Rect::new(dom::Point2D::new(x, y), Size2D::new(width, height)),
        }
    }

    /// At offset 0 the first two cells fill the 180px scrollport, and the
    /// third, whose start touches its end, is not visible.
    #[test]
    fn get_visible_cells_reports_the_cells_in_the_scrollport() {
        let (mut document, list) = cells_list();
        assert_eq!(
            visible_cells(&document, list),
            [
                visible("a,b", "k0", 0, [0.0, 0.0, 180.0, 100.0]),
                visible("", "", 1, [0.0, 100.0, 180.0, 80.0]),
            ]
        );
        assert_eq!(
            invoke(&mut document, list, "getVisibleCells", "{}"),
            MethodOutcome::Data(
                "1:23:a,b2:k01:01:01:03:1803:1000:0:1:11:03:1003:1803:180".to_owned()
            ),
            "the count, then id, itemKey, index, left, top, right, bottom per cell"
        );
    }

    /// After a 150px scroll the rects are relative to the scrollport, and
    /// `index` counts `list-item` children only: the hidden cell counts, the
    /// `view` does not.
    #[test]
    fn get_visible_cells_follows_the_scroll_offset() {
        let (mut document, list) = cells_list();
        document.scroll_to(list, Vector2D::new(0.0, 150.0));
        assert_eq!(
            visible_cells(&document, list),
            [
                visible("", "", 1, [0.0, -50.0, 180.0, 80.0]),
                visible("", "", 2, [0.0, 30.0, 180.0, 40.0]),
                visible("", "", 4, [0.0, 70.0, 180.0, 80.0]),
                visible("", "", 5, [0.0, 150.0, 180.0, 70.0]),
            ]
        );
    }

    #[test]
    fn a_horizontal_list_reports_cells_along_x() {
        let (mut document, list, _) = methods_list(Some(("scroll-orientation", "horizontal")));
        assert_eq!(
            visible_cells(&document, list),
            [
                visible("", "", 0, [0.0, 0.0, 100.0, 180.0]),
                visible("", "", 1, [100.0, 0.0, 80.0, 180.0]),
            ]
        );
        document.scroll_to(list, Vector2D::new(250.0, 0.0));
        assert_eq!(
            visible_cells(&document, list),
            [
                visible("", "", 3, [-30.0, 0.0, 80.0, 180.0]),
                visible("", "", 4, [50.0, 0.0, 70.0, 180.0]),
                visible("", "", 5, [120.0, 0.0, 100.0, 180.0]),
            ]
        );
    }

    /// A cell whose contents `content-visibility: auto` still skips has its
    /// box, and is reported when the box is in the scrollport.
    #[test]
    fn a_cell_that_skips_its_contents_is_still_visible() {
        let (mut document, list, cells) = virtualized_list();
        assert!(document.render());
        document.scroll_to(list, Vector2D::new(0.0, CELLS_EXTENT));
        let (_, tail_label) = cells[CELLS - 1];
        assert!(
            document.text_block_size(tail_label).is_none(),
            "no render has reached the tail since the scroll"
        );
        let reported = visible_cells(&document, list);
        assert_eq!(reported.len(), 10, "the last 200px of 20px cells");
        assert_eq!(
            reported.last(),
            Some(&visible(
                "",
                "",
                CELLS - 1,
                [0.0, 180.0, 200.0, CELL_HEIGHT]
            ))
        );
    }

    /// `autoScroll` is not built, `getScrollInfo` is left to the natives,
    /// `scrollTo` and the rest are the base set's or no one's.
    #[test]
    fn methods_not_built_are_not_found() {
        let (mut document, list, _) = methods_list(None);
        for name in [
            "autoScroll",
            "getScrollInfo",
            "selectTab",
            "scrollTo",
            "boundingClientRect",
            "scrollIntoView",
            "",
        ] {
            assert_eq!(
                invoke(&mut document, list, name, r#"{"rate":60,"start":true}"#),
                MethodOutcome::NotFound,
                "{name}"
            );
        }
        assert_eq!(document.pending_scroll_request(list), None);
    }
}
