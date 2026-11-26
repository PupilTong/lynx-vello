//! Replications of `lynx-stack`'s web-platform text fixtures, at the tree
//! layer: what a `text` element's subtree turns into.
//!
//! The reference is web-core, not native Lynx (`AGENTS.md`'s compatibility
//! target): where the two disagree these tests follow the browser the
//! `.web.bundle` runs in today. Each test names the fixture and the spec line
//! it comes from, and the original assertion is always the same one — a
//! whole-page screenshot diff at `maxDiffPixelRatio: 0`. A screenshot is not
//! portable, so each replica splits that diff into the two claims it actually
//! carries: geometry, which this file asserts against the vendored Ahem face
//! (solid em squares, so an advance is exactly glyph count times font size),
//! and paint, which belongs to `crates/terminus`'s pixel tests.
//!
//! One deliberate exception to that rule is on record. The truncation marker
//! `text-maxline` and `text-maxlength` append is gated on
//! `text-overflow: ellipsis` here (`crates/seldon/src/text/block/truncate.rs`),
//! which neither the initial value `clip` nor the Lynx UA sheet ever sets. Both
//! references append it unconditionally for `text-maxlength`, and web-core does
//! so for `text-maxline` too. The 2026-09-14 ruling adopts this engine's gating
//! as the intended behavior, so the maxlength replicas below assert the bare cut
//! and state in their own doc comments what the references render instead.
//!
//! The fixtures are written in web-core's custom-element vocabulary; this
//! engine spells the same tags without the prefix. `x-text` is `text`,
//! `x-view` is `view`, `x-image` is `image`, and `lynx-wrapper` is `wrapper`.
//! `--lynx-display` and `--lynx-display-toggle`, which web-core needs because
//! it polyfills Lynx's layout modes onto CSS flexbox, have no counterpart
//! either: this engine has the modes, so a fixture's toggle pair becomes the
//! `display` the toggle was selecting. Where a fixture's viewport is wider than
//! this document's 393px phone, the replica gives the paragraph the explicit
//! width that keeps the fixture's line breaking, and says so.
//!
//! One divergence cuts across most of these fixtures and belongs to none of
//! them: the markup indents its content. A browser collapses that
//! indentation — a newline and the spaces after it to one space, a space at
//! the start of a line removed — while a `<text>` here keeps its text
//! verbatim, as every native Lynx backend does (the user's ruling of
//! 2026-10-16, `docs/tracking/deviations.md`): the newline is a forced break
//! and the spaces stay. A compiled `ReactLynx` card never carries HTML
//! indentation into a `<text>` — its compiler trims it — so where a case's
//! claim is something other than the collapsing, the replica writes the
//! fixture's content the way such a card carries it: what the browser's
//! collapsing leaves, as literal text, and the test's own doc comment says
//! so. Replicas that compare two trees carrying the same indentation keep the
//! markup, since the comparison holds whatever the indentation does. The cases
//! whose claim *is* the collapsing deviate from web-core: they keep asserting
//! it, ignored as DEVIATIONs, and a companion asserts what the engine
//! renders.

// Ahem and explicit line heights have exact metrics, so the geometry these
// replicas assert is compared exactly; glyph counts convert to advances.
#![allow(clippy::float_cmp, clippy::cast_precision_loss, clippy::similar_names)]

use terminus::link::NodeId;
use terminus::link::stylo::color::AbsoluteColor;
use terminus::link::stylo::properties::PropertyId;
use terminus::link::stylo::values::computed::{ColorPropertyValue, Display};

use super::test_support::{child, display, element_under, style_of, with_config};
use super::{LynxDocument, PageConfig};

const AHEM: &[u8] = include_bytes!("../../../../seldon/tests/fixtures/Ahem.ttf");

const TEXT_ATTRIBUTE: &str = "text";

const IMAGE_SRC: &str = "/tests/fixtures/resources/inline-image.png";

fn ahem_document() -> LynxDocument {
    ahem_document_with(PageConfig::default())
}

fn ahem_document_with(config: PageConfig) -> LynxDocument {
    let mut document = with_config(config);
    assert_eq!(
        document.register_fonts(terminus::link::FontBlob::from_static(AHEM)),
        1
    );
    assert!(document.set_default_font_family("Ahem"));
    document
}

fn computed(document: &LynxDocument, element: NodeId, property: &str) -> String {
    let id = PropertyId::parse_enabled_for_all_content(property)
        .unwrap_or_else(|()| panic!("unknown property `{property}`"));
    let declaration = id
        .as_shorthand()
        .err()
        .unwrap_or_else(|| panic!("`{property}` is a shorthand"));
    style_of(document, element).computed_value_to_string(declaration)
}

fn literal(document: &mut LynxDocument, parent: NodeId, content: &str) {
    let node = document.create_text_node(content, ());
    document.append_child(parent, node);
}

fn raw_text(document: &mut LynxDocument, parent: NodeId, content: &str) -> NodeId {
    let carrier = element_under(document, parent, "raw-text", "");
    document.set_attribute(carrier, TEXT_ATTRIBUTE, content);
    carrier
}

fn image(document: &mut LynxDocument, parent: NodeId, style: &str) -> NodeId {
    let element = element_under(document, parent, "image", style);
    document.set_attribute(element, "src", IMAGE_SRC);
    element
}

fn set_limit(document: &mut LynxDocument, element: NodeId, name: &str, value: &str) {
    document.set_attribute(element, name, value);
}

fn ink(document: &LynxDocument, id: NodeId) -> (f32, f32) {
    let size = document.text_block_size(id).expect("a laid-out paragraph");
    (size.width, size.height)
}

fn frame(document: &LynxDocument, id: NodeId) -> (f32, f32, f32, f32) {
    let layout = document.rounded_layout(id).expect("a laid-out box");
    (
        layout.location.x,
        layout.location.y,
        layout.size.width,
        layout.size.height,
    )
}

fn horizontal_frame(document: &LynxDocument, id: NodeId) -> (f32, f32, f32) {
    let (x, _, width, height) = frame(document, id);
    (x, width, height)
}

fn placement(document: &LynxDocument, id: NodeId) -> Option<(f32, f32, f32)> {
    let layout = document.rounded_layout(id)?;
    Some((layout.location.x, layout.size.width, layout.size.height))
}

#[test]
fn a_text_attribute_on_a_text_element_carries_the_paragraph_s_run() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "font-size: 24px; color: blue");
    document.set_attribute(text, TEXT_ATTRIBUTE, "hello world");
    literal(&mut document, text, " ");
    document.layout();

    assert_eq!(
        ink(&document, text),
        (11.0 * 24.0, 24.0),
        "eleven em squares on one line, and the whitespace child collapses away"
    );
}

#[test]
fn a_view_child_of_a_text_is_one_atomic_inline_box_laid_out_on_its_own() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "");
    let view = element_under(
        &mut document,
        text,
        "view",
        "width: 200px; height: 200px; flex-direction: column",
    );
    let atoms: Vec<NodeId> = (0..3)
        .map(|_| {
            element_under(
                &mut document,
                view,
                "view",
                "width: 50px; height: 20px; border: 1px solid red",
            )
        })
        .collect();
    document.layout();

    assert_eq!(
        display(&document, view),
        Display::Flex,
        "a view written inside a text is content of that paragraph, not one of \
         the children the text suppresses"
    );
    assert_eq!(frame(&document, view), (0.0, 0.0, 200.0, 200.0));
    for (index, atom) in atoms.iter().enumerate() {
        assert_eq!(
            frame(&document, *atom),
            (0.0, 20.0 * index as f32, 50.0, 20.0),
            "the atom's own container algorithm runs independently of line \
             breaking: box {index}"
        );
    }
    assert_eq!(
        ink(&document, text).0,
        200.0,
        "and the paragraph advances by exactly the one unbreakable unit it holds"
    );
}

#[test]
fn a_container_nested_inside_an_inline_atom_sizes_itself_without_re_entering_the_line() {
    const BOX: &str = "width: 50px; height: 20px; border: 1px solid red";

    let mut document = ahem_document();
    let text = child(&mut document, "text", "");
    let outer = element_under(
        &mut document,
        text,
        "view",
        "width: 200px; height: 200px; flex-direction: column",
    );
    let inner = element_under(&mut document, outer, "view", "");
    let nested: Vec<NodeId> = (0..3)
        .map(|_| element_under(&mut document, inner, "view", BOX))
        .collect();
    let trailing: Vec<NodeId> = (0..2)
        .map(|_| element_under(&mut document, outer, "view", BOX))
        .collect();
    document.layout();

    assert_eq!(
        frame(&document, inner),
        (0.0, 0.0, 200.0, 60.0),
        "the inner container takes its height from its three 20px children"
    );
    for (index, atom) in nested.iter().enumerate() {
        assert_eq!(
            frame(&document, *atom),
            (0.0, 20.0 * index as f32, 50.0, 20.0)
        );
    }
    for (index, atom) in trailing.iter().enumerate() {
        assert_eq!(
            frame(&document, *atom),
            (0.0, 60.0 + 20.0 * index as f32, 50.0, 20.0),
            "the two remaining boxes follow the inner container in the outer flow"
        );
    }
    assert_eq!(
        ink(&document, text).0,
        200.0,
        "the outer view is still measured as one inline unit"
    );
}

#[test]
#[ignore = "DEVIATION, not a missing feature: this engine follows native \
            Lynx, by the user's ruling of 2026-10-16. A `<text>` does no CSS \
            white-space processing on any native backend \
            (`raw_text_element.cc:16-46`, Android \
            `BaseTextShadowNode.java:494-498`, iOS \
            `LynxBaseTextShadowNode.m:240-249`), so the markup's newlines are \
            forced breaks and its indentation stays: four lines 312 wide, not \
            web-core's one line 504 wide. \
            `a_nested_text_run_keeps_the_markup_s_newlines_in_a_text` asserts \
            that and passes."]
fn a_text_nested_in_a_text_is_an_inline_run_that_scopes_only_its_own_colour() {
    const PARAGRAPH: &str = "width: 600px; font-size: 24px; font-weight: bold";
    const RED: &str = "font-size: 24px; color: red";

    let mut document = ahem_document();
    let text = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, text, "\n    I am bold\n    ");
    let nested = element_under(&mut document, text, "text", RED);
    literal(&mut document, nested, "and red");
    literal(&mut document, text, "\n  ");

    let joined = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, joined, "\n    I am bold");
    let joined_run = element_under(&mut document, joined, "text", RED);
    literal(&mut document, joined_run, "and red");
    document.layout();

    assert_eq!(display(&document, nested), Display::Flex);
    assert_eq!(document.paragraph_of(nested), Some(text));
    assert_eq!(
        ink(&document, text),
        (ink(&document, joined).0 + 24.0, 24.0),
        "one line, one square wider than the same content written without that \
         whitespace: the newline and its indentation collapsed to a single space"
    );
    assert_eq!(
        style_of(&document, nested).get_color().solid_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0),
        "the nested run overrides the colour it is handed"
    );
    assert_eq!(
        *style_of(&document, nested).get_font_weight(),
        *style_of(&document, text).get_font_weight(),
        "and still inherits the weight the paragraph was given"
    );
}

#[test]
fn a_nested_text_run_keeps_the_markup_s_newlines_in_a_text() {
    const PARAGRAPH: &str = "width: 600px; font-size: 24px; font-weight: bold";
    const RED: &str = "font-size: 24px; color: red";

    let mut document = ahem_document();
    let text = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, text, "\n    I am bold\n    ");
    let nested = element_under(&mut document, text, "text", RED);
    literal(&mut document, nested, "and red");
    literal(&mut document, text, "\n  ");
    document.layout();

    assert_eq!(display(&document, nested), Display::Flex);
    assert_eq!(document.paragraph_of(nested), Some(text));
    assert_eq!(
        ink(&document, text),
        (13.0 * 24.0, 4.0 * 24.0),
        "every newline breaks the line and every space stays"
    );
    assert_eq!(
        style_of(&document, nested).get_color().solid_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0),
    );
    assert_eq!(
        *style_of(&document, nested).get_font_weight(),
        *style_of(&document, text).get_font_weight(),
    );
}

#[test]
fn a_wrapper_between_a_text_and_its_nested_run_changes_nothing() {
    const PARAGRAPH: &str = "width: 600px; font-size: 24px; font-weight: bold";
    const RED: &str = "font-size: 24px; color: red";

    let mut document = ahem_document();
    let plain = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, plain, "\n    I am bold\n    ");
    let direct = element_under(&mut document, plain, "text", RED);
    literal(&mut document, direct, "and red");

    let wrapped = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, wrapped, "\n    I am bold\n    ");
    let wrapper = element_under(&mut document, wrapped, "wrapper", "");
    let through = element_under(&mut document, wrapper, "text", RED);
    literal(&mut document, through, "and red");
    document.layout();

    assert_eq!(display(&document, wrapper), Display::Contents);
    assert_eq!(display(&document, through), Display::Flex);
    assert_eq!(document.paragraph_of(through), Some(wrapped));
    assert_eq!(
        ink(&document, wrapped),
        ink(&document, plain),
        "the wrapper dissolves: same runs, same line, same advance"
    );
    assert_eq!(
        style_of(&document, through).get_color().solid_color(),
        style_of(&document, direct).get_color().solid_color(),
    );
}

#[test]
fn splitting_a_paragraph_into_two_runs_leaves_its_line_geometry_untouched() {
    const PARAGRAPH: &str = "width: 600px; font-size: 24px; font-weight: bold";

    let mut document = ahem_document();
    let split = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, split, "I am bold ");
    let nested = element_under(&mut document, split, "text", "font-size: 24px; color: red");
    literal(&mut document, nested, "and red");

    let undivided = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, undivided, "I am bold and red");
    document.layout();

    assert_eq!(
        ink(&document, split),
        ink(&document, undivided),
        "the nested run shares the parent's line box instead of opening one"
    );
    assert_eq!(
        *style_of(&document, nested).get_font_weight(),
        *style_of(&document, split).get_font_weight(),
    );
    assert_ne!(
        style_of(&document, nested).get_color().solid_color(),
        style_of(&document, split).get_color().solid_color(),
    );
}

#[test]
fn a_maxline_on_a_nested_inline_run_neither_clips_nor_re_boxes_it() {
    const PARAGRAPH: &str = "width: 600px; font-size: 24px; font-weight: bold";
    const RED: &str = "font-size: 24px; color: red";

    let mut document = ahem_document();
    let control = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, control, "\n    I am bold\n    ");
    let plain = element_under(&mut document, control, "text", RED);
    literal(&mut document, plain, "and red");

    let faulty = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, faulty, "\n    I am bold\n    ");
    let limited = element_under(&mut document, faulty, "text", RED);
    literal(&mut document, limited, "and red");
    set_limit(&mut document, limited, "text-maxline", "1");
    document.layout();

    assert_eq!(
        ink(&document, faulty),
        ink(&document, control),
        "the nested limit is ignored: the paragraph still renders both runs \
         unclipped on one line"
    );
    assert_eq!(display(&document, limited), display(&document, plain));
}

#[test]
fn runs_of_different_size_share_a_baseline_while_sibling_text_blocks_do_not() {
    let mut document = ahem_document();

    let stacked = child(&mut document, "view", "");
    let small = element_under(&mut document, stacked, "text", "font-size: 20px");
    literal(&mut document, small, "hello world");
    let large = element_under(&mut document, stacked, "text", "font-size: 30px");
    literal(&mut document, large, "111");

    let row = child(&mut document, "view", "display: flex; flex-direction: row");
    let row_small = element_under(&mut document, row, "text", "font-size: 20px");
    literal(&mut document, row_small, "hello world");
    let row_large = element_under(&mut document, row, "text", "font-size: 30px");
    literal(&mut document, row_large, "111");

    let holder = child(&mut document, "view", "");
    let paragraph = element_under(&mut document, holder, "text", "font-size: 20px");
    let run_small = element_under(&mut document, paragraph, "text", "font-size: 20px");
    literal(&mut document, run_small, "hello world");
    literal(&mut document, paragraph, " ");
    let run_large = element_under(&mut document, paragraph, "text", "font-size: 30px");
    literal(&mut document, run_large, "111");
    document.layout();

    assert_eq!(
        (frame(&document, small).1, frame(&document, large).1),
        (0.0, 20.0),
        "sibling text blocks are block-level: the second follows the first down"
    );
    assert_eq!(
        (frame(&document, row_small).0, frame(&document, row_large).0),
        (0.0, 11.0 * 20.0),
        "as flex items they are placed side by side by the container, not by \
         any baseline of their own"
    );
    assert_eq!(
        ink(&document, paragraph).1,
        30.0,
        "one line whose height is the 30px run's own em box: both runs hang \
         from the same baseline instead of stacking"
    );
}

#[test]
fn an_image_child_of_a_text_advances_the_line_by_its_used_size() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, text, "abcd ");
    let inline = image(&mut document, text, "width: 22px; height: 22px");
    document.layout();

    assert_eq!(display(&document, inline), Display::Flex);
    assert_eq!(
        horizontal_frame(&document, inline),
        (4.0 * 24.0 + 24.0, 22.0, 22.0),
        "the replaced box is its specified 22x22, and follows the run plus one \
         collapsed space"
    );
    assert_eq!(
        ink(&document, text).0,
        frame(&document, inline).0 + 22.0,
        "the box is the last thing on the line, so the line ends where it does"
    );
}

#[test]
fn a_wrapper_between_a_text_and_an_inline_image_changes_nothing() {
    let mut document = ahem_document();
    let plain = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, plain, "abcd ");
    let direct = image(&mut document, plain, "width: 22px; height: 22px");

    let wrapped = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, wrapped, "abcd ");
    let wrapper = element_under(&mut document, wrapped, "wrapper", "");
    let through = image(&mut document, wrapper, "width: 22px; height: 22px");
    document.layout();

    assert_eq!(display(&document, wrapper), Display::Contents);
    assert_eq!(display(&document, through), Display::Flex);
    assert_eq!(
        ink(&document, wrapped),
        ink(&document, plain),
        "the wrapper contributes nothing to the paragraph"
    );
    assert_eq!(
        frame(&document, through),
        frame(&document, direct),
        "and the atom below it is placed exactly where the unwrapped one is"
    );
    assert_eq!(
        horizontal_frame(&document, through),
        (4.0 * 24.0 + 24.0, 22.0, 22.0),
        "stated absolutely as well as against the control, so that a wrapper \
         cannot be certified transparent over two identically-invisible boxes"
    );
}

#[test]
fn an_inline_image_s_margin_reaches_the_line_s_advance_and_its_padding_does_not() {
    let mut document = ahem_document();
    let margined = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, margined, "abcd ");
    let outer = image(
        &mut document,
        margined,
        "width: 22px; height: 22px; margin-left: 50px",
    );

    let padded = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, padded, "abcd ");
    let inner = image(
        &mut document,
        padded,
        "width: 22px; height: 22px; padding-left: 50px",
    );
    document.layout();

    assert_eq!(
        ink(&document, margined).0,
        4.0 * 24.0 + 24.0 + 50.0 + 22.0,
        "the margin is outer advance: the line grows by it ahead of the box"
    );
    assert_eq!(
        horizontal_frame(&document, outer),
        (4.0 * 24.0 + 24.0 + 50.0, 22.0, 22.0),
        "and the border box itself is still the specified 22x22"
    );
    assert_eq!(
        ink(&document, padded).0,
        4.0 * 24.0 + 24.0 + 22.0,
        "the padding never reaches a box: the host is `display: contents` and \
         the box on the line inherits everything but padding, so this paragraph \
         advances by the same 22 the unstyled one does"
    );
    assert_eq!(
        horizontal_frame(&document, inner),
        (4.0 * 24.0 + 24.0, 22.0, 22.0),
        "and the box is still the specified 22x22, sitting where an unpadded \
         one would"
    );
    assert!(
        frame(&document, margined).1 < frame(&document, padded).1,
        "the two text elements are block-level, so they occupy separate lines"
    );
}

#[test]
fn adjacent_runs_written_without_whitespace_shape_as_one_uninterrupted_line() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "font-size: 16px");
    literal(&mut document, text, "helloworld");
    let nested = element_under(&mut document, text, "text", "");
    literal(&mut document, nested, "lynxweb");
    literal(&mut document, text, "hello");
    document.layout();

    assert_eq!(
        ink(&document, text),
        (22.0 * 16.0, 16.0),
        "twenty-two em squares with nothing inserted at either run boundary"
    );
}

#[test]
#[ignore = "DEVIATION, not a missing feature: this engine follows native \
            Lynx, by the user's ruling of 2026-10-16. A `<text>` does no CSS \
            white-space processing on any native backend \
            (`raw_text_element.cc:16-46`, Android \
            `BaseTextShadowNode.java:494-498`, iOS \
            `LynxBaseTextShadowNode.m:240-249`), so `\"Hello,\\n      web!\"` \
            is two lines, the second indented: 200 by 40, not web-core's 220 \
            by 20. \
            `adjacent_text_siblings_are_separate_blocks_that_keep_their_own_newlines` \
            asserts that and passes."]
fn adjacent_text_siblings_are_separate_blocks_and_collapse_their_own_newlines() {
    let mut document = ahem_document();
    let column = child(
        &mut document,
        "view",
        "display: flex; flex-direction: column",
    );
    let first = element_under(&mut document, column, "text", "font-size: 20px");
    literal(&mut document, first, "Hello,\n      web!");
    let second = element_under(&mut document, column, "text", "font-size: 20px");
    literal(&mut document, second, "Hello, Speedy!");
    document.layout();

    assert_eq!(
        ink(&document, first),
        (11.0 * 20.0, 20.0),
        "the newline and its indentation collapse to the one space between the \
         two words"
    );
    assert_eq!(
        (frame(&document, first).1, frame(&document, second).1),
        (0.0, 20.0),
        "no whitespace between the tags, and still two boxes: a text is not an \
         inline element"
    );
}

#[test]
fn adjacent_text_siblings_are_separate_blocks_that_keep_their_own_newlines() {
    let mut document = ahem_document();
    let column = child(
        &mut document,
        "view",
        "display: flex; flex-direction: column",
    );
    let first = element_under(&mut document, column, "text", "font-size: 20px");
    literal(&mut document, first, "Hello,\n      web!");
    let second = element_under(&mut document, column, "text", "font-size: 20px");
    literal(&mut document, second, "Hello, Speedy!");
    document.layout();

    assert_eq!(ink(&document, first), (10.0 * 20.0, 2.0 * 20.0));
    assert_eq!(
        (frame(&document, first).1, frame(&document, second).1),
        (0.0, 40.0),
        "two boxes, the second below the first's two lines"
    );
}

#[test]
fn a_flex_view_inside_a_text_shrinks_to_fit_and_stays_on_the_line() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "width: 600px; font-size: 20px");
    literal(&mut document, text, "\n    this is text ");
    let atom = element_under(
        &mut document,
        text,
        "view",
        "display: flex; flex-direction: row",
    );
    for _ in 0..3 {
        element_under(
            &mut document,
            atom,
            "view",
            "width: 50px; height: 20px; border: 1px solid red; margin: 2px",
        );
    }
    document.layout();

    assert_eq!(
        (frame(&document, atom).2, frame(&document, atom).3),
        (3.0 * 54.0, 24.0),
        "three border-box items and their margins shrink-wrap the atom to 162"
    );
    assert_eq!(
        ink(&document, text).0,
        frame(&document, atom).0 + 3.0 * 54.0,
        "and the atom ends the one line the paragraph occupies, so its width is \
         what the line advanced by"
    );
}

#[test]
fn a_raw_text_carrier_contributes_a_run_and_no_box_of_its_own() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "font-size: 24px; color: blue");
    let carrier = raw_text(&mut document, text, "abcd");
    literal(&mut document, text, " ");
    let inline = image(&mut document, text, "width: 22px; height: 22px");
    document.layout();

    assert_eq!(
        display(&document, carrier),
        Display::Contents,
        "the carrier dissolves into the text it is written inside"
    );
    assert_eq!(
        (frame(&document, inline).2, frame(&document, inline).3),
        (22.0, 22.0),
    );
    assert_eq!(
        ink(&document, text).0,
        frame(&document, inline).0 + 22.0,
        "the carrier's run, the collapsed space, then the atom that ends the line"
    );
}

#[test]
fn a_wrapper_between_a_text_and_a_raw_text_carrier_changes_nothing() {
    let mut document = ahem_document();
    let plain = child(&mut document, "text", "font-size: 24px; color: blue");
    raw_text(&mut document, plain, "abcd");
    literal(&mut document, plain, " ");
    let direct = image(&mut document, plain, "width: 22px; height: 22px");

    let wrapped = child(&mut document, "text", "font-size: 24px; color: blue");
    let wrapper = element_under(&mut document, wrapped, "wrapper", "");
    let carrier = raw_text(&mut document, wrapper, "abcd");
    literal(&mut document, wrapped, " ");
    let through = image(&mut document, wrapped, "width: 22px; height: 22px");
    document.layout();

    assert_eq!(display(&document, wrapper), Display::Contents);
    assert_eq!(display(&document, carrier), Display::Contents);
    assert_eq!(
        ink(&document, wrapped),
        ink(&document, plain),
        "same run, same order, same line"
    );
    assert_eq!(frame(&document, through), frame(&document, direct));
}

#[test]
fn the_image_beside_a_wrapped_carrier_is_a_real_box_on_the_line() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, text, "\n    ");
    let wrapper = element_under(&mut document, text, "wrapper", "");
    literal(&mut document, wrapper, "\n      ");
    raw_text(&mut document, wrapper, "abcd");
    literal(&mut document, wrapper, "\n    ");
    literal(&mut document, text, "\n    ");
    let inline = image(&mut document, text, "width: 22px; height: 22px");
    document.layout();

    let (_, _, width, height) = frame(&document, inline);
    assert_eq!(
        (width, height),
        (22.0, 22.0),
        "the picture beside the wrapped carrier is its specified 22x22, not a \
         zero-size box that happens to compare equal to another one"
    );
}

#[test]
fn truncation_content_is_skipped_entirely_when_no_maxline_is_declared() {
    const PARAGRAPH: &str = "width: 200px; font-size: 16px";

    let mut document = ahem_document();
    let text = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, text, "hello world ");
    let truncation = element_under(&mut document, text, "inline-truncation", "");
    literal(&mut document, truncation, "!!!!!!!");

    let without = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, without, "hello world ");
    document.layout();

    assert_eq!(
        display(&document, truncation),
        Display::Flex,
        "a text's own inline-truncation child is shown by the cascade; \
         whether its content is shown is the paragraph's decision, not the \
         cascade's"
    );
    assert_eq!(
        document.paragraph_of(truncation),
        Some(text),
        "and it is a text scope of that paragraph"
    );
    assert_eq!(
        ink(&document, text),
        ink(&document, without),
        "and with no clamp to overflow it contributes nothing at all to the \
         paragraph that holds it"
    );
    assert_eq!(
        ink(&document, text).1,
        16.0,
        "one line: the seven exclamation marks did not open a second"
    );
}

const CLAMPED: &str = "width: 200px; font-size: 16px; word-break: break-all";

#[test]
fn truncation_content_is_laid_in_at_the_clamp_a_maxline_overflows() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", CLAMPED);
    literal(&mut document, text, &"a".repeat(24));
    let truncation = element_under(&mut document, text, "inline-truncation", "");
    let icon = image(&mut document, truncation, "width: 32px; height: 22px");
    set_limit(&mut document, text, "text-maxline", "1");
    document.layout();

    assert_eq!(
        display(&document, truncation),
        Display::Flex,
        "the truncation subtree is shown by the cascade"
    );
    assert_eq!(
        document.paragraph_of(truncation),
        Some(text),
        "it is a text scope, and the paragraph overflows the one line it is \
         allowed, so its content is laid in"
    );
    assert_eq!(
        placement(&document, icon),
        Some((10.0 * 16.0, 32.0, 22.0)),
        "the cut retreats the two squares the 32px content needs, and the \
         content takes the width they vacated"
    );
    assert_eq!(
        ink(&document, text).0,
        12.0 * 16.0,
        "one clamped line, still filling the measure: ten kept squares and the \
         content that replaced the other two"
    );
}

#[test]
fn a_custom_truncation_s_content_replaces_the_clamp_marker_at_every_maxline() {
    let mut document = ahem_document();

    let unclamped = child(&mut document, "text", CLAMPED);
    literal(&mut document, unclamped, &"a".repeat(100));
    let unclamped_truncation = element_under(&mut document, unclamped, "inline-truncation", "");
    literal(&mut document, unclamped_truncation, "!!");

    let control = child(&mut document, "text", CLAMPED);
    literal(&mut document, control, &"a".repeat(100));

    let mut clamped = Vec::new();
    for limit in [1u32, 2, 4, 6] {
        let block = child(&mut document, "text", CLAMPED);
        literal(&mut document, block, &"a".repeat(100));
        let truncation = element_under(&mut document, block, "inline-truncation", "");
        literal(&mut document, truncation, "!!");
        set_limit(&mut document, block, "text-maxline", &limit.to_string());
        clamped.push((limit, block, truncation));
    }
    document.layout();

    for (limit, block, truncation) in clamped {
        assert_eq!(
            display(&document, truncation),
            Display::Flex,
            "clamp {limit}: the block's own inline-truncation child is shown"
        );
        assert_eq!(
            document.paragraph_of(truncation),
            Some(block),
            "clamp {limit}: and is a text scope of the block"
        );
        assert_eq!(
            ink(&document, block),
            (12.0 * 16.0, limit as f32 * 16.0),
            "clamp {limit}: exactly that many lines, the last of them ten kept \
             squares and the two-square content that replaced the two the \
             retreat freed"
        );
    }

    assert_eq!(
        display(&document, unclamped_truncation),
        Display::Flex,
        "the block that declared no clamp shows its truncation child as the \
         clamped ones do"
    );
    assert_eq!(
        document.paragraph_of(unclamped_truncation),
        Some(unclamped),
        "and holds the same text scope — what differs is whether the paragraph \
         lays its content in, which the two measures below report"
    );
    assert_eq!(
        ink(&document, unclamped),
        ink(&document, control),
        "so it measures what the same run measures with no truncation child \
         written at all"
    );
    assert_eq!(
        ink(&document, unclamped).1,
        9.0 * 16.0,
        "nine lines: a hundred squares at twelve to a line, none of them \
         dropped"
    );
}

#[test]
fn a_maxline_clamp_lays_its_first_inline_truncation_child_in_at_the_cut() {
    const MEASURE: &str = "width: 100px; font-size: 20px";
    const RUN: &str = "aa bbbbb ccccc ddddd eeeee";

    let mut document = ahem_document();

    let marked = child(&mut document, "text", MEASURE);
    literal(&mut document, marked, RUN);
    let marker = element_under(&mut document, marked, "inline-truncation", "");
    literal(&mut document, marker, "M");
    let ignored = element_under(&mut document, marked, "inline-truncation", "");
    literal(&mut document, ignored, "XXX");
    set_limit(&mut document, marked, "text-maxline", "2");

    let bare = child(&mut document, "text", MEASURE);
    literal(&mut document, bare, RUN);
    set_limit(&mut document, bare, "text-maxline", "2");

    let fitting = child(&mut document, "text", MEASURE);
    literal(&mut document, fitting, "aa bbbbb");
    let unused = element_under(&mut document, fitting, "inline-truncation", "");
    literal(&mut document, unused, "M");
    set_limit(&mut document, fitting, "text-maxline", "2");
    document.layout();

    assert_eq!(
        ink(&document, marked),
        (4.0 * 20.0, 2.0 * 20.0),
        "the clamp line gives up two squares and the marker takes one of them, \
         so the widest line is three kept squares plus the marker",
    );
    assert_eq!(
        ink(&document, bare),
        (5.0 * 20.0, 2.0 * 20.0),
        "the same clamp with no marker keeps the whole line",
    );
    assert_eq!(
        ink(&document, fitting),
        (5.0 * 20.0, 2.0 * 20.0),
        "and a paragraph that fits inside its clamp never lays its marker in",
    );
}

#[test]
fn a_leading_image_survives_the_retreat_that_lays_the_truncation_content_in() {
    let mut document = ahem_document();
    let column = child(&mut document, "view", "width: 300px");
    let text = element_under(
        &mut document,
        column,
        "text",
        "width: 300px; font-size: 24px; line-height: 32px; word-break: break-all",
    );
    let lead = image(&mut document, text, "width: 32px; height: 18px");
    literal(&mut document, text, &"a".repeat(60));
    let truncation = element_under(&mut document, text, "inline-truncation", "");
    let label = element_under(
        &mut document,
        truncation,
        "text",
        "font-size: 14px; line-height: 22px",
    );
    literal(&mut document, label, "ab");
    let icon = image(&mut document, truncation, "width: 12px; height: 12px");
    set_limit(&mut document, text, "text-maxline", "3");
    document.layout();

    assert_ne!(
        display(&document, truncation),
        Display::None,
        "three lines out of five: the block overflows, so its truncation \
         content is content"
    );
    assert_eq!(
        placement(&document, icon),
        Some((10.0 * 24.0 + 2.0 * 14.0, 12.0, 12.0)),
        "the icon follows the label at the end of the last visible line, and \
         both sit inside the 300px measure"
    );
    assert_eq!(
        placement(&document, lead),
        Some((0.0, 32.0, 18.0)),
        "the leading box is unit zero of the run, not a seed the cut search \
         mistakes for the absence of one: the retreat consumed the tail of \
         line 3 and left it where it was"
    );
    assert_eq!(
        ink(&document, text).1,
        3.0 * 32.0,
        "three clamped lines at the declared line height"
    );
}

#[test]
fn padding_narrows_the_measure_a_maxline_clamp_counts_lines_in() {
    let mut document = ahem_document();
    let text = child(
        &mut document,
        "text",
        "width: 300px; padding: 50px; font-size: 14px",
    );
    literal(
        &mut document,
        text,
        "aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj kkkk llll mmmm nnnn oooo",
    );
    set_limit(&mut document, text, "text-maxline", "4");
    document.layout();

    assert_eq!(
        ink(&document, text).0,
        14.0 * 14.0,
        "three four-square words and their two spaces: the measure is the 300px \
         border box less its 100px of padding, not 300"
    );
    assert_eq!(
        frame(&document, text),
        (0.0, 0.0, 300.0, 4.0 * 14.0 + 100.0),
        "four clamped lines, and the padding box survives around them"
    );
}

#[test]
#[ignore = "DEVIATION, not a missing feature: this engine follows native \
            Lynx, by the user's ruling of 2026-09-15. `text-maxline=\"1\"` is \
            a one-line clamp of the paragraph as it normally wraps (Android \
            `TextRenderer.shouldBeSingleLine()` plus a `StaticLayout` at the \
            available width, iOS `maximumNumberOfLines`), so neither \
            `white-space: nowrap` nor web-core's \
            `x-text[text-maxline=\"1\"] { max-width: -webkit-fill-available }` \
            (`x-text.css:216-241`) is replicated, and the block is as wide as \
            the line it wrapped into"]
fn a_single_line_clamp_caps_the_block_to_its_parent_s_available_width() {
    let mut document = ahem_document();
    let column = child(
        &mut document,
        "view",
        "width: 88px; display: flex; flex-direction: column; align-items: center",
    );
    let text = element_under(
        &mut document,
        column,
        "text",
        "font-size: 16px; color: rgb(22, 24, 35)",
    );
    literal(&mut document, text, "aaaaaaaaaa\n  ");
    set_limit(&mut document, text, "text-maxline", "1");
    document.layout();

    assert_eq!(
        frame(&document, text).2,
        88.0,
        "one unwrapped line capped at the 88 its parent offers, ellipsized \
         there rather than sized by whatever the block wrapped into"
    );
}

#[test]
fn a_maxlength_cut_inside_a_raw_text_payload_lands_bare_at_the_kept_five() {
    let mut document = ahem_document();
    let column = child(
        &mut document,
        "view",
        "display: flex; flex-direction: column",
    );
    let text = element_under(&mut document, column, "text", "font-size: 20px");
    let carrier = raw_text(&mut document, text, "12345678");
    set_limit(&mut document, text, "text-maxlength", "5");
    document.layout();

    assert_eq!(
        display(&document, carrier),
        Display::Contents,
        "the carrier contributes no character of its own to the index the cut \
         is counted in"
    );
    assert_eq!(
        ink(&document, text),
        (5.0 * 20.0, 20.0),
        "the five kept em squares at 20px and nothing after them: no marker is \
         emitted, and with no marker to make room for nothing is backed off \
         either, so all five characters the limit allows survive"
    );
}

#[test]
fn a_maxlength_cut_shows_the_tail_once_text_overflow_is_ellipsis() {
    let mut document = ahem_document();
    let column = child(
        &mut document,
        "view",
        "display: flex; flex-direction: column",
    );
    let text = element_under(
        &mut document,
        column,
        "text",
        "font-size: 20px; text-overflow: ellipsis",
    );
    raw_text(&mut document, text, "12345678");
    set_limit(&mut document, text, "text-maxlength", "5");
    document.layout();

    assert_eq!(
        ink(&document, text),
        (8.0 * 20.0, 20.0),
        "the five kept em squares plus the three dots the ellipsis appends, at \
         20px each — the maxlength candidate is never backed off, so the dots \
         are added to the five rather than taken out of them"
    );
}

#[test]
fn wrapping_every_string_in_a_raw_text_changes_neither_the_runs_nor_the_clamp() {
    const PARAGRAPH: &str = "width: 300px; font-size: 14px";
    const PINK: &str = "color: pink";
    const BIG: &str = "font-weight: 500; font-size: 3em";
    const TRACKED: &str = "font-weight: 500; letter-spacing: 7px";
    const LEAD: &str = "4hello world, this is a long enough text without any limitation.";
    const INLINE: &str = "we could use inline-text to set color of some text";
    const LARGER: &str = "also, font-size could be different";
    const SPACED: &str = "additionally, letter-space could be different";

    let mut document = ahem_document();

    let bare = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, bare, LEAD);
    let bare_pink = element_under(&mut document, bare, "text", PINK);
    literal(&mut document, bare_pink, INLINE);
    let bare_big = element_under(&mut document, bare, "text", BIG);
    literal(&mut document, bare_big, LARGER);
    let bare_tracked = element_under(&mut document, bare, "text", TRACKED);
    literal(&mut document, bare_tracked, SPACED);
    set_limit(&mut document, bare, "text-maxline", "4");

    let carried = child(&mut document, "text", PARAGRAPH);
    raw_text(&mut document, carried, LEAD);
    let carried_pink = element_under(&mut document, carried, "text", PINK);
    raw_text(&mut document, carried_pink, INLINE);
    let carried_big = element_under(&mut document, carried, "text", BIG);
    raw_text(&mut document, carried_big, LARGER);
    let carried_tracked = element_under(&mut document, carried, "text", TRACKED);
    raw_text(&mut document, carried_tracked, SPACED);
    set_limit(&mut document, carried, "text-maxline", "4");
    document.layout();

    assert_eq!(
        ink(&document, carried),
        ink(&document, bare),
        "a carrier establishes no box and no break opportunity, so the clamp \
         lands in the same place"
    );
    assert_eq!(frame(&document, carried).3, frame(&document, bare).3);
}

#[test]
fn a_text_block_as_a_flex_item_sizes_to_its_run_and_is_centred_not_stretched() {
    let mut document = ahem_document();
    let button = child(
        &mut document,
        "view",
        "width: 358px; height: 44px; display: flex; flex-direction: row; \
         justify-content: center; align-items: center",
    );
    let text = element_under(
        &mut document,
        button,
        "text",
        "font-size: 15px; line-height: 21px; font-weight: 500",
    );
    literal(&mut document, text, "abcdef ");
    document.layout();

    assert_eq!(
        ink(&document, text),
        (6.0 * 15.0, 21.0),
        "six em squares on one line; the trailing whitespace hangs and adds no \
         advance"
    );
    assert_eq!(
        frame(&document, text),
        ((358.0 - 90.0) / 2.0, 12.0, 6.0 * 15.0, 21.0),
        "the item takes its max-content width and the container centres it on \
         both axes: a text does not stretch"
    );
}

#[test]
fn a_one_line_clamp_keeps_the_nested_run_s_colour_and_the_parent_s_weight() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "font-size: 24px; font-weight: bold");
    raw_text(&mut document, text, "I am bold");
    let nested = element_under(&mut document, text, "text", "font-size: 24px; color: red");
    raw_text(&mut document, nested, "and red");
    raw_text(
        &mut document,
        text,
        "longlonglonglonglonglonglonglonglong text \
         longlonglonglonglonglonglonglonglong",
    );
    set_limit(&mut document, text, "text-maxline", "1");
    document.set_attribute(text, "ellipsize-mode", "tail");
    document.layout();

    assert_eq!(
        ink(&document, text).1,
        24.0,
        "the whole mixed-style paragraph is clamped to a single line box"
    );
    assert_eq!(
        style_of(&document, nested).get_color().solid_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0),
    );
    assert_eq!(
        *style_of(&document, nested).get_font_weight(),
        *style_of(&document, text).get_font_weight(),
    );
}

#[test]
#[ignore = "DEVIATION, not a missing feature: this engine follows native \
            Lynx, by the user's ruling of 2026-09-15. `text-maxline=\"1\"` is \
            a one-line clamp of the paragraph as it normally wraps (Android \
            `TextRenderer.shouldBeSingleLine()` plus a `StaticLayout` at the \
            available width, iOS `maximumNumberOfLines`), so web-core's \
            `x-text[text-maxline=\"1\"]` nowrap/`-webkit-fill-available` pair \
            (`x-text.css:216-241`) is not replicated and the kept line stops \
            at the last word boundary that fit"]
fn a_one_line_clamp_fills_the_available_width_instead_of_breaking_at_a_word() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "font-size: 24px; font-weight: bold");
    raw_text(&mut document, text, "I am bold");
    let nested = element_under(&mut document, text, "text", "font-size: 24px; color: red");
    raw_text(&mut document, nested, "and red");
    raw_text(
        &mut document,
        text,
        "longlonglonglonglonglonglonglonglong text \
         longlonglonglonglonglonglonglonglong",
    );
    set_limit(&mut document, text, "text-maxline", "1");
    document.layout();

    assert_eq!(
        ink(&document, text).0,
        384.0,
        "the clamped line is not broken at a word boundary: it runs out to the \
         16 whole units the page's 393 offers and is ellipsized there"
    );
}

#[test]
fn the_compiled_baseline_card_rows_its_blocks_and_shares_one_run_baseline() {
    let mut document = ahem_document();
    let page = child(&mut document, "view", "display: flex");

    let blocks = element_under(&mut document, page, "view", "display: flex");
    let small = element_under(&mut document, blocks, "text", "font-size: 20px");
    document.set_attribute(small, TEXT_ATTRIBUTE, "hello world");
    let large = element_under(&mut document, blocks, "text", "font-size: 30px");
    document.set_attribute(large, TEXT_ATTRIBUTE, "111");

    let inline = element_under(&mut document, page, "view", "display: flex");
    let paragraph = element_under(&mut document, inline, "text", "");
    let run_small = element_under(&mut document, paragraph, "text", "font-size: 20px");
    raw_text(&mut document, run_small, "hello world");
    let run_large = element_under(&mut document, paragraph, "text", "font-size: 30px");
    raw_text(&mut document, run_large, "111");
    document.layout();

    assert_eq!(
        (ink(&document, small), ink(&document, large)),
        ((5.0 * 20.0, 2.0 * 20.0), (30.0, 3.0 * 30.0)),
        "each sibling block measures the run its attribute carries, at the \
         width the row's shrink leaves it — `111` below its min-content"
    );
    assert_eq!(
        (frame(&document, small).0, frame(&document, large).0),
        (0.0, 139.0),
        "and the row places them side by side, not stacked: the container is a \
         flex row, and with `min-width: 0` both items shrink in proportion to \
         their bases"
    );
    assert_eq!(
        ink(&document, paragraph),
        (5.0 * 20.0 + 3.0 * 30.0, 20.0 + 30.0),
        "and on its last line the two inline runs hang from one baseline, so \
         that line is the larger run's em box rather than the sum of the two"
    );
}

#[test]
fn a_compiled_card_s_inline_image_takes_its_used_size_and_its_margin() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "font-size: 24px; color: blue");
    raw_text(&mut document, text, "abcd");
    let inline = image(
        &mut document,
        text,
        "width: 22px; height: 22px; margin-left: 10px",
    );
    document.layout();

    assert_eq!(
        ink(&document, text).0,
        4.0 * 24.0 + 10.0 + 22.0,
        "no collapsed space between the two children, and the margin is outer \
         advance ahead of the border box"
    );
    assert_eq!(
        horizontal_frame(&document, inline),
        (4.0 * 24.0 + 10.0, 22.0, 22.0),
        "the box itself keeps the used size its CSS asked for"
    );
}

#[test]
fn a_padded_view_nested_in_a_text_is_an_atom_the_surrounding_runs_flow_around() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "width: 1500px; font-size: 40px");
    raw_text(&mut document, text, "hello world");
    let atom = element_under(
        &mut document,
        text,
        "view",
        "padding: 10px; border: 1px solid red; border-radius: 20px",
    );
    let inner = element_under(
        &mut document,
        atom,
        "text",
        "font-size: 30px; color: linear-gradient(green, yellow)",
    );
    document.set_attribute(inner, TEXT_ATTRIBUTE, "sub text");
    raw_text(&mut document, text, "other text content");
    document.layout();

    assert!(
        matches!(
            style_of(&document, inner).get_color(),
            ColorPropertyValue::Gradient(_)
        ),
        "a nested text inside the atom is its own text block, and a Lynx colour \
         holds the gradient it was given"
    );
    assert_eq!(
        ink(&document, inner),
        (8.0 * 30.0, 30.0),
        "the inner paragraph measures its own run"
    );
    assert_eq!(
        horizontal_frame(&document, atom),
        (11.0 * 40.0, 8.0 * 30.0 + 22.0, 30.0 + 22.0),
        "the atom's border box is that content plus 10px of padding and 1px of \
         border on each side, and it starts where the first run ended"
    );
    assert_eq!(
        ink(&document, text).0,
        11.0 * 40.0 + (8.0 * 30.0 + 22.0) + 18.0 * 40.0,
        "run, atom, run — in source order on one line"
    );
}

#[test]
fn a_literal_newline_in_a_dynamic_string_child_breaks_the_line() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "font-size: 20px");
    raw_text(&mut document, text, "hello\nworld!");
    document.layout();

    assert_eq!(
        ink(&document, text),
        (6.0 * 20.0, 2.0 * 20.0),
        "two lines, the wider of them the six squares of 'world!'"
    );
}

#[test]
fn a_compiled_maxline_card_clamps_one_two_and_unlimited_lines() {
    const PARA: &str = "The layout of the text component is different from that of the view \
                        component. It does not support setting display and related properties \
                        for layout, and has its own text layout method internally. Currently, \
                        native layout and rendering are used.";

    let mut document = ahem_document();
    let column = child(
        &mut document,
        "view",
        "display: flex; flex-direction: column",
    );
    let blocks: Vec<NodeId> = ["1", "2", ""]
        .iter()
        .map(|limit| {
            let block = element_under(&mut document, column, "text", "font-size: 16px");
            document.set_attribute(block, TEXT_ATTRIBUTE, PARA);
            if !limit.is_empty() {
                set_limit(&mut document, block, "text-maxline", limit);
            }
            block
        })
        .collect();
    document.layout();

    let heights: Vec<f32> = blocks
        .iter()
        .map(|block| ink(&document, *block).1)
        .collect();
    assert_eq!(
        (heights[0], heights[1]),
        (16.0, 32.0),
        "one and two line boxes, the clamp counting lines out of the many the \
         paragraph would otherwise wrap into"
    );
    assert!(
        heights[2] > heights[1],
        "and the unclamped block keeps every line it wraps into"
    );
}

#[test]
fn display_none_removes_a_text_block_and_an_inline_run_alike() {
    let mut document = ahem_document();

    let blocks = child(&mut document, "view", "");
    let small = element_under(
        &mut document,
        blocks,
        "text",
        "font-size: 20px; display: none",
    );
    raw_text(&mut document, small, "hello world");
    let large = element_under(
        &mut document,
        blocks,
        "text",
        "font-size: 30px; display: none",
    );
    raw_text(&mut document, large, "111");

    let inline = child(&mut document, "view", "");
    let paragraph = element_under(&mut document, inline, "text", "font-size: 20px");
    let run_small = element_under(
        &mut document,
        paragraph,
        "text",
        "font-size: 20px; display: none",
    );
    raw_text(&mut document, run_small, "hello world");
    let run_large = element_under(
        &mut document,
        paragraph,
        "text",
        "font-size: 30px; display: none",
    );
    raw_text(&mut document, run_large, "111");
    document.layout();

    for hidden in [small, large, run_small, run_large] {
        assert_eq!(display(&document, hidden), Display::None);
    }
    assert_eq!(
        frame(&document, blocks).3,
        0.0,
        "two hidden block texts generate no box, so their container is empty"
    );
    assert_eq!(
        ink(&document, paragraph),
        (0.0, 0.0),
        "and two hidden inline runs are skipped by the flatten walk, so the \
         paragraph has no content at all"
    );

    document.set_inline_style(run_small, "font-size: 20px");
    document.set_inline_style(run_large, "font-size: 30px");
    document.layout();
    assert_eq!(
        ink(&document, paragraph),
        (11.0 * 20.0 + 3.0 * 30.0, 30.0),
        "lifting the declaration puts both runs back on one line, which is what \
         makes their absence a removal rather than a zero-sized box"
    );
}

#[test]
fn the_later_of_two_equal_specificity_class_rules_wins_on_a_text() {
    let mut document = ahem_document();
    document.add_stylesheet(
        ".a { color: red; font-size: 16px }\n\
         .b { color: linear-gradient(green, #ff0); font-size: 80px }",
        terminus::link::StylesheetOrigin::Author,
    );
    let text = child(&mut document, "text", "width: 900px");
    document.add_class(text, "a");
    document.add_class(text, "b");
    raw_text(&mut document, text, "hello lynx");
    document.layout();

    let style = style_of(&document, text);
    assert!(
        matches!(style.get_color(), ColorPropertyValue::Gradient(_)),
        "source order decides the cascade, so `.b`'s gradient wins over `.a`'s red"
    );
    assert_eq!(
        style.get_font().slow_clone_font_size().used_size().px(),
        80.0
    );
    assert_eq!(
        ink(&document, text),
        (10.0 * 80.0, 80.0),
        "and the run is shaped at that size: ten em squares on one line, not \
         the 16px the earlier rule asked for"
    );
}

#[test]
fn a_text_child_does_not_inherit_its_view_parent_s_colour() {
    let mut document = ahem_document();
    let view = child(&mut document, "view", "color: red");
    let text = element_under(&mut document, view, "text", "font-size: 20px");
    raw_text(&mut document, text, "123456");
    document.layout();

    assert_eq!(
        ink(&document, text),
        (6.0 * 20.0, 20.0),
        "the paragraph the reset applies to is not an empty one"
    );
    assert_eq!(
        style_of(&document, text).get_color().solid_color(),
        AbsoluteColor::BLACK,
        "the reset stops the cascade at the text root"
    );
    assert_ne!(
        style_of(&document, view).get_color().solid_color(),
        AbsoluteColor::BLACK,
        "and the ancestor really did declare a colour to inherit"
    );
}

const INHERITANCE_CARD_CSS: &str = "
.typography {
  color: red;
  direction: rtl;
  font-family: monospace;
  font-size: 24px;
  font-style: italic;
  font-weight: 700;
  letter-spacing: 2px;
  line-height: 32px;
  text-align: right;
  text-decoration: underline;
  text-shadow: 1px 2px 3px blue;
}

.gradient {
  color: linear-gradient(red, blue);
}

.override {
  color: green;
  font-size: 18px;
  text-decoration: none;
}

.updated {
  color: linear-gradient(green, blue);
  font-size: 30px;
}
";

const RED_TO_BLUE: &str = "linear-gradient(rgb(255, 0, 0), rgb(0, 0, 255))";
const GREEN_TO_BLUE: &str = "linear-gradient(rgb(0, 128, 0), rgb(0, 0, 255))";

const INHERITANCE_CONFIGS: [(&str, Option<bool>, bool); 3] = [
    ("true", Some(true), true),
    ("false", Some(false), false),
    ("default", None, false),
];

struct InheritanceCard {
    document: LynxDocument,
    solid_parent: NodeId,
    solid_text: NodeId,
    wrapped_text: NodeId,
    nested_text: NodeId,
    gradient_text: NodeId,
    wrapped_gradient: NodeId,
    override_text: NodeId,
    own_gradient: NodeId,
}

fn card_text(document: &mut LynxDocument, parent: NodeId, class: &str, content: &str) -> NodeId {
    let text = element_under(document, parent, "text", "");
    if !class.is_empty() {
        document.add_class(text, class);
    }
    raw_text(document, text, content);
    text
}

fn card_view(document: &mut LynxDocument, parent: NodeId, class: &str) -> NodeId {
    let view = element_under(document, parent, "view", "");
    if !class.is_empty() {
        document.add_class(view, class);
    }
    view
}

fn inheritance_card(switch: Option<bool>) -> InheritanceCard {
    let mut config = PageConfig::default();
    if let Some(enabled) = switch {
        config.enable_css_inheritance = enabled;
    }
    let mut document = ahem_document_with(config);
    document.add_stylesheet(
        INHERITANCE_CARD_CSS,
        terminus::link::StylesheetOrigin::Author,
    );
    let root = child(&mut document, "view", "");

    let solid_parent = card_view(&mut document, root, "typography");
    let solid_text = card_text(&mut document, solid_parent, "", "Inherited text");
    let wrapper = element_under(&mut document, solid_parent, "wrapper", "");
    let wrapped_text = card_text(&mut document, wrapper, "", "Wrapped text");
    let nested_parent = element_under(&mut document, solid_parent, "text", "");
    let nested_text = card_text(&mut document, nested_parent, "", "Nested text");

    let gradient_parent = card_view(&mut document, root, "gradient");
    let gradient_text = card_text(&mut document, gradient_parent, "", "Gradient text");
    let wrapper = element_under(&mut document, gradient_parent, "wrapper", "");
    let wrapped_gradient = card_text(&mut document, wrapper, "", "Wrapped gradient");
    let override_text = card_text(
        &mut document,
        gradient_parent,
        "override",
        "Explicit solid color",
    );

    let own_parent = card_view(&mut document, root, "typography");
    let own_gradient = card_text(
        &mut document,
        own_parent,
        "gradient",
        "Explicit gradient color",
    );

    document.layout();
    InheritanceCard {
        document,
        solid_parent,
        solid_text,
        wrapped_text,
        nested_text,
        gradient_text,
        wrapped_gradient,
        override_text,
        own_gradient,
    }
}

#[test]
fn css_inheritance_gives_the_text_inside_a_view_the_view_s_typography() {
    const INHERITED_EITHER_WAY: [(&str, &str); 9] = [
        ("direction", "rtl"),
        ("font-family", "monospace"),
        ("font-size", "24px"),
        ("font-style", "italic"),
        ("font-weight", "700"),
        ("letter-spacing", "2px"),
        ("line-height", "32px"),
        ("text-align", "right"),
        ("text-shadow", "rgb(0, 0, 255) 1px 2px 3px"),
    ];

    for (setting, switch, enabled) in INHERITANCE_CONFIGS {
        let card = inheritance_card(switch);
        let read = |property| computed(&card.document, card.solid_text, property);

        assert_eq!(
            read("color"),
            if enabled {
                "rgb(255, 0, 0)"
            } else {
                "rgb(0, 0, 0)"
            },
            "{setting}"
        );
        assert_eq!(
            read("text-decoration-line"),
            if enabled { "underline" } else { "none" },
            "{setting}"
        );
        for (property, value) in INHERITED_EITHER_WAY {
            assert_eq!(read(property), value, "{property}: {setting}");
        }
    }
}

#[test]
fn css_inheritance_reaches_a_text_through_a_wrapper() {
    for (setting, switch, enabled) in INHERITANCE_CONFIGS {
        let card = inheritance_card(switch);
        let read = |element, property| computed(&card.document, element, property);

        assert_eq!(
            read(card.wrapped_text, "color"),
            if enabled {
                "rgb(255, 0, 0)"
            } else {
                "rgb(0, 0, 0)"
            },
            "{setting}"
        );
        assert_eq!(
            read(card.wrapped_text, "text-decoration-line"),
            if enabled { "underline" } else { "none" },
            "{setting}"
        );
        assert_eq!(
            read(card.wrapped_gradient, "color"),
            if enabled { RED_TO_BLUE } else { "rgb(0, 0, 0)" },
            "{setting}"
        );
    }
}

#[test]
fn css_inheritance_reaches_a_nested_text_through_its_parent_text() {
    for (setting, switch, enabled) in INHERITANCE_CONFIGS {
        let card = inheritance_card(switch);

        assert_eq!(
            computed(&card.document, card.nested_text, "color"),
            if enabled {
                "rgb(255, 0, 0)"
            } else {
                "rgb(0, 0, 0)"
            },
            "{setting}"
        );
    }
}

#[test]
fn css_inheritance_inherits_a_view_s_gradient_colour() {
    for (setting, switch, enabled) in INHERITANCE_CONFIGS {
        let card = inheritance_card(switch);

        assert_eq!(
            computed(&card.document, card.gradient_text, "color"),
            if enabled { RED_TO_BLUE } else { "rgb(0, 0, 0)" },
            "{setting}"
        );
        assert_eq!(
            matches!(
                style_of(&card.document, card.gradient_text).get_color(),
                ColorPropertyValue::Gradient(_)
            ),
            enabled,
            "the value the glyph painter reads is the gradient itself: {setting}"
        );
    }
}

#[test]
fn an_authored_text_style_outranks_css_inheritance() {
    for (setting, switch, _) in INHERITANCE_CONFIGS {
        let card = inheritance_card(switch);
        let read = |property| computed(&card.document, card.override_text, property);

        assert_eq!(read("color"), "rgb(0, 128, 0)", "{setting}");
        assert_eq!(read("font-size"), "18px", "{setting}");
        assert_eq!(read("text-decoration-line"), "none", "{setting}");
    }
}

#[test]
fn a_text_s_own_gradient_outranks_css_inheritance() {
    for (setting, switch, _) in INHERITANCE_CONFIGS {
        let card = inheritance_card(switch);

        assert_eq!(
            computed(&card.document, card.own_gradient, "color"),
            RED_TO_BLUE,
            "{setting}"
        );
    }
}

#[test]
fn css_inheritance_follows_a_change_of_the_view_s_style() {
    for (setting, switch, enabled) in INHERITANCE_CONFIGS {
        let mut card = inheritance_card(switch);
        assert_eq!(
            computed(&card.document, card.solid_text, "font-size"),
            "24px",
            "{setting}"
        );

        card.document.add_class(card.solid_parent, "updated");
        card.document.layout();

        let read = |element, property| computed(&card.document, element, property);
        assert_eq!(read(card.solid_parent, "color"), GREEN_TO_BLUE, "{setting}");
        assert_eq!(
            read(card.solid_text, "color"),
            if enabled {
                GREEN_TO_BLUE
            } else {
                "rgb(0, 0, 0)"
            },
            "{setting}"
        );
        assert_eq!(
            read(card.wrapped_text, "color"),
            if enabled {
                GREEN_TO_BLUE
            } else {
                "rgb(0, 0, 0)"
            },
            "the change reaches the text inside the wrapper as the first value did: {setting}"
        );
        assert_eq!(
            read(card.solid_text, "font-size"),
            "30px",
            "an inherited property follows the view under every configuration: {setting}"
        );
    }
}

#[test]
fn css_inheritance_follows_a_change_of_the_view_s_text_decoration() {
    let mut document = ahem_document_with(PageConfig {
        enable_css_inheritance: true,
        ..PageConfig::default()
    });
    document.add_stylesheet(
        ".struck { text-decoration: line-through; }",
        terminus::link::StylesheetOrigin::Author,
    );
    let view = child(&mut document, "view", "");
    let direct = card_text(&mut document, view, "", "direct");
    let wrapper = element_under(&mut document, view, "wrapper", "");
    let wrapped = card_text(&mut document, wrapper, "", "wrapped");
    document.layout();
    for text in [direct, wrapped] {
        assert_eq!(computed(&document, text, "text-decoration-line"), "none");
    }

    document.add_class(view, "struck");
    document.layout();
    for text in [direct, wrapped] {
        assert_eq!(
            computed(&document, text, "text-decoration-line"),
            "line-through"
        );
    }

    document.remove_class(view, "struck");
    document.layout();
    for text in [direct, wrapped] {
        assert_eq!(computed(&document, text, "text-decoration-line"), "none");
    }
}

#[test]
fn css_inheritance_is_scoped_to_a_view_s_own_text_and_wrapper() {
    let mut document = ahem_document_with(PageConfig {
        enable_css_inheritance: true,
        ..PageConfig::default()
    });
    document.add_stylesheet(
        "page, .coloured { color: rgb(1, 2, 3); }",
        terminus::link::StylesheetOrigin::Author,
    );
    let page = document.document_element().id();
    let mut texts = vec![("page", card_text(&mut document, page, "", "page"))];
    for tag in ["scroll-view", "list-item", "blur-view", "x-blur-view"] {
        let container = element_under(&mut document, page, tag, "");
        document.add_class(container, "coloured");
        texts.push((tag, card_text(&mut document, container, "", tag)));
    }
    let view = card_view(&mut document, page, "coloured");
    let outer = element_under(&mut document, view, "wrapper", "");
    let inner = element_under(&mut document, outer, "wrapper", "");
    texts.push((
        "two wrappers",
        card_text(&mut document, inner, "", "wrapped twice"),
    ));
    let inside = card_text(&mut document, view, "", "inside the view");
    document.layout();

    assert_eq!(computed(&document, inside, "color"), "rgb(1, 2, 3)");
    for (parent, text) in texts {
        assert_eq!(
            computed(&document, text, "color"),
            "rgb(0, 0, 0)",
            "a text under {parent} keeps the reset"
        );
    }
}

#[test]
fn every_authored_declaration_outranks_css_inheritance() {
    let mut document = ahem_document_with(PageConfig {
        enable_css_inheritance: true,
        ..PageConfig::default()
    });
    document.add_stylesheet(
        "text { color: rgb(4, 5, 6); text-decoration: line-through; }",
        terminus::link::StylesheetOrigin::Author,
    );
    let view = child(
        &mut document,
        "view",
        "color: rgb(1, 2, 3); text-decoration: underline",
    );
    let by_type = card_text(&mut document, view, "", "type selector");
    let inline = element_under(&mut document, view, "text", "color: rgb(7, 8, 9)");
    raw_text(&mut document, inline, "inline style");
    document.layout();

    assert_eq!(computed(&document, by_type, "color"), "rgb(4, 5, 6)");
    assert_eq!(
        computed(&document, by_type, "text-decoration-line"),
        "line-through"
    );
    assert_eq!(computed(&document, inline, "color"), "rgb(7, 8, 9)");
}
