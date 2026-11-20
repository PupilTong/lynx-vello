// The `bobcat:canvas` ESM: the HTML Standard's `OffscreenCanvas`, its 2D
// context, `Path2D`, `CanvasGradient`, `CanvasPattern`, `TextMetrics`,
// `ImageData`, `ImageBitmap` and `createImageBitmap`, over the canvas
// members of `bobcat-internal:host` (`crates/bobcat-core/src/canvas`).
//
// Like `bobcat:timers`, it is imported for its effect: evaluating it puts
// those names, and `bobcat:dom-exception`'s `DOMException`, on the realm's
// global object, as WebIDL puts interface objects there (writable,
// configurable, not enumerable). `bobcat:runtime` imports it in the MTS
// realm, `bobcat:bts` in the BTS realm, and a plain worker script imports it
// itself; it also exports every name.
//
// An `OffscreenCanvas` is made by its constructor, or by
// `__BobcatTransferredCanvas` for the entry a `<canvas>` element's
// `transferControlToOffscreen()` filed (`bobcat:element`'s
// `invokeUIMethod`); the host commits the latter's bitmap to the element at
// the end of each JavaScript entry that changed it.
//
// `OffscreenCanvas` and `ImageBitmap` are transferable: each is marked so at
// construction (`markTransferable`), `bobcat:transfer` detaches the ones a
// `postMessage` listed, and the realm's transfer reviver, set below, makes
// the object each transferred entry arrives as (`canvasAdopt`).
//
// Every object that names host state holds an id of the realm's table; a
// `FinalizationRegistry` drops the entry once the object is collected, and
// `ImageBitmap.close()` drops it at once. WebIDL's argument conversions are
// done here, so the host members take numbers, strings, booleans and an
// `ArrayBuffer` only. A host member that fails as the standard says answers
// the exception's name, which `check` throws.
//
// The context keeps four attributes itself, with the state stack they ride:
// `fillStyle`, `strokeStyle` and `shadowColor` (the object or the serialized
// color) and `font` (the serialization the host answered).

import { EventTarget } from "bobcat:event-target";
import { DOMException } from "bobcat:dom-exception";
import { registerTransferables } from "bobcat:transfer";
import {
  canvasAdopt,
  canvasAddColorStop,
  canvasAddPath,
  canvasArc,
  canvasArcTo,
  canvasBeginPath,
  canvasBezierCurveTo,
  canvasBitmapFrom,
  canvasBitmapFromPixels,
  canvasBitmapHeight,
  canvasBitmapWidth,
  canvasClearRect,
  canvasClip,
  canvasClosePath,
  canvasConicGradient,
  canvasCreate,
  canvasCreatePattern,
  canvasDrawImage,
  canvasDrop,
  canvasEllipse,
  canvasFill,
  canvasFillRect,
  canvasFillText,
  canvasGetImageData,
  canvasGetKeyword,
  canvasGetLineDash,
  canvasGetNumber,
  canvasGetSmoothing,
  canvasGetTransform,
  canvasIsPointInPath,
  canvasIsPointInStroke,
  canvasLinearGradient,
  canvasLineTo,
  canvasMeasureText,
  canvasMoveTo,
  canvasPath,
  canvasPathFromSvg,
  canvasPatternSetTransform,
  canvasPutImageData,
  canvasQuadraticCurveTo,
  canvasRadialGradient,
  canvasRect,
  canvasReset,
  canvasResetTransform,
  canvasRestore,
  canvasRotate,
  canvasRoundRect,
  canvasSave,
  canvasScale,
  canvasSetColor,
  canvasSetFont,
  canvasSetKeyword,
  canvasSetLineDash,
  canvasSetNumber,
  canvasSetPaint,
  canvasSetSize,
  canvasSetSmoothing,
  canvasSetTransform,
  canvasStroke,
  canvasStrokeRect,
  canvasStrokeText,
  canvasTransferToBitmap,
  canvasTransform,
  canvasTranslate,
  markTransferable,
  setTransferReviver,
} from "bobcat-internal:host";

export { DOMException };

// `canvasGetNumber`/`canvasSetNumber`'s selectors.
const GLOBAL_ALPHA = 0;
const SHADOW_BLUR = 1;
const SHADOW_OFFSET_X = 2;
const SHADOW_OFFSET_Y = 3;
const LINE_WIDTH = 4;
const MITER_LIMIT = 5;
const LINE_DASH_OFFSET = 6;
// `canvasGetKeyword`/`canvasSetKeyword`'s selectors.
const COMPOSITE = 0;
const SMOOTHING_QUALITY = 1;
const LINE_CAP = 2;
const LINE_JOIN = 3;
const TEXT_ALIGN = 4;
const TEXT_BASELINE = 5;
const DIRECTION = 6;
// `canvasSetColor`/`canvasSetPaint`'s selectors.
const FILL = 0;
const STROKE = 1;
const SHADOW = 2;

/** What the non-constructible interfaces are constructed with, here only. */
const INTERNAL = Symbol("bobcat:canvas");

/** Drops a collected object's entry. */
const entries = new FinalizationRegistry<number>((id) => {
  canvasDrop(id);
});

// WebIDL conversions.

function illegalConstructor(): TypeError {
  return new TypeError("Illegal constructor");
}

function requireArguments(count: number, needed: number, what: string): void {
  if (count < needed) {
    throw new TypeError(`${what}: ${needed} argument(s) required, but only ${count} present`);
  }
}

/** `unrestricted double`: ToNumber, which throws for a Symbol or BigInt. */
function toDouble(value: unknown): number {
  return +(value as number);
}

/** `double`: a non-finite value is a `TypeError`. */
function toFiniteDouble(value: unknown, what: string): number {
  const number = toDouble(value);
  if (!Number.isFinite(number)) throw new TypeError(`${what}: the value is not finite`);
  return number;
}

/** `long`. */
function toLong(value: unknown): number {
  return toDouble(value) | 0;
}

/** `[EnforceRange]` over an unsigned integer type whose largest value is `max`. */
function toEnforceRange(value: unknown, max: number, what: string): number {
  const number = toDouble(value);
  if (!Number.isFinite(number)) throw new TypeError(`${what}: the value is not finite`);
  const integer = Math.trunc(number) + 0;
  if (integer < 0 || integer > max) throw new TypeError(`${what}: the value is out of range`);
  return integer;
}

const UNSIGNED_LONG_LONG_MAX = Number.MAX_SAFE_INTEGER;
const UNSIGNED_LONG_MAX = 0xffffffff;

/** `DOMString`: throws for a Symbol, as WebIDL does. */
function toDOMString(value: unknown): string {
  return `${value as string}`;
}

/** Throws the exception a host member answered by name. */
function check(answer: unknown): void {
  if (typeof answer === "string") throw exception(answer);
}

function exception(name: string): Error {
  switch (name) {
    case "TypeError":
      return new TypeError("The value is not valid for this canvas operation");
    case "RangeError":
      return new RangeError("The value is out of range for this canvas operation");
    default:
      return new DOMException(messageOf(name), name);
  }
}

function messageOf(name: string): string {
  switch (name) {
    case "IndexSizeError":
      return "The index or size is negative or out of range";
    case "InvalidStateError":
      return "The object is in a state that does not allow this operation";
    case "SyntaxError":
      return "The string did not match the expected pattern";
    default:
      return name;
  }
}

/** A host member's answer that is an id, or the exception it names. */
function created(answer: number | string): number {
  if (typeof answer === "string") throw exception(answer);
  return answer;
}

const FILL_RULES = new Set(["nonzero", "evenodd"]);

/** `CanvasFillRule`, an enumeration argument: another string is a `TypeError`. */
function toFillRule(value: unknown): string {
  if (value === undefined) return "nonzero";
  const rule = toDOMString(value);
  if (!FILL_RULES.has(rule)) throw new TypeError(`'${rule}' is not a valid CanvasFillRule`);
  return rule;
}

/** `[a, b, c, d, e, f]` of a `DOMMatrix2DInit`, validated and fixed up. */
function matrix2D(init: unknown): [number, number, number, number, number, number] {
  if (init === undefined || init === null) init = {};
  if (typeof init !== "object" && typeof init !== "function") {
    throw new TypeError("The transform is not a DOMMatrix2DInit");
  }
  const dictionary = init as Record<string, unknown>;
  const member = (name: string): number | undefined => {
    const value = dictionary[name];
    return value === undefined ? undefined : toDouble(value);
  };
  // Dictionary members are read in lexicographic order.
  const [a, b, c, d, e, f] = ["a", "b", "c", "d", "e", "f"].map(member);
  const [m11, m12, m21, m22, m41, m42] = ["m11", "m12", "m21", "m22", "m41", "m42"].map(member);
  const pick = (short: number | undefined, long: number | undefined, initial: number): number => {
    if (short !== undefined && long !== undefined && !sameValueZero(short, long)) {
      throw new TypeError("The transform's members disagree");
    }
    return long ?? short ?? initial;
  };
  return [pick(a, m11, 1), pick(b, m12, 0), pick(c, m21, 0), pick(d, m22, 1), pick(e, m41, 0),
    pick(f, m42, 0)];
}

function sameValueZero(x: number, y: number): boolean {
  return x === y || (x !== x && y !== y);
}

// The interfaces. Each class reads another's private state through the
// function its static block assigns.

let canvasState: (value: unknown) => {id: number; width: number; height: number} | undefined;
let canvasTransfer: (value: unknown) => {id: number; context: boolean; detach(): void} | undefined;
let bitmapDetach: (value: unknown) => (() => void) | undefined;
let bitmapState: (value: unknown) => {id: number; closed: boolean} | undefined;
let pathId: (value: unknown) => number | undefined;
let gradientId: (value: unknown) => number | undefined;
let patternId: (value: unknown) => number | undefined;
let imageDataState: (value: unknown) => {data: Uint8ClampedArray; width: number; height: number} | undefined;
let contextReset: (context: OffscreenCanvasRenderingContext2D) => void;
let contextId: (value: unknown) => number | undefined;

const CONTEXT_IDS = new Set(["2d", "bitmaprenderer", "webgl", "webgl2", "webgpu"]);

/** The entry the next `OffscreenCanvas` constructed takes instead of a new one. */
let adopted: number | undefined;

/** HTML's `OffscreenCanvas`. */
export class OffscreenCanvas extends EventTarget {
  #id: number;
  #width: number;
  #height: number;
  #context: OffscreenCanvasRenderingContext2D | null = null;
  /** HTML's context mode `detached`: transferred to another realm. */
  #detached = false;

  static {
    canvasState = (value) => {
      if (typeof value !== "object" || value === null || !(#id in value)) return undefined;
      const canvas = value as OffscreenCanvas;
      return {id: canvas.#id, width: canvas.#width, height: canvas.#height};
    };
    canvasTransfer = (value) => {
      if (typeof value !== "object" || value === null || !(#id in value)) return undefined;
      const canvas = value as OffscreenCanvas;
      return {
        id: canvas.#id,
        context: canvas.#context !== null,
        detach() {
          // HTML's transfer steps: the bitmap is unset, so the canvas reads
          // as 0 × 0, and its entry is the receiving realm's now.
          canvas.#detached = true;
          canvas.#width = 0;
          canvas.#height = 0;
          entries.unregister(canvas);
          markTransferable(canvas, false);
        },
      };
    };
  }

  constructor(width: unknown, height: unknown) {
    super();
    requireArguments(arguments.length, 2, "OffscreenCanvas");
    this.#width = toEnforceRange(width, UNSIGNED_LONG_LONG_MAX, "OffscreenCanvas");
    this.#height = toEnforceRange(height, UNSIGNED_LONG_LONG_MAX, "OffscreenCanvas");
    this.#id = adopted ?? canvasCreate(this.#width, this.#height);
    adopted = undefined;
    entries.register(this, this.#id, this);
    markTransferable(this, true);
  }

  get width(): number {
    return this.#width;
  }

  /** A detached canvas has no bitmap to resize: setting its size does nothing. */
  set width(value: unknown) {
    const width = toEnforceRange(value, UNSIGNED_LONG_LONG_MAX, "width");
    if (this.#detached) return;
    this.#width = width;
    this.#resized();
  }

  get height(): number {
    return this.#height;
  }

  set height(value: unknown) {
    const height = toEnforceRange(value, UNSIGNED_LONG_LONG_MAX, "height");
    if (this.#detached) return;
    this.#height = height;
    this.#resized();
  }

  #resized(): void {
    canvasSetSize(this.#id, this.#width, this.#height);
    if (this.#context !== null) contextReset(this.#context);
  }

  /**
   * The one 2D context, for `"2d"`; `null` for the other context ids, which
   * this engine does not implement. `options` is accepted and not read:
   * `willReadFrequently` and the rest change nothing here.
   */
  getContext(contextId: unknown, options?: unknown): OffscreenCanvasRenderingContext2D | null {
    void options;
    requireArguments(arguments.length, 1, "getContext");
    const id = toDOMString(contextId);
    if (!CONTEXT_IDS.has(id)) {
      throw new TypeError(`'${id}' is not a valid OffscreenRenderingContextId`);
    }
    if (this.#detached) throw detachedCanvas();
    if (id !== "2d") return null;
    this.#context ??= new OffscreenCanvasRenderingContext2D(INTERNAL, this, this.#id);
    return this.#context;
  }

  transferToImageBitmap(): ImageBitmap {
    if (this.#detached) throw detachedCanvas();
    if (this.#context === null) {
      throw new DOMException("The canvas has no rendering context", "InvalidStateError");
    }
    return newBitmap(canvasTransferToBitmap(this.#id));
  }
}

function detachedCanvas(): DOMException {
  return new DOMException("The OffscreenCanvas has been transferred", "InvalidStateError");
}

/**
 * The `OffscreenCanvas` over the entry `canvasTransferControl` answered, a
 * placeholder `<canvas>`'s: `[id, width, height]` in a `Float64Array`'s bytes.
 */
export function __BobcatTransferredCanvas(entry: ArrayBuffer): OffscreenCanvas {
  const [id, width, height] = new Float64Array(entry) as unknown as [number, number, number];
  adopted = id;
  return new OffscreenCanvas(width, height);
}

/** HTML's `ImageBitmap`. */
export class ImageBitmap {
  #id: number;
  #width: number;
  #height: number;
  #closed = false;

  static {
    bitmapState = (value) => {
      if (typeof value !== "object" || value === null || !(#id in value)) return undefined;
      const bitmap = value as ImageBitmap;
      return {id: bitmap.#id, closed: bitmap.#closed};
    };
    bitmapDetach = (value) => {
      if (typeof value !== "object" || value === null || !(#id in value)) return undefined;
      const bitmap = value as ImageBitmap;
      // HTML's transfer steps unset the bitmap data, as `close()` does; the
      // entry is the receiving realm's now, so it is not dropped here.
      return () => bitmap.#detach();
    };
  }

  constructor(token: unknown, id: number) {
    if (token !== INTERNAL) throw illegalConstructor();
    this.#id = id;
    this.#width = canvasBitmapWidth(id);
    this.#height = canvasBitmapHeight(id);
    entries.register(this, id, this);
    markTransferable(this, true);
  }

  get width(): number {
    return this.#closed ? 0 : this.#width;
  }

  get height(): number {
    return this.#closed ? 0 : this.#height;
  }

  /** Releases the pixels now; the bitmap reads as 0 × 0 afterwards. */
  close(): undefined {
    if (!this.#closed) {
      this.#detach();
      canvasDrop(this.#id);
    }
    return undefined;
  }

  /** `[[Detached]]`: 0 × 0, unusable, and neither cloneable nor transferable. */
  #detach(): void {
    this.#closed = true;
    entries.unregister(this);
    markTransferable(this, false);
  }
}

function newBitmap(id: number): ImageBitmap {
  return new ImageBitmap(INTERNAL, id);
}

/** HTML's `ImageData`. Only `"srgb"` is implemented. */
export class ImageData {
  #data: Uint8ClampedArray;
  #width: number;
  #height: number;

  static {
    imageDataState = (value) => {
      if (typeof value !== "object" || value === null || !(#data in value)) return undefined;
      const image = value as ImageData;
      return {data: image.#data, width: image.#width, height: image.#height};
    };
  }

  constructor(first: unknown, second: unknown, third?: unknown, fourth?: unknown) {
    requireArguments(arguments.length, 2, "ImageData");
    let settings: unknown;
    if (first instanceof Uint8ClampedArray) {
      const buffer = first.buffer as ArrayBuffer & {resizable?: boolean};
      if (buffer.resizable) throw new TypeError("ImageData: the array's buffer is resizable");
      const width = toEnforceRange(second, UNSIGNED_LONG_MAX, "ImageData");
      const length = first.length;
      if (length === 0 || length % 4 !== 0) {
        throw new DOMException("The data length is not a nonzero multiple of 4", "InvalidStateError");
      }
      const pixels = length / 4;
      if (width === 0 || pixels % width !== 0) {
        throw new DOMException("The data length is not a multiple of the width", "IndexSizeError");
      }
      const height = pixels / width;
      if (third !== undefined && toEnforceRange(third, UNSIGNED_LONG_MAX, "ImageData") !== height) {
        throw new DOMException("The height does not match the data length", "IndexSizeError");
      }
      this.#data = first;
      this.#width = width;
      this.#height = height;
      settings = fourth;
    } else {
      const width = toEnforceRange(first, UNSIGNED_LONG_MAX, "ImageData");
      const height = toEnforceRange(second, UNSIGNED_LONG_MAX, "ImageData");
      if (width === 0 || height === 0) {
        throw new DOMException("The width or height is zero", "IndexSizeError");
      }
      this.#data = new Uint8ClampedArray(width * height * 4);
      this.#width = width;
      this.#height = height;
      settings = third;
    }
    checkColorSpace(settings);
  }

  get width(): number {
    return this.#width;
  }

  get height(): number {
    return this.#height;
  }

  get data(): Uint8ClampedArray {
    return this.#data;
  }

  get colorSpace(): string {
    return "srgb";
  }
}

/** `ImageDataSettings.colorSpace`: `"srgb"` alone is implemented. */
function checkColorSpace(settings: unknown): void {
  if (settings === undefined || settings === null) return;
  if (typeof settings !== "object") throw new TypeError("The settings are not a dictionary");
  const space = (settings as {colorSpace?: unknown}).colorSpace;
  if (space === undefined) return;
  const name = toDOMString(space);
  if (name === "display-p3") {
    throw new DOMException("The display-p3 color space is not supported", "NotSupportedError");
  }
  if (name !== "srgb") throw new TypeError(`'${name}' is not a valid PredefinedColorSpace`);
}

/** HTML's `CanvasGradient`. */
export class CanvasGradient {
  #id: number;

  static {
    gradientId = (value) =>
      typeof value === "object" && value !== null && #id in value
        ? (value as CanvasGradient).#id
        : undefined;
  }

  constructor(token: unknown, id: number) {
    if (token !== INTERNAL) throw illegalConstructor();
    this.#id = id;
    entries.register(this, id);
  }

  addColorStop(offset: unknown, color: unknown): undefined {
    requireArguments(arguments.length, 2, "addColorStop");
    const at = toFiniteDouble(offset, "addColorStop");
    check(canvasAddColorStop(this.#id, at, toDOMString(color)));
    return undefined;
  }
}

/** HTML's `CanvasPattern`. */
export class CanvasPattern {
  #id: number;

  static {
    patternId = (value) =>
      typeof value === "object" && value !== null && #id in value
        ? (value as CanvasPattern).#id
        : undefined;
  }

  constructor(token: unknown, id: number) {
    if (token !== INTERNAL) throw illegalConstructor();
    this.#id = id;
    entries.register(this, id);
  }

  setTransform(transform?: unknown): undefined {
    const matrix = matrix2D(transform);
    // A matrix with a non-finite entry leaves the transform as it is.
    if (matrix.every(Number.isFinite)) check(canvasPatternSetTransform(this.#id, ...matrix));
    return undefined;
  }
}

/** HTML's `TextMetrics`. */
export class TextMetrics {
  #values: Float64Array;

  constructor(token: unknown, values: Float64Array) {
    if (token !== INTERNAL) throw illegalConstructor();
    this.#values = values;
  }

  get width(): number { return this.#values[0]!; }
  get actualBoundingBoxLeft(): number { return this.#values[1]!; }
  get actualBoundingBoxRight(): number { return this.#values[2]!; }
  get fontBoundingBoxAscent(): number { return this.#values[3]!; }
  get fontBoundingBoxDescent(): number { return this.#values[4]!; }
  get actualBoundingBoxAscent(): number { return this.#values[5]!; }
  get actualBoundingBoxDescent(): number { return this.#values[6]!; }
  get emHeightAscent(): number { return this.#values[7]!; }
  get emHeightDescent(): number { return this.#values[8]!; }
  get hangingBaseline(): number { return this.#values[9]!; }
  get alphabeticBaseline(): number { return this.#values[10]!; }
  get ideographicBaseline(): number { return this.#values[11]!; }
}

/** The members of HTML's `CanvasPath` mixin, which the 2D context and `Path2D` share. */
interface CanvasPathMethods {
  closePath(): undefined;
  moveTo(x: unknown, y: unknown): undefined;
  lineTo(x: unknown, y: unknown): undefined;
  quadraticCurveTo(cpx: unknown, cpy: unknown, x: unknown, y: unknown): undefined;
  bezierCurveTo(cp1x: unknown, cp1y: unknown, cp2x: unknown, cp2y: unknown, x: unknown,
    y: unknown): undefined;
  arcTo(x1: unknown, y1: unknown, x2: unknown, y2: unknown, radius: unknown): undefined;
  rect(x: unknown, y: unknown, w: unknown, h: unknown): undefined;
  roundRect(x: unknown, y: unknown, w: unknown, h: unknown, radii?: unknown): undefined;
  arc(x: unknown, y: unknown, radius: unknown, startAngle: unknown, endAngle: unknown,
    counterclockwise?: unknown): undefined;
  ellipse(x: unknown, y: unknown, radiusX: unknown, radiusY: unknown, rotation: unknown,
    startAngle: unknown, endAngle: unknown, counterclockwise?: unknown): undefined;
}

/** The id a `CanvasPath` member acts on: a context's canvas, or a `Path2D`. */
function pathTarget(value: unknown): number {
  const id = pathId(value) ?? contextId(value);
  if (id === undefined) throw new TypeError("Illegal invocation");
  return id;
}

/** `roundRect`'s radii as `x, y` pairs (HTML "roundRect" steps 1-5). */
function radiusPairs(radii: unknown): number[] {
  const one = (radius: unknown): [number, number] => {
    if (typeof radius === "object" && radius !== null) {
      const point = radius as {x?: unknown; y?: unknown};
      const x = point.x === undefined ? 0 : toDouble(point.x);
      const y = point.y === undefined ? 0 : toDouble(point.y);
      return [x, y];
    }
    const r = toDouble(radius);
    return [r, r];
  };
  const list = typeof radii === "object" && radii !== null && Symbol.iterator in radii
    ? Array.from(radii as Iterable<unknown>)
    : [radii];
  if (list.length < 1 || list.length > 4) {
    throw new RangeError(`roundRect takes 1 to 4 radii, not ${list.length}`);
  }
  return list.flatMap(one);
}

const pathMethods: CanvasPathMethods = {
  closePath() {
    canvasClosePath(pathTarget(this));
    return undefined;
  },
  moveTo(x, y) {
    requireArguments(arguments.length, 2, "moveTo");
    canvasMoveTo(pathTarget(this), toDouble(x), toDouble(y));
    return undefined;
  },
  lineTo(x, y) {
    requireArguments(arguments.length, 2, "lineTo");
    canvasLineTo(pathTarget(this), toDouble(x), toDouble(y));
    return undefined;
  },
  quadraticCurveTo(cpx, cpy, x, y) {
    requireArguments(arguments.length, 4, "quadraticCurveTo");
    canvasQuadraticCurveTo(pathTarget(this), toDouble(cpx), toDouble(cpy), toDouble(x), toDouble(y));
    return undefined;
  },
  bezierCurveTo(cp1x, cp1y, cp2x, cp2y, x, y) {
    requireArguments(arguments.length, 6, "bezierCurveTo");
    canvasBezierCurveTo(pathTarget(this), toDouble(cp1x), toDouble(cp1y), toDouble(cp2x),
      toDouble(cp2y), toDouble(x), toDouble(y));
    return undefined;
  },
  arcTo(x1, y1, x2, y2, radius) {
    requireArguments(arguments.length, 5, "arcTo");
    check(canvasArcTo(pathTarget(this), toDouble(x1), toDouble(y1), toDouble(x2), toDouble(y2),
      toDouble(radius)));
    return undefined;
  },
  rect(x, y, w, h) {
    requireArguments(arguments.length, 4, "rect");
    canvasRect(pathTarget(this), toDouble(x), toDouble(y), toDouble(w), toDouble(h));
    return undefined;
  },
  roundRect(x, y, w, h, radii = 0) {
    requireArguments(arguments.length, 4, "roundRect");
    const target = pathTarget(this);
    const [nx, ny, nw, nh] = [toDouble(x), toDouble(y), toDouble(w), toDouble(h)];
    check(canvasRoundRect(target, nx, ny, nw, nh, ...radiusPairs(radii)));
    return undefined;
  },
  arc(x, y, radius, startAngle, endAngle, counterclockwise = false) {
    requireArguments(arguments.length, 5, "arc");
    check(canvasArc(pathTarget(this), toDouble(x), toDouble(y), toDouble(radius),
      toDouble(startAngle), toDouble(endAngle), !!counterclockwise));
    return undefined;
  },
  ellipse(x, y, radiusX, radiusY, rotation, startAngle, endAngle, counterclockwise = false) {
    requireArguments(arguments.length, 7, "ellipse");
    check(canvasEllipse(pathTarget(this), toDouble(x), toDouble(y), toDouble(radiusX),
      toDouble(radiusY), toDouble(rotation), toDouble(startAngle), toDouble(endAngle),
      !!counterclockwise));
    return undefined;
  },
};

function installPathMethods(prototype: object): void {
  for (const [name, descriptor] of Object.entries(Object.getOwnPropertyDescriptors(pathMethods))) {
    Object.defineProperty(prototype, name, {...descriptor, enumerable: true});
  }
}

/** HTML's `Path2D`. */
export interface Path2D extends CanvasPathMethods {}
export class Path2D {
  #id: number;

  static {
    pathId = (value) =>
      typeof value === "object" && value !== null && #id in value
        ? (value as Path2D).#id
        : undefined;
  }

  constructor(path?: unknown) {
    const source = path === undefined ? undefined : pathId(path);
    if (path === undefined) this.#id = canvasPath(0);
    else if (source !== undefined) this.#id = canvasPath(source);
    else this.#id = canvasPathFromSvg(toDOMString(path));
    entries.register(this, this.#id);
  }

  addPath(path: unknown, transform?: unknown): undefined {
    requireArguments(arguments.length, 1, "addPath");
    const source = pathId(path);
    if (source === undefined) throw new TypeError("addPath takes a Path2D");
    canvasAddPath(this.#id, source, ...matrix2D(transform));
    return undefined;
  }
}
installPathMethods(Path2D.prototype);

/** The source id of a `CanvasImageSource` this engine draws, its usability checked. */
function imageSource(image: unknown): number {
  const canvas = canvasState(image);
  if (canvas !== undefined) {
    if (canvas.width === 0 || canvas.height === 0) {
      throw new DOMException("The canvas has no pixels", "InvalidStateError");
    }
    return canvas.id;
  }
  const bitmap = bitmapState(image);
  if (bitmap !== undefined) {
    if (bitmap.closed) throw new DOMException("The image bitmap is closed", "InvalidStateError");
    return bitmap.id;
  }
  throw new TypeError("The image is not an OffscreenCanvas or an ImageBitmap");
}

/** The attributes the context keeps itself, with the state stack. */
interface KeptState {
  fillStyle: string | CanvasGradient | CanvasPattern;
  strokeStyle: string | CanvasGradient | CanvasPattern;
  shadowColor: string;
  font: string;
}

function initialState(): KeptState {
  return {
    fillStyle: "#000000",
    strokeStyle: "#000000",
    shadowColor: "rgba(0, 0, 0, 0)",
    font: "10px sans-serif",
  };
}

/** The (x, y, w, h) a rectangle method takes, converted. */
function rectangle(what: string, count: number, x: unknown, y: unknown, w: unknown,
  h: unknown): [number, number, number, number] {
  requireArguments(count, 4, what);
  return [toDouble(x), toDouble(y), toDouble(w), toDouble(h)];
}

/** HTML's `OffscreenCanvasRenderingContext2D`: every member, over the canvas `id` names. */
export interface OffscreenCanvasRenderingContext2D extends CanvasPathMethods {}
export class OffscreenCanvasRenderingContext2D {
  #id: number;
  #canvas: OffscreenCanvas;
  #state: KeptState = initialState();
  #stack: KeptState[] = [];

  static {
    contextReset = (context) => {
      context.#state = initialState();
      context.#stack = [];
    };
    contextId = (value) =>
      typeof value === "object" && value !== null && #id in value
        ? (value as OffscreenCanvasRenderingContext2D).#id
        : undefined;
  }

  constructor(token: unknown, canvas: OffscreenCanvas, id: number) {
    if (token !== INTERNAL) throw illegalConstructor();
    this.#canvas = canvas;
    this.#id = id;
  }

  get canvas(): OffscreenCanvas {
    return this.#canvas;
  }

  // CanvasState.

  save(): undefined {
    canvasSave(this.#id);
    this.#stack.push({...this.#state});
    return undefined;
  }

  restore(): undefined {
    canvasRestore(this.#id);
    const state = this.#stack.pop();
    if (state !== undefined) this.#state = state;
    return undefined;
  }

  reset(): undefined {
    canvasReset(this.#id);
    contextReset(this);
    return undefined;
  }

  isContextLost(): boolean {
    return false;
  }

  // CanvasTransform.

  scale(x: unknown, y: unknown): undefined {
    requireArguments(arguments.length, 2, "scale");
    canvasScale(this.#id, toDouble(x), toDouble(y));
    return undefined;
  }

  rotate(angle: unknown): undefined {
    requireArguments(arguments.length, 1, "rotate");
    canvasRotate(this.#id, toDouble(angle));
    return undefined;
  }

  translate(x: unknown, y: unknown): undefined {
    requireArguments(arguments.length, 2, "translate");
    canvasTranslate(this.#id, toDouble(x), toDouble(y));
    return undefined;
  }

  transform(a: unknown, b: unknown, c: unknown, d: unknown, e: unknown, f: unknown): undefined {
    requireArguments(arguments.length, 6, "transform");
    canvasTransform(this.#id, toDouble(a), toDouble(b), toDouble(c), toDouble(d), toDouble(e),
      toDouble(f));
    return undefined;
  }

  /** Six numbers, or one `DOMMatrix2DInit`, or nothing (the identity). */
  setTransform(...values: unknown[]): undefined {
    const count = values.length;
    if (count <= 1) {
      canvasSetTransform(this.#id, ...matrix2D(values[0]));
      return undefined;
    }
    requireArguments(count, 6, "setTransform");
    const [a, b, c, d, e, f] = values.slice(0, 6).map(toDouble) as
      [number, number, number, number, number, number];
    canvasSetTransform(this.#id, a, b, c, d, e, f);
    return undefined;
  }

  /** The current transform as a read-only 2D matrix: `a`..`f`, `m11`.., `is2D`, `isIdentity`. */
  getTransform(): object {
    const [a, b, c, d, e, f] = new Float64Array(canvasGetTransform(this.#id)) as unknown as number[];
    return Object.freeze({
      a, b, c, d, e, f,
      m11: a, m12: b, m21: c, m22: d, m41: e, m42: f,
      is2D: true,
      isIdentity: a === 1 && b === 0 && c === 0 && d === 1 && e === 0 && f === 0,
    });
  }

  resetTransform(): undefined {
    canvasResetTransform(this.#id);
    return undefined;
  }

  // CanvasCompositing and CanvasImageSmoothing.

  get globalAlpha(): number { return canvasGetNumber(this.#id, GLOBAL_ALPHA); }
  set globalAlpha(value: unknown) { canvasSetNumber(this.#id, GLOBAL_ALPHA, toDouble(value)); }
  get globalCompositeOperation(): string { return canvasGetKeyword(this.#id, COMPOSITE); }
  set globalCompositeOperation(value: unknown) {
    canvasSetKeyword(this.#id, COMPOSITE, toDOMString(value));
  }
  get imageSmoothingEnabled(): boolean { return canvasGetSmoothing(this.#id); }
  set imageSmoothingEnabled(value: unknown) { canvasSetSmoothing(this.#id, !!value); }
  get imageSmoothingQuality(): string { return canvasGetKeyword(this.#id, SMOOTHING_QUALITY); }
  set imageSmoothingQuality(value: unknown) {
    canvasSetKeyword(this.#id, SMOOTHING_QUALITY, toDOMString(value));
  }

  // CanvasFillStrokeStyles.

  get fillStyle(): string | CanvasGradient | CanvasPattern { return this.#state.fillStyle; }
  set fillStyle(value: unknown) { this.#setStyle(FILL, "fillStyle", value); }
  get strokeStyle(): string | CanvasGradient | CanvasPattern { return this.#state.strokeStyle; }
  set strokeStyle(value: unknown) { this.#setStyle(STROKE, "strokeStyle", value); }

  #setStyle(which: number, key: "fillStyle" | "strokeStyle", value: unknown): void {
    const paint = gradientId(value) ?? patternId(value);
    if (paint !== undefined) {
      canvasSetPaint(this.#id, which, paint);
      this.#state[key] = value as CanvasGradient | CanvasPattern;
      return;
    }
    const color = canvasSetColor(this.#id, which, toDOMString(value));
    if (color !== undefined) this.#state[key] = color;
  }

  createLinearGradient(x0: unknown, y0: unknown, x1: unknown, y1: unknown): CanvasGradient {
    requireArguments(arguments.length, 4, "createLinearGradient");
    const id = created(canvasLinearGradient(toDouble(x0), toDouble(y0), toDouble(x1), toDouble(y1)));
    return new CanvasGradient(INTERNAL, id);
  }

  createRadialGradient(x0: unknown, y0: unknown, r0: unknown, x1: unknown, y1: unknown,
    r1: unknown): CanvasGradient {
    requireArguments(arguments.length, 6, "createRadialGradient");
    const id = created(canvasRadialGradient(toDouble(x0), toDouble(y0), toDouble(r0),
      toDouble(x1), toDouble(y1), toDouble(r1)));
    return new CanvasGradient(INTERNAL, id);
  }

  createConicGradient(startAngle: unknown, x: unknown, y: unknown): CanvasGradient {
    requireArguments(arguments.length, 3, "createConicGradient");
    const id = created(canvasConicGradient(toDouble(startAngle), toDouble(x), toDouble(y)));
    return new CanvasGradient(INTERNAL, id);
  }

  createPattern(image: unknown, repetition: unknown): CanvasPattern | null {
    requireArguments(arguments.length, 2, "createPattern");
    const source = imageSource(image);
    const name = repetition === null ? "" : toDOMString(repetition);
    return new CanvasPattern(INTERNAL, created(canvasCreatePattern(source, name)));
  }

  // CanvasShadowStyles.

  get shadowColor(): string { return this.#state.shadowColor; }
  set shadowColor(value: unknown) {
    const color = canvasSetColor(this.#id, SHADOW, toDOMString(value));
    if (color !== undefined) this.#state.shadowColor = color;
  }
  get shadowBlur(): number { return canvasGetNumber(this.#id, SHADOW_BLUR); }
  set shadowBlur(value: unknown) { canvasSetNumber(this.#id, SHADOW_BLUR, toDouble(value)); }
  get shadowOffsetX(): number { return canvasGetNumber(this.#id, SHADOW_OFFSET_X); }
  set shadowOffsetX(value: unknown) { canvasSetNumber(this.#id, SHADOW_OFFSET_X, toDouble(value)); }
  get shadowOffsetY(): number { return canvasGetNumber(this.#id, SHADOW_OFFSET_Y); }
  set shadowOffsetY(value: unknown) { canvasSetNumber(this.#id, SHADOW_OFFSET_Y, toDouble(value)); }

  // CanvasRect.

  clearRect(x: unknown, y: unknown, w: unknown, h: unknown): undefined {
    canvasClearRect(this.#id, ...rectangle("clearRect", arguments.length, x, y, w, h));
    return undefined;
  }

  fillRect(x: unknown, y: unknown, w: unknown, h: unknown): undefined {
    canvasFillRect(this.#id, ...rectangle("fillRect", arguments.length, x, y, w, h));
    return undefined;
  }

  strokeRect(x: unknown, y: unknown, w: unknown, h: unknown): undefined {
    canvasStrokeRect(this.#id, ...rectangle("strokeRect", arguments.length, x, y, w, h));
    return undefined;
  }

  // CanvasDrawPath.

  beginPath(): undefined {
    canvasBeginPath(this.#id);
    return undefined;
  }

  /** `fill(fillRule?)` or `fill(path, fillRule?)`. */
  fill(first?: unknown, second?: unknown): undefined {
    const path = pathId(first);
    if (path !== undefined) canvasFill(this.#id, path, toFillRule(second));
    else canvasFill(this.#id, 0, toFillRule(first));
    return undefined;
  }

  /** `stroke()` or `stroke(path)`. */
  stroke(path?: unknown): undefined {
    if (arguments.length === 0) {
      canvasStroke(this.#id, 0);
      return undefined;
    }
    const id = pathId(path);
    if (id === undefined) throw new TypeError("stroke takes a Path2D");
    canvasStroke(this.#id, id);
    return undefined;
  }

  /** `clip(fillRule?)` or `clip(path, fillRule?)`. */
  clip(first?: unknown, second?: unknown): undefined {
    const path = pathId(first);
    if (path !== undefined) canvasClip(this.#id, path, toFillRule(second));
    else canvasClip(this.#id, 0, toFillRule(first));
    return undefined;
  }

  /** `isPointInPath(x, y, fillRule?)` or `isPointInPath(path, x, y, fillRule?)`. */
  isPointInPath(...values: unknown[]): boolean {
    const path = pathId(values[0]);
    if (path !== undefined) {
      requireArguments(values.length, 3, "isPointInPath");
      return canvasIsPointInPath(this.#id, path, toDouble(values[1]), toDouble(values[2]),
        toFillRule(values[3]));
    }
    requireArguments(values.length, 2, "isPointInPath");
    return canvasIsPointInPath(this.#id, 0, toDouble(values[0]), toDouble(values[1]),
      toFillRule(values[2]));
  }

  /** `isPointInStroke(x, y)` or `isPointInStroke(path, x, y)`. */
  isPointInStroke(...values: unknown[]): boolean {
    const path = pathId(values[0]);
    if (path !== undefined) {
      requireArguments(values.length, 3, "isPointInStroke");
      return canvasIsPointInStroke(this.#id, path, toDouble(values[1]), toDouble(values[2]));
    }
    requireArguments(values.length, 2, "isPointInStroke");
    return canvasIsPointInStroke(this.#id, 0, toDouble(values[0]), toDouble(values[1]));
  }

  // CanvasPathDrawingStyles.

  get lineWidth(): number { return canvasGetNumber(this.#id, LINE_WIDTH); }
  set lineWidth(value: unknown) { canvasSetNumber(this.#id, LINE_WIDTH, toDouble(value)); }
  get lineCap(): string { return canvasGetKeyword(this.#id, LINE_CAP); }
  set lineCap(value: unknown) { canvasSetKeyword(this.#id, LINE_CAP, toDOMString(value)); }
  get lineJoin(): string { return canvasGetKeyword(this.#id, LINE_JOIN); }
  set lineJoin(value: unknown) { canvasSetKeyword(this.#id, LINE_JOIN, toDOMString(value)); }
  get miterLimit(): number { return canvasGetNumber(this.#id, MITER_LIMIT); }
  set miterLimit(value: unknown) { canvasSetNumber(this.#id, MITER_LIMIT, toDouble(value)); }
  get lineDashOffset(): number { return canvasGetNumber(this.#id, LINE_DASH_OFFSET); }
  set lineDashOffset(value: unknown) {
    canvasSetNumber(this.#id, LINE_DASH_OFFSET, toDouble(value));
  }

  setLineDash(segments: unknown): undefined {
    requireArguments(arguments.length, 1, "setLineDash");
    if (typeof segments !== "object" || segments === null || !(Symbol.iterator in segments)) {
      throw new TypeError("setLineDash takes a sequence of numbers");
    }
    canvasSetLineDash(this.#id, ...Array.from(segments as Iterable<unknown>, toDouble));
    return undefined;
  }

  getLineDash(): number[] {
    return Array.from(new Float64Array(canvasGetLineDash(this.#id)));
  }

  // CanvasTextDrawingStyles and CanvasText.

  get font(): string { return this.#state.font; }
  set font(value: unknown) {
    const font = canvasSetFont(this.#id, toDOMString(value));
    if (font !== undefined) this.#state.font = font;
  }
  get textAlign(): string { return canvasGetKeyword(this.#id, TEXT_ALIGN); }
  set textAlign(value: unknown) { canvasSetKeyword(this.#id, TEXT_ALIGN, toDOMString(value)); }
  get textBaseline(): string { return canvasGetKeyword(this.#id, TEXT_BASELINE); }
  set textBaseline(value: unknown) {
    canvasSetKeyword(this.#id, TEXT_BASELINE, toDOMString(value));
  }
  get direction(): string { return canvasGetKeyword(this.#id, DIRECTION); }
  set direction(value: unknown) { canvasSetKeyword(this.#id, DIRECTION, toDOMString(value)); }

  fillText(text: unknown, x: unknown, y: unknown, maxWidth?: unknown): undefined {
    requireArguments(arguments.length, 3, "fillText");
    canvasFillText(this.#id, toDOMString(text), toDouble(x), toDouble(y),
      maxWidth === undefined ? undefined : toDouble(maxWidth));
    return undefined;
  }

  strokeText(text: unknown, x: unknown, y: unknown, maxWidth?: unknown): undefined {
    requireArguments(arguments.length, 3, "strokeText");
    canvasStrokeText(this.#id, toDOMString(text), toDouble(x), toDouble(y),
      maxWidth === undefined ? undefined : toDouble(maxWidth));
    return undefined;
  }

  measureText(text: unknown): TextMetrics {
    requireArguments(arguments.length, 1, "measureText");
    return new TextMetrics(INTERNAL, new Float64Array(canvasMeasureText(this.#id, toDOMString(text))));
  }

  // CanvasDrawImage.

  /** `(image, dx, dy)`, `(image, dx, dy, dw, dh)` or `(image, sx, sy, sw, sh, dx, dy, dw, dh)`. */
  drawImage(image: unknown, ...values: unknown[]): undefined {
    const count = Math.min(arguments.length, 9);
    if (count !== 3 && count !== 5 && count !== 9) {
      throw new TypeError(`drawImage takes 3, 5 or 9 arguments, not ${arguments.length}`);
    }
    const source = imageSource(image);
    check(canvasDrawImage(this.#id, source, ...values.slice(0, count - 1).map(toDouble)));
    return undefined;
  }

  // CanvasImageData.

  /** `createImageData(sw, sh, settings?)` or `createImageData(imagedata)`. */
  createImageData(first: unknown, second?: unknown, settings?: unknown): ImageData {
    requireArguments(arguments.length, 1, "createImageData");
    if (arguments.length === 1) {
      const source = imageDataState(first);
      if (source === undefined) throw new TypeError("createImageData takes an ImageData");
      return new ImageData(source.width, source.height);
    }
    const [sw, sh] = [toLong(first), toLong(second)];
    if (sw === 0 || sh === 0) throw new DOMException("The width or height is zero", "IndexSizeError");
    checkColorSpace(settings);
    return new ImageData(Math.abs(sw), Math.abs(sh));
  }

  getImageData(sx: unknown, sy: unknown, sw: unknown, sh: unknown, settings?: unknown): ImageData {
    requireArguments(arguments.length, 4, "getImageData");
    const [x, y, w, h] = [toLong(sx), toLong(sy), toLong(sw), toLong(sh)];
    checkColorSpace(settings);
    const pixels = canvasGetImageData(this.#id, x, y, w, h);
    if (typeof pixels === "string") throw exception(pixels);
    return new ImageData(new Uint8ClampedArray(pixels), Math.abs(w), Math.abs(h));
  }

  /** `(imagedata, dx, dy)` or `(imagedata, dx, dy, dirtyX, dirtyY, dirtyWidth, dirtyHeight)`. */
  putImageData(imagedata: unknown, dx: unknown, dy: unknown, ...dirty: unknown[]): undefined {
    const count = arguments.length;
    if (count !== 3 && count < 7) {
      throw new TypeError(`putImageData takes 3 or 7 arguments, not ${count}`);
    }
    const image = imageDataState(imagedata);
    if (image === undefined) throw new TypeError("putImageData takes an ImageData");
    const [x, y] = [toLong(dx), toLong(dy)];
    const rectangle = count === 3 ? [] : dirty.slice(0, 4).map(toLong);
    const {data, width, height} = image;
    if (data.length !== width * height * 4) {
      throw new DOMException("The image data's buffer is detached", "InvalidStateError");
    }
    check(canvasPutImageData(this.#id, data.buffer as ArrayBuffer, data.byteOffset,
      data.byteLength, width, height, x, y, ...rectangle));
    return undefined;
  }
}
installPathMethods(OffscreenCanvasRenderingContext2D.prototype);

const ORIENTATIONS = new Set(["from-image", "flipY"]);
const PREMULTIPLY = new Set(["none", "premultiply", "default"]);
const CONVERSIONS = new Set(["none", "default"]);
const QUALITIES = new Set(["pixelated", "low", "medium", "high"]);

/**
 * `ImageBitmapOptions`, whose values this engine does not implement — a
 * resize, `flipY`, `premultiplyAlpha: "none"` — reject rather than being
 * ignored.
 */
function checkBitmapOptions(options: unknown): void {
  if (options === undefined || options === null) return;
  if (typeof options !== "object") throw new TypeError("The options are not a dictionary");
  const read = options as Record<string, unknown>;
  const keyword = (name: string, allowed: Set<string>): string | undefined => {
    const value = read[name];
    if (value === undefined) return undefined;
    const text = toDOMString(value);
    if (!allowed.has(text)) throw new TypeError(`'${text}' is not a valid ${name}`);
    return text;
  };
  const conversion = keyword("colorSpaceConversion", CONVERSIONS);
  const orientation = keyword("imageOrientation", ORIENTATIONS);
  const premultiply = keyword("premultiplyAlpha", PREMULTIPLY);
  const resizeHeight = read["resizeHeight"] === undefined
    ? undefined
    : toEnforceRange(read["resizeHeight"], UNSIGNED_LONG_MAX, "resizeHeight");
  keyword("resizeQuality", QUALITIES);
  const resizeWidth = read["resizeWidth"] === undefined
    ? undefined
    : toEnforceRange(read["resizeWidth"], UNSIGNED_LONG_MAX, "resizeWidth");
  void conversion;
  if (resizeWidth === 0 || resizeHeight === 0) {
    throw new DOMException("The resize width or height is zero", "InvalidStateError");
  }
  const unsupported = orientation === "flipY" ? "imageOrientation: \"flipY\""
    : premultiply === "none" ? "premultiplyAlpha: \"none\""
    : resizeWidth !== undefined || resizeHeight !== undefined ? "resizing"
    : undefined;
  if (unsupported !== undefined) {
    throw new DOMException(`createImageBitmap does not support ${unsupported}`, "NotSupportedError");
  }
}

/**
 * HTML's `createImageBitmap(image, options?)` and `createImageBitmap(image,
 * sx, sy, sw, sh, options?)`, for an `OffscreenCanvas`, an `ImageBitmap` or
 * an `ImageData`. The bitmap is made at once; every failure rejects.
 */
export async function createImageBitmap(image: unknown, ...values: unknown[]): Promise<ImageBitmap> {
  const count = values.length + 1;
  if (count < 1 || count === 3 || count === 4) {
    throw new TypeError(`createImageBitmap takes 1, 2, 5 or 6 arguments, not ${count}`);
  }
  let crop: number[] = [];
  let options: unknown = values[0];
  if (count >= 5) {
    crop = values.slice(0, 4).map(toLong);
    options = values[4];
    if (crop[2] === 0 || crop[3] === 0) throw new RangeError("The crop width or height is zero");
  }
  const data = imageDataState(image);
  if (data !== undefined) {
    checkBitmapOptions(options);
    const {data: pixels, width, height} = data;
    if (pixels.length !== width * height * 4) {
      throw new DOMException("The image data's buffer is detached", "InvalidStateError");
    }
    return newBitmap(created(canvasBitmapFromPixels(pixels.buffer as ArrayBuffer,
      pixels.byteOffset, pixels.byteLength, width, height, ...crop)));
  }
  const source = imageSource(image);
  checkBitmapOptions(options);
  return newBitmap(created(canvasBitmapFrom(source, ...crop)));
}

// The two transferable interfaces, for `bobcat:transfer`'s `postMessage`.
registerTransferables({
  id(value) {
    const canvas = canvasTransfer(value);
    if (canvas !== undefined) return canvas.context ? 0 : canvas.id;
    return bitmapState(value)?.id;
  },
  detach(value) {
    canvasTransfer(value)?.detach();
    bitmapDetach(value)?.();
  },
});

// A message delivered with a transfer list: the object each slot reads as,
// over the entry the message carried there.
setTransferReviver((slot) => {
  const entry = canvasAdopt(slot);
  return typeof entry === "number" ? newBitmap(entry) : __BobcatTransferredCanvas(entry);
});

for (const [name, value] of Object.entries({
  OffscreenCanvas,
  OffscreenCanvasRenderingContext2D,
  CanvasGradient,
  CanvasPattern,
  TextMetrics,
  ImageData,
  ImageBitmap,
  Path2D,
  DOMException,
})) {
  Object.defineProperty(globalThis, name, {configurable: true, enumerable: false, writable: true, value});
}
Object.defineProperty(globalThis, "createImageBitmap", {
  configurable: true,
  enumerable: true,
  writable: true,
  value: createImageBitmap,
});
