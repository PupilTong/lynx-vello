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
