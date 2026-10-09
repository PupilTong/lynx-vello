//! Golden screenshot comparison for **box rendering** — backgrounds, borders,
//! radii and nested flex geometry — over the full test pipeline:
//! inline-styled fragment → `dom` → headless GPU.
//!
//! Per-topic siblings own the rest: `tests/text_screenshots.rs` and
//! `tests/web_text_screenshots.rs` for text, `tests/blur_screenshots.rs` for
//! `filter: blur()`. All of them write into the same crate-level
//! `tests/screenshots` golden tree, each under its own subdirectory. Capture,
//! comparison and golden management belong to `flashbulb` — these files only
//! supply the documents. Refresh with:
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p dom --test screenshots`.

#[path = "support/html.rs"]
mod html;
mod paint_common;
#[path = "support/screenshot.rs"]
mod screenshot;

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

#[test]
fn object_fit_matrix_matches_reference() {
    let mut doc = paint_common::Doc::with_css_sized(OBJECT_FIT_CSS, SCREEN_WIDTH, SCREEN_HEIGHT);
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
        ("r4 c0 cover rounded", &large),
        ("r4 c1 contain bordered", &large),
        ("r4 c2 fill rounded bordered", &small),
    ];

    let images = flashbulb::TestImages::new();
    let root = doc.root;
    // No `set_natural_size` anywhere: every box below is sized purely by the
    // dimensions the store reports when its load completes. That is the whole
    // point of the golden — it fails if the intrinsic size does not travel
    // from the store through the registry into layout.
    for (class, (width, height, rgba)) in cells {
        let node = doc.el_tag(root, "img", class);
        let source = format!("app:///{}.png", node.to_bits());
        images.insert_rgba8(&source, *width, *height, rgba.clone());
        doc.dom
            .set_image_source(node, dom::ImageRole::Source, Some(&source));
    }

    // The natural size now arrives from the store's own load report, through
    // the same request/report loop the painter runs.
    flashbulb::render_with_images(&mut doc.dom, &images);
    let actual = screenshot::capture_prebuilt_document(
        "object_fit_matrix_matches_reference",
        &mut doc.dom,
        &images,
    );
    screenshot::assert_golden(&["replaced-object-fit"], &actual);
}

fn decode_checker(width: u32, height: u32) -> Checker {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let left = x < width / 2;
            let top = y < height / 2;
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

/// CSS Grid Level 3 `display: grid-lanes`: three `minmax(0, 1fr)` lanes, a
/// 15px gutter in both axes, `flow-tolerance: 0` so every item lands in the
/// strictly shortest lane, and one `grid-column: 1 / -1` item that spans them
/// all and leaves them level behind it.
///
/// Solid fills only: the subject is where the boxes land, and a colour per
/// item is what makes a lane, a gutter and the full-span reset legible.
const LANES_FRAGMENT: &str = r#"
<div style="display: grid-lanes; width: 300px; height: 400px; gap: 15px; flow-tolerance: 0; grid-template-columns: repeat(3, minmax(0, 1fr)); background-color: #e5e7eb">
  <div style="height: 60px; background-color: #2563eb"></div>
  <div style="height: 90px; background-color: #14b8a6"></div>
  <div style="height: 40px; background-color: #f59e0b"></div>
  <div style="height: 70px; background-color: #8b5cf6"></div>
  <div style="grid-column: 1 / -1; height: 50px; background-color: #1f2937"></div>
  <div style="height: 80px; background-color: #ef4444"></div>
  <div style="height: 55px; background-color: #0f766e"></div>
  <div style="height: 65px; background-color: #22c55e"></div>
</div>
"#;

#[test]
fn grid_lanes_waterfall_matches_reference() {
    let actual = screenshot::capture(
        "grid_lanes_waterfall_matches_reference",
        LANES_FRAGMENT,
        300.0,
        400.0,
    );
    screenshot::assert_golden(&["grid-lanes-waterfall"], &actual);
}

/// A scroller holding two `z-index` columns beside a `z-index: 2` bar that
/// is its DOM sibling. The scroller's own `z-index` is the only difference
/// between the two goldens below; both scroll it 40px, so each column shows
/// its scrolled position and the scrollport's cut.
const SCROLL_STACKING_CSS: &str = "
    page { display: flex; position: relative; width: 240px; height: 200px;
           background-color: #e5e7eb; }
    .scroller { display: flex; position: absolute; left: 20px; top: 20px;
                width: 200px; height: 160px; padding: 10px; gap: 10px;
                box-sizing: border-box; overflow: scroll;
                background-color: #cbd5e1; }
    .stacked { z-index: 0; }
    .column { display: flex; flex-shrink: 0; position: relative;
              width: 85px; height: 220px; }
    .a { z-index: 1; background-color: #38bdf8; }
    .b { z-index: 3; background-color: #22c55e; }
    .bar { display: flex; position: absolute; left: 0; top: 80px;
           width: 240px; height: 40px; z-index: 2; background-color: #5b21b6; }
";

fn capture_scroll_stacking(test: &str, scroller_class: &str) -> flashbulb::Image {
    let mut doc = paint_common::Doc::with_css_sized(SCROLL_STACKING_CSS, 240.0, 200.0);
    let root = doc.root;
    let scroller = doc.el(root, scroller_class);
    doc.el(scroller, "column a");
    doc.el(scroller, "column b");
    doc.el(root, "bar");
    doc.dom.layout();
    let applied = doc.dom.scroll_to(scroller, dom::Vector2D::new(0.0, 40.0));
    assert!(
        (applied.y - 40.0).abs() < f32::EPSILON,
        "the columns must overflow the scrollport"
    );
    screenshot::capture_prebuilt_document(test, &mut doc.dom, &dom::NoImages)
}

/// `z-index: auto`: the scroller is no stacking context, so its columns sort
/// against the bar in the page's context — A (1) under the bar, B (3) over it.
#[test]
fn scroll_container_without_z_index_interleaves_its_content_with_a_sibling() {
    let actual = capture_scroll_stacking(
        "scroll_container_without_z_index_interleaves_its_content_with_a_sibling",
        "scroller",
    );
    screenshot::assert_golden(&["scroll-stacking", "z-index-auto"], &actual);
}

/// `z-index: 0`: the scroller is a stacking context at level 0, so both
/// columns stay inside it and the bar (2) covers them both.
#[test]
fn scroll_container_with_z_index_zero_keeps_its_content_below_a_sibling() {
    let actual = capture_scroll_stacking(
        "scroll_container_with_z_index_zero_keeps_its_content_below_a_sibling",
        "scroller stacked",
    );
    screenshot::assert_golden(&["scroll-stacking", "z-index-zero"], &actual);
}

/// A modal top-layer element and its default `::backdrop` over a page whose
/// `z-index: 100` box would cover anything in the root stacking context
/// (web-elements' `basic-z-index.html`: the top layer beats any `z-index`).
/// The dialog sits inside a faded, clipped, translated ancestor, none of
/// which reaches it; the backdrop tints the whole viewport, the
/// `z-index: 100` box included, and the dialog paints over it untinted.
const TOP_LAYER_CSS: &str = "
    page { display: flex; position: relative; width: 240px; height: 200px;
           background-color: #e5e7eb; }
    .high { display: flex; position: absolute; left: 10px; top: 10px;
            width: 220px; height: 60px; z-index: 100; background-color: #ef4444; }
    .fx { display: flex; position: absolute; left: 150px; top: 120px;
          width: 20px; height: 20px; opacity: 0.5; overflow: clip;
          transform: translate(30px, 30px); background-color: #2563eb; }
    dialog { display: flex; position: fixed; inset: 0; margin: auto;
             width: 120px; height: 80px; border: 3px solid #111827;
             background-color: #ffffff; }
    .inner { display: flex; width: 40px; height: 40px; margin: 10px;
             background-color: #14b8a6; }
";

/// What bobcat-core's UA sheet gives every `::backdrop` (HTML's
/// `dialog::backdrop` tint included).
const TOP_LAYER_UA: &str = "
    ::backdrop { display: flex; position: fixed; inset: 0; }
    dialog::backdrop { background-color: rgba(0, 0, 0, 0.1); }
";

#[test]
fn a_modal_top_layer_element_and_its_backdrop_paint_over_any_z_index() {
    let mut doc = paint_common::Doc::with_css_sized(TOP_LAYER_CSS, 240.0, 200.0);
    doc.dom
        .add_stylesheet(TOP_LAYER_UA, dom::StylesheetOrigin::UserAgent);
    let root = doc.root;
    let fx = doc.el(root, "fx");
    let dialog = doc.el_tag(fx, "dialog", "");
    doc.el(dialog, "inner");
    doc.el(root, "high");
    doc.dom.add_to_top_layer(dialog, true);
    let actual = screenshot::capture_prebuilt_document(
        "a_modal_top_layer_element_and_its_backdrop_paint_over_any_z_index",
        &mut doc.dom,
        &dom::NoImages,
    );
    screenshot::assert_golden(&["top-layer", "modal-backdrop"], &actual);
}

/// The Explorer border/background/shadow example: the top edge must continue
/// around its lone rounded corner, and the shadow must follow that same shape.
/// Styles come from `@lynx-example/css@74e8136`, `border_background_shadow`.
/// The reference was captured with Chrome 154.0.8037.99 at device scale 1;
/// fixed positioning replaces viewport-dependent centering for this comparison.
#[test]
fn rounded_box_showcase_matches_browser() {
    let mut doc = paint_common::Doc::with_css_sized("", 220.0, 220.0);
    doc.dom
        .register_fonts(dom::FontBlob::from_static(screenshot::ROBOTO));
    doc.dom.set_inline_style(
        doc.root,
        "display:flex;position:relative;width:220px;height:220px;background:#fff",
    );
    let subject = doc.el(doc.root, "");
    doc.dom.set_inline_style(subject,
        "display:flex;position:absolute;left:32px;top:32px;width:150px;height:150px;box-sizing:border-box;background:linear-gradient(to right,rgb(255,53,26),rgb(0,235,235));border-radius:0 50% 0 0;box-shadow:3px 5px 5px black;border-left:2px rgb(0,235,235) dotted;border-top:2px rgb(255,53,26) dashed");
    let actual = screenshot::capture_prebuilt_document(
        "rounded_box_showcase_matches_browser",
        &mut doc.dom,
        &dom::NoImages,
    );
    let references = flashbulb::Screenshots::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/screenshots/rounded-box-browser"
    ))
    .with_artifacts_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/artifacts"))
    // CSS permits different dash/dot phasing. The two geometric assertions
    // below are independent of that allowance and of the stored reference.
    .with_options(flashbulb::CompareOptions::default().with_max_diff_pixels(150));
    assert!(
        !references.is_updating(),
        "recapture this reference with Chrome; never accept native output"
    );
    assert!(
        references.path(&["showcase"]).exists(),
        "the browser reference must be committed"
    );
    references.assert_matches(&["showcase"], &actual);
    let comparison = flashbulb::compare(
        &flashbulb::Image::read_png(references.path(&["showcase"])).unwrap(),
        &actual,
        references.options(),
    );
    eprintln!(
        "showcase browser comparison: {} differing pixels",
        comparison.diff_pixels
    );

    let pixel = |x: usize, y: usize| &actual.pixels()[(y * 220 + x) * 4..][..3];
    assert!(
        (170..178).all(|x| (43..49).all(|y| pixel(x, y).iter().all(|&v| v > 245))),
        "the removed corner must not contain a rectangular shadow"
    );
    let red_arc = (120..180)
        .flat_map(|x| (38..104).map(move |y| (x, y)))
        .filter(|&(x, y)| {
            let rgb = pixel(x, y);
            rgb[0] > 220 && rgb[1] < 90 && rgb[2] < 65
        })
        .count();
    assert!(
        red_arc > 8,
        "the red top border must extend down the rounded corner: {red_arc} pixels"
    );
}

/// Separate controls for elliptical/mixed radii, inward and outward blur,
/// spread, zero blur, asymmetric border widths, and nested filter composition.
#[test]
fn rounded_box_matrix_matches_reference() {
    let subjects = [
        "border-radius:0 60px 12px 24px;box-shadow:4px 6px 8px 2px #111827;border-top:3px dashed #ef4444;border-left:2px dotted #06b6d4",
        "border-radius:60px 20px 50px 5px / 20px 50px 15px 40px;box-shadow:4px 6px 8px #111827;border:3px solid #ef4444;border-right:9px solid #2563eb",
        "border-radius:0 60px 12px 24px;box-shadow:inset 4px 6px 12px 2px #111827;border:3px solid #06b6d4",
        "border-radius:0 60px 12px 24px;box-shadow:4px 6px 0px -2px #111827;border:6px double #ef4444;border-left:12px double #2563eb",
        "border-radius:24px;box-shadow:4px 6px 8px #111827;border:4px dashed #ef4444;border-bottom:4px dotted #2563eb",
        "border-radius:0 60px 12px 24px;box-shadow:4px 6px 8px #111827,inset 3px 4px 6px #ef4444;filter:blur(1px);opacity:0.7;border-top:3px dashed #ef4444",
    ];
    let mut doc = paint_common::Doc::with_css_sized("", 480.0, 320.0);
    doc.dom
        .register_fonts(dom::FontBlob::from_static(screenshot::ROBOTO));
    doc.dom.set_inline_style(
        doc.root,
        "display:flex;position:relative;width:480px;height:320px;background:#fff",
    );
    for (index, style) in subjects.iter().enumerate() {
        let subject = doc.el(doc.root, "");
        doc.dom.set_inline_style(subject, &format!(
            "display:flex;position:absolute;left:{}px;top:{}px;width:120px;height:120px;box-sizing:border-box;background:#d1fae5;{style}",
            20 + index % 3 * 160, 20 + index / 3 * 160));
    }
    let actual = screenshot::capture_prebuilt_document(
        "rounded_box_matrix_matches_reference",
        &mut doc.dom,
        &dom::NoImages,
    );
    screenshot::assert_golden(&["rounded-box-matrix"], &actual);
}
