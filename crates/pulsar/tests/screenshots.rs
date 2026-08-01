//! Golden screenshot comparison for **box rendering** — backgrounds, borders,
//! radii and nested flex geometry — over the full test pipeline:
//! inline-styled fragment → `dom` → `pulsar` → headless GPU.
//!
//! Text rendering has its own binary, `tests/text_screenshots.rs`; both write
//! into the same crate-level `tests/screenshots` golden tree. Capture,
//! comparison and golden management belong to `flashbulb` — these files only
//! supply the documents. Refresh with:
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p pulsar --test screenshots`.

mod common;
#[path = "support/html.rs"]
mod html;
#[path = "support/screenshot.rs"]
mod screenshot;

use flashbulb::ImageStore;

// lynx-stack's Playwright Chromium project uses the Pixel 5 viewport, and its
// tracked viewport screenshots are 393 × 727 CSS pixels.
const SCREEN_WIDTH: f32 = 393.0;
const SCREEN_HEIGHT: f32 = 727.0;
const FRAGMENT: &str = r#"
<div style="display: flex; width: 393px; height: 727px; padding: 12px; gap: 12px; box-sizing: border-box; background-color: #e5e7eb">
  <div style="display: flex; flex-direction: column; width: 78px; height: 104px; padding: 8px; gap: 8px; box-sizing: border-box; border: 4px solid #2563eb; background-color: white">
    <div style="display: flex; width: 54px; height: 28px; background-color: #14b8a6"></div>
    <div style="display: flex; width: 54px; height: 40px; gap: 6px">
      <div style="display: flex; width: 24px; height: 40px; background-color: #8b5cf6"></div>
      <div style="display: flex; width: 24px; height: 40px; background-color: #f59e0b"></div>
    </div>
  </div>
  <div style="display: flex; flex-direction: column; width: 78px; height: 104px; padding: 8px; gap: 8px; box-sizing: border-box; border: 4px solid #1f2937; background-color: white">
    <div style="display: flex; width: 54px; height: 28px; background-color: #ef4444"></div>
    <div style="display: flex; width: 54px; height: 40px; gap: 6px">
      <div style="display: flex; width: 24px; height: 40px; background-color: #0f766e"></div>
      <div style="display: flex; width: 24px; height: 40px; background-color: #8b5cf6"></div>
    </div>
  </div>
</div>
"#;

#[test]
fn inline_style_fragment_matches_reference() {
    let actual = screenshot::capture(
        "inline_style_fragment_matches_reference",
        FRAGMENT,
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
    );
    screenshot::assert_golden(&["inline-style"], &actual);
}

/// One synthesised source image: dimensions plus its RGBA8 pixels.
type Checker = (u32, u32, Vec<u8>);

const OBJECT_FIT_CSS: &str = "
    page { display: flex; position: relative; width: 393px; height: 727px;
           background-color: #e5e7eb; image-rendering: pixelated; }
    img { display: flex; position: absolute; width: 110px; height: 80px;
          background-color: #ffffff; }
    .c0 { left: 10px; } .c1 { left: 141px; } .c2 { left: 272px; }
    .r0 { top: 10px; } .r1 { top: 100px; } .r2 { top: 190px; }
    .r3 { top: 280px; } .r4 { top: 370px; }
    .fill { object-fit: fill; }
    .contain { object-fit: contain; }
    .cover { object-fit: cover; }
    .none { object-fit: none; }
    .down { object-fit: scale-down; }
    .top-right { object-position: 100% 0%; }
    .bottom-centre { object-position: 50% 100%; }
    .rounded { border-radius: 20px; }
    .bordered { border: 6px solid #1f2937; }
";

/// Every `object-fit` value, against a source both smaller and larger than its
/// box.
///
/// Two natural sizes is the point: with only a small source, `none` and
/// `scale-down` render identically and `scale-down`'s interesting branch — where
/// it equals `contain` — is never exercised at all.
///
/// `image-rendering: pixelated` throughout, so sampling is nearest and the
/// golden carries no bilinear filtering noise to absorb with tolerance.
#[test]
fn object_fit_matrix_matches_reference() {
    let mut doc = common::Doc::with_css_sized(OBJECT_FIT_CSS, SCREEN_WIDTH, SCREEN_HEIGHT);
    let mut images = ImageStore::new();

    // Smaller than the 110x80 box (so `none` and `scale-down` agree), and
    // larger than it (so `scale-down` collapses onto `contain`).
    let small = decode_checker(8, 8);
    let large = decode_checker(160, 120);

    let cells: &[(&str, &Checker)] = &[
        ("r0 c0 fill", &small),
        ("r0 c1 contain", &small),
        ("r0 c2 cover", &small),
        ("r1 c0 none", &small),
        ("r1 c1 down", &small),
        ("r1 c2 none top-right", &small),
        ("r2 c0 fill", &large),
        ("r2 c1 contain", &large),
        ("r2 c2 cover", &large),
        ("r3 c0 none", &large),
        ("r3 c1 down", &large),
        ("r3 c2 cover bottom-centre", &large),
        // Geometry interactions: the content-box radii clip and the
        // border/padding inset that shrinks the content box under the image.
        ("r4 c0 cover rounded", &large),
        ("r4 c1 contain bordered", &large),
        ("r4 c2 fill rounded bordered", &small),
    ];

    let root = doc.root;
    for (class, (width, height, rgba)) in cells {
        let node = doc.el_tag(root, "img", class);
        #[allow(clippy::cast_precision_loss)]
        doc.dom.set_natural_size(
            node,
            dom::layout::NaturalSize::from_size(dom::layout::Size::new(
                *width as f32,
                *height as f32,
            )),
        );
        images.insert_node(node, image_data(*width, *height, rgba.clone()));
    }

    let actual = screenshot::capture_document_with_images(
        "object_fit_matrix_matches_reference",
        &mut doc.dom,
        &images,
    );
    screenshot::assert_golden(&["replaced-object-fit"], &actual);
}

/// A four-quadrant checker with a diagonal marker, so a flip, a rotation or an
/// axis swap is visible rather than merely plausible.
fn decode_checker(width: u32, height: u32) -> Checker {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let left = x < width / 2;
            let top = y < height / 2;
            // The marker stripe runs top-left to bottom-right.
            let on_diagonal = (x * height).abs_diff(y * width) < width.max(height);
            let pixel = if on_diagonal {
                [0, 0, 0, 255]
            } else {
                match (left, top) {
                    (true, true) => [220, 38, 38, 255],
                    (false, true) => [22, 163, 74, 255],
                    (true, false) => [37, 99, 235, 255],
                    (false, false) => [250, 204, 21, 255],
                }
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    (width, height, rgba)
}

fn image_data(width: u32, height: u32, rgba: Vec<u8>) -> flashbulb::vello::peniko::ImageData {
    use flashbulb::vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
    ImageData {
        data: Blob::from(rgba),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    }
}
