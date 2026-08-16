// Copyright 2026 The Lynx Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Compatibility tests for the restricted Lynx XML source-container grammar.
//!
//! This is a Rust port of lynx-stack's Apache-2.0-licensed
//! `parseLynxXML.spec.ts` suite at pull request #3390. Keep the acceptance and
//! rejection corpus aligned with that reference parser; Lynx XML deliberately
//! is not general-purpose XML.

use lynx_xml::{LynxXml, parse};

fn expect_success(source: &str) -> LynxXml<'_> {
    match parse(source) {
        Ok(parsed) => parsed,
        Err(error) => panic!("expected a successful parse, got: {error}"),
    }
}

fn expect_failure(source: &str) -> String {
    let Err(error) = parse(source) else {
        panic!("expected a failed parse for: {source}");
    };
    let message = error.message().to_owned();
    assert_eq!(
        error.to_string(),
        format!(
            "invalid TemplateBundle XML at offset {}: {}",
            error.offset(),
            error.message()
        )
    );
    assert!(
        error
            .to_string()
            .starts_with("invalid TemplateBundle XML at offset ")
    );
    message
}

#[test]
fn parses_a_full_document_with_declaration_doctype_and_cdata() {
    let style = "\n.card { width: 100px; color: red; }\n";
    let main_thread_script = "\nfunction renderPage() { return null; }\n";
    let background_thread_script = "\nglobalThis.__background_started = true;\n";
    let source = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE lynx>\n\
         <lynx version=\"5.4.2\">\n\
         <style><![CDATA[{style}]]></style>\n\
         <script main-thread=\"true\"><![CDATA[{main_thread_script}]]></script>\n\
         <script background=\"true\"><![CDATA[{background_thread_script}]]></script>\n\
         </lynx>"
    );
    let result = expect_success(&source);

    assert_eq!(result.style, Some(style));
    assert_eq!(result.main_thread_script, main_thread_script);
    assert_eq!(
        result.background_thread_script,
        Some(background_thread_script)
    );
}

#[test]
fn parses_the_minimal_legal_document() {
    let result = expect_success("<lynx version=\"5.4.2\"><script main-thread>main</script></lynx>");

    assert_eq!(result.style, None);
    assert_eq!(result.main_thread_script, "main");
    assert_eq!(result.background_thread_script, None);
}

#[test]
fn keeps_bare_content_verbatim_whitespace_included() {
    let result = expect_success(concat!(
        "<lynx version=\"5.4.2\">\n",
        "<style>\n.card { width: 1px; }\n</style>\n",
        "<script main-thread>\nmain\n</script>\n",
        "</lynx>",
    ));

    assert_eq!(result.style, Some("\n.card { width: 1px; }\n"));
    assert_eq!(result.main_thread_script, "\nmain\n");
}

#[test]
fn allows_cdata_content_that_contains_a_closing_tag() {
    let main_thread_script = "\nconst closingTag = \"</script>\";\n";
    let source = format!(
        "<lynx version=\"5.4.2\"><script main-thread=\"true\"><![CDATA[{main_thread_script}]]></script></lynx>"
    );
    let result = expect_success(&source);

    assert_eq!(result.main_thread_script, main_thread_script);
}

#[test]
fn accepts_the_three_boolean_attribute_spellings() {
    for attribute in ["main-thread", "main-thread=\"true\"", "main-thread='true'"] {
        let source = format!("<lynx version=\"5.4.2\"><script {attribute}>main</script></lynx>");
        assert_eq!(expect_success(&source).main_thread_script, "main");
    }

    for attribute in ["background", "background=\"true\"", "background='true'"] {
        let source = format!(
            "<lynx version=\"5.4.2\"><script main-thread>main</script><script {attribute}>bg</script></lynx>"
        );
        assert_eq!(expect_success(&source).background_thread_script, Some("bg"));
    }
}

#[test]
fn accepts_doctype_casing_variants() {
    for doctype in [
        "<!DOCTYPE lynx>",
        "<!doctype lynx>",
        "<!DoCtYpE LyNx>",
        "<!DOCTYPE   lynx  >",
    ] {
        let source =
            format!("{doctype}<lynx version=\"5.4.2\"><script main-thread>main</script></lynx>");
        assert_eq!(expect_success(&source).main_thread_script, "main");
    }
}

#[test]
fn accepts_a_single_quoted_version_attribute() {
    let result = expect_success("<lynx version='5.4.2'><script main-thread>main</script></lynx>");

    assert_eq!(result.main_thread_script, "main");
}

#[test]
fn skips_comments_in_every_ignorable_position() {
    let result = expect_success(concat!(
        "<!-- before declaration -->\n",
        "<?xml version=\"1.0\"?>\n",
        "<!-- between declaration and doctype -->\n",
        "<!DOCTYPE lynx>\n",
        "<!-- before root -->\n",
        "<lynx version=\"5.4.2\">\n",
        "<!-- inside root -->\n",
        "<script main-thread>main</script>\n",
        "<!-- between sections -->\n",
        "<script background>bg</script>\n",
        "</lynx>\n",
        "<!-- after root -->\n",
    ));

    assert_eq!(result.main_thread_script, "main");
    assert_eq!(result.background_thread_script, Some("bg"));
}

#[test]
fn accepts_optional_sections_in_any_order_and_a_leading_bom() {
    let result = expect_success(concat!(
        "\u{feff}<lynx version=\"5.4.2\">\n",
        "<script background>background-code</script>\n",
        "<script main-thread>main-code</script>\n",
        "</lynx>",
    ));

    assert_eq!(result.style, None);
    assert_eq!(result.main_thread_script, "main-code");
    assert_eq!(result.background_thread_script, Some("background-code"));
}

#[test]
fn parses_a_document_without_a_style_section() {
    let result = expect_success(concat!(
        "<lynx version=\"5.4.2\">",
        "<script main-thread>main</script>",
        "<script background>bg</script>",
        "</lynx>",
    ));

    assert_eq!(result.style, None);
    assert_eq!(result.background_thread_script, Some("bg"));
}

#[test]
fn parses_a_document_without_a_background_script_section() {
    let result = expect_success(concat!(
        "<lynx version=\"5.4.2\">",
        "<style>.a { width: 1px; }</style>",
        "<script main-thread>main</script>",
        "</lynx>",
    ));

    assert_eq!(result.style, Some(".a { width: 1px; }"));
    assert_eq!(result.background_thread_script, None);
}

#[test]
fn keeps_an_empty_style_section_as_an_empty_string() {
    let result = expect_success(concat!(
        "<lynx version=\"5.4.2\"><style></style>",
        "<script main-thread>main</script></lynx>",
    ));

    assert_eq!(result.style, Some(""));
}

#[test]
fn accepts_extra_whitespace_around_attributes() {
    for source in [
        "<lynx  version = \"5.4.2\" ><script main-thread>m</script></lynx>",
        "<lynx version=\"5.4.2\" ><script main-thread>m</script></lynx>",
        "<lynx version=\"5.4.2\"><script  main-thread  =  \"true\" >m</script></lynx>",
    ] {
        assert_eq!(expect_success(source).main_thread_script, "m");
    }
}

#[test]
fn keeps_an_empty_cdata_section_as_an_empty_string() {
    let result = expect_success(concat!(
        "<lynx version=\"5.4.2\">",
        "<script main-thread><![CDATA[]]></script></lynx>",
    ));

    assert_eq!(result.main_thread_script, "");
}

#[test]
fn tolerates_trailing_whitespace_after_the_root_closing_tag() {
    let result =
        expect_success("<lynx version=\"5.4.2\"><script main-thread>main</script></lynx>\n\n");

    assert_eq!(result.main_thread_script, "main");
}

#[test]
fn rejects_a_missing_root_element() {
    assert!(expect_failure("<script main-thread>main</script>").contains("root element"));
}

#[test]
fn rejects_a_non_lynx_doctype() {
    assert_eq!(
        expect_failure(concat!(
            "<!doctype html><lynx version=\"5.4.2\">",
            "<script main-thread>main</script></lynx>",
        )),
        "expected '<!doctype lynx>'"
    );
}

#[test]
fn rejects_an_unterminated_doctype_declaration() {
    assert_eq!(
        expect_failure("<!doctype lynx"),
        "unterminated doctype declaration"
    );
}

#[test]
fn rejects_a_missing_version_attribute() {
    assert!(
        expect_failure("<lynx><script main-thread>main</script></lynx>")
            .contains("'version' attribute")
    );
}

#[test]
fn rejects_an_empty_version_attribute() {
    assert!(
        expect_failure("<lynx version=\"\"><script main-thread>main</script></lynx>")
            .contains("'version' attribute")
    );
}

#[test]
fn rejects_an_unrelated_root_attribute() {
    assert!(
        expect_failure("<lynx lang=\"en\"><script main-thread>main</script></lynx>")
            .contains("'version' attribute")
    );
}

#[test]
fn rejects_an_unterminated_root_opening_tag() {
    assert_eq!(
        expect_failure("<lynx version=\"5.4.2\""),
        "unterminated '<lynx>' opening tag"
    );
}

#[test]
fn rejects_a_missing_main_thread_script() {
    assert_eq!(
        expect_failure(concat!(
            "<lynx version=\"5.4.2\">",
            "<script background>background</script></lynx>",
        )),
        "missing '<script main-thread>' section"
    );
}

#[test]
fn rejects_a_document_with_only_a_style_section() {
    assert_eq!(
        expect_failure(concat!(
            "<lynx version=\"5.4.2\"><style>.a { width: 1px; }</style>",
            "</lynx>",
        )),
        "missing '<script main-thread>' section"
    );
}

#[test]
fn rejects_duplicate_script_sections() {
    assert!(
        expect_failure(concat!(
            "<lynx version=\"5.4.2\"><script main-thread>main</script>",
            "<script main-thread>duplicate</script></lynx>",
        ))
        .contains("duplicate")
    );
    assert!(
        expect_failure(concat!(
            "<lynx version=\"5.4.2\"><script main-thread>main</script>",
            "<script background>a</script><script background>b</script></lynx>",
        ))
        .contains("duplicate")
    );
}

#[test]
fn rejects_duplicate_style_sections() {
    assert_eq!(
        expect_failure(concat!(
            "<lynx version=\"5.4.2\"><style>a</style><style>b</style>",
            "<script main-thread>main</script></lynx>",
        )),
        "duplicate '<style>' section"
    );
}

#[test]
fn rejects_attributes_on_the_style_section() {
    assert_eq!(
        expect_failure(concat!(
            "<lynx version=\"5.4.2\"><style scoped>.a { width: 1px; }</style>",
            "<script main-thread>main</script></lynx>",
        )),
        "'<style>' does not accept attributes"
    );
}

#[test]
fn rejects_a_script_without_main_thread_or_background() {
    for opening_tag in [
        "<script>",
        "<script worker>",
        "<script main-thread=\"false\">",
        "<script background=\"false\">",
        "<script main-thread background>",
    ] {
        let source = format!("<lynx version=\"5.4.2\">{opening_tag}main</script></lynx>");
        assert_eq!(
            expect_failure(&source),
            "'<script>' requires exactly one of 'main-thread' or 'background'"
        );
    }
}

#[test]
fn rejects_unknown_top_level_tags_naming_the_tag() {
    assert_eq!(
        expect_failure(concat!(
            "<lynx version=\"5.4.2\"><view></view>",
            "<script main-thread>main</script></lynx>",
        )),
        "unsupported top-level tag '<view>'"
    );
}

#[test]
fn rejects_an_unexpected_closing_tag_at_the_top_level() {
    assert_eq!(
        expect_failure(concat!(
            "<lynx version=\"5.4.2\"></style>",
            "<script main-thread>main</script></lynx>",
        )),
        "unexpected closing tag"
    );
}

#[test]
fn rejects_an_unterminated_section_opening_tag() {
    assert_eq!(
        expect_failure("<lynx version=\"5.4.2\"><script main-thread"),
        "unterminated opening tag"
    );
}

#[test]
fn rejects_an_unterminated_cdata_section() {
    assert_eq!(
        expect_failure(concat!(
            "<lynx version=\"5.4.2\">",
            "<script main-thread=\"true\"><![CDATA[main</script></lynx>",
        )),
        "unterminated CDATA section"
    );
}

#[test]
fn rejects_content_after_the_cdata_section() {
    assert_eq!(
        expect_failure(concat!(
            "<lynx version=\"5.4.2\">",
            "<script main-thread><![CDATA[a]]>trailing]]></script></lynx>",
        )),
        "unexpected content after the CDATA section"
    );
}

#[test]
fn rejects_an_unterminated_comment() {
    assert_eq!(
        expect_failure("<lynx version=\"5.4.2\"><!-- unterminated"),
        "unterminated comment"
    );
}

#[test]
fn rejects_a_missing_section_closing_tag() {
    assert_eq!(
        expect_failure("<lynx version=\"5.4.2\"><script main-thread>main"),
        "missing closing tag '</script>'"
    );
}

#[test]
fn rejects_a_missing_root_closing_tag() {
    assert_eq!(
        expect_failure("<lynx version=\"5.4.2\"><script main-thread=\"true\">main</script>"),
        "missing closing tag '</lynx>'"
    );
}

#[test]
fn rejects_content_after_the_root_closing_tag() {
    assert_eq!(
        expect_failure(concat!(
            "<lynx version=\"5.4.2\"><script main-thread>main</script>",
            "</lynx>trailing",
        )),
        "unexpected content after '</lynx>'"
    );
}

#[test]
fn rejects_an_unterminated_xml_declaration() {
    assert_eq!(
        expect_failure(concat!(
            "<?xml version=\"1.0\"<lynx version=\"5.4.2\">",
            "<script main-thread>main</script></lynx>",
        )),
        "unterminated XML declaration"
    );
}

#[test]
fn rejects_an_empty_document() {
    assert!(expect_failure("").contains("root element"));
}

#[test]
fn rejects_bare_text_between_sections() {
    assert_eq!(
        expect_failure(concat!(
            "<lynx version=\"5.4.2\">garbage",
            "<script main-thread>main</script></lynx>",
        )),
        "unexpected content outside a section"
    );
}

#[test]
fn never_panics_whatever_the_input_is() {
    for source in [
        "",
        "<",
        "<!",
        "<!-",
        "<?xml",
        "<lynx",
        "<lynx version",
        "<lynx version=",
        "<lynx version=\"",
        "</lynx>",
        "<![CDATA[",
        "\u{feff}",
        "\u{feff}<lynx version=\"1\">",
    ] {
        let result = std::panic::catch_unwind(|| parse(source));
        assert!(result.is_ok(), "parser panicked for {source:?}");
        assert!(
            result.expect("checked above").is_err(),
            "parser accepted {source:?}"
        );
    }
}

#[test]
fn reports_the_offset_of_the_failing_section_not_of_the_document() {
    let prefix = "<lynx version=\"5.4.2\">\n";
    let source = format!("{prefix}<view></view><script main-thread>main</script></lynx>");
    let Err(error) = parse(&source) else {
        panic!("expected the unsupported top-level tag to fail");
    };

    assert_eq!(error.offset(), prefix.len());
}

#[test]
fn reports_web_and_rust_offsets_after_non_ascii_text() {
    let prefix = "<!-- é😀 -->\n<lynx version=\"1\">";
    let source = format!("{prefix}<view></view></lynx>");
    let error = parse(&source).expect_err("the unsupported top-level tag must fail");

    assert_eq!(error.offset(), prefix.encode_utf16().count());
    assert_eq!(error.byte_offset(), prefix.len());
    assert_eq!(
        error.to_string(),
        format!(
            "invalid TemplateBundle XML at offset {}: unsupported top-level tag '<view>'",
            prefix.encode_utf16().count()
        )
    );
}

#[test]
fn uses_the_reference_ascii_whitespace_set() {
    let accepted = concat!(
        "\u{000c}<lynx version=\"1\">\u{000c}",
        "<script main-thread>main</script>\u{000c}</lynx>\u{000c}",
    );
    assert_eq!(expect_success(accepted).main_thread_script, "main");

    for rejected in [
        "\u{000b}<lynx version=\"1\"><script main-thread>main</script></lynx>",
        "\u{00a0}<lynx version=\"1\"><script main-thread>main</script></lynx>",
    ] {
        assert_eq!(
            expect_failure(rejected),
            "expected '<lynx version=\"...\">' root element"
        );
    }
}

#[test]
fn treats_the_xml_declaration_as_an_unvalidated_prefix_slot() {
    let result = expect_success(concat!(
        "<?xml-stylesheet?>",
        "<lynx version=\"opaque\"><script main-thread>main</script></lynx>",
    ));
    assert_eq!(result.main_thread_script, "main");
}

#[test]
fn preserves_comments_inside_section_bodies_as_payload() {
    let result = expect_success(concat!(
        "<lynx version=\"1\"><script main-thread>",
        "<!-- this is JavaScript payload here -->",
        "</script></lynx>",
    ));
    assert_eq!(
        result.main_thread_script,
        "<!-- this is JavaScript payload here -->"
    );
}

#[test]
fn distinguishes_the_two_unterminated_root_prefixes() {
    assert_eq!(
        expect_failure("<lynx"),
        "expected '<lynx version=\"...\">' root element"
    );
    assert_eq!(
        expect_failure("<lynx "),
        "unterminated '<lynx>' opening tag"
    );
}

#[test]
fn parses_markup_card_into_three_non_empty_sections() {
    let result = expect_success(MARKUP_CARD_XML);

    let style = result.style.expect("fixture has a style section");
    assert!(!style.is_empty());
    assert!(style.contains(".page"));
    assert!(!result.main_thread_script.is_empty());
    assert!(result.main_thread_script.contains("__CreatePage"));
    let background = result
        .background_thread_script
        .expect("fixture has a background script");
    assert!(!background.is_empty());
    assert!(background.contains("lynx.getCoreContext"));

    for section in [style, result.main_thread_script, background] {
        assert!(!section.contains("<![CDATA["));
        assert!(!section.contains("]]>"));
    }
}

#[test]
fn rejects_every_document_the_reference_parser_rejects() {
    let sources = [
        "<script main-thread>main</script>",
        concat!(
            "<!doctype html><lynx version=\"5.4.2\">",
            "<script main-thread>main</script></lynx>",
        ),
        concat!(
            "<lynx version=\"5.4.2\"><style scoped>.a { width: 1px; }</style>",
            "<script main-thread>main</script></lynx>",
        ),
        "<lynx version=\"5.4.2\"><script>main</script></lynx>",
        "<lynx version=\"5.4.2\"><script worker>main</script></lynx>",
        concat!(
            "<lynx version=\"5.4.2\"><script main-thread>main</script>",
            "<script main-thread>duplicate</script></lynx>",
        ),
        "<lynx version=\"5.4.2\"><script background>background</script></lynx>",
        concat!(
            "<lynx version=\"5.4.2\"><view></view>",
            "<script main-thread>main</script></lynx>",
        ),
        "<lynx version=\"5.4.2\"><script main-thread>main",
        "<lynx version=\"5.4.2\"><!-- unterminated",
        "<lynx><script main-thread>main</script></lynx>",
        "<lynx version=\"\"><script main-thread>main</script></lynx>",
        "<lynx lang=\"en\"><script main-thread>main</script></lynx>",
        "<lynx version=\"5.4.2\"><script main-thread=\"false\">main</script></lynx>",
        concat!(
            "<lynx version=\"5.4.2\">",
            "<script main-thread=\"true\"><![CDATA[main</script></lynx>",
        ),
        "<lynx version=\"5.4.2\"><script main-thread=\"true\">main</script>",
    ];

    for source in sources {
        expect_failure(source);
    }
}

#[test]
fn extracts_the_same_sections_as_the_reference_parser() {
    let full = expect_success(concat!(
        "<?xml version=\"1.0\"?>\n",
        "<!-- A Lynx single-file bundle. -->\n",
        "<lynx version=\"5.4.2\">\n",
        "<style>\n.card { width: 100px; }\n</style>\n",
        "<script main-thread>\nmain\n</script>\n",
        "<script background>\nbackground\n</script>\n",
        "</lynx>",
    ));
    assert_eq!(full.style, Some("\n.card { width: 100px; }\n"));
    assert_eq!(full.main_thread_script, "\nmain\n");
    assert_eq!(full.background_thread_script, Some("\nbackground\n"));

    let any_order = expect_success(concat!(
        "\u{feff}<lynx version=\"5.4.2\">\n",
        "<script background>background-code</script>\n",
        "<script main-thread>main-code</script>\n",
        "</lynx>",
    ));
    assert_eq!(any_order.style, None);
    assert_eq!(any_order.main_thread_script, "main-code");
    assert_eq!(any_order.background_thread_script, Some("background-code"));
}

// Kept inline so this integration test remains self-contained while still
// exercising a substantial, production-shaped card from lynx-stack.
const MARKUP_CARD_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE lynx>
<lynx version="5.4.2">
<style><![CDATA[
  /* A representative buildless card stylesheet. */
  .page {
    --accent: #2f6d54;
    width: 100%;
    font-size: calc(100vw / 24);
    background-color: #edf4ef;
  }

  .shell {
    width: 100%;
    display: flex;
    flex-direction: column;
    padding: 1rem;
  }

  .card {
    width: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    border-radius: 1.75rem;
    background: linear-gradient(145deg, #123f35 0%, #4c8665 100%);
  }

  .title {
    color: #f7f4e7;
    font-size: 1.5rem;
    font-weight: 700;
  }
]]></style>

<script main-thread="true"><![CDATA[
  const days = [
    { tab: 'Day 1', title: 'Lakeside', detail: 'Walk at sunrise.' },
    { tab: 'Day 2', title: 'Tea hills', detail: 'Climb the tea rows.' },
  ];

  const page = __CreatePage('0', 0);
  const pageId = __GetElementUniqueID(page);
  const engine = lynx.getEngine();
  let rendered = false;

  __SetClasses(page, 'page');

  function createView(className) {
    const node = __CreateView(pageId);
    __SetClasses(node, className);
    return node;
  }

  function renderPage() {
    if (rendered) return;
    rendered = true;
    const shell = createView('shell');
    __AppendElement(page, shell);
  }

  function onRenderPage() {
    renderPage();
  }

  engine.addEventListener('__RenderPage', onRenderPage);
]]></script>

<script background="true"><![CDATA[
  const mainThread = lynx.getCoreContext();

  function cleanupBackground() {
    mainThread.removeEventListener('__DestroyLifetime', cleanupBackground);
  }

  mainThread.addEventListener('__DestroyLifetime', cleanupBackground);
]]></script>
</lynx>
"#;
