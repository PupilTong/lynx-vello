//! The `x-refresh-view`, `x-refresh-header` and `x-refresh-footer` tags: a
//! column whose content can be pulled down past its top to show a header,
//! and pushed up past its end to show a footer, each of which stays shown
//! once it is pulled far enough in.
//!
//! Translated from web-elements'
//! `lynx-stack/packages/web-platform/web-elements/src/elements/XRefreshView/x-refresh-view.css`
//! and its template (`htmlTemplates.ts:174-219`). Only web-core's three tag
//! names exist here. web-core's tag map also turns `<refresh>` and
//! `<refresh-header>` into the first two (`web-core/ts/constants.ts:112-113`);
//! those two spellings are not registered.
//!
//! # The model
//!
//! [`RefreshView`] attaches a UA shadow tree when the element is constructed,
//! web-core's, part for part:
//!
//! - `#container`, the scroll container: a column flex box of the host's size, `overflow-y:
//!   scroll`, `overscroll-behavior: contain`, `scroll-snap-type: y mandatory`.
//! - `#placeholder-top`, an empty box `30%` of `#container`'s height: how far past its own height
//!   the header can be pulled down.
//! - `<slot name="header">`, which the header is assigned to.
//! - `#content`, a column flex box exactly `#container`'s size, holding the default `<slot>` the
//!   content children are assigned to. It is the only box that snaps at rest (`scroll-snap-align:
//!   center`), and `scroll-initial-target: nearest` scrolls `#container` to it on the first layout,
//!   so the view opens with the header above the scrollport and the footer below it.
//! - `<slot name="footer">` and `#placeholder-bot`, the footer's counterparts of the first two.
//!
//! The slots are `display: contents`, so the header, the content children and the footer are
//! flex items of `#container` and `#content` in the flat tree, while the light-DOM rules
//! ([`UA_RULES`]) match them as the host's children. A pull is the scroll chain: a drag on an
//! inner `scroll-view` at its top edge hands what it cannot take to `#container`, the inner
//! scroller's flat-tree scroll parent, and the painter holds every container a drag moves until
//! the release, then glides each to its snap position (`ScrollIntents::settle` in
//! `crates/bobcat-core/src/paint/lib.rs`).
//!
//! # Slot assignment
//!
//! web-core's header and footer set their own `slot` attribute when they connect
//! (`XRefreshHeader.ts`, `XRefreshFooter.ts`). Here [`RefreshView`] does it: on the standard's
//! `slotchange` at its default slot, every `x-refresh-header` assigned there gets `slot="header"`
//! and every `x-refresh-footer` `slot="footer"`. That moves it to its named slot and fires
//! `slotchange` again after the current set; the second pass finds neither tag in the default
//! slot. The header and the footer need no component of their own.
//!
//! # The snap alignment is a view-timeline animation
//!
//! On release web-core's scroller snaps back to `#content` unless the header (footer) is at
//! least 90% visible: then an `IntersectionObserver` has set `x-magnet-enable` on it
//! (`XRefreshSubElementIntersectionObserver.ts`), whose rule gives it `scroll-snap-align: start
//! !important` (`end` for the footer), and the release lands on the header instead. Here the same
//! alignment is an animation of `scroll-snap-align` on the header's own `view(block)` timeline.
//! The property is discrete, so it changes half way between two keyframes:
//!
//! - header: `start` from `cover 0%` to `exit 0%`, `none` from `exit 20%` to `cover 100%`; the
//!   change sits at `exit 10%`, so a header at least 90% visible is `start`. At rest its end edge
//!   is on the scrollport's start (`exit 100%`): `none`.
//! - footer: `none` from `cover 0%` to `entry 80%`, `end` from `entry 100%`; the change sits at
//!   `entry 90%`. At rest its start edge is on the scrollport's end (`entry 0%`): `none`.
//!
//! Neither animates a property the painter samples, so each re-cascades on the main thread when
//! it adopts a scroll of `#container`, and the commit that follows publishes the snap positions
//! the release reads. web-core keeps the attribute until `finishRefresh` removes it; here the
//! alignment follows the header's visibility, so a header that snapped in stays until the user
//! scrolls it out (`docs/tracking/deviations.md`).
//!
//! # Attributes
//!
//! Attribute values arrive as strings: this engine's `__SetAttribute` stringifies a value
//! (`packages/bobcat-element/src/element-papi.ts:1608`), so `enable-refresh={false}` in JSX is
//! the attribute `"false"`. web-core keeps `"false"` for this component's three attributes
//! (`notToFilterFalseAttributes`, `XRefreshView.ts:19-23`) and matches it in its CSS, as here:
//!
//! - `enable-refresh="false"`: the header generates no box and `#placeholder-top` is hidden, so
//!   nothing can be pulled down. Any other value, or none, shows them.
//! - `enable-loadmore="false"`: the same for the footer and `#placeholder-bot`.
//! - `enable-auto-loadmore` and `enable-footer-rebound` only change web-core's events, which are
//!   not implemented: no visible effect.
//!
//! # Which headers and footers show
//!
//! The first `x-refresh-header` and the first `x-refresh-footer` among a refresh view's children
//! show; the hide rule matches only what web-core hides — a later one of either, one whose view
//! turns its end off, and one whose parent is some other element. A header with no parent
//! element at all (the top of a shadow tree) is not hidden, an edge accepted here. A shown one
//! takes the engine's display policy, like every container tag ([`super::ua_sheet`]):
//! `display: linear` (a column) under `defaultDisplayLinear`, as web-core's linear toggle gives
//! it and as native's header is a plain group, and the engine default otherwise.
//!
//! # The inner scroller chains on y
//!
//! Every content child gets `overscroll-behavior-y: auto !important`. A y boundary that contains
//! the chain — an author's `overscroll-behavior`, or the `contain` a `scroll-coordinator`
//! declares on itself — would stop the pull at the child, and nothing would reach `#container`.
//! A `scroll-view`'s `bounces` attribute reaches no style at all. Native attaches the header to
//! the inner scroller's own bounce; here the inner scroller does not bounce on y inside a
//! refresh view.
//!
//! # Deviations and what is not implemented
//!
//! - web-core gives `#container`, `#content` and the slot the host's display mode (`--lynx-display:
//!   inherit`, `x-refresh-view.css:22-26`); here they are fixed flex columns.
//! - The header's and the footer's `position: relative !important` are web-core's own and stay
//!   `!important`: web-core's demo card writes `position: absolute` on its header
//!   (`basic-element-x-refresh-view-demo/index.css`). `docs/style-assumptions.md` §D.15 records
//!   them, with the content children's `overscroll-behavior-y`.
//! - Every event (`startrefresh`, `headeroffset`, `startloadmore`, …) and every UI method
//!   (`finishRefresh`, `autoStartRefresh`, `finishLoadMore`).

use dom::event::{ElementEvent, ElementEventKind, EventPhase};
use dom::{CustomElement, Node, NodeId, ShadowRootMode};

use super::LynxDocument;

/// The container tag, web-core's registered spelling.
pub(super) const REFRESH_VIEW_TAG: &str = "x-refresh-view";
/// The header tag.
pub(super) const REFRESH_HEADER_TAG: &str = "x-refresh-header";
/// The footer tag.
pub(super) const REFRESH_FOOTER_TAG: &str = "x-refresh-footer";

/// The light-DOM policy, in `x-refresh-view.css`'s order; see the module
/// documentation. The host's box defaults are that file's. Each important
/// declaration sits on a line of its own, because [`super::ua_sheet`]'s
/// pinned test reads the sheet line by line.
pub(super) const UA_RULES: &str = r#"
x-refresh-view > x-refresh-header:not(:first-of-type), x-refresh-view > x-refresh-footer:not(:first-of-type),
:not(x-refresh-view) > x-refresh-header, :not(x-refresh-view) > x-refresh-footer,
x-refresh-view[enable-refresh="false"] > x-refresh-header,
x-refresh-view[enable-loadmore="false"] > x-refresh-footer { display: none; }
x-refresh-view {
  display: flex; flex-direction: column; box-sizing: border-box;
  border-width: 0; border-style: solid; position: relative; min-width: 0; min-height: 0;
}
x-refresh-view:not([enable-refresh="false"]) > x-refresh-header:first-of-type {
  flex-shrink: 0; scroll-snap-align: none;
  animation: x-refresh-header-magnet auto linear both; animation-timeline: view(block);
}
x-refresh-view:not([enable-refresh="false"]) > x-refresh-header:first-of-type { position: relative !important; }
x-refresh-view:not([enable-loadmore="false"]) > x-refresh-footer:first-of-type {
  flex-shrink: 0; margin-top: auto; scroll-snap-align: none;
  animation: x-refresh-footer-magnet auto linear both; animation-timeline: view(block);
}
x-refresh-view:not([enable-loadmore="false"]) > x-refresh-footer:first-of-type { position: relative !important; }
x-refresh-view > :not(x-refresh-header, x-refresh-footer) { overscroll-behavior-y: auto !important; }
@keyframes x-refresh-header-magnet {
  cover 0%, exit 0% { scroll-snap-align: start; }
  exit 20%, cover 100% { scroll-snap-align: none; }
}
@keyframes x-refresh-footer-magnet {
  cover 0%, entry 80% { scroll-snap-align: none; }
  entry 100%, cover 100% { scroll-snap-align: end; }
}
"#;

/// The shadow sheet, added to each instance's shadow root: see the module
/// documentation.
const SHADOW_RULES: &str = r#"
slot { display: contents; }
#container {
  display: flex; flex-direction: column; width: 100%; height: 100%; flex-shrink: 0;
  overflow-x: clip; overflow-y: scroll; overscroll-behavior: contain;
  scroll-snap-type: y mandatory; contain: content;
}
#placeholder-top, #placeholder-bot { min-height: 30%; width: 100%; flex-shrink: 0; }
#content {
  display: flex; flex-direction: column; width: 100%; height: 100%; flex-shrink: 0;
  scroll-snap-align: center; scroll-initial-target: nearest;
}
:host([enable-refresh="false"]) #placeholder-top { display: none; }
:host([enable-loadmore="false"]) #placeholder-bot { display: none; }
"#;

/// Installs the `x-refresh-view` component. Must run before any element
/// could carry the tag, [`Document::define`](dom::Document::define)'s own
/// precondition.
pub(super) fn define(document: &mut LynxDocument) {
    document.define(REFRESH_VIEW_TAG, Box::new(RefreshView));
}

/// The `x-refresh-view` component: builds the shadow tree when the element
/// is constructed, and moves the header and the footer into their named
/// slots.
///
/// The shadow tree is built in `constructed`, as the swiper's is: the
/// element is new and detached then, so every child arrives as a
/// `slotchange` at the default slot.
struct RefreshView;

impl CustomElement<()> for RefreshView {
    fn constructed(&self, document: &mut LynxDocument, element: NodeId) {
        let shadow = document.attach_shadow(element, ShadowRootMode::Open);
        document.add_shadow_stylesheet(shadow, SHADOW_RULES);
        let container = div(document, "container");
        let top = div(document, "placeholder-top");
        let header = named_slot(document, "header");
        let content = div(document, "content");
        let slot = document.create_element("slot", ());
        document.append_child(content, slot);
        let footer = named_slot(document, "footer");
        let bottom = div(document, "placeholder-bot");
        for part in [top, header, content, footer, bottom] {
            document.append_child(container, part);
        }
        document.append_child(shadow, container);
    }

    /// `slotchange` at the default slot — the one slot with no `name` —
    /// delivered at the shadow root once per pass: the capture delivery is
    /// skipped, the bubble one acts. The named slots' own are ignored.
    fn handle_event(&self, document: &mut LynxDocument, _: NodeId, event: &mut ElementEvent) {
        if event.kind() != ElementEventKind::SlotChange || event.phase() != EventPhase::Bubbling {
            return;
        }
        let slot = event.target();
        if document
            .get(slot)
            .is_none_or(|node| node.attribute("name").is_some())
        {
            return;
        }
        let moves: Vec<(NodeId, &str)> = document
            .assigned_nodes(slot)
            .iter()
            .filter_map(|&node| match document.get(node).and_then(Node::tag_name)? {
                REFRESH_HEADER_TAG => Some((node, "header")),
                REFRESH_FOOTER_TAG => Some((node, "footer")),
                _ => None,
            })
            .collect();
        for (node, name) in moves {
            document.set_attribute(node, "slot", name);
        }
    }
}

/// A `div` with `id`.
fn div(document: &mut LynxDocument, id: &str) -> NodeId {
    let div = document.create_element("div", ());
    document.set_id_attribute(div, Some(id));
    div
}

/// A `<slot>` named `name`.
fn named_slot(document: &mut LynxDocument, name: &str) -> NodeId {
    let slot = document.create_element("slot", ());
    document.set_attribute(slot, "name", name);
    slot
}

/// The refresh view's scroll container, `#container`, the one element of
/// its shadow tree.
#[cfg(test)]
fn container(document: &LynxDocument, view: NodeId) -> Option<NodeId> {
    let root = document.shadow_root(view)?;
    document
        .get(root)?
        .children()
        .find(|child| child.is_element())
        .map(Node::id)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Explicit pixel sizes lay out exactly.

    use dom::stylo::properties::PropertyId;
    use dom::stylo::values::computed::Display;
    use dom::{NodeId, StylesheetOrigin, Vector2D};

    use super::super::test_support::{child, element_under, style_of, with_config};
    use super::super::{LynxDocument, PageConfig};
    use super::{REFRESH_FOOTER_TAG, REFRESH_HEADER_TAG, REFRESH_VIEW_TAG};

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

    fn display(document: &LynxDocument, element: NodeId) -> Display {
        style_of(document, element).slow_clone_display()
    }

    /// A 393px x 800px refresh view with `attributes`, a 50px header, one
    /// content `view` of the view's full height, and a 50px footer.
    struct Built {
        document: LynxDocument,
        view: NodeId,
        header: NodeId,
        content: NodeId,
        footer: NodeId,
    }

    impl Built {
        fn new(attributes: &[(&str, &str)], content_tag: &str) -> Self {
            Self::with_config(PageConfig::default(), attributes, content_tag)
        }

        fn with_config(config: PageConfig, attributes: &[(&str, &str)], content_tag: &str) -> Self {
            let mut document = with_config(config);
            let view = child(
                &mut document,
                REFRESH_VIEW_TAG,
                "width: 393px; height: 800px",
            );
            for (name, value) in attributes {
                document.set_attribute(view, name, value);
            }
            let header = element_under(&mut document, view, REFRESH_HEADER_TAG, "height: 50px");
            let content = element_under(&mut document, view, content_tag, "height: 100%");
            let footer = element_under(&mut document, view, REFRESH_FOOTER_TAG, "height: 50px");
            Self {
                document,
                view,
                header,
                content,
                footer,
            }
        }

        /// `#container`, the shadow scroll container.
        fn container(&self) -> NodeId {
            super::container(&self.document, self.view).expect("every refresh view has one")
        }

        /// `#container`'s shadow children: the two placeholders, the two
        /// named slots and `#content`, in tree order.
        fn parts(&self) -> Vec<NodeId> {
            self.document
                .get(self.container())
                .expect("live")
                .child_ids()
                .to_vec()
        }

        /// `#container`'s snap positions on y, as single offsets.
        fn snap_points(&self) -> Vec<f32> {
            let positions = self
                .document
                .snap_positions(self.container())
                .expect("a snapping container");
            positions
                .y()
                .expect("snaps on y")
                .points
                .iter()
                .map(|point| {
                    assert_eq!(point.min, point.max, "no snap area outgrows the port");
                    point.min
                })
                .collect()
        }

        /// Scrolls `#container` to `y` as an adopted painter post does,
        /// and re-samples what follows the scroll.
        fn scroll_to(&mut self, y: f32) {
            let container = self.container();
            self.document.scroll_to(container, Vector2D::new(0.0, y));
            self.document.advance_scroll_timelines(&[container]);
            self.document.commit();
        }
    }

    // --- the shadow tree ----------------------------------------------------

    /// `#container` holds the top placeholder, the header slot, `#content`
    /// with the default slot, the footer slot and the bottom placeholder.
    /// The header and the footer get their `slot` attributes from the
    /// component and land in the named slots; the content child stays in
    /// the default one. The light DOM is untouched otherwise.
    #[test]
    fn the_header_and_footer_are_slotted_around_the_content() {
        let mut built = Built::new(&[], "view");
        built.document.layout();
        let document = &built.document;
        let parts = built.parts();
        let ids: Vec<_> = parts
            .iter()
            .map(|part| {
                let node = document.get(*part).expect("live");
                (
                    node.tag_name().map(str::to_owned),
                    node.attribute("id")
                        .or(node.attribute("name"))
                        .map(str::to_owned),
                )
            })
            .collect();
        let named = |tag: &str, name: &str| (Some(tag.to_owned()), Some(name.to_owned()));
        assert_eq!(
            ids,
            [
                named("div", "placeholder-top"),
                named("slot", "header"),
                named("div", "content"),
                named("slot", "footer"),
                named("div", "placeholder-bot"),
            ]
        );
        let default_slot = document.get(parts[2]).expect("live").child_ids()[0];
        assert_eq!(document.assigned_nodes(parts[1]), [built.header]);
        assert_eq!(document.assigned_nodes(default_slot), [built.content]);
        assert_eq!(document.assigned_nodes(parts[3]), [built.footer]);
        let slot_of = |node: NodeId| document.get(node).and_then(|node| node.attribute("slot"));
        assert_eq!(slot_of(built.header), Some("header"));
        assert_eq!(slot_of(built.footer), Some("footer"));
        assert_eq!(slot_of(built.content), None);
        for slot in [parts[1], default_slot, parts[3]] {
            assert_eq!(display(document, slot), Display::Contents);
        }
        assert_eq!(
            document.get(built.view).expect("live").child_ids(),
            [built.header, built.content, built.footer]
        );
        assert_eq!(
            document.scroll_box(built.view),
            None,
            "the host does not scroll"
        );
    }

    /// A second header or footer is slotted too, but only the first of each
    /// generates a box (`:first-of-type`), and a header outside a refresh
    /// view generates none.
    #[test]
    fn only_the_first_header_and_footer_generate_a_box() {
        let mut built = Built::new(&[], "view");
        let second_header = element_under(&mut built.document, built.view, REFRESH_HEADER_TAG, "");
        let second_footer = element_under(&mut built.document, built.view, REFRESH_FOOTER_TAG, "");
        let stray = child(&mut built.document, REFRESH_HEADER_TAG, "height: 50px");
        built.document.layout();
        let document = &built.document;
        let slot_of = |node: NodeId| document.get(node).and_then(|node| node.attribute("slot"));
        assert_eq!(slot_of(second_header), Some("header"));
        assert_eq!(slot_of(second_footer), Some("footer"));
        assert_eq!(slot_of(stray), None);
        for (element, expected) in [
            (built.header, Display::Linear),
            (built.footer, Display::Linear),
            (second_header, Display::None),
            (second_footer, Display::None),
            (stray, Display::None),
        ] {
            assert_eq!(display(document, element), expected);
        }
    }

    /// A shown header and footer take the engine's display policy:
    /// `linear` under `defaultDisplayLinear`, the engine default (`flex`)
    /// without it.
    #[test]
    fn the_header_and_footer_follow_the_display_switch() {
        for (linear, expected) in [(true, Display::Linear), (false, Display::Flex)] {
            let config = PageConfig {
                default_display_linear: linear,
                ..PageConfig::default()
            };
            let mut built = Built::with_config(config, &[], "view");
            built.document.layout();
            for end in [built.header, built.footer] {
                assert_eq!(display(&built.document, end), expected, "linear={linear}");
            }
        }
    }

    // --- the attributes -----------------------------------------------------

    /// `"false"` hides the header (footer) and the placeholder above
    /// (below) it; `"true"`, any other value and no attribute show them.
    #[test]
    fn enable_refresh_and_enable_loadmore_false_hide_their_ends() {
        for (attribute, shown) in [
            (None, true),
            (Some("true"), true),
            (Some(""), true),
            (Some("false"), false),
        ] {
            for (name, end, placeholder) in [("enable-refresh", 0, 0), ("enable-loadmore", 2, 4)] {
                let attributes: Vec<_> = attribute.map(|value| (name, value)).into_iter().collect();
                let mut built = Built::new(&attributes, "view");
                built.document.layout();
                let ends = [built.header, built.content, built.footer];
                let expected = if shown {
                    Display::Linear
                } else {
                    Display::None
                };
                assert_eq!(
                    display(&built.document, ends[end]),
                    expected,
                    "{name}={attribute:?}"
                );
                let placeholder = built.parts()[placeholder];
                assert_eq!(
                    display(&built.document, placeholder) != Display::None,
                    shown,
                    "{name}={attribute:?}"
                );
            }
        }
    }

    // --- the geometry -------------------------------------------------------

    /// The scroll range is the two 240px placeholders (30% of 800), the
    /// header, `#content` (800) and the footer: 1380. The first commit
    /// opens on `#content` at 290. Without the top end, it opens at 0.
    #[test]
    fn the_view_opens_on_its_content_with_the_ends_out_of_view() {
        for (attributes, height, offset) in [
            (vec![], 1380.0, 290.0),
            (vec![("enable-refresh", "false")], 1090.0, 0.0),
            (vec![("enable-loadmore", "false")], 1090.0, 290.0),
        ] {
            let mut built = Built::new(&attributes, "view");
            built.document.commit();
            let container = built.container();
            let scroll_box = built
                .document
                .scroll_box(container)
                .expect("`#container` scrolls");
            assert_eq!(scroll_box.scrollport.height, 800.0, "{attributes:?}");
            assert_eq!(scroll_box.scroll_size.height, height, "{attributes:?}");
            assert_eq!(
                built.document.scroll_offset(container),
                Vector2D::new(0.0, offset),
                "{attributes:?}"
            );
            let content = built
                .document
                .rounded_layout(built.content)
                .expect("laid out");
            assert_eq!(
                (content.size.width, content.size.height),
                (393.0, 800.0),
                "{attributes:?}"
            );
        }
    }

    /// An author `position: absolute` on the header, as web-core's demo card
    /// writes it, loses to the UA's `!important`: the header stays in the
    /// column and the range keeps its 50px.
    #[test]
    fn an_author_absolute_header_stays_in_the_column() {
        let mut built = Built::new(&[], "view");
        built.document.add_stylesheet(
            "x-refresh-header { position: absolute !important; }",
            StylesheetOrigin::Author,
        );
        built
            .document
            .set_inline_style(built.header, "height: 50px; position: absolute");
        built.document.commit();
        assert_eq!(value(&built.document, built.header, "position"), "relative");
        assert_eq!(value(&built.document, built.footer, "position"), "relative");
        let container = built.container();
        let scroll_box = built.document.scroll_box(container).expect("scrolls");
        assert_eq!(scroll_box.scroll_size.height, 1380.0);
        assert_eq!(
            built.document.scroll_offset(container),
            Vector2D::new(0.0, 290.0)
        );
    }

    // --- the snap alignment -------------------------------------------------

    /// At rest only `#content` snaps (290). A header at least 90% visible
    /// snaps at its start (240) and a footer at least 90% visible at its
    /// end (340); one less visible snaps nowhere.
    #[test]
    fn a_header_or_footer_pulled_nine_tenths_in_becomes_a_snap_position() {
        let mut built = Built::new(&[], "view");
        built.document.commit();
        assert_eq!(built.snap_points(), [290.0]);
        assert_eq!(
            value(&built.document, built.header, "scroll-snap-align"),
            "none"
        );
        assert_eq!(
            value(&built.document, built.footer, "scroll-snap-align"),
            "none"
        );
        assert_eq!(
            value(&built.document, built.content, "scroll-snap-align"),
            "none"
        );

        // Fully in view, then 92% (header top 4px above the scrollport).
        for y in [190.0, 0.0, 244.0] {
            built.scroll_to(y);
            assert_eq!(
                value(&built.document, built.header, "scroll-snap-align"),
                "start",
                "at {y}"
            );
            assert_eq!(built.snap_points(), [240.0, 290.0], "at {y}");
        }
        // 80% in view: `exit 20%`.
        built.scroll_to(250.0);
        assert_eq!(
            value(&built.document, built.header, "scroll-snap-align"),
            "none"
        );
        assert_eq!(built.snap_points(), [290.0]);

        for y in [490.0, 580.0, 336.0] {
            built.scroll_to(y);
            assert_eq!(
                value(&built.document, built.footer, "scroll-snap-align"),
                "end",
                "at {y}"
            );
            assert_eq!(built.snap_points(), [290.0, 340.0], "at {y}");
        }
        built.scroll_to(330.0);
        assert_eq!(
            value(&built.document, built.footer, "scroll-snap-align"),
            "none"
        );
        assert_eq!(built.snap_points(), [290.0]);
    }

    /// The alignment animations export nothing to the painter and need no
    /// main-thread tick: they re-sample when a scroll is adopted.
    #[test]
    fn the_snap_alignment_needs_no_ticks() {
        let mut built = Built::new(&[], "view");
        let frame = built.document.commit();
        assert!(frame.animation_slots().is_empty());
        assert!(!frame.needs_main_ticks() && !frame.has_live_curves());
    }

    // --- the scroll chain ---------------------------------------------------

    /// `#container` contains its chain; the content child, a scroller or
    /// not, chains on y whatever an author declares on it, and keeps its
    /// own x.
    #[test]
    fn the_content_child_chains_on_y_into_the_container() {
        let mut built = Built::new(&[], "scroll-view");
        built.document.set_attribute(built.content, "bounces", "");
        built.document.add_stylesheet(
            "scroll-view { overscroll-behavior: contain-bounce !important; }",
            StylesheetOrigin::Author,
        );
        built.document.layout();
        let container = built.container();
        for (property, expected) in [
            ("overscroll-behavior-x", "contain"),
            ("overscroll-behavior-y", "contain"),
            ("scroll-snap-type", "y mandatory"),
            ("contain", "content"),
        ] {
            assert_eq!(
                value(&built.document, container, property),
                expected,
                "{property}"
            );
        }
        assert_eq!(
            value(&built.document, built.content, "overscroll-behavior-y"),
            "auto"
        );
        assert_eq!(
            value(&built.document, built.content, "overscroll-behavior-x"),
            "contain-bounce"
        );
        for chrome in [built.header, built.footer] {
            assert_eq!(
                value(&built.document, chrome, "overscroll-behavior-y"),
                "auto"
            );
        }
    }

    /// The scroll chain from the slotted content child runs over the flat
    /// tree into `#container`: a pull past the inner scroller's top moves
    /// `#container`, and a push past its end moves it the other way.
    #[test]
    fn a_pull_on_the_content_scrolls_the_container() {
        let mut built = Built::new(&[], "scroll-view");
        let tall = element_under(
            &mut built.document,
            built.content,
            "view",
            "flex-shrink: 0; height: 1000px",
        );
        built.document.commit();
        let container = built.container();
        let moved = built
            .document
            .scroll_chain(tall, Vector2D::new(0.0, -100.0));
        assert_eq!(moved, Some((container, Vector2D::new(0.0, -100.0))));
        assert_eq!(
            built.document.scroll_offset(container),
            Vector2D::new(0.0, 190.0)
        );
        assert_eq!(
            built.document.scroll_offset(built.content),
            Vector2D::zero()
        );

        let moved = built.document.scroll_chain(tall, Vector2D::new(0.0, 400.0));
        assert_eq!(
            moved,
            Some((built.content, Vector2D::new(0.0, 400.0))),
            "the inner scroller goes first: 200 of its own, 200 into the container"
        );
        assert_eq!(
            built.document.scroll_offset(built.content),
            Vector2D::new(0.0, 200.0)
        );
        assert_eq!(
            built.document.scroll_offset(container),
            Vector2D::new(0.0, 390.0)
        );
    }
}
