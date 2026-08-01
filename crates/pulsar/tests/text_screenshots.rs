//! Golden screenshots of `pulsar::paint::text`.
//!
//! Each case exists to be *looked at* as much as to be diffed: glyph
//! rasterization quality, synthesis, and how decorations sit against real
//! letterforms are all things a numeric assertion cannot judge but a reviewer
//! can see at a glance. `tests/gpu_pixels.rs` covers the complementary half —
//! Ahem's solid em squares make individual pixels assertable, at the cost of
//! showing nothing about rendering quality.
//!
//! Every fixture renders **vendored Roboto**, never a host font; see
//! `support/screenshot.rs` for why that is not optional. Refresh with:
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p pulsar --test text_screenshots`.

mod common;
#[path = "support/html.rs"]
mod html;
#[path = "support/screenshot.rs"]
mod screenshot;

use flashbulb::Image;

fn assert_text_golden(case: &str, actual: &Image) {
    screenshot::assert_golden(&["text", case], actual);
}

/// A type specimen: one line per size, then the synthesis and color rows.
///
/// Roboto Regular is the only registered face, so `font-weight: 700` and
/// `font-style: italic` have nothing to match and must come out of
/// `paint::text`'s synthesis path — `FontEmbolden` for the fake bold, a skew
/// on `glyph_transform` for the fake oblique. Reviewing this one means asking:
/// do the letterforms look like Roboto at every size, is the antialiasing even
/// (no dropped stems at 11px, no fringing at 44px), does the bold row read as
/// heavier without smearing, and does the oblique row lean *right*.
#[test]
fn text_specimen() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 420px; height: 300px; padding: 16px 20px; gap: 2px; box-sizing: border-box; background-color: white; font-family: Roboto; color: #111827">
  <div style="display: flex; font-size: 11px">Sphinx of black quartz, judge my vow — 11px</div>
  <div style="display: flex; font-size: 13px">Sphinx of black quartz, judge my vow — 13px</div>
  <div style="display: flex; font-size: 16px">Sphinx of black quartz — 16px</div>
  <div style="display: flex; font-size: 22px">Sphinx of black — 22px</div>
  <div style="display: flex; font-size: 32px">Hamburgefonstiv</div>
  <div style="display: flex; font-size: 44px">Hamburg 0123</div>
  <div style="display: flex; font-size: 20px; font-weight: 700">Synthetic bold 700</div>
  <div style="display: flex; font-size: 20px; font-style: italic">Synthetic oblique</div>
  <div style="display: flex; font-size: 20px; color: #dc2626">Colored fill (#dc2626)</div>
</div>
"#;
    let actual = screenshot::capture("text_specimen", FRAGMENT, 420.0, 300.0);
    assert_text_golden("specimen", &actual);
}

/// Every `text-decoration-style` this engine can reach, on both lines.
///
/// The `lynx` stylo fork compiles `text-decoration-line` without an `OVERLINE`
/// bit, so underline and line-through are the whole surface. Thickness and
/// offset always come from the run's font metrics — `text-decoration-thickness`
/// is gecko-only in the fork — which is exactly what makes this worth looking
/// at: the bands have to land against Roboto's own underline position, not a
/// guessed one. The size is deliberately large: metrics thickness at body size
/// is about one pixel, and at one pixel `dotted`, `dashed`, `wavy` and `solid`
/// are indistinguishable to a reviewer, which would make the golden useless
/// for the thing it exists to show.
///
/// The last row decorates an ancestor rather than the text's own box, so it
/// also covers `propagated_decorations` drawing in the *decorating* box's
/// color rather than the text's (css-text-decor-3 §2).
#[test]
fn text_decorations() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 460px; height: 430px; padding: 16px 20px; gap: 6px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 26px; color: #111827">
  <div style="display: flex; text-decoration: underline">Underline solid</div>
  <div style="display: flex; text-decoration: underline double">Underline double</div>
  <div style="display: flex; text-decoration: underline dotted">Underline dotted</div>
  <div style="display: flex; text-decoration: underline dashed">Underline dashed</div>
  <div style="display: flex; text-decoration: underline wavy">Underline wavy</div>
  <div style="display: flex; text-decoration: line-through">Line-through solid</div>
  <div style="display: flex; text-decoration: line-through double">Line-through double</div>
  <div style="display: flex; text-decoration: underline line-through">Both lines at once</div>
  <div style="display: flex; text-decoration: underline #2563eb wavy">Decoration color wins</div>
  <div style="display: flex; text-decoration: underline #16a34a">
    <span style="display: flex; color: #dc2626">Propagated from the parent</span>
  </div>
</div>
"#;
    let actual = screenshot::capture("text_decorations", FRAGMENT, 460.0, 430.0);
    assert_text_golden("decorations", &actual);
}

/// `text-shadow` and `text-stroke`, the two passes that repeat the glyph
/// silhouette.
///
/// Both are authored in **Lynx's** grammar, not the W3C one, because that is
/// what the fork parses: `SimpleShadow`'s `lynx` arm requires all four
/// components in the fixed order `<x> <y> <blur> <color>` — a W3C-legal
/// `text-shadow: 3px 3px #93c5fd` is a parse error here and paints nothing —
/// and `lynx_vector.single_item` accepts exactly one shadow, so the
/// comma-separated list, and with it the last-specified-first paint order in
/// `paint::text`, is unauthorable through CSS. `text-stroke` is Lynx's
/// unprefixed alias for `-webkit-text-stroke`.
///
/// Every row therefore states a blur radius, and every row is expected to
/// render a **hard-edged** offset copy: blur is parsed and not painted, a
/// recorded v1 limit. This golden is what makes that limit visible instead of
/// merely documented — when blur lands, these rows change and the diff is the
/// review. The stroke rows follow the `WebKit` and Lynx convention of filling
/// first and stroking over it, which is why the heavy stroke eats into the
/// glyph rather than only growing outward.
#[test]
fn text_shadow_and_stroke() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 460px; height: 330px; padding: 16px 20px; gap: 12px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 28px; color: #111827">
  <div style="display: flex; text-shadow: 4px 4px 0px #93c5fd">Offset shadow, no blur</div>
  <div style="display: flex; text-shadow: 4px 4px 6px #9ca3af">Blur radius is ignored</div>
  <div style="display: flex; color: white; text-shadow: 2px 2px 0px #111827">White fill, dark shadow</div>
  <div style="display: flex; text-stroke: 1px #2563eb">Thin stroke over fill</div>
  <div style="display: flex; color: #fde68a; text-stroke: 3px #b45309">Heavy stroke over fill</div>
  <div style="display: flex; text-decoration: underline; text-shadow: 4px 4px 0px #fca5a5">Shadow takes the underline</div>
</div>
"#;
    let actual = screenshot::capture("text_shadow_and_stroke", FRAGMENT, 460.0, 330.0);
    assert_text_golden("shadow-and-stroke", &actual);
}

/// Multi-line body text: wrapping, `line-height`, `letter-spacing`,
/// `text-align`, and `text-indent`.
///
/// Every other text case is one glyph run per box. This one is where
/// `paint_pass` walks several lines and several runs per line, so it is what
/// catches a baseline or per-line offset that is wrong by a constant — a bug
/// invisible in single-line goldens.
///
/// The aligned rows are deliberately long enough to wrap. A short flex item
/// shrinks to its content, leaving no room inside the line box for `center` or
/// `right` to move anything, so a short row would look identical under all
/// three values and prove nothing. Wrapping fills the line box, and then the
/// alignment shows up on the *last* line — which also checks that the painter
/// positions each run from the run's own `offset()` rather than the box's left
/// edge.
#[test]
fn text_paragraph() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 420px; height: 420px; padding: 16px 20px; gap: 10px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 15px; color: #1f2937">
  <div style="display: flex; width: 380px; line-height: 1.5">Typography is the craft of arranging type to make written language legible, readable and appealing when displayed.</div>
  <div style="display: flex; width: 380px; line-height: 2.2; background-color: #f3f4f6">Loose line-height at 2.2 spreads these wrapped lines apart, and the band of background behind them shows where each line box sits.</div>
  <div style="display: flex; width: 380px; letter-spacing: 2px">Letter-spacing of two pixels</div>
  <div style="display: flex; width: 380px; text-align: center; background-color: #f3f4f6">Centered text, wrapped over enough lines that the last one has room left over to be centered in.</div>
  <div style="display: flex; width: 380px; text-align: right; background-color: #f3f4f6">Right-aligned text, wrapped over enough lines that the last one has room left over to sit against the right edge.</div>
  <div style="display: flex; width: 380px; text-indent: 28px; background-color: #f3f4f6">Indented first line, then the rest of the paragraph wraps back to the left edge of the box.</div>
</div>
"#;
    let actual = screenshot::capture("text_paragraph", FRAGMENT, 420.0, 420.0);
    assert_text_golden("paragraph", &actual);
}

/// Lynx's gradient-valued `color` — `color: linear-gradient(…)`.
///
/// The fork keeps the gradient in the computed value
/// (`ColorPropertyValue::Gradient`) instead of collapsing it, and
/// `paint::text` turns it into a peniko gradient brush on the glyph run. Note
/// this is authored *without* `background-clip: text` and without
/// `color: transparent` — compare `text_background_clip`, which reaches a
/// similar look the long way round and is the only thing this engine could do
/// before.
///
/// What the rows are for:
/// - the first three are the three gradient shapes, so the shared `background::gradient_brush`
///   resolution is exercised through the text path, not just the background path;
/// - `padded` proves the anchor. The ramp resolves against the element's padding box, so with 40px
///   of left padding the text starts partway into the ramp and must *not* begin at the ramp's first
///   stop;
/// - the wrapped row is the case a per-run anchor would get wrong, and it uses a *vertical* ramp to
///   make that unmissable: anchored to the element, line one takes the top of the ramp and line two
///   takes further down. Anchored per run — each run being one line tall — both lines would come
///   out identical;
/// - the last row pairs the gradient with an underline, which stays black:
///   `text-decoration-color`'s initial `currentcolor` resolves through the fork's parallel solid
///   color, matching Lynx's own style engine.
#[test]
fn text_color_gradient() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 440px; height: 400px; padding: 16px 20px; gap: 12px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 32px; font-weight: 700">
  <div style="display: flex; color: linear-gradient(90deg, #f43f5e, #6366f1, #14b8a6)">Linear ramp</div>
  <div style="display: flex; color: radial-gradient(circle, #f59e0b, #7c3aed)">Radial ramp</div>
  <div style="display: flex; color: conic-gradient(#ef4444, #22c55e, #3b82f6, #ef4444)">Conic ramp</div>
  <div style="display: flex; padding-left: 40px; color: linear-gradient(90deg, #f43f5e, #14b8a6)">Padded</div>
  <div style="display: flex; width: 320px; font-size: 22px; color: linear-gradient(180deg, #f43f5e, #14b8a6)">This wraps onto three lines, so a vertical ramp has to walk rose to teal down them.</div>
  <div style="display: flex; font-size: 26px; text-decoration: underline; color: linear-gradient(90deg, #f59e0b, #ef4444)">Underline stays solid</div>
</div>
"#;
    let actual = screenshot::capture("text_color_gradient", FRAGMENT, 440.0, 400.0);
    assert_text_golden("color-gradient", &actual);
}

/// Gradient-valued `color` on an element that *also* paints a background.
///
/// This is the case that decided the implementation. Lynx's text-gradient
/// sugar is often described as lowering to
/// `background-image: <gradient>; background-clip: text; color: transparent`,
/// and that lowering cannot survive this fixture: the element's own
/// background would be clipped to the glyph ink along with the gradient, so
/// every row here would lose its background. Painting the gradient as a brush
/// on the glyph run instead leaves `background-*` completely untouched, which
/// is what these rows check.
///
/// Row by row: a solid background under gradient text; a border, which moves
/// the padding box and so must move the ramp with it; two *independent*
/// gradients running opposite directions on the same element, which must not
/// share brush state; a rounded background, which clips the background and not
/// the text; and the genuine conflict — `background-clip: text` on an element
/// whose `color` is also a gradient. The expected outcome of that last row is
/// that it looks like *plain gradient text on white*: the clipped background
/// paints into the glyph ink first and the opaque gradient then covers it
/// exactly, so the backdrop is invisible by construction rather than by
/// accident. It is here to pin that the two features do not corrupt each
/// other, which is the only thing an image can show about it.
#[test]
fn text_color_gradient_over_background() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 460px; height: 400px; padding: 16px 20px; gap: 14px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 30px; font-weight: 700">
  <div style="display: flex; background-color: #111827; color: linear-gradient(90deg, #fde68a, #f43f5e)">Solid backdrop</div>
  <div style="display: flex; border: 8px solid #0f766e; padding: 4px; background-color: #111827; color: linear-gradient(90deg, #fde68a, #f43f5e)">Border moves it</div>
  <div style="display: flex; background-image: linear-gradient(90deg, #1e3a8a, #111827); color: linear-gradient(270deg, #fde68a, #f43f5e)">Two ramps, opposed</div>
  <div style="display: flex; padding: 2px 10px; border-radius: 16px; background-color: #4c1d95; color: linear-gradient(90deg, #fde68a, #5eead4)">Rounded backdrop</div>
  <div style="display: flex; background-clip: text; background-color: #111827; color: linear-gradient(90deg, #fbbf24, #a855f7)">Clip plus gradient</div>
</div>
"#;
    let actual = screenshot::capture(
        "text_color_gradient_over_background",
        FRAGMENT,
        460.0,
        400.0,
    );
    assert_text_golden("color-gradient-over-background", &actual);
}

/// `background-clip: text` with real letterforms behind it.
///
/// `gpu_pixels.rs` already asserts the clip numerically on Ahem's em squares;
/// what it cannot show is whether the sandwich holds along an antialiased
/// glyph edge, which is the only place this feature can go subtly wrong.
///
/// The third row pairs the clip with a decoration to make a recorded v1 limit
/// visible: `paint_silhouette` draws glyph ink only, so the underline is not
/// part of the clip. It needs an explicit `text-decoration-color`, because the
/// `color: transparent` these rows are authored with is also what
/// `currentcolor` would resolve the decoration to — an underline that is
/// missing because it is transparent would look exactly like the limit under
/// test, and prove nothing.
#[test]
fn text_background_clip() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 440px; height: 260px; padding: 16px 20px; gap: 14px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 40px; font-weight: 700">
  <div style="display: flex; color: transparent; background-clip: text; background-image: linear-gradient(90deg, #f43f5e, #6366f1, #14b8a6)">Gradient ink</div>
  <div style="display: flex; color: transparent; background-clip: text; background-color: #7c3aed">Solid ink</div>
  <div style="display: flex; color: transparent; text-decoration: underline #111827; background-clip: text; background-image: linear-gradient(90deg, #f59e0b, #ef4444)">Bare underline</div>
</div>
"#;
    let actual = screenshot::capture("text_background_clip", FRAGMENT, 440.0, 260.0);
    assert_text_golden("background-clip", &actual);
}
