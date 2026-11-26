//! Golden screenshots of `filter: blur()` (pages 1-4) and `backdrop-filter`
//! (pages 5-6).
//!
//! These are regression goldens of *our own* output, not browser references —
//! the nine Chromium-referenced `filter-04x` cases in `tests/css_atlas.rs`
//! stay `Skip`, because promoting them is an audited disposition that needs a
//! Chromium comparison this workspace cannot run.
//!
//! The offscreen path is only observable in pixels: the compose program's
//! bracket and backdrop ops encode nothing without a baked texture, so every
//! property below — that the bake happens, that it is premultiplied, that a
//! filter's 3σ margin survives a cell or a page edge, that a backdrop's crop
//! is its own border box and its edges mirror, that σ scales with the
//! element's transform, that a chain splits where the list says — is
//! invisible to an encoding comparison and to a numeric probe that is not
//! already looking at the right pixel. `tests/gpu_pixels.rs` asserts the
//! analytic half (monotone profiles, symmetry, ink bounds, crops); this
//! binary is the half a reviewer *looks at*.
//!
//! Every page carries unfiltered controls beside the filtered cells on
//! purpose: a golden of a blur alone can only regress into "different", while
//! one beside its own reference regresses into "wrong".
//!
//! Every fixture renders **vendored Roboto**, never a host font; see
//! `support/screenshot.rs` for why that is not optional. Refresh with:
//! `PRIME_RADIANT_UPDATE_SNAPSHOTS=1 cargo test -p terminus --test blur_screenshots`.

#[path = "support/html.rs"]
mod html;
mod paint_common;
#[path = "support/screenshot.rs"]
mod screenshot;

const SCREEN_WIDTH: f32 = 393.0;
const SCREEN_HEIGHT: f32 = 727.0;

const LEFT: [u32; 3] = [12, 139, 266];
const TOP: [u32; 5] = [12, 152, 292, 432, 572];

const SUBJECT: &str =
    "display: flex; position: absolute; left: 26px; top: 32px; width: 64px; height: 64px";

fn cell(row: usize, col: usize, inner: &str) -> String {
    format!(
        r#"<div style="display: flex; position: absolute; left: {left}px; top: {top}px; width: 115px; height: 128px; background-color: #ffffff">{inner}</div>"#,
        left = LEFT[col],
        top = TOP[row],
    )
}

fn subject(extra: &str) -> String {
    format!(r#"<div style="{SUBJECT}; {extra}"></div>"#)
}

fn page(body: &str) -> String {
    format!(
        r#"<div style="display: flex; position: relative; width: 393px; height: 727px; background-color: #e5e7eb; font-family: Roboto; color: #111827">{body}</div>"#
    )
}

#[test]
fn blur_shapes_matrix_matches_reference() {
    let body = [
        cell(0, 0, &subject("background-color: #0d9488; filter: blur(4px)")),
        cell(
            0,
            1,
            &subject(
                "background-color: #fef3c7; border: 6px solid #b45309; \
                 border-radius: 16px; box-sizing: border-box; filter: blur(4px)",
            ),
        ),
        cell(
            0,
            2,
            &subject(
                "background-image: linear-gradient(135deg, #dc2626, #2563eb); \
                 filter: blur(6px)",
            ),
        ),
        cell(1, 0, &subject("background-color: #0d9488; filter: blur(16px)")),
        cell(1, 1, &subject("background-color: #0d9488; filter: blur(1px)")),
        cell(1, 2, &subject("background-color: #0d9488")),
        cell(
            2,
            0,
            &subject("background-color: #0d9488; filter: blur(3px); opacity: 0.5"),
        ),
        cell(
            2,
            1,
            &subject(
                "background-color: #0d9488; filter: blur(3px); clip-path: circle(40%)",
            ),
        ),
        cell(
            2,
            2,
            &subject("background-color: #0d9488; filter: blur(3px); border-radius: 50%"),
        ),
        cell(
            3,
            0,
            r#"<div style="display: flex; position: absolute; left: 26px; top: 32px; width: 64px; height: 64px; overflow: hidden; background-color: #f1f5f9">
  <div style="display: flex; position: absolute; left: 8px; top: 8px; width: 48px; height: 48px; background-color: #0d9488; filter: blur(3px)"></div>
</div>"#,
        ),
        cell(
            3,
            1,
            &subject(
                "background-color: #0d9488; filter: blur(3px); \
                 mask-image: linear-gradient(to right, #000000, transparent)",
            ),
        ),
        cell(
            3,
            2,
            &format!(
                r#"<div style="{SUBJECT}; filter: blur(3px)">
  <div style="display: flex; position: absolute; left: 12px; top: 12px; width: 40px; height: 40px; background-color: #0d9488; filter: blur(3px)"></div>
</div>"#
            ),
        ),
        cell(
            4,
            0,
            &subject("border: 6px solid #b45309; box-sizing: border-box; filter: blur(3px)"),
        ),
        cell(4, 1, &subject("background-color: #ffffff; filter: blur(6px)")),
        cell(
            4,
            2,
            &subject("background-color: #0d9488; box-shadow: 0px 5px 9px #475569; filter: blur(3px)"),
        ),
    ]
    .concat();

    let actual = screenshot::capture(
        "blur_shapes_matrix_matches_reference",
        &page(&body),
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
    );
    screenshot::assert_golden(&["filter-blur", "shapes"], &actual);
}

fn two_colour(extra: &str) -> String {
    format!(
        r#"<div style="{SUBJECT}; background-color: #dc2626; {extra}">
  <div style="display: flex; position: absolute; left: 16px; top: 16px; width: 32px; height: 32px; background-color: #1d4ed8"></div>
</div>"#
    )
}

fn line(size: u32, extra: &str, text: &str) -> String {
    format!(
        r#"<div class="text-block" style="display: flex; position: absolute; left: 8px; top: 48px; width: 99px; font-size: {size}px; {extra}">{text}</div>"#
    )
}

#[test]
fn blur_text_and_chains_matrix_matches_reference() {
    let body = [
        cell(
            0,
            0,
            &format!(
                r#"<div style="display: flex; position: absolute; left: 8px; top: 40px; width: 99px; height: 48px; background-color: #fde68a; filter: blur(2px)">
  {}
</div>"#,
                r#"<div class="text-block" style="display: flex; position: absolute; left: 6px; top: 12px; width: 88px; font-size: 15px">Blurred</div>"#
            ),
        ),
        cell(
            0,
            1,
            &line(
                17,
                "filter: blur(2px); text-shadow: 2px 2px #93c5fd",
                "Shadow",
            ),
        ),
        cell(0, 2, &line(17, "", "Crisp")),
        cell(
            1,
            0,
            &format!(
                r#"<div style="{SUBJECT}; background-color: #fecaca; filter: blur(3px)">
  <div style="display: flex; position: absolute; left: 6px; top: 6px; width: 24px; height: 24px; background-color: #1d4ed8"></div>
  <div class="text-block" style="display: flex; position: absolute; left: 4px; top: 36px; width: 58px; font-size: 14px">Ab c</div>
</div>"#
            ),
        ),
        cell(1, 1, &two_colour("filter: grayscale(1) blur(3px)")),
        cell(1, 2, &two_colour("filter: blur(3px) grayscale(1)")),
        cell(2, 0, &two_colour("")),
        cell(2, 1, &two_colour("filter: blur(3px) brightness(0.6)")),
        cell(2, 2, &two_colour("filter: brightness(0.6)")),
        cell(
            3,
            0,
            &subject("background-color: #0d9488; filter: blur(3px) blur(4px)"),
        ),
        cell(3, 1, &subject("background-color: #0d9488; filter: blur(5px)")),
        cell(3, 2, &two_colour("filter: grayscale(1)")),
        format!(
            r#"<div style="display: flex; position: absolute; left: 12px; top: {top}px; width: 369px; height: 128px; background-color: #ffffff">
  <div class="text-block" style="display: flex; position: absolute; left: 14px; top: 24px; width: 341px; font-size: 24px; filter: blur(2px)">Sphinx of black quartz</div>
  <div class="text-block" style="display: flex; position: absolute; left: 14px; top: 72px; width: 341px; font-size: 24px">Sphinx of black quartz</div>
</div>"#,
            top = TOP[4],
        ),
    ]
    .concat();

    let actual = screenshot::capture(
        "blur_text_and_chains_matrix_matches_reference",
        &page(&body),
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
    );
    screenshot::assert_golden(&["filter-blur", "text-and-chains"], &actual);
}

#[test]
fn blur_transforms_matrix_matches_reference() {
    const FRAGMENT: &str = r#"
<div style="display: flex; position: relative; width: 393px; height: 727px; background-color: #e5e7eb">
  <div style="display: flex; position: absolute; left: 12px; top: 12px; width: 369px; height: 116px; background-color: #ffffff">
    <div style="display: flex; position: absolute; left: 30px; top: 26px; width: 64px; height: 64px; background-color: #0d9488; transform: rotate(30deg); filter: blur(4px)"></div>
    <div style="display: flex; position: absolute; left: 240px; top: 26px; width: 64px; height: 64px; background-color: #0d9488; transform: rotate(30deg)"></div>
  </div>
  <div style="display: flex; position: absolute; left: 12px; top: 140px; width: 369px; height: 116px; background-color: #ffffff">
    <div style="display: flex; position: absolute; left: 46px; top: 42px; width: 32px; height: 32px; background-color: #0d9488; transform: scale(2); filter: blur(2px)"></div>
    <div style="display: flex; position: absolute; left: 256px; top: 42px; width: 32px; height: 32px; background-color: #0d9488; transform: scale(2)"></div>
  </div>
  <div style="display: flex; position: absolute; left: 12px; top: 268px; width: 369px; height: 116px; background-color: #ffffff">
    <div style="display: flex; position: absolute; left: 58px; top: 34px; width: 48px; height: 48px; background-color: #0d9488; transform: scaleX(3); filter: blur(3px)"></div>
    <div style="display: flex; position: absolute; left: 238px; top: 34px; width: 48px; height: 48px; background-color: #0d9488; transform: scaleX(3)"></div>
  </div>
  <div style="display: flex; position: absolute; left: 12px; top: 396px; width: 369px; height: 116px; background-color: #ffffff">
    <div style="display: flex; position: absolute; left: 30px; top: 26px; width: 64px; height: 64px; background-color: #0d9488; transform: scale(0.5); filter: blur(4px)"></div>
    <div style="display: flex; position: absolute; left: 240px; top: 26px; width: 64px; height: 64px; background-color: #0d9488; transform: scale(0.5)"></div>
  </div>
  <div style="display: flex; position: absolute; left: 12px; top: 524px; width: 369px; height: 116px; background-color: #ffffff">
    <div style="display: flex; position: absolute; left: 46px; top: 42px; width: 32px; height: 32px; background-color: #0d9488; transform: scale(2); filter: blur(2px)"></div>
    <div style="display: flex; position: absolute; left: 240px; top: 26px; width: 64px; height: 64px; background-color: #0d9488; filter: blur(4px)"></div>
  </div>
</div>
"#;

    let actual = screenshot::capture(
        "blur_transforms_matrix_matches_reference",
        FRAGMENT,
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
    );
    screenshot::assert_golden(&["filter-blur", "transforms"], &actual);
}

#[test]
fn blur_edges_and_replaced_content_matches_reference() {
    const CSS: &str = "
        page { display: flex; position: relative; width: 393px; height: 727px;
               background-color: #e5e7eb; }
        .band { display: flex; position: absolute; left: 12px; top: 100px;
                width: 369px; height: 128px; background-color: #ffffff; }
        img { display: flex; position: absolute; top: 24px;
              width: 110px; height: 80px; object-fit: contain;
              image-rendering: pixelated; }
        .left { left: 30px; } .right { left: 200px; }
        .b3 { filter: blur(3px); }
        .edge { display: flex; position: absolute; width: 96px; height: 96px;
                background-color: #7c3aed; filter: blur(6px); }
        .edge-top { left: 148px; top: -40px; }
        .edge-left { left: -40px; top: 300px; }
        .edge-right { left: 337px; top: 300px; }
        .edge-bottom { left: 148px; top: 671px; }
    ";

    let mut doc = paint_common::Doc::with_css_sized(CSS, SCREEN_WIDTH, SCREEN_HEIGHT);
    let root = doc.root;

    let images = prime_radiant::TestImages::new();
    let band = doc.el(root, "band");
    let mut presented = Vec::new();
    for class in ["left b3", "right"] {
        let node = doc.el_tag(band, "img", class);
        let source = format!("app:///{}.png", node.to_bits());
        let (width, height, rgba) = checker(8, 8);
        images.insert_rgba8(&source, width, height, rgba);
        presented.push((node, source));
    }

    for class in [
        "edge edge-top",
        "edge edge-left",
        "edge edge-right",
        "edge edge-bottom",
    ] {
        doc.el(root, class);
    }

    let presented: Vec<_> = presented
        .iter()
        .map(|(node, source)| (*node, source.as_str()))
        .collect();
    prime_radiant::render_presenting(&mut doc.dom, &images, &presented);
    let actual = screenshot::capture_prebuilt_document(
        "blur_edges_and_replaced_content_matches_reference",
        &mut doc.dom,
        &images,
    );
    screenshot::assert_golden(&["filter-blur", "edges-and-replaced"], &actual);
}

fn checker(width: u32, height: u32) -> (u32, u32, Vec<u8>) {
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

fn board(row: usize, col: usize, inner: &str) -> String {
    format!(
        r#"<div style="display: flex; position: absolute; left: {left}px; top: {top}px; width: 115px; height: 128px; background-color: #ffffff">
  <div style="display: flex; position: absolute; left: 0px; top: 0px; width: 57px; height: 128px; background-color: #dc2626"></div>
  <div style="display: flex; position: absolute; left: 0px; top: 78px; width: 115px; height: 18px; background-color: #f59e0b"></div>
  {inner}
</div>"#,
        left = LEFT[col],
        top = TOP[row],
    )
}

#[test]
fn backdrop_shapes_matrix_matches_reference() {
    const RING: &str = "border: 2px solid #1d4ed8; box-sizing: border-box; \
                        transform: rotate(20deg)";
    let subject = |extra: &str| -> String {
        format!(
            r#"<div style="display: flex; position: absolute; left: 16px; top: 28px; width: 84px; height: 72px; {extra}"></div>"#
        )
    };
    let body = [
        board(0, 0, &subject("backdrop-filter: blur(6px)")),
        board(
            0,
            1,
            &subject("backdrop-filter: blur(6px); border-radius: 24px"),
        ),
        board(0, 2, &subject("")),
        board(1, 0, &subject("backdrop-filter: blur(2px)")),
        board(1, 1, &subject("backdrop-filter: blur(12px)")),
        board(
            1,
            2,
            r#"<div style="display: flex; position: absolute; left: 22px; top: 28px; width: 72px; height: 72px; border-radius: 50%; backdrop-filter: blur(6px)"></div>"#,
        ),
        board(
            2,
            0,
            &subject(
                "backdrop-filter: blur(8px); background-color: rgb(255 255 255 / 45%); \
                 border-radius: 14px; border: 1px solid rgb(29 78 216 / 60%); \
                 box-sizing: border-box",
            ),
        ),
        board(
            2,
            1,
            &subject(
                "background-color: rgb(255 255 255 / 45%); border-radius: 14px; \
                 border: 1px solid rgb(29 78 216 / 60%); box-sizing: border-box",
            ),
        ),
        board(2, 2, &subject("backdrop-filter: blur(6px); opacity: 0.6")),
        board(
            3,
            0,
            &subject(&format!("{RING}; backdrop-filter: blur(6px)")),
        ),
        board(3, 1, &subject(RING)),
        board(3, 2, &subject("backdrop-filter: blur(4px) brightness(0.7)")),
        board(4, 0, &subject("backdrop-filter: brightness(0.5)")),
        board(4, 1, &subject("backdrop-filter: grayscale(1) blur(4px)")),
        board(
            4,
            2,
            r#"<div style="display: flex; position: absolute; left: 0px; top: 0px; width: 115px; height: 128px; opacity: 0.5">
  <div style="display: flex; position: absolute; left: 0px; top: 52px; width: 115px; height: 22px; background-color: #0d9488"></div>
  <div style="display: flex; position: absolute; left: 16px; top: 28px; width: 84px; height: 72px; backdrop-filter: blur(6px)"></div>
</div>"#,
        ),
    ]
    .concat();

    let actual = screenshot::capture(
        "backdrop_shapes_matrix_matches_reference",
        &page(&body),
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
    );
    screenshot::assert_golden(&["backdrop-filter", "shapes"], &actual);
}

#[test]
fn backdrop_card_and_edges_matches_reference() {
    const FRAGMENT: &str = r#"
<div style="display: flex; position: relative; width: 393px; height: 727px; background-color: #e5e7eb; font-family: Roboto; color: #111827">
  <div style="display: flex; position: absolute; left: 12px; top: 12px; width: 369px; height: 200px; background-color: #fef3c7">
    <div class="text-block" style="display: flex; position: absolute; left: 14px; top: 16px; width: 341px; font-size: 19px">Sphinx of black quartz judge</div>
    <div class="text-block" style="display: flex; position: absolute; left: 14px; top: 48px; width: 341px; font-size: 19px">Pack my box with five dozen</div>
    <div class="text-block" style="display: flex; position: absolute; left: 14px; top: 80px; width: 341px; font-size: 19px">The quick brown fox jumps</div>
    <div class="text-block" style="display: flex; position: absolute; left: 14px; top: 112px; width: 341px; font-size: 19px">How vexingly quick zebras</div>
    <div class="text-block" style="display: flex; position: absolute; left: 14px; top: 144px; width: 341px; font-size: 19px">Waltz bad nymph for jigs</div>
    <div style="display: flex; position: absolute; left: 14px; top: 40px; width: 160px; height: 120px; border-radius: 16px; background-color: rgb(255 255 255 / 40%); backdrop-filter: blur(6px)">
      <div style="display: flex; position: absolute; left: 16px; top: 76px; width: 128px; height: 28px; border-radius: 8px; backdrop-filter: brightness(0.6)"></div>
    </div>
    <div style="display: flex; position: absolute; left: 195px; top: 40px; width: 160px; height: 120px; border-radius: 16px; background-color: rgb(255 255 255 / 40%)"></div>
    <div style="display: flex; position: absolute; left: 0px; top: 92px; width: 369px; height: 12px; background-color: #f59e0b"></div>
  </div>
  <div style="display: flex; position: absolute; left: 12px; top: 232px; width: 369px; height: 190px; background-color: #ffffff">
    <div style="display: flex; position: absolute; left: 0px; top: 0px; width: 185px; height: 190px; background-color: #0f172a"></div>
    <div style="display: flex; position: absolute; left: 0px; top: 82px; width: 369px; height: 22px; background-color: #0d9488"></div>
    <div style="display: flex; position: absolute; left: 137px; top: 16px; width: 96px; height: 56px; backdrop-filter: blur(8px)"></div>
    <div style="display: flex; position: absolute; left: 137px; top: 114px; width: 96px; height: 56px; border-radius: 28px; backdrop-filter: blur(8px) grayscale(1)"></div>
  </div>
  <div style="display: flex; position: absolute; left: 12px; top: 442px; width: 369px; height: 150px; background-color: #0f172a"></div>
  <div style="display: flex; position: absolute; left: 60px; top: 470px; width: 200px; height: 44px; background-color: #f59e0b"></div>
  <div style="display: flex; position: absolute; left: -48px; top: 460px; width: 240px; height: 120px; background-color: rgb(255 255 255 / 30%); border-radius: 20px; backdrop-filter: blur(10px)"></div>
  <div style="display: flex; position: absolute; left: 236px; top: 612px; width: 145px; height: 100px; background-color: #0d9488"></div>
  <div style="display: flex; position: absolute; left: 300px; top: 656px; width: 120px; height: 90px; backdrop-filter: blur(6px)"></div>
</div>
"#;

    let actual = screenshot::capture(
        "backdrop_card_and_edges_matches_reference",
        FRAGMENT,
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
    );
    screenshot::assert_golden(&["backdrop-filter", "card-and-edges"], &actual);
}
