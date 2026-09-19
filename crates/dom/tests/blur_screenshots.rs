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
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p dom --test blur_screenshots`.

#[path = "support/html.rs"]
mod html;
mod paint_common;
#[path = "support/screenshot.rs"]
mod screenshot;

const SCREEN_WIDTH: f32 = 393.0;
const SCREEN_HEIGHT: f32 = 727.0;

/// Column origins of the 3-wide grid, in CSS px.
const LEFT: [u32; 3] = [12, 139, 266];
/// Row origins of the 5-tall grid, in CSS px.
const TOP: [u32; 5] = [12, 152, 292, 432, 572];

/// The subject every geometry cell blurs: a 64×64 box centred in its cell, so
/// a 3σ margin of up to 25 px fits inside the cell and anything larger is
/// visibly *allowed* to leave it. Nothing clips a cell — that is the point.
const SUBJECT: &str =
    "display: flex; position: absolute; left: 26px; top: 32px; width: 64px; height: 64px";

/// One grid cell: a white card at (`row`, `col`) holding `inner`.
///
/// The card is deliberately opaque white rather than transparent: a blur over
/// white is where an unpremultiplied filter shows its dark halo, so every one
/// of these cells is also a halo check.
fn cell(row: usize, col: usize, inner: &str) -> String {
    format!(
        r#"<div style="display: flex; position: absolute; left: {left}px; top: {top}px; width: 115px; height: 128px; background-color: #ffffff">{inner}</div>"#,
        left = LEFT[col],
        top = TOP[row],
    )
}

/// The 64×64 subject box carrying `extra` declarations.
fn subject(extra: &str) -> String {
    format!(r#"<div style="{SUBJECT}; {extra}"></div>"#)
}

/// The page the cells sit on. `position: relative` so every cell's absolute
/// placement resolves against it, and `font-family: Roboto` so text cells
/// inherit the one vendored face.
fn page(body: &str) -> String {
    format!(
        r#"<div style="display: flex; position: relative; width: 393px; height: 727px; background-color: #e5e7eb; font-family: Roboto; color: #111827">{body}</div>"#
    )
}

/// Page 1 — geometry and the group.
///
/// | | c0 | c1 | c2 |
/// | --- | --- | --- | --- |
/// | r0 | solid box, `blur(4px)` | rounded 16px box, 6px border + background, `blur(4px)` | linear-gradient background, `blur(6px)` |
/// | r1 | `blur(16px)` — the decimated path | `blur(1px)` — undecimated, kernel radius ≤ 3 | no filter — control for r0c0 |
/// | r2 | `blur(3px)` + `opacity: 0.5` | `blur(3px)` + `clip-path: circle(40%)` | `blur(3px)` + `border-radius: 50%` |
/// | r3 | `blur(3px)` under an `overflow: hidden` parent smaller than the ink | `blur(3px)` + `mask-image` gradient | nested: parent `blur(3px)` ⊃ child `blur(3px)` |
/// | r4 | `blur(3px)` on a border-only box (no background) | white box, `blur(6px)`, on white — the halo check | `blur(3px)` on a box with a `box-shadow` |
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
        // The one cell whose blurred output *is* clipped, and the only one:
        // the clip belongs to an ancestor, outside the group, so it cuts the
        // baked texture rather than the content inside the bake.
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
        // White on white. The premultiply pass is the whole reason this cell
        // is blank rather than ringed: filtering vello's unpremultiplied
        // target averages the colour of the fully transparent margin into the
        // square's edge, and a dark halo appears.
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

/// The two-colour subject the chain cells share: a red box with a blue child,
/// so a colour pass applied before the blur and one applied after it are
/// distinguishable, and so is the order of the list.
fn two_colour(extra: &str) -> String {
    format!(
        r#"<div style="{SUBJECT}; background-color: #dc2626; {extra}">
  <div style="display: flex; position: absolute; left: 16px; top: 16px; width: 32px; height: 32px; background-color: #1d4ed8"></div>
</div>"#
    )
}

/// One line of Roboto, positioned in its cell.
fn line(size: u32, extra: &str, text: &str) -> String {
    format!(
        r#"<div class="text-block" style="display: flex; position: absolute; left: 8px; top: 48px; width: 99px; font-size: {size}px; {extra}">{text}</div>"#
    )
}

/// Page 2 — text, filter chains, and σ folding.
///
/// | | c0 | c1 | c2 |
/// | --- | --- | --- | --- |
/// | r0 | Roboto text `blur(2px)` on a coloured background | text `blur(2px)` + `text-shadow` | text, no filter — control |
/// | r1 | `blur(3px)` on a subtree: nested box **and** a text run in one group | `grayscale(1) blur(3px)` | `blur(3px) grayscale(1)` |
/// | r2 | red box + blue child, no filter — control for r1c1/r1c2 | `blur(3px) brightness(0.6)` | `brightness(0.6)` — control |
/// | r3 | `blur(3px) blur(4px)` — two blurs fold by variance | `blur(5px)` — the reference the fold must match | `grayscale(1)` — control |
/// | r4 | one 24 px line across the page at `blur(2px)`: the legibility check | | |
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
        // The group is the whole subtree: one bake holds the wrapper's
        // background, the nested box, and the glyph run together.
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
        // 3² + 4² = 5², so r3c0 and r3c1 must be the same picture. A fold that
        // stopped working would show as the left cell being sharper.
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

/// Page 3 — transforms.
///
/// σ is authored in the element's own CSS px and baked in *device* px, so a
/// transform between the two scales it. Every band here is a blurred subject
/// beside the same subject unfiltered, because the scaling is a relationship
/// rather than a picture: a golden of a blurred rotated box alone could only
/// regress into "different".
///
/// | band | left (blurred) | right (control) |
/// | --- | --- | --- |
/// | 0 | `rotate(30deg)` + `blur(4px)` | `rotate(30deg)` |
/// | 1 | `scale(2)` on a 32 px box + `blur(2px)` | `scale(2)` on a 32 px box |
/// | 2 | `scaleX(3)` on a 48 px box + `blur(3px)` — the isotropic approximation | `scaleX(3)` |
/// | 3 | `scale(0.5)` + `blur(4px)` — σ shrinks with the group | `scale(0.5)` |
/// | 4 | `scale(2)` on a 32 px box + `blur(2px)` | *no transform*, `blur(4px)` |
///
/// Band 4 is the sharpest statement of the rule: 2 px of σ under a 2× scale
/// and 4 px of σ under none are the same blur, so the two halves have to be
/// the same picture. They would diverge the moment σ stopped being scaled
/// into device space.
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

/// Page 4 — a replaced element inside a bake, and the page's own four edges.
///
/// The image band draws the 8×8 checker at 110×80 under `object-fit: contain`,
/// which letterboxes it to 80×80 and leaves *transparent* bands inside the
/// element's own box. So the blurred copy is a halo check over transparency
/// the content produced, not over the bake's margin — a different way to get
/// the premultiply wrong. `image-rendering: pixelated` keeps the source's
/// 8×8 structure crisp in the control, so the blur is unmistakable beside it.
///
/// The four purple boxes straddle the page's top, left, right and bottom
/// edges under `blur(6px)`. Their 18 px of ink is baked *outside* the
/// viewport and composed back in, so every visible edge is a smooth gradient.
/// Inflating a group's bounds after the viewport intersection rather than
/// before would cut that margin off and leave a hard edge exactly at the
/// page boundary — which is what this page exists to catch.
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

    let images = flashbulb::TestImages::new();
    let band = doc.el(root, "band");
    for class in ["left b3", "right"] {
        let node = doc.el_tag(band, "img", class);
        let source = format!("app:///{}.png", node.to_bits());
        let (width, height, rgba) = checker(8, 8);
        images.insert_rgba8(&source, width, height, rgba);
        doc.dom.set_image_source(node, Some(&source));
    }

    for class in [
        "edge edge-top",
        "edge edge-left",
        "edge edge-right",
        "edge edge-bottom",
    ] {
        doc.el(root, class);
    }

    flashbulb::render_with_images(&mut doc.dom, &images);
    let actual = screenshot::capture_prebuilt_document(
        "blur_edges_and_replaced_content_matches_reference",
        &mut doc.dom,
        &images,
    );
    screenshot::assert_golden(&["filter-blur", "edges-and-replaced"], &actual);
}

/// The same four-quadrant checker `tests/screenshots.rs` draws, so a reader
/// comparing the two goldens is looking at one bitmap.
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

/// The board every `backdrop-filter` cell filters: a white card with a
/// crimson left half and an amber bar across it, so every subject sits over
/// one vertical seam, one horizontal seam and two hues.
///
/// A backdrop is made of what the *scene* drew, not of the render's base
/// colour, so the card paints its own opaque white. The two hues are what
/// makes a colour pass in a chain legible beside a blur.
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

/// Page 5 — `backdrop-filter` geometry.
///
/// Every cell is the same board with one subject over its seams, so a reader
/// compares what is *inside* the subject against the same board beside it.
/// Nothing here paints a background of its own unless the cell says so: what
/// is drawn is the element's own backdrop, filtered.
///
/// | | c0 | c1 | c2 |
/// | --- | --- | --- | --- |
/// | r0 | `blur(6px)` | `blur(6px)` + `border-radius: 24px` | no filter — control |
/// | r1 | `blur(2px)` | `blur(12px)` — the decimated path | `blur(6px)` on a circle |
/// | r2 | frosted card: `blur(8px)` + a translucent white background | the same card unfiltered — control | `blur(6px)` + `opacity: 0.6` |
/// | r3 | `blur(6px)` under `rotate(20deg)`, ringed so the crop is legible | the same ring unfiltered — control | `blur(4px) brightness(0.7)` — a chain |
/// | r4 | `brightness(0.5)` alone — the σ = 0 bake | `grayscale(1) blur(4px)` | a box **inside** an `opacity: 0.5` wrapper: its backdrop stops at the wrapper |
#[test]
fn backdrop_shapes_matrix_matches_reference() {
    // The rotated pair carries a ring so the crop's own shape is visible:
    // the blur inside it is bounded by a rotated rectangle, which is the one
    // place the axis-aligned bake rect and the drawn shape differ.
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
        // The wrapper is a Backdrop Root, so the board behind it is *not* in
        // the subject's backdrop. What the subject filters is the wrapper's
        // own teal bar and nothing else, and the wrapper's own `opacity`
        // fades backdrop and subject together.
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

/// Page 6 — a frosted card over text, and the crop's own edges.
///
/// The top band is the shape the property exists for: a translucent card with
/// `backdrop-filter: blur()` over body text, beside the identical card with
/// no filter. Underneath it, four statements about the crop:
///
/// - **Nesting.** The small box inside the frosted card filters the card's *filtered* backdrop,
///   because the card is not a Backdrop Root and its filtered image is painted inside its own
///   group.
/// - **Later siblings.** The amber bar is painted *after* the card, so it crosses it with a hard
///   edge — it is in front of the backdrop, not in it.
/// - **Straddling a seam.** The middle band's two subjects sit astride a navy/white edge, one
///   square and one rounded with a colour pass after the blur.
/// - **The page's own edges.** The last card hangs off the page's left edge and the last box off
///   its bottom-right corner. Their crops run outside the viewport, where the commit culled
///   everything; the mirror edge mode is what keeps the visible part from darkening toward the
///   boundary.
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
