//! The `viewpager` and `viewpager-item` tags: a row of pages, each one
//! scrollport wide, that the user swipes through one page at a time, and the
//! `selectTab` UI method that turns to a page from script.
//!
//! Everything the user can see is a UA sheet over machinery the engine
//! already has. The pager is a horizontal scroll container that snaps
//! `mandatory` on x (css-scroll-snap-1), its items are `100%` of its content
//! box and snap at their start with `scroll-snap-stop: always`, and the
//! painter's snap rules do the paging: a release under half a page glides
//! back, one past half glides on, a flick stops at the next page
//! (`crates/bobcat-core/src/paint/inertia.rs`). The pager's component
//! ([`Viewpager`]) has one member, the `selectTab` method ([`select_tab`]),
//! which the runtime's `callElementMethod` reaches through
//! [`dom::Document::invoke_element_method`]; nothing here reacts to an
//! attribute outside the cascade, and the item tags have no component.
//!
//! Translated from web-elements'
//! `lynx-stack/packages/web-platform/web-elements/src/elements/XViewpagerNg/x-viewpager-ng.css`
//! with its shadow tree dropped. web-core lays the pages out in a shadow
//! `#content` box (`htmlTemplates.ts:379-405`), whose `flex-direction: row`
//! no author rule on the host can reach, and scrolls that box. Here the
//! authored pager *is* the scroll container and lays its items out itself,
//! the way [`super::list`]'s authored `list` is its own grid.
//!
//! # Four tags
//!
//! Native registers `viewpager` and `viewpager-item`
//! (`LYNX_LAZY_REGISTER_UI` in iOS's `LynxUIViewPagerAutoRegistry.m:11,17`,
//! `@LynxBehavior(tagName = ["viewpager"])` and `["viewpager-item"]` in
//! Android's `LynxUIViewPager{,Item}AutoRegistry.kt:12`, `map["viewpager"]` and
//! `map["viewpager-item"]` in Harmony's `lynx_xelement/registry.cc:40-41`).
//! web-core maps both onto `x-viewpager-ng` / `x-viewpager-item-ng`
//! (`web-core/ts/constants.ts:108-109`) and also accepts those two names
//! verbatim (`:137-138`), which is what the e2e cards write. This engine
//! creates a tag as the bundle spells it, so all four reach it, and each rule
//! below names both spellings of its tag.
//!
//! # The pages always form one row
//!
//! In both references the author cannot turn the pages into a column. web-core
//! keeps the row in its shadow `#content`; native places the pages itself. Yet
//! authors do write a main axis on the pager: the e2e cards style it
//! `display: flex; flex-direction: column` or `display: linear`
//! (`web-core-e2e/tests/reactlynx/basic-element-x-viewpager-ng-*/index.css`),
//! and a card relies on still getting a row. With no shadow box, the only
//! place left to keep the row is the cascade, so the pager's main axis is
//! pinned with UA `!important`: `flex-direction: row` for `display: flex`,
//! `linear-direction: row` for `display: linear`, and `flex-wrap: nowrap`,
//! which would otherwise stack each full-width page on a line of its own.
//! Those three are every property this grammar lets an author move a flex or
//! linear main axis with — `flex-flow` is their shorthand, and the deprecated
//! `linear-orientation` does not parse here (`docs/tracking/deviations.md`,
//! "Deprecated CSS properties/values are dropped"). The item's
//! `position: relative` is pinned the same way, as web-core pins it
//! (`x-viewpager-ng.css:63`): an item an author positions absolutely would
//! leave the row (the e2e card `item-position-absolute` does exactly that).
//! Each is a structural invariant rather than a default, which is the test
//! `docs/style-assumptions.md` §D.15 sets for an important UA declaration;
//! [`super::ua_sheet`]'s pinned test lists them. `direction: rtl` still
//! reverses the row; right-to-left paging is out of scope.
//!
//! # What the attributes do
//!
//! - `enable-scroll="false"` and `allow-horizontal-gesture="false"` leave the pager a scroll
//!   container only script can move (`overflow-x: hidden`), as web-core does (`:30-33`). Native
//!   treats the two as one switch too (iOS `LynxUIViewPager.m:728-739`, Harmony
//!   `ui_viewpager.cc:46-49`).
//! - `bounces` (present and not `"false"`) is `overscroll-behavior-x: contain-bounce`: past the
//!   first and the last page the pager stretches on the rubber band and springs back. web-core
//!   stretches the leading edge only — its `bounces` shows a page-wide blank box ahead of the pages
//!   (`x-viewpager-ng.css:43-45`, `htmlTemplates.ts:380-386`) and there is none after them — while
//!   native bounces both edges (iOS hands the attribute to `UIScrollView.bounces`,
//!   `LynxUIViewPager.m:712`; Harmony to `ARKUI_EDGE_EFFECT_SPRING`, `ui_viewpager.cc:55-59`).
//!   Without the attribute nothing bounces, which is web-core's default and Harmony's. Recorded in
//!   `docs/tracking/deviations.md`.
//! - `select-index`, else `initial-select-index` — web-core's order, `getAttribute('select-index')
//!   || getAttribute('initial-select-index')` (`XViewpagerNg.ts:38-39`) — names the page the pager
//!   starts on. See below.
//!
//! # The initial page is CSS
//!
//! Three rules and no component code (`docs/style-assumptions.md` §27 for what `if()` and
//! `sibling-index()` do here). The pager sets a registered, inherited `<integer>`,
//! `--viewpager-initial-index`, from the typed `attr()` of its two attributes, `-1` when neither
//! is an integer; each page is its container's `scroll-initial-target` exactly when that number
//! equals its own `sibling-index() - 1`. css-scroll-snap-2's initial target then does the rest
//! (`crates/dom/src/scroll/initial_target.rs`): the first layout with a target scrolls the pager
//! to it, instantly, in the same commit, and the frame carries the position to the painter as an
//! instant scroll request.
//!
//! Because `scroll-initial-target` honours each *new* target once, this is not web-core's
//! read-once-at-connect attribute:
//!
//! - Changing the attribute in force after the first layout names a new target, and the pager turns
//!   to it, instantly, over whatever offset the user left it at.
//! - Inserting or removing a page ahead of the target makes another page the target, and the pager
//!   turns to that one.
//! - A commit that leaves the target where it was moves nothing, so a user who swiped away stays.
//! - `sibling-index()` counts the children of the page's own parent: pages wrapped one by one in
//!   `wrapper` all count as index 0, and pages sharing one `wrapper` count as without it.
//! - A present `select-index` that is not an integer (`"abc"`, `"1.5"`, `""`) does not fall back to
//!   `initial-select-index` (the fork's typed `attr()` fallback gap, §27): the first page. web-core
//!   shows the first page for `"abc"` (`NaN`), lands between pages for `"1.5"`, and falls back for
//!   `""`.
//! - A negative or out-of-range index names no page: the first page.
//! - Every pager sets the property from its own attributes, so an inner pager does not inherit an
//!   outer one's index.
//! - A `display: none` pager has no scroll slot, so its target waits for the first commit that
//!   shows it. A pager zero pixels wide at its first commit honours its target at offset 0 and, the
//!   target being honoured, stays on the first page once it gets a width; web-core retries every
//!   animation frame until it has one. Pages that arrive after the first layout bring the target
//!   with them.
//!
//! All of these are recorded in `docs/tracking/deviations.md`.
//!
//! # Where this deliberately leaves web-core
//!
//! - **An item outside a pager is an ordinary container**, where web-core hides every
//!   `x-viewpager-item-ng` that is not a pager's child or grandchild through a `lynx-wrapper`
//!   (`:6-14`). That is the decision `list-item` records (see [`super::list`]): the alternative
//!   needs `!important` on `display`, which is a default here, not an invariant. The pager's own
//!   `display` follows `defaultDisplayLinear` like every other container tag.
//! - **No `contain: strict` from the fifth item on** (`:66-68`): a browser paint and layout
//!   shortcut, not a behavior; the engine's own culling needs no hint.
//! - `scrollbar-width` and `::-webkit-scrollbar` (`:23,35-37`) hide a scrollbar this engine does
//!   not draw, and `scroll-snap-stop` on the pager itself (`:27`) applies to snap areas only, so
//!   all three are left out.

use dom::scroll::ScrollBehavior;
use dom::{CustomElement, MethodCall, MethodError, MethodOutcome, NodeId, Vector2D};
use serde_json::Value;

use super::{LynxDocument, is_truthy};

/// Native's pager tag, and the one `x-viewpager-ng` stands for in web-core.
pub(super) const VIEWPAGER_TAG: &str = "viewpager";
pub(super) const X_VIEWPAGER_TAG: &str = "x-viewpager-ng";
/// Native's page tag, and the one `x-viewpager-item-ng` stands for in web-core.
pub(super) const VIEWPAGER_ITEM_TAG: &str = "viewpager-item";
pub(super) const X_VIEWPAGER_ITEM_TAG: &str = "x-viewpager-item-ng";

/// The `viewpager` and `viewpager-item` policy, in `x-viewpager-ng.css`'s
/// own order; see the module documentation for what each rule is for.
///
/// Each important declaration sits on a line of its own, because
/// [`super::ua_sheet`]'s pinned test reads the sheet line by line.
pub(super) const UA_RULES: &str = r#"
@property --viewpager-initial-index { syntax: "<integer>"; inherits: true; initial-value: -1; }
viewpager, x-viewpager-ng {
  width: 100%; height: 100%; contain: content;
  overflow-x: scroll; overflow-y: clip;
  scroll-snap-type: x mandatory;
  --viewpager-initial-index:
    attr(select-index type(<integer>), attr(initial-select-index type(<integer>), -1));
}
viewpager, x-viewpager-ng { flex-direction: row !important; linear-direction: row !important; flex-wrap: nowrap !important; }
viewpager[allow-horizontal-gesture="false"], viewpager[enable-scroll="false"],
x-viewpager-ng[allow-horizontal-gesture="false"], x-viewpager-ng[enable-scroll="false"] { overflow-x: hidden; }
viewpager[bounces]:not([bounces="false"]),
x-viewpager-ng[bounces]:not([bounces="false"]) { overscroll-behavior-x: contain-bounce; }
viewpager-item, x-viewpager-item-ng {
  width: 100%; height: 100%; contain: content; flex: 0 0 auto;
  scroll-snap-align: start; scroll-snap-stop: always;
  scroll-initial-target:
    if(style(--viewpager-initial-index: calc(sibling-index() - 1)): nearest; else: none);
}
viewpager-item, x-viewpager-item-ng { position: relative !important; }
"#;

/// Installs the pager component under both of its tag names. Must run before
/// any element could carry either tag, which is
/// [`Document::define`](dom::Document::define)'s own precondition.
pub(super) fn define(document: &mut LynxDocument) {
    document.define(VIEWPAGER_TAG, Box::new(Viewpager));
    document.define(X_VIEWPAGER_TAG, Box::new(Viewpager));
}

/// The pager component: the `selectTab` method alone.
struct Viewpager;

impl CustomElement<()> for Viewpager {
    fn invoke(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        match call.name {
            "selectTab" => match select_tab(document, element, call.params) {
                Ok(()) => MethodOutcome::Done,
                Err(InvalidParams) => MethodOutcome::Failed(MethodError::InvalidParams),
            },
            _ => MethodOutcome::NotFound,
        }
    }
}

/// `selectTab`'s params were not `{index: <number>, …}`: the UI-method
/// status table's code 4, `PARAM_INVALID`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct InvalidParams;

/// `selectTab({index, smooth = true})` on `pager`, with `params` as the JSON
/// text the realm serialized.
///
/// The target is `index` times the scrollport width, which is web-core's
/// formula (`XViewpagerNg.ts:26-35`): a fractional index lands between two
/// pages and the pager's `mandatory` snapping settles it, and an index past
/// either end clamps to the scroll range, where native answers an error
/// (Android `LynxUIViewPager.kt:151,168-170`, Harmony
/// `ui_viewpager.cc:229-235`). The width is the last completed layout's: a
/// UI method never flushes. A missing `index`, or one that is not a JSON
/// number — `NaN` and the infinities serialize as `null` — is refused with
/// nothing moved, which is Android's and Harmony's answer
/// (`LynxUIViewPager.kt:172-174`, `ui_viewpager.cc:224-227`); web-core
/// multiplies whatever it is given and scrolls to 0 for a `NaN`.
///
/// `smooth` is read with JavaScript truthiness, as web-core's
/// `smooth ? 'smooth' : 'instant'` reads it, and a missing one is `true`. A
/// smooth turn is animated by the painter and leaves the document's offset
/// where it was until the painter posts it back; an instant one moves the
/// document at once ([`dom::Document::scroll_to_with`]). A `display: none`
/// pager has a zero-width scrollport, so it turns to offset 0 in a request no
/// frame carries, and one restyled into no scroll container records nothing;
/// either way the call succeeds, as web-core's does against a zero
/// `clientWidth`.
pub(super) fn select_tab(
    document: &mut LynxDocument,
    pager: NodeId,
    params: &str,
) -> Result<(), InvalidParams> {
    let params: Value = serde_json::from_str(params).map_err(|_| InvalidParams)?;
    let index = params
        .get("index")
        .and_then(Value::as_f64)
        .ok_or(InvalidParams)?;
    let behavior = if params.get("smooth").is_none_or(is_truthy) {
        ScrollBehavior::Smooth
    } else {
        ScrollBehavior::Instant
    };
    let Some(scroll_box) = document.scroll_box(pager) else {
        return Ok(());
    };
    // Clamped in f64 first: an index as large as JSON allows would make the
    // product infinite in f32, and the range is all the document keeps.
    let max = f64::from(scroll_box.max_offset().x);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clamped to the scroll range, which is an f32"
    )]
    let x = (index * f64::from(scroll_box.scrollport.width)).clamp(0.0, max) as f32;
    document.scroll_to_with(pager, Vector2D::new(x, scroll_box.offset.y), behavior);
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Explicit pixel sizes lay out exactly.

    use dom::scroll::ScrollBehavior;
    use dom::stylo::properties::PropertyId;
    use dom::stylo::values::computed::{Display, Overflow};
    use dom::{NodeId, StylesheetOrigin, Vector2D};

    use super::super::test_support::{
        child, document, element_under, overflow, style_of, with_config,
    };
    use super::super::{LynxDocument, PageConfig};
    use super::{
        InvalidParams, VIEWPAGER_ITEM_TAG, VIEWPAGER_TAG, X_VIEWPAGER_ITEM_TAG, X_VIEWPAGER_TAG,
        select_tab,
    };

    /// Whether `node` is a pager, under either of its tag names.
    fn is_viewpager(document: &LynxDocument, node: NodeId) -> bool {
        document
            .get(node)
            .and_then(dom::Node::tag_name)
            .is_some_and(|tag| tag == VIEWPAGER_TAG || tag == X_VIEWPAGER_TAG)
    }

    /// Both spellings of the pager, each with the item spelling that goes
    /// with it.
    const SPELLINGS: [(&str, &str); 2] = [
        (VIEWPAGER_TAG, VIEWPAGER_ITEM_TAG),
        (X_VIEWPAGER_TAG, X_VIEWPAGER_ITEM_TAG),
    ];

    /// One computed longhand, as CSSOM serializes it.
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

    /// A pager of `tag` with `count` items of `item_tag` under the page,
    /// with `style` inline on the pager.
    fn build_pager(
        document: &mut LynxDocument,
        (tag, item_tag): (&str, &str),
        style: &str,
        count: usize,
    ) -> (NodeId, Vec<NodeId>) {
        let pager = child(document, tag, style);
        let items = (0..count)
            .map(|_| element_under(document, pager, item_tag, ""))
            .collect();
        (pager, items)
    }

    // --- the UA rules -------------------------------------------------------

    #[test]
    fn a_pager_scrolls_x_snapping_mandatory_and_keeps_a_row() {
        for spelling in SPELLINGS {
            let mut document = document();
            let (pager, _) = build_pager(&mut document, spelling, "", 0);
            document.layout();

            let tag = spelling.0;
            assert!(is_viewpager(&document, pager), "{tag}");
            assert_eq!(
                overflow(&document, pager),
                (Overflow::Scroll, Overflow::Hidden),
                "the clipped y computes to `hidden` beside a scrolling x: {tag}"
            );
            for (property, expected) in [
                ("width", "100%"),
                ("height", "100%"),
                ("contain", "content"),
                ("scroll-snap-type", "x mandatory"),
                ("flex-direction", "row"),
                ("linear-direction", "row"),
                ("flex-wrap", "nowrap"),
                ("overscroll-behavior-x", "auto"),
                ("box-sizing", "border-box"),
            ] {
                assert_eq!(
                    value(&document, pager, property),
                    expected,
                    "{tag} {property}"
                );
            }
            assert_eq!(*style_of(&document, pager).get_display(), Display::Linear);
        }
    }

    #[test]
    fn an_item_is_a_full_size_page_that_snaps_at_its_start() {
        for spelling in SPELLINGS {
            let mut document = document();
            let (_, items) = build_pager(&mut document, spelling, "", 1);
            document.layout();

            let tag = spelling.1;
            for (property, expected) in [
                ("width", "100%"),
                ("height", "100%"),
                ("contain", "content"),
                ("flex-grow", "0"),
                ("flex-shrink", "0"),
                ("flex-basis", "auto"),
                ("scroll-snap-align", "start"),
                ("scroll-snap-stop", "always"),
                ("position", "relative"),
                ("box-sizing", "border-box"),
            ] {
                assert_eq!(
                    value(&document, items[0], property),
                    expected,
                    "{tag} {property}"
                );
            }
            assert_eq!(
                *style_of(&document, items[0]).get_display(),
                Display::Linear
            );
            assert!(!is_viewpager(&document, items[0]), "{tag}");
        }
    }

    /// An item written outside any pager is an ordinary container, not a
    /// hidden one (the recorded `list-item` decision).
    #[test]
    fn an_item_outside_a_pager_still_generates_a_box() {
        let mut document = document();
        let item = child(&mut document, VIEWPAGER_ITEM_TAG, "");
        document.layout();
        assert_eq!(*style_of(&document, item).get_display(), Display::Linear);
    }

    #[test]
    fn pager_and_item_follow_default_display_linear_both_ways() {
        for (linear, expected) in [(true, Display::Linear), (false, Display::Flex)] {
            for spelling in SPELLINGS {
                let mut document = with_config(PageConfig {
                    default_display_linear: linear,
                    ..PageConfig::default()
                });
                let (pager, items) = build_pager(&mut document, spelling, "", 1);
                document.layout();
                assert_eq!(
                    *style_of(&document, pager).get_display(),
                    expected,
                    "{spelling:?} linear={linear}"
                );
                assert_eq!(
                    *style_of(&document, items[0]).get_display(),
                    expected,
                    "{spelling:?} linear={linear}"
                );
            }
        }
    }

    // --- the row invariant --------------------------------------------------

    /// Every way this grammar lets an author move a flex or linear main axis
    /// or wrap its line, inline or from an author sheet, `!important` or
    /// not, still lays the pages out as one row at the pager's width. The
    /// deprecated `linear-orientation` is not a property here at all.
    #[test]
    fn author_main_axis_styles_still_produce_a_row_of_pages() {
        const SIZE: &str = "width: 200px; height: 100px;";
        assert!(
            PropertyId::parse_enabled_for_all_content("linear-orientation").is_err(),
            "`linear-orientation` is dropped by the grammar, so it cannot move the axis"
        );
        let inline = [
            "display: linear;",
            "display: linear; linear-direction: column;",
            "display: linear; linear-direction: column-reverse;",
            "display: linear; linear-direction: row-reverse;",
            "display: flex; flex-direction: column;",
            "display: flex; flex-direction: column-reverse;",
            "display: flex; flex-direction: row-reverse;",
            "display: flex; flex-wrap: wrap;",
            "display: flex; flex-flow: column wrap;",
            "display: flex; flex-direction: column !important; flex-wrap: wrap !important;",
            "display: linear; linear-direction: column !important;",
            "linear-orientation: vertical;",
        ];
        let sheets = [
            ".pager { display: flex; flex-direction: column !important; }",
            ".pager { display: linear; linear-direction: column !important; }",
            ".pager { flex-flow: column wrap !important; display: flex; }",
        ];
        let cases = inline
            .iter()
            .map(|style| (format!("{SIZE} {style}"), None))
            .chain(sheets.iter().map(|sheet| (SIZE.to_owned(), Some(*sheet))));
        for (style, sheet) in cases {
            for spelling in SPELLINGS {
                let mut document = document();
                if let Some(sheet) = sheet {
                    document.add_stylesheet(sheet, StylesheetOrigin::Author);
                }
                let (pager, items) = build_pager(&mut document, spelling, &style, 3);
                document.add_class(pager, "pager");
                document.layout();
                let placed: Vec<_> = items.iter().map(|item| rect(&document, *item)).collect();
                assert_eq!(
                    placed,
                    [
                        (0.0, 0.0, 200.0, 100.0),
                        (200.0, 0.0, 200.0, 100.0),
                        (400.0, 0.0, 200.0, 100.0),
                    ],
                    "{spelling:?} `{style}` {sheet:?}"
                );
                let scroll_box = document.scroll_box(pager).expect("a scroll container");
                assert_eq!(scroll_box.max_offset(), Vector2D::new(400.0, 0.0));
            }
        }
    }

    /// The e2e card `item-position-absolute`: items the author positions
    /// absolutely stay in the row.
    #[test]
    fn an_item_positioned_absolutely_stays_in_the_row() {
        for spelling in SPELLINGS {
            let mut document = document();
            document.add_stylesheet(
                ".item { position: absolute !important; }",
                StylesheetOrigin::Author,
            );
            let pager = child(&mut document, spelling.0, "width: 200px; height: 100px");
            let items: Vec<_> = (0..2)
                .map(|_| {
                    let item =
                        element_under(&mut document, pager, spelling.1, "position: absolute");
                    document.add_class(item, "item");
                    item
                })
                .collect();
            document.layout();
            assert_eq!(value(&document, items[1], "position"), "relative");
            assert_eq!(rect(&document, items[0]), (0.0, 0.0, 200.0, 100.0));
            assert_eq!(rect(&document, items[1]), (200.0, 0.0, 200.0, 100.0));
        }
    }

    /// A page is the pager's content box: its border and padding sit around
    /// the row, and each page is exactly as wide as what is left.
    #[test]
    fn items_are_the_pagers_content_box() {
        for spelling in SPELLINGS {
            let mut document = document();
            let (pager, items) = build_pager(
                &mut document,
                spelling,
                "width: 300px; height: 200px; padding: 10px; border: 5px solid",
                2,
            );
            document.layout();
            assert_eq!(rect(&document, pager), (0.0, 0.0, 300.0, 200.0));
            assert_eq!(rect(&document, items[0]), (15.0, 15.0, 270.0, 170.0));
            assert_eq!(rect(&document, items[1]), (285.0, 15.0, 270.0, 170.0));
        }
    }

    // --- the attributes -----------------------------------------------------

    #[test]
    fn enable_scroll_or_horizontal_gesture_false_leaves_only_script() {
        for attribute in ["enable-scroll", "allow-horizontal-gesture"] {
            for spelling in SPELLINGS {
                let mut document = document();
                let (pager, _) = build_pager(&mut document, spelling, "", 0);
                let (other, _) = build_pager(&mut document, spelling, "", 0);
                document.set_attribute(pager, attribute, "false");
                document.set_attribute(other, attribute, "true");
                document.layout();
                assert_eq!(
                    overflow(&document, pager),
                    (Overflow::Hidden, Overflow::Hidden),
                    "{spelling:?} {attribute}"
                );
                assert_eq!(
                    overflow(&document, other),
                    (Overflow::Scroll, Overflow::Hidden),
                    "{spelling:?} {attribute}=true"
                );
            }
        }
    }

    #[test]
    fn bounces_stretches_both_edges_unless_it_says_false() {
        for (attribute, expected) in [
            (None, "auto"),
            (Some(""), "contain-bounce"),
            (Some("true"), "contain-bounce"),
            (Some("false"), "auto"),
        ] {
            for spelling in SPELLINGS {
                let mut document = document();
                let (pager, _) = build_pager(&mut document, spelling, "", 0);
                if let Some(attribute) = attribute {
                    document.set_attribute(pager, "bounces", attribute);
                }
                document.layout();
                assert_eq!(
                    value(&document, pager, "overscroll-behavior-x"),
                    expected,
                    "{spelling:?} bounces={attribute:?}"
                );
                assert_eq!(value(&document, pager, "overscroll-behavior-y"), "auto");
            }
        }
    }

    // --- the initial page ---------------------------------------------------

    /// A 200px pager of `count` pages under the page, spelled `spelling`, with
    /// `attributes` written before anything is laid out.
    fn initial_pager(
        spelling: (&str, &str),
        attributes: &[(&str, &str)],
        count: usize,
    ) -> (LynxDocument, NodeId, Vec<NodeId>) {
        let mut document = document();
        let (pager, items) = build_pager(
            &mut document,
            spelling,
            "width: 200px; height: 100px",
            count,
        );
        for (name, value) in attributes {
            document.set_attribute(pager, name, value);
        }
        (document, pager, items)
    }

    fn offset_x(document: &LynxDocument, pager: NodeId) -> f32 {
        document.scroll_offset(pager).x
    }

    fn targets(document: &LynxDocument, items: &[NodeId]) -> Vec<String> {
        items
            .iter()
            .map(|item| value(document, *item, "scroll-initial-target"))
            .collect()
    }

    /// The request the committed frame carries to the painter for `pager`.
    fn carried(document: &mut LynxDocument, pager: NodeId) -> Option<(f32, ScrollBehavior)> {
        let frame = document.commit();
        let index = frame.slot_of(pager).expect("the pager has a slot");
        frame.scroll_slots()[index as usize]
            .request
            .map(|request| (request.target.x, request.behavior))
    }

    /// `select-index` first, then `initial-select-index` — web-core's order —
    /// in the first commit, which carries the position to the painter as an
    /// instant request.
    #[test]
    fn the_first_commit_starts_on_the_selected_page() {
        for spelling in SPELLINGS {
            for (attributes, index, offset) in [
                (&[][..], "-1", 0.0),
                (&[("select-index", "1")][..], "1", 200.0),
                (&[("initial-select-index", "2")][..], "2", 400.0),
                (
                    &[("select-index", "3"), ("initial-select-index", "1")][..],
                    "3",
                    600.0,
                ),
                (
                    &[("select-index", "0"), ("initial-select-index", "2")][..],
                    "0",
                    0.0,
                ),
            ] {
                let (mut document, pager, items) = initial_pager(spelling, attributes, 4);
                let request = carried(&mut document, pager);
                assert_eq!(
                    offset_x(&document, pager),
                    offset,
                    "{spelling:?} {attributes:?}"
                );
                assert_eq!(
                    value(&document, pager, "--viewpager-initial-index"),
                    index,
                    "{spelling:?} {attributes:?}"
                );
                let expected: Vec<&str> = (0..4)
                    .map(|page| {
                        if page.to_string() == index {
                            "nearest"
                        } else {
                            "none"
                        }
                    })
                    .collect();
                assert_eq!(
                    targets(&document, &items),
                    expected,
                    "{spelling:?} {attributes:?}"
                );
                if index == "-1" {
                    assert_eq!(request, None);
                } else {
                    assert_eq!(request, Some((offset, ScrollBehavior::Instant)));
                }
            }
        }
    }

    /// A present `select-index` that is not an integer does not fall back to
    /// `initial-select-index` — the fork's typed `attr()` fallback gap
    /// (`docs/style-assumptions.md` §27) — so the pager shows its first page.
    #[test]
    fn a_select_index_that_is_no_integer_shows_the_first_page() {
        for select in ["abc", "1.5", ""] {
            let (mut document, pager, items) = initial_pager(
                SPELLINGS[0],
                &[("select-index", select), ("initial-select-index", "2")],
                4,
            );
            document.commit();
            assert_eq!(
                value(&document, pager, "--viewpager-initial-index"),
                "-1",
                "{select:?}"
            );
            assert_eq!(targets(&document, &items), ["none"; 4], "{select:?}");
            assert_eq!(offset_x(&document, pager), 0.0, "{select:?}");
        }
    }

    #[test]
    fn an_index_naming_no_page_shows_the_first_page() {
        for index in ["-1", "-5", "4", "10"] {
            let (mut document, pager, items) =
                initial_pager(SPELLINGS[1], &[("select-index", index)], 4);
            document.commit();
            assert_eq!(targets(&document, &items), ["none"; 4], "{index}");
            assert_eq!(offset_x(&document, pager), 0.0, "{index}");
        }
    }

    /// Each new target is honoured once, so changing the attribute in force
    /// after the first layout turns the pager, instantly — where web-core
    /// reads it once, at connect.
    #[test]
    fn changing_the_index_in_force_turns_the_pager() {
        let (mut document, pager, _) = initial_pager(SPELLINGS[0], &[("select-index", "1")], 4);
        document.commit();
        assert_eq!(offset_x(&document, pager), 200.0);
        document.set_attribute(pager, "select-index", "3");
        assert_eq!(
            carried(&mut document, pager),
            Some((600.0, ScrollBehavior::Instant))
        );
        assert_eq!(offset_x(&document, pager), 600.0);

        let (mut document, pager, _) =
            initial_pager(SPELLINGS[1], &[("initial-select-index", "1")], 4);
        document.commit();
        document.set_attribute(pager, "initial-select-index", "2");
        document.commit();
        assert_eq!(offset_x(&document, pager), 400.0);
        // `select-index` takes over from `initial-select-index`.
        document.set_attribute(pager, "select-index", "0");
        document.commit();
        assert_eq!(offset_x(&document, pager), 0.0);
    }

    /// A user who moved away is not moved back by a commit that leaves the
    /// target where it was.
    #[test]
    fn an_unrelated_commit_leaves_a_swiped_pager_alone() {
        let (mut document, pager, items) = initial_pager(SPELLINGS[0], &[("select-index", "2")], 4);
        document.commit();
        // Where main writes the offset the painter posted for a swipe.
        document.scroll_to(pager, Vector2D::new(0.0, 0.0));
        document.set_inline_style(items[0], "opacity: 0.5");
        assert_eq!(
            carried(&mut document, pager),
            Some((400.0, ScrollBehavior::Instant))
        );
        assert_eq!(
            offset_x(&document, pager),
            0.0,
            "the request still carried is the first commit's, which the painter has handled"
        );
        let serial = document
            .pending_scroll_request(pager)
            .expect("not yet acknowledged");
        document.acknowledge_scroll_request(pager, serial);
        document.set_inline_style(items[1], "opacity: 0.5");
        assert_eq!(carried(&mut document, pager), None);
        assert_eq!(offset_x(&document, pager), 0.0);
    }

    /// `sibling-index()` counts the item's own parent's children, so a page
    /// inserted or removed ahead of the target makes another page the
    /// target, and the pager turns to it.
    #[test]
    fn inserting_or_removing_a_page_ahead_of_the_target_moves_the_target() {
        let (mut document, pager, items) = initial_pager(SPELLINGS[0], &[("select-index", "2")], 4);
        document.commit();
        document.scroll_to(pager, Vector2D::new(0.0, 0.0));
        let inserted = document.create_element(VIEWPAGER_ITEM_TAG, ());
        document.insert_before(pager, inserted, Some(items[0]));
        document.commit();
        assert_eq!(
            targets(&document, &items),
            ["none", "nearest", "none", "none"]
        );
        assert_eq!(offset_x(&document, pager), 400.0);

        document.scroll_to(pager, Vector2D::new(0.0, 0.0));
        document.remove_element(inserted);
        document.commit();
        assert_eq!(
            targets(&document, &items),
            ["none", "none", "nearest", "none"]
        );
        assert_eq!(offset_x(&document, pager), 400.0);
    }

    /// A `wrapper` around each page makes every page the first child of its
    /// own parent: all of them count as index 0. Pages sharing one
    /// `wrapper` count as they would without it.
    #[test]
    fn wrapped_pages_count_among_their_own_parents_children() {
        for (select, expected) in [("0", "nearest"), ("1", "none")] {
            let mut document = document();
            let pager = child(&mut document, VIEWPAGER_TAG, "width: 200px; height: 100px");
            document.set_attribute(pager, "select-index", select);
            let items: Vec<_> = (0..3)
                .map(|_| {
                    let wrapper = element_under(&mut document, pager, "wrapper", "");
                    element_under(&mut document, wrapper, VIEWPAGER_ITEM_TAG, "")
                })
                .collect();
            document.commit();
            assert_eq!(targets(&document, &items), [expected; 3], "{select}");
            assert_eq!(offset_x(&document, pager), 0.0, "{select}");
        }

        let mut document = document();
        let pager = child(&mut document, VIEWPAGER_TAG, "width: 200px; height: 100px");
        document.set_attribute(pager, "select-index", "2");
        let wrapper = element_under(&mut document, pager, "wrapper", "");
        let items: Vec<_> = (0..3)
            .map(|_| element_under(&mut document, wrapper, VIEWPAGER_ITEM_TAG, ""))
            .collect();
        document.commit();
        assert_eq!(targets(&document, &items), ["none", "none", "nearest"]);
        assert_eq!(offset_x(&document, pager), 400.0);
    }

    /// Every pager sets the index from its own attributes, so a pager inside
    /// another's page does not inherit the outer one's.
    #[test]
    fn a_nested_pager_does_not_inherit_the_outer_index() {
        let (mut document, outer, pages) = initial_pager(SPELLINGS[0], &[("select-index", "1")], 2);
        let inner = element_under(&mut document, pages[1], VIEWPAGER_TAG, "");
        let inner_pages: Vec<_> = (0..3)
            .map(|_| element_under(&mut document, inner, VIEWPAGER_ITEM_TAG, ""))
            .collect();
        document.commit();
        assert_eq!(offset_x(&document, outer), 200.0);
        assert_eq!(value(&document, inner, "--viewpager-initial-index"), "-1");
        assert_eq!(targets(&document, &inner_pages), ["none"; 3]);
        assert_eq!(offset_x(&document, inner), 0.0);

        document.set_attribute(inner, "select-index", "2");
        document.commit();
        assert_eq!(offset_x(&document, inner), 400.0);
        assert_eq!(offset_x(&document, outer), 200.0);
    }

    /// A `display: none` pager has no scroll slot, so nothing is honoured
    /// until it is shown; the first commit that shows it starts on the page.
    #[test]
    fn a_pager_shown_later_starts_on_the_selected_page_when_shown() {
        let mut document = document();
        let (pager, _) = build_pager(
            &mut document,
            SPELLINGS[0],
            "display: none; width: 200px; height: 100px",
            4,
        );
        document.set_attribute(pager, "select-index", "2");
        document.commit();
        assert_eq!(offset_x(&document, pager), 0.0);
        document.set_inline_style(pager, "width: 200px; height: 100px");
        document.commit();
        assert_eq!(offset_x(&document, pager), 400.0);
    }

    /// A zero-width pager is a scroll container with nowhere to go: its
    /// target is honoured at offset 0 in its first commit, and a target is
    /// honoured once, so widening the pager later leaves it on the first
    /// page. web-core instead retries each animation frame until the pager
    /// has a width. Pinned as it is; recorded in `deviations.md`.
    #[test]
    fn a_pager_that_is_zero_wide_at_its_first_commit_stays_on_the_first_page() {
        let mut document = document();
        let (pager, items) =
            build_pager(&mut document, SPELLINGS[0], "width: 0px; height: 100px", 4);
        document.set_attribute(pager, "select-index", "2");
        document.commit();
        assert_eq!(
            targets(&document, &items),
            ["none", "none", "nearest", "none"]
        );
        assert_eq!(offset_x(&document, pager), 0.0);
        document.set_inline_style(pager, "width: 200px; height: 100px");
        document.commit();
        assert_eq!(offset_x(&document, pager), 0.0);
    }

    /// Pages that arrive after the pager's first layout bring the target
    /// with them, and a new target is honoured.
    #[test]
    fn pages_that_arrive_later_bring_the_target_with_them() {
        let (mut document, pager, _) = initial_pager(SPELLINGS[1], &[("select-index", "2")], 0);
        document.commit();
        assert_eq!(offset_x(&document, pager), 0.0);
        for _ in 0..4 {
            element_under(&mut document, pager, X_VIEWPAGER_ITEM_TAG, "");
        }
        document.commit();
        assert_eq!(offset_x(&document, pager), 400.0);
    }

    // --- selectTab ----------------------------------------------------------

    /// A laid-out 200px pager of four pages: max offset 600.
    fn four_pages() -> (LynxDocument, NodeId) {
        let mut document = document();
        let (pager, _) = build_pager(
            &mut document,
            SPELLINGS[0],
            "width: 200px; height: 100px",
            4,
        );
        document.layout();
        (document, pager)
    }

    fn pending(
        document: &mut LynxDocument,
        pager: NodeId,
    ) -> Option<(Vector2D<f32>, ScrollBehavior)> {
        let frame = document.commit();
        let index = frame.slot_of(pager).expect("the pager has a slot");
        frame.scroll_slots()[index as usize]
            .request
            .map(|request| (request.target, request.behavior))
    }

    #[test]
    fn select_tab_turns_to_index_times_the_width() {
        let (mut document, pager) = four_pages();
        assert_eq!(
            select_tab(&mut document, pager, r#"{"index":2,"smooth":false}"#),
            Ok(())
        );
        assert_eq!(document.scroll_offset(pager), Vector2D::new(400.0, 0.0));
        assert_eq!(
            pending(&mut document, pager),
            Some((Vector2D::new(400.0, 0.0), ScrollBehavior::Instant))
        );

        assert_eq!(select_tab(&mut document, pager, r#"{"index":1}"#), Ok(()));
        assert_eq!(
            document.scroll_offset(pager),
            Vector2D::new(400.0, 0.0),
            "smooth by default: the painter moves it"
        );
        assert_eq!(
            pending(&mut document, pager),
            Some((Vector2D::new(200.0, 0.0), ScrollBehavior::Smooth))
        );
    }

    #[test]
    fn select_tab_reads_smooth_with_javascript_truthiness() {
        for (smooth, behavior) in [
            ("true", ScrollBehavior::Smooth),
            ("1", ScrollBehavior::Smooth),
            ("\"no\"", ScrollBehavior::Smooth),
            ("{}", ScrollBehavior::Smooth),
            ("false", ScrollBehavior::Instant),
            ("0", ScrollBehavior::Instant),
            ("\"\"", ScrollBehavior::Instant),
            ("null", ScrollBehavior::Instant),
        ] {
            let (mut document, pager) = four_pages();
            let params = format!(r#"{{"index":1,"smooth":{smooth}}}"#);
            select_tab(&mut document, pager, &params).expect("valid params");
            assert_eq!(
                pending(&mut document, pager).map(|(_, behavior)| behavior),
                Some(behavior),
                "{smooth}"
            );
        }
    }

    #[test]
    fn select_tab_clamps_and_multiplies_a_fractional_index() {
        for (index, expected) in [("9", 600.0), ("-3", 0.0), ("1.5", 300.0), ("1e308", 600.0)] {
            let (mut document, pager) = four_pages();
            let params = format!(r#"{{"index":{index},"smooth":false}}"#);
            assert_eq!(select_tab(&mut document, pager, &params), Ok(()), "{index}");
            assert_eq!(
                document.scroll_offset(pager),
                Vector2D::new(expected, 0.0),
                "{index}"
            );
        }
    }

    #[test]
    fn select_tab_refuses_params_without_a_numeric_index() {
        for params in [
            "{}",
            r#"{"index":null}"#,
            r#"{"index":"2"}"#,
            r#"{"index":true}"#,
            r#"{"smooth":false}"#,
            "null",
            "[2]",
            "2",
            "not json",
        ] {
            let (mut document, pager) = four_pages();
            assert_eq!(
                select_tab(&mut document, pager, params),
                Err(InvalidParams),
                "{params}"
            );
            assert_eq!(document.scroll_offset(pager), Vector2D::zero(), "{params}");
            assert_eq!(document.pending_scroll_request(pager), None, "{params}");
        }
    }

    /// A pager with no box, or one that is no scroll container, has nothing
    /// to turn: the call succeeds, nothing moves, and no frame carries a
    /// request for it.
    #[test]
    fn select_tab_on_a_pager_that_cannot_scroll_moves_nothing() {
        for style in ["display: none", "overflow: visible"] {
            let mut document = document();
            let (pager, _) = build_pager(&mut document, SPELLINGS[1], style, 2);
            document.layout();
            assert_eq!(
                select_tab(&mut document, pager, r#"{"index":1,"smooth":false}"#),
                Ok(()),
                "{style}"
            );
            assert_eq!(document.scroll_offset(pager), Vector2D::zero(), "{style}");
            let frame = document.commit();
            assert_eq!(frame.slot_of(pager), None, "{style}");
            assert_eq!(document.pending_scroll_request(pager), None, "{style}");
        }
    }
}
