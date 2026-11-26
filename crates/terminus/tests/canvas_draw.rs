//! Pixel checks for the drawing surface, away from antialiased edges, read
//! back from the GPU replay.

// Proving a wgpu handle `Send`/`Sync` walks wgpu-core's whole context type
// (`Texture` → `CoreTexture` → `ContextWgpuCore` → ...), deeper than the
// default limit of 128; nightly reports that overflow as the future-error
// `recursion_depth_exceeding_limit` (rust-lang/rust#159228).
#![recursion_limit = "256"]
#![allow(
    clippy::float_cmp,
    reason = "the transforms compared are products of exact binary values"
)]

use terminus::link::html::canvas::{
    CanvasError, CompositeOp, FillRule, Gradient, Image, LineCap, OffscreenCanvas, Path, Pattern,
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

fn pixel(canvas: &mut OffscreenCanvas, x: i32, y: i32) -> [u8; 4] {
    let data = canvas.context_2d().get_image_data(x, y, 1, 1).unwrap();
    [data[0], data[1], data[2], data[3]]
}

fn filled(width: u32, height: u32, rgba: [u8; 4]) -> OffscreenCanvas {
    let mut canvas = OffscreenCanvas::new(width, height);
    canvas.context_2d().set_fill_style(solid(rgba));
    canvas
        .context_2d()
        .fill_rect(0.0, 0.0, f64::from(width), f64::from(height));
    canvas
}

fn image(width: u32, height: u32, rgba: [u8; 4]) -> Image {
    Image::from_unpremultiplied(width, height, rgba.repeat((width * height) as usize)).unwrap()
}

#[test]
fn canvas_is_send() {
    fn send<T: Send>() {}
    fn send_sync<T: Send + Sync>() {}
    send::<OffscreenCanvas>();
    send_sync::<Image>();
    send::<Gradient>();
}

#[test]
fn starts_transparent_and_fills_rect() {
    let mut canvas = OffscreenCanvas::new(20, 20);
    assert_eq!(pixel(&mut canvas, 10, 10), CLEAR);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(5.0, 5.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 10, 10), RED);
    assert_eq!(pixel(&mut canvas, 2, 2), CLEAR);
    assert_eq!(pixel(&mut canvas, 17, 10), CLEAR);
}

#[test]
fn flush_replays_what_is_recorded() {
    let mut canvas = OffscreenCanvas::new(20, 20);
    assert!(!canvas.has_pending_draws());
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(5.0, 5.0, 10.0, 10.0);
    assert!(canvas.has_pending_draws());
    canvas.flush();
    assert!(!canvas.has_pending_draws());
    assert_eq!(pixel(&mut canvas, 10, 10), RED);
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::DestinationOut);
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 20.0);
    canvas.flush();
    assert_eq!(pixel(&mut canvas, 7, 10), CLEAR);
    assert_eq!(pixel(&mut canvas, 12, 10), RED);
}

#[test]
fn many_draws_between_reads_accumulate() {
    let mut canvas = OffscreenCanvas::new(40, 4);
    for i in 0..40 {
        let rgba = if i % 2 == 0 { RED } else { BLUE };
        canvas.context_2d().set_fill_style(solid(rgba));
        canvas.context_2d().fill_rect(f64::from(i), 0.0, 1.0, 4.0);
    }
    assert_eq!(pixel(&mut canvas, 0, 1), RED);
    assert_eq!(pixel(&mut canvas, 1, 1), BLUE);
    assert_eq!(pixel(&mut canvas, 38, 1), RED);
    canvas.context_2d().set_fill_style(solid(GREEN));
    canvas.context_2d().fill_rect(0.0, 0.0, 1.0, 4.0);
    assert_eq!(pixel(&mut canvas, 0, 1), GREEN);
    assert_eq!(pixel(&mut canvas, 1, 1), BLUE);
}

#[test]
fn fill_rules() {
    let mut canvas = OffscreenCanvas::new(30, 30);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().rect(0.0, 0.0, 30.0, 30.0);
    canvas.context_2d().rect(10.0, 10.0, 10.0, 10.0);
    canvas.context_2d().fill(FillRule::EvenOdd);
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
    assert_eq!(pixel(&mut canvas, 15, 15), CLEAR);
    canvas.context_2d().fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 15, 15), RED);
}

#[test]
fn open_subpaths_fill_closed() {
    let mut canvas = OffscreenCanvas::new(20, 20);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().move_to(0.0, 0.0);
    canvas.context_2d().line_to(20.0, 0.0);
    canvas.context_2d().line_to(20.0, 20.0);
    canvas.context_2d().fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 15, 3), RED);
    assert_eq!(pixel(&mut canvas, 3, 15), CLEAR);
}

#[test]
fn begin_path_discards_subpaths() {
    let mut canvas = OffscreenCanvas::new(20, 20);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().rect(0.0, 0.0, 10.0, 10.0);
    canvas.context_2d().begin_path();
    canvas.context_2d().rect(10.0, 10.0, 10.0, 10.0);
    canvas.context_2d().fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 15, 15), RED);
}

#[test]
fn strokes_and_caps() {
    let mut canvas = OffscreenCanvas::new(40, 20);
    canvas.context_2d().set_stroke_style(solid(BLUE));
    canvas.context_2d().set_line_width(6.0);
    canvas.context_2d().move_to(10.0, 10.0);
    canvas.context_2d().line_to(30.0, 10.0);
    canvas.context_2d().stroke();
    assert_eq!(pixel(&mut canvas, 20, 10), BLUE);
    assert_eq!(pixel(&mut canvas, 20, 15), CLEAR);
    assert_eq!(pixel(&mut canvas, 8, 10), CLEAR);

    canvas.context_2d().set_line_cap(LineCap::Square);
    canvas.context_2d().set_stroke_style(solid(RED));
    canvas.context_2d().stroke();
    assert_eq!(pixel(&mut canvas, 8, 10), RED);
    assert_eq!(pixel(&mut canvas, 32, 10), RED);
    assert_eq!(pixel(&mut canvas, 35, 10), CLEAR);
}

#[test]
fn stroke_width_follows_the_transform_at_stroke_time() {
    let mut canvas = OffscreenCanvas::new(40, 40);
    canvas.context_2d().set_stroke_style(solid(RED));
    canvas.context_2d().set_line_width(2.0);
    canvas.context_2d().move_to(5.0, 20.0);
    canvas.context_2d().line_to(35.0, 20.0);
    canvas.context_2d().scale(1.0, 5.0);
    canvas.context_2d().stroke();
    assert_eq!(pixel(&mut canvas, 20, 24), RED);
    assert_eq!(pixel(&mut canvas, 20, 27), CLEAR);
}

#[test]
fn stroke_rect_and_invalid_line_values() {
    let mut canvas = OffscreenCanvas::new(30, 30);
    canvas.context_2d().set_line_width(0.0);
    canvas.context_2d().set_line_width(-1.0);
    canvas.context_2d().set_line_width(f64::NAN);
    assert!((canvas.context_2d().line_width() - 1.0).abs() < f64::EPSILON);
    canvas.context_2d().set_line_width(4.0);
    canvas.context_2d().set_stroke_style(solid(GREEN));
    canvas.context_2d().stroke_rect(5.0, 5.0, 20.0, 20.0);
    assert_eq!(pixel(&mut canvas, 5, 15), GREEN);
    assert_eq!(pixel(&mut canvas, 15, 15), CLEAR);
}

#[test]
fn dashes() {
    let mut canvas = OffscreenCanvas::new(40, 10);
    canvas.context_2d().set_line_dash(&[4.0, 4.0]);
    canvas.context_2d().set_stroke_style(solid(RED));
    canvas.context_2d().set_line_width(4.0);
    canvas.context_2d().move_to(0.0, 5.0);
    canvas.context_2d().line_to(40.0, 5.0);
    canvas.context_2d().stroke();
    assert_eq!(pixel(&mut canvas, 2, 5), RED);
    assert_eq!(pixel(&mut canvas, 6, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 10, 5), RED);

    canvas.context_2d().set_line_dash(&[1.0, 2.0, 3.0]);
    assert_eq!(
        canvas.context_2d().line_dash(),
        [1.0, 2.0, 3.0, 1.0, 2.0, 3.0]
    );
    canvas.context_2d().set_line_dash(&[1.0, -2.0]);
    canvas.context_2d().set_line_dash(&[1.0, f64::INFINITY]);
    assert_eq!(
        canvas.context_2d().line_dash(),
        [1.0, 2.0, 3.0, 1.0, 2.0, 3.0]
    );
}

#[test]
fn a_dash_past_a_million_dashes_strokes_solid() {
    let fine = [4e-5, 5e-5];
    let coarse = [4e-4, 5e-4];
    let gap = |dash: [f64; 2]| dash[0] + dash[1] / 2.0;
    let mut line = Path::new();
    line.move_to(0.0, 5.0);
    line.line_to(100.0, 5.0);

    let mut canvas = OffscreenCanvas::new(100, 10);
    canvas.context_2d().set_line_width(4.0);
    canvas.context_2d().set_line_dash(&fine);
    assert!(
        canvas
            .context_2d()
            .is_point_in_stroke_path(&line, gap(fine), 5.0)
    );
    canvas.context_2d().move_to(0.0, 5.0);
    canvas.context_2d().line_to(100.0, 5.0);
    assert!(canvas.context_2d().is_point_in_stroke(gap(fine), 5.0));
    canvas.context_2d().set_line_dash(&coarse);
    assert!(
        !canvas
            .context_2d()
            .is_point_in_stroke_path(&line, gap(coarse), 5.0)
    );
    assert!(!canvas.context_2d().is_point_in_stroke(gap(coarse), 5.0));
    canvas.context_2d().set_line_dash(&[1e-300]);
    assert!(
        canvas
            .context_2d()
            .is_point_in_stroke_path(&line, 50.0, 5.0)
    );

    canvas.context_2d().set_line_dash(&fine);
    canvas.context_2d().set_stroke_style(solid(RED));
    canvas.context_2d().stroke();
    assert_eq!(pixel(&mut canvas, 50, 5), RED, "stroke()");
    let mut canvas = OffscreenCanvas::new(100, 10);
    canvas.context_2d().set_line_width(4.0);
    canvas.context_2d().set_line_dash(&fine);
    canvas.context_2d().set_stroke_style(solid(RED));
    canvas.context_2d().stroke_path(&line);
    assert_eq!(pixel(&mut canvas, 50, 5), RED, "stroke(path)");
    let mut canvas = OffscreenCanvas::new(100, 10);
    canvas.context_2d().set_line_width(4.0);
    canvas.context_2d().set_line_dash(&fine);
    canvas.context_2d().set_stroke_style(solid(RED));
    canvas.context_2d().stroke_rect(0.0, 5.0, 100.0, 0.0);
    assert_eq!(pixel(&mut canvas, 50, 5), RED, "strokeRect");
    let mut canvas = OffscreenCanvas::new(100, 10);
    canvas.context_2d().set_line_width(4.0);
    canvas.context_2d().set_line_dash(&coarse);
    canvas.context_2d().set_stroke_style(solid(RED));
    canvas.context_2d().stroke_path(&line);
    let [_, _, _, a] = pixel(&mut canvas, 50, 5);
    assert!(a < 160, "dashed, alpha {a}");
}

#[test]
fn dash_offset_shifts_the_pattern() {
    let mut canvas = OffscreenCanvas::new(40, 10);
    canvas.context_2d().set_line_dash(&[4.0, 4.0]);
    canvas.context_2d().set_line_dash_offset(4.0);
    canvas.context_2d().set_stroke_style(solid(RED));
    canvas.context_2d().set_line_width(4.0);
    canvas.context_2d().move_to(0.0, 5.0);
    canvas.context_2d().line_to(40.0, 5.0);
    canvas.context_2d().stroke();
    assert_eq!(pixel(&mut canvas, 2, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 6, 5), RED);
}

#[test]
fn clip_restricts_and_restore_lifts_it() {
    let mut canvas = OffscreenCanvas::new(30, 30);
    canvas.context_2d().save();
    canvas.context_2d().rect(10.0, 10.0, 10.0, 10.0);
    canvas.context_2d().clip(FillRule::NonZero);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(0.0, 0.0, 30.0, 30.0);
    assert_eq!(pixel(&mut canvas, 15, 15), RED);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    canvas.context_2d().restore();
    canvas.context_2d().fill_rect(0.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 2, 2), [0, 0, 0, 255]);
}

#[test]
fn clips_intersect() {
    let mut canvas = OffscreenCanvas::new(30, 30);
    let mut a = Path::new();
    a.rect(0.0, 0.0, 20.0, 20.0);
    let mut b = Path::new();
    b.rect(10.0, 10.0, 20.0, 20.0);
    canvas.context_2d().clip_path(&a, FillRule::NonZero);
    canvas.context_2d().clip_path(&b, FillRule::NonZero);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(0.0, 0.0, 30.0, 30.0);
    assert_eq!(pixel(&mut canvas, 15, 15), RED);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 25, 25), CLEAR);
}

#[test]
fn empty_clip_clips_everything() {
    let mut canvas = OffscreenCanvas::new(10, 10);
    canvas.context_2d().clip(FillRule::NonZero);
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
}

#[test]
fn transforms() {
    let mut canvas = OffscreenCanvas::new(40, 40);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().translate(20.0, 0.0);
    canvas.context_2d().scale(2.0, 2.0);
    canvas.context_2d().fill_rect(0.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 25, 5), RED);
    assert_eq!(pixel(&mut canvas, 15, 5), CLEAR);
    assert_eq!(
        canvas.context_2d().get_transform(),
        [2.0, 0.0, 0.0, 2.0, 20.0, 0.0]
    );

    canvas
        .context_2d()
        .set_transform(1.0, 0.0, 0.0, 1.0, f64::NAN, 0.0);
    canvas.context_2d().scale(f64::INFINITY, 1.0);
    assert_eq!(
        canvas.context_2d().get_transform(),
        [2.0, 0.0, 0.0, 2.0, 20.0, 0.0]
    );

    canvas.context_2d().reset_transform();
    canvas.context_2d().translate(20.0, 20.0);
    canvas.context_2d().rotate(std::f64::consts::FRAC_PI_2);
    canvas.context_2d().set_fill_style(solid(BLUE));
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 4.0);
    assert_eq!(pixel(&mut canvas, 18, 25), BLUE);
    assert_eq!(pixel(&mut canvas, 25, 22), CLEAR);

    canvas
        .context_2d()
        .set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 30.0);
    canvas.context_2d().transform(1.0, 0.0, 0.0, 1.0, 5.0, 0.0);
    canvas.context_2d().set_fill_style(solid(GREEN));
    canvas.context_2d().fill_rect(0.0, 0.0, 4.0, 4.0);
    assert_eq!(pixel(&mut canvas, 7, 32), GREEN);
}

#[test]
fn default_path_keeps_the_transform_it_was_built_under() {
    let mut canvas = OffscreenCanvas::new(40, 20);
    canvas.context_2d().translate(20.0, 0.0);
    canvas.context_2d().rect(0.0, 0.0, 10.0, 10.0);
    canvas.context_2d().reset_transform();
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 25, 5), RED);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);

    let mut path = Path::new();
    path.rect(0.0, 0.0, 10.0, 10.0);
    canvas.context_2d().translate(0.0, 10.0);
    canvas.context_2d().set_fill_style(solid(BLUE));
    canvas.context_2d().fill_path(&path, FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 5, 15), BLUE);
}

#[test]
fn global_alpha() {
    let mut canvas = OffscreenCanvas::new(10, 10);
    canvas.context_2d().set_global_alpha(2.0);
    canvas.context_2d().set_global_alpha(-0.5);
    canvas.context_2d().set_global_alpha(f64::NAN);
    assert!((canvas.context_2d().global_alpha() - 1.0).abs() < f64::EPSILON);
    canvas.context_2d().set_global_alpha(0.5);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
    let [r, g, b, a] = pixel(&mut canvas, 5, 5);
    assert_eq!([r, g, b], [255, 0, 0]);
    assert!((127..=128).contains(&a), "alpha {a}");
}

#[test]
fn linear_gradient_and_late_stops() {
    let gradient = Gradient::linear(0.0, 0.0, 100.0, 0.0).unwrap();
    let mut canvas = OffscreenCanvas::new(100, 4);
    canvas
        .context_2d()
        .set_fill_style(Style::Gradient(gradient.clone()));
    canvas.context_2d().fill_rect(0.0, 0.0, 100.0, 4.0);
    assert_eq!(pixel(&mut canvas, 50, 2), CLEAR);
    gradient.add_color_stop(0.0, color(RED)).unwrap();
    gradient.add_color_stop(1.0, color(BLUE)).unwrap();
    canvas.context_2d().fill_rect(0.0, 0.0, 100.0, 4.0);
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
    let gradient = Gradient::linear(0.0, 0.0, 100.0, 0.0).unwrap();
    gradient.add_color_stop(0.0, color([0, 255, 0, 0])).unwrap();
    gradient.add_color_stop(1.0, color(BLUE)).unwrap();
    let mut canvas = OffscreenCanvas::new(100, 2);
    canvas
        .context_2d()
        .set_fill_style(Style::Gradient(gradient));
    canvas.context_2d().fill_rect(0.0, 0.0, 100.0, 2.0);
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
        let mut canvas = OffscreenCanvas::new(10, 10);
        canvas
            .context_2d()
            .set_fill_style(Style::Gradient(gradient));
        canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
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
    let mut canvas = OffscreenCanvas::new(100, 100);
    canvas
        .context_2d()
        .set_fill_style(Style::Gradient(gradient));
    canvas.context_2d().fill_rect(0.0, 0.0, 100.0, 100.0);
    let center = pixel(&mut canvas, 50, 50);
    assert!(center[0] > 240, "{center:?}");
    assert_eq!(pixel(&mut canvas, 98, 50), BLUE);
}

#[test]
fn conic_gradient_starts_at_the_angle_clockwise() {
    let gradient = Gradient::conic(0.0, 50.0, 50.0).unwrap();
    gradient.add_color_stop(0.0, color(RED)).unwrap();
    gradient.add_color_stop(0.25, color(RED)).unwrap();
    gradient.add_color_stop(0.25, color(BLUE)).unwrap();
    gradient.add_color_stop(1.0, color(BLUE)).unwrap();
    let mut canvas = OffscreenCanvas::new(100, 100);
    canvas
        .context_2d()
        .set_fill_style(Style::Gradient(gradient));
    canvas.context_2d().fill_rect(0.0, 0.0, 100.0, 100.0);
    assert_eq!(pixel(&mut canvas, 80, 70), RED);
    assert_eq!(pixel(&mut canvas, 20, 70), BLUE);
    assert_eq!(pixel(&mut canvas, 80, 30), BLUE);
}

#[test]
fn patterns() {
    let checker = Image::from_unpremultiplied(2, 2, [RED, BLUE, BLUE, RED].concat()).unwrap();

    let mut canvas = OffscreenCanvas::new(8, 8);
    canvas.context_2d().set_image_smoothing_enabled(false);
    let pattern = Pattern::new(checker.clone(), Repetition::Repeat).unwrap();
    canvas.context_2d().set_fill_style(Style::Pattern(pattern));
    canvas.context_2d().fill_rect(0.0, 0.0, 8.0, 8.0);
    assert_eq!(pixel(&mut canvas, 4, 4), RED);
    assert_eq!(pixel(&mut canvas, 5, 4), BLUE);
    assert_eq!(pixel(&mut canvas, 7, 7), RED);

    let mut canvas = OffscreenCanvas::new(8, 8);
    canvas.context_2d().set_image_smoothing_enabled(false);
    let pattern = Pattern::new(checker.clone(), Repetition::NoRepeat).unwrap();
    canvas
        .context_2d()
        .set_fill_style(Style::Pattern(pattern.clone()));
    canvas.context_2d().fill_rect(0.0, 0.0, 8.0, 8.0);
    assert_eq!(pixel(&mut canvas, 0, 0), RED);
    assert_eq!(pixel(&mut canvas, 1, 0), BLUE);
    assert_eq!(pixel(&mut canvas, 4, 4), CLEAR);
    assert_eq!(pixel(&mut canvas, 4, 0), CLEAR);

    pattern
        .set_transform([1.0, 0.0, 0.0, 1.0, 4.0, 4.0])
        .unwrap();
    canvas.context_2d().fill_rect(0.0, 0.0, 8.0, 8.0);
    assert_eq!(pixel(&mut canvas, 4, 4), RED);
    assert_eq!(pixel(&mut canvas, 5, 4), BLUE);

    let mut canvas = OffscreenCanvas::new(8, 8);
    canvas.context_2d().set_image_smoothing_enabled(false);
    let pattern = Pattern::new(checker, Repetition::RepeatX).unwrap();
    canvas.context_2d().set_fill_style(Style::Pattern(pattern));
    canvas.context_2d().fill_rect(0.0, 0.0, 8.0, 8.0);
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
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::SourceIn);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 20.0);
    assert_eq!(pixel(&mut canvas, 5, 10), RED);
    assert_eq!(pixel(&mut canvas, 15, 10), CLEAR);
}

#[test]
fn a_zero_size_fill_rect_has_no_effect() {
    for op in [
        CompositeOp::Copy,
        CompositeOp::SourceIn,
        CompositeOp::DestinationIn,
        CompositeOp::SourceOut,
        CompositeOp::DestinationAtop,
    ] {
        let mut canvas = filled(20, 20, RED);
        canvas.context_2d().set_global_composite_operation(op);
        canvas.context_2d().set_fill_style(solid(BLUE));
        canvas.context_2d().fill_rect(0.0, 0.0, 0.0, 10.0);
        canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 0.0);
        canvas.context_2d().fill_rect(5.0, 5.0, -0.0, -0.0);
        assert_eq!(pixel(&mut canvas, 10, 10), RED, "{op:?}");
    }
}

#[test]
fn composite_copy_respects_the_clip() {
    let mut canvas = filled(20, 20, BLUE);
    canvas.context_2d().rect(0.0, 0.0, 20.0, 10.0);
    canvas.context_2d().clip(FillRule::NonZero);
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::Copy);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(0.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 2, 2), RED);
    assert_eq!(pixel(&mut canvas, 15, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 15, 15), BLUE);
}

#[test]
fn composite_destination_ops_and_xor() {
    let mut canvas = filled(20, 20, BLUE);
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::DestinationOut);
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 20.0);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 15, 5), BLUE);

    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::DestinationOver);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(0.0, 0.0, 20.0, 20.0);
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
    assert_eq!(pixel(&mut canvas, 15, 5), BLUE);

    let mut canvas = filled(20, 20, BLUE);
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::Xor);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 15, 15), BLUE);
}

#[test]
fn composite_blend_modes() {
    let mut canvas = filled(10, 10, [255, 255, 0, 255]);
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::Multiply);
    canvas
        .context_2d()
        .set_fill_style(solid([0, 255, 255, 255]));
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 5, 5), GREEN);

    let mut canvas = filled(10, 10, [255, 0, 0, 255]);
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::Lighter);
    canvas.context_2d().set_fill_style(solid([0, 0, 255, 255]));
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
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
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::DestinationOut);
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::SourceOver);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(15.0, 15.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 12, 5), BLUE);
    assert_eq!(pixel(&mut canvas, 17, 17), RED);
}

#[test]
fn clear_rect_ignores_composite_and_alpha() {
    let mut canvas = filled(20, 20, BLUE);
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::DestinationOver);
    canvas.context_2d().set_global_alpha(0.1);
    canvas.context_2d().clear_rect(5.0, 5.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 10, 10), CLEAR);
    assert_eq!(pixel(&mut canvas, 2, 2), BLUE);
}

#[test]
fn clear_rect_respects_clip_and_whole_canvas_clear() {
    let mut canvas = filled(20, 20, BLUE);
    canvas.context_2d().save();
    canvas.context_2d().rect(0.0, 0.0, 10.0, 20.0);
    canvas.context_2d().clip(FillRule::NonZero);
    canvas.context_2d().clear_rect(0.0, 0.0, 20.0, 20.0);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 15, 5), BLUE);
    canvas.context_2d().restore();
    canvas.context_2d().clear_rect(-5.0, -5.0, 40.0, 40.0);
    assert_eq!(pixel(&mut canvas, 15, 5), CLEAR);
}

#[test]
fn draw_image_forms() {
    let mut canvas = OffscreenCanvas::new(30, 30);
    canvas.context_2d().set_image_smoothing_enabled(false);
    let red = image(4, 4, RED);
    canvas.context_2d().draw_image(&red, 2.0, 2.0).unwrap();
    assert_eq!(pixel(&mut canvas, 3, 3), RED);
    assert_eq!(pixel(&mut canvas, 7, 3), CLEAR);

    canvas
        .context_2d()
        .draw_image_scaled(&image(2, 2, BLUE), 10.0, 10.0, 8.0, 8.0)
        .unwrap();
    assert_eq!(pixel(&mut canvas, 16, 16), BLUE);
    assert_eq!(pixel(&mut canvas, 19, 16), CLEAR);

    let left_red = Image::from_unpremultiplied(2, 1, [RED, GREEN].concat()).unwrap();
    canvas
        .context_2d()
        .draw_image_subrect(&left_red, 1.0, 0.0, 2.0, 1.0, 20.0, 0.0, 8.0, 4.0)
        .unwrap();
    assert_eq!(pixel(&mut canvas, 21, 2), GREEN);
    assert_eq!(pixel(&mut canvas, 25, 2), CLEAR);

    assert_eq!(
        canvas.context_2d().draw_image(&image(0, 0, RED), 0.0, 0.0),
        Err(CanvasError::InvalidState)
    );
}

#[test]
fn draw_image_honours_transform_alpha_and_clip() {
    let mut canvas = OffscreenCanvas::new(20, 20);
    canvas.context_2d().rect(0.0, 0.0, 20.0, 10.0);
    canvas.context_2d().clip(FillRule::NonZero);
    canvas.context_2d().translate(5.0, 5.0);
    canvas.context_2d().set_global_alpha(0.5);
    canvas
        .context_2d()
        .draw_image(&image(10, 10, RED), 0.0, 0.0)
        .unwrap();
    let [r, _, _, a] = pixel(&mut canvas, 8, 7);
    assert_eq!(r, 255);
    assert!((127..=128).contains(&a), "alpha {a}");
    assert_eq!(pixel(&mut canvas, 8, 12), CLEAR);
    assert_eq!(pixel(&mut canvas, 2, 2), CLEAR);
}

#[test]
fn canvas_draws_itself() {
    let mut canvas = OffscreenCanvas::new(20, 10);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
    let own = canvas.image();
    let kept = own.crop(None).unwrap();
    canvas.context_2d().draw_image(&own, 10.0, 0.0).unwrap();
    canvas.context_2d().set_fill_style(solid(BLUE));
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 15, 5), RED);
    assert_eq!(pixel(&mut canvas, 5, 5), BLUE);
    assert_eq!(kept.read().unwrap()[..4], RED);
}

#[test]
fn one_canvas_draws_another_as_it_was_at_the_call() {
    let mut source = filled(4, 4, RED);
    let mut target = OffscreenCanvas::new(8, 4);
    target.context_2d().set_image_smoothing_enabled(false);
    target
        .context_2d()
        .draw_image(&source.image(), 0.0, 0.0)
        .unwrap();
    source.context_2d().set_fill_style(solid(BLUE));
    source.context_2d().fill_rect(0.0, 0.0, 4.0, 4.0);
    target
        .context_2d()
        .draw_image(&source.image(), 4.0, 0.0)
        .unwrap();
    assert_eq!(pixel(&mut target, 1, 1), RED);
    assert_eq!(pixel(&mut target, 5, 1), BLUE);
    let pattern = Pattern::new(source.image(), Repetition::Repeat).unwrap();
    source.context_2d().clear_rect(0.0, 0.0, 4.0, 4.0);
    target.context_2d().set_fill_style(Style::Pattern(pattern));
    target.context_2d().fill_rect(0.0, 0.0, 8.0, 4.0);
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
fn image_bitmap_crop_sizes_are_checked_first() {
    let mut canvas = filled(4, 4, BLUE);
    let mut taken = filled(4, 4, BLUE);
    let images = [image(4, 4, RED), canvas.image(), taken.take_image()];
    for (i, image) in images.iter().enumerate() {
        for size in [(0, 16), (16, 0), (0, 0)] {
            assert_eq!(
                image.crop(Some((0, 0, size.0, size.1))).err(),
                Some(CanvasError::Range),
                "image {i}, {size:?}"
            );
        }
        for size in [(20_000, 16), (16, 8193), (u32::MAX, u32::MAX)] {
            assert_eq!(
                image.crop(Some((0, 0, size.0, size.1))).err(),
                Some(CanvasError::InvalidState),
                "image {i}, {size:?}"
            );
        }
        for origin in [(i64::MAX, i64::MAX), (i64::MIN, i64::MIN)] {
            let far = image
                .crop(Some((origin.0, origin.1, 2, 1)))
                .unwrap_or_else(|error| panic!("image {i}, {origin:?}: {error:?}"));
            assert_eq!(far.read().unwrap(), [CLEAR, CLEAR].concat(), "image {i}");
        }
        let edge = image.crop(Some((3, 0, 8192, 1))).unwrap();
        assert_eq!(edge.width(), 8192, "image {i}");
    }
    assert_eq!(
        OffscreenCanvas::new(0, 0)
            .image()
            .crop(Some((0, 0, 0, 1)))
            .err(),
        Some(CanvasError::Range)
    );
}

#[test]
fn a_long_recording_is_replayed_before_a_read() {
    let mut canvas = OffscreenCanvas::new(64, 1);
    for i in 0..40_000 {
        let rgba = if i % 2 == 0 { RED } else { BLUE };
        canvas.context_2d().set_fill_style(solid(rgba));
        canvas
            .context_2d()
            .fill_rect(f64::from(i % 64), 0.0, 1.0, 1.0);
    }
    assert!(canvas.has_pending_draws());
    assert_eq!(pixel(&mut canvas, 0, 0), RED);
    assert_eq!(pixel(&mut canvas, 1, 0), BLUE);
}

#[test]
fn get_and_put_image_data() {
    let mut canvas = OffscreenCanvas::new(4, 4);
    let data = [[255, 255, 255, 128], [10, 20, 30, 255]].concat();
    canvas
        .context_2d()
        .put_image_data(&data, 2, 1, 1, 1, None)
        .unwrap();
    let back = canvas.context_2d().get_image_data(1, 1, 2, 1).unwrap();
    assert_eq!(back[4..], [10, 20, 30, 255]);
    assert!(back[..3].iter().all(|c| *c >= 254), "{back:?}");
    assert_eq!(back[3], 128);
    let bitmap = canvas.image().read().unwrap();
    let at = (4 + 1) * 4;
    assert_eq!(bitmap[at + 3], 128);
    assert!((127..=128).contains(&bitmap[at]));

    let read = canvas.context_2d().get_image_data(3, 2, -3, -2).unwrap();
    assert_eq!(read.len(), 3 * 2 * 4);
    assert_eq!(read[(3 + 2) * 4..(3 + 2) * 4 + 4], [10, 20, 30, 255]);
    let outside = canvas.context_2d().get_image_data(-2, -2, 2, 2).unwrap();
    assert!(outside.iter().all(|byte| *byte == 0));
    assert_eq!(
        canvas.context_2d().get_image_data(0, 0, 0, 1),
        Err(CanvasError::IndexSize)
    );
}

#[test]
fn put_image_data_replaces_and_ignores_state() {
    let mut canvas = filled(4, 4, BLUE);
    canvas.context_2d().set_global_alpha(0.2);
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::Xor);
    canvas.context_2d().translate(2.0, 2.0);
    canvas.context_2d().clip(FillRule::NonZero);
    let data = [CLEAR, RED, RED, RED].concat();
    canvas
        .context_2d()
        .put_image_data(&data, 2, 2, 1, 0, Some([1, 0, 1, 2]))
        .unwrap();
    assert_eq!(pixel(&mut canvas, 2, 0), RED);
    assert_eq!(pixel(&mut canvas, 2, 1), RED);
    assert_eq!(pixel(&mut canvas, 1, 0), BLUE);
    canvas
        .context_2d()
        .put_image_data(&data, 2, 2, 0, 0, None)
        .unwrap();
    assert_eq!(pixel(&mut canvas, 0, 0), CLEAR);
    assert_eq!(
        canvas
            .context_2d()
            .put_image_data(&data[..4], 2, 2, 0, 0, None),
        Err(CanvasError::InvalidState)
    );
}

#[test]
fn shadows() {
    let mut canvas = OffscreenCanvas::new(40, 40);
    canvas.context_2d().set_shadow_color(color(BLUE));
    canvas.context_2d().set_shadow_offset_x(15.0);
    canvas.context_2d().set_shadow_offset_y(15.0);
    canvas.context_2d().scale(2.0, 2.0);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(0.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
    assert_eq!(pixel(&mut canvas, 20, 20), BLUE);
    assert_eq!(pixel(&mut canvas, 30, 30), CLEAR);
    canvas.context_2d().set_shadow_color(Color::TRANSPARENT);
    canvas.context_2d().fill_rect(10.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 35, 25), CLEAR);
}

#[test]
fn a_shadow_past_the_device_limits_still_draws_the_shape() {
    let draw = |canvas: &mut OffscreenCanvas, blur: f64, offset: f64, rect: [f64; 4]| {
        canvas.context_2d().set_shadow_color(color(BLUE));
        canvas.context_2d().set_shadow_blur(blur);
        canvas.context_2d().set_shadow_offset_x(offset);
        canvas.context_2d().set_shadow_offset_y(offset);
        canvas.context_2d().set_fill_style(solid(RED));
        canvas
            .context_2d()
            .fill_rect(rect[0], rect[1], rect[2], rect[3]);
    };
    for (blur, offset) in [
        (20_000.0, 0.0),
        (1e308, 0.0),
        (f64::MAX, 0.0),
        (2.0, 1e300),
        (2.0, -1e300),
        (2.0, 6000.0),
        (1e308, 1e308),
    ] {
        let mut canvas = OffscreenCanvas::new(16, 16);
        draw(&mut canvas, blur, offset, [4.0, 4.0, 4.0, 4.0]);
        assert_eq!(canvas.context_2d().shadow_blur(), blur);
        assert_eq!(canvas.context_2d().shadow_offset_x(), offset);
        assert_eq!(
            pixel(&mut canvas, 5, 5),
            RED,
            "blur {blur}, offset {offset}"
        );
    }
    for blur in [2.0, 20_000.0] {
        let mut canvas = OffscreenCanvas::new(8192, 4);
        draw(&mut canvas, blur, 0.0, [0.0, 0.0, 8192.0, 4.0]);
        assert_eq!(pixel(&mut canvas, 8000, 2), RED, "blur {blur}");
    }
    let mut canvas = OffscreenCanvas::new(16, 16);
    draw(&mut canvas, 20_000.0, 0.0, [-1e6, -1e6, 2e6, 2e6]);
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
}

#[test]
fn a_large_shadow_blur_under_a_backdrop_draw_keeps_the_bitmap() {
    let mut canvas = filled(16, 16, RED);
    canvas.context_2d().set_shadow_color(color(BLUE));
    canvas.context_2d().set_shadow_blur(20_000.0);
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::DestinationOver);
    canvas.context_2d().set_fill_style(solid(GREEN));
    canvas.context_2d().fill_rect(4.0, 4.0, 4.0, 4.0);
    assert_eq!(pixel(&mut canvas, 12, 12), RED);
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
}

#[test]
fn shadow_blur_spreads_and_is_clipped() {
    let mut canvas = OffscreenCanvas::new(60, 80);
    canvas.context_2d().rect(0.0, 0.0, 60.0, 60.0);
    canvas.context_2d().clip(FillRule::NonZero);
    canvas.context_2d().set_shadow_color(color(BLUE));
    canvas.context_2d().set_shadow_blur(8.0);
    canvas.context_2d().set_shadow_offset_y(30.0);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(10.0, 5.0, 40.0, 40.0);
    assert_eq!(pixel(&mut canvas, 30, 20), RED);
    assert_eq!(pixel(&mut canvas, 30, 50), BLUE);
    let edge = pixel(&mut canvas, 52, 50);
    assert!(edge[3] > 0 && edge[3] < 255, "{edge:?}");
    assert_eq!(pixel(&mut canvas, 30, 65), CLEAR);
}

#[test]
fn hit_tests() {
    let mut canvas = OffscreenCanvas::new(40, 40);
    canvas.context_2d().translate(10.0, 10.0);
    canvas.context_2d().rect(0.0, 0.0, 10.0, 10.0);
    canvas.context_2d().reset_transform();
    assert!(
        canvas
            .context_2d()
            .is_point_in_path(15.0, 15.0, FillRule::NonZero)
    );
    assert!(
        !canvas
            .context_2d()
            .is_point_in_path(5.0, 5.0, FillRule::NonZero)
    );
    assert!(
        !canvas
            .context_2d()
            .is_point_in_path(f64::NAN, 5.0, FillRule::NonZero)
    );

    let mut path = Path::new();
    path.rect(0.0, 0.0, 10.0, 10.0);
    canvas.context_2d().translate(20.0, 20.0);
    assert!(
        canvas
            .context_2d()
            .is_point_in_path2d(&path, 25.0, 25.0, FillRule::EvenOdd)
    );
    assert!(
        !canvas
            .context_2d()
            .is_point_in_path2d(&path, 5.0, 5.0, FillRule::EvenOdd)
    );

    canvas.context_2d().set_line_width(4.0);
    assert!(
        canvas
            .context_2d()
            .is_point_in_stroke_path(&path, 21.0, 25.0)
    );
    assert!(
        !canvas
            .context_2d()
            .is_point_in_stroke_path(&path, 25.0, 25.0)
    );
    canvas.context_2d().reset_transform();
    assert!(canvas.context_2d().is_point_in_stroke(10.0, 15.0));
    assert!(!canvas.context_2d().is_point_in_stroke(15.0, 15.0));
}

#[test]
fn points_on_the_path_are_inside_it() {
    let mut canvas = OffscreenCanvas::new(40, 40);
    canvas.context_2d().rect(0.0, 0.0, 10.0, 10.0);
    let mut path = Path::new();
    path.rect(0.0, 0.0, 10.0, 10.0);
    let on = [
        (0.0, 0.0),
        (5.0, 0.0),
        (10.0, 0.0),
        (10.0, 5.0),
        (10.0, 10.0),
        (5.0, 10.0),
        (0.0, 10.0),
        (0.0, 5.0),
    ];
    let off = [(5.0, -0.01), (10.01, 5.0), (5.0, 10.01), (-0.01, 5.0)];
    for rule in [FillRule::NonZero, FillRule::EvenOdd] {
        for (x, y) in on {
            assert!(
                canvas.context_2d().is_point_in_path(x, y, rule),
                "({x}, {y}) {rule:?}"
            );
            assert!(
                canvas.context_2d().is_point_in_path2d(&path, x, y, rule),
                "({x}, {y}) {rule:?}"
            );
        }
        for (x, y) in off {
            assert!(
                !canvas.context_2d().is_point_in_path(x, y, rule),
                "({x}, {y}) {rule:?}"
            );
            assert!(
                !canvas.context_2d().is_point_in_path2d(&path, x, y, rule),
                "({x}, {y}) {rule:?}"
            );
        }
    }
    let mut line = Path::new();
    line.move_to(0.0, 5.0);
    line.line_to(10.0, 5.0);
    canvas.context_2d().set_line_width(2.0);
    for (x, y) in [(5.0, 4.0), (5.0, 6.0), (0.0, 5.0), (10.0, 5.0)] {
        assert!(
            canvas.context_2d().is_point_in_stroke_path(&line, x, y),
            "({x}, {y})"
        );
    }
    assert!(
        !canvas
            .context_2d()
            .is_point_in_stroke_path(&line, 5.0, 6.01)
    );
}

#[test]
fn arcs_and_ellipses() {
    let mut canvas = OffscreenCanvas::new(40, 40);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas
        .context_2d()
        .arc(20.0, 20.0, 10.0, 0.0, std::f64::consts::TAU, false)
        .unwrap();
    canvas.context_2d().fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 20, 20), RED);
    assert_eq!(pixel(&mut canvas, 20, 12), RED);
    assert_eq!(pixel(&mut canvas, 12, 12), CLEAR);
    assert_eq!(
        canvas.context_2d().arc(0.0, 0.0, -1.0, 0.0, 1.0, false),
        Err(CanvasError::IndexSize)
    );

    let mut canvas = OffscreenCanvas::new(40, 40);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas
        .context_2d()
        .arc(20.0, 20.0, 10.0, 0.0, std::f64::consts::PI, false)
        .unwrap();
    canvas.context_2d().fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 20, 25), RED);
    assert_eq!(pixel(&mut canvas, 20, 15), CLEAR);
    canvas.context_2d().begin_path();
    canvas.context_2d().set_fill_style(solid(BLUE));
    canvas
        .context_2d()
        .arc(20.0, 20.0, 10.0, 0.0, std::f64::consts::PI, true)
        .unwrap();
    canvas.context_2d().fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 20, 15), BLUE);
    assert_eq!(pixel(&mut canvas, 20, 25), RED);

    let mut canvas = OffscreenCanvas::new(40, 40);
    canvas.context_2d().set_fill_style(solid(GREEN));
    canvas
        .context_2d()
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
    canvas.context_2d().fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 32, 20), GREEN);
    assert_eq!(pixel(&mut canvas, 20, 28), CLEAR);
}

#[test]
fn arc_to_rounds_a_corner() {
    let mut canvas = OffscreenCanvas::new(40, 40);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().move_to(0.0, 0.0);
    canvas
        .context_2d()
        .arc_to(30.0, 0.0, 30.0, 30.0, 10.0)
        .unwrap();
    canvas.context_2d().line_to(30.0, 30.0);
    canvas.context_2d().line_to(0.0, 30.0);
    canvas.context_2d().fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 29, 1), CLEAR);
    assert_eq!(pixel(&mut canvas, 15, 2), RED);
    assert_eq!(pixel(&mut canvas, 28, 15), RED);
    assert_eq!(
        canvas.context_2d().arc_to(0.0, 0.0, 1.0, 1.0, -1.0),
        Err(CanvasError::IndexSize)
    );
}

#[test]
fn round_rect() {
    let mut canvas = OffscreenCanvas::new(40, 40);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas
        .context_2d()
        .round_rect(5.0, 5.0, 30.0, 30.0, &[[10.0, 10.0]])
        .unwrap();
    canvas.context_2d().fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 6, 6), CLEAR);
    assert_eq!(pixel(&mut canvas, 20, 6), RED);
    assert_eq!(pixel(&mut canvas, 20, 20), RED);

    let mut canvas = OffscreenCanvas::new(40, 40);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas
        .context_2d()
        .round_rect(
            35.0,
            5.0,
            -30.0,
            30.0,
            &[[10.0, 10.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
        )
        .unwrap();
    canvas.context_2d().fill(FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 34, 6), CLEAR);
    assert_eq!(pixel(&mut canvas, 6, 6), RED);

    assert_eq!(
        canvas.context_2d().round_rect(0.0, 0.0, 1.0, 1.0, &[]),
        Err(CanvasError::Range)
    );
    assert_eq!(
        canvas
            .context_2d()
            .round_rect(0.0, 0.0, 1.0, 1.0, &[[-1.0, 0.0]]),
        Err(CanvasError::Range)
    );
}

#[test]
fn path2d_from_svg_and_add_path() {
    let svg = Path::from_svg("M0 0 H10 V10 H0 Z");
    let mut path = Path::new();
    path.add_path(&svg, [1.0, 0.0, 0.0, 1.0, 20.0, 0.0]);
    let mut canvas = OffscreenCanvas::new(40, 20);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_path(&svg, FillRule::NonZero);
    canvas.context_2d().set_fill_style(solid(BLUE));
    canvas.context_2d().fill_path(&path, FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
    assert_eq!(pixel(&mut canvas, 25, 5), BLUE);
    assert_eq!(pixel(&mut canvas, 15, 5), CLEAR);
}

#[test]
fn path2d_from_svg_keeps_the_segments_before_an_error() {
    let mut canvas = OffscreenCanvas::new(40, 20);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_path(
        &Path::from_svg("M0 0 L10 0 L10 10 L0 10 Z X"),
        FillRule::NonZero,
    );
    canvas.context_2d().fill_path(
        &Path::from_svg("M20 0 h10 v10 h-10 z M30 0 L 40"),
        FillRule::NonZero,
    );
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
    assert_eq!(pixel(&mut canvas, 25, 5), RED);
    assert_eq!(pixel(&mut canvas, 35, 15), CLEAR);
    canvas
        .context_2d()
        .fill_path(&Path::from_svg("L0 0 L40 0 L40 20"), FillRule::NonZero);
    assert_eq!(pixel(&mut canvas, 35, 5), CLEAR);
}

#[test]
fn resize_resets_bitmap_and_state() {
    let mut canvas = OffscreenCanvas::new(10, 10);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().translate(1.0, 1.0);
    canvas.context_2d().save();
    canvas.context_2d().fill_rect(0.0, 0.0, 5.0, 5.0);
    canvas.set_size(10, 10);
    assert_eq!(pixel(&mut canvas, 3, 3), CLEAR);
    assert_eq!(
        canvas.context_2d().get_transform(),
        [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
    );
    canvas.context_2d().fill_rect(0.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 0, 0), [0, 0, 0, 255]);

    canvas.set_size(20, 5);
    assert_eq!((canvas.width(), canvas.height()), (20, 5));
    assert_eq!(pixel(&mut canvas, 0, 0), CLEAR);
    canvas.context_2d().fill_rect(15.0, 0.0, 5.0, 5.0);
    assert_eq!(pixel(&mut canvas, 17, 2), [0, 0, 0, 255]);
}

#[test]
fn zero_sized_canvas_draws_nothing() {
    let mut canvas = OffscreenCanvas::new(0, 10);
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
    assert!(!canvas.has_pending_draws());
    assert_eq!(
        canvas.context_2d().get_image_data(0, 0, 1, 1).unwrap(),
        CLEAR
    );
    assert_eq!(canvas.image().width(), 0);
    let mut canvas = OffscreenCanvas::new(8193, 1);
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(
        canvas.context_2d().get_image_data(0, 0, 1, 1).unwrap(),
        CLEAR
    );
}

#[test]
fn take_image_leaves_transparent_and_keeps_state() {
    let mut canvas = OffscreenCanvas::new(10, 10);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
    let taken = canvas.take_image();
    assert_eq!(taken.read().unwrap()[..4], RED);
    assert_eq!(pixel(&mut canvas, 5, 5), CLEAR);
    canvas.context_2d().fill_rect(0.0, 0.0, 1.0, 1.0);
    assert_eq!(pixel(&mut canvas, 0, 0), RED);
    assert_eq!(taken.read().unwrap()[..4], RED);
    let mut other = OffscreenCanvas::new(10, 10);
    other.context_2d().draw_image(&taken, 0.0, 0.0).unwrap();
    assert_eq!(pixel(&mut other, 5, 5), RED);
}

#[test]
fn changes_are_tracked_until_a_commit() {
    let mut canvas = OffscreenCanvas::new(10, 10);
    assert!(!canvas.has_changes());
    canvas.context_2d().fill_rect(0.0, 0.0, 1.0, 1.0);
    assert!(canvas.has_changes());
    canvas.context_2d().reset();
    assert!(canvas.has_changes());
}

#[test]
fn shadow_and_shape_composite_separately() {
    let mut canvas = filled(40, 40, BLUE);
    canvas.context_2d().set_shadow_color(color(GREEN));
    canvas.context_2d().set_shadow_offset_x(20.0);
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::Copy);
    canvas.context_2d().set_fill_style(solid(RED));
    canvas.context_2d().fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(pixel(&mut canvas, 5, 5), RED);
    assert_eq!(pixel(&mut canvas, 25, 5), CLEAR);
    assert_eq!(pixel(&mut canvas, 25, 25), CLEAR);
}

#[test]
fn bitmap_carries_through_a_destructive_op_exactly() {
    let pixels: Vec<u8> = (0..=255u8)
        .flat_map(|i| [i, i.wrapping_mul(7), 255 - i, 255])
        .collect();
    let mut canvas = OffscreenCanvas::new(16, 16);
    canvas
        .context_2d()
        .put_image_data(&pixels, 16, 16, 0, 0, None)
        .unwrap();
    canvas
        .context_2d()
        .set_global_composite_operation(CompositeOp::DestinationOut);
    canvas.context_2d().fill_rect(0.0, 0.0, 16.0, 4.0);
    let after = canvas.context_2d().get_image_data(0, 0, 16, 16).unwrap();
    assert!(after[..16 * 4 * 4].iter().all(|byte| *byte == 0));
    assert_eq!(after[16 * 4 * 4..], pixels[16 * 4 * 4..]);
}
