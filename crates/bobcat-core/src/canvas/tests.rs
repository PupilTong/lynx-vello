//! The canvas host state and `bobcat:canvas`, run in every realm kind.

use std::sync::Arc;
use std::time::{Duration, Instant};

use terminus::link::html::canvas::{GenericFamily, ResolvedFamily, ResolvedFontStyle};
use terminus::link::peniko::Color;

use super::css::{parse_color, parse_font, serialize_color};
use crate::test_support::TestViewSpec;
use crate::view::{EngineEvent, NoWakeup};

#[test]
fn colors_parse_with_stylo_and_serialize_as_the_standard_says() {
    let rgba = |text: &str| parse_color(text).map(|color| color.to_rgba8().to_u8_array());
    assert_eq!(rgba("red"), Some([255, 0, 0, 255]));
    assert_eq!(rgba("#0f08"), Some([0, 255, 0, 136]));
    assert_eq!(rgba("hsl(240 100% 50%)"), Some([0, 0, 255, 255]));
    assert_eq!(rgba("rgb(0 0 0 / 30%)"), Some([0, 0, 0, 77]));
    // No element to inherit from: `currentcolor` is opaque black.
    assert_eq!(rgba("currentcolor"), Some([0, 0, 0, 255]));
    assert_eq!(rgba(" CurrentColor "), Some([0, 0, 0, 255]));
    assert_eq!(rgba("transparent"), Some([0, 0, 0, 0]));
    assert_eq!(rgba("not a color"), None);
    assert_eq!(rgba("red blue"), None);

    assert_eq!(
        serialize_color(Color::from_rgba8(255, 0, 16, 255)),
        "#ff0010"
    );
    assert_eq!(
        serialize_color(Color::from_rgba8(0, 0, 255, 128)),
        "rgba(0, 0, 255, 0.5)"
    );
    assert_eq!(
        serialize_color(Color::from_rgba8(0, 0, 0, 77)),
        "rgba(0, 0, 0, 0.3)"
    );
    assert_eq!(
        serialize_color(Color::from_rgba8(0, 0, 0, 0)),
        "rgba(0, 0, 0, 0)"
    );
}

#[test]
fn the_font_shorthand_parses_with_stylo() {
    // Stylo serializes a family name of plain identifiers without quotes.
    let parsed = parse_font("italic bold 20px \"Times New Roman\", serif").unwrap();
    assert_eq!(parsed.css, "italic bold 20px Times New Roman, serif");
    assert!((parsed.font.size - 20.0).abs() < f32::EPSILON);
    assert!((parsed.font.weight - 700.0).abs() < f32::EPSILON);
    assert_eq!(parsed.font.style, ResolvedFontStyle::Italic);
    assert_eq!(
        parsed.font.families,
        [
            ResolvedFamily::Named("Times New Roman".to_owned()),
            ResolvedFamily::Generic(GenericFamily::Serif)
        ]
    );
    // Relative sizes resolve against 10px; the line height is dropped.
    let size = |text: &str| parse_font(text).map(|parsed| (parsed.font.size, parsed.css));
    assert_eq!(size("2em serif"), Some((20.0, "20px serif".to_owned())));
    assert_eq!(size("150% serif"), Some((15.0, "15px serif".to_owned())));
    assert_eq!(size("larger serif"), Some((12.0, "12px serif".to_owned())));
    assert_eq!(
        size("12px/30px sans-serif"),
        Some((12.0, "12px sans-serif".to_owned()))
    );
    assert_eq!(size("medium serif").map(|(px, _)| px), Some(16.0));
    // Not a shorthand, a CSS-wide keyword, or a size that needs a viewport.
    for refused in ["inherit", "initial", "20px", "bold", "10vw serif", ""] {
        assert!(parse_font(refused).is_none(), "{refused}");
    }
}

/// Neither parser reads a document or anything of the thread it runs on.
#[test]
fn colors_and_fonts_parse_on_a_thread_with_no_document() {
    std::thread::spawn(|| {
        assert!(parse_color("rebeccapurple").is_some());
        assert_eq!(parse_font("bold 1em serif").unwrap().css, "bold 10px serif");
    })
    .join()
    .expect("the parsing thread finishes");
}

/// One suite of drawing and error checks, run in each realm kind: it
/// answers the failures it found, each as text.
const SUITE: &str = r"
async function canvasSuite() {
  const failures = [];
  const expect = (name, actual, expected) => {
    const a = JSON.stringify(actual), e = JSON.stringify(expected);
    if (a !== e) failures.push(`${name}: ${a} != ${e}`);
  };
  const throwsName = (name, call, expected) => {
    try { call(); failures.push(`${name}: did not throw`); }
    catch (error) { if (error.name !== expected) failures.push(`${name}: threw ${error.name}: ${error.message}`); }
  };
  const px = (ctx, x, y) => Array.from(ctx.getImageData(x, y, 1, 1).data);

  // Contexts and fills.
  const c = new OffscreenCanvas(20, 20);
  const ctx = c.getContext('2d');
  expect('same context', c.getContext('2d') === ctx, true);
  expect('other context id', c.getContext('webgl'), null);
  throwsName('bad context id', () => c.getContext('nope'), 'TypeError');
  expect('canvas', ctx.canvas === c, true);
  expect('instanceof', ctx instanceof OffscreenCanvasRenderingContext2D, true);
  expect('initial fillStyle', ctx.fillStyle, '#000000');
  ctx.fillStyle = 'red'; expect('fillStyle', ctx.fillStyle, '#ff0000');
  ctx.fillStyle = 'not a color'; expect('fillStyle ignored', ctx.fillStyle, '#ff0000');
  ctx.fillStyle = 'rgba(0, 0, 255, 0.5)'; expect('fillStyle rgba', ctx.fillStyle, 'rgba(0, 0, 255, 0.5)');
  ctx.fillStyle = 'currentcolor'; expect('currentcolor', ctx.fillStyle, '#000000');
  ctx.fillStyle = '#00ff00'; ctx.fillRect(0, 0, 10, 10);
  expect('fillRect', px(ctx, 5, 5), [0, 255, 0, 255]);
  expect('fillRect outside', px(ctx, 15, 15), [0, 0, 0, 0]);

  // Paths, transforms, save and restore.
  ctx.save(); ctx.translate(10, 10); ctx.fillStyle = 'blue';
  ctx.beginPath(); ctx.rect(0, 0, 10, 10); ctx.fill(); ctx.restore();
  expect('translated path', px(ctx, 15, 15), [0, 0, 255, 255]);
  expect('restored fillStyle', ctx.fillStyle, '#00ff00');
  expect('restored transform', ctx.getTransform().isIdentity, true);
  ctx.setTransform({a: 2, d: 2});
  const m = ctx.getTransform();
  expect('setTransform init', [m.a, m.m22, m.is2D, m.isIdentity], [2, 2, true, false]);
  throwsName('matrix members disagree', () => ctx.setTransform({a: 1, m11: 2}), 'TypeError');
  throwsName('setTransform arguments', () => ctx.setTransform(1, 2), 'TypeError');
  ctx.setTransform(); expect('setTransform()', ctx.getTransform().isIdentity, true);

  // clearRect and compositing.
  ctx.clearRect(0, 0, 5, 5);
  expect('clearRect', px(ctx, 2, 2), [0, 0, 0, 0]);
  expect('clearRect keeps', px(ctx, 7, 7), [0, 255, 0, 255]);
  const k = new OffscreenCanvas(10, 10).getContext('2d');
  k.fillStyle = 'red'; k.fillRect(0, 0, 10, 10);
  k.globalCompositeOperation = 'destination-out'; k.fillRect(0, 0, 5, 10);
  expect('composite', [px(k, 2, 5), px(k, 7, 5)], [[0, 0, 0, 0], [255, 0, 0, 255]]);
  k.globalCompositeOperation = 'bogus';
  expect('composite ignored', k.globalCompositeOperation, 'destination-out');
  k.globalAlpha = 2; expect('globalAlpha ignored', k.globalAlpha, 1);

  // Gradients.
  const g = new OffscreenCanvas(100, 1).getContext('2d');
  const linear = g.createLinearGradient(0, 0, 100, 0);
  linear.addColorStop(0, 'red'); linear.addColorStop(1, 'blue');
  g.fillStyle = linear; expect('gradient identity', g.fillStyle === linear, true);
  g.fillRect(0, 0, 100, 1);
  const left = px(g, 1, 0), right = px(g, 98, 0);
  if (!(left[0] > 200 && left[2] < 50 && right[2] > 200 && right[0] < 50)) failures.push(`gradient ${left} ${right}`);
  throwsName('stop offset', () => linear.addColorStop(2, 'red'), 'IndexSizeError');
  throwsName('stop color', () => linear.addColorStop(0.5, 'nope'), 'SyntaxError');
  throwsName('stop non-finite', () => linear.addColorStop(NaN, 'red'), 'TypeError');
  throwsName('gradient non-finite', () => g.createLinearGradient(0, 0, NaN, 0), 'TypeError');
  throwsName('radial radius', () => g.createRadialGradient(0, 0, -1, 0, 0, 1), 'IndexSizeError');
  expect('conic', g.createConicGradient(0, 50, 0) instanceof CanvasGradient, true);

  // A pattern of another canvas.
  const tile = new OffscreenCanvas(2, 2);
  const t = tile.getContext('2d');
  t.fillStyle = 'red'; t.fillRect(0, 0, 2, 2);
  t.fillStyle = 'blue'; t.fillRect(1, 0, 1, 1); t.fillRect(0, 1, 1, 1);
  const p = new OffscreenCanvas(4, 4).getContext('2d');
  p.fillStyle = p.createPattern(tile, 'repeat'); p.fillRect(0, 0, 4, 4);
  expect('pattern', [px(p, 0, 0), px(p, 1, 0), px(p, 2, 0), px(p, 2, 2)],
    [[255, 0, 0, 255], [0, 0, 255, 255], [255, 0, 0, 255], [255, 0, 0, 255]]);
  throwsName('pattern repetition', () => p.createPattern(tile, 'diagonal'), 'SyntaxError');
  throwsName('pattern of no pixels', () => p.createPattern(new OffscreenCanvas(0, 5), 'repeat'), 'InvalidStateError');

  // drawImage between canvases, in its three forms.
  const d = new OffscreenCanvas(10, 10).getContext('2d');
  d.drawImage(tile, 0, 0); expect('drawImage', px(d, 1, 0), [0, 0, 255, 255]);
  d.imageSmoothingEnabled = false; expect('smoothing', d.imageSmoothingEnabled, false);
  d.drawImage(tile, 4, 4, 4, 4);
  expect('drawImage scaled', [px(d, 5, 5), px(d, 7, 5)], [[255, 0, 0, 255], [0, 0, 255, 255]]);
  d.drawImage(tile, 1, 0, 1, 1, 0, 8, 2, 2); expect('drawImage source rectangle', px(d, 1, 9), [0, 0, 255, 255]);
  d.drawImage(d.canvas, 0, 0); expect('drawImage of itself', px(d, 1, 0), [0, 0, 255, 255]);
  throwsName('drawImage arguments', () => d.drawImage(tile, 0), 'TypeError');
  throwsName('drawImage source', () => d.drawImage({}, 0, 0), 'TypeError');

  // putImageData and getImageData.
  const r = new OffscreenCanvas(3, 3).getContext('2d');
  const img = r.createImageData(2, 2);
  img.data.set([10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255]);
  r.putImageData(img, 1, 1);
  expect('round trip', Array.from(r.getImageData(1, 1, 2, 2).data),
    [10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255]);
  r.clearRect(0, 0, 3, 3); r.putImageData(img, 0, 0, 1, 1, 1, 1);
  expect('dirty rectangle', [px(r, 1, 1), px(r, 0, 0)], [[100, 110, 120, 255], [0, 0, 0, 0]]);
  throwsName('getImageData zero', () => r.getImageData(0, 0, 0, 1), 'IndexSizeError');
  throwsName('putImageData arguments', () => r.putImageData(img, 0, 0, 1), 'TypeError');
  expect('negative size', r.getImageData(3, 3, -2, -2).width, 2);
  expect('createImageData of', r.createImageData(img).width, 2);
  throwsName('createImageData zero', () => r.createImageData(0, 2), 'IndexSizeError');

  // ImageData's constructor.
  expect('ImageData', [new ImageData(2, 3).data.length, new ImageData(2, 3).colorSpace], [24, 'srgb']);
  throwsName('ImageData zero', () => new ImageData(0, 1), 'IndexSizeError');
  throwsName('ImageData length', () => new ImageData(new Uint8ClampedArray(6), 1), 'InvalidStateError');
  throwsName('ImageData width', () => new ImageData(new Uint8ClampedArray(12), 2), 'IndexSizeError');
  throwsName('ImageData height', () => new ImageData(new Uint8ClampedArray(16), 2, 3), 'IndexSizeError');
  throwsName('ImageData range', () => new ImageData(-1, 1), 'TypeError');

  // transferToImageBitmap, drawImage of a bitmap, close.
  const s = new OffscreenCanvas(4, 4);
  throwsName('transfer without a context', () => s.transferToImageBitmap(), 'InvalidStateError');
  const sc = s.getContext('2d');
  sc.fillStyle = 'blue'; sc.fillRect(0, 0, 4, 4);
  const bitmap = s.transferToImageBitmap();
  expect('bitmap size', [bitmap.width, bitmap.height], [4, 4]);
  expect('transfer clears', px(sc, 1, 1), [0, 0, 0, 0]);
  expect('transfer keeps the state', sc.fillStyle, '#0000ff');
  const b = new OffscreenCanvas(4, 4).getContext('2d');
  b.drawImage(bitmap, 0, 0); expect('drawImage of a bitmap', px(b, 2, 2), [0, 0, 255, 255]);
  bitmap.close();
  expect('closed size', [bitmap.width, bitmap.height], [0, 0]);
  throwsName('drawImage of a closed bitmap', () => b.drawImage(bitmap, 0, 0), 'InvalidStateError');

  // A resize resets the bitmap and the state.
  sc.fillStyle = 'red'; sc.lineWidth = 5; sc.fillRect(0, 0, 4, 4); s.width = 4;
  expect('resize clears', px(sc, 1, 1), [0, 0, 0, 0]);
  expect('resize resets', [sc.fillStyle, sc.lineWidth], ['#000000', 1]);
  throwsName('width range', () => { s.width = -1; }, 'TypeError');
  throwsName('constructor arguments', () => new OffscreenCanvas(1), 'TypeError');

  // Text.
  const tx = new OffscreenCanvas(100, 30).getContext('2d');
  expect('default font', tx.font, '10px sans-serif');
  tx.font = 'bold 20px serif'; expect('font', tx.font, 'bold 20px serif');
  tx.font = 'inherit'; expect('font ignored', tx.font, 'bold 20px serif');
  tx.font = '2em sans-serif'; expect('font em', tx.font, '20px sans-serif');
  tx.save(); tx.font = '12px/30px serif'; expect('font line height', tx.font, '12px serif');
  tx.restore(); expect('font restored', tx.font, '20px sans-serif');
  expect('measure nothing', tx.measureText('').width, 0);
  const one = tx.measureText('a').width, two = tx.measureText('aa').width;
  if (!(one > 0 && two > one * 1.5 && two < one * 2.5)) failures.push(`measureText ${one} ${two}`);
  tx.textAlign = 'center'; tx.textAlign = 'middle'; expect('textAlign', tx.textAlign, 'center');
  tx.textAlign = 'start';
  tx.fillText('Hello', 5, 22);
  expect('fillText inks', tx.getImageData(0, 0, 100, 30).data.some((v, i) => i % 4 === 3 && v > 0), true);

  // Line styles, dashes and the errors of path building.
  const l = new OffscreenCanvas(10, 10).getContext('2d');
  l.setLineDash([1, 2, 3]); expect('line dash', l.getLineDash(), [1, 2, 3, 1, 2, 3]);
  l.setLineDash([1, -1]); expect('line dash ignored', l.getLineDash(), [1, 2, 3, 1, 2, 3]);
  l.setLineDash([]);
  l.lineWidth = 0; expect('lineWidth ignored', l.lineWidth, 1);
  l.lineCap = 'round'; l.lineCap = 'x'; expect('lineCap', l.lineCap, 'round');
  throwsName('arc radius', () => l.arc(0, 0, -1, 0, 1), 'IndexSizeError');
  throwsName('roundRect radii', () => l.roundRect(0, 0, 5, 5, [1, 2, 3, 4, 5]), 'RangeError');
  throwsName('fill rule', () => l.fill('sideways'), 'TypeError');
  throwsName('missing argument', () => l.fillRect(0, 0, 1), 'TypeError');
  l.lineWidth = 2; l.strokeStyle = 'red'; l.strokeRect(2, 2, 6, 6);
  expect('strokeRect', px(l, 2, 5), [255, 0, 0, 255]);

  // Path2D and the hit tests.
  const path = new Path2D(); path.rect(1, 1, 4, 4);
  expect('isPointInPath', [l.isPointInPath(path, 2, 2), l.isPointInPath(path, 8, 8)], [true, false]);
  expect('svg path', l.isPointInPath(new Path2D('M0 0 L10 0 L10 10 Z'), 8, 2), true);
  const copy = new Path2D(path); copy.addPath(new Path2D('M6 6 h3 v3 h-3 Z'));
  expect('addPath', [l.isPointInPath(copy, 7, 7), l.isPointInPath(path, 7, 7)], [true, false]);
  expect('isPointInStroke', l.isPointInStroke(path, 1, 3), true);
  l.clearRect(0, 0, 10, 10); l.fillStyle = 'lime'; l.fill(path);
  expect('fill a Path2D', px(l, 3, 3), [0, 255, 0, 255]);
  l.beginPath(); l.roundRect(0, 0, 10, 10, 2); expect('roundRect', l.isPointInPath(5, 5), true);

  // createImageBitmap.
  const data = new ImageData(new Uint8ClampedArray([255, 0, 0, 255, 0, 255, 0, 255]), 2);
  const fromData = await createImageBitmap(data);
  expect('bitmap of ImageData', [fromData.width, fromData.height], [2, 1]);
  const cb = new OffscreenCanvas(2, 1).getContext('2d');
  cb.drawImage(fromData, 0, 0); expect('bitmap pixels', px(cb, 1, 0), [0, 255, 0, 255]);
  const cropped = await createImageBitmap(data, 1, 0, 1, 1);
  cb.clearRect(0, 0, 2, 1); cb.drawImage(cropped, 0, 0);
  expect('cropped bitmap', [cropped.width, px(cb, 0, 0)], [1, [0, 255, 0, 255]]);
  expect('bitmap of a canvas', (await createImageBitmap(tile)).width, 2);
  for (const [name, call, expected] of [
    ['bitmap option', () => createImageBitmap(data, {imageOrientation: 'flipY'}), 'NotSupportedError'],
    ['bitmap resize', () => createImageBitmap(data, {resizeWidth: 4}), 'NotSupportedError'],
    ['bitmap source', () => createImageBitmap({}), 'TypeError'],
    ['bitmap crop', () => createImageBitmap(data, 0, 0, 0, 1), 'RangeError'],
    ['bitmap of a closed bitmap', () => createImageBitmap(bitmap), 'InvalidStateError'],
  ]) {
    try { await call(); failures.push(`${name}: resolved`); }
    catch (error) { if (error.name !== expected) failures.push(`${name}: ${error.name}`); }
  }

  // DOMException and the interfaces script cannot construct.
  const e = new DOMException('m', 'IndexSizeError');
  expect('DOMException', [e.name, e.message, e.code, e instanceof Error, DOMException.INDEX_SIZE_ERR],
    ['IndexSizeError', 'm', 1, true, 1]);
  for (const name of ['TextMetrics', 'CanvasGradient', 'CanvasPattern', 'OffscreenCanvasRenderingContext2D', 'ImageBitmap']) {
    throwsName(`new ${name}`, () => new globalThis[name](), 'TypeError');
  }
  return failures;
}
function runCanvasSuite(realm) {
  canvasSuite().then(
    failures => console.log(`canvas ${realm} ${JSON.stringify(failures)}`),
    error => console.log(`canvas ${realm} threw ${error} ${error.stack}`));
}
";

const WORKER_URL: &str = "app:///canvas-worker.js";

/// The suite passes in the MTS realm, the BTS realm and a plain `Worker`,
/// whose global `OffscreenCanvas` and the rest come from the module each
/// imports for its effect: `bobcat:runtime`, `bobcat:bts`, and the worker
/// script's own `import "bobcat:canvas"`.
#[test]
fn the_canvas_suite_passes_in_every_realm_kind() {
    let main = format!(
        "{SUITE}
import {{ Worker }} from 'bobcat-internal';
__CreatePage();
globalThis.canvasWorker = new Worker('{WORKER_URL}');
runCanvasSuite('main');
"
    );
    let background = format!("{SUITE}\nrunCanvasSuite('background');\n");
    let worker = format!(
        "import 'bobcat:worker';\nimport 'bobcat:canvas';\n{SUITE}\nrunCanvasSuite('worker');\n"
    );
    let mut engine = TestViewSpec::new(&main)
        .with_background_entry(&background)
        .with_module(WORKER_URL, &worker)
        .create(Arc::new(NoWakeup));
    let deadline = Instant::now() + Duration::from_mins(2);
    let mut results = Vec::new();
    while results.len() < 3 {
        for event in engine.pump() {
            match event {
                EngineEvent::ConsoleMessage { message, .. } if message.starts_with("canvas ") => {
                    results.push(message);
                }
                EngineEvent::ScriptReported { message, .. } => panic!("reported: {message}"),
                EngineEvent::ScriptRunError(error) => panic!("the entry failed: {error}"),
                EngineEvent::WorkerThrew { source, error } => panic!("{source:?} threw: {error}"),
                EngineEvent::StartupFailed(error) => panic!("the view did not boot: {error}"),
                EngineEvent::Panicked(error) => panic!("the engine panicked: {error}"),
                _ => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "the suites did not finish: {results:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    results.sort();
    assert_eq!(
        results,
        ["canvas background []", "canvas main []", "canvas worker []"]
    );
}
