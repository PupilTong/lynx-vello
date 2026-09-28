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
//! (`crates/bobcat-core/src/paint/inertia.rs`). No component is defined for
//! either tag: nothing here reacts to an attribute outside the cascade, and
//! `selectTab` is dispatched by tag name from the runtime's
//! `callElementMethod` ([`select_tab`]).
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
//! - `select-index` and `initial-select-index` are not implemented yet: the pager always starts on
//!   its first page (`docs/tracking/components.md`).
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
use dom::{NodeId, Vector2D};
use serde_json::Value;

use super::LynxDocument;

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
viewpager, x-viewpager-ng {
  width: 100%; height: 100%; contain: content;
  overflow-x: scroll; overflow-y: clip;
  scroll-snap-type: x mandatory;
}
viewpager, x-viewpager-ng { flex-direction: row !important; linear-direction: row !important; flex-wrap: nowrap !important; }
viewpager[allow-horizontal-gesture="false"], viewpager[enable-scroll="false"],
x-viewpager-ng[allow-horizontal-gesture="false"], x-viewpager-ng[enable-scroll="false"] { overflow-x: hidden; }
viewpager[bounces]:not([bounces="false"]),
x-viewpager-ng[bounces]:not([bounces="false"]) { overscroll-behavior-x: contain-bounce; }
viewpager-item, x-viewpager-item-ng {
  width: 100%; height: 100%; contain: content; flex: 0 0 auto;
  scroll-snap-align: start; scroll-snap-stop: always;
}
viewpager-item, x-viewpager-item-ng { position: relative !important; }
"#;

/// Whether `node` is a pager, under either of its tag names.
pub(crate) fn is_viewpager(document: &LynxDocument, node: NodeId) -> bool {
    document
        .get(node)
        .and_then(dom::Node::tag_name)
        .is_some_and(|tag| tag == VIEWPAGER_TAG || tag == X_VIEWPAGER_TAG)
}

/// `selectTab`'s params were not `{index: <number>, …}`: the UI-method
/// status table's code 4, `PARAM_INVALID`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InvalidParams;

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
pub(crate) fn select_tab(
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

/// JavaScript's `ToBoolean` over the values JSON can carry.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
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
        is_viewpager, select_tab,
    };

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
            assert_eq!(style_of(&document, pager).clone_display(), Display::Linear);
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
                style_of(&document, items[0]).clone_display(),
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
        assert_eq!(style_of(&document, item).clone_display(), Display::Linear);
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
                    style_of(&document, pager).clone_display(),
                    expected,
                    "{spelling:?} linear={linear}"
                );
                assert_eq!(
                    style_of(&document, items[0]).clone_display(),
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
