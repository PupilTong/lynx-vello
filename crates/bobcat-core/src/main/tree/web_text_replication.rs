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
//! and paint, which belongs to `crates/dom`'s pixel tests.
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
//! them: the markup indents its content, and a browser removes collapsible
//! whitespace at the start of a line while this engine emits it as a space
//! (`crates/hughie/src/text/block/content.rs:352-383` resolves an open
//! whitespace span against the following character, with no start-of-line
//! suppression). Wherever a case's own claim survives that divergence the
//! replica keeps the fixture's indentation and pins the claim against a
//! control tree, so a paragraph's absolute advance is not what the case turns
//! on. The exceptions are the cases whose claim *is* an absolute advance — the
//! atomic-inline replicas, where the line's width is the atom's own measured
//! width. Those drop the fixture's surrounding whitespace instead, and each
//! such test's own doc comment says so.

// Ahem and explicit line heights have exact metrics, so the geometry these
// replicas assert is compared exactly; glyph counts convert to advances.
#![allow(clippy::float_cmp, clippy::cast_precision_loss, clippy::similar_names)]

use dom::NodeId;
use dom::stylo::color::AbsoluteColor;
use dom::stylo::values::computed::{ColorPropertyValue, Display};

use super::LynxDocument;
use super::test_support::{child, display, document, element_under, style_of};

/// Solid em squares, so a run's advance is its glyph count times its font size.
const AHEM: &[u8] = include_bytes!("../../../../hughie/tests/fixtures/Ahem.ttf");

/// The attribute a `raw-text` carries its run in, and the one a compiled
/// `ReactLynx` card writes a static string child into.
const TEXT_ATTRIBUTE: &str = "text";

/// The picture every `x-text` fixture draws, at its fixture-relative path.
const IMAGE_SRC: &str = "/tests/fixtures/resources/inline-image.png";

/// A phone-shaped document shaping every generic family with Ahem, so a
/// fixture's unstyled `font-family` measures as exactly as a named one.
fn ahem_document() -> LynxDocument {
    let mut document = document();
    assert_eq!(document.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
    assert!(document.set_default_font_family("Ahem"));
    document
}

/// A literal text node, the way a fixture's markup writes one between tags.
fn literal(document: &mut LynxDocument, parent: NodeId, content: &str) {
    let node = document.create_text_node(content, ());
    document.append_child(parent, node);
}

/// What `__CreateRawText` compiles to: a carrier whose `text` attribute is the
/// run, attached where the string stood.
fn raw_text(document: &mut LynxDocument, parent: NodeId, content: &str) -> NodeId {
    let carrier = element_under(document, parent, "raw-text", "");
    document.set_attribute(carrier, TEXT_ATTRIBUTE, content);
    carrier
}

/// An `image` carrying the fixture's source, so the box is a replaced one.
fn image(document: &mut LynxDocument, parent: NodeId, style: &str) -> NodeId {
    let element = element_under(document, parent, "image", style);
    document.set_attribute(element, "src", IMAGE_SRC);
    element
}

/// Writes a paragraph-limit attribute the way the runtime does: the attribute
/// itself, then the presentational hint it reflects into.
fn set_limit(document: &mut LynxDocument, element: NodeId, name: &str, value: &str) {
    document.set_attribute(element, name, value);
    super::apply_attribute_style(document, element, name, Some(value));
}

/// The measured size of the paragraph `id` establishes — the block's ink, not
/// its box.
fn ink(document: &LynxDocument, id: NodeId) -> (f32, f32) {
    let size = document.text_block_size(id).expect("a laid-out paragraph");
    (size.width, size.height)
}

/// `id`'s border box as `(x, y, width, height)`, in its parent's coordinates.
fn frame(document: &LynxDocument, id: NodeId) -> (f32, f32, f32, f32) {
    let layout = document.rounded_layout(id).expect("a laid-out box");
    (
        layout.location.x,
        layout.location.y,
        layout.size.width,
        layout.size.height,
    )
}

/// `id`'s border box without its vertical placement, which for an atomic inline
/// box follows the recorded line-box deviation rather than the CSS
/// ascent/descent split (`docs/tracking/deviations.md:213-220`).
fn horizontal_frame(document: &LynxDocument, id: NodeId) -> (f32, f32, f32) {
    let (x, _, width, height) = frame(document, id);
    (x, width, height)
}

/// The same shape as [`horizontal_frame`], for a box a case does not yet know
/// exists: `None` where nothing was laid out. A test whose claim is that a box
/// appears at all states it as an equality against `Some(..)`, so it fails on
/// its own assertion instead of unwinding inside a helper.
fn placement(document: &LynxDocument, id: NodeId) -> Option<(f32, f32, f32)> {
    let layout = document.rounded_layout(id)?;
    Some((layout.location.x, layout.size.width, layout.size.height))
}

/// Replicates `x-text/text-attribute-text`
/// (`web-elements/tests/fixtures/x-text/text-attribute-text.html`,
/// `web-elements/tests/web-elements.spec.ts:135`): a `text` attribute written
/// on a `text` element is that paragraph's inline content, and the element's
/// whitespace-only child contributes nothing after collapsing.
///
/// This is the dominant `ReactLynx` path, not a corner: the compiler collapses a
/// static string child into `__SetAttribute(textEl, "text", …)`, so most text
/// cards reach the engine this way and render blank until it is implemented.
/// Held only from `d19cbea2` (#227) on: before it only `raw-text` reflected a
/// `text` attribute into a run, so a `text` element's own attribute was inert
/// and the block measured zero. #227 renders `text[text] { content: attr(text) }`
/// from the element's primary computed style.
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

/// Replicates `x-text/view-in-text`
/// (`web-elements/tests/fixtures/x-text/view-in-text.html`,
/// `web-elements/tests/web-elements.spec.ts:140`): a block-level `view` child
/// of a `text` is reified as one atomic inline box that keeps its own specified
/// size and runs its own container algorithm over its children, independently
/// of line breaking.
///
/// Geometry only — the fixture's backgrounds and borders are paint, and
/// `crates/dom`'s pixel tests own them. The fixture selects the view's vertical
/// stacking with `--lynx-display: linear` and no toggle; here the `text > view`
/// UA rule computes the child to `flex`, so the replica writes the column
/// direction the toggle was selecting.
///
/// Two things the fixture carries are deliberately left out. Its markup indents
/// the `x-view`, and that surrounding whitespace is dropped here so the atom's
/// advance is the atom's own and not the module-level leading-space divergence.
/// And the line's own height is not asserted: a baseline-aligned box reserving
/// no descent below it is a recorded deviation
/// (`docs/tracking/deviations.md:213-220`), so the browser's line box is taller
/// than the atom and this case is not the one that should pin the difference.
/// Held only from `d19cbea2` (#227) on: before it the paragraph ran every
/// atom with `LayoutInput::measure`, which writes no box, and copied that
/// unwritten zero size into the placed layout. #227 needed committed geometry
/// for atomic children restored after a content replacement and gave the
/// paragraph a commit-goal path through `compute_inline_box_layout`
/// (`crates/dom/src/layout/text_block.rs:436-444`).
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

/// Replicates `x-text/nested-view-in-text`
/// (`web-elements/tests/fixtures/x-text/nested-view-in-text.html`,
/// `web-elements/tests/web-elements.spec.ts:144`): container nesting inside the
/// atomic inline box a `view` child forms — the inner container sizes itself
/// from its own children, and none of that re-enters the parent's line
/// breaking, because the atom is measured once at max-content.
///
/// As in `a_view_child_of_a_text_is_one_atomic_inline_box_laid_out_on_its_own`,
/// the fixture's indentation around the `x-view` is dropped so the atom's
/// advance carries no leading-space divergence, and the line's own height is
/// left out because a baseline-aligned box reserving no descent is a recorded
/// deviation (`docs/tracking/deviations.md:213-220`).
/// Held only from `d19cbea2` (#227) on: before it the paragraph ran every
/// atom with `LayoutInput::measure`, which writes no box, and copied that
/// unwritten zero size into the placed layout. #227 needed committed geometry
/// for atomic children restored after a content replacement and gave the
/// paragraph a commit-goal path through `compute_inline_box_layout`
/// (`crates/dom/src/layout/text_block.rs:436-444`).
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

/// Replicates `x-text/inline-text`
/// (`web-elements/tests/fixtures/x-text/inline-text.html`,
/// `web-elements/tests/web-elements.spec.ts:148`): a `text` nested directly
/// inside a `text` is an inline run in the parent's paragraph, not a nested
/// block — it inherits inherited properties, overrides its own colour, shares
/// the line box, and the whitespace between the parent's literal text and the
/// child collapses to exactly one space.
///
/// The colour half of the screenshot diff is a per-run paint claim that
/// `crates/dom`'s pixel tests own; here it is asserted as computed style. The
/// collapse claim is pinned against a control written without that whitespace,
/// which is one square narrower — an absolute advance would also carry the
/// module-level leading-indentation divergence, which this case is not about.
#[test]
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

    assert_eq!(display(&document, nested), Display::LynxText);
    assert_eq!(
        ink(&document, text),
        (ink(&document, joined).0 + 24.0, 24.0),
        "one line, one square wider than the same content written without that \
         whitespace: the newline and its indentation collapsed to a single space"
    );
    assert_eq!(
        style_of(&document, nested).clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0),
        "the nested run overrides the colour it is handed"
    );
    assert_eq!(
        style_of(&document, nested).clone_font_weight(),
        style_of(&document, text).clone_font_weight(),
        "and still inherits the weight the paragraph was given"
    );
}

/// Replicates `x-text/inline-text-with-lynx-wrapper`
/// (`web-elements/tests/fixtures/x-text/inline-text-with-lynx-wrapper.html`,
/// `web-elements/tests/web-elements.spec.ts:153`): a transparent wrapper
/// between a `text` and its nested inline `text` creates neither a box nor a
/// break in the inline formatting context.
///
/// The fixture is a lynx-stack authoring bug — despite its name it contains no
/// `lynx-wrapper` and is byte-identical to `inline-text.html` — so the replica
/// asserts the case the name intends, against the unwrapped tree as its
/// control.
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
    assert_eq!(display(&document, through), Display::LynxText);
    assert_eq!(
        ink(&document, wrapped),
        ink(&document, plain),
        "the wrapper dissolves: same runs, same line, same advance"
    );
    assert_eq!(
        style_of(&document, through).clone_color(),
        style_of(&document, direct).clone_color(),
    );
}

/// Replicates `x-text/text-in-text`
/// (`web-elements/tests/fixtures/x-text/text-in-text.html`,
/// `web-elements/tests/web-elements.spec.ts:159`): one paragraph, two
/// differently-coloured runs sharing a line box, the parent's weight carrying
/// into the child.
///
/// The fixture's body is the markup `inline-text.html` also carries; it stays
/// its own replica because it is its own golden in lynx-stack. It is pinned
/// from the other direction: splitting content into a nested run is
/// geometrically invisible, so the paragraph measures what the same characters
/// measure as one undivided run.
#[test]
fn splitting_a_paragraph_into_two_runs_leaves_its_line_geometry_untouched() {
    const PARAGRAPH: &str = "width: 600px; font-size: 24px; font-weight: bold";

    let mut document = ahem_document();
    let split = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, split, "\n    I am bold\n    ");
    let nested = element_under(&mut document, split, "text", "font-size: 24px; color: red");
    literal(&mut document, nested, "and red");

    let undivided = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, undivided, "\n    I am bold and red");
    document.layout();

    assert_eq!(
        ink(&document, split),
        ink(&document, undivided),
        "the nested run shares the parent's line box instead of opening one"
    );
    assert_eq!(
        style_of(&document, nested).clone_font_weight(),
        style_of(&document, split).clone_font_weight(),
    );
    assert_ne!(
        style_of(&document, nested).clone_color(),
        style_of(&document, split).clone_color(),
    );
}

/// Replicates `x-text/text-in-text-fault-torrent`
/// (`web-elements/tests/fixtures/x-text/text-in-text-fault-torrent.html`,
/// `web-elements/tests/web-elements.spec.ts:163`): a `text-maxline` declared on
/// an *inline* nested `text` is malformed input, and must not clip, hide or
/// re-box that run — clamping is a property of the block that establishes the
/// paragraph.
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

/// Replicates `x-text/text-baseline-alignment`
/// (`web-elements/tests/fixtures/x-text/text-baseline-alignment.html`,
/// `web-elements/tests/web-elements.spec.ts:167`): four arrangements at once —
/// two sibling `text` blocks stack in a linear container, are placed by the
/// container's own rules in a flex row, and two runs of different font-size
/// inside one paragraph sit on a common baseline.
///
/// The shared baseline is asserted through the line box it produces: two runs
/// hanging from one baseline give the larger run's ascent plus the larger run's
/// descent, which for Ahem is exactly the larger font size — where two stacked
/// baselines would cost the sum of the two.
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
    literal(&mut document, paragraph, "\n      ");
    let run_small = element_under(&mut document, paragraph, "text", "font-size: 20px");
    literal(&mut document, run_small, "hello world");
    literal(&mut document, paragraph, "\n      ");
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

/// Replicates `x-text/inline-image`
/// (`web-elements/tests/fixtures/x-text/inline-image.html`,
/// `web-elements/tests/web-elements.spec.ts:171`): an `image` child of a `text`
/// is an atomic inline-level replaced box that takes its specified used size —
/// 22x22, whatever the 162x162 file's intrinsic size is — and advances the line
/// by that width after the collapsed space.
///
/// Geometry only; whether the bitmap is drawn is paint, and `crates/dom` owns
/// it. The fixture's four-CJK run becomes four Ahem em squares, which carry the
/// same one-glyph-per-em advance and are deterministic here. The line's own
/// height is left out: a baseline-aligned box reserving no descent is a
/// recorded deviation (`docs/tracking/deviations.md:213-220`), not this case.
/// Held only from `d19cbea2` (#227) on: before it the paragraph ran every
/// atom with `LayoutInput::measure`, which writes no box, and copied that
/// unwritten zero size into the placed layout. #227 needed committed geometry
/// for atomic children restored after a content replacement and gave the
/// paragraph a commit-goal path through `compute_inline_box_layout`
/// (`crates/dom/src/layout/text_block.rs:436-444`).
#[test]
fn an_image_child_of_a_text_advances_the_line_by_its_used_size() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, text, "abcd\n    ");
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

/// Replicates `x-text/inline-image-with-lynx-wrapper`
/// (`web-elements/tests/fixtures/x-text/inline-image-with-lynx-wrapper.html`,
/// `web-elements/tests/web-elements.spec.ts:183`): an inline replaced `image`
/// reached through a transparent wrapper still forms one atomic inline box, in
/// the same place the unwrapped one occupies.
///
/// The fixture is a lynx-stack authoring bug — byte-identical to
/// `inline-image.html`, with no wrapper in it — so the replica asserts the case
/// the name intends, against the unwrapped tree as its control. Worth recording
/// that the paragraph half already holds for a different reason than in
/// web-core: there is no `text > wrapper > image` UA rule, and the wrapped image
/// simply escapes `text > * { display: none }` because the combinator stops
/// matching.
#[test]
#[ignore = "GAP: a wrapper leaves the atom below it unrounded — a \
            `display: contents` element's slot is never written, so \
            crates/hughie/src/compute/mod.rs:780-784 stops the rounding walk \
            there and the wrapped atom keeps a zero rounded box while its \
            unwrapped control gets its placement (120, 22, 22)"]
fn a_wrapper_between_a_text_and_an_inline_image_changes_nothing() {
    let mut document = ahem_document();
    let plain = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, plain, "abcd\n    ");
    let direct = image(&mut document, plain, "width: 22px; height: 22px");

    let wrapped = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, wrapped, "abcd\n    ");
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

/// Replicates `x-text/inline-image-padding-and-margin`
/// (`web-elements/tests/fixtures/x-text/inline-image-padding-and-margin.html`,
/// `web-elements/tests/web-elements.spec.ts:189`): what an atomic inline
/// replaced box contributes to the line's advance — `margin-left` adds 50px of
/// outer advance before the box, and `padding-left` adds nothing at all.
///
/// The asymmetry is web-core's, not generic CSS's. `x-text > x-image` is
/// `display: contents !important` (`x-text.css:69-82`), so the authored element
/// generates no box; the box on the line is the shadow `::part(img)`, and
/// `x-text.css:120-135` forwards `width`, `height`, `border`, `border-radius`,
/// `background-color`, `vertical-align`, `object-fit`, `flex`, `align-self` and
/// `margin` into it — `padding` is not on that list. The only `padding:
/// inherit` anywhere in web-elements is `x-image[auto-size]::part(img)`
/// (`XImage/x-image.css:56-61`), and this fixture sets no `auto-size`. So the
/// golden's two paragraphs advance by the same 22px box and differ only by the
/// margin's 50px.
///
/// Geometry only: the bitmap and where it sits inside its box are paint.
#[test]
#[ignore = "GAP (two of them). An atom's margin never reaches the line: \
            crates/dom/src/layout/text_block.rs:350-366 hands the block \
            `output.size`, which is the atom's border box, so the margin box \
            the line should advance by is lost. And an atom's padding inflates \
            it: this engine gives the authored `image` a real box, so \
            `padding-left: 50px` grows the border-box width from 22 to 50 under \
            the Lynx `box-sizing: border-box` default, where web-core drops the \
            padding entirely because the host box is `display: contents`"]
fn an_inline_image_s_margin_reaches_the_line_s_advance_and_its_padding_does_not() {
    let mut document = ahem_document();
    let margined = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, margined, "abcd\n    ");
    let outer = image(
        &mut document,
        margined,
        "width: 22px; height: 22px; margin-left: 50px",
    );

    let padded = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, padded, "abcd\n    ");
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

/// Replicates `x-text/text-inline-no-whitespace`
/// (`web-elements/tests/fixtures/x-text/text-inline-no-whitespace.html`,
/// `web-elements/tests/web-elements.spec.ts:234`): three adjacent runs written
/// with no whitespace between the tags shape as one uninterrupted line — a
/// nested inline `text` introduces neither a space, a box edge, nor a break
/// opportunity of its own.
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

/// Replicates `x-text/text-is-block-element`
/// (`web-elements/tests/fixtures/x-text/text-is-block-element.html`,
/// `web-elements/tests/web-elements.spec.ts:239`): a `text` is a block-level
/// box, so two siblings written with no whitespace between their tags still
/// occupy separate lines, and a source newline plus indentation inside one
/// collapses to a single space.
#[test]
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

/// Replicates `x-text/view-flex-in-text`
/// (`web-elements/tests/fixtures/x-text/view-flex-in-text.html`,
/// `web-elements/tests/web-elements.spec.ts:352`): a `view` child of a `text`
/// that selects flex layout becomes an atomic inline box whose shrink-to-fit
/// width — three 50x20 border-box items with 1px borders and 2px margins, so
/// 162 — feeds the line's advance, placed after the preceding run rather than
/// breaking the paragraph.
///
/// Geometry only; the borders and backgrounds the golden shows are paint. The
/// paragraph carries an explicit width because the fixture's viewport is wider
/// than this document's phone. The line's own height is left out for the same
/// reason as in the other atom replicas: a baseline-aligned box reserving no
/// descent below it is a recorded deviation
/// (`docs/tracking/deviations.md:213-220`), so the browser's line box is taller
/// than the atom.
/// Held only from `d19cbea2` (#227) on: before it the paragraph ran every
/// atom with `LayoutInput::measure`, which writes no box, and copied that
/// unwritten zero size into the placed layout. #227 needed committed geometry
/// for atomic children restored after a content replacement and gave the
/// paragraph a commit-goal path through `compute_inline_box_layout`
/// (`crates/dom/src/layout/text_block.rs:436-444`).
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

/// Replicates `x-text/raw-text`
/// (`web-elements/tests/fixtures/x-text/raw-text.html`,
/// `web-elements/tests/web-elements.spec.ts:392`): a `raw-text` child
/// contributes its `text` attribute as a plain run in the parent paragraph,
/// adds no box of its own, and sits in source order alongside an atomic inline
/// `image`.
///
/// The fixture's four-CJK payload becomes four Ahem em squares; the claim is
/// the carrier's transparency and the source order, not the script.
/// Held only from `d19cbea2` (#227) on: before it the paragraph ran every
/// atom with `LayoutInput::measure`, which writes no box, and copied that
/// unwritten zero size into the placed layout. #227 needed committed geometry
/// for atomic children restored after a content replacement and gave the
/// paragraph a commit-goal path through `compute_inline_box_layout`
/// (`crates/dom/src/layout/text_block.rs:436-444`).
#[test]
fn a_raw_text_carrier_contributes_a_run_and_no_box_of_its_own() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, text, "\n    ");
    let carrier = raw_text(&mut document, text, "abcd");
    literal(&mut document, text, "\n    ");
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

/// Replicates `x-text/raw-text-with-lynx-wrapper`
/// (`web-elements/tests/fixtures/x-text/raw-text-with-lynx-wrapper.html`,
/// `web-elements/tests/web-elements.spec.ts:399`): a transparent wrapper
/// between a `text` and a `raw-text` is invisible to the inline formatting
/// context — the wrapped carrier still contributes its run in source order
/// ahead of the atomic inline `image`, adding neither a box, a break
/// opportunity, nor extra whitespace.
///
/// This is the one of the three "with-lynx-wrapper" fixtures that actually
/// contains the wrapper.
///
/// Every geometric claim here is relative to the unwrapped control, which is
/// what "the wrapper changes nothing" means. The fixture's other half — that
/// the `x-image` really is a 22x22 box on that line — is
/// `the_image_beside_a_wrapped_carrier_is_a_real_box_on_the_line`, so that a
/// passing control comparison can never stand in for it: while both atoms were
/// zero-size a control comparison held over two invisible boxes.
#[test]
fn a_wrapper_between_a_text_and_a_raw_text_carrier_changes_nothing() {
    let mut document = ahem_document();
    let plain = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, plain, "\n    ");
    raw_text(&mut document, plain, "abcd");
    literal(&mut document, plain, "\n    ");
    let direct = image(&mut document, plain, "width: 22px; height: 22px");

    let wrapped = child(&mut document, "text", "font-size: 24px; color: blue");
    literal(&mut document, wrapped, "\n    ");
    let wrapper = element_under(&mut document, wrapped, "wrapper", "");
    literal(&mut document, wrapper, "\n      ");
    let carrier = raw_text(&mut document, wrapper, "abcd");
    literal(&mut document, wrapper, "\n    ");
    literal(&mut document, wrapped, "\n    ");
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

/// The absolute half of `x-text/raw-text-with-lynx-wrapper`
/// (`web-elements/tests/fixtures/x-text/raw-text-with-lynx-wrapper.html`,
/// `web-elements/tests/web-elements.spec.ts:399`): the golden shows the
/// `x-image` beside the wrapped carrier as a 22x22 picture on the line, which
/// no comparison against the unwrapped control can establish — two boxes that
/// are both invisible compare equal.
///
/// Split out of `a_wrapper_between_a_text_and_a_raw_text_carrier_changes_nothing`
/// so that the transparency claim and the claim that the wrapped fixture draws
/// anything at all are answered separately.
/// Held only from `d19cbea2` (#227) on: before it the paragraph ran every
/// atom with `LayoutInput::measure`, which writes no box, and copied that
/// unwritten zero size into the placed layout. #227 needed committed geometry
/// for atomic children restored after a content replacement and gave the
/// paragraph a commit-goal path through `compute_inline_box_layout`
/// (`crates/dom/src/layout/text_block.rs:436-444`).
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

/// Replicates `x-text/text-no-maxline-do-not-show-inline-truncation`
/// (`web-elements/tests/fixtures/x-text/text-no-maxline-do-not-show-inline-truncation.html`,
/// `web-elements/tests/web-elements.spec.ts:244`): truncation content is
/// content only while the block is in the overflowing-maxline state. With no
/// `text-maxline` the block is never in it, so the `inline-truncation` subtree
/// is skipped rather than clipped — no width, no line box, no trailing
/// whitespace — which the replica pins against the same paragraph written
/// without the child.
///
/// On its own this test cannot tell the two reasons for that emptiness apart,
/// and it would keep passing if the whole custom-truncation feature were
/// deleted. web-core skips the subtree because the block is not in the
/// overflowing-maxline state: `inline-truncation` starts at `display: none`
/// (`XText/x-text.css:45-49`) and `XTextTruncation` lifts it with
/// `x-show-inline-truncation` once a clamp is found to overflow
/// (`XText/XTextTruncation.ts:194-204`, `x-text.css:96-100`). This engine's UA
/// sheet declares the same `display: none` *unconditionally*
/// (`crates/bobcat-core/src/main/tree/text.rs:97`), with nothing to lift it, so
/// the negative case holds here for a reason that has nothing to do with the
/// state it is about. What pins the feature is its positive twin,
/// `truncation_content_is_laid_in_at_the_clamp_a_maxline_overflows`, which
/// declares a `text-maxline` the same paragraph overflows and fails today.
#[test]
fn truncation_content_is_skipped_entirely_when_no_maxline_is_declared() {
    const PARAGRAPH: &str = "width: 200px; font-size: 16px";

    let mut document = ahem_document();
    let text = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, text, "\n  hello world\n  ");
    let truncation = element_under(&mut document, text, "inline-truncation", "");
    literal(&mut document, truncation, "!!!!!!!");

    let without = child(&mut document, "text", PARAGRAPH);
    literal(&mut document, without, "\n  hello world\n  ");
    document.layout();

    assert_eq!(
        display(&document, truncation),
        Display::None,
        "the truncation subtree generates no box"
    );
    assert_eq!(
        ink(&document, text),
        ink(&document, without),
        "and contributes nothing at all to the paragraph that holds it"
    );
    assert_eq!(
        ink(&document, text).1,
        16.0,
        "one line: the seven exclamation marks did not open a second"
    );
}

/// The style every custom-truncation replica below is written in: a 200px
/// measure at 16px, breaking between characters, so a line holds exactly twelve
/// em squares and a cut point is an exact number of them.
const CLAMPED: &str = "width: 200px; font-size: 16px; word-break: break-all";

/// The positive twin of
/// `truncation_content_is_skipped_entirely_when_no_maxline_is_declared`
/// (`web-elements/tests/fixtures/x-text/text-no-maxline-do-not-show-inline-truncation.html`,
/// `web-elements/tests/web-elements.spec.ts:244`), and the tree-layer carrier
/// of the custom-truncation gap: the same paragraph with a `text-maxline` it
/// overflows, where web-core lays the `inline-truncation` content in at the
/// clamp.
///
/// web-core reaches that through a state rather than a fixed rule.
/// `inline-truncation` is `display: none` to begin with
/// (`XText/x-text.css:45-49`); `XTextTruncation` sets `x-show-inline-truncation`
/// on the host once the clamp is found to overflow
/// (`XText/XTextTruncation.ts:194-204`), which switches the subtree to
/// `display: inline-flex` (`x-text.css:96-100`). The same pass moves the cut:
/// the last visible line's kept end walks back from `end - 1` until the
/// discarded tail is at least as wide as the truncation content
/// (`XTextTruncation.ts:205-232`), and the content is laid in there with no
/// dots beside it — `::part(inner-box)::after` is emptied out for a block that
/// has an `inline-truncation` child (`x-text.css:196-201`).
///
/// The numbers follow from that. Twelve em squares fill the 200px measure at
/// 16px; the truncation content is one 32x22 image, two squares wide. One
/// square is not enough to cover it and two are exactly enough, so the cut
/// retreats by two and the image takes the 160..192 they vacated.
///
/// The fixture's indentation is dropped, as in the atomic-inline replicas:
/// this case's claim is an absolute placement, and the module-level
/// leading-space divergence would move it.
#[test]
#[ignore = "GAP (the wiring, in two places). The tree hands hughie no \
            truncation content at all: crates/dom/src/layout/text_block.rs:360 \
            passes `None` for `TextBlock::new`'s truncation slice. And the \
            subtree is dropped before it could be collected — \
            `inline-truncation { display: none }` \
            (crates/bobcat-core/src/main/tree/text.rs:97) is unconditional \
            here, where web-core's identical default is lifted by \
            `x-show-inline-truncation` once the block overflows its clamp. The \
            algorithm itself is complete one layer down: \
            crates/hughie/tests/web_text_replication.rs' \
            `custom_truncation_content_replaces_the_marker_at_the_clamp` passes"]
fn truncation_content_is_laid_in_at_the_clamp_a_maxline_overflows() {
    let mut document = ahem_document();
    let text = child(&mut document, "text", CLAMPED);
    literal(&mut document, text, &"a".repeat(24));
    let truncation = element_under(&mut document, text, "inline-truncation", "");
    let icon = image(&mut document, truncation, "width: 32px; height: 22px");
    set_limit(&mut document, text, "text-maxline", "1");
    document.layout();

    assert_ne!(
        display(&document, truncation),
        Display::None,
        "the paragraph overflows the one line it is allowed, so its truncation \
         subtree is content: the default the unclamped case keeps is lifted \
         exactly in this state"
    );
    assert_eq!(
        placement(&document, icon),
        Some((10.0 * 16.0, 32.0, 22.0)),
        "the cut retreats the two squares the 32px content needs, and the \
         content takes the width they vacated"
    );
    assert_eq!(
        ink(&document, text),
        (12.0 * 16.0, 16.0),
        "one clamped line, still filling the measure: ten kept squares and the \
         content that replaced the other two"
    );
}

/// Replicates `x-text/text-maxline-with-custom-truncation`
/// (`web-elements/tests/fixtures/x-text/text-maxline-with-custom-truncation.html`,
/// `web-elements/tests/web-elements.spec.ts:226`) at the tree layer: five
/// paragraphs holding the same run and the same `inline-truncation` child, the
/// first with no `text-maxline` and the rest clamped to 1, 2, 4 and 6 lines.
/// The unclamped one never shows the content; each clamped one shows it at the
/// end of its last visible line, inside the block's width, with no dots.
///
/// The fixture reaches its line structure with `word-break: break-all` over a
/// run of mixed sizes and letter-spacings; the replica keeps the break-all and
/// makes every unit one Ahem em square, so a line is twelve squares and the
/// retreat is countable. The freed-unit arithmetic over the fixture's *own*
/// mixed runs is not re-derived here — `crates/hughie`'s replica of this same
/// fixture owns it. What this layer adds is the wiring: whether an
/// `inline-truncation` written in the tree reaches the paragraph at all.
///
/// Each clamped block measures the full 192 that an unclamped line measures:
/// the two squares the retreat freed are exactly the two the 32px content
/// occupies. Three dots beside it would have to come out of a further retreat,
/// and the golden shows none.
#[test]
#[ignore = "GAP (the wiring, in two places). The tree hands hughie no \
            truncation content at all: crates/dom/src/layout/text_block.rs:360 \
            passes `None` for `TextBlock::new`'s truncation slice. And the \
            subtree is dropped before it could be collected — \
            `inline-truncation { display: none }` \
            (crates/bobcat-core/src/main/tree/text.rs:97) is unconditional \
            here, where web-core's identical default is lifted by \
            `x-show-inline-truncation` once the block overflows its clamp. The \
            algorithm itself is complete one layer down: \
            crates/hughie/tests/web_text_replication.rs' \
            `custom_truncation_content_replaces_the_marker_at_the_clamp` passes"]
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
        assert_ne!(
            display(&document, truncation),
            Display::None,
            "clamp {limit}: the block overflows, so its truncation content is \
             content"
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
        Display::None,
        "and the block that declared no clamp never enters the state that \
         would show its content"
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

/// Replicates `x-text/truncation-first-element-is-image`
/// (`web-elements/tests/fixtures/x-text/truncation-first-element-is-image.html`,
/// `web-elements/tests/web-elements.spec.ts:385`) at the tree layer: the first
/// unit of the run is a replaced box rather than text, and the truncation
/// content is itself a run plus a replaced box. The leading image survives at
/// the head of line 1 while the retreat consumes only the tail of the last
/// visible line, and both halves of the truncation content land inside the
/// block's width.
///
/// The fixture's CJK run becomes Ahem em squares under `word-break: break-all`,
/// which breaks between units the way the CJK run does, and its 更多 label
/// becomes a two-square run at the same 14px. Two things the fixture carries
/// are left out: the leading image's 4px `margin-right`, because an atom's
/// margin never reaching the line is a separate filed gap
/// (`a_compiled_card_s_inline_image_takes_its_used_size_and_its_margin`), and
/// the boxes' vertical placement, which for an atomic inline follows the
/// recorded line-box deviation (`docs/tracking/deviations.md:213-220`).
///
/// Numbers: a 300px measure at 24px, so line 1 holds the 32px image and eleven
/// squares and the lines under it hold twelve. The truncation content is 40
/// wide — two 14px squares and a 12x12 icon — so the retreat gives up two 24px
/// units of line 3, and the content occupies 240..280 of it.
#[test]
#[ignore = "GAP (the wiring, in two places). The tree hands hughie no \
            truncation content at all: crates/dom/src/layout/text_block.rs:360 \
            passes `None` for `TextBlock::new`'s truncation slice. And the \
            subtree is dropped before it could be collected — \
            `inline-truncation { display: none }` \
            (crates/bobcat-core/src/main/tree/text.rs:97) is unconditional \
            here, where web-core's identical default is lifted by \
            `x-show-inline-truncation` once the block overflows its clamp. The \
            algorithm itself is complete one layer down: \
            crates/hughie/tests/web_text_replication.rs' \
            `custom_truncation_content_replaces_the_marker_at_the_clamp` passes"]
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
    let label = element_under(&mut document, truncation, "text", "font-size: 14px");
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

/// Replicates `x-text/text-maxline-with-padding`
/// (`web-elements/tests/fixtures/x-text/text-maxline-with-padding.html`,
/// `web-elements/tests/web-elements.spec.ts:252`): line clamping composes with
/// the box model — the block is border-box sized, so padding narrows the inline
/// measure before line breaking, the clamp counts lines in that narrowed
/// measure, and the bottom padding still sits below the last line.
///
/// Geometry only, and padding is exactly what this case is about: whether the
/// clipped remainder is painted over that padding belongs to `crates/dom`. The
/// fixture reaches its four lines with `word-break: break-all` over one long
/// string; the replica reaches them with spaced words, so the break points are
/// the ones every engine agrees on and the measure is what is under test.
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

/// Replicates `x-text/text-maxline-1-instrict-size`
/// (`web-elements/tests/fixtures/x-text/text-maxline-1-instrict-size.html`,
/// `web-elements/tests/web-elements.spec.ts:340`): `text-maxline="1"` caps the
/// block's max inline size to the parent's available content width, even where
/// the block is a cross-axis-centred column flex item whose max-content size is
/// far wider. Without the cap the block sizes to max-content and paints outside
/// its 88px parent.
///
/// The fixture's ten CJK glyphs become ten Ahem em squares, which carry the
/// same per-glyph advance. Today the block wraps first and then keeps the one
/// line the clamp allows, so it ends up as wide as that wrapped line rather
/// than as wide as the parent — 80 instead of 88 — which is the same missing
/// cap seen from the other side.
#[test]
#[ignore = "GAP: no fill-available cap for a single-line clamp — the UA sheet \
            has no counterpart to web-core's \
            `x-text[text-maxline=\"1\"] { max-width: -webkit-fill-available }` \
            (crates/bobcat-core/src/main/tree/text.rs:91-101), and \
            content_widths() reports the pre-alignment layout's own widths \
            (crates/hughie/src/text/block/mod.rs:227-231,966-972)"]
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

/// Replicates `x-text/text-maxlength-with-raw-text`
/// (`web-elements/tests/fixtures/x-text/text-maxlength-with-raw-text.html`,
/// `web-elements/tests/web-elements.spec.ts:406`): a `raw-text` is transparent
/// to the character-index run — its payload joins the parent block's run
/// directly, so `text-maxlength="5"` cuts inside the payload and the block's
/// three-dot tail follows the five kept characters.
///
/// The tail is unconditional in web-core: `text-overflow` is consulted for
/// neither truncation attribute — it appears nowhere in `XTextTruncation.ts`,
/// which observes only `text-maxlength`, `text-maxline` and
/// `tail-color-convert` — and `x-text[text-maxlength]::part(inner-box)::after`
/// carries `content: "..."` outright (`x-text.css:191-194`).
#[test]
#[ignore = "GAP: the maxlength tail is gated on `TextOverflow::Ellipsis` \
            (crates/hughie/src/text/block/truncate.rs:135-146), whose initial \
            value is `clip` and which the Lynx UA sheet never declares \
            (crates/bobcat-core/src/main/tree/text.rs:91-101)"]
fn a_maxlength_cut_inside_a_raw_text_payload_still_gets_the_block_s_tail() {
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
        (8.0 * 20.0, 20.0),
        "five kept characters and the three-dot tail"
    );
}

/// Replicates `x-text/text-maxline-with-raw-text`
/// (`web-elements/tests/fixtures/x-text/text-maxline-with-raw-text.html`,
/// `web-elements/tests/web-elements.spec.ts:413`): `raw-text` is transparent to
/// inline layout and to line clamping, so the flattened run — and therefore the
/// clamp point — is identical to the same content authored as bare text nodes.
///
/// A carrier additionally inherits `white-space-collapse: preserve-breaks`,
/// which is inert for these payloads and is exactly why it has to be flattened
/// rather than treated as an atomic inline box.
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

/// Replicates `x-text/text-sizing-in-flex-container`
/// (`web-elements/tests/fixtures/x-text/text-sizing-in-flex-container.html`,
/// `web-elements/tests/web-elements.spec.ts:258`): a text block used as a flex
/// item in a centred row sizes to the max-content width of its run — it neither
/// stretches to the container nor wraps — and its trailing whitespace adds no
/// advance.
///
/// The fixture's six CJK glyphs become six Ahem em squares. The button's
/// background and radius are paint. `rounded_layout` snaps the centred item's
/// half-pixel cross offset to the device grid, which is why the y is 12 and not
/// the 11.5 the unrounded box carries.
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
    literal(&mut document, text, "abcdef\n    ");
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

/// Replicates `text/nest-text`
/// (`web-tests/dist/basic-element-text-nest-text/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2584`): a block `text` holding a
/// nested inline `text` of a different colour and a long trailing run, clamped
/// to one line.
///
/// `ellipsize-mode="tail"` is written for fidelity and is inert here — the
/// engine reflects `text-maxline` and `text-maxlength` alone
/// (`crates/bobcat-core/src/main/tree/text.rs:26-31`) — and it asks for nothing
/// web-core does not already do, tail being its only behaviour. The clamp
/// marker itself is not observable through this crate's surface, since a full
/// clamped line measures the same width with or without it; the marker is
/// pinned in `crates/hughie`.
///
/// This is the card's style half, which holds today. Its geometric half — that
/// a one-line clamp does not wrap at all in web-core, so the kept line runs to
/// the width the parent offers instead of stopping at the last word boundary
/// that fit — is
/// `a_one_line_clamp_fills_the_available_width_instead_of_breaking_at_a_word`,
/// split out so that a passing colour claim can never stand in for it.
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
        style_of(&document, nested).clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0),
    );
    assert_eq!(
        style_of(&document, nested).clone_font_weight(),
        style_of(&document, text).clone_font_weight(),
    );
}

/// The geometric half of `text/nest-text`
/// (`web-tests/dist/basic-element-text-nest-text/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2584`): what `text-maxline="1"` does
/// to the block's own measure before any marker is drawn.
///
/// web-core's single-line clamp is not "wrap, then keep line one". The
/// stylesheet puts `white-space: nowrap` (with `text-overflow: ellipsis`) on
/// `::part(inner-box)` for `text-maxline="1"` (`x-text.css:216-230`) and
/// `max-width: -webkit-fill-available` on the host (`x-text.css:232-237`), so
/// the paragraph never breaks: the one line runs to the width the parent
/// offers — the page's 393 here — and is ellipsized at that edge. This engine
/// breaks the paragraph first and then keeps the first line, so its measure
/// stops at the last word boundary that fit.
///
/// Split out of `a_one_line_clamp_keeps_the_nested_run_s_colour_and_the_parent_s_weight`
/// so that the card's style claim, which holds today, stays in CI while the
/// divergence this half carries is filed as the gap it is. It is the same
/// missing `fill-available` cap that
/// `a_single_line_clamp_caps_the_block_to_its_parent_s_available_width` names
/// from the other side.
#[test]
#[ignore = "GAP: no nowrap and no fill-available cap for a single-line clamp — \
            the UA sheet has no counterpart to web-core's \
            `x-text[text-maxline=\"1\"]` pair \
            (crates/bobcat-core/src/main/tree/text.rs:91-101), so the block \
            wraps at the measure and keeps the first line rather than running \
            one unbroken line to the parent's edge"]
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

    // `ink` is the shaped paragraph, so the expectation is glyph-quantised:
    // Ahem's 24px em square fits 16 whole units in the 393 the page offers, and
    // 393 itself is unreachable on this surface. 384 is what distinguishes a
    // line that ran to the edge and was ellipsized from today's word-boundary
    // break. The 393 cap itself is a box property, asserted through `frame` by
    // `a_single_line_clamp_caps_the_block_to_its_parent_s_available_width`.
    assert_eq!(
        ink(&document, text).0,
        384.0,
        "the clamped line is not broken at a word boundary: it runs out to the \
         16 whole units the page's 393 offers and is ellipsized there"
    );
}

/// Replicates `text/baseline`
/// (`web-tests/dist/basic-element-text-baseline/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2590`): the compiled `ReactLynx` form of
/// the baseline case — two sibling block `text` elements whose runs arrive as
/// `text` attributes, then one `text` holding two inline runs whose strings
/// arrive as `raw-text` children.
///
/// Both spellings appear in one card because the compiler picks between them by
/// whether the string is a static child or a nested element's content. The
/// replica keeps both; the same baseline claim written with literal text is
/// `runs_of_different_size_share_a_baseline_while_sibling_text_blocks_do_not`.
///
/// The two attribute blocks do not stack: the card writes
/// `__SetInlineStyles(l, "display:flex;")` on the `view` that holds them, so
/// they are flex items in a row and sit side by side. They do **not** get their
/// max-content widths, and nothing in the card's own numbers survives
/// unshrunk — the card's viewport is a phone, the same 393px this document
/// carries (`playwright-fixtures/src/playwright.common.ts:60-66`: Pixel 5 for
/// chromium, iPhone 12 Pro for webkit), and the card's outer `view` is a flex
/// row holding two 310px groups. Resolving that by CSS flexbox 9.7:
///
/// - The outer row is 393 and its two groups have equal 310px shrink factors, so each loses half
///   the 227px deficit: 196.5, above both their 190px min-content floors.
/// - Inside the attribute group, `hello world` (220) and `111` (90) shrink in proportion; `111`
///   hits its 90px min-content floor, freezes there, and the rest goes to `hello world`: 106.5,
///   which this engine rounds to 107.
/// - `hello world` at 106.5 keeps `hello` on line one and `world` on line two, so its paragraph ink
///   is 100 wide and two 20px lines tall. `111` still fits one line.
/// - The inline group is 196.5 too, and its paragraph's two runs are written back to back with no
///   whitespace, so `world111` is one unbreakable 190px unit: `hello` on line one, `world111` on
///   line two, ink 190 x (20 + 30).
///
/// The baseline claim the case exists for is the last line of that paragraph:
/// a 20px run and a 30px run hang from one baseline, so the line is the larger
/// run's 30px em box rather than the sum of the two.
///
/// Held only from `d19cbea2` (#227) on. Before it a `text` attribute on a
/// `text` element was inert — only `raw-text` reflected one — so both attribute
/// blocks measured zero and none of this was reachable.
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
        ((5.0 * 20.0, 2.0 * 20.0), (3.0 * 30.0, 30.0)),
        "each sibling block measures the run its attribute carries, at the \
         width the row's shrink leaves it"
    );
    assert_eq!(
        (frame(&document, small).0, frame(&document, large).0),
        (0.0, 107.0),
        "and the row places them side by side, not stacked: the container is a \
         flex row, and `111` froze at its 90px min-content floor so the rest \
         of the group's 196.5 went to `hello world`"
    );
    assert_eq!(
        ink(&document, paragraph),
        (5.0 * 20.0 + 3.0 * 30.0, 20.0 + 30.0),
        "and on its last line the two inline runs hang from one baseline, so \
         that line is the larger run's em box rather than the sum of the two"
    );
}

/// Replicates `text/nest-image`
/// (`web-tests/dist/basic-element-text-nest-image/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2611`): an `image` nested in a `text`
/// as an atomic inline box — a fixed 22x22 used size with a 10px left margin,
/// placed straight after the run, with no collapsed space between them because
/// the compiled card appends the two children back to back.
///
/// The asset is a 162x162 PNG drawn at 22x22, and the used size wins: an
/// `image` box is CSS-sized, never bitmap-sized. Whether the bitmap is drawn is
/// paint. Two separate defects meet here — the margin the line never advances
/// by, and the atom's own box, which the `x-text/inline-image` replica names.
#[test]
#[ignore = "GAP: an atom's margin never reaches the line — \
            crates/dom/src/layout/text_block.rs:350-366 hands the block \
            `output.size`, which is the atom's border box, so the 10px \
            margin-left is dropped from the advance"]
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

/// Replicates `text/nest-view`
/// (`web-tests/dist/basic-element-text-nest-view/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2617`): a padded, bordered, rounded
/// `view` nested in a `text` as an atomic inline box, holding a `text` of its
/// own, with raw strings flowing before and after it.
///
/// Geometry only: the border, its radius and the inner run's gradient fill are
/// paint. The paragraph carries an explicit width because the card's viewport
/// is wider than this document's phone, and the width is what preserves the
/// fixture's single line.
///
/// The atom's vertical placement in the line is left out, as in the other
/// atomic-inline replicas: a baseline-aligned box reserving no descent below it
/// is a recorded deviation (`docs/tracking/deviations.md:213-220`), so the
/// browser's line box and this one put the box at different `y`, and this case
/// is not the one that should pin that difference.
///
/// Held only from `d19cbea2` (#227) on: before it an atom's subtree was only
/// ever measured, so a text block nested inside an atom committed no paragraph
/// at all and the inner run measured zero. The inner `text` carries its run as
/// a `text` attribute, which the same commit made live — see
/// `a_text_attribute_on_a_text_element_carries_the_paragraph_s_run`.
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
            style_of(&document, inner).clone_color_value(),
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

/// Replicates `text/with-new-line`
/// (`web-tests/dist/basic-element-text-with-new-line/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2636`): a literal `\n` inside a
/// `raw-text` child of a `text` forces a hard line break — Lynx text does not
/// collapse it the way HTML's `white-space: normal` would.
///
/// This is the compiled shape of a *dynamic* string child: it becomes
/// `__CreateRawText`. A static one becomes a `text` attribute instead, which
/// `a_text_attribute_on_a_text_element_carries_the_paragraph_s_run` covers.
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

/// Replicates `text/maxline`
/// (`web-tests/dist/basic-element-text-maxline/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2718`): three `text` blocks carrying
/// the same wrapping paragraph at `text-maxline` 1, 2 and unset, stacked in a
/// column.
///
/// The default tail marker is unconditional in web-core: for `text-maxline >= 2`
/// the stylesheet sets `-webkit-line-clamp`, and for `text-maxline == 1` it
/// declares `text-overflow: ellipsis; white-space: nowrap` on the inner box
/// itself (`x-text.css:207-241`). Neither consults the author's `text-overflow`.
/// Held only from `d19cbea2` (#227) on: before it only `raw-text` reflected a
/// `text` attribute into a run, so a `text` element's own attribute was inert
/// and the block measured zero. #227 renders `text[text] { content: attr(text) }`
/// from the element's primary computed style.
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

/// Replicates `text/display-none`
/// (`web-tests/dist/basic-element-text-display-none/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2742`): `display: none` on a block
/// `text` and on an inline run inside one — both leave layout entirely, and the
/// card renders nothing at all.
///
/// The card's first pair carries its strings as `text` attributes; the replica
/// writes them as the `raw-text` the dynamic path compiles to, so that lifting
/// the declaration is a real control. The attribute spelling itself is pinned
/// by `a_text_attribute_on_a_text_element_carries_the_paragraph_s_run`.
///
/// This does not hold here, and the cause is a deliberate, already-recorded
/// decision rather than an unbuilt feature. Both engines fix a text's
/// inline-ness, but at different layers: native rewrites the tag outright
/// (`TextElement::ConvertToInlineElement`,
/// `/Users/akiwah/repos/lynx/core/renderer/dom/fiber/text_element.cc:196-205`),
/// while web-core spells it as ordinary author-origin CSS
/// (`x-text { display: flex }`, `x-text.css:7-13`, with no `!important`), so
/// the card's inline `display: none` outranks it there. This engine writes the
/// same invariant as an important user-agent declaration — the one exception
/// §D.15 of `docs/style-assumptions.md` grants, recorded at
/// `docs/tracking/deviations.md:485-494` — which swallows the author's
/// `display: none` along with it. The exception was written to fix a text's
/// inline-ness, not to make a text unhideable; that is the deviation-review
/// item this test stands for.
#[test]
#[ignore = "DEVIATION, not a missing feature: \
            `text { display: -lynx-text !important }` \
            (crates/bobcat-core/src/main/tree/text.rs:94) is user-agent origin, \
            so it outranks the author's inline `display: none` and no `text` \
            element can be hidden by the declaration the card writes. The \
            important-UA exception is §D.15's, recorded at \
            docs/tracking/deviations.md:485-494; it was granted for a text's \
            inline-ness, not to make a text unhideable"]
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

/// Replicates `text/color`
/// (`web-tests/dist/basic-element-text-color/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2881`): two equal-specificity class
/// rules on one `text`, where the later one in the sheet wins both the
/// `font-size` and a gradient `color` over a solid one — the class attribute's
/// own order decides nothing.
///
/// The card writes its string as a `text` attribute on the `text`; the replica
/// writes the same string as the `raw-text` the
/// dynamic path compiles to, so the golden's shaped half — ten glyphs at the
/// winning rule's 80px, not its 16px — is a real claim rather than a computed
/// style read over an empty paragraph. Ahem's em square makes those ten glyphs
/// 800px wide, so the paragraph carries the explicit width that keeps the
/// golden's single line.
#[test]
fn the_later_of_two_equal_specificity_class_rules_wins_on_a_text() {
    let mut document = ahem_document();
    document.add_stylesheet(
        ".a { color: red; font-size: 16px }\n\
         .b { color: linear-gradient(green, #ff0); font-size: 80px }",
        dom::StylesheetOrigin::Author,
    );
    let text = child(&mut document, "text", "width: 900px");
    document.add_class(text, "a");
    document.add_class(text, "b");
    raw_text(&mut document, text, "hello lynx");
    document.layout();

    let style = style_of(&document, text);
    assert!(
        matches!(style.clone_color_value(), ColorPropertyValue::Gradient(_)),
        "source order decides the cascade, so `.b`'s gradient wins over `.a`'s red"
    );
    assert_eq!(style.get_font().clone_font_size().used_size().px(), 80.0);
    assert_eq!(
        ink(&document, text),
        (10.0 * 80.0, 80.0),
        "and the run is shaped at that size: ten em squares on one line, not \
         the 16px the earlier rule asked for"
    );
}

/// Replicates `reactlynx/basic-color-not-inherit`
/// (`web-core-e2e/tests/reactlynx/basic-color-not-inherit/index.jsx`,
/// `web-core-e2e/tests/reactlynx.spec.ts:1275`): a `view` with `color: red`
/// does not pass that colour into a `text` child, which resolves to the black
/// initial value.
///
/// The original asserts `toHaveCSS('color', 'rgb(0, 0, 0)')` on the text. The
/// mechanism differs from web-core's: that target reaches the result through
/// the card's `enableCSSInheritance: false` page config, while this engine
/// always cascades and buys the same value with the UA `text { color: initial }`
/// reset (`crates/bobcat-core/src/main/tree/text.rs:94`).
///
/// The card writes the string as a `text` attribute on the `text`; the replica
/// writes it as the `raw-text` the dynamic path
/// compiles to, so the colour being asserted is the colour of a paragraph that
/// really holds a run.
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
        style_of(&document, text).clone_color(),
        AbsoluteColor::BLACK,
        "the reset stops the cascade at the text root"
    );
    assert_ne!(
        style_of(&document, view).clone_color(),
        AbsoluteColor::BLACK,
        "and the ancestor really did declare a colour to inherit"
    );
}
