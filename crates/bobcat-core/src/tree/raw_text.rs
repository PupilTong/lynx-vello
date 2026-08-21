//! The `raw-text` component: the join between how Lynx *writes* a text run
//! and what the engine can measure and paint.
//!
//! Script writes a run as an attribute — `__CreateRawText(value)` builds a
//! `raw-text` element and sets `text` on it — while shaping, line breaking,
//! and glyph painting downstream all speak the W3C text node. So this module
//! owns both halves of the join: a [`dom::CustomElement`] reflecting the
//! attribute into one text node, the way web-core's `RawTextAttributes` does
//! (`web-elements`' `RawText.ts`), and the UA rules that decide where a
//! carrier's run lays out ([`UA_RULES`]).
//!
//! The `text` element's own defaults are not here — they are
//! [`super::text`]'s, one module over.

use dom::{CustomElement, NodeId};

use super::LynxDocument;

/// Lynx's text carrier, and the attribute carrying its content.
const RAW_TEXT_TAG: &str = "raw-text";
const TEXT_ATTRIBUTE: &str = "text";

/// Where a carrier's run lays out, from `web-elements`' own `x-text.css`.
///
/// A `raw-text` generates no box of its own: it dissolves into the `text` it
/// is written inside, and renders nothing anywhere else — a stray carrier is
/// not content. `white-space-collapse: preserve-breaks` inherits into the
/// reflected run, making a literal newline in the attribute break the line,
/// which is the one place Lynx preserves one (its `white-space` grammar is
/// `normal | nowrap`, so nothing else can ask for this).
///
/// The opt-in rule earns its keep twice over: [`super::text`]'s defaults
/// suppress every child of a `text` that is not content, so without it a
/// carrier would generate no box even in the one place it belongs. It outranks
/// that suppression on specificity, not on source order.
pub(super) const UA_RULES: &str = "\
raw-text { display: none; white-space-collapse: preserve-breaks; }
text > raw-text, text > wrapper > raw-text { display: contents; }
";

/// Installs the component. Must run before any element could carry the tag,
/// which is [`Document::define`](dom::Document::define)'s own precondition.
pub(super) fn define(document: &mut LynxDocument) {
    document.define(RAW_TEXT_TAG, Box::new(RawText));
}

/// Keeps one text node holding the `text` attribute's current value.
///
/// An empty value carries no node at all, matching web-core's `if (newVal)`
/// guard: an empty run must not claim a line box's height.
struct RawText;

impl CustomElement<()> for RawText {
    fn observed_attributes(&self) -> Vec<String> {
        vec![TEXT_ATTRIBUTE.to_owned()]
    }

    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        old: Option<&str>,
        new: Option<&str>,
    ) {
        debug_assert_eq!(name, TEXT_ATTRIBUTE, "`raw-text` observes `text` alone");
        if old == new {
            // `setAttribute` raises this reaction even when it writes the
            // value that was already there, and nothing but this component
            // touches the node it reflects into — so the run is already
            // right, and re-pointing it would cost a reshape and a relayout.
            return;
        }
        reflect_text(document, element, new.unwrap_or_default());
    }
}

/// The text node a `raw-text` element currently owns.
///
/// A reflected run is always its element's *first* child, whatever else is
/// attached to a carrier, so this stays a single link read.
#[must_use]
fn owned_text_node(document: &LynxDocument, element: NodeId) -> Option<NodeId> {
    document
        .get(element)?
        .first_child()
        .filter(|child| child.is_text_node())
        .map(dom::Node::id)
}

/// Points the element's text node at `text`, minting or freeing it as the
/// value becomes non-empty or empty.
///
/// The in-place update is the reason this is not web-core's remove-and-append:
/// a text node keeps its retained Parley layout and its layout cache under its
/// own id, so re-pointing one costs a reshape while replacing it would also
/// churn an arena slot and discard both.
fn reflect_text(document: &mut LynxDocument, element: NodeId, text: &str) {
    match (owned_text_node(document, element), text.is_empty()) {
        (Some(node), false) => document.set_text_node_data(node, text),
        (Some(node), true) => document.drop_element(node),
        (None, false) => {
            let node = document.create_text_node(text, ());
            let first = document
                .get(element)
                .and_then(dom::Node::first_child)
                .map(dom::Node::id);
            document.insert_before(element, node, first);
        }
        (None, true) => {}
    }
}

/// Frees `element` together with the text node a `raw-text` owns.
///
/// [`Document::drop_element`](dom::Document::drop_element) leaves an element's
/// children detached rather than freed, because a child element can still be
/// named by a live script handle. A reflected text node never can be — the
/// realm mints no handle for one — so its element's release is the only
/// occasion it could ever be freed on.
pub(crate) fn drop_element_and_owned_text(document: &mut LynxDocument, element: NodeId) {
    if let Some(text) = owned_text_node(document, element) {
        document.drop_element(text);
    }
    document.drop_element(element);
}

#[cfg(test)]
mod tests {
    use dom::NodeId;

    use super::super::test_support::{child, display, document};
    use super::{
        LynxDocument, RAW_TEXT_TAG, TEXT_ATTRIBUTE, drop_element_and_owned_text, owned_text_node,
    };

    /// Solid em squares, so a run's box is its glyph count times its font size.
    const AHEM: &[u8] = include_bytes!("../../../hughie/tests/fixtures/Ahem.ttf");

    /// A `text` element under the page, styled to Ahem's exact metrics.
    fn text_element(document: &mut LynxDocument) -> NodeId {
        child(document, "text", "font-family: Ahem; font-size: 20px")
    }

    /// What `__CreateRawText` does: mint the carrier, write the run on it,
    /// then attach it.
    fn raw_text(document: &mut LynxDocument, parent: NodeId, text: &str) -> NodeId {
        let element = document.create_element(RAW_TEXT_TAG, ());
        document.set_attribute(element, TEXT_ATTRIBUTE, text);
        document.append_child(parent, element);
        element
    }

    fn run_of(document: &LynxDocument, element: NodeId) -> NodeId {
        owned_text_node(document, element).expect("a non-empty raw-text carries a text node")
    }

    #[test]
    fn a_raw_text_reflects_its_attribute_into_one_reused_text_node() {
        let mut document = document();
        let text = text_element(&mut document);
        let raw = raw_text(&mut document, text, "hello");

        let run = run_of(&document, raw);
        assert_eq!(document.get(run).and_then(dom::Node::text), Some("hello"));
        assert_eq!(document.get(raw).expect("the carrier").child_ids(), [run]);

        document.set_attribute(raw, TEXT_ATTRIBUTE, "world");

        assert_eq!(
            owned_text_node(&document, raw),
            Some(run),
            "an update re-points the run's node instead of replacing it"
        );
        assert_eq!(document.get(run).and_then(dom::Node::text), Some("world"));

        document.set_attribute(raw, TEXT_ATTRIBUTE, "world");

        assert_eq!(
            (
                owned_text_node(&document, raw),
                document.get(run).and_then(dom::Node::text)
            ),
            (Some(run), Some("world")),
            "and rewriting the value it already had leaves the run alone"
        );
    }

    #[test]
    fn a_reflected_run_is_the_carrier_s_first_child_whatever_else_is_attached() {
        let mut document = document();
        let text = text_element(&mut document);
        let carrier = document.create_element(RAW_TEXT_TAG, ());
        document.append_child(text, carrier);
        let foreign = document.create_element("view", ());
        document.append_child(carrier, foreign);

        document.set_attribute(carrier, TEXT_ATTRIBUTE, "hello");
        let run = run_of(&document, carrier);
        document.set_attribute(carrier, TEXT_ATTRIBUTE, "world");

        assert_eq!(
            document.get(carrier).expect("the carrier").child_ids(),
            [run, foreign],
            "the run stays the first child, so a second update finds it \
             instead of minting another node"
        );
    }

    #[test]
    fn an_empty_value_carries_no_text_node_at_all() {
        let mut document = document();
        let text = text_element(&mut document);
        let raw = raw_text(&mut document, text, "hello");
        let run = run_of(&document, raw);

        document.set_attribute(raw, TEXT_ATTRIBUTE, "");
        assert_eq!(owned_text_node(&document, raw), None);
        assert!(document.get(run).is_none(), "the emptied node is freed");

        document.set_attribute(raw, TEXT_ATTRIBUTE, "again");
        assert_eq!(
            document
                .get(run_of(&document, raw))
                .and_then(dom::Node::text),
            Some("again"),
            "a value arriving after an empty one mints a node again"
        );

        document.remove_attribute(raw, TEXT_ATTRIBUTE);
        assert_eq!(
            owned_text_node(&document, raw),
            None,
            "removing the attribute clears the run, like emptying it"
        );
    }

    #[test]
    fn releasing_a_raw_text_frees_the_text_node_no_handle_can_name() {
        let mut document = document();
        let text = text_element(&mut document);
        let raw = raw_text(&mut document, text, "hello");
        let run = run_of(&document, raw);

        drop_element_and_owned_text(&mut document, raw);

        assert!(document.get(raw).is_none());
        assert!(
            document.get(run).is_none(),
            "the run's node dies with its carrier: nothing else would ever free it"
        );
    }

    #[test]
    fn the_ua_sheet_dissolves_a_raw_text_only_inside_the_text_it_is_written_in() {
        use dom::stylo::values::computed::Display;

        let mut document = document();
        let page = document.document_element().id();
        let text = text_element(&mut document);
        let direct = raw_text(&mut document, text, "direct");
        let wrapper = document.create_element("wrapper", ());
        document.append_child(text, wrapper);
        let inside = raw_text(&mut document, wrapper, "wrapped");
        let view = document.create_element("view", ());
        document.append_child(page, view);
        let stray = raw_text(&mut document, view, "stray");
        document.layout();

        assert_eq!(display(&document, text), Display::Flex);
        assert_eq!(display(&document, wrapper), Display::Contents);
        assert_eq!(display(&document, direct), Display::Contents);
        assert_eq!(display(&document, inside), Display::Contents);
        assert_eq!(
            display(&document, stray),
            Display::None,
            "a carrier written outside a `text` renders nothing, as in web-core"
        );
    }

    #[test]
    fn a_text_element_is_sized_by_the_run_its_raw_text_carries() {
        let mut document = document();
        assert_eq!(document.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
        let text = text_element(&mut document);
        let raw = raw_text(&mut document, text, "hello");
        document.layout();

        let run = document
            .rounded_layout(run_of(&document, raw))
            .expect("the run is laid out");
        assert!(
            (run.size.width - 100.0).abs() < f32::EPSILON
                && (run.size.height - 20.0).abs() < f32::EPSILON,
            "five Ahem em squares at 20px, got {:?}",
            run.size
        );

        let box_ = document.rounded_layout(text).expect("the text is laid out");
        assert!(
            (box_.size.height - 20.0).abs() < f32::EPSILON,
            "the text element takes the run's height, got {:?}",
            box_.size
        );
    }

    #[test]
    fn a_literal_newline_in_the_attribute_breaks_the_line() {
        let mut document = document();
        assert_eq!(document.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
        let text = text_element(&mut document);
        let raw = raw_text(&mut document, text, "ab\ncd");
        document.layout();

        let run = document
            .rounded_layout(run_of(&document, raw))
            .expect("the run is laid out");
        assert!(
            (run.size.width - 40.0).abs() < f32::EPSILON
                && (run.size.height - 40.0).abs() < f32::EPSILON,
            "two lines of two em squares, got {:?}",
            run.size
        );
    }
}
