//! The canvas members of `bobcat-internal:host`: one member per Canvas 2D
//! method or attribute, numbers, strings, booleans and a bare `ArrayBuffer`
//! in, the same out. The engine's built-in modules and an embedder's
//! native-module ES module (`lynx:<name>`) call them directly; no realm has a
//! JavaScript class over them.
//!
//! A member converts nothing: it takes the numbers, strings and booleans it
//! is given. A value of the wrong type, or an id naming nothing of the kind
//! the member takes, is the caller's bug: the member throws the bridge's
//! `InternalError`, its message starting with the member's full name. A call
//! that fails as the HTML Standard says answers the exception's name as a
//! string (`"IndexSizeError"`, `"TypeError"`, …) and throws nothing;
//! `packages/bobcat-element/src/native.d.ts` lists each member's answer.

use std::rc::{Rc, Weak};

use quickjs_rust_bridge::{HostArgument, HostValue};
use smallvec::SmallVec;
use terminus::link::NodeId;
use terminus::link::html::canvas::{
    CanvasError, CompositeOp, Direction, FillRule, Gradient, Image, LineCap, LineJoin,
    OffscreenCanvas, Path, Pattern, Repetition, SmoothingQuality, Style, TextAlign, TextBaseline,
};

use super::{CanvasDocument, CanvasEntry, CanvasTable, Entry, Fonts, no_such};
use crate::esm::HOST_MODULE_SPECIFIER;
use crate::main::quickjs::ScriptEngine;
use crate::script::ScriptError;

/// Registers one member per `fn`, each `$parser` applied at its argument's
/// position; `NAME` is the member's full name, which every message it
/// throws starts with ([`named`]).
macro_rules! members {
    ($engine:ident, $table:ident; $(
        fn $name:ident($($arg:ident: $parser:ident),*) |$this:ident| $body:block
    )*) => {$({
        const NAME: &str = concat!("bobcat-internal:host.", stringify!($name));
        let table = Rc::clone($table);
        let arity = 0u8 $(+ { let _ = stringify!($arg); 1u8 })*;
        $engine.register_host_module_function(
            HOST_MODULE_SPECIFIER,
            stringify!($name),
            arity,
            move |arguments| named(NAME, || {
                #[allow(unused_mut, reason = "zero-argument members never advance it")]
                let mut index = 0usize;
                $(
                    let $arg = $parser(NAME, arguments, index)?;
                    index += 1;
                )*
                let _ = (arguments, index);
                let $this: &CanvasTable = &table;
                $body
            }),
        )?;
    })*};
}

/// Runs `member`, its error message starting with the member's full name
/// `name`: a message an argument parser wrote already does, and one of
/// [`CanvasTable`]'s lookups ([`no_such`]) is prefixed with it.
fn named(
    name: &str,
    member: impl FnOnce() -> Result<HostValue, String>,
) -> Result<HostValue, String> {
    member().map_err(|message| {
        if message.starts_with(name) {
            message
        } else {
            format!("{name}: {message}")
        }
    })
}

/// Runs `$call` on the path the id `$id` names: a canvas's current default
/// path, through its 2D context, or a path entry. Both have the builder
/// methods under the same names.
macro_rules! on_path {
    ($table:ident, $id:ident, |$path:ident| $call:expr) => {
        match $table.entries().get_mut(&$id) {
            Some(Entry::Canvas(entry)) => {
                let $path = &mut entry.canvas().context_2d();
                Ok($call)
            }
            Some(Entry::Path($path)) => Ok($call),
            _ => Err(no_such("canvas or path", $id)),
        }
    };
}

/// Installs every canvas member on `engine`, over `table`, and in a realm
/// with a document `canvasTransferControl` too.
pub(crate) fn install(engine: &ScriptEngine, table: &Rc<CanvasTable>) -> Result<(), ScriptError> {
    install_context_members(engine, table)?;
    if let Fonts::Document(document) = &table.fonts {
        install_transfer(engine, table, document)?;
    }
    Ok(())
}

/// `canvasTransferControl(node)`: HTML's `transferControlToOffscreen()` on
/// the element `node`, the canvas it makes filed in `table`.
/// Answers `[id, width, height]` as a `Float64Array`'s bytes, or
/// `"InvalidStateError"` for an element that is not a canvas in context mode
/// none.
fn install_transfer(
    engine: &ScriptEngine,
    table: &Rc<CanvasTable>,
    document: &Weak<dyn CanvasDocument>,
) -> Result<(), ScriptError> {
    const NAME: &str = "bobcat-internal:host.canvasTransferControl";
    let table = Rc::clone(table);
    let document = Weak::clone(document);
    engine.register_host_module_function(
        HOST_MODULE_SPECIFIER,
        "canvasTransferControl",
        1,
        move |arguments| {
            let node = node(NAME, arguments, 0)?;
            let document = document
                .upgrade()
                .ok_or_else(|| format!("{NAME}: the document is gone"))?;
            Ok(match document.transfer_control_to_offscreen(node)? {
                Ok(transferred) => {
                    let size = [transferred.width, transferred.height].map(f64::from);
                    let id = table.insert(Entry::Canvas(CanvasEntry::transferred(transferred)));
                    #[expect(clippy::cast_precision_loss, reason = "ids stay below 2^53")]
                    float_bytes(&[id as f64, size[0], size[1]])
                }
                Err(_) => error_value(CanvasError::InvalidState),
            })
        },
    )
}

#[expect(clippy::too_many_lines, reason = "one member per context method")]
#[expect(
    clippy::redundant_closure_for_method_calls,
    reason = "a `Context2d` method's path names one context lifetime, and the helpers take a \
              closure over any"
)]
fn install_context_members(
    engine: &ScriptEngine,
    table: &Rc<CanvasTable>,
) -> Result<(), ScriptError> {
    members! { engine, table;
        // Objects.
        fn canvasCreate(width: size, height: size) |this| {
            Ok(id_value(this.insert(Entry::Canvas(CanvasEntry::new(width, height)))))
        }
        fn canvasDrop(target: id) |this| {
            this.remove(target);
            Ok(HostValue::Undefined)
        }
        fn canvasSetSize(target: id, width: size, height: size) |this| {
            let mut entries = this.entries();
            let Some(Entry::Canvas(entry)) = entries.get_mut(&target) else {
                return Err(no_such("canvas", target));
            };
            entry.set_size(width, height);
            // A bitmap there was is now transparent black.
            if entry.canvas.is_some() {
                this.note_drawn(target, entry);
            }
            Ok(HostValue::Undefined)
        }
        fn canvasPath(source: optional_id) |this| {
            let path = match source {
                None => Path::new(),
                Some(source) => match this.entries().get(&source) {
                    Some(Entry::Path(path)) => path.clone(),
                    _ => return Err(no_such("path", source)),
                },
            };
            Ok(id_value(this.insert(Entry::Path(path))))
        }
        fn canvasPathFromSvg(data: text) |this| {
            Ok(id_value(this.insert(Entry::Path(Path::from_svg(data)))))
        }
        fn canvasAddPath(target: id, source: id, a: number, b: number, c: number, d: number,
            e: number, f: number) |this| {
            let mut entries = this.entries();
            let Some(Entry::Path(added)) = entries.get(&source) else {
                return Err(no_such("path", source));
            };
            let added = added.clone();
            let Some(Entry::Path(path)) = entries.get_mut(&target) else {
                return Err(no_such("path", target));
            };
            path.add_path(&added, [a, b, c, d, e, f]);
            Ok(HostValue::Undefined)
        }

        // The state stack.
        fn canvasSave(target: id) |this| {
            this.context(target, |context| context.save()).map(undefined)
        }
        fn canvasRestore(target: id) |this| {
            this.context(target, |context| context.restore()).map(undefined)
        }
        fn canvasReset(target: id) |this| {
            this.draw(target, |context| context.reset()).map(undefined)
        }

        // Transformations.
        fn canvasScale(target: id, x: number, y: number) |this| {
            this.context(target, |context| context.scale(x, y)).map(undefined)
        }
        fn canvasRotate(target: id, angle: number) |this| {
            this.context(target, |context| context.rotate(angle)).map(undefined)
        }
        fn canvasTranslate(target: id, x: number, y: number) |this| {
            this.context(target, |context| context.translate(x, y)).map(undefined)
        }
        fn canvasTransform(target: id, a: number, b: number, c: number, d: number, e: number,
            f: number) |this| {
            this.context(target, |context| context.transform(a, b, c, d, e, f)).map(undefined)
        }
        fn canvasSetTransform(target: id, a: number, b: number, c: number, d: number,
            e: number, f: number) |this| {
            this.context(target, |context| context.set_transform(a, b, c, d, e, f)).map(undefined)
        }
        fn canvasGetTransform(target: id) |this| {
            this.context(target, |context| float_bytes(&context.get_transform()))
        }
        fn canvasResetTransform(target: id) |this| {
            this.context(target, |context| context.reset_transform()).map(undefined)
        }

        // Numeric, keyword and boolean attributes.
        fn canvasGetNumber(target: id, which: selector) |this| {
            this.context(target, |context| {
                HostValue::Number(match which {
                    GLOBAL_ALPHA => context.global_alpha(),
                    SHADOW_BLUR => context.shadow_blur(),
                    SHADOW_OFFSET_X => context.shadow_offset_x(),
                    SHADOW_OFFSET_Y => context.shadow_offset_y(),
                    LINE_WIDTH => context.line_width(),
                    MITER_LIMIT => context.miter_limit(),
                    _ => context.line_dash_offset(),
                })
            })
        }
        fn canvasSetNumber(target: id, which: selector, value: number) |this| {
            this.context(target, |context| match which {
                GLOBAL_ALPHA => context.set_global_alpha(value),
                SHADOW_BLUR => context.set_shadow_blur(value),
                SHADOW_OFFSET_X => context.set_shadow_offset_x(value),
                SHADOW_OFFSET_Y => context.set_shadow_offset_y(value),
                LINE_WIDTH => context.set_line_width(value),
                MITER_LIMIT => context.set_miter_limit(value),
                _ => context.set_line_dash_offset(value),
            })
            .map(undefined)
        }
        fn canvasGetKeyword(target: id, which: selector) |this| {
            this.context(target, |context| {
                HostValue::String(
                    match which {
                        COMPOSITE => context.global_composite_operation().name(),
                        SMOOTHING_QUALITY => context.image_smoothing_quality().name(),
                        LINE_CAP => context.line_cap().name(),
                        LINE_JOIN => context.line_join().name(),
                        TEXT_ALIGN => context.text_align().name(),
                        TEXT_BASELINE => context.text_baseline().name(),
                        _ => context.direction().name(),
                    }
                    .to_owned(),
                )
            })
        }
        fn canvasSetKeyword(target: id, which: selector, value: text) |this| {
            // A string the attribute does not take is ignored.
            this.context(target, |context| match which {
                COMPOSITE => CompositeOp::from_name(value)
                    .map(|op| context.set_global_composite_operation(op)),
                SMOOTHING_QUALITY => SmoothingQuality::from_name(value)
                    .map(|quality| context.set_image_smoothing_quality(quality)),
                LINE_CAP => LineCap::from_name(value).map(|cap| context.set_line_cap(cap)),
                LINE_JOIN => LineJoin::from_name(value).map(|join| context.set_line_join(join)),
                TEXT_ALIGN => TextAlign::from_name(value).map(|align| context.set_text_align(align)),
                TEXT_BASELINE => TextBaseline::from_name(value)
                    .map(|baseline| context.set_text_baseline(baseline)),
                _ => Direction::from_name(value).map(|direction| context.set_direction(direction)),
            })
            .map(undefined)
        }
        fn canvasGetSmoothing(target: id) |this| {
            this.context(target, |context| HostValue::Boolean(context.image_smoothing_enabled()))
        }
        fn canvasSetSmoothing(target: id, enabled: flag) |this| {
            this.context(target, |context| context.set_image_smoothing_enabled(enabled))
                .map(undefined)
        }

        // Fill, stroke and shadow styles.
        fn canvasSetColor(target: id, which: selector, css: text) |this| {
            this.context(target, |context| match which {
                FILL => context.set_fill_style_css(css),
                STROKE => context.set_stroke_style_css(css),
                _ => context.set_shadow_color_css(css),
            })
            .map(serialization)
        }
        fn canvasSetPaint(target: id, which: selector, paint: id) |this| {
            let style = match this.entries().get(&paint) {
                Some(Entry::Gradient(gradient)) => Style::Gradient(gradient.clone()),
                Some(Entry::Pattern(pattern)) => Style::Pattern(pattern.clone()),
                _ => return Err(no_such("gradient or pattern", paint)),
            };
            this.context(target, |context| {
                if which == FILL {
                    context.set_fill_style(style);
                } else {
                    context.set_stroke_style(style);
                }
            })
            .map(undefined)
        }
        fn canvasLinearGradient(x0: number, y0: number, x1: number, y1: number) |this| {
            Ok(created(this, Gradient::linear(x0, y0, x1, y1).map(Entry::Gradient)))
        }
        fn canvasRadialGradient(x0: number, y0: number, r0: number, x1: number, y1: number,
            r1: number) |this| {
            Ok(created(this, Gradient::radial(x0, y0, r0, x1, y1, r1).map(Entry::Gradient)))
        }
        fn canvasConicGradient(angle: number, x: number, y: number) |this| {
            Ok(created(this, Gradient::conic(angle, x, y).map(Entry::Gradient)))
        }
        fn canvasAddColorStop(gradient: id, offset: number, css: text) |this| {
            let gradient = this.gradient(gradient)?;
            Ok(outcome(gradient.add_color_stop_css(offset, css)))
        }
        fn canvasCreatePattern(source: id, repetition: text) |this| {
            let Some(repetition) = Repetition::from_pattern_name(repetition) else {
                return Ok(error_value(CanvasError::Syntax));
            };
            let image = this.image(source)?;
            Ok(created(this, Pattern::new(image, repetition).map(Entry::Pattern)))
        }
        fn canvasPatternSetTransform(pattern: id, a: number, b: number, c: number, d: number,
            e: number, f: number) |this| {
            let pattern = this.pattern(pattern)?;
            Ok(outcome(pattern.set_transform([a, b, c, d, e, f])))
        }

        // Rectangles.
        fn canvasClearRect(target: id, x: number, y: number, w: number, h: number) |this| {
            this.draw(target, |context| context.clear_rect(x, y, w, h)).map(undefined)
        }
        fn canvasFillRect(target: id, x: number, y: number, w: number, h: number) |this| {
            this.draw(target, |context| context.fill_rect(x, y, w, h)).map(undefined)
        }
        fn canvasStrokeRect(target: id, x: number, y: number, w: number, h: number) |this| {
            this.draw(target, |context| context.stroke_rect(x, y, w, h)).map(undefined)
        }

        // Building paths, on a canvas's default path or a path entry.
        fn canvasBeginPath(target: id) |this| {
            this.context(target, |context| context.begin_path()).map(undefined)
        }
        fn canvasClosePath(target: id) |this| {
            on_path!(this, target, |path| path.close_path()).map(undefined)
        }
        fn canvasMoveTo(target: id, x: number, y: number) |this| {
            on_path!(this, target, |path| path.move_to(x, y)).map(undefined)
        }
        fn canvasLineTo(target: id, x: number, y: number) |this| {
            on_path!(this, target, |path| path.line_to(x, y)).map(undefined)
        }
        fn canvasQuadraticCurveTo(target: id, cpx: number, cpy: number, x: number,
            y: number) |this| {
            on_path!(this, target, |path| path.quadratic_curve_to(cpx, cpy, x, y))
                .map(undefined)
        }
        fn canvasBezierCurveTo(target: id, cp1x: number, cp1y: number, cp2x: number,
            cp2y: number, x: number, y: number) |this| {
            on_path!(this, target, |path| path.bezier_curve_to(cp1x, cp1y, cp2x, cp2y, x, y))
                .map(undefined)
        }
        fn canvasArc(target: id, x: number, y: number, radius: number, start: number,
            end: number, anticlockwise: flag) |this| {
            on_path!(this, target, |path| path.arc(x, y, radius, start, end, anticlockwise))
                .map(outcome)
        }
        fn canvasArcTo(target: id, x1: number, y1: number, x2: number, y2: number,
            radius: number) |this| {
            on_path!(this, target, |path| path.arc_to(x1, y1, x2, y2, radius)).map(outcome)
        }
        fn canvasEllipse(target: id, x: number, y: number, rx: number, ry: number,
            rotation: number, start: number, end: number, anticlockwise: flag) |this| {
            on_path!(this, target, |path| {
                path.ellipse(x, y, rx, ry, rotation, start, end, anticlockwise)
            })
            .map(outcome)
        }
        fn canvasRect(target: id, x: number, y: number, w: number, h: number) |this| {
            on_path!(this, target, |path| path.rect(x, y, w, h)).map(undefined)
        }
        fn canvasRoundRect(target: id, x: number, y: number, w: number, h: number,
            radii: rest) |this| {
            // Each radius is an `[x, y]` pair; a caller writes a number `r`
            // as `r, r`.
            let pairs: SmallVec<[[f64; 2]; 4]> =
                radii.chunks_exact(2).map(|pair| [pair[0], pair[1]]).collect();
            on_path!(this, target, |path| path.round_rect(x, y, w, h, &pairs)).map(outcome)
        }

        // Drawing paths. A path id of 0 is the current default path.
        fn canvasFill(target: id, path: optional_id, rule: fill_rule) |this| {
            let path = path.map(|path| this.path(path)).transpose()?;
            this.draw(target, |context| match &path {
                Some(path) => context.fill_path(path, rule),
                None => context.fill(rule),
            })
            .map(undefined)
        }
        fn canvasStroke(target: id, path: optional_id) |this| {
            let path = path.map(|path| this.path(path)).transpose()?;
            this.draw(target, |context| match &path {
                Some(path) => context.stroke_path(path),
                None => context.stroke(),
            })
            .map(undefined)
        }
        fn canvasClip(target: id, path: optional_id, rule: fill_rule) |this| {
            let path = path.map(|path| this.path(path)).transpose()?;
            this.context(target, |context| match &path {
                Some(path) => context.clip_path(path, rule),
                None => context.clip(rule),
            })
            .map(undefined)
        }
        fn canvasIsPointInPath(target: id, path: optional_id, x: number, y: number,
            rule: fill_rule) |this| {
            let path = path.map(|path| this.path(path)).transpose()?;
            this.context(target, |context| {
                HostValue::Boolean(match &path {
                    Some(path) => context.is_point_in_path2d(path, x, y, rule),
                    None => context.is_point_in_path(x, y, rule),
                })
            })
        }
        fn canvasIsPointInStroke(target: id, path: optional_id, x: number, y: number) |this| {
            let path = path.map(|path| this.path(path)).transpose()?;
            this.context(target, |context| {
                HostValue::Boolean(match &path {
                    Some(path) => context.is_point_in_stroke_path(path, x, y),
                    None => context.is_point_in_stroke(x, y),
                })
            })
        }

        // Line styles.
        fn canvasSetLineDash(target: id, segments: rest) |this| {
            this.context(target, |context| context.set_line_dash(&segments)).map(undefined)
        }
        fn canvasGetLineDash(target: id) |this| {
            this.context(target, |context| float_bytes(context.line_dash()))
        }

        // Text.
        fn canvasSetFont(target: id, css: text) |this| {
            this.context(target, |context| context.set_font_css(css)).map(serialization)
        }
        fn canvasFillText(target: id, content: text, x: number, y: number,
            max_width: optional_number) |this| {
            this.text(target, |context, fonts| {
                context.fill_text(fonts, content, x, y, max_width);
            })?;
            Ok(HostValue::Undefined)
        }
        fn canvasStrokeText(target: id, content: text, x: number, y: number,
            max_width: optional_number) |this| {
            this.text(target, |context, fonts| {
                context.stroke_text(fonts, content, x, y, max_width);
            })?;
            Ok(HostValue::Undefined)
        }
        fn canvasMeasureText(target: id, content: text) |this| {
            let metrics = this
                .text(target, |context, fonts| context.measure_text(fonts, content))?
                .unwrap_or_default();
            Ok(float_bytes(&[
                metrics.width,
                metrics.actual_bounding_box_left,
                metrics.actual_bounding_box_right,
                metrics.font_bounding_box_ascent,
                metrics.font_bounding_box_descent,
                metrics.actual_bounding_box_ascent,
                metrics.actual_bounding_box_descent,
                metrics.em_height_ascent,
                metrics.em_height_descent,
                metrics.hanging_baseline,
                metrics.alphabetic_baseline,
                metrics.ideographic_baseline,
            ]))
        }

        // Images and pixels.
        fn canvasDrawImage(target: id, source: id, rects: rest) |this| {
            let image = this.image(source)?;
            this.draw(target, |context| match *rects.as_slice() {
                [dx, dy] => context.draw_image(&image, dx, dy),
                [dx, dy, dw, dh] => context.draw_image_scaled(&image, dx, dy, dw, dh),
                [sx, sy, sw, sh, dx, dy, dw, dh] => {
                    context.draw_image_subrect(&image, sx, sy, sw, sh, dx, dy, dw, dh)
                }
                _ => Err(CanvasError::Type),
            })
            .map(outcome)
        }
        fn canvasGetImageData(target: id, sx: long, sy: long, sw: long, sh: long) |this| {
            // The read is issued under the table's borrow and waited on
            // after it.
            let read = this.context(target, |context| context.read_image_data(sx, sy, sw, sh))?;
            Ok(match read.and_then(|read| this.reads.wait(read)) {
                Ok(pixels) => HostValue::Bytes(pixels.into_boxed_slice()),
                Err(error) => error_value(error),
            })
        }
        fn canvasPutImageData(target: id, buffer: bytes, offset: size, length: size,
            width: size, height: size, dx: long, dy: long, dirty: rest) |this| {
            let pixels = view(buffer, offset, length)?;
            let dirty = match *dirty.as_slice() {
                [] => None,
                [x, y, w, h] => Some([x, y, w, h].map(to_long)),
                _ => return Err(format!("{NAME} takes four dirty-rectangle numbers or none")),
            };
            this.draw(target, |context| context.put_image_data(pixels, width, height, dx, dy, dirty))
                .map(outcome)
        }
        fn canvasTransferToBitmap(target: id) |this| {
            let image = this.draw_canvas(target, OffscreenCanvas::take_image)?;
            Ok(id_value(this.insert(Entry::Bitmap(image))))
        }
        fn canvasBitmapFrom(source: id, crop: rest) |this| {
            let image = this.image(source)?;
            Ok(created(this, cropped(&image, &crop).map(Entry::Bitmap)))
        }
        fn canvasBitmapFromPixels(buffer: bytes, offset: size, length: size, width: size,
            height: size, crop: rest) |this| {
            let pixels = view(buffer, offset, length)?;
            let image = match Image::from_unpremultiplied(width, height, pixels.to_vec()) {
                Ok(image) => image,
                Err(error) => return Ok(error_value(error)),
            };
            Ok(created(this, cropped(&image, &crop).map(Entry::Bitmap)))
        }
        fn canvasBitmapWidth(source: id) |this| {
            Ok(HostValue::Number(f64::from(this.bitmap(source)?.width())))
        }
        fn canvasBitmapHeight(source: id) |this| {
            Ok(HostValue::Number(f64::from(this.bitmap(source)?.height())))
        }
    }
    Ok(())
}

// `canvasGetNumber`/`canvasSetNumber`'s `which`, in `native.d.ts`'s order;
// anything past the last is `lineDashOffset`.
const GLOBAL_ALPHA: u8 = 0;
const SHADOW_BLUR: u8 = 1;
const SHADOW_OFFSET_X: u8 = 2;
const SHADOW_OFFSET_Y: u8 = 3;
const LINE_WIDTH: u8 = 4;
const MITER_LIMIT: u8 = 5;

// `canvasGetKeyword`/`canvasSetKeyword`'s `which`; anything past the last is
// `direction`.
const COMPOSITE: u8 = 0;
const SMOOTHING_QUALITY: u8 = 1;
const LINE_CAP: u8 = 2;
const LINE_JOIN: u8 = 3;
const TEXT_ALIGN: u8 = 4;
const TEXT_BASELINE: u8 = 5;

// `canvasSetColor`/`canvasSetPaint`'s `which`: anything else is the stroke
// style for a paint and the shadow color for a color.
const FILL: u8 = 0;
const STROKE: u8 = 1;

impl CanvasTable {
    /// A clone of the path under `id`.
    fn path(&self, id: u64) -> Result<Path, String> {
        match self.entries().get(&id) {
            Some(Entry::Path(path)) => Ok(path.clone()),
            _ => Err(no_such("path", id)),
        }
    }

    fn gradient(&self, id: u64) -> Result<Gradient, String> {
        match self.entries().get(&id) {
            Some(Entry::Gradient(gradient)) => Ok(gradient.clone()),
            _ => Err(no_such("gradient", id)),
        }
    }

    fn pattern(&self, id: u64) -> Result<Pattern, String> {
        match self.entries().get(&id) {
            Some(Entry::Pattern(pattern)) => Ok(pattern.clone()),
            _ => Err(no_such("pattern", id)),
        }
    }

    /// The pixels of the image bitmap under `id`.
    fn bitmap(&self, id: u64) -> Result<Image, String> {
        match self.entries().get(&id) {
            Some(Entry::Bitmap(image)) => Ok(image.clone()),
            _ => Err(no_such("image bitmap", id)),
        }
    }

    /// Runs a text call on the canvas under `id` with this realm's fonts,
    /// listing the canvas as drawn. `None` when the realm has no fonts to
    /// lend yet (an MTS realm before its document).
    fn text<R>(
        &self,
        id: u64,
        call: impl FnOnce(
            &mut terminus::link::html::canvas::Context2d<'_>,
            &mut terminus::link::TextContext,
        ) -> R,
    ) -> Result<Option<R>, String> {
        self.draw(id, |context| self.with_fonts(|fonts| call(context, fonts)))
    }
}

/// `canvasBitmapFrom`'s and `canvasBitmapFromPixels`' crop of `image`: none, or `[sx, sy, sw, sh]`,
/// a negative size counted back from its corner, the area outside the image
/// transparent black.
fn cropped(image: &Image, crop: &[f64]) -> Result<Image, CanvasError> {
    // HTML: a source whose bitmap has a zero side, which a canvas over the
    // size limit is too.
    if image.width() == 0 || image.height() == 0 {
        return Err(CanvasError::InvalidState);
    }
    let [sx, sy, sw, sh] = match *crop {
        [] => return image.crop(None),
        [sx, sy, sw, sh] => [sx, sy, sw, sh].map(to_long).map(i64::from),
        _ => return Err(CanvasError::Type),
    };
    let (x0, w) = if sw < 0 { (sx + sw, -sw) } else { (sx, sw) };
    let (y0, h) = if sh < 0 { (sy + sh, -sh) } else { (sy, sh) };
    let (Ok(width), Ok(height)) = (u32::try_from(w), u32::try_from(h)) else {
        return Err(CanvasError::Range);
    };
    image.crop(Some((x0, y0, width, height)))
}

/// `offset..offset + length` of a buffer the caller passed with the offset
/// and length of the bytes to read.
fn view(buffer: &[u8], offset: u32, length: u32) -> Result<&[u8], String> {
    let start = offset as usize;
    start
        .checked_add(length as usize)
        .and_then(|end| buffer.get(start..end))
        .ok_or_else(|| "the bytes to read are outside the buffer".to_owned())
}

/// A number taken as a `long`: its fraction dropped, clamped to the `i32`
/// range, `NaN` as 0.
#[expect(clippy::cast_possible_truncation, reason = "the value is a long")]
fn to_long(value: f64) -> i32 {
    value as i32
}

fn undefined<T>(_: T) -> HostValue {
    HostValue::Undefined
}

/// What a string setter answers: the attribute's serialization when it took
/// the string, `undefined` when it ignored it.
fn serialization(css: Option<String>) -> HostValue {
    css.map_or(HostValue::Undefined, HostValue::String)
}

fn id_value(id: u64) -> HostValue {
    #[expect(clippy::cast_precision_loss, reason = "ids stay below 2^53")]
    HostValue::Number(id as f64)
}

fn error_value(error: CanvasError) -> HostValue {
    HostValue::String(error.name().to_owned())
}

fn outcome(result: Result<(), CanvasError>) -> HostValue {
    result.map_or_else(error_value, undefined)
}

/// Files a new object, or answers why it could not be made.
fn created(table: &CanvasTable, entry: Result<Entry, CanvasError>) -> HostValue {
    match entry {
        Ok(entry) => id_value(table.insert(entry)),
        Err(error) => error_value(error),
    }
}

/// `values` as the bytes of a `Float64Array`.
fn float_bytes(values: &[f64]) -> HostValue {
    HostValue::Bytes(
        values
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect(),
    )
}

// Argument parsers, each at one position of a member's arguments.

fn argument<'a>(arguments: &[HostArgument<'a>], index: usize) -> HostArgument<'a> {
    arguments
        .get(index)
        .copied()
        .unwrap_or(HostArgument::Undefined)
}

fn number(function: &str, arguments: &[HostArgument<'_>], index: usize) -> Result<f64, String> {
    match argument(arguments, index) {
        HostArgument::Number(value) => Ok(value),
        _ => Err(format!("{function} expects a number for argument {index}")),
    }
}

fn optional_number(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<Option<f64>, String> {
    match argument(arguments, index) {
        HostArgument::Undefined => Ok(None),
        _ => number(function, arguments, index).map(Some),
    }
}

/// A number taken as a `long` ([`to_long`]).
fn long(function: &str, arguments: &[HostArgument<'_>], index: usize) -> Result<i32, String> {
    number(function, arguments, index).map(to_long)
}

/// A size or offset: a non-negative integer, at most `u32::MAX` (a canvas
/// side beyond that has no bitmap either way).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to u32 first"
)]
fn size(function: &str, arguments: &[HostArgument<'_>], index: usize) -> Result<u32, String> {
    let value = number(function, arguments, index)?;
    if value < 0.0 || value.fract() != 0.0 {
        return Err(format!("{function} expects a size for argument {index}"));
    }
    Ok(value.min(f64::from(u32::MAX)) as u32)
}

/// An element's node id.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked to be a non-negative integer below 2^53"
)]
fn node(function: &str, arguments: &[HostArgument<'_>], index: usize) -> Result<NodeId, String> {
    let value = number(function, arguments, index)?;
    if value < 0.0 || value.fract() != 0.0 || value >= 9_007_199_254_740_992.0 {
        return Err(format!("{function} expects a node id for argument {index}"));
    }
    NodeId::from_bits(value as u64)
        .ok_or_else(|| format!("{function} got a number that is no element id: {value}"))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked to be a non-negative integer below 2^53"
)]
fn id(function: &str, arguments: &[HostArgument<'_>], index: usize) -> Result<u64, String> {
    let value = number(function, arguments, index)?;
    if value < 1.0 || value.fract() != 0.0 || value >= 9_007_199_254_740_992.0 {
        return Err(format!(
            "{function} expects an object id for argument {index}"
        ));
    }
    Ok(value as u64)
}

/// Which attribute a shared getter or setter is for: a small integer.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked to be a small integer"
)]
fn selector(function: &str, arguments: &[HostArgument<'_>], index: usize) -> Result<u8, String> {
    match argument(arguments, index) {
        HostArgument::Number(value) if value.fract() == 0.0 && (0.0..=255.0).contains(&value) => {
            Ok(value as u8)
        }
        _ => Err(format!(
            "{function} expects a selector for argument {index}"
        )),
    }
}

/// An id, or 0 for none.
fn optional_id(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<Option<u64>, String> {
    match argument(arguments, index) {
        HostArgument::Number(0.0) => Ok(None),
        _ => id(function, arguments, index).map(Some),
    }
}

fn text<'a>(
    function: &str,
    arguments: &[HostArgument<'a>],
    index: usize,
) -> Result<&'a str, String> {
    match argument(arguments, index) {
        HostArgument::String(value) => Ok(value),
        _ => Err(format!("{function} expects a string for argument {index}")),
    }
}

fn flag(function: &str, arguments: &[HostArgument<'_>], index: usize) -> Result<bool, String> {
    match argument(arguments, index) {
        HostArgument::Boolean(value) => Ok(value),
        _ => Err(format!("{function} expects a boolean for argument {index}")),
    }
}

fn fill_rule(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<FillRule, String> {
    text(function, arguments, index).and_then(|name| {
        FillRule::from_name(name)
            .ok_or_else(|| format!("{function} expects a fill rule for argument {index}"))
    })
}

fn bytes<'a>(
    function: &str,
    arguments: &[HostArgument<'a>],
    index: usize,
) -> Result<&'a [u8], String> {
    match argument(arguments, index) {
        HostArgument::Bytes(bytes) => Ok(bytes),
        _ => Err(format!(
            "{function} expects a fixed-length ArrayBuffer for argument {index}"
        )),
    }
}

/// Every number from `index` on.
fn rest(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<SmallVec<[f64; 8]>, String> {
    (index..arguments.len())
        .map(|at| number(function, arguments, at))
        .collect()
}
