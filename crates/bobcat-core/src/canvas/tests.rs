//! The canvas host state and the canvas members of `bobcat-internal:host`,
//! run in every realm kind.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::test_support::TestViewSpec;
use crate::view::{EngineEvent, NoWakeup};

/// One suite of drawing and error checks over the canvas members, as a
/// module every realm kind imports: `run(realm, report)` reports
/// `canvas <realm> <failures>`, the failures it found as JSON text.
const SUITE: &str = r"
import * as H from 'bobcat-internal:host';

function canvasSuite() {
  const failures = [];
  const expect = (name, actual, expected) => {
    const a = JSON.stringify(actual), e = JSON.stringify(expected);
    if (a !== e) failures.push(name + ': ' + a + ' != ' + e);
  };
  const throwsMisuse = (name, call, member) => {
    try { call(); failures.push(name + ': did not throw'); }
    catch (error) {
      if (error.name !== 'InternalError'
          || !String(error.message).startsWith('bobcat-internal:host.' + member + ':')
          && !String(error.message).startsWith('bobcat-internal:host.' + member + ' ')) {
        failures.push(name + ': threw ' + error.name + ': ' + error.message);
      }
    }
  };
  const bytes = (buffer) => Array.from(new Uint8Array(buffer));
  const floats = (buffer) => Array.from(new Float64Array(buffer));
  const px = (canvas, x, y) => bytes(H.canvasGetImageData(canvas, x, y, 1, 1));
  const FILL = 0, STROKE = 1;
  const GLOBAL_ALPHA = 0, LINE_WIDTH = 4;
  const COMPOSITE = 0, LINE_CAP = 2, TEXT_ALIGN = 4;

  // Canvases and fills.
  const c = H.canvasCreate(20, 20);
  expect('an id', typeof c === 'number' && c >= 1, true);
  expect('fillStyle', H.canvasSetColor(c, FILL, 'red'), '#ff0000');
  expect('not a color', H.canvasSetColor(c, FILL, 'not a color'), undefined);
  expect('rgba', H.canvasSetColor(c, FILL, 'rgba(0, 0, 255, 0.5)'), 'rgba(0, 0, 255, 0.5)');
  expect('currentcolor', H.canvasSetColor(c, FILL, 'currentcolor'), '#000000');
  H.canvasSetColor(c, FILL, '#00ff00');
  H.canvasFillRect(c, 0, 0, 10, 10);
  expect('fillRect', px(c, 5, 5), [0, 255, 0, 255]);
  expect('fillRect outside', px(c, 15, 15), [0, 0, 0, 0]);

  // Paths, transforms, save and restore.
  H.canvasSave(c); H.canvasTranslate(c, 10, 10); H.canvasSetColor(c, FILL, 'blue');
  H.canvasBeginPath(c); H.canvasRect(c, 0, 0, 10, 10); H.canvasFill(c, 0, 'nonzero');
  H.canvasRestore(c);
  expect('translated path', px(c, 15, 15), [0, 0, 255, 255]);
  expect('restored transform', floats(H.canvasGetTransform(c)), [1, 0, 0, 1, 0, 0]);
  H.canvasFillRect(c, 10, 0, 1, 1);
  expect('restored fillStyle', px(c, 10, 0), [0, 255, 0, 255]);
  H.canvasSetTransform(c, 2, 0, 0, 2, 0, 0);
  expect('setTransform', floats(H.canvasGetTransform(c)), [2, 0, 0, 2, 0, 0]);
  H.canvasResetTransform(c);

  // clearRect and compositing.
  H.canvasClearRect(c, 0, 0, 5, 5);
  expect('clearRect', px(c, 2, 2), [0, 0, 0, 0]);
  expect('clearRect keeps', px(c, 7, 7), [0, 255, 0, 255]);
  const k = H.canvasCreate(10, 10);
  H.canvasSetColor(k, FILL, 'red'); H.canvasFillRect(k, 0, 0, 10, 10);
  H.canvasSetKeyword(k, COMPOSITE, 'destination-out'); H.canvasFillRect(k, 0, 0, 5, 10);
  expect('composite', [px(k, 2, 5), px(k, 7, 5)], [[0, 0, 0, 0], [255, 0, 0, 255]]);
  H.canvasSetKeyword(k, COMPOSITE, 'bogus');
  expect('composite ignored', H.canvasGetKeyword(k, COMPOSITE), 'destination-out');
  H.canvasSetNumber(k, GLOBAL_ALPHA, 2);
  expect('globalAlpha ignored', H.canvasGetNumber(k, GLOBAL_ALPHA), 1);

  // Gradients.
  const g = H.canvasCreate(100, 1);
  const linear = H.canvasLinearGradient(0, 0, 100, 0);
  expect('add stop', H.canvasAddColorStop(linear, 0, 'red'), undefined);
  H.canvasAddColorStop(linear, 1, 'blue');
  H.canvasSetPaint(g, FILL, linear); H.canvasFillRect(g, 0, 0, 100, 1);
  const left = px(g, 1, 0), right = px(g, 98, 0);
  if (!(left[0] > 200 && left[2] < 50 && right[2] > 200 && right[0] < 50)) {
    failures.push('gradient ' + left + ' ' + right);
  }
  expect('stop offset', H.canvasAddColorStop(linear, 2, 'red'), 'IndexSizeError');
  expect('stop color', H.canvasAddColorStop(linear, 0.5, 'nope'), 'SyntaxError');
  expect('stop non-finite', H.canvasAddColorStop(linear, NaN, 'nope'), 'TypeError');
  expect('gradient non-finite', H.canvasLinearGradient(0, 0, NaN, 0), 'TypeError');
  expect('radial radius', H.canvasRadialGradient(0, 0, -1, 0, 0, 1), 'IndexSizeError');
  expect('conic', typeof H.canvasConicGradient(0, 50, 0), 'number');

  // A pattern of another canvas.
  const tile = H.canvasCreate(2, 2);
  H.canvasSetColor(tile, FILL, 'red'); H.canvasFillRect(tile, 0, 0, 2, 2);
  H.canvasSetColor(tile, FILL, 'blue');
  H.canvasFillRect(tile, 1, 0, 1, 1); H.canvasFillRect(tile, 0, 1, 1, 1);
  const p = H.canvasCreate(4, 4);
  const pattern = H.canvasCreatePattern(tile, 'repeat');
  H.canvasSetPaint(p, FILL, pattern); H.canvasFillRect(p, 0, 0, 4, 4);
  expect('pattern', [px(p, 0, 0), px(p, 1, 0), px(p, 2, 0), px(p, 2, 2)],
    [[255, 0, 0, 255], [0, 0, 255, 255], [255, 0, 0, 255], [255, 0, 0, 255]]);
  expect('pattern repetition', H.canvasCreatePattern(tile, 'diagonal'), 'SyntaxError');
  expect('pattern of no pixels', H.canvasCreatePattern(H.canvasCreate(0, 5), 'repeat'),
    'InvalidStateError');

  // drawImage between canvases, in its three forms.
  const d = H.canvasCreate(10, 10);
  H.canvasDrawImage(d, tile, 0, 0); expect('drawImage', px(d, 1, 0), [0, 0, 255, 255]);
  H.canvasSetSmoothing(d, false); expect('smoothing', H.canvasGetSmoothing(d), false);
  H.canvasDrawImage(d, tile, 4, 4, 4, 4);
  expect('drawImage scaled', [px(d, 5, 5), px(d, 7, 5)], [[255, 0, 0, 255], [0, 0, 255, 255]]);
  H.canvasDrawImage(d, tile, 1, 0, 1, 1, 0, 8, 2, 2);
  expect('drawImage source rectangle', px(d, 1, 9), [0, 0, 255, 255]);
  H.canvasDrawImage(d, d, 0, 0); expect('drawImage of itself', px(d, 1, 0), [0, 0, 255, 255]);
  expect('drawImage arguments', H.canvasDrawImage(d, tile, 0), 'TypeError');

  // putImageData and getImageData.
  const r = H.canvasCreate(3, 3);
  const pixels = new Uint8Array([10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255]);
  expect('put', H.canvasPutImageData(r, pixels.buffer, 0, 16, 2, 2, 1, 1), undefined);
  expect('round trip', bytes(H.canvasGetImageData(r, 1, 1, 2, 2)), Array.from(pixels));
  H.canvasClearRect(r, 0, 0, 3, 3);
  H.canvasPutImageData(r, pixels.buffer, 0, 16, 2, 2, 0, 0, 1, 1, 1, 1);
  expect('dirty rectangle', [px(r, 1, 1), px(r, 0, 0)], [[100, 110, 120, 255], [0, 0, 0, 0]]);
  expect('getImageData zero', H.canvasGetImageData(r, 0, 0, 0, 1), 'IndexSizeError');
  expect('negative size', H.canvasGetImageData(r, 3, 3, -2, -2).byteLength, 16);

  // canvasTransferToBitmap, drawImage of a bitmap, a dropped bitmap.
  const s = H.canvasCreate(4, 4);
  H.canvasSetColor(s, FILL, 'blue'); H.canvasFillRect(s, 0, 0, 4, 4);
  const bitmap = H.canvasTransferToBitmap(s);
  expect('bitmap size', [H.canvasBitmapWidth(bitmap), H.canvasBitmapHeight(bitmap)], [4, 4]);
  expect('transfer clears', px(s, 1, 1), [0, 0, 0, 0]);
  const b = H.canvasCreate(4, 4);
  H.canvasDrawImage(b, bitmap, 0, 0); expect('drawImage of a bitmap', px(b, 2, 2), [0, 0, 255, 255]);
  H.canvasDrop(bitmap);
  throwsMisuse('a dropped bitmap', () => H.canvasDrawImage(b, bitmap, 0, 0), 'canvasDrawImage');

  // A new size resets the bitmap and the state.
  H.canvasSetColor(s, FILL, 'red'); H.canvasSetNumber(s, LINE_WIDTH, 5);
  H.canvasFillRect(s, 0, 0, 4, 4); H.canvasSetSize(s, 4, 4);
  expect('resize clears', px(s, 1, 1), [0, 0, 0, 0]);
  expect('resize resets', H.canvasGetNumber(s, LINE_WIDTH), 1);

  // Text.
  const tx = H.canvasCreate(100, 30);
  expect('font', H.canvasSetFont(tx, 'bold 20px serif'), 'bold 20px serif');
  expect('font ignored', H.canvasSetFont(tx, 'inherit'), undefined);
  expect('font em', H.canvasSetFont(tx, '2em sans-serif'), '20px sans-serif');
  expect('font line height', H.canvasSetFont(tx, '12px/30px serif'), '12px serif');
  H.canvasSetFont(tx, '20px sans-serif');
  expect('measure nothing', floats(H.canvasMeasureText(tx, ''))[0], 0);
  const one = floats(H.canvasMeasureText(tx, 'a'))[0], two = floats(H.canvasMeasureText(tx, 'aa'))[0];
  if (!(one > 0 && two > one * 1.5 && two < one * 2.5)) failures.push('measureText ' + one + ' ' + two);
  H.canvasSetKeyword(tx, TEXT_ALIGN, 'center'); H.canvasSetKeyword(tx, TEXT_ALIGN, 'middle');
  expect('textAlign', H.canvasGetKeyword(tx, TEXT_ALIGN), 'center');
  H.canvasSetKeyword(tx, TEXT_ALIGN, 'start');
  H.canvasFillText(tx, 'Hello', 5, 22);
  expect('fillText inks', new Uint8Array(H.canvasGetImageData(tx, 0, 0, 100, 30)).some((v, i) => i % 4 === 3 && v > 0), true);

  // Line styles, dashes and the errors of path building.
  const l = H.canvasCreate(10, 10);
  H.canvasSetLineDash(l, 1, 2, 3); expect('line dash', floats(H.canvasGetLineDash(l)), [1, 2, 3, 1, 2, 3]);
  H.canvasSetLineDash(l, 1, -1); expect('line dash ignored', floats(H.canvasGetLineDash(l)), [1, 2, 3, 1, 2, 3]);
  H.canvasSetLineDash(l);
  H.canvasSetNumber(l, LINE_WIDTH, 0); expect('lineWidth ignored', H.canvasGetNumber(l, LINE_WIDTH), 1);
  H.canvasSetKeyword(l, LINE_CAP, 'round'); H.canvasSetKeyword(l, LINE_CAP, 'x');
  expect('lineCap', H.canvasGetKeyword(l, LINE_CAP), 'round');
  expect('arc radius', H.canvasArc(l, 0, 0, -1, 0, 1, false), 'IndexSizeError');
  expect('roundRect radii', H.canvasRoundRect(l, 0, 0, 5, 5, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5), 'RangeError');
  H.canvasSetNumber(l, LINE_WIDTH, 2); H.canvasSetColor(l, STROKE, 'red'); H.canvasStrokeRect(l, 2, 2, 6, 6);
  expect('strokeRect', px(l, 2, 5), [255, 0, 0, 255]);

  // Paths and the hit tests.
  const path = H.canvasPath(0); H.canvasRect(path, 1, 1, 4, 4);
  expect('isPointInPath', [H.canvasIsPointInPath(l, path, 2, 2, 'nonzero'),
    H.canvasIsPointInPath(l, path, 8, 8, 'nonzero')], [true, false]);
  expect('svg path', H.canvasIsPointInPath(l, H.canvasPathFromSvg('M0 0 L10 0 L10 10 Z'), 8, 2, 'nonzero'), true);
  const copy = H.canvasPath(path);
  H.canvasAddPath(copy, H.canvasPathFromSvg('M6 6 h3 v3 h-3 Z'), 1, 0, 0, 1, 0, 0);
  expect('addPath', [H.canvasIsPointInPath(l, copy, 7, 7, 'nonzero'),
    H.canvasIsPointInPath(l, path, 7, 7, 'nonzero')], [true, false]);
  expect('isPointInStroke', H.canvasIsPointInStroke(l, path, 1, 3), true);
  H.canvasClearRect(l, 0, 0, 10, 10); H.canvasSetColor(l, FILL, 'lime'); H.canvasFill(l, path, 'nonzero');
  expect('fill a path', px(l, 3, 3), [0, 255, 0, 255]);
  H.canvasBeginPath(l); H.canvasRoundRect(l, 0, 0, 10, 10, 2, 2);
  expect('roundRect', H.canvasIsPointInPath(l, 0, 5, 5, 'nonzero'), true);

  // Image bitmaps of pixels and of a canvas, cropped and not.
  const data = new Uint8Array([255, 0, 0, 255, 0, 255, 0, 255]);
  const fromData = H.canvasBitmapFromPixels(data.buffer, 0, 8, 2, 1);
  expect('bitmap of pixels', [H.canvasBitmapWidth(fromData), H.canvasBitmapHeight(fromData)], [2, 1]);
  const cb = H.canvasCreate(2, 1);
  H.canvasDrawImage(cb, fromData, 0, 0); expect('bitmap pixels', px(cb, 1, 0), [0, 255, 0, 255]);
  const cropped = H.canvasBitmapFromPixels(data.buffer, 0, 8, 2, 1, 1, 0, 1, 1);
  H.canvasClearRect(cb, 0, 0, 2, 1); H.canvasDrawImage(cb, cropped, 0, 0);
  expect('cropped bitmap', [H.canvasBitmapWidth(cropped), px(cb, 0, 0)], [1, [0, 255, 0, 255]]);
  expect('bitmap of a canvas', H.canvasBitmapWidth(H.canvasBitmapFrom(tile)), 2);
  expect('bitmap crop', H.canvasBitmapFrom(tile, 0, 0, 0, 1), 'RangeError');
  // A crop side past the longest texture is refused before the GPU.
  expect('bitmap crops the device cannot hold', [
    H.canvasBitmapFrom(tile, 0, 0, 20000, 16), H.canvasBitmapFrom(tile, 0, 0, 16, 20000),
  ], ['InvalidStateError', 'InvalidStateError']);

  // A value of the wrong type, or an id of nothing of the kind, throws the
  // bridge's `InternalError`, its message starting with the member's name.
  throwsMisuse('a string for a number', () => H.canvasFillRect(c, 0, 0, '1', 1), 'canvasFillRect');
  throwsMisuse('no such canvas', () => H.canvasFillRect(999999, 0, 0, 1, 1), 'canvasFillRect');
  // The id is checked whether or not the string parses.
  throwsMisuse('no such canvas, a color', () => H.canvasSetColor(999999, FILL, 'red'),
    'canvasSetColor');
  throwsMisuse('no such canvas, not a color',
    () => H.canvasSetColor(999999, FILL, 'not a color'), 'canvasSetColor');
  throwsMisuse('no such canvas, a font', () => H.canvasSetFont(999999, '10px serif'),
    'canvasSetFont');
  throwsMisuse('no such canvas, not a font', () => H.canvasSetFont(999999, 'not a font'),
    'canvasSetFont');
  throwsMisuse('a path for a canvas', () => H.canvasFillRect(path, 0, 0, 1, 1), 'canvasFillRect');
  throwsMisuse('a fill rule', () => H.canvasFill(l, 0, 'sideways'), 'canvasFill');
  throwsMisuse('a negative size', () => H.canvasCreate(-1, 1), 'canvasCreate');
  H.canvasDrop(c);
  throwsMisuse('a dropped canvas', () => H.canvasFillRect(c, 0, 0, 1, 1), 'canvasFillRect');
  return failures;
}

export function run(realm, report) {
  let message;
  try {
    message = 'canvas ' + realm + ' ' + JSON.stringify(canvasSuite());
  } catch (error) {
    message = 'canvas ' + realm + ' threw ' + error + ' ' + error.stack;
  }
  report(message);
}
";

/// What a realm reports of the canvas names a card might look for: the
/// `typeof` of each would-be global, then how `import('bobcat:canvas')`
/// ends.
const GLOBALS: &str = r"
globalThis.canvasGlobals = (realm, report) => {
  const names = ['OffscreenCanvas', 'Path2D', 'ImageData', 'ImageBitmap', 'DOMException',
    'createImageBitmap', 'OffscreenCanvasRenderingContext2D', 'CanvasGradient'];
  const types = names.map((name) => typeof globalThis[name]).join(',');
  import('bobcat:canvas').then(
    () => report('globals ' + realm + ' ' + types + ' imported'),
    (error) => report('globals ' + realm + ' ' + types + ' ' + error.name));
};
";

const SUITE_URL: &str = "app:///canvas-suite.js";
const WORKER_URL: &str = "app:///canvas-worker.js";

/// Pumps `engine` until `count` console messages starting with `prefix` came,
/// failing on any script failure, and answers them sorted.
fn messages(
    engine: &mut crate::test_support::TestEngine,
    prefix: &str,
    count: usize,
) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_mins(2);
    let mut results = Vec::new();
    while results.len() < count {
        for event in engine.pump() {
            match event {
                EngineEvent::ConsoleMessage { message, .. } if message.starts_with(prefix) => {
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
            "not all of them came: {results:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    results.sort();
    results
}

/// The suite passes over the members of the MTS realm, a `ShadowRealm`
/// child of it, the BTS realm and a plain `Worker`: each realm kind has
/// every canvas member over a table of its own.
#[test]
fn the_canvas_members_pass_the_suite_in_every_realm_kind() {
    let main = format!(
        "
import {{ Worker }} from 'bobcat-internal';
import {{ run }} from '{SUITE_URL}';
__CreatePage();
globalThis.canvasWorker = new Worker('{WORKER_URL}');
run('main', (message) => console.log(message));
const child = new ShadowRealm();
child.importValue('{SUITE_URL}', 'run').then((run) => run('shadow', (message) => console.log(message)));
"
    );
    let background = format!(
        "import {{ run }} from '{SUITE_URL}';\nrun('background', (message) => console.log(message));\n"
    );
    let worker = format!(
        "import 'bobcat:worker';\nimport {{ run }} from '{SUITE_URL}';\nrun('worker', (message) => console.log(message));\n"
    );
    let mut engine = TestViewSpec::new(&main)
        .with_background_entry(&background)
        .with_module(SUITE_URL, SUITE)
        .with_module(WORKER_URL, &worker)
        .create(Arc::new(NoWakeup));
    assert_eq!(
        messages(&mut engine, "canvas ", 4),
        [
            "canvas background []",
            "canvas main []",
            "canvas shadow []",
            "canvas worker []"
        ]
    );
}

/// A card has no canvas global in any realm kind, and `bobcat:canvas` is no
/// module: its import fails in the realm with a `ReferenceError`.
#[test]
fn a_card_has_no_canvas_globals_and_no_canvas_module() {
    let main = format!(
        "{GLOBALS}
import {{ Worker }} from 'bobcat-internal';
__CreatePage();
globalThis.canvasWorker = new Worker('{WORKER_URL}');
canvasGlobals('main', (message) => console.log(message));
"
    );
    let background =
        format!("{GLOBALS}\ncanvasGlobals('background', (message) => console.log(message));\n");
    let worker = format!(
        "import 'bobcat:worker';\n{GLOBALS}\ncanvasGlobals('worker', (message) => console.log(message));\n"
    );
    let mut engine = TestViewSpec::new(&main)
        .with_background_entry(&background)
        .with_module(WORKER_URL, &worker)
        .create(Arc::new(NoWakeup));
    let undefined = ["undefined"; 8].join(",");
    assert_eq!(
        messages(&mut engine, "globals ", 3),
        [
            format!("globals background {undefined} ReferenceError"),
            format!("globals main {undefined} ReferenceError"),
            format!("globals worker {undefined} ReferenceError"),
        ]
    );
}
