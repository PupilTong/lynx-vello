//! Pixel checks for the drawing surface, away from antialiased edges, read
//! back from the GPU replay.

#![allow(
    clippy::float_cmp,
    reason = "the transforms compared are products of exact binary values"
)]

use terminus::link::html::canvas::{
    Canvas, CanvasError, CompositeOp, FillRule, Gradient, Image, LineCap, Path, Pattern,
    Repetition, Style,
};
use terminus::link::peniko::Color;

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

fn color([r, g, b, a]: [u8; 4]) -> Color {
    Color::from_rgba8(r, g, b, a)
}

fn solid(rgba: [u8; 4]) -> Style {
    Style::Color(color(rgba))
}

/// The unpremultiplied pixel at `(x, y)`.
fn pixel(canvas: &mut Canvas, x: i32, y: i32) -> [u8; 4] {
    let data = canvas.get_image_data(x, y, 1, 1).unwrap();
    [data[0], data[1], data[2], data[3]]
}

fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    canvas.set_fill_style(solid(rgba));
    canvas.fill_rect(0.0, 0.0, f64::from(width), f64::from(height));
    canvas
}

/// A `width` × `height` image of one color.
fn image(width: u32, height: u32, rgba: [u8; 4]) -> Image {
    Image::from_unpremultiplied(width, height, rgba.repeat((width * height) as usize)).unwrap()
}

#[test]
fn canvas_is_send() {
    fn send<T: Send>() {}
    fn send_sync<T: Send + Sync>() {}
    send::<Canvas>();
    send_sync::<Image>();
    send::<Gradient>();
}

#[test]
fn starts_transparent_and_fills_rect() {
    let mut canvas = Canvas::new(20, 20);
    assert_eq!(pixel(&mut canvas, 10, 10), CLEAR);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(5.0, 5.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 10, 10), RED);
    assert_eq!(pixel(&mut canvas, 2, 2), CLEAR);
    assert_eq!(pixel(&mut canvas, 17, 10), CLEAR);
}

#[test]
fn flush_replays_what_is_recorded() {
    let mut canvas = Canvas::new(20, 20);
    assert!(!canvas.has_pending_draws());
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(5.0, 5.0, 10.0, 10.0);
    assert!(canvas.has_pending_draws());
    canvas.flush();
    assert!(!canvas.has_pending_draws());
    assert_eq!(pixel(&mut canvas, 10, 10), RED);
    // A non-source-over draw after a flush still composites onto it.
    canvas.set_global_composite_operation(CompositeOp::DestinationOut);
    canvas.fill_rect(0.0, 0.0, 10.0, 20.0);
    canvas.flush();
    assert_eq!(pixel(&mut canvas, 7, 10), CLEAR);
    assert_eq!(pixel(&mut canvas, 12, 10), RED);
}

#[test]
fn many_draws_between_reads_accumulate() {
    let mut canvas = Canvas::new(40, 4);
    for i in 0..40 {
        let rgba = if i % 2 == 0 { RED } else { BLUE };
        canvas.set_fill_style(solid(rgba));
        canvas.fill_rect(f64::from(i), 0.0, 1.0, 4.0);
    }
    assert_eq!(pixel(&mut canvas, 0, 1), RED);
    assert_eq!(pixel(&mut canvas, 1, 1), BLUE);
    assert_eq!(pixel(&mut canvas, 38, 1), RED);
    // A draw after a read lands on the pixels read.
    canvas.set_fill_style(solid(GREEN));
    canvas.fill_rect(0.0, 0.0, 1.0, 4.0);
    assert_eq!(pixel(&mut canvas, 0, 1), GREEN);
    assert_eq!(pixel(&mut canvas, 1, 1), BLUE);
}

#[test]
fn fill_rules() {
    // Two squares, the inner one wound the same way.
    let mut canvas = Canvas::new(30, 30);
    canvas.set_fill_style(solid(RED));
    canvas.rect(0.0, 0.0, 30.0, 30.0);
    canvas.rect(10.0, 10.0, 10.0, 10.0);
    canvas.fill(FillRule::EvenOdd);
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
    assert_eq!(pixel(&mut canvas, 15, 15), CLEAR);
    canvas.fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 15, 15), RED);
}

#[test]
fn open_subpaths_fill_closed() {
    let mut canvas = Canvas::new(20, 20);
    canvas.set_fill_style(solid(RED));
    canvas.move_to(0.0, 0.0);
    canvas.line_to(20.0, 0.0);
    canvas.line_to(20.0, 20.0);
    canvas.fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 15, 3), RED);
    assert_eq!(pixel(&mut canvas, 3, 15), CLEAR);
}

#[test]
fn begin_path_discards_subpaths() {
    let mut canvas = Canvas::new(20, 20);
    canvas.set_fill_style(solid(RED));
    canvas.rect(0.0, 0.0, 10.0, 10.0);
    canvas.begin_path();
    canvas.rect(10.0, 10.0, 10.0, 10.0);
    canvas.fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 15, 15), RED);
}

#[test]
fn strokes_and_caps() {
    let mut canvas = Canvas::new(40, 20);
    canvas.set_stroke_style(solid(BLUE));
    canvas.set_line_width(6.0);
    canvas.move_to(10.0, 10.0);
    canvas.line_to(30.0, 10.0);
    canvas.stroke();
    assert_eq!(pixel(&mut canvas, 20, 10), BLUE);
    assert_eq!(pixel(&mut canvas, 20, 15), CLEAR);
    // Butt caps end at the endpoints.
    assert_eq!(pixel(&mut canvas, 8, 10), CLEAR);

    canvas.set_line_cap(LineCap::Square);
    canvas.set_stroke_style(solid(RED));
    canvas.stroke();
    assert_eq!(pixel(&mut canvas, 8, 10), RED);
    assert_eq!(pixel(&mut canvas, 32, 10), RED);
    assert_eq!(pixel(&mut canvas, 35, 10), CLEAR);
}

#[test]
fn stroke_width_follows_the_transform_at_stroke_time() {
    let mut canvas = Canvas::new(40, 40);
    canvas.set_stroke_style(solid(RED));
    canvas.set_line_width(2.0);
    canvas.move_to(5.0, 20.0);
    canvas.line_to(35.0, 20.0);
    // The path is already in canvas space; scaling now widens the line.
    canvas.scale(1.0, 5.0);
    canvas.stroke();
    assert_eq!(pixel(&mut canvas, 20, 24), RED);
    assert_eq!(pixel(&mut canvas, 20, 27), CLEAR);
}

#[test]
fn stroke_rect_and_invalid_line_values() {
    let mut canvas = Canvas::new(30, 30);
    canvas.set_line_width(0.0);
    canvas.set_line_width(-1.0);
    canvas.set_line_width(f64::NAN);
    assert!((canvas.line_width() - 1.0).abs() < f64::EPSILON);
    canvas.set_line_width(4.0);
    canvas.set_stroke_style(solid(GREEN));
    canvas.stroke_rect(5.0, 5.0, 20.0, 20.0);
    assert_eq!(pixel(&mut canvas, 5, 15), GREEN);
    assert_eq!(pixel(&mut canvas, 15, 15), CLEAR);
}

#[test]
fn dashes() {
    let mut canvas = Canvas::new(40, 10);
    canvas.set_line_dash(&[4.0, 4.0]);
    canvas.set_stroke_style(solid(RED));
    canvas.set_line_width(4.0);
    canvas.move_to(0.0, 5.0);
    canvas.line_to(40.0, 5.0);
    canvas.stroke();
    assert_eq!(pixel(&mut canvas, 2, 5), RED);
    assert_eq!(pixel(&mut canvas, 6, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 10, 5), RED);

    canvas.set_line_dash(&[1.0, 2.0, 3.0]);
    assert_eq!(canvas.line_dash(), [1.0, 2.0, 3.0, 1.0, 2.0, 3.0]);
    canvas.set_line_dash(&[1.0, -2.0]);
    canvas.set_line_dash(&[1.0, f64::INFINITY]);
    assert_eq!(canvas.line_dash(), [1.0, 2.0, 3.0, 1.0, 2.0, 3.0]);
}

#[test]
fn dash_offset_shifts_the_pattern() {
    let mut canvas = Canvas::new(40, 10);
    canvas.set_line_dash(&[4.0, 4.0]);
    canvas.set_line_dash_offset(4.0);
    canvas.set_stroke_style(solid(RED));
    canvas.set_line_width(4.0);
    canvas.move_to(0.0, 5.0);
    canvas.line_to(40.0, 5.0);
    canvas.stroke();
    assert_eq!(pixel(&mut canvas, 2, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 6, 5), RED);
}

#[test]
fn clip_restricts_and_restore_lifts_it() {
    let mut canvas = Canvas::new(30, 30);
    canvas.save();
    canvas.rect(10.0, 10.0, 10.0, 10.0);
    canvas.clip(FillRule::NonZero);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(0.0, 0.0, 30.0, 30.0);
    assert_eq!(pixel(&mut canvas, 15, 15), RED);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    canvas.restore();
    // The fill style was saved too: black again.
    canvas.fill_rect(0.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 2, 2), [0, 0, 0, 255]);
}

#[test]
fn clips_intersect() {
    let mut canvas = Canvas::new(30, 30);
    let mut a = Path::new();
    a.rect(0.0, 0.0, 20.0, 20.0);
    let mut b = Path::new();
    b.rect(10.0, 10.0, 20.0, 20.0);
    canvas.clip_path(&a, FillRule::NonZero);
    canvas.clip_path(&b, FillRule::NonZero);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(0.0, 0.0, 30.0, 30.0);
    assert_eq!(pixel(&mut canvas, 15, 15), RED);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 25, 25), CLEAR);
}

#[test]
fn empty_clip_clips_everything() {
    let mut canvas = Canvas::new(10, 10);
    canvas.clip(FillRule::NonZero);
    canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
}

#[test]
fn transforms() {
    let mut canvas = Canvas::new(40, 40);
    canvas.set_fill_style(solid(RED));
    canvas.translate(20.0, 0.0);
    canvas.scale(2.0, 2.0);
    canvas.fill_rect(0.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 25, 5), RED);
    assert_eq!(pixel(&mut canvas, 15, 5), CLEAR);
    assert_eq!(canvas.get_transform(), [2.0, 0.0, 0.0, 2.0, 20.0, 0.0]);

    canvas.set_transform(1.0, 0.0, 0.0, 1.0, f64::NAN, 0.0);
    canvas.scale(f64::INFINITY, 1.0);
    assert_eq!(canvas.get_transform(), [2.0, 0.0, 0.0, 2.0, 20.0, 0.0]);

    // A quarter turn clockwise about (20, 20).
    canvas.reset_transform();
    canvas.translate(20.0, 20.0);
    canvas.rotate(std::f64::consts::FRAC_PI_2);
    canvas.set_fill_style(solid(BLUE));
    canvas.fill_rect(0.0, 0.0, 10.0, 4.0);
    assert_eq!(pixel(&mut canvas, 18, 25), BLUE);
    assert_eq!(pixel(&mut canvas, 25, 22), CLEAR);

    canvas.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 30.0);
    canvas.transform(1.0, 0.0, 0.0, 1.0, 5.0, 0.0);
    canvas.set_fill_style(solid(GREEN));
    canvas.fill_rect(0.0, 0.0, 4.0, 4.0);
    assert_eq!(pixel(&mut canvas, 7, 32), GREEN);
}

#[test]
fn default_path_keeps_the_transform_it_was_built_under() {
    let mut canvas = Canvas::new(40, 20);
    canvas.translate(20.0, 0.0);
    canvas.rect(0.0, 0.0, 10.0, 10.0);
    canvas.reset_transform();
    canvas.set_fill_style(solid(RED));
    canvas.fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 25, 5), RED);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);

    // A Path2D takes the transform at draw time.
    let mut path = Path::new();
    path.rect(0.0, 0.0, 10.0, 10.0);
    canvas.translate(0.0, 10.0);
    canvas.set_fill_style(solid(BLUE));
    canvas.fill_path(&path, FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 5, 15), BLUE);
}

#[test]
fn global_alpha() {
    let mut canvas = Canvas::new(10, 10);
    canvas.set_global_alpha(2.0);
    canvas.set_global_alpha(-0.5);
    canvas.set_global_alpha(f64::NAN);
    assert!((canvas.global_alpha() - 1.0).abs() < f64::EPSILON);
    canvas.set_global_alpha(0.5);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
    let [r, g, b, a] = pixel(&mut canvas, 5, 5);
    assert_eq!([r, g, b], [255, 0, 0]);
    assert!((127..=128).contains(&a), "alpha {a}");
}

#[test]
fn linear_gradient_and_late_stops() {
    let gradient = Gradient::linear(0.0, 0.0, 100.0, 0.0).unwrap();
    let mut canvas = Canvas::new(100, 4);
    canvas.set_fill_style(Style::Gradient(gradient.clone()));
    // No stops yet: transparent black, nothing drawn.
    canvas.fill_rect(0.0, 0.0, 100.0, 4.0);
    assert_eq!(pixel(&mut canvas, 50, 2), CLEAR);
    // Stops added to the shared gradient show in later draws.
    gradient.add_color_stop(0.0, color(RED)).unwrap();
    gradient.add_color_stop(1.0, color(BLUE)).unwrap();
    canvas.fill_rect(0.0, 0.0, 100.0, 4.0);
    let left = pixel(&mut canvas, 1, 2);
    let right = pixel(&mut canvas, 98, 2);
    assert!(left[0] > 240 && left[2] < 15, "{left:?}");
    assert!(right[2] > 240 && right[0] < 15, "{right:?}");
    let middle = pixel(&mut canvas, 50, 2);
    assert!(
        (110..=145).contains(&middle[0]) && (110..=145).contains(&middle[2]),
        "{middle:?}"
    );

    assert_eq!(
        gradient.add_color_stop(1.5, color(RED)),
        Err(CanvasError::IndexSize)
    );
    assert_eq!(
        gradient.add_color_stop(f64::NAN, color(RED)),
        Err(CanvasError::Type)
    );
}

#[test]
fn gradient_interpolates_unpremultiplied() {
    // Transparent green to opaque blue: halfway is half-transparent
    // cyan-ish, not half-transparent blue.
    let gradient = Gradient::linear(0.0, 0.0, 100.0, 0.0).unwrap();
    gradient.add_color_stop(0.0, color([0, 255, 0, 0])).unwrap();
    gradient.add_color_stop(1.0, color(BLUE)).unwrap();
    let mut canvas = Canvas::new(100, 2);
    canvas.set_fill_style(Style::Gradient(gradient));
    canvas.fill_rect(0.0, 0.0, 100.0, 2.0);
    let middle = pixel(&mut canvas, 50, 1);
    assert!(middle[1] > 100, "{middle:?}");
}

#[test]
fn degenerate_gradients_paint_nothing() {
    let linear = Gradient::linear(5.0, 5.0, 5.0, 5.0).unwrap();
    linear.add_color_stop(0.0, color(RED)).unwrap();
    let radial = Gradient::radial(5.0, 5.0, 3.0, 5.0, 5.0, 3.0).unwrap();
    radial.add_color_stop(0.0, color(RED)).unwrap();
    for gradient in [linear, radial] {
        let mut canvas = Canvas::new(10, 10);
        canvas.set_fill_style(Style::Gradient(gradient));
        canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
        assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    }
    assert_eq!(
        Gradient::radial(0.0, 0.0, -1.0, 0.0, 0.0, 1.0).err(),
        Some(CanvasError::IndexSize)
    );
    assert_eq!(
        Gradient::linear(f64::NAN, 0.0, 1.0, 1.0).err(),
        Some(CanvasError::Type)
    );
}

#[test]
fn radial_gradient() {
    let gradient = Gradient::radial(50.0, 50.0, 0.0, 50.0, 50.0, 40.0).unwrap();
    gradient.add_color_stop(0.0, color(RED)).unwrap();
    gradient.add_color_stop(1.0, color(BLUE)).unwrap();
    let mut canvas = Canvas::new(100, 100);
    canvas.set_fill_style(Style::Gradient(gradient));
    canvas.fill_rect(0.0, 0.0, 100.0, 100.0);
    let center = pixel(&mut canvas, 50, 50);
    assert!(center[0] > 240, "{center:?}");
    // Past the outer circle the last stop pads.
    assert_eq!(pixel(&mut canvas, 98, 50), BLUE);
}

#[test]
fn conic_gradient_starts_at_the_angle_clockwise() {
    // Red over the first quarter turn clockwise from +x, blue after.
    let gradient = Gradient::conic(0.0, 50.0, 50.0).unwrap();
    gradient.add_color_stop(0.0, color(RED)).unwrap();
    gradient.add_color_stop(0.25, color(RED)).unwrap();
    gradient.add_color_stop(0.25, color(BLUE)).unwrap();
    gradient.add_color_stop(1.0, color(BLUE)).unwrap();
    let mut canvas = Canvas::new(100, 100);
    canvas.set_fill_style(Style::Gradient(gradient));
    canvas.fill_rect(0.0, 0.0, 100.0, 100.0);
    // Below-right of the center is within the first quarter (y is down).
    assert_eq!(pixel(&mut canvas, 80, 70), RED);
    assert_eq!(pixel(&mut canvas, 20, 70), BLUE);
    assert_eq!(pixel(&mut canvas, 80, 30), BLUE);
}

#[test]
fn patterns() {
    // A 2×2 checker: red on the diagonal, blue off it.
    let checker = Image::from_unpremultiplied(2, 2, [RED, BLUE, BLUE, RED].concat()).unwrap();

    let mut canvas = Canvas::new(8, 8);
    canvas.set_image_smoothing_enabled(false);
    let pattern = Pattern::new(checker.clone(), Repetition::Repeat).unwrap();
    canvas.set_fill_style(Style::Pattern(pattern));
    canvas.fill_rect(0.0, 0.0, 8.0, 8.0);
    assert_eq!(pixel(&mut canvas, 4, 4), RED);
    assert_eq!(pixel(&mut canvas, 5, 4), BLUE);
    assert_eq!(pixel(&mut canvas, 7, 7), RED);

    let mut canvas = Canvas::new(8, 8);
    canvas.set_image_smoothing_enabled(false);
    let pattern = Pattern::new(checker.clone(), Repetition::NoRepeat).unwrap();
    canvas.set_fill_style(Style::Pattern(pattern.clone()));
    canvas.fill_rect(0.0, 0.0, 8.0, 8.0);
    assert_eq!(pixel(&mut canvas, 0, 0), RED);
    assert_eq!(pixel(&mut canvas, 1, 0), BLUE);
    assert_eq!(pixel(&mut canvas, 4, 4), CLEAR);
    assert_eq!(pixel(&mut canvas, 4, 0), CLEAR);

    // The pattern transform moves the tile; shared by clones.
    pattern
        .set_transform([1.0, 0.0, 0.0, 1.0, 4.0, 4.0])
        .unwrap();
    canvas.fill_rect(0.0, 0.0, 8.0, 8.0);
    assert_eq!(pixel(&mut canvas, 4, 4), RED);
    assert_eq!(pixel(&mut canvas, 5, 4), BLUE);

    let mut canvas = Canvas::new(8, 8);
    canvas.set_image_smoothing_enabled(false);
    let pattern = Pattern::new(checker, Repetition::RepeatX).unwrap();
    canvas.set_fill_style(Style::Pattern(pattern));
    canvas.fill_rect(0.0, 0.0, 8.0, 8.0);
    assert_eq!(pixel(&mut canvas, 6, 0), RED);
    assert_eq!(pixel(&mut canvas, 7, 1), RED);
    assert_eq!(pixel(&mut canvas, 6, 4), CLEAR);

    assert_eq!(Repetition::from_pattern_name(""), Some(Repetition::Repeat));
    assert_eq!(Repetition::from_pattern_name("repeat-z"), None);
    assert_eq!(
        Pattern::new(image(0, 0, RED), Repetition::Repeat).err(),
        Some(CanvasError::InvalidState)
    );
}

#[test]
fn composite_source_in_clears_outside_the_shape() {
    let mut canvas = filled(20, 20, BLUE);
    canvas.set_global_composite_operation(CompositeOp::SourceIn);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(0.0, 0.0, 10.0, 20.0);
    assert_eq!(pixel(&mut canvas, 5, 10), RED);
    assert_eq!(pixel(&mut canvas, 15, 10), CLEAR);
}

#[test]
fn composite_copy_respects_the_clip() {
    let mut canvas = filled(20, 20, BLUE);
    canvas.rect(0.0, 0.0, 20.0, 10.0);
    canvas.clip(FillRule::NonZero);
    canvas.set_global_composite_operation(CompositeOp::Copy);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(0.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 2, 2), RED);
    // Inside the clip, outside the shape: replaced by transparent.
    assert_eq!(pixel(&mut canvas, 15, 5), CLEAR);
    // Outside the clip: untouched.
    assert_eq!(pixel(&mut canvas, 15, 15), BLUE);
}

#[test]
fn composite_destination_ops_and_xor() {
    let mut canvas = filled(20, 20, BLUE);
    canvas.set_global_composite_operation(CompositeOp::DestinationOut);
    canvas.fill_rect(0.0, 0.0, 10.0, 20.0);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 15, 5), BLUE);

    canvas.set_global_composite_operation(CompositeOp::DestinationOver);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(0.0, 0.0, 20.0, 20.0);
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
    assert_eq!(pixel(&mut canvas, 15, 5), BLUE);

    let mut canvas = filled(20, 20, BLUE);
    canvas.set_global_composite_operation(CompositeOp::Xor);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 15, 15), BLUE);
}

#[test]
fn composite_blend_modes() {
    let mut canvas = filled(10, 10, [255, 255, 0, 255]);
    canvas.set_global_composite_operation(CompositeOp::Multiply);
    canvas.set_fill_style(solid([0, 255, 255, 255]));
    canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 5, 5), GREEN);

    let mut canvas = filled(10, 10, [255, 0, 0, 255]);
    canvas.set_global_composite_operation(CompositeOp::Lighter);
    canvas.set_fill_style(solid([0, 0, 255, 255]));
    canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 5, 5), [255, 0, 255, 255]);

    assert_eq!(
        CompositeOp::from_name("hard-light"),
        Some(CompositeOp::HardLight)
    );
    assert_eq!(CompositeOp::from_name("plus-lighter"), None);
    assert_eq!(CompositeOp::Luminosity.name(), "luminosity");
}

#[test]
fn source_over_after_a_destructive_op_keeps_both() {
    let mut canvas = filled(20, 20, BLUE);
    canvas.set_global_composite_operation(CompositeOp::DestinationOut);
    canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
    canvas.set_global_composite_operation(CompositeOp::SourceOver);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(15.0, 15.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 12, 5), BLUE);
    assert_eq!(pixel(&mut canvas, 17, 17), RED);
}

#[test]
fn clear_rect_ignores_composite_and_alpha() {
    let mut canvas = filled(20, 20, BLUE);
    canvas.set_global_composite_operation(CompositeOp::DestinationOver);
    canvas.set_global_alpha(0.1);
    canvas.clear_rect(5.0, 5.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 10, 10), CLEAR);
    assert_eq!(pixel(&mut canvas, 2, 2), BLUE);
}

#[test]
fn clear_rect_respects_clip_and_whole_canvas_clear() {
    let mut canvas = filled(20, 20, BLUE);
    canvas.save();
    canvas.rect(0.0, 0.0, 10.0, 20.0);
    canvas.clip(FillRule::NonZero);
    canvas.clear_rect(0.0, 0.0, 20.0, 20.0);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 15, 5), BLUE);
    canvas.restore();
    canvas.clear_rect(-5.0, -5.0, 40.0, 40.0);
    assert_eq!(pixel(&mut canvas, 15, 5), CLEAR);
}

#[test]
fn draw_image_forms() {
    let mut canvas = Canvas::new(30, 30);
    canvas.set_image_smoothing_enabled(false);
    let red = image(4, 4, RED);
    canvas.draw_image(&red, 2.0, 2.0).unwrap();
    assert_eq!(pixel(&mut canvas, 3, 3), RED);
    assert_eq!(pixel(&mut canvas, 7, 3), CLEAR);

    canvas
        .draw_image_scaled(&image(2, 2, BLUE), 10.0, 10.0, 8.0, 8.0)
        .unwrap();
    assert_eq!(pixel(&mut canvas, 16, 16), BLUE);
    assert_eq!(pixel(&mut canvas, 19, 16), CLEAR);

    // Source rectangle half outside the image: clipped, and the destination
    // with it.
    let left_red = Image::from_unpremultiplied(2, 1, [RED, GREEN].concat()).unwrap();
    canvas
        .draw_image_subrect(&left_red, 1.0, 0.0, 2.0, 1.0, 20.0, 0.0, 8.0, 4.0)
        .unwrap();
    assert_eq!(pixel(&mut canvas, 21, 2), GREEN);
    assert_eq!(pixel(&mut canvas, 25, 2), CLEAR);

    assert_eq!(
        canvas.draw_image(&image(0, 0, RED), 0.0, 0.0),
        Err(CanvasError::InvalidState)
    );
}

#[test]
fn draw_image_honours_transform_alpha_and_clip() {
    let mut canvas = Canvas::new(20, 20);
    canvas.rect(0.0, 0.0, 20.0, 10.0);
    canvas.clip(FillRule::NonZero);
    canvas.translate(5.0, 5.0);
    canvas.set_global_alpha(0.5);
    canvas.draw_image(&image(10, 10, RED), 0.0, 0.0).unwrap();
    let [r, _, _, a] = pixel(&mut canvas, 8, 7);
    assert_eq!(r, 255);
    assert!((127..=128).contains(&a), "alpha {a}");
    assert_eq!(pixel(&mut canvas, 8, 12), CLEAR);
    assert_eq!(pixel(&mut canvas, 2, 2), CLEAR);
}

#[test]
fn canvas_draws_itself() {
    let mut canvas = Canvas::new(20, 10);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
    let own = canvas.image();
    let kept = own.crop(None).unwrap();
    canvas.draw_image(&own, 10.0, 0.0).unwrap();
    // Drawing over the left half afterwards reaches neither the right half
    // nor the copy kept as an image bitmap is.
    canvas.set_fill_style(solid(BLUE));
    canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 15, 5), RED);
    assert_eq!(pixel(&mut canvas, 5, 5), BLUE);
    assert_eq!(kept.read().unwrap()[..4], RED);
}

#[test]
fn one_canvas_draws_another_as_it_was_at_the_call() {
    let mut source = filled(4, 4, RED);
    let mut target = Canvas::new(8, 4);
    target.set_image_smoothing_enabled(false);
    target.draw_image(&source.image(), 0.0, 0.0).unwrap();
    // Recorded on the source after the draw: not in the target's copy.
    source.set_fill_style(solid(BLUE));
    source.fill_rect(0.0, 0.0, 4.0, 4.0);
    target.draw_image(&source.image(), 4.0, 0.0).unwrap();
    assert_eq!(pixel(&mut target, 1, 1), RED);
    assert_eq!(pixel(&mut target, 5, 1), BLUE);
    // A pattern copies the canvas as it is when made.
    let pattern = Pattern::new(source.image(), Repetition::Repeat).unwrap();
    source.clear_rect(0.0, 0.0, 4.0, 4.0);
    target.set_fill_style(Style::Pattern(pattern));
    target.fill_rect(0.0, 0.0, 8.0, 4.0);
    assert_eq!(pixel(&mut target, 1, 1), BLUE);
}

#[test]
fn image_bitmaps_from_pixels_and_canvases_crop() {
    let pixels = Image::from_unpremultiplied(2, 1, [[255, 255, 255, 128], GREEN].concat()).unwrap();
    let cropped = pixels.crop(Some((1, 0, 2, 1))).unwrap();
    assert_eq!(cropped.read().unwrap(), [GREEN, CLEAR].concat());
    let mut canvas = filled(3, 1, BLUE);
    let from_canvas = canvas.image().crop(Some((-1, 0, 2, 1))).unwrap();
    assert_eq!(from_canvas.read().unwrap(), [CLEAR, BLUE].concat());
    assert_eq!((from_canvas.width(), from_canvas.height()), (2, 1));
}

#[test]
fn a_long_recording_is_replayed_before_a_read() {
    // Enough draws to pass the recording bound several times over.
    let mut canvas = Canvas::new(64, 1);
    for i in 0..40_000 {
        let rgba = if i % 2 == 0 { RED } else { BLUE };
        canvas.set_fill_style(solid(rgba));
        canvas.fill_rect(f64::from(i % 64), 0.0, 1.0, 1.0);
    }
    assert!(canvas.has_pending_draws());
    // The last pass over the row: even draws red, odd ones blue.
    assert_eq!(pixel(&mut canvas, 0, 0), RED);
    assert_eq!(pixel(&mut canvas, 1, 0), BLUE);
}

#[test]
fn get_and_put_image_data() {
    let mut canvas = Canvas::new(4, 4);
    // Half-transparent white premultiplies on the way in and comes back.
    let data = [[255, 255, 255, 128], [10, 20, 30, 255]].concat();
    canvas.put_image_data(&data, 2, 1, 1, 1, None).unwrap();
    let back = canvas.get_image_data(1, 1, 2, 1).unwrap();
    assert_eq!(back[4..], [10, 20, 30, 255]);
    assert!(back[..3].iter().all(|c| *c >= 254), "{back:?}");
    assert_eq!(back[3], 128);
    // Premultiplied in the bitmap.
    let bitmap = canvas.image().read().unwrap();
    let at = (4 + 1) * 4;
    assert_eq!(bitmap[at + 3], 128);
    assert!((127..=128).contains(&bitmap[at]));

    // Negative sizes read the same rectangle from its other corner; pixels
    // outside the bitmap are transparent black.
    let read = canvas.get_image_data(3, 2, -3, -2).unwrap();
    assert_eq!(read.len(), 3 * 2 * 4);
    assert_eq!(read[(3 + 2) * 4..(3 + 2) * 4 + 4], [10, 20, 30, 255]);
    let outside = canvas.get_image_data(-2, -2, 2, 2).unwrap();
    assert!(outside.iter().all(|byte| *byte == 0));
    assert_eq!(
        canvas.get_image_data(0, 0, 0, 1),
        Err(CanvasError::IndexSize)
    );
}

#[test]
fn put_image_data_replaces_and_ignores_state() {
    let mut canvas = filled(4, 4, BLUE);
    canvas.set_global_alpha(0.2);
    canvas.set_global_composite_operation(CompositeOp::Xor);
    canvas.translate(2.0, 2.0);
    canvas.clip(FillRule::NonZero);
    let data = [CLEAR, RED, RED, RED].concat();
    // Only the dirty rectangle's second column, at dx = 1.
    canvas
        .put_image_data(&data, 2, 2, 1, 0, Some([1, 0, 1, 2]))
        .unwrap();
    assert_eq!(pixel(&mut canvas, 2, 0), RED);
    assert_eq!(pixel(&mut canvas, 2, 1), RED);
    assert_eq!(pixel(&mut canvas, 1, 0), BLUE);
    // A transparent source pixel replaces rather than composites.
    canvas.put_image_data(&data, 2, 2, 0, 0, None).unwrap();
    assert_eq!(pixel(&mut canvas, 0, 0), CLEAR);
    assert_eq!(
        canvas.put_image_data(&data[..4], 2, 2, 0, 0, None),
        Err(CanvasError::InvalidState)
    );
}

#[test]
fn shadows() {
    let mut canvas = Canvas::new(40, 40);
    canvas.set_shadow_color(color(BLUE));
    canvas.set_shadow_offset_x(15.0);
    canvas.set_shadow_offset_y(15.0);
    // Offsets are in canvas space whatever the transform.
    canvas.scale(2.0, 2.0);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(0.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
    assert_eq!(pixel(&mut canvas, 20, 20), BLUE);
    assert_eq!(pixel(&mut canvas, 30, 30), CLEAR);
    // A transparent shadow color casts nothing.
    canvas.set_shadow_color(Color::TRANSPARENT);
    canvas.fill_rect(10.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 35, 25), CLEAR);
}

#[test]
fn shadow_blur_spreads_and_is_clipped() {
    let mut canvas = Canvas::new(60, 80);
    canvas.rect(0.0, 0.0, 60.0, 60.0);
    canvas.clip(FillRule::NonZero);
    canvas.set_shadow_color(color(BLUE));
    // σ = 4.
    canvas.set_shadow_blur(8.0);
    canvas.set_shadow_offset_y(30.0);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(10.0, 5.0, 40.0, 40.0);
    assert_eq!(pixel(&mut canvas, 30, 20), RED);
    // Inside the shadow by more than 3σ, outside the shape.
    assert_eq!(pixel(&mut canvas, 30, 50), BLUE);
    // Blurred past the shadow's edge.
    let edge = pixel(&mut canvas, 52, 50);
    assert!(edge[3] > 0 && edge[3] < 255, "{edge:?}");
    // Below the clip.
    assert_eq!(pixel(&mut canvas, 30, 65), CLEAR);
}

#[test]
fn hit_tests() {
    let mut canvas = Canvas::new(40, 40);
    canvas.translate(10.0, 10.0);
    canvas.rect(0.0, 0.0, 10.0, 10.0);
    canvas.reset_transform();
    assert!(canvas.is_point_in_path(15.0, 15.0, FillRule::NonZero));
    assert!(!canvas.is_point_in_path(5.0, 5.0, FillRule::NonZero));
    assert!(!canvas.is_point_in_path(f64::NAN, 5.0, FillRule::NonZero));

    let mut path = Path::new();
    path.rect(0.0, 0.0, 10.0, 10.0);
    canvas.translate(20.0, 20.0);
    assert!(canvas.is_point_in_path2d(&path, 25.0, 25.0, FillRule::EvenOdd));
    assert!(!canvas.is_point_in_path2d(&path, 5.0, 5.0, FillRule::EvenOdd));

    canvas.set_line_width(4.0);
    assert!(canvas.is_point_in_stroke_path(&path, 21.0, 25.0));
    assert!(!canvas.is_point_in_stroke_path(&path, 25.0, 25.0));
    canvas.reset_transform();
    assert!(canvas.is_point_in_stroke(10.0, 15.0));
    assert!(!canvas.is_point_in_stroke(15.0, 15.0));
}

#[test]
fn arcs_and_ellipses() {
    let mut canvas = Canvas::new(40, 40);
    canvas.set_fill_style(solid(RED));
    canvas
        .arc(20.0, 20.0, 10.0, 0.0, std::f64::consts::TAU, false)
        .unwrap();
    canvas.fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 20, 20), RED);
    assert_eq!(pixel(&mut canvas, 20, 12), RED);
    assert_eq!(pixel(&mut canvas, 12, 12), CLEAR);
    assert_eq!(
        canvas.arc(0.0, 0.0, -1.0, 0.0, 1.0, false),
        Err(CanvasError::IndexSize)
    );

    // Half a disc: clockwise from 0 to π covers the lower half (y down).
    let mut canvas = Canvas::new(40, 40);
    canvas.set_fill_style(solid(RED));
    canvas
        .arc(20.0, 20.0, 10.0, 0.0, std::f64::consts::PI, false)
        .unwrap();
    canvas.fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 20, 25), RED);
    assert_eq!(pixel(&mut canvas, 20, 15), CLEAR);
    // Anticlockwise over the same angles covers the upper half.
    canvas.begin_path();
    canvas.set_fill_style(solid(BLUE));
    canvas
        .arc(20.0, 20.0, 10.0, 0.0, std::f64::consts::PI, true)
        .unwrap();
    canvas.fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 20, 15), BLUE);
    assert_eq!(pixel(&mut canvas, 20, 25), RED);

    let mut canvas = Canvas::new(40, 40);
    canvas.set_fill_style(solid(GREEN));
    canvas
        .ellipse(
            20.0,
            20.0,
            15.0,
            5.0,
            0.0,
            0.0,
            std::f64::consts::TAU,
            false,
        )
        .unwrap();
    canvas.fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 32, 20), GREEN);
    assert_eq!(pixel(&mut canvas, 20, 28), CLEAR);
}

#[test]
fn arc_to_rounds_a_corner() {
    let mut canvas = Canvas::new(40, 40);
    canvas.set_fill_style(solid(RED));
    canvas.move_to(0.0, 0.0);
    canvas.arc_to(30.0, 0.0, 30.0, 30.0, 10.0).unwrap();
    canvas.line_to(30.0, 30.0);
    canvas.line_to(0.0, 30.0);
    canvas.fill(FillRule::NonZero);
    // The corner at (30, 0) is cut by the arc.
    assert_eq!(pixel(&mut canvas, 29, 1), CLEAR);
    assert_eq!(pixel(&mut canvas, 15, 2), RED);
    assert_eq!(pixel(&mut canvas, 28, 15), RED);
    assert_eq!(
        canvas.arc_to(0.0, 0.0, 1.0, 1.0, -1.0),
        Err(CanvasError::IndexSize)
    );
}

#[test]
fn round_rect() {
    let mut canvas = Canvas::new(40, 40);
    canvas.set_fill_style(solid(RED));
    canvas
        .round_rect(5.0, 5.0, 30.0, 30.0, &[[10.0, 10.0]])
        .unwrap();
    canvas.fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 6, 6), CLEAR);
    assert_eq!(pixel(&mut canvas, 20, 6), RED);
    assert_eq!(pixel(&mut canvas, 20, 20), RED);

    // A negative width mirrors: the one rounded corner (upper-left) lands
    // on the right.
    let mut canvas = Canvas::new(40, 40);
    canvas.set_fill_style(solid(RED));
    canvas
        .round_rect(
            35.0,
            5.0,
            -30.0,
            30.0,
            &[[10.0, 10.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
        )
        .unwrap();
    canvas.fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 34, 6), CLEAR);
    assert_eq!(pixel(&mut canvas, 6, 6), RED);

    assert_eq!(
        canvas.round_rect(0.0, 0.0, 1.0, 1.0, &[]),
        Err(CanvasError::Range)
    );
    assert_eq!(
        canvas.round_rect(0.0, 0.0, 1.0, 1.0, &[[-1.0, 0.0]]),
        Err(CanvasError::Range)
    );
}

#[test]
fn path2d_from_svg_and_add_path() {
    let svg = Path::from_svg("M0 0 H10 V10 H0 Z");
    let mut path = Path::new();
    path.add_path(&svg, [1.0, 0.0, 0.0, 1.0, 20.0, 0.0]);
    let mut canvas = Canvas::new(40, 20);
    canvas.set_fill_style(solid(RED));
    canvas.fill_path(&svg, FillRule::NonZero);
    canvas.set_fill_style(solid(BLUE));
    canvas.fill_path(&path, FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
    assert_eq!(pixel(&mut canvas, 25, 5), BLUE);
    assert_eq!(pixel(&mut canvas, 15, 5), CLEAR);
}

#[test]
fn resize_resets_bitmap_and_state() {
    let mut canvas = Canvas::new(10, 10);
    canvas.set_fill_style(solid(RED));
    canvas.translate(1.0, 1.0);
    canvas.save();
    canvas.fill_rect(0.0, 0.0, 5.0, 5.0);
    canvas.set_size(10, 10);
    assert_eq!(pixel(&mut canvas, 3, 3), CLEAR);
    assert_eq!(canvas.get_transform(), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    canvas.fill_rect(0.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 0, 0), [0, 0, 0, 255]);

    canvas.set_size(20, 5);
    assert_eq!((canvas.width(), canvas.height()), (20, 5));
    assert_eq!(pixel(&mut canvas, 0, 0), CLEAR);
    canvas.fill_rect(15.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 17, 2), [0, 0, 0, 255]);
}

#[test]
fn zero_sized_canvas_draws_nothing() {
    let mut canvas = Canvas::new(0, 10);
    canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
    assert!(!canvas.has_pending_draws());
    assert_eq!(canvas.get_image_data(0, 0, 1, 1).unwrap(), CLEAR);
    assert_eq!(canvas.image().width(), 0);
    // Past the longest texture side: no bitmap either.
    let mut canvas = Canvas::new(8193, 1);
    canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(canvas.get_image_data(0, 0, 1, 1).unwrap(), CLEAR);
}

#[test]
fn take_image_leaves_transparent_and_keeps_state() {
    let mut canvas = Canvas::new(10, 10);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
    let taken = canvas.take_image();
    assert_eq!(taken.read().unwrap()[..4], RED);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    canvas.fill_rect(0.0, 0.0, 1.0, 1.0);
    assert_eq!(pixel(&mut canvas, 0, 0), RED);
    // The taken image is not drawn into afterwards.
    assert_eq!(taken.read().unwrap()[..4], RED);
    let mut other = Canvas::new(10, 10);
    other.draw_image(&taken, 0.0, 0.0).unwrap();
    assert_eq!(pixel(&mut other, 5, 5), RED);
}

#[test]
fn changes_are_tracked_until_a_commit() {
    let mut canvas = Canvas::new(10, 10);
    assert!(!canvas.has_changes());
    canvas.fill_rect(0.0, 0.0, 1.0, 1.0);
    assert!(canvas.has_changes());
    canvas.reset();
    assert!(canvas.has_changes());
}

#[test]
fn shadow_and_shape_composite_separately() {
    // Under `copy` the shape's own composite replaces the shadow composited
    // just before it.
    let mut canvas = filled(40, 40, BLUE);
    canvas.set_shadow_color(color(GREEN));
    canvas.set_shadow_offset_x(20.0);
    canvas.set_global_composite_operation(CompositeOp::Copy);
    canvas.set_fill_style(solid(RED));
    canvas.fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
    assert_eq!(pixel(&mut canvas, 25, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 25, 25), CLEAR);
}

#[test]
fn bitmap_carries_through_a_destructive_op_exactly() {
    // Every premultiplied value survives the recording that draws the
    // bitmap as its backdrop.
    let pixels: Vec<u8> = (0..=255u8)
        .flat_map(|i| [i, i.wrapping_mul(7), 255 - i, 255])
        .collect();
    let mut canvas = Canvas::new(16, 16);
    canvas.put_image_data(&pixels, 16, 16, 0, 0, None).unwrap();
    canvas.set_global_composite_operation(CompositeOp::DestinationOut);
    canvas.fill_rect(0.0, 0.0, 16.0, 4.0);
    let after = canvas.get_image_data(0, 0, 16, 16).unwrap();
    assert!(after[..16 * 4 * 4].iter().all(|byte| *byte == 0));
    assert_eq!(after[16 * 4 * 4..], pixels[16 * 4 * 4..]);
}
