// Behavior tests for `bobcat:canvas` and `bobcat:dom-exception` over a
// recording host mock.
//
// These pin what lives in canvas.ts: Web IDL's argument conversions, the
// exception a host member's answer is thrown as, the attributes the context
// keeps itself with their state stack, the overloads, and the globals the
// module installs. The drawing is the host's and runs in the crate's own
// tests, in every realm kind (`crates/bobcat-core/src/canvas/tests.rs`).

import { beforeEach, describe, expect, it, rstest } from "@rstest/core";

import * as eventTarget from "../src/event-target.ts";
import * as domException from "../src/dom-exception.ts";
rstest.mockRequire("bobcat:event-target", () => eventTarget);
import * as transfer from "../src/transfer.ts";
rstest.mockRequire("bobcat:transfer", () => transfer);
rstest.mockRequire("bobcat:dom-exception", () => domException);

/** Every host call, as `[member, ...arguments]`. */
const calls: unknown[][] = [];
/** What a member answers, by name; `undefined` otherwise. */
const answers = new Map<string, (...args: unknown[]) => unknown>();
let nextId = 1;

rstest.mockRequire("bobcat-internal:host", () =>
  new Proxy({}, {
    get: (_target, name: string) => (...args: unknown[]) => {
      // The transfer members, which the module calls as it is evaluated,
      // ahead of this file's bindings; `transfer.test.ts` covers transfer.
      if (name === "markTransferable" || name === "setTransferReviver") return undefined;
      calls.push([name, ...args]);
      const answer = answers.get(name);
      if (answer !== undefined) return answer(...args);
      if (/^canvas(Create|Path|PathFromSvg|TransferToBitmap|LinearGradient|RadialGradient|ConicGradient|CreatePattern|BitmapFrom|BitmapFromPixels)$/.test(name)) {
        return nextId++;
      }
      if (name === "canvasBitmapWidth" || name === "canvasBitmapHeight") return 4;
      if (name === "canvasGetTransform") return new Float64Array([1, 0, 0, 1, 0, 0]).buffer;
      return undefined;
    },
  }));

import {
  CanvasGradient,
  DOMException,
  ImageData,
  OffscreenCanvas,
  Path2D,
  TextMetrics,
  createImageBitmap,
} from "../src/canvas.ts";

function callsOf(member: string): unknown[][] {
  return calls.filter(([name]) => name === member).map(([, ...args]) => args);
}

function context() {
  const canvas = new OffscreenCanvas(10, 10);
  const ctx = canvas.getContext("2d");
  if (ctx === null) throw new Error("no context");
  return {canvas, ctx};
}

beforeEach(() => {
  calls.length = 0;
  answers.clear();
});

describe("DOMException", () => {
  it("has a name, a message and the legacy code", () => {
    const error = new DOMException("gone", "InvalidStateError");
    expect([error.name, error.message, error.code]).toEqual(["InvalidStateError", "gone", 11]);
    expect(error).toBeInstanceOf(Error);
    expect(new DOMException().name).toBe("Error");
    expect(new DOMException("x", "Unknown").code).toBe(0);
    expect((DOMException as unknown as Record<string, number>)["DATA_CLONE_ERR"]).toBe(25);
  });
});

describe("the globals", () => {
  it("are installed as interface objects are", () => {
    for (const name of ["OffscreenCanvas", "ImageData", "ImageBitmap", "Path2D", "DOMException",
      "CanvasGradient", "CanvasPattern", "TextMetrics", "OffscreenCanvasRenderingContext2D"]) {
      const descriptor = Object.getOwnPropertyDescriptor(globalThis, name);
      expect(descriptor?.enumerable).toBe(false);
      expect(descriptor?.writable).toBe(true);
    }
    expect((globalThis as Record<string, unknown>)["createImageBitmap"]).toBe(createImageBitmap);
  });
});

describe("OffscreenCanvas", () => {
  it("converts its size with [EnforceRange] and resets the context on a resize", () => {
    expect(() => new OffscreenCanvas(-1, 1)).toThrow(TypeError);
    expect(() => new OffscreenCanvas(Infinity, 1)).toThrow(TypeError);
    expect(() => new (OffscreenCanvas as unknown as new (w: number) => object)(1)).toThrow(TypeError);
    const {canvas, ctx} = context();
    expect(callsOf("canvasCreate").at(-1)).toEqual([10, 10]);
    answers.set("canvasSetColor", () => "#ff0000");
    ctx.fillStyle = "red";
    canvas.width = 7.9;
    expect(canvas.width).toBe(7);
    expect(callsOf("canvasSetSize").at(-1)?.slice(1)).toEqual([7, 10]);
    expect(ctx.fillStyle).toBe("#000000");
  });

  it("answers one context for 2d, null for another id and a TypeError for no id", () => {
    const {canvas, ctx} = context();
    expect(canvas.getContext("2d")).toBe(ctx);
    expect(canvas.getContext("webgl")).toBeNull();
    expect(() => canvas.getContext("3d")).toThrow(TypeError);
  });

  it("refuses to transfer a bitmap without a context", () => {
    expect(() => new OffscreenCanvas(1, 1).transferToImageBitmap()).toThrow(
      expect.objectContaining({name: "InvalidStateError"}),
    );
  });
});

describe("the 2D context", () => {
  it("keeps the styles and the font with the state stack", () => {
    const {ctx} = context();
    answers.set("canvasSetColor", (_id, _which, css) => (css === "nope" ? undefined : `#${css}`));
    answers.set("canvasSetFont", (_id, css) => `${css}!`);
    ctx.fillStyle = "aaaaaa";
    ctx.save();
    ctx.fillStyle = "bbbbbb";
    ctx.fillStyle = "nope";
    ctx.font = "20px serif";
    expect([ctx.fillStyle, ctx.font]).toEqual(["#bbbbbb", "20px serif!"]);
    ctx.restore();
    expect([ctx.fillStyle, ctx.font]).toEqual(["#aaaaaa", "10px sans-serif"]);
    const gradient = ctx.createLinearGradient(0, 0, 1, 1);
    ctx.strokeStyle = gradient;
    expect(ctx.strokeStyle).toBe(gradient);
    expect(callsOf("canvasSetPaint").at(-1)?.slice(1, 2)).toEqual([1]);
    ctx.reset();
    expect(ctx.strokeStyle).toBe("#000000");
  });

  it("throws the exception a member answers by name", () => {
    const {ctx} = context();
    answers.set("canvasArc", () => "IndexSizeError");
    expect(() => ctx.arc(0, 0, -1, 0, 1)).toThrow(expect.objectContaining({name: "IndexSizeError"}));
    answers.set("canvasRoundRect", () => "RangeError");
    expect(() => ctx.roundRect(0, 0, 1, 1, [1])).toThrow(RangeError);
    answers.set("canvasLinearGradient", () => "TypeError");
    expect(() => ctx.createLinearGradient(0, 0, NaN, 0)).toThrow(TypeError);
  });

  it("converts the pixel arguments as longs", () => {
    const {ctx} = context();
    answers.set("canvasGetImageData", () => new ArrayBuffer(4));
    ctx.getImageData(2 ** 32 + 1, -1.9, 1, "1");
    expect(callsOf("canvasGetImageData").at(-1)?.slice(1)).toEqual([1, -1, 1, 1]);
  });

  it("fixes up a DOMMatrix2DInit and refuses one whose members disagree", () => {
    const {ctx} = context();
    ctx.setTransform({m11: 2, b: 3, f: 4});
    expect(callsOf("canvasSetTransform").at(-1)?.slice(1)).toEqual([2, 3, 0, 1, 0, 4]);
    expect(() => ctx.setTransform({a: 1, m11: 2})).toThrow(TypeError);
    expect(() => ctx.setTransform(1, 2, 3)).toThrow(TypeError);
    ctx.setTransform();
    expect(callsOf("canvasSetTransform").at(-1)?.slice(1)).toEqual([1, 0, 0, 1, 0, 0]);
    expect(ctx.getTransform()).toMatchObject({a: 1, m22: 1, is2D: true, isIdentity: true});
  });

  it("resolves the overloads", () => {
    const {ctx} = context();
    const path = new Path2D();
    ctx.fill(path, "evenodd");
    ctx.fill();
    const fills = callsOf("canvasFill");
    expect(fills.map((args) => args[2])).toEqual(["evenodd", "nonzero"]);
    expect(fills[0]![1]).toBeGreaterThan(0);
    expect(fills[1]![1]).toBe(0);
    expect(() => ctx.fill("sideways")).toThrow(TypeError);
    expect(() => ctx.stroke({} as Path2D)).toThrow(TypeError);
    expect(() => (ctx.drawImage as (...args: unknown[]) => void)(new OffscreenCanvas(1, 1), 0)).toThrow(TypeError);
    expect(() => ctx.drawImage(new OffscreenCanvas(0, 1), 0, 0)).toThrow(
      expect.objectContaining({name: "InvalidStateError"}),
    );
    ctx.roundRect(0, 0, 5, 5, [1, {x: 2, y: 3}]);
    expect(callsOf("canvasRoundRect").at(-1)?.slice(5)).toEqual([1, 1, 2, 3]);
  });

  it("measures text into TextMetrics", () => {
    const {ctx} = context();
    answers.set("canvasMeasureText", () => new Float64Array([5, 0, 5, 8, 2, 7, 1, 8, 2, 6, 0, -2]).buffer);
    const metrics = ctx.measureText("a");
    expect(metrics).toBeInstanceOf(TextMetrics);
    expect([metrics.width, metrics.ideographicBaseline]).toEqual([5, -2]);
    expect(() => new (TextMetrics as unknown as new () => object)()).toThrow(TypeError);
    expect(() => new (CanvasGradient as unknown as new () => object)()).toThrow(TypeError);
  });
});

describe("ImageData", () => {
  it("raises the standard's constructor errors", () => {
    expect(new ImageData(2, 1).data.length).toBe(8);
    const name = (call: () => unknown) => {
      try { call(); return "none"; } catch (error) { return (error as Error).name; }
    };
    expect(name(() => new ImageData(0, 1))).toBe("IndexSizeError");
    expect(name(() => new ImageData(new Uint8ClampedArray(6), 1))).toBe("InvalidStateError");
    expect(name(() => new ImageData(new Uint8ClampedArray(12), 2))).toBe("IndexSizeError");
    expect(name(() => new ImageData(new Uint8ClampedArray(8), 1, 3))).toBe("IndexSizeError");
    expect(name(() => new ImageData(1, 1, {colorSpace: "display-p3"}))).toBe("NotSupportedError");
    expect(name(() => new ImageData(1, 1, {colorSpace: "cmyk"}))).toBe("TypeError");
  });
});

describe("createImageBitmap", () => {
  it("rejects what it does not implement and the wrong argument count", async () => {
    const data = new ImageData(1, 1);
    await expect(createImageBitmap(data, {imageOrientation: "flipY"})).rejects.toMatchObject({name: "NotSupportedError"});
    await expect(createImageBitmap(data, {resizeWidth: 0})).rejects.toMatchObject({name: "InvalidStateError"});
    await expect(createImageBitmap(data, {premultiplyAlpha: "nope"})).rejects.toBeInstanceOf(TypeError);
    await expect(createImageBitmap(data, 0, 0)).rejects.toBeInstanceOf(TypeError);
    await expect(createImageBitmap(data, 0, 0, 0, 1)).rejects.toBeInstanceOf(RangeError);
    const bitmap = await createImageBitmap(data, 0, 0, 1, 1, {premultiplyAlpha: "premultiply"});
    expect(bitmap.width).toBe(4);
    bitmap.close();
    expect(bitmap.width).toBe(0);
    await expect(createImageBitmap(bitmap)).rejects.toMatchObject({name: "InvalidStateError"});
  });
});
