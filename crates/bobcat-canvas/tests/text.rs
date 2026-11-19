//! Text against Ahem, the web-platform-tests font whose every visible glyph
//! is a full em square from the ascent (0.8 em) to the descent (0.2 em), so
//! no assertion depends on a system font.

use std::borrow::Cow;
use std::sync::Arc;

use bobcat_canvas::parley::fontique::Blob;
use bobcat_canvas::parley::{FontContext, FontFamilyName};
use bobcat_canvas::peniko::Color;
use bobcat_canvas::{Canvas, Direction, Font, Style, TextAlign, TextBaseline};

const AHEM: &[u8] = include_bytes!("../../seldon/tests/fixtures/Ahem.ttf");

fn fonts() -> FontContext {
    let mut fonts = FontContext::new();
    fonts
        .collection
        .register_fonts(Blob::new(Arc::new(AHEM.to_vec())), None);
    fonts
}

fn ahem(size: f32) -> Font {
    Font {
        families: Arc::new([FontFamilyName::Named(Cow::Borrowed("Ahem"))]),
        size,
        ..Font::default()
    }
}

fn alpha(canvas: &mut Canvas, x: i32, y: i32) -> u8 {
    canvas.get_image_data(x, y, 1, 1).unwrap()[3]
}

/// The columns of row `y` with any ink.
fn ink_columns(canvas: &mut Canvas, y: i32) -> Option<(i32, i32)> {
    let row = canvas
        .get_image_data(0, y, i32::try_from(canvas.width()).unwrap(), 1)
        .unwrap();
    let inked: Vec<i32> = row
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, px)| px[3] > 0)
        .map(|(x, _)| i32::try_from(x).unwrap())
        .collect();
    Some((*inked.first()?, *inked.last()?))
}

#[test]
fn fill_text_inks_the_em_squares() {
    let mut fonts = fonts();
    let mut canvas = Canvas::new(100, 50);
    canvas.set_font(ahem(20.0));
    canvas.set_fill_style(Style::Color(Color::BLACK));
    canvas.fill_text(&mut fonts, "XX", 10.0, 30.0, None);
    // Two 20 px squares from x = 10, from 16 px above the baseline to 4
    // below.
    assert_eq!(alpha(&mut canvas, 12, 16), 255);
    assert_eq!(alpha(&mut canvas, 48, 32), 255);
    assert_eq!(alpha(&mut canvas, 52, 25), 0);
    assert_eq!(alpha(&mut canvas, 20, 12), 0);
    assert_eq!(alpha(&mut canvas, 20, 36), 0);
}

#[test]
fn measure_text_ahem() {
    let mut fonts = fonts();
    let mut canvas = Canvas::new(10, 10);
    canvas.set_font(ahem(20.0));
    let m = canvas.measure_text(&mut fonts, "XX");
    assert!((m.width - 40.0).abs() < 0.01, "{m:?}");
    assert!(m.actual_bounding_box_left.abs() < 0.01, "{m:?}");
    assert!((m.actual_bounding_box_right - 40.0).abs() < 0.01, "{m:?}");
    assert!((m.actual_bounding_box_ascent - 16.0).abs() < 0.01, "{m:?}");
    assert!((m.actual_bounding_box_descent - 4.0).abs() < 0.01, "{m:?}");
    assert!((m.font_bounding_box_ascent - 16.0).abs() < 0.01, "{m:?}");
    assert!((m.font_bounding_box_descent - 4.0).abs() < 0.01, "{m:?}");
    assert!(m.alphabetic_baseline.abs() < 0.01);
    assert!(
        (m.em_height_ascent + m.em_height_descent - 20.0).abs() < 0.01,
        "{m:?}"
    );
    // Measured from `top`, the em square's top is the baseline line.
    canvas.set_text_baseline(TextBaseline::Top);
    let top = canvas.measure_text(&mut fonts, "XX");
    assert!(top.em_height_ascent.abs() < 0.01, "{top:?}");
    assert!((top.alphabetic_baseline + 16.0).abs() < 0.01, "{top:?}");

    // Trailing spaces count toward the width but not the ink.
    canvas.set_text_baseline(TextBaseline::Alphabetic);
    let spaced = canvas.measure_text(&mut fonts, "X ");
    assert!((spaced.width - 40.0).abs() < 0.01, "{spaced:?}");
    assert!(
        (spaced.actual_bounding_box_right - 20.0).abs() < 0.01,
        "{spaced:?}"
    );
    // Whitespace other than spaces becomes spaces rather than breaking.
    let tabbed = canvas.measure_text(&mut fonts, "X\nX");
    assert!((tabbed.width - 60.0).abs() < 0.01, "{tabbed:?}");
}

#[test]
fn alignment_matches_measure_text() {
    let mut fonts = fonts();
    for (align, direction) in [
        (TextAlign::Left, Direction::Ltr),
        (TextAlign::Center, Direction::Ltr),
        (TextAlign::Right, Direction::Ltr),
        (TextAlign::Start, Direction::Rtl),
        (TextAlign::End, Direction::Inherit),
    ] {
        let mut canvas = Canvas::new(120, 40);
        canvas.set_font(ahem(10.0));
        canvas.set_text_align(align);
        canvas.set_direction(direction);
        let m = canvas.measure_text(&mut fonts, "XXX");
        canvas.fill_text(&mut fonts, "XXX", 60.0, 20.0, None);
        let (first, last) = ink_columns(&mut canvas, 18).unwrap();
        let left = 60.0 - m.actual_bounding_box_left;
        let right = 60.0 + m.actual_bounding_box_right;
        assert!(
            (f64::from(first) - left).abs() <= 1.0,
            "{align:?}: {first} vs {left}"
        );
        assert!(
            (f64::from(last + 1) - right).abs() <= 1.0,
            "{align:?}: {last} vs {right}"
        );
    }
}

#[test]
fn baselines_move_the_text_vertically() {
    let mut fonts = fonts();
    let mut canvas = Canvas::new(60, 60);
    canvas.set_font(ahem(20.0));
    canvas.set_text_baseline(TextBaseline::Top);
    canvas.fill_text(&mut fonts, "X", 10.0, 10.0, None);
    // From `top` the square hangs below y = 10.
    assert_eq!(alpha(&mut canvas, 20, 11), 255);
    assert_eq!(alpha(&mut canvas, 20, 28), 255);
    assert_eq!(alpha(&mut canvas, 20, 8), 0);
    assert_eq!(alpha(&mut canvas, 20, 32), 0);

    canvas.set_text_baseline(TextBaseline::Bottom);
    canvas.fill_text(&mut fonts, "X", 35.0, 50.0, None);
    assert_eq!(alpha(&mut canvas, 45, 48), 255);
    assert_eq!(alpha(&mut canvas, 45, 31), 255);
    assert_eq!(alpha(&mut canvas, 45, 52), 0);
}

#[test]
fn max_width_squeezes() {
    let mut fonts = fonts();
    let mut canvas = Canvas::new(100, 40);
    canvas.set_font(ahem(20.0));
    canvas.fill_text(&mut fonts, "XXXX", 10.0, 30.0, Some(40.0));
    let (first, last) = ink_columns(&mut canvas, 20).unwrap();
    assert_eq!(first, 10);
    assert_eq!(last, 49);
    // A max width of zero draws nothing.
    let mut canvas = Canvas::new(100, 40);
    canvas.set_font(ahem(20.0));
    canvas.fill_text(&mut fonts, "XXXX", 10.0, 30.0, Some(0.0));
    assert!(ink_columns(&mut canvas, 20).is_none());
}

#[test]
fn stroke_text_outlines() {
    let mut fonts = fonts();
    let mut canvas = Canvas::new(60, 60);
    canvas.set_font(ahem(40.0));
    canvas.set_line_width(2.0);
    canvas.stroke_text(&mut fonts, "X", 10.0, 42.0, None);
    // The square spans x 10..50, y 10..50: its edge is inked, its middle
    // is not.
    assert_eq!(alpha(&mut canvas, 10, 30), 255);
    assert_eq!(alpha(&mut canvas, 30, 30), 0);
}

#[test]
fn text_follows_the_transform() {
    let mut fonts = fonts();
    let mut canvas = Canvas::new(80, 40);
    canvas.set_font(ahem(10.0));
    canvas.translate(40.0, 0.0);
    canvas.scale(2.0, 2.0);
    canvas.fill_text(&mut fonts, "X", 0.0, 10.0, None);
    assert_eq!(ink_columns(&mut canvas, 15), Some((40, 59)));
}
