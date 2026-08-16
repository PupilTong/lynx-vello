//! Exercises the interactive headless CLI end to end.

use std::io::Write;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[test]
fn a_file_bundle_supports_debugger_style_screenshots() {
    let gpu = flashbulb::headless("a_file_bundle_supports_debugger_style_screenshots");
    drop(gpu);

    let root = std::env::temp_dir().join(format!(
        "bobcat-cli-e2e-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let bundle_path = root.join("minimal.web.bundle");
    let screenshot_path = root.join("capture.png");
    std::fs::write(&bundle_path, minimal_bundle()).unwrap();
    let input = url::Url::from_file_path(&bundle_path)
        .expect("absolute temporary path")
        .to_string();

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_bobcat"))
        .args([
            "-i",
            &input,
            "--headless",
            "--viewport",
            "32x24",
            "--vsync",
            "75",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start interactive bobcat");
    write!(
        child.stdin.take().expect("piped stdin"),
        "screenshot {}\npause\nset vsync 30\nshow vsync\nframe\nquit\n",
        screenshot_path.display()
    )
    .unwrap();
    let output = child
        .wait_with_output()
        .expect("wait for interactive bobcat");
    assert!(
        output.status.success(),
        "interactive bobcat failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "Frame clock paused.",
        "Headless vsync is now 30 Hz.",
        "Headless vsync is 30 Hz.",
        "Rendered one frame.",
        "Saved screenshot",
    ] {
        assert!(
            stdout.contains(expected),
            "missing `{expected}` in:\n{stdout}"
        );
    }
    let image = flashbulb::Image::read_png(&screenshot_path).unwrap();
    assert_eq!((image.width(), image.height()), (32, 24));
    assert!(
        image
            .pixels()
            .chunks_exact(4)
            .any(|pixel| pixel != [255, 255, 255, 255]),
        "the screenshot-first command must wait for the script's non-white frame"
    );

    std::fs::remove_dir_all(root).unwrap();
}

/// The whole ingestion path through the real binary: a bundle whose only
/// styling lives in its rkyv `StyleInfo` section must render the same pixels
/// the inline-style bundle does.
#[test]
fn author_css_from_the_style_info_section_renders() {
    let gpu = flashbulb::headless("author_css_from_the_style_info_section_renders");
    drop(gpu);

    let root = std::env::temp_dir().join(format!(
        "bobcat-cli-e2e-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let bundle_path = root.join("styled.web.bundle");
    let screenshot_path = root.join("styled.png");
    std::fs::write(&bundle_path, styled_bundle()).unwrap();
    let input = url::Url::from_file_path(&bundle_path)
        .expect("absolute temporary path")
        .to_string();

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_bobcat"))
        .args(["-i", &input, "--headless", "--viewport", "32x24"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start interactive bobcat");
    write!(
        child.stdin.take().expect("piped stdin"),
        "screenshot {}\nquit\n",
        screenshot_path.display()
    )
    .unwrap();
    let output = child
        .wait_with_output()
        .expect("wait for interactive bobcat");
    assert!(
        output.status.success(),
        "interactive bobcat failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let image = flashbulb::Image::read_png(&screenshot_path).unwrap();
    assert_eq!((image.width(), image.height()), (32, 24));
    let red = image
        .pixels()
        .chunks_exact(4)
        .filter(|pixel| *pixel == [255, 0, 0, 255])
        .count();
    assert_eq!(
        red,
        16 * 12,
        "the 16x12 box sized and coloured by the bundle's author CSS must be painted"
    );

    std::fs::remove_dir_all(root).unwrap();
}

/// A bundle whose `.styled` class comes only from its `StyleInfo` section.
fn styled_bundle() -> Vec<u8> {
    use lynx_template_decoder::style_info::{
        CssProperty, CssPropertyId, DeclarationBlock, ParsedDeclaration, Rule, RuleKind,
        RulePrelude, Selector, SimpleSelector, SimpleSelectorKind, StyleInfo, StyleSheet,
        ValueToken, token_types,
    };

    let declaration = |id: CssPropertyId, token_type: u8, value: &str| ParsedDeclaration {
        property: CssProperty {
            id,
            unknown_name: None,
        },
        value_tokens: vec![ValueToken {
            token_type,
            value: value.to_owned(),
        }],
        is_important: false,
    };
    let style_info = StyleInfo {
        css_id_to_style_sheet: std::collections::HashMap::from([(
            0,
            StyleSheet {
                imports: vec![],
                rules: vec![Rule {
                    kind: RuleKind::Style,
                    prelude: RulePrelude {
                        selectors: vec![Selector {
                            components: vec![SimpleSelector {
                                kind: SimpleSelectorKind::Class,
                                value: "styled".to_owned(),
                            }],
                        }],
                    },
                    declaration_block: DeclarationBlock {
                        declarations: vec![
                            declaration(CssPropertyId::Width, token_types::DIMENSION_TOKEN, "16px"),
                            declaration(
                                CssPropertyId::Height,
                                token_types::DIMENSION_TOKEN,
                                "12px",
                            ),
                            declaration(
                                CssPropertyId::BackgroundColor,
                                token_types::HASH_TOKEN,
                                "#ff0000",
                            ),
                        ],
                    },
                    children: vec![],
                }],
            },
        )]),
        style_text_size_hint: 0,
    };
    let style_info = rkyv::to_bytes::<_, 1024>(&style_info).expect("serialize StyleInfo");

    let script = r"
        globalThis.renderPage = function renderPage() {
            const page = __CreatePage('card', 0);
            const view = __CreateView(0);
            __SetClasses(view, 'styled');
            __AppendElement(page, view);
        };
    ";
    let mut bytes = bundle_header();
    push_lepus_section(&mut bytes, script);
    push_section(
        &mut bytes,
        lynx_template_decoder::SectionLabel::StyleInfo as u32,
        &style_info,
    );
    bytes
}

fn bundle_header() -> Vec<u8> {
    let config = r#"{
        "defaultDisplayLinear": "true",
        "defaultOverflowVisible": "true",
        "enableCSSSelector": "true"
    }"#;
    let mut bytes = Vec::new();
    push_u32(&mut bytes, lynx_template_decoder::MAGIC_0);
    push_u32(&mut bytes, lynx_template_decoder::MAGIC_1);
    push_u32(&mut bytes, 1);
    let config: Vec<u8> = config.encode_utf16().flat_map(u16::to_le_bytes).collect();
    push_section(
        &mut bytes,
        lynx_template_decoder::SectionLabel::Configurations as u32,
        &config,
    );
    bytes
}

fn push_lepus_section(bytes: &mut Vec<u8>, script: &str) {
    let mut lepus = Vec::new();
    push_u32(&mut lepus, 1);
    push_string(&mut lepus, "root");
    push_string(&mut lepus, script);
    push_section(
        bytes,
        lynx_template_decoder::SectionLabel::LepusCode as u32,
        &lepus,
    );
}

fn minimal_bundle() -> Vec<u8> {
    let config = r#"{
        "defaultDisplayLinear": "true",
        "defaultOverflowVisible": "true",
        "enableCSSSelector": "true"
    }"#;
    let script = r"
        const deadline = Date.now() + 200;
        while (Date.now() < deadline) {}
        globalThis.renderPage = function renderPage() {
            const page = __CreatePage('card', 0);
            const view = __CreateView(0);
            __SetInlineStyles(
                view,
                'width:32px;height:24px;background-color:#ff0000'
            );
            __AppendElement(page, view);
        };
    ";

    let mut bytes = Vec::new();
    push_u32(&mut bytes, lynx_template_decoder::MAGIC_0);
    push_u32(&mut bytes, lynx_template_decoder::MAGIC_1);
    push_u32(&mut bytes, 1);

    let config: Vec<u8> = config.encode_utf16().flat_map(u16::to_le_bytes).collect();
    push_section(
        &mut bytes,
        lynx_template_decoder::SectionLabel::Configurations as u32,
        &config,
    );

    let mut lepus = Vec::new();
    push_u32(&mut lepus, 1);
    push_string(&mut lepus, "root");
    push_string(&mut lepus, script);
    push_section(
        &mut bytes,
        lynx_template_decoder::SectionLabel::LepusCode as u32,
        &lepus,
    );
    bytes
}

fn push_section(bytes: &mut Vec<u8>, label: u32, content: &[u8]) {
    push_u32(bytes, label);
    push_u32(
        bytes,
        u32::try_from(content.len()).expect("tiny test section"),
    );
    bytes.extend_from_slice(content);
}

fn push_string(bytes: &mut Vec<u8>, value: &str) {
    push_u32(bytes, u32::try_from(value.len()).expect("tiny test string"));
    bytes.extend_from_slice(value.as_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
