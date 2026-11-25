/**
 * The canvas members of `bobcat-internal:host`, in every realm kind: the
 * Canvas 2D API as functions over ids. They are the surface an embedder's
 * native-module ES module (`lynx:<name>`, `NativeModule::esm` in
 * `crates/bobcat-core/src/native_module.rs`) and the engine's built-in
 * modules call. Nothing checks the caller, and no realm has an
 * `OffscreenCanvas` class or any other canvas global.
 * `crates/bobcat-core/docs/canvas.md` describes them, and
 * `crates/bobcat-core/src/canvas/` implements them.
 *
 * The member contract:
 *
 * - A member converts nothing. Arguments are numbers, strings, booleans,
 *   and a fixed-length `ArrayBuffer` with the offset and length of the bytes
 *   to read in it; lists of numbers are trailing arguments.
 * - A value of the wrong type, or an id naming nothing of the kind the
 *   member takes, throws the bridge's `InternalError`, whose message starts
 *   with the member's full name.
 * - A failure the HTML Standard defines throws nothing: the member answers
 *   the exception's name as a string (`CanvasErrorName`), and the caller
 *   decides what to do with it.
 * - A member that answers several numbers answers a `Float64Array`'s bytes
 *   as an `ArrayBuffer`.
 *
 * Each canvas, path, gradient, pattern and image bitmap is an entry of the
 * realm's own table under an id, from 1 and never reused. An entry lives
 * until `canvasDrop(id)`, `canvasDetach(id)` or the realm's end; nothing
 * drops one when the caller stops using its id. A canvas `target` names a
 * canvas; the path members also take a path's id there. A `which` selects
 * one attribute of a shared getter and setter, in the order its comment
 * lists. `fillStyle`, `strokeStyle`, `shadowColor` and `font` have setters
 * only: `canvasSetColor` and `canvasSetFont` answer the serialization of
 * what they set, and nothing reads the value back.
 *
 * The bytecode an embedder compiles with `compile_native_module_esm` is
 * tied to one engine build: a view refuses bytecode another build wrote. A
 * module is therefore compiled against this file as it is at the revision of
 * the engine that runs it. A change to a member's name, parameters or answer
 * changes this file and the member list in `docs/canvas.md` in the same
 * change; a test in `crates/bobcat-core/src/canvas/tests.rs` holds the names
 * exported here equal to the ones the engine registers.
 *
 * This file and `native.d.ts` are both ambient declaration files that
 * declare `bobcat-internal:host`; TypeScript merges the two declarations.
 */

/**
 * The name of the exception a canvas member answers instead of a value, as
 * the HTML Standard names the failure: a `DOMException` name
 * (`"IndexSizeError"`, `"InvalidStateError"`, `"SyntaxError"`) or
 * `"TypeError"`/`"RangeError"`. The member throws nothing for it; the caller
 * decides what to do with the name.
 */
type CanvasErrorName = string;

/** The canvas members, one per Canvas 2D method or attribute, each with its answer. */
interface BobcatCanvasNative {
  /** A transparent black `width` × `height` canvas, its context in its default state. */
  canvasCreate(width: number, height: number): number;
  /** Removes the entry `id` names, whatever its kind. */
  canvasDrop(id: number): void;
  /** The canvas's new size, as setting `width`/`height` does: the bitmap and the context state reset. */
  canvasSetSize(target: number, width: number, height: number): void;
  /**
   * Takes the canvas or image bitmap `target` out of this realm's table and
   * puts it in the view's hand-over store, answering its token: a number
   * any realm of the same view can pass to `canvasAttach`. A canvas goes as
   * it is — bitmap, context state, draws not yet replayed, its placeholder
   * binding. `target` names nothing afterwards. An id of a path, gradient or
   * pattern, or of nothing, throws as any wrong id does.
   */
  canvasDetach(target: number): number;
  /**
   * Takes the entry the view's hand-over store holds under `token` into
   * this realm's table, answering `[id, width, height]` in a
   * `Float64Array`'s bytes; `"InvalidStateError"` for a token that names
   * nothing in the store (never issued, already attached, another view's).
   */
  canvasAttach(token: number): ArrayBuffer | CanvasErrorName;
  /** A new path (HTML's `Path2D`): empty for `source` 0, else a copy of that path. */
  canvasPath(source: number): number;
  /** A new path of SVG path data (empty for data with an error). */
  canvasPathFromSvg(data: string): number;
  canvasAddPath(target: number, source: number, a: number, b: number, c: number, d: number,
    e: number, f: number): void;
  canvasSave(target: number): void;
  canvasRestore(target: number): void;
  canvasReset(target: number): void;
  canvasScale(target: number, x: number, y: number): void;
  canvasRotate(target: number, angle: number): void;
  canvasTranslate(target: number, x: number, y: number): void;
  canvasTransform(target: number, a: number, b: number, c: number, d: number, e: number,
    f: number): void;
  canvasSetTransform(target: number, a: number, b: number, c: number, d: number, e: number,
    f: number): void;
  /** The current transform's `a`..`f` as a `Float64Array`'s buffer. */
  canvasGetTransform(target: number): ArrayBuffer;
  canvasResetTransform(target: number): void;
  /**
   * `which`: 0 `globalAlpha`, 1 `shadowBlur`, 2 `shadowOffsetX`, 3
   * `shadowOffsetY`, 4 `lineWidth`, 5 `miterLimit`, 6 `lineDashOffset`.
   */
  canvasGetNumber(target: number, which: number): number;
  /** Ignored where the attribute ignores the value. */
  canvasSetNumber(target: number, which: number, value: number): void;
  /**
   * `which`: 0 `globalCompositeOperation`, 1 `imageSmoothingQuality`, 2
   * `lineCap`, 3 `lineJoin`, 4 `textAlign`, 5 `textBaseline`, 6
   * `direction`.
   */
  canvasGetKeyword(target: number, which: number): string;
  /** A string the attribute does not take is ignored. */
  canvasSetKeyword(target: number, which: number, value: string): void;
  canvasGetSmoothing(target: number): boolean;
  canvasSetSmoothing(target: number, enabled: boolean): void;
  /**
   * Parses `css` as a CSS color (`currentcolor` is opaque black) and sets
   * `which` — 0 `fillStyle`, 1 `strokeStyle`, 2 `shadowColor` — to it,
   * answering the color's serialization; `undefined`, changing nothing, for
   * a string that is not a color.
   */
  canvasSetColor(target: number, which: number, css: string): string | undefined;
  /** Sets `which` — 0 `fillStyle`, 1 `strokeStyle` — to a gradient or pattern. */
  canvasSetPaint(target: number, which: number, paint: number): void;
  canvasLinearGradient(x0: number, y0: number, x1: number, y1: number): number | CanvasErrorName;
  canvasRadialGradient(x0: number, y0: number, r0: number, x1: number, y1: number,
    r1: number): number | CanvasErrorName;
  canvasConicGradient(angle: number, x: number, y: number): number | CanvasErrorName;
  canvasAddColorStop(gradient: number, offset: number, css: string): CanvasErrorName | undefined;
  /** `source` is a canvas or an image bitmap; `repetition` "" is `repeat`. */
  canvasCreatePattern(source: number, repetition: string): number | CanvasErrorName;
  canvasPatternSetTransform(pattern: number, a: number, b: number, c: number, d: number,
    e: number, f: number): CanvasErrorName | undefined;
  canvasClearRect(target: number, x: number, y: number, w: number, h: number): void;
  canvasFillRect(target: number, x: number, y: number, w: number, h: number): void;
  canvasStrokeRect(target: number, x: number, y: number, w: number, h: number): void;
  canvasBeginPath(target: number): void;
  canvasClosePath(target: number): void;
  canvasMoveTo(target: number, x: number, y: number): void;
  canvasLineTo(target: number, x: number, y: number): void;
  canvasQuadraticCurveTo(target: number, cpx: number, cpy: number, x: number, y: number): void;
  canvasBezierCurveTo(target: number, cp1x: number, cp1y: number, cp2x: number, cp2y: number,
    x: number, y: number): void;
  canvasArc(target: number, x: number, y: number, radius: number, start: number, end: number,
    anticlockwise: boolean): CanvasErrorName | undefined;
  canvasArcTo(target: number, x1: number, y1: number, x2: number, y2: number,
    radius: number): CanvasErrorName | undefined;
  canvasEllipse(target: number, x: number, y: number, radiusX: number, radiusY: number,
    rotation: number, start: number, end: number,
    anticlockwise: boolean): CanvasErrorName | undefined;
  canvasRect(target: number, x: number, y: number, w: number, h: number): void;
  /** `radii` are `x, y` pairs, one to four of them. */
  canvasRoundRect(target: number, x: number, y: number, w: number, h: number,
    ...radii: number[]): CanvasErrorName | undefined;
  /** `path` 0 is the current default path. */
  canvasFill(target: number, path: number, fillRule: string): void;
  canvasStroke(target: number, path: number): void;
  canvasClip(target: number, path: number, fillRule: string): void;
  canvasIsPointInPath(target: number, path: number, x: number, y: number,
    fillRule: string): boolean;
  canvasIsPointInStroke(target: number, path: number, x: number, y: number): boolean;
  canvasSetLineDash(target: number, ...segments: number[]): void;
  /** The dash list as a `Float64Array`'s buffer. */
  canvasGetLineDash(target: number): ArrayBuffer;
  /**
   * Parses `css` as the CSS `font` shorthand and sets it, answering its
   * serialization (size in px, no line height); `undefined`, changing
   * nothing, for a string that is not one.
   */
  canvasSetFont(target: number, css: string): string | undefined;
  canvasFillText(target: number, text: string, x: number, y: number, maxWidth?: number): void;
  canvasStrokeText(target: number, text: string, x: number, y: number, maxWidth?: number): void;
  /**
   * The `TextMetrics` of `text` as a `Float64Array`'s buffer, in the IDL's
   * attribute order: `width`, `actualBoundingBoxLeft`, `…Right`,
   * `fontBoundingBoxAscent`, `…Descent`, `actualBoundingBoxAscent`,
   * `…Descent`, `emHeightAscent`, `emHeightDescent`, `hangingBaseline`,
   * `alphabeticBaseline`, `ideographicBaseline`.
   */
  canvasMeasureText(target: number, text: string): ArrayBuffer;
  /** `source` is a canvas or an image bitmap; `rects` are 2, 4 or 8 numbers. */
  canvasDrawImage(target: number, source: number, ...rects: number[]): CanvasErrorName | undefined;
  /** Unpremultiplied RGBA8 of the rectangle, `|sw|` × `|sh|` pixels. */
  canvasGetImageData(target: number, sx: number, sy: number, sw: number,
    sh: number): ArrayBuffer | CanvasErrorName;
  /** `dirty` is none or `dirtyX, dirtyY, dirtyWidth, dirtyHeight`. */
  canvasPutImageData(target: number, buffer: ArrayBuffer, offset: number, length: number,
    width: number, height: number, dx: number, dy: number,
    ...dirty: number[]): CanvasErrorName | undefined;
  /** The bitmap as a new image bitmap's id, the canvas left transparent black. */
  canvasTransferToBitmap(target: number): number;
  /** A new image bitmap of a canvas's or bitmap's pixels, cropped by `sx, sy, sw, sh` if given. */
  canvasBitmapFrom(source: number, ...crop: number[]): number | CanvasErrorName;
  /** A new image bitmap of unpremultiplied RGBA8 pixels, cropped if given. */
  canvasBitmapFromPixels(buffer: ArrayBuffer, offset: number, length: number, width: number,
    height: number, ...crop: number[]): number | CanvasErrorName;
  canvasBitmapWidth(bitmap: number): number;
  canvasBitmapHeight(bitmap: number): number;
  /**
   * HTML's `transferControlToOffscreen()` on the element `nodeId`: a new
   * canvas entry bound to the element as its placeholder, answered as
   * `[id, width, height]` in a `Float64Array`'s bytes, or
   * `"InvalidStateError"` for an element that is not a canvas in context
   * mode none. Throws for an id that is not a live element. The MTS realm's
   * and its `ShadowRealm` children's only.
   */
  canvasTransferControl(nodeId: number): ArrayBuffer | CanvasErrorName;
}

/** What `bobcat-internal:host` exports of the canvas members. */
declare module "bobcat-internal:host" {
  export const canvasCreate: BobcatCanvasNative["canvasCreate"];
  export const canvasDrop: BobcatCanvasNative["canvasDrop"];
  export const canvasSetSize: BobcatCanvasNative["canvasSetSize"];
  export const canvasDetach: BobcatCanvasNative["canvasDetach"];
  export const canvasAttach: BobcatCanvasNative["canvasAttach"];
  export const canvasPath: BobcatCanvasNative["canvasPath"];
  export const canvasPathFromSvg: BobcatCanvasNative["canvasPathFromSvg"];
  export const canvasAddPath: BobcatCanvasNative["canvasAddPath"];
  export const canvasSave: BobcatCanvasNative["canvasSave"];
  export const canvasRestore: BobcatCanvasNative["canvasRestore"];
  export const canvasReset: BobcatCanvasNative["canvasReset"];
  export const canvasScale: BobcatCanvasNative["canvasScale"];
  export const canvasRotate: BobcatCanvasNative["canvasRotate"];
  export const canvasTranslate: BobcatCanvasNative["canvasTranslate"];
  export const canvasTransform: BobcatCanvasNative["canvasTransform"];
  export const canvasSetTransform: BobcatCanvasNative["canvasSetTransform"];
  export const canvasGetTransform: BobcatCanvasNative["canvasGetTransform"];
  export const canvasResetTransform: BobcatCanvasNative["canvasResetTransform"];
  export const canvasGetNumber: BobcatCanvasNative["canvasGetNumber"];
  export const canvasSetNumber: BobcatCanvasNative["canvasSetNumber"];
  export const canvasGetKeyword: BobcatCanvasNative["canvasGetKeyword"];
  export const canvasSetKeyword: BobcatCanvasNative["canvasSetKeyword"];
  export const canvasGetSmoothing: BobcatCanvasNative["canvasGetSmoothing"];
  export const canvasSetSmoothing: BobcatCanvasNative["canvasSetSmoothing"];
  export const canvasSetColor: BobcatCanvasNative["canvasSetColor"];
  export const canvasSetPaint: BobcatCanvasNative["canvasSetPaint"];
  export const canvasLinearGradient: BobcatCanvasNative["canvasLinearGradient"];
  export const canvasRadialGradient: BobcatCanvasNative["canvasRadialGradient"];
  export const canvasConicGradient: BobcatCanvasNative["canvasConicGradient"];
  export const canvasAddColorStop: BobcatCanvasNative["canvasAddColorStop"];
  export const canvasCreatePattern: BobcatCanvasNative["canvasCreatePattern"];
  export const canvasPatternSetTransform: BobcatCanvasNative["canvasPatternSetTransform"];
  export const canvasClearRect: BobcatCanvasNative["canvasClearRect"];
  export const canvasFillRect: BobcatCanvasNative["canvasFillRect"];
  export const canvasStrokeRect: BobcatCanvasNative["canvasStrokeRect"];
  export const canvasBeginPath: BobcatCanvasNative["canvasBeginPath"];
  export const canvasClosePath: BobcatCanvasNative["canvasClosePath"];
  export const canvasMoveTo: BobcatCanvasNative["canvasMoveTo"];
  export const canvasLineTo: BobcatCanvasNative["canvasLineTo"];
  export const canvasQuadraticCurveTo: BobcatCanvasNative["canvasQuadraticCurveTo"];
  export const canvasBezierCurveTo: BobcatCanvasNative["canvasBezierCurveTo"];
  export const canvasArc: BobcatCanvasNative["canvasArc"];
  export const canvasArcTo: BobcatCanvasNative["canvasArcTo"];
  export const canvasEllipse: BobcatCanvasNative["canvasEllipse"];
  export const canvasRect: BobcatCanvasNative["canvasRect"];
  export const canvasRoundRect: BobcatCanvasNative["canvasRoundRect"];
  export const canvasFill: BobcatCanvasNative["canvasFill"];
  export const canvasStroke: BobcatCanvasNative["canvasStroke"];
  export const canvasClip: BobcatCanvasNative["canvasClip"];
  export const canvasIsPointInPath: BobcatCanvasNative["canvasIsPointInPath"];
  export const canvasIsPointInStroke: BobcatCanvasNative["canvasIsPointInStroke"];
  export const canvasSetLineDash: BobcatCanvasNative["canvasSetLineDash"];
  export const canvasGetLineDash: BobcatCanvasNative["canvasGetLineDash"];
  export const canvasSetFont: BobcatCanvasNative["canvasSetFont"];
  export const canvasFillText: BobcatCanvasNative["canvasFillText"];
  export const canvasStrokeText: BobcatCanvasNative["canvasStrokeText"];
  export const canvasMeasureText: BobcatCanvasNative["canvasMeasureText"];
  export const canvasDrawImage: BobcatCanvasNative["canvasDrawImage"];
  export const canvasGetImageData: BobcatCanvasNative["canvasGetImageData"];
  export const canvasPutImageData: BobcatCanvasNative["canvasPutImageData"];
  export const canvasTransferToBitmap: BobcatCanvasNative["canvasTransferToBitmap"];
  export const canvasBitmapFrom: BobcatCanvasNative["canvasBitmapFrom"];
  export const canvasBitmapFromPixels: BobcatCanvasNative["canvasBitmapFromPixels"];
  export const canvasBitmapWidth: BobcatCanvasNative["canvasBitmapWidth"];
  export const canvasBitmapHeight: BobcatCanvasNative["canvasBitmapHeight"];
  /** The MTS realm's and its `ShadowRealm` children's only. */
  export const canvasTransferControl: BobcatCanvasNative["canvasTransferControl"];
}
