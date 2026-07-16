//! CSS `<color>` value-grammar tests ported from the `LynxJS` C++ engine.
//!
//! Ports:
//! - `core/renderer/css/css_color_unittest.cc` (`CSSColor.Keywords`, `CSSColor.Parse`)
//! - `core/renderer/css/parser/color_handler_unittest.cc` (`ColorHandler.Process`)
//! - `core/renderer/css/css_keywords_unittest.cc` (`CSSKeywords.TokenTypeCheck` — skipped,
//!   tokenizer-internal; see footer)
//!
//! Scope: `enableCSSSelector = true` / `enableRemoveCSSScope = true` only.
//!
//! Expectation policy (see `tests/common/mod.rs` and
//! `docs/tracking/deviations.md`): each ported assertion uses the inventory's
//! `ours_expected`. `<color>` is a real W3C feature, so W3C-correct behavior
//! wins where the C++ engine deviates. The one flip vs. Lynx: legacy comma
//! `rgb()` with *mixed* percentage+number channels (`rgb(100%, 0, 100%)`) is
//! invalid per CSS Color 4 and must be rejected here even though Lynx's lenient
//! parser accepts it.

#![allow(clippy::too_many_lines)]
// The AbsoluteColor -> u8 channel conversion rounds and clamps into `[0, 255]`
// before the cast, so truncation/sign-loss cannot occur.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

mod common;

use common::{Doc, parses};
use stylo::color::AbsoluteColor;
use stylo::values::computed::ColorPropertyValue;
use stylo_dom::ElementId;

/// Assert a computed `AbsoluteColor` resolves to the given legacy-sRGB channels.
///
/// Every color under test is a legacy syntax color (`<named-color>`,
/// `<hex-color>`, `rgb()/rgba()`, `hsl()/hsla()`), so we normalize through
/// `into_srgb_legacy()` and compare rounded 8-bit channels — this is
/// representation-independent (a color stored in the `hsl` color space compares
/// equal to its sRGB equivalent). Alpha is a float and is compared with a
/// tolerance below `1/255` so fractional alphas derived from hex nibbles or
/// `hsla()` don't hinge on the float→u8 rounding convention the C++ test bakes
/// into its packed `0xAARRGGBB`.
fn assert_rgba(got: AbsoluteColor, r: u8, g: u8, b: u8, alpha: f32, ctx: &str) {
    let srgb = got.into_srgb_legacy();
    let comps = *srgb.raw_components();
    let to_u8 = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    let rgb = (to_u8(comps[0]), to_u8(comps[1]), to_u8(comps[2]));
    assert_eq!(rgb, (r, g, b), "rgb channels for `{ctx}`");
    assert!(
        (comps[3] - alpha).abs() < 0.01,
        "alpha for `{ctx}`: got {}, want {alpha}",
        comps[3]
    );
}

/// The computed `background-color`, resolved to an absolute color.
fn background_color(doc: &Doc, id: ElementId) -> AbsoluteColor {
    let style = doc.style(id);
    let current = style.clone_color();
    style.clone_background_color().resolve_to_absolute(&current)
}

/// Lynx text gradients must survive parsing and cascade as a computed value;
/// the future layout/paint adapter reads this from `stylo-dom`, not from the
/// Widget/PAPI facade.
#[test]
fn text_gradient_survives_document_cascade() {
    let mut doc = Doc::new();
    let element = doc.el(doc.root, "text");
    doc.set_inline(element, "color: linear-gradient(red, blue)");
    doc.flush();

    assert!(matches!(
        doc.style(element).clone_color_value(),
        ColorPropertyValue::Gradient(_)
    ));
}

/// The full Lynx `<named-color>` table (148 entries incl. `transparent`) with
/// its canonical sRGB channels. Every non-transparent alpha is 1.0.
const NAMED_COLORS: &[(&str, u8, u8, u8, f32)] = &[
    ("transparent", 0, 0, 0, 0.0),
    ("aliceblue", 240, 248, 255, 1.0),
    ("antiquewhite", 250, 235, 215, 1.0),
    ("aqua", 0, 255, 255, 1.0),
    ("aquamarine", 127, 255, 212, 1.0),
    ("azure", 240, 255, 255, 1.0),
    ("beige", 245, 245, 220, 1.0),
    ("bisque", 255, 228, 196, 1.0),
    ("black", 0, 0, 0, 1.0),
    ("blanchedalmond", 255, 235, 205, 1.0),
    ("blue", 0, 0, 255, 1.0),
    ("blueviolet", 138, 43, 226, 1.0),
    ("brown", 165, 42, 42, 1.0),
    ("burlywood", 222, 184, 135, 1.0),
    ("cadetblue", 95, 158, 160, 1.0),
    ("chartreuse", 127, 255, 0, 1.0),
    ("chocolate", 210, 105, 30, 1.0),
    ("coral", 255, 127, 80, 1.0),
    ("cornflowerblue", 100, 149, 237, 1.0),
    ("cornsilk", 255, 248, 220, 1.0),
    ("crimson", 220, 20, 60, 1.0),
    ("cyan", 0, 255, 255, 1.0),
    ("darkblue", 0, 0, 139, 1.0),
    ("darkcyan", 0, 139, 139, 1.0),
    ("darkgoldenrod", 184, 134, 11, 1.0),
    ("darkgray", 169, 169, 169, 1.0),
    ("darkgreen", 0, 100, 0, 1.0),
    ("darkgrey", 169, 169, 169, 1.0),
    ("darkkhaki", 189, 183, 107, 1.0),
    ("darkmagenta", 139, 0, 139, 1.0),
    ("darkolivegreen", 85, 107, 47, 1.0),
    ("darkorange", 255, 140, 0, 1.0),
    ("darkorchid", 153, 50, 204, 1.0),
    ("darkred", 139, 0, 0, 1.0),
    ("darksalmon", 233, 150, 122, 1.0),
    ("darkseagreen", 143, 188, 143, 1.0),
    ("darkslateblue", 72, 61, 139, 1.0),
    ("darkslategray", 47, 79, 79, 1.0),
    ("darkslategrey", 47, 79, 79, 1.0),
    ("darkturquoise", 0, 206, 209, 1.0),
    ("darkviolet", 148, 0, 211, 1.0),
    ("deeppink", 255, 20, 147, 1.0),
    ("deepskyblue", 0, 191, 255, 1.0),
    ("dimgray", 105, 105, 105, 1.0),
    ("dimgrey", 105, 105, 105, 1.0),
    ("dodgerblue", 30, 144, 255, 1.0),
    ("firebrick", 178, 34, 34, 1.0),
    ("floralwhite", 255, 250, 240, 1.0),
    ("forestgreen", 34, 139, 34, 1.0),
    ("fuchsia", 255, 0, 255, 1.0),
    ("gainsboro", 220, 220, 220, 1.0),
    ("ghostwhite", 248, 248, 255, 1.0),
    ("gold", 255, 215, 0, 1.0),
    ("goldenrod", 218, 165, 32, 1.0),
    ("gray", 128, 128, 128, 1.0),
    ("green", 0, 128, 0, 1.0),
    ("greenyellow", 173, 255, 47, 1.0),
    ("grey", 128, 128, 128, 1.0),
    ("honeydew", 240, 255, 240, 1.0),
    ("hotpink", 255, 105, 180, 1.0),
    ("indianred", 205, 92, 92, 1.0),
    ("indigo", 75, 0, 130, 1.0),
    ("ivory", 255, 255, 240, 1.0),
    ("khaki", 240, 230, 140, 1.0),
    ("lavender", 230, 230, 250, 1.0),
    ("lavenderblush", 255, 240, 245, 1.0),
    ("lawngreen", 124, 252, 0, 1.0),
    ("lemonchiffon", 255, 250, 205, 1.0),
    ("lightblue", 173, 216, 230, 1.0),
    ("lightcoral", 240, 128, 128, 1.0),
    ("lightcyan", 224, 255, 255, 1.0),
    ("lightgoldenrodyellow", 250, 250, 210, 1.0),
    ("lightgray", 211, 211, 211, 1.0),
    ("lightgreen", 144, 238, 144, 1.0),
    ("lightgrey", 211, 211, 211, 1.0),
    ("lightpink", 255, 182, 193, 1.0),
    ("lightsalmon", 255, 160, 122, 1.0),
    ("lightseagreen", 32, 178, 170, 1.0),
    ("lightskyblue", 135, 206, 250, 1.0),
    ("lightslategray", 119, 136, 153, 1.0),
    ("lightslategrey", 119, 136, 153, 1.0),
    ("lightsteelblue", 176, 196, 222, 1.0),
    ("lightyellow", 255, 255, 224, 1.0),
    ("lime", 0, 255, 0, 1.0),
    ("limegreen", 50, 205, 50, 1.0),
    ("linen", 250, 240, 230, 1.0),
    ("magenta", 255, 0, 255, 1.0),
    ("maroon", 128, 0, 0, 1.0),
    ("mediumaquamarine", 102, 205, 170, 1.0),
    ("mediumblue", 0, 0, 205, 1.0),
    ("mediumorchid", 186, 85, 211, 1.0),
    ("mediumpurple", 147, 112, 219, 1.0),
    ("mediumseagreen", 60, 179, 113, 1.0),
    ("mediumslateblue", 123, 104, 238, 1.0),
    ("mediumspringgreen", 0, 250, 154, 1.0),
    ("mediumturquoise", 72, 209, 204, 1.0),
    ("mediumvioletred", 199, 21, 133, 1.0),
    ("midnightblue", 25, 25, 112, 1.0),
    ("mintcream", 245, 255, 250, 1.0),
    ("mistyrose", 255, 228, 225, 1.0),
    ("moccasin", 255, 228, 181, 1.0),
    ("navajowhite", 255, 222, 173, 1.0),
    ("navy", 0, 0, 128, 1.0),
    ("oldlace", 253, 245, 230, 1.0),
    ("olive", 128, 128, 0, 1.0),
    ("olivedrab", 107, 142, 35, 1.0),
    ("orange", 255, 165, 0, 1.0),
    ("orangered", 255, 69, 0, 1.0),
    ("orchid", 218, 112, 214, 1.0),
    ("palegoldenrod", 238, 232, 170, 1.0),
    ("palegreen", 152, 251, 152, 1.0),
    ("paleturquoise", 175, 238, 238, 1.0),
    ("palevioletred", 219, 112, 147, 1.0),
    ("papayawhip", 255, 239, 213, 1.0),
    ("peachpuff", 255, 218, 185, 1.0),
    ("peru", 205, 133, 63, 1.0),
    ("pink", 255, 192, 203, 1.0),
    ("plum", 221, 160, 221, 1.0),
    ("powderblue", 176, 224, 230, 1.0),
    ("purple", 128, 0, 128, 1.0),
    ("red", 255, 0, 0, 1.0),
    ("rosybrown", 188, 143, 143, 1.0),
    ("royalblue", 65, 105, 225, 1.0),
    ("saddlebrown", 139, 69, 19, 1.0),
    ("salmon", 250, 128, 114, 1.0),
    ("sandybrown", 244, 164, 96, 1.0),
    ("seagreen", 46, 139, 87, 1.0),
    ("seashell", 255, 245, 238, 1.0),
    ("sienna", 160, 82, 45, 1.0),
    ("silver", 192, 192, 192, 1.0),
    ("skyblue", 135, 206, 235, 1.0),
    ("slateblue", 106, 90, 205, 1.0),
    ("slategray", 112, 128, 144, 1.0),
    ("slategrey", 112, 128, 144, 1.0),
    ("snow", 255, 250, 250, 1.0),
    ("springgreen", 0, 255, 127, 1.0),
    ("steelblue", 70, 130, 180, 1.0),
    ("tan", 210, 180, 140, 1.0),
    ("teal", 0, 128, 128, 1.0),
    ("thistle", 216, 191, 216, 1.0),
    ("tomato", 255, 99, 71, 1.0),
    ("turquoise", 64, 224, 208, 1.0),
    ("violet", 238, 130, 238, 1.0),
    ("wheat", 245, 222, 179, 1.0),
    ("white", 255, 255, 255, 1.0),
    ("whitesmoke", 245, 245, 245, 1.0),
    ("yellow", 255, 255, 0, 1.0),
    ("yellowgreen", 154, 205, 50, 1.0),
];

/// C++: `css_color_unittest.cc::CSSColor.Keywords`
///
/// Every one of the 148 `<named-color>` keywords parses to its canonical sRGB
/// triple (`transparent` -> `rgba(0,0,0,0)`). stylo supports the identical set
/// (plus `rebeccapurple`/system colors as a superset the C++ table never
/// asserts absent), so this ports as-is.
#[test]
fn named_color_keywords() {
    assert_eq!(
        NAMED_COLORS.len(),
        148,
        "the Lynx named-color table has 148 entries"
    );
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    for &(name, r, g, b, a) in NAMED_COLORS {
        doc.set_inline(el, &format!("color: {name}"));
        doc.flush();
        assert_rgba(doc.color(el), r, g, b, a, name);
    }
}

/// C++: `css_color_unittest.cc::CSSColor.Parse`
///
/// The `<color>` value grammar: `<hex-color>` (4/6/8 digit), legacy
/// `rgb()/rgba()` with clamping, `hsl()/hsla()`, `<named-color>`, and invalid
/// rejection. Ported W3C-corrected: `rgb(100%, 0, 100%)` (mixed percentage +
/// number channels) is rejected here, unlike Lynx's lenient accept.
#[test]
#[allow(clippy::items_after_statements)]
fn color_string_parse_grammar() {
    // (input, r, g, b, alpha). Fractional alphas come from hex nibbles / hsla
    // and are compared with tolerance.
    const VALID: &[(&str, u8, u8, u8, f32)] = &[
        ("red", 255, 0, 0, 1.0),
        ("#00ff00", 0, 255, 0, 1.0),
        // CSS Color 4 four-digit #RGBA: each nibble doubled.
        ("#056b", 0, 85, 102, 0.733),    // A = 0xbb
        ("#090a", 0, 153, 0, 0.667),     // A = 0xaa
        ("#abcd", 170, 187, 204, 0.867), // A = 0xdd
        // CSS Color 4 eight-digit #RRGGBBAA.
        ("#00ff00ee", 0, 255, 0, 0.933), // A = 0xee
        ("rgb(0,0,255)", 0, 0, 255, 1.0),
        // Number channels clamp to [0,255]; negatives clamp to 0.
        ("rgb(2000,-1,255)", 255, 0, 255, 1.0),
        ("rgb(0, 0, 255)", 0, 0, 255, 1.0),
        // Alpha <number> clamps to [0,1].
        ("rgba(0, 0, 255, 100)", 0, 0, 255, 1.0),
        ("hsl(240, 100%, 50%)", 0, 0, 255, 1.0),
        ("hsla(240, 100%, 50%, 0.3)", 0, 0, 255, 0.3),
    ];
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    for &(input, r, g, b, a) in VALID {
        doc.set_inline(el, &format!("color: {input}"));
        doc.flush();
        assert_rgba(doc.color(el), r, g, b, a, input);
    }

    // Invalid inputs: the declaration is dropped (parse fails).
    const INVALID: &[&str] = &[
        "not color", // non-color garbage
        "#hello0",   // 6 chars, non-hex letters
        "#errors",   // 6 chars, non-hex letters
        // W3C-corrected flip: CSS Color 4 legacy comma rgb() is
        // `rgb(<percentage>#{3} ...)` OR `rgb(<number>#{3} ...)`; mixing the two
        // is invalid. Lynx accepts it; stylo (spec-correct) rejects it.
        "rgb(100%, 0, 100%)",
    ];
    for &input in INVALID {
        assert!(!parses("color", input), "`{input}` must be rejected");
    }
}

/// C++: `color_handler_unittest.cc::ColorHandler.Process`
///
/// The same `<color>` grammar exercised through a real color-typed property
/// (`background-color`): named / hex / `rgb()/rgba()` / `hsl()/hsla()`,
/// ASCII case-insensitive keyword matching, all-percentage `rgb()`, clamping,
/// and invalid rejection (nothing emitted -> property stays at its transparent
/// initial). Every `rgb()` row here is all-number or all-percentage, so unlike
/// `CSSColor.Parse` nothing flips — pure port-as-is.
#[test]
#[allow(clippy::items_after_statements)]
fn background_color_value_parse() {
    const VALID: &[(&str, u8, u8, u8, f32)] = &[
        ("red", 255, 0, 0, 1.0),
        ("#00ff00", 0, 255, 0, 1.0),
        ("#056b", 0, 85, 102, 0.733),
        ("#090a", 0, 153, 0, 0.667),
        ("#00ff00ee", 0, 255, 0, 0.933),
        ("rgb(0,0,255)", 0, 0, 255, 1.0),
        ("rgb(2000,-1,255)", 255, 0, 255, 1.0),
        ("rgb(0, 0, 255)", 0, 0, 255, 1.0),
        ("rgba(0, 0, 255, 100)", 0, 0, 255, 1.0),
        ("hsl(240, 100%, 50%)", 0, 0, 255, 1.0),
        ("hsla(240, 100%, 50%, 0.3)", 0, 0, 255, 0.3),
        // Named colors are ASCII case-insensitive.
        ("Red", 255, 0, 0, 1.0),
        // All-percentage legacy rgb(): 50% -> round(0.5*255) = 128.
        ("rgb(50%, 0%, 100%)", 128, 0, 255, 1.0),
        // Percentages > 100% clamp to 255.
        ("rgb(100%, 0%, 105%)", 255, 0, 255, 1.0),
    ];
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    for &(input, r, g, b, a) in VALID {
        doc.set_inline(el, &format!("background-color: {input}"));
        doc.flush();
        assert_rgba(background_color(&doc, el), r, g, b, a, input);
    }

    // Invalid inputs: `Process == false` / `output.empty()` — nothing is
    // emitted, so background-color stays at its transparent initial.
    const INVALID: &[&str] = &[
        "not color", // garbage
        "#ghff00",   // invalid hex letters g, h
        "#unknow",   // 6 chars, non-hex letters
    ];
    let initial = background_color(&doc, el);
    let _ = initial; // (documented below; the check is per-input)
    for &input in INVALID {
        assert!(
            !parses("background-color", input),
            "`{input}` must be rejected"
        );
        // Applying the invalid value leaves the property unset (transparent).
        doc.set_inline(el, &format!("background-color: {input}"));
        doc.flush();
        assert_rgba(background_color(&doc, el), 0, 0, 0, 0.0, input);
    }
}

// Skipped (disposition): CSSKeywords.TokenTypeCheck — skip-internal. Pure
// tokenizer/internal-data-structure test (keyword string -> Lynx `TokenType`
// enum perfect-hash lookup + enum-count completeness); stylo/cssparser has no
// equivalent public keyword->token enum, and keyword *semantics* are covered by
// the per-property value-grammar tests (named colors above, and unit/gradient/
// transform/timing grammars elsewhere), not a flat lookup table.
