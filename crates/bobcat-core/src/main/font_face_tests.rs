//! The `@font-face` loader seam, driven end to end through a real view.
//!
//! What the pieces are: `terminus` reports the rules a mounted sheet declared
//! ([`terminus::link::Document::take_font_face_requests`]), the page's epilogue spawns
//! one task per rule, each task asks the host for
//! [`SourceRequest::Font`](crate::resource::SourceRequest::Font) in author
//! order, and the first answer is registered under the declared family. These
//! tests are about the joins between them, so they measure text: the vendored
//! Ahem face's glyphs are solid em squares, so a run shaped with it is exactly
//! its glyph count times its font size and nothing else in a test process
//! produces that number.

use std::time::{Duration, Instant};

use crate::test_support::{TestEngine, TestViewSpec, wait_for_boot};

const AHEM: &[u8] = include_bytes!("../../../seldon/tests/fixtures/Ahem.ttf");

const ENTRY: &str = r"
    const page = __CreatePage('card', 0);
    const text = __CreateText(0);
    __AppendElement(text, __CreateRawText('EXTRA'));
    __AppendElement(page, text);
    __FlushElementTree();
";

fn measured_width(engine: &mut TestEngine) -> Option<f32> {
    engine
        .probe_document(|tree| {
            let text = tree.document_element().first_child()?.id();
            tree.text_block_size(text).map(|size| size.width)
        })
        .flatten()
}

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
