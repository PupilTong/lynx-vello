//! The `scroll-coordinator` family: a collapsing header over a content slot.
//! A vertical drag that starts in a scroll container inside the slot folds
//! the header away first and scrolls the content after; the other way it
//! scrolls the content back to its start first and unfolds the header after.
//! A toolbar, when there is one, stays pinned over the top of the
//! coordinator and covers the bottom band of the folded header.
//!
//! Everything here is a UA sheet over machinery the engine already has, the
//! way [`super::viewpager`] is: the authored coordinator is the scroll
//! container, its header and slot are absolutely positioned boxes that read
//! each other's sizes through css-anchor-position-1's `anchor-size()`
//! (`docs/style-assumptions.md` §28), the toolbar is `position: sticky`, and
//! the fold order is the engine's `scroll-capture-y: nearest forward` on every
//! box inside the slot (`docs/style-assumptions.md` §25). The one component
//! is on the coordinator's two tags, and its only member is the
//! `setFoldExpanded` UI method ([`set_fold_expanded`]); the other eight tags
//! have no component.
//!
//! Translated from web-elements'
//! `lynx-stack/packages/web-platform/web-elements/src/elements/XFoldViewNg/x-foldview-ng.css`,
//! with the component code that sheet leans on replaced by CSS: web-core
//! writes the slot's `top` from a `ResizeObserver` on the header and the
//! toolbar (`XFoldviewNg.ts:27-38`), caps `scrollTop` at the header height
//! less the toolbar height (`:42-61`), and runs its own touch handler on the
//! slot to decide which box a drag moves (`XFoldviewSlotNgTouchEventsHandler.ts`).
//!
//! # Ten tags
//!
//! Native registers five: `scroll-coordinator`, `scroll-coordinator-header`,
//! `scroll-coordinator-toolbar`, `scroll-coordinator-slot` and
//! `scroll-coordinator-slot-drag` (Android's `@LynxBehavior(tagName = …)` in
//! `lynx_xelement_scroll_coordinator`'s `LynxUIScrollCoordinator.kt:47` and
//! `childitem/LynxUIScrollCoordinator{Header,Toolbar,Slot,SlotDrag}.kt`, iOS's
//! `lynx_xelement/scroll_coordinator/LynxUIScrollCoordinator*.m`, the typings'
//! `IntrinsicElements` in `lynx/js_libraries/types/types/common/element/element.d.ts:79-83`).
//! web-core has no map entry for them and passes its own names through
//! verbatim: `x-foldview-ng`, `x-foldview-header-ng`, `x-foldview-toolbar-ng`,
//! `x-foldview-slot-ng` and `x-foldview-slot-drag-ng`, which is what its e2e
//! cards write. This engine creates a tag as the bundle spells it, so all ten
//! reach it, and each rule below names both spellings of its role.
//!
//! # Geometry
//!
//! With `h_h` the header's height, `h_t` the toolbar's (0 without one) and
//! `H` the coordinator's scrollport height, all three references agree
//! (web-core's `scrollableLength`, Android's `totalScrollRange`, iOS's
//! `expandHeight`): the header sits at the top of the content and scrolls
//! with it, the toolbar is pinned at the top of the scrollport above the
//! header, the slot starts at `h_h` and is `H − h_t` tall, and the scroll
//! range is `h_h − h_t`. Folded, the toolbar covers exactly the header's
//! bottom band and the slot fills the rest of the scrollport.
//!
//! The rules reach it as follows. The coordinator is a column whose only
//! in-flow child is the toolbar (`position: sticky`, `h_t`). The header is
//! `position: absolute` at `top: 0` and carries `anchor-name:
//! --lynx-scroll-coordinator-header`; the toolbar carries
//! `--lynx-scroll-coordinator-toolbar`. The slot is `position: absolute` with
//! `top: anchor-size(--lynx-scroll-coordinator-header height, 0px)` and
//! `height: calc(100% - anchor-size(--lynx-scroll-coordinator-toolbar height,
//! 0px))`. A scroll container's scrollable overflow counts its own absolutely
//! positioned children, so the scroll size is `h_h + H − h_t` and the range
//! `h_h − h_t` without any cap; the sticky toolbar's containing block is the
//! scroller's content, so it stays pinned over the whole range.
//!
//! The coordinator is in [`super::ua_sheet`]'s common block, which gives it
//! `position: relative`, and `contain: content` here makes it a containing
//! block on its own as well: the header and the slot are laid out by it,
//! which is the only case the engine resolves `anchor-size()` in (§28). Only
//! absolutely positioned boxes may use the function (css-anchor-position-1
//! §5.1.1), which is why the slot is one where web-core's is in flow.
//!
//! The anchors must be *acceptable* to the slot (§2.3): the toolbar is in
//! flow, so it is acceptable wherever it is written; the header is absolutely
//! positioned in the same containing block, so it must come before the slot
//! in tree order. The documented structure (toolbar, header, slot) does; a
//! slot written before its header takes the `0px` fallback and sits at the
//! top.
//!
//! # Why six of the rules are `!important`
//!
//! The same test `docs/style-assumptions.md` §D.15 sets, and the same shape
//! of argument as the pager's row: web-core keeps each of these either in
//! its own `!important` or in component code no author rule reaches, and
//! native lays the boxes out itself. Here the authored boxes *are* the
//! layout, so a normal declaration would let an author CSS rule take the
//! coordinator apart.
//!
//! - The coordinator's `overflow-y: scroll` is web-core's own `!important` (`x-foldview-ng.css:8`):
//!   the coordinator is the scroll container, and an author `overflow: hidden` would leave nothing
//!   to fold.
//! - Its main axis, `flex-direction: column` and `linear-direction: column` (the two properties
//!   this grammar moves a flex or linear main axis with; `linear-orientation` does not parse). In a
//!   row the toolbar, the only in-flow child, would no longer span the coordinator's width but size
//!   to its content, and one without an author height would stretch to the coordinator's full
//!   height and leave the slot (`H` less that height) empty. The same argument as the pager's row,
//!   with the axis turned.
//! - The header's and the slot's `position: absolute`: the slot's `anchor-size()` resolves only in
//!   an absolutely positioned box, and the header must be one so it neither pushes the slot nor
//!   adds to the in-flow height. web-core's header is `position: absolute` too
//!   (`x-foldview-ng.css:61`).
//! - The toolbar's `position: sticky` (`x-foldview-ng.css:53`): without it the toolbar scrolls away
//!   with the header, and there is no fold to cover.
//! - `enable-scroll="false"` turns the coordinator's `overflow-y` to `hidden`, which has to be
//!   `!important` to beat the pinned `scroll`.
//!
//! `ua_sheet`'s pinned test lists all six lines.
//!
//! Plain author declarations can still take the geometry apart, and are not
//! pinned, the same way an author `scroll-snap-align` on a pager item is not:
//! a `top` on the slot (`top: auto` included) replaces its `anchor-size()`
//! offset, and an author `anchor-name` on the header or the toolbar renames
//! the anchor the slot reads, which then takes its `0px` fallback.
//!
//! # The fold order
//!
//! `scroll-coordinator-slot *, x-foldview-slot-ng * { scroll-capture-y:
//! nearest forward; }`: every box inside the slot defers a forward vertical
//! delta (one that increases the vertical offset) to its nearest ancestor
//! scroll container first. Boxes that are not scroll containers ignore the
//! property. A scroll container directly inside the slot therefore lets the
//! coordinator fold before it scrolls, and scrolls back to its start before
//! the coordinator unfolds. `nearest` nests outward-first, so a list inside a
//! scroll-view inside the slot goes forward in the order coordinator,
//! scroll-view, list; web-core's touch handler pairs the coordinator with
//! the innermost scroller that can still move in the drag's direction
//! (`XFoldviewSlotNgTouchEventsHandler.ts:75-93`). Only the vertical
//! longhand is set, so horizontal nesting inside the slot (a horizontal list
//! in a pager, say) stays inner first. A drag on content in the slot that is
//! not a scroll container latches the coordinator itself. The two descendant
//! selectors cost one ancestor-bloom-filter probe each per element outside a
//! slot.
//!
//! web-core also sets `overscroll-behavior-y: none` on a `scroll-view` inside
//! the slot (`x-foldview-ng.css:75-78`, to stop Safari's bounce). That is not
//! carried: here it would fence the chain and the coordinator would never
//! unfold from inside the content. For the same reason an author
//! `overscroll-behavior-y: contain` on a list inside the slot keeps the fold
//! from ever moving from that list.
//!
//! # What the attributes do
//!
//! - `enable-scroll="false"`, and `scroll-enable="false"` (iOS's alias, the spelling web-core
//!   reads): the coordinator is no longer user-scrollable (`overflow-y: hidden`), so the chain walk
//!   skips it and the fold stays where it is; the scroll containers inside the slot scroll
//!   themselves, and script can still scroll the coordinator. The three references disagree:
//!   web-core's rule is dead (`x-foldview-ng.css:32-34` loses to its own `overflow-y: scroll
//!   !important`, and its slot handler writes `scrollTop` regardless), Android swallows every
//!   vertical drag in the coordinator including those over inner lists, iOS disables its pan and
//!   keeps the inner content pinned while expanded. Recorded in `docs/tracking/deviations.md`.
//! - `bounces` (present and not `"false"`): `overscroll-behavior-y: contain-bounce`, the pager's
//!   precedent. Default off, as web-core has it; iOS bounces by default.
//! - `header-over-slot` (present and not `"false"`): the header paints above the slot (`z-index:
//!   1`), the documented and native meaning; the toolbar's `z-index: 2` keeps it above both.
//!   web-core raises the slot instead (`x-foldview-ng.css:66-69`), which contradicts its own docs
//!   and both platforms and changes nothing visible, since the slot already paints after the
//!   header. Recorded in `docs/tracking/deviations.md`.
//! - `enable-scroll-bar` / `scroll-bar-enable`: nothing; the engine draws no scrollbars.
//! - `granularity` and every event, `refresh-mode`, `tab-movable-enable`,
//!   `toolbar-interaction-enable`, `header-scrollview-enable`, `compat-container-popup` and every
//!   `ios-*` / `android-*` prop: not implemented.
//!
//! # The UI methods
//!
//! The coordinator's component ([`ScrollCoordinator`], defined on
//! `scroll-coordinator` and `x-foldview-ng`) answers one method through its
//! [`terminus::link::CustomElement::invoke`], which the runtime's `callElementMethod`
//! reaches through [`terminus::link::Document::invoke_element_method`]:
//! `setFoldExpanded({offset, smooth = true})` ([`set_fold_expanded`]), the one
//! method web-core defines on the coordinator (`XFoldviewNg.ts:63-72`).
//!
//! iOS also answers `getScrollInfo` (`{scrollX: 0, scrollY, scrollRange}`) and
//! `scrollBy({offset})` (what it consumed) on the coordinator
//! (`LynxUIScrollCoordinator.m:771-830`). Neither is in web-core or in the
//! docs, so neither is built, by the user's 2026-10-11 ruling on
//! `setDragGesture` (an undocumented native-only method stays unanswered).
//! `getScrollInfo` is code 3. `scrollBy` is not: the coordinator's kind
//! answers `NotFound` for it, so the base set's CSSOM-View `scrollBy`
//! ([`super::base_methods`]) answers instead, which reads `left`/`top` and no
//! `offset`, and answers code 0 with no data. That is what web-core answers
//! too, where `HTMLElement.prototype.scrollBy` takes the name. The base set's
//! `boundingClientRect`, `scrollIntoView` and `scrollTo` answer on the
//! coordinator as on every element.
//!
//! # `scroll-coordinator-slot-drag`
//!
//! The drag region gets nothing beyond [`super::ua_sheet`]'s common block (a
//! border box, `position: relative`, `defaultDisplayLinear`). Native's
//! `enable-drag="false"` excludes the region from driving the fold; web-core
//! implements nothing for the tag (`XFoldviewSlotDragNg.ts` is an empty
//! component). With the default, a drag on non-scrolling content anywhere in
//! the slot already latches the coordinator and folds it, so the tag has no
//! behaviour to express, and `enable-drag` (Android
//! `LynxUIScrollCoordinatorSlotDrag.kt:114`) is not implemented.
//!
//! # Where this deliberately leaves web-core and native
//!
//! Each is recorded in `docs/tracking/deviations.md`.
//!
//! - **A coordinator with `height: auto` collapses to its toolbar's height**: absolutely positioned
//!   children never size their parent. The coordinator's UA `height: 100%` covers a definite
//!   parent; the docs require the coordinator to be sized ("coordinator height == slot height +
//!   toolbar height").
//! - **`flex-grow` on the toolbar fills the coordinator**, being its only in-flow child; web-core
//!   shares the space between toolbar and slot.
//! - **A slot before its header** sits at the top (the `0px` fallback).
//! - **A slot taller than `H − h_t`** (an author height) extends the range past `h_h − h_t`;
//!   web-core and native cap the fold. An author `flex: 1` on the slot (the docs' recipe) is
//!   ignored, but the UA height gives the same length whenever the coordinator's height is
//!   definite.
//! - **Stray children of the coordinator are shown**; web-core `display: none`s every child that is
//!   not one of the three roles (`x-foldview-ng.css:36-50`), which needs `!important` on `display`,
//!   the `list-item` decision (`docs/tracking/deviations.md`).
//! - **A fling continues from the fold into the content**, as native does; web-core replaces a
//!   release with `scrollBy(deltaY * 4, smooth)` on the last box it scrolled, and skips it once the
//!   fold is complete.
//! - **`anchor-scope` is not implemented**, but targets are only the containing block's own
//!   children, so a coordinator nested in another's header or slot reads its own header and
//!   toolbar.
//! - No `scrollbar-width` / `::-webkit-scrollbar` rules (`:13-30`): nothing draws a scrollbar here.

use terminus::link::scroll::ScrollBehavior;
use terminus::link::{CustomElement, MethodCall, MethodError, MethodOutcome, NodeId, Vector2D};

use super::scroll_container::{clamp_to_range, object, offset_px};
use super::{InvalidParams, LynxDocument, is_truthy};

pub(super) const SCROLL_COORDINATOR_TAG: &str = "scroll-coordinator";
pub(super) const X_FOLDVIEW_TAG: &str = "x-foldview-ng";
pub(super) const SCROLL_COORDINATOR_HEADER_TAG: &str = "scroll-coordinator-header";
pub(super) const X_FOLDVIEW_HEADER_TAG: &str = "x-foldview-header-ng";
pub(super) const SCROLL_COORDINATOR_TOOLBAR_TAG: &str = "scroll-coordinator-toolbar";
pub(super) const X_FOLDVIEW_TOOLBAR_TAG: &str = "x-foldview-toolbar-ng";
pub(super) const SCROLL_COORDINATOR_SLOT_TAG: &str = "scroll-coordinator-slot";
pub(super) const X_FOLDVIEW_SLOT_TAG: &str = "x-foldview-slot-ng";
pub(super) const SCROLL_COORDINATOR_SLOT_DRAG_TAG: &str = "scroll-coordinator-slot-drag";
pub(super) const X_FOLDVIEW_SLOT_DRAG_TAG: &str = "x-foldview-slot-drag-ng";

pub(super) const TAGS: [&str; 10] = [
    SCROLL_COORDINATOR_TAG,
    X_FOLDVIEW_TAG,
    SCROLL_COORDINATOR_HEADER_TAG,
    X_FOLDVIEW_HEADER_TAG,
    SCROLL_COORDINATOR_TOOLBAR_TAG,
    X_FOLDVIEW_TOOLBAR_TAG,
    SCROLL_COORDINATOR_SLOT_TAG,
    X_FOLDVIEW_SLOT_TAG,
    SCROLL_COORDINATOR_SLOT_DRAG_TAG,
    X_FOLDVIEW_SLOT_DRAG_TAG,
];

pub(super) const UA_RULES: &str = r#"
scroll-coordinator, x-foldview-ng {
  width: 100%; height: 100%; contain: content;
  overflow-x: clip; overscroll-behavior-y: contain;
}
scroll-coordinator, x-foldview-ng { overflow-y: scroll !important; }
scroll-coordinator, x-foldview-ng { flex-direction: column !important; linear-direction: column !important; }
scroll-coordinator[enable-scroll="false"], scroll-coordinator[scroll-enable="false"], x-foldview-ng[enable-scroll="false"], x-foldview-ng[scroll-enable="false"] { overflow-y: hidden !important; }
scroll-coordinator[bounces]:not([bounces="false"]),
x-foldview-ng[bounces]:not([bounces="false"]) { overscroll-behavior-y: contain-bounce; }
scroll-coordinator-slot *, x-foldview-slot-ng * { scroll-capture-y: nearest forward; }
scroll-coordinator-header, x-foldview-header-ng {
  top: 0; left: 0; width: 100%;
  anchor-name: --lynx-scroll-coordinator-header;
}
scroll-coordinator-header, x-foldview-header-ng { position: absolute !important; }
scroll-coordinator-toolbar, x-foldview-toolbar-ng {
  top: 0; z-index: 2; flex: 0 0 auto;
  anchor-name: --lynx-scroll-coordinator-toolbar;
}
scroll-coordinator-toolbar, x-foldview-toolbar-ng { position: sticky !important; }
scroll-coordinator-slot, x-foldview-slot-ng {
  left: 0; width: 100%; contain: strict;
  top: anchor-size(--lynx-scroll-coordinator-header height, 0px);
  height: calc(100% - anchor-size(--lynx-scroll-coordinator-toolbar height, 0px));
}
scroll-coordinator-slot, x-foldview-slot-ng { position: absolute !important; }
scroll-coordinator[header-over-slot]:not([header-over-slot="false"]) > scroll-coordinator-header,
scroll-coordinator[header-over-slot]:not([header-over-slot="false"]) > wrapper > scroll-coordinator-header,
x-foldview-ng[header-over-slot]:not([header-over-slot="false"]) > x-foldview-header-ng,
x-foldview-ng[header-over-slot]:not([header-over-slot="false"]) > wrapper > x-foldview-header-ng { z-index: 1; }
"#;

pub(super) fn define(document: &mut LynxDocument) {
    document.define(SCROLL_COORDINATOR_TAG, Box::new(ScrollCoordinator));
    document.define(X_FOLDVIEW_TAG, Box::new(ScrollCoordinator));
}

struct ScrollCoordinator;

impl CustomElement<()> for ScrollCoordinator {
    fn invoke(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        if call.name != "setFoldExpanded" {
            return MethodOutcome::NotFound;
        }
        match set_fold_expanded(document, element, call.params) {
            Ok(()) => MethodOutcome::Done,
            Err(InvalidParams) => MethodOutcome::Failed(MethodError::InvalidParams),
        }
    }
}

pub(super) fn set_fold_expanded(
    document: &mut LynxDocument,
    coordinator: NodeId,
    params: &str,
) -> Result<(), InvalidParams> {
    let params = object(params)?;
    let offset = params
        .get("offset")
        .and_then(|offset| offset_px(document, offset))
        .ok_or(InvalidParams)?;
    let behavior = if params.get("smooth").is_none_or(is_truthy) {
        ScrollBehavior::Smooth
    } else {
        ScrollBehavior::Instant
    };
    let Some(scroll_box) = document.scroll_box(coordinator) else {
        return Ok(());
    };
    let target = clamp_to_range(offset, scroll_box.max_offset().y);
    document.scroll_to_with(
        coordinator,
        Vector2D::new(scroll_box.offset.x, target),
        behavior,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use terminus::link::scroll::ScrollBehavior;
    use terminus::link::stylo::properties::PropertyId;
    use terminus::link::stylo::values::computed::{Display, Overflow};
    use terminus::link::{
        MethodCall, MethodError, MethodOutcome, NodeId, StylesheetOrigin, Vector2D,
    };

    use super::super::test_support::{
        child, document, element_under, overflow, style_of, with_config,
    };
    use super::super::{LynxDocument, PageConfig};
    use super::{
        SCROLL_COORDINATOR_HEADER_TAG, SCROLL_COORDINATOR_SLOT_DRAG_TAG,
        SCROLL_COORDINATOR_SLOT_TAG, SCROLL_COORDINATOR_TAG, SCROLL_COORDINATOR_TOOLBAR_TAG, TAGS,
        X_FOLDVIEW_HEADER_TAG, X_FOLDVIEW_SLOT_DRAG_TAG, X_FOLDVIEW_SLOT_TAG, X_FOLDVIEW_TAG,
        X_FOLDVIEW_TOOLBAR_TAG,
    };

    #[derive(Debug, Clone, Copy)]
    struct Spelling {
        coordinator: &'static str,
        header: &'static str,
        toolbar: &'static str,
        slot: &'static str,
        slot_drag: &'static str,
    }

    const SPELLINGS: [Spelling; 2] = [
        Spelling {
            coordinator: SCROLL_COORDINATOR_TAG,
            header: SCROLL_COORDINATOR_HEADER_TAG,
            toolbar: SCROLL_COORDINATOR_TOOLBAR_TAG,
            slot: SCROLL_COORDINATOR_SLOT_TAG,
            slot_drag: SCROLL_COORDINATOR_SLOT_DRAG_TAG,
        },
        Spelling {
            coordinator: X_FOLDVIEW_TAG,
            header: X_FOLDVIEW_HEADER_TAG,
            toolbar: X_FOLDVIEW_TOOLBAR_TAG,
            slot: X_FOLDVIEW_SLOT_TAG,
            slot_drag: X_FOLDVIEW_SLOT_DRAG_TAG,
        },
    ];

    fn value(document: &LynxDocument, element: NodeId, property: &str) -> String {
        let id = PropertyId::parse_enabled_for_all_content(property)
            .unwrap_or_else(|()| panic!("unknown property `{property}`"));
        let declaration = id
            .as_shorthand()
            .err()
            .unwrap_or_else(|| panic!("`{property}` is a shorthand"));
        style_of(document, element).computed_value_to_string(declaration)
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

    struct Built {
        coordinator: NodeId,
        toolbar: Option<NodeId>,
        header: NodeId,
        slot: NodeId,
    }

    fn build(
        document: &mut LynxDocument,
        spelling: Spelling,
        style: &str,
        toolbar: Option<u32>,
        header: u32,
    ) -> Built {
        let coordinator = child(document, spelling.coordinator, style);
        let toolbar = toolbar.map(|height| {
            element_under(
                document,
                coordinator,
                spelling.toolbar,
                &format!("height: {height}px"),
            )
        });
        let header = element_under(
            document,
            coordinator,
            spelling.header,
            &format!("height: {header}px"),
        );
        let slot = element_under(document, coordinator, spelling.slot, "");
        Built {
            coordinator,
            toolbar,
            header,
            slot,
        }
    }

    fn assert_values(
        document: &LynxDocument,
        element: NodeId,
        tag: &str,
        expected: &[(&str, &str)],
    ) {
        for (property, expected) in expected {
            assert_eq!(
                value(document, element, property),
                *expected,
                "{tag} {property}"
            );
        }
    }

    #[test]
    fn the_coordinator_is_a_vertical_scroll_container_in_a_column() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "", None, 0);
            document.layout();

            let tag = spelling.coordinator;
            assert_eq!(
                overflow(&document, built.coordinator),
                (Overflow::Hidden, Overflow::Scroll),
                "the clipped x computes to `hidden` beside a scrolling y: {tag}"
            );
            assert_values(
                &document,
                built.coordinator,
                tag,
                &[
                    ("width", "100%"),
                    ("height", "100%"),
                    ("contain", "content"),
                    ("overscroll-behavior-y", "contain"),
                    ("overscroll-behavior-x", "auto"),
                    ("flex-direction", "column"),
                    ("linear-direction", "column"),
                    ("position", "relative"),
                    ("box-sizing", "border-box"),
                    ("z-index", "auto"),
                ],
            );
            assert_eq!(
                *style_of(&document, built.coordinator).get_display(),
                Display::Linear
            );
        }
    }

    #[test]
    fn the_header_is_absolute_at_the_top_and_names_its_anchor() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "", None, 0);
            document.layout();
            assert_values(
                &document,
                built.header,
                spelling.header,
                &[
                    ("position", "absolute"),
                    ("top", "0px"),
                    ("left", "0px"),
                    ("width", "100%"),
                    ("anchor-name", "--lynx-scroll-coordinator-header"),
                    ("z-index", "auto"),
                    ("box-sizing", "border-box"),
                ],
            );
            assert_eq!(
                *style_of(&document, built.header).get_display(),
                Display::Linear
            );
        }
    }

    #[test]
    fn the_toolbar_is_sticky_on_top_and_names_its_anchor() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "", Some(10), 0);
            document.layout();
            let toolbar = built.toolbar.expect("built with a toolbar");
            assert_values(
                &document,
                toolbar,
                spelling.toolbar,
                &[
                    ("position", "sticky"),
                    ("top", "0px"),
                    ("z-index", "2"),
                    ("flex-grow", "0"),
                    ("flex-shrink", "0"),
                    ("flex-basis", "auto"),
                    ("anchor-name", "--lynx-scroll-coordinator-toolbar"),
                ],
            );
            assert_eq!(*style_of(&document, toolbar).get_display(), Display::Linear);
        }
    }

    #[test]
    fn the_slot_is_absolute_below_the_header_and_reads_both_anchors() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "", None, 0);
            document.layout();
            assert_values(
                &document,
                built.slot,
                spelling.slot,
                &[
                    ("position", "absolute"),
                    ("left", "0px"),
                    ("width", "100%"),
                    ("contain", "strict"),
                    (
                        "top",
                        "anchor-size(--lynx-scroll-coordinator-header height, 0px)",
                    ),
                    (
                        "height",
                        "calc(100% - anchor-size(--lynx-scroll-coordinator-toolbar height, 0px))",
                    ),
                    ("scroll-capture-x", "auto"),
                    ("scroll-capture-y", "auto"),
                ],
            );
            assert_eq!(
                *style_of(&document, built.slot).get_display(),
                Display::Linear
            );
        }
    }

    #[test]
    fn everything_inside_the_slot_captures_forward() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "", None, 0);
            let scroller = element_under(&mut document, built.slot, "scroll-view", "");
            let deep = element_under(&mut document, scroller, "list", "");
            let wrapper = element_under(&mut document, built.slot, "wrapper", "");
            let in_wrapper = element_under(&mut document, wrapper, "view", "");
            let drag = element_under(&mut document, built.slot, spelling.slot_drag, "");
            let outside = child(&mut document, "scroll-view", "");
            document.layout();
            for (element, name) in [
                (scroller, "scroll-view"),
                (deep, "list"),
                (in_wrapper, "wrapped view"),
                (drag, spelling.slot_drag),
            ] {
                assert_eq!(
                    value(&document, element, "scroll-capture-y"),
                    "nearest forward",
                    "{} > {name}",
                    spelling.slot
                );
                assert_eq!(
                    value(&document, element, "scroll-capture-x"),
                    "auto",
                    "{} > {name}",
                    spelling.slot
                );
            }
            for element in [built.coordinator, built.header, outside] {
                assert_eq!(value(&document, element, "scroll-capture-y"), "auto");
                assert_eq!(value(&document, element, "scroll-capture-x"), "auto");
            }
        }
    }

    #[test]
    fn slot_drag_is_a_plain_container() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "", None, 0);
            let drag = element_under(&mut document, built.slot, spelling.slot_drag, "");
            document.layout();
            assert_values(
                &document,
                drag,
                spelling.slot_drag,
                &[
                    ("position", "relative"),
                    ("box-sizing", "border-box"),
                    ("width", "auto"),
                    ("height", "auto"),
                    ("z-index", "auto"),
                    ("anchor-name", "none"),
                ],
            );
            assert_eq!(overflow(&document, drag), (Overflow::Clip, Overflow::Clip));
            assert_eq!(*style_of(&document, drag).get_display(), Display::Linear);
        }
    }

    #[test]
    fn every_role_follows_default_display_linear_both_ways() {
        for (linear, expected) in [(true, Display::Linear), (false, Display::Flex)] {
            let mut document = with_config(PageConfig {
                default_display_linear: linear,
                ..PageConfig::default()
            });
            let elements: Vec<_> = TAGS
                .iter()
                .map(|tag| (*tag, child(&mut document, tag, "")))
                .collect();
            document.layout();
            for (tag, element) in elements {
                assert_eq!(
                    *style_of(&document, element).get_display(),
                    expected,
                    "{tag} linear={linear}"
                );
            }
        }
    }

    #[test]
    fn author_styles_cannot_undo_the_pinned_structure() {
        const SHEET: &str = "
            .c { overflow: hidden !important; flex-direction: row !important;
                 linear-direction: row !important; display: flex; }
            .h, .s { position: relative !important; }
            .t { position: static !important; }";
        for spelling in SPELLINGS {
            let mut document = document();
            document.add_stylesheet(SHEET, StylesheetOrigin::Author);
            let built = build(
                &mut document,
                spelling,
                "overflow: visible; flex-direction: row; linear-direction: row; height: 600px",
                Some(200),
                400,
            );
            let toolbar = built.toolbar.expect("built with a toolbar");
            document.add_class(built.coordinator, "c");
            document.add_class(built.header, "h");
            document.add_class(built.slot, "s");
            document.add_class(toolbar, "t");
            for element in [built.header, built.slot, toolbar] {
                document.set_inline_style(element, "position: static; height: 200px");
            }
            document.set_inline_style(built.header, "position: static; height: 400px");
            document.layout();

            assert_eq!(
                overflow(&document, built.coordinator),
                (Overflow::Hidden, Overflow::Scroll)
            );
            assert_eq!(
                value(&document, built.coordinator, "flex-direction"),
                "column"
            );
            assert_eq!(
                value(&document, built.coordinator, "linear-direction"),
                "column"
            );
            assert_eq!(value(&document, built.header, "position"), "absolute");
            assert_eq!(value(&document, built.slot, "position"), "absolute");
            assert_eq!(value(&document, toolbar, "position"), "sticky");
            assert_eq!(rect(&document, built.slot), (0.0, 400.0, 393.0, 200.0));
        }
    }

    #[test]
    fn inline_important_cannot_undo_the_pins() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "height: 600px", Some(200), 400);
            let toolbar = built.toolbar.expect("built with a toolbar");
            document.set_inline_style(
                built.coordinator,
                "height: 600px; overflow-y: visible !important; flex-direction: row !important",
            );
            document.set_inline_style(built.header, "height: 400px; position: static !important");
            document.set_inline_style(built.slot, "position: relative !important");
            document.set_inline_style(toolbar, "height: 200px; position: static !important");
            document.layout();

            assert_eq!(overflow(&document, built.coordinator).1, Overflow::Scroll);
            assert_eq!(
                value(&document, built.coordinator, "flex-direction"),
                "column"
            );
            assert_eq!(value(&document, built.header, "position"), "absolute");
            assert_eq!(value(&document, built.slot, "position"), "absolute");
            assert_eq!(value(&document, toolbar, "position"), "sticky");
            assert_eq!(rect(&document, built.slot), (0.0, 400.0, 393.0, 400.0));
        }
    }

    #[test]
    fn the_defaults_are_author_overridable() {
        let mut document = document();
        let built = build(
            &mut document,
            SPELLINGS[0],
            "width: 300px; height: 600px; overscroll-behavior-y: auto; contain: none",
            Some(100),
            300,
        );
        let toolbar = built.toolbar.expect("built with a toolbar");
        document.set_inline_style(toolbar, "height: 100px; z-index: 5");
        document.set_inline_style(built.slot, "height: 250px; left: 10px; width: 200px");
        document.layout();
        assert_eq!(
            value(&document, built.coordinator, "overscroll-behavior-y"),
            "auto"
        );
        assert_eq!(value(&document, built.coordinator, "contain"), "none");
        assert_eq!(value(&document, toolbar, "z-index"), "5");
        assert_eq!(rect(&document, built.slot), (10.0, 300.0, 200.0, 250.0));
    }

    #[test]
    fn enable_scroll_false_leaves_the_coordinator_to_script() {
        for attribute in ["enable-scroll", "scroll-enable"] {
            for spelling in SPELLINGS {
                for (written, expected) in [
                    ("false", Overflow::Hidden),
                    ("true", Overflow::Scroll),
                    ("", Overflow::Scroll),
                ] {
                    let mut document = document();
                    let built = build(&mut document, spelling, "", None, 0);
                    document.set_attribute(built.coordinator, attribute, written);
                    document.layout();
                    assert_eq!(
                        overflow(&document, built.coordinator),
                        (Overflow::Hidden, expected),
                        "{} {attribute}={written:?}",
                        spelling.coordinator
                    );
                }
            }
        }
    }

    #[test]
    fn bounces_stretches_the_fold_unless_it_says_false() {
        for (attribute, expected) in [
            (None, "contain"),
            (Some(""), "contain-bounce"),
            (Some("true"), "contain-bounce"),
            (Some("false"), "contain"),
        ] {
            for spelling in SPELLINGS {
                let mut document = document();
                let built = build(&mut document, spelling, "", None, 0);
                if let Some(attribute) = attribute {
                    document.set_attribute(built.coordinator, "bounces", attribute);
                }
                document.layout();
                assert_eq!(
                    value(&document, built.coordinator, "overscroll-behavior-y"),
                    expected,
                    "{} bounces={attribute:?}",
                    spelling.coordinator
                );
            }
        }
    }

    #[test]
    fn header_over_slot_raises_the_header() {
        for (attribute, expected) in [
            (None, "auto"),
            (Some(""), "1"),
            (Some("true"), "1"),
            (Some("false"), "auto"),
        ] {
            for spelling in SPELLINGS {
                for wrapped in [false, true] {
                    let mut document = document();
                    let coordinator = child(&mut document, spelling.coordinator, "");
                    let parent = if wrapped {
                        element_under(&mut document, coordinator, "wrapper", "")
                    } else {
                        coordinator
                    };
                    let header = element_under(&mut document, parent, spelling.header, "");
                    let slot = element_under(&mut document, parent, spelling.slot, "");
                    if let Some(attribute) = attribute {
                        document.set_attribute(coordinator, "header-over-slot", attribute);
                    }
                    document.layout();
                    let case = format!("{} {attribute:?} wrapped={wrapped}", spelling.coordinator);
                    assert_eq!(value(&document, header, "z-index"), expected, "{case}");
                    assert_eq!(value(&document, slot, "z-index"), "auto", "{case}");
                }
            }
        }
    }

    #[test]
    fn the_slot_sits_under_the_header_and_fills_below_the_toolbar() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "height: 600px", Some(200), 400);
            document.layout();
            let toolbar = built.toolbar.expect("built with a toolbar");
            assert_eq!(rect(&document, toolbar), (0.0, 0.0, 393.0, 200.0));
            assert_eq!(rect(&document, built.header), (0.0, 0.0, 393.0, 400.0));
            assert_eq!(rect(&document, built.slot), (0.0, 400.0, 393.0, 400.0));
            let scroll_box = document
                .scroll_box(built.coordinator)
                .expect("a scroll container");
            assert_eq!(scroll_box.max_offset(), Vector2D::new(0.0, 200.0));
            assert!(scroll_box.user_scrollable.y);
            assert!(!scroll_box.user_scrollable.x);
        }
    }

    #[test]
    fn without_a_toolbar_the_slot_fills_the_scrollport() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "height: 600px", None, 400);
            document.layout();
            assert_eq!(rect(&document, built.slot), (0.0, 400.0, 393.0, 600.0));
            let scroll_box = document
                .scroll_box(built.coordinator)
                .expect("a scroll container");
            assert_eq!(scroll_box.max_offset(), Vector2D::new(0.0, 400.0));
        }
    }

    #[test]
    fn a_wrapper_around_the_roles_changes_nothing() {
        let mut document = document();
        let coordinator = child(&mut document, SCROLL_COORDINATOR_TAG, "height: 600px");
        let wrapper = element_under(&mut document, coordinator, "wrapper", "");
        element_under(
            &mut document,
            wrapper,
            SCROLL_COORDINATOR_TOOLBAR_TAG,
            "height: 200px",
        );
        element_under(
            &mut document,
            wrapper,
            SCROLL_COORDINATOR_HEADER_TAG,
            "height: 400px",
        );
        let slot = element_under(&mut document, wrapper, SCROLL_COORDINATOR_SLOT_TAG, "");
        document.layout();
        assert_eq!(rect(&document, slot), (0.0, 400.0, 393.0, 400.0));
        assert_eq!(
            document.scroll_box(coordinator).map(|b| b.max_offset()),
            Some(Vector2D::new(0.0, 200.0))
        );
    }

    #[test]
    fn folded_the_toolbar_covers_the_header_and_the_slot_fills_the_rest() {
        let mut document = document();
        let built = build(&mut document, SPELLINGS[0], "height: 600px", Some(200), 400);
        document.layout();
        document.scroll_to(built.coordinator, Vector2D::new(0.0, 1000.0));
        document.layout();
        assert_eq!(
            document.scroll_offset(built.coordinator),
            Vector2D::new(0.0, 200.0)
        );
        let toolbar = document
            .bounding_client_rect(built.toolbar.expect("built with a toolbar"))
            .expect("a toolbar box");
        let slot = document
            .bounding_client_rect(built.slot)
            .expect("a slot box");
        assert_eq!((toolbar.origin.y, toolbar.size.height), (0.0, 200.0));
        assert_eq!((slot.origin.y, slot.size.height), (200.0, 400.0));
    }

    #[test]
    fn enable_scroll_false_is_not_user_scrollable() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "height: 600px", Some(200), 400);
            document.set_attribute(built.coordinator, "enable-scroll", "false");
            document.layout();
            let scroll_box = document
                .scroll_box(built.coordinator)
                .expect("still a scroll container");
            assert_eq!(scroll_box.max_offset(), Vector2D::new(0.0, 200.0));
            assert!(!scroll_box.user_scrollable.y);
        }
    }

    fn folding(spelling: Spelling) -> (LynxDocument, NodeId, NodeId) {
        let mut document = document();
        let built = build(&mut document, spelling, "height: 600px", Some(200), 400);
        let inner = element_under(
            &mut document,
            built.slot,
            "scroll-view",
            "width: 100%; height: 100%",
        );
        document.set_attribute(inner, "scroll-y", "");
        for _ in 0..10 {
            element_under(
                &mut document,
                inner,
                "view",
                "height: 100px; flex-shrink: 0",
            );
        }
        document.layout();
        (document, built.coordinator, inner)
    }

    #[test]
    fn a_scroll_in_the_slot_folds_first_and_unfolds_last() {
        for spelling in SPELLINGS {
            let (mut document, coordinator, inner) = folding(spelling);
            assert_eq!(
                document
                    .scroll_box(inner)
                    .expect("the inner scroller")
                    .max_offset(),
                Vector2D::new(0.0, 600.0)
            );
            let offsets = |document: &LynxDocument| {
                (
                    document.scroll_offset(coordinator).y,
                    document.scroll_offset(inner).y,
                )
            };

            document.scroll_chain(inner, Vector2D::new(0.0, 150.0));
            assert_eq!(offsets(&document), (150.0, 0.0), "{spelling:?}");
            document.scroll_chain(inner, Vector2D::new(0.0, 150.0));
            assert_eq!(offsets(&document), (200.0, 100.0), "{spelling:?}");
            document.scroll_chain(inner, Vector2D::new(0.0, -50.0));
            assert_eq!(offsets(&document), (200.0, 50.0), "{spelling:?}");
            document.scroll_chain(inner, Vector2D::new(0.0, -150.0));
            assert_eq!(offsets(&document), (100.0, 0.0), "{spelling:?}");
        }
    }

    #[test]
    fn enable_scroll_false_disables_the_fold() {
        let (mut document, coordinator, inner) = folding(SPELLINGS[1]);
        document.set_attribute(coordinator, "scroll-enable", "false");
        document.layout();
        document.scroll_chain(inner, Vector2D::new(0.0, 150.0));
        assert_eq!(document.scroll_offset(coordinator).y, 0.0);
        assert_eq!(document.scroll_offset(inner).y, 150.0);
    }

    fn invoke(
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        params: &str,
    ) -> MethodOutcome {
        document.invoke_element_method(element, MethodCall { name, params })
    }

    fn carried(
        document: &mut LynxDocument,
        coordinator: NodeId,
    ) -> Option<(Vector2D<f32>, ScrollBehavior)> {
        let frame = document.commit();
        let index = frame
            .slot_of(coordinator)
            .expect("the coordinator has a scroll slot");
        frame.scroll_slots()[index as usize]
            .request
            .map(|request| (request.target, request.behavior))
    }

    fn foldable(spelling: Spelling) -> (LynxDocument, NodeId) {
        let mut document = document();
        let built = build(&mut document, spelling, "height: 600px", Some(200), 400);
        document.layout();
        (document, built.coordinator)
    }

    #[test]
    fn both_coordinator_tags_are_defined_elements() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "", None, 0);
            document.layout();
            assert!(
                document
                    .matches(built.coordinator, ":defined")
                    .expect("a valid selector"),
                "{}",
                spelling.coordinator
            );
        }
    }

    #[test]
    fn set_fold_expanded_is_smooth_by_default() {
        for spelling in SPELLINGS {
            let (mut document, coordinator) = foldable(spelling);
            assert_eq!(
                value(&document, coordinator, "scroll-snap-type"),
                "none",
                "{spelling:?}"
            );
            assert_eq!(
                invoke(
                    &mut document,
                    coordinator,
                    "setFoldExpanded",
                    r#"{"offset":30}"#
                ),
                MethodOutcome::Done,
                "{spelling:?}"
            );
            assert_eq!(
                document.scroll_offset(coordinator),
                Vector2D::zero(),
                "{spelling:?}"
            );
            assert_eq!(
                carried(&mut document, coordinator),
                Some((Vector2D::new(0.0, 30.0), ScrollBehavior::Smooth)),
                "{spelling:?}"
            );
        }
    }

    #[test]
    fn set_fold_expanded_is_instant_for_a_falsy_smooth() {
        for (smooth, behavior) in [
            ("false", ScrollBehavior::Instant),
            ("0", ScrollBehavior::Instant),
            ("\"\"", ScrollBehavior::Instant),
            ("null", ScrollBehavior::Instant),
            ("true", ScrollBehavior::Smooth),
            ("1", ScrollBehavior::Smooth),
            ("\"no\"", ScrollBehavior::Smooth),
        ] {
            for spelling in SPELLINGS {
                let (mut document, coordinator) = foldable(spelling);
                let params = format!(r#"{{"offset":"30px","smooth":{smooth}}}"#);
                assert_eq!(
                    invoke(&mut document, coordinator, "setFoldExpanded", &params),
                    MethodOutcome::Done,
                    "{spelling:?} {params}"
                );
                let moved = if behavior == ScrollBehavior::Instant {
                    30.0
                } else {
                    0.0
                };
                assert_eq!(
                    document.scroll_offset(coordinator),
                    Vector2D::new(0.0, moved),
                    "{spelling:?} {params}"
                );
                assert_eq!(
                    carried(&mut document, coordinator),
                    Some((Vector2D::new(0.0, 30.0), behavior)),
                    "{spelling:?} {params}"
                );
            }
        }
    }

    #[test]
    fn set_fold_expanded_clamps_to_the_fold_range() {
        for spelling in SPELLINGS {
            let (mut document, coordinator) = foldable(spelling);
            assert_eq!(
                invoke(
                    &mut document,
                    coordinator,
                    "setFoldExpanded",
                    r#"{"offset":"1e9px"}"#
                ),
                MethodOutcome::Done
            );
            assert_eq!(
                carried(&mut document, coordinator),
                Some((Vector2D::new(0.0, 200.0), ScrollBehavior::Smooth)),
                "{spelling:?}"
            );
            for (params, expected) in [
                (r#"{"offset":1e308,"smooth":false}"#, 200.0),
                (r#"{"offset":-5,"smooth":false}"#, 0.0),
                (r#"{"offset":"-5px","smooth":false}"#, 0.0),
            ] {
                document.scroll_to(coordinator, Vector2D::new(0.0, 100.0));
                assert_eq!(
                    invoke(&mut document, coordinator, "setFoldExpanded", params),
                    MethodOutcome::Done,
                    "{spelling:?} {params}"
                );
                assert_eq!(
                    document.scroll_offset(coordinator),
                    Vector2D::new(0.0, expected),
                    "{spelling:?} {params}"
                );
            }
        }
    }

    #[test]
    fn set_fold_expanded_converts_rpx_and_reads_a_unitless_string() {
        for spelling in SPELLINGS {
            let (mut document, coordinator) = foldable(spelling);
            invoke(
                &mut document,
                coordinator,
                "setFoldExpanded",
                r#"{"offset":"10rpx","smooth":false}"#,
            );
            let offset = document.scroll_offset(coordinator).y;
            assert!((offset - 5.24).abs() < 1e-4, "{spelling:?} {offset}");
            invoke(
                &mut document,
                coordinator,
                "setFoldExpanded",
                r#"{"offset":"12.5","smooth":false}"#,
            );
            assert_eq!(
                document.scroll_offset(coordinator),
                Vector2D::new(0.0, 12.5),
                "{spelling:?}"
            );
        }
    }

    #[test]
    fn set_fold_expanded_refuses_a_missing_or_unreadable_offset() {
        for params in [
            "{}",
            r#"{"smooth":false}"#,
            r#"{"offset":"abc"}"#,
            r#"{"offset":"10vh"}"#,
            r#"{"offset":""}"#,
            r#"{"offset":null}"#,
            r#"{"offset":true}"#,
            r#"{"offset":[10]}"#,
            "null",
            "[1]",
            "30",
            "not json",
        ] {
            for spelling in SPELLINGS {
                let (mut document, coordinator) = foldable(spelling);
                document.scroll_to(coordinator, Vector2D::new(0.0, 30.0));
                assert_eq!(
                    invoke(&mut document, coordinator, "setFoldExpanded", params),
                    MethodOutcome::Failed(MethodError::InvalidParams),
                    "{spelling:?} {params}"
                );
                assert_eq!(
                    document.scroll_offset(coordinator),
                    Vector2D::new(0.0, 30.0),
                    "{spelling:?} {params}"
                );
                assert_eq!(
                    document.pending_scroll_request(coordinator),
                    None,
                    "{spelling:?} {params}"
                );
            }
        }
    }

    #[test]
    fn set_fold_expanded_scrolls_a_coordinator_the_user_cannot() {
        for attribute in ["enable-scroll", "scroll-enable"] {
            for spelling in SPELLINGS {
                let (mut document, coordinator) = foldable(spelling);
                document.set_attribute(coordinator, attribute, "false");
                document.layout();
                assert_eq!(
                    invoke(
                        &mut document,
                        coordinator,
                        "setFoldExpanded",
                        r#"{"offset":50,"smooth":false}"#,
                    ),
                    MethodOutcome::Done
                );
                assert_eq!(
                    document.scroll_offset(coordinator),
                    Vector2D::new(0.0, 50.0),
                    "{spelling:?} {attribute}"
                );
            }
        }
    }

    #[test]
    fn a_coordinator_with_no_box_moves_nothing() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "display: none", Some(200), 400);
            document.layout();
            assert_eq!(
                invoke(
                    &mut document,
                    built.coordinator,
                    "setFoldExpanded",
                    r#"{"offset":50}"#
                ),
                MethodOutcome::Done,
                "{spelling:?}"
            );
            assert_eq!(
                document.scroll_offset(built.coordinator),
                Vector2D::zero(),
                "{spelling:?}"
            );
            let frame = document.commit();
            assert_eq!(frame.slot_of(built.coordinator), None, "{spelling:?}");
            assert_eq!(
                document.pending_scroll_request(built.coordinator),
                None,
                "{spelling:?}"
            );
        }
    }

    #[test]
    fn the_coordinator_answers_set_fold_expanded_alone() {
        for spelling in SPELLINGS {
            let (mut document, coordinator) = foldable(spelling);
            for name in ["getScrollInfo", "scrollBy", "scrollTo", "selectTab"] {
                assert_eq!(
                    invoke(&mut document, coordinator, name, r#"{"offset":10}"#),
                    MethodOutcome::NotFound,
                    "{spelling:?} {name}"
                );
            }
            assert_eq!(document.pending_scroll_request(coordinator), None);
            assert_eq!(document.scroll_offset(coordinator), Vector2D::zero());
        }
    }

    #[test]
    fn the_other_roles_have_no_set_fold_expanded() {
        for spelling in SPELLINGS {
            let mut document = document();
            let built = build(&mut document, spelling, "height: 600px", Some(200), 400);
            document.layout();
            for element in [
                built.header,
                built.toolbar.expect("built with a toolbar"),
                built.slot,
            ] {
                assert_eq!(
                    invoke(
                        &mut document,
                        element,
                        "setFoldExpanded",
                        r#"{"offset":10}"#
                    ),
                    MethodOutcome::NotFound,
                    "{spelling:?}"
                );
            }
        }
    }
}
