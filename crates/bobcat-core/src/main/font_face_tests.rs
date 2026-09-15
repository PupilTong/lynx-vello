//! The `@font-face` loader seam, driven end to end through a real view.
//!
//! What the pieces are: `dom` reports the rules a mounted sheet declared
//! ([`dom::Document::take_font_face_requests`]), the page's epilogue spawns
//! one task per rule, each task asks the host for
//! [`SourceRequest::Font`](crate::resource::SourceRequest::Font) in author
//! order, and the first answer is registered under the declared family. These
//! tests are about the joins between them, so they measure text: the vendored
//! Ahem face's glyphs are solid em squares, so a run shaped with it is exactly
//! its glyph count times its font size and nothing else in a test process
//! produces that number.

use std::time::{Duration, Instant};

use crate::test_support::{TestEngine, TestViewSpec, wait_for_boot};

/// Solid em squares: five characters at 16px are exactly 80px of advance.
const AHEM: &[u8] = include_bytes!("../../../hughie/tests/fixtures/Ahem.ttf");

/// One text element holding one carrier, which is what a compiled card's
/// update slots produce.
const ENTRY: &str = r"
    const page = __CreatePage('card', 0);
    const text = __CreateText(0);
    __AppendElement(text, __CreateRawText('EXTRA'));
    __AppendElement(page, text);
    __FlushElementTree();
";

/// The advance the page's one text element measured, once it has one.
fn measured_width(engine: &mut TestEngine) -> Option<f32> {
    engine
        .probe_document(|tree| {
            let text = tree.document_element().first_child()?.id();
            tree.text_block_size(text).map(|size| size.width)
        })
        .flatten()
}

/// Turns until the page's text measures `width`, or says what it measured
/// instead. A declared face arrives after boot — the load is a task of its
/// own — so every assertion here is a wait rather than a read.
fn wait_for_width(engine: &mut TestEngine, width: f32) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = None;
    loop {
        engine.pump();
        last = measured_width(engine).or(last);
        if last.is_some_and(|measured| (measured - width).abs() < f32::EPSILON) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the text never measured {width}px; it measured {last:?}"
        );
        std::thread::yield_now();
    }
}

/// The whole seam: a card declares a face with `@font-face`, the host serves
/// its `src`, and the text naming that family is shaped by it.
///
/// Before the load lands the family is unknown and the run falls back to
/// whatever the platform answers with, so the 80px this waits for can only
/// come from the declared face having reached shaping.
#[test]
fn a_declared_font_face_is_fetched_and_shapes_the_text_naming_it() {
    let mut engine = TestViewSpec::new(ENTRY)
        .with_style_sheet(
            "@font-face { font-family: DeclaredAhem; src: url(\"app:///ahem.ttf\"); }
             text { font-family: DeclaredAhem; font-size: 16px; }",
        )
        .with_font("app:///ahem.ttf", AHEM)
        .create(std::sync::Arc::new(crate::view::NoWakeup));
    wait_for_boot(&mut engine);

    wait_for_width(&mut engine, 80.0);
}

/// A `src` list is tried in author order until one source loads: the two the
/// host serves nothing for fail, and the third is what shapes the text.
///
/// The `local()` in the middle is skipped rather than requested — this engine
/// has no platform font enumerator — and a rule whose sources all failed would
/// leave the run on its fallback, so reaching 80px is the pass.
#[test]
fn a_failing_font_face_source_falls_through_to_the_next_one() {
    let mut engine = TestViewSpec::new(ENTRY)
        .with_style_sheet(
            "@font-face { font-family: DeclaredAhem;
                          src: url(\"app:///missing.ttf\"),
                               local(\"Some Installed Face\"),
                               url(\"app:///ahem.ttf\"); }
             text { font-family: DeclaredAhem; font-size: 16px; }",
        )
        .with_font("app:///ahem.ttf", AHEM)
        .create(std::sync::Arc::new(crate::view::NoWakeup));
    wait_for_boot(&mut engine);

    wait_for_width(&mut engine, 80.0);
}
