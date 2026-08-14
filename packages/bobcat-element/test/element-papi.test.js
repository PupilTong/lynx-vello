// @ts-check
// Behavior tests for the Element PAPI runtime over a recording native mock.
//
// These pin exactly the semantics that live in element-papi.js: the PAPI
// surface and arities, unique-id allocation, tag vocabulary, argument
// marshaling, handle identity and brand checks, and explicit drop
// bookkeeping. The mock mirrors the native contract the real `bobcat` object
// implements: the id sequence is validated in `createElement`, and
// `dropElement` rejects the page and unknown ids.
// Structural tree validation and the collection-driven drop path run against
// the real native side in crates/bobcat-core/tests/main_thread.rs, where a
// real QuickJS collector exists.

import { beforeEach, describe, expect, it, rstest } from "@rstest/core";

/**
 * @returns {BobcatNative & { calls: unknown[][], named: (name: string) => unknown[][] }}
 */
function createMockBobcat() {
  /** @type {unknown[][]} */
  const calls = [];
  /** @param {string} name */
  const named = (name) => calls.filter((call) => call[0] === name);
  /**
   * @param {string} name
   * @returns {(...args: unknown[]) => void}
   */
  const record = (name) => {
    return (...args) => {
      calls.push([name, ...args]);
    };
  };
  const live = new Set([1]);
  let expectedId = 2;
  return {
    calls,
    named,
    createPage: record("createPage"),
    /**
     * @param {string} tag
     * @param {number} uniqueId
     */
    createElement: (tag, uniqueId) => {
      if (uniqueId !== expectedId) {
        throw new Error(
          `a new element must take the next unique id ${expectedId}, got ${uniqueId}`,
        );
      }
      expectedId += 1;
      live.add(uniqueId);
      calls.push(["createElement", tag, uniqueId]);
    },
    setAttribute: record("setAttribute"),
    insertBefore: record("insertBefore"),
    removeElement: record("removeElement"),
    replaceElement: record("replaceElement"),
    /** @param {number} uniqueId */
    dropElement: (uniqueId) => {
      if (uniqueId === 1) {
        throw new Error("the page element cannot be removed");
      }
      if (!live.delete(uniqueId)) {
        throw new Error(`no element has the unique id ${uniqueId}`);
      }
      calls.push(["dropElement", uniqueId]);
    },
    flushElementTree: record("flushElementTree"),
  };
}

/** @type {ReturnType<typeof createMockBobcat>} */
let mock;

beforeEach(async () => {
  rstest.resetModules();
  mock = createMockBobcat();
  globalThis.bobcat = mock;
  await import("../src/element-papi.js");
});

describe("installation", () => {
  it("assigns every PAPI global with web-core's arity", () => {
    /** @type {[string, number][]} */
    const arities = [
      ["__CreatePage", 2],
      ["__CreateElement", 2],
      ["__CreateWrapperElement", 1],
      ["__CreateText", 1],
      ["__CreateImage", 1],
      ["__CreateView", 1],
      ["__CreateScrollView", 1],
      ["__CreateRawText", 1],
      ["__CreateList", 3],
      ["__AppendElement", 2],
      ["__InsertElementBefore", 3],
      ["__RemoveElement", 2],
      ["__ReplaceElement", 2],
      ["__DropElement", 1],
      ["__FlushElementTree", 0],
    ];
    for (const [name, arity] of arities) {
      const papi = /** @type {Record<string, unknown>} */ (globalThis)[name];
      expect(papi, name).toBeTypeOf("function");
      expect(/** @type {Function} */ (papi).length, name).toBe(arity);
    }
  });

  it("installs the drop deliverer on the native object", () => {
    expect(mock.deliverPendingElementDrops).toBeTypeOf("function");
  });

  it("fails loudly when the native object is missing", async () => {
    rstest.resetModules();
    // @ts-expect-error deliberately removing the native object
    globalThis.bobcat = undefined;
    await expect(import("../src/element-papi.js")).rejects.toThrow(
      "requires the native bobcat object",
    );
  });

  it("keeps working when the native object is tampered with afterwards", () => {
    const replaced = rstest.fn();
    mock.createElement = replaced;
    __CreateView(0);
    expect(replaced).not.toHaveBeenCalled();
    expect(mock.named("createElement")).toEqual([
      ["createElement", "view", 2],
    ]);
  });
});

describe("unique ids", () => {
  it("start at 2 and increment per creation", () => {
    __CreateView(0);
    __CreateText(0);
    __CreateRawText("hi");
    expect(mock.named("createElement")).toEqual([
      ["createElement", "view", 2],
      ["createElement", "text", 3],
      ["createElement", "raw-text", 4],
    ]);
  });

  it("are not consumed by a rejected creation", () => {
    expect(() => __CreateView(1.5)).toThrow(
      "__CreateView expects an unsigned 32-bit integer for argument 0, got 1.5",
    );
    __CreateView(0);
    expect(mock.named("createElement")).toEqual([
      ["createElement", "view", 2],
    ]);
  });

  it("are never reused after a drop", () => {
    const dropped = __CreateView(0);
    __DropElement(dropped);
    __CreateView(0);
    expect(mock.named("createElement")).toEqual([
      ["createElement", "view", 2],
      ["createElement", "view", 3],
    ]);
  });
});

describe("__CreatePage", () => {
  it("returns one permanent handle and marks the page each call", () => {
    const first = __CreatePage("card", 0);
    const second = __CreatePage("other", 7);
    expect(first).toBeTypeOf("object");
    expect(second).toBe(first);
    expect(mock.named("createPage")).toHaveLength(2);
  });

  it("coerces missing arguments like the native host did", () => {
    expect(() => __CreatePage()).not.toThrow();
    expect(() => __CreatePage(null, null)).not.toThrow();
  });

  it("rejects non-string component ids and non-integer css ids", () => {
    expect(() => __CreatePage(5, 0)).toThrow(
      "__CreatePage expects a string for argument 0",
    );
    expect(() => __CreatePage("card", 1.5)).toThrow(
      "__CreatePage expects an integer for argument 1",
    );
  });

});

describe("constructors", () => {
  it("use the Lynx tag vocabulary", () => {
    __CreateElement("custom-widget", 1);
    __CreateWrapperElement(0);
    __CreateText(0);
    __CreateImage(0);
    __CreateView(0);
    __CreateScrollView(0);
    __CreateList(1, () => {}, () => {});
    expect(mock.named("createElement").map((call) => call[1])).toEqual([
      "custom-widget",
      "wrapper",
      "text",
      "image",
      "view",
      "scroll-view",
      "list",
    ]);
  });

  it("return distinct opaque object handles", () => {
    const first = __CreateView(0);
    const second = __CreateView(0);
    expect(first).toBeTypeOf("object");
    expect(second).not.toBe(first);
  });

  it("validate the parent component id as a u32 and use it nowhere", () => {
    expect(() => __CreateView("x")).toThrow(
      "__CreateView expects a number for argument 0",
    );
    for (const bad of [1.5, -1, 4294967296, Number.NaN, Infinity]) {
      expect(() => __CreateView(bad)).toThrow(
        `__CreateView expects an unsigned 32-bit integer for argument 0, got ${bad}`,
      );
    }
    expect(mock.named("createElement")).toEqual([]);
    // Any in-range id is accepted without a liveness lookup, matching
    // web-core's silent fallback for a parent component that names nothing.
    expect(() => __CreateView(4294967295)).not.toThrow();
    expect(() => __CreateElement("custom-widget", 9)).not.toThrow();
  });

  it("coerce nullish __CreateElement tags to the empty string", () => {
    __CreateElement(null, 0);
    expect(mock.named("createElement")).toEqual([["createElement", "", 2]]);
    expect(() => __CreateElement(5, 0)).toThrow(
      "__CreateElement expects a string for argument 0",
    );
  });

  it("store raw text through setAttribute", () => {
    __CreateRawText("Hello, Lynx");
    __CreateRawText();
    expect(mock.named("setAttribute")).toEqual([
      ["setAttribute", 2, "text", "Hello, Lynx"],
      ["setAttribute", 3, "text", ""],
    ]);
  });
});

describe("tree mutations", () => {
  it("forward unique ids and return the child handle", () => {
    const page = __CreatePage("card", 0);
    const first = __CreateView(0);
    const second = __CreateView(0);
    expect(__AppendElement(page, first)).toBe(first);
    expect(__InsertElementBefore(page, second, first)).toBe(second);
    expect(__InsertElementBefore(page, second)).toBe(second);
    expect(__InsertElementBefore(page, second, null)).toBe(second);
    expect(__RemoveElement(page, first)).toBe(first);
    expect(__ReplaceElement(second, first)).toBeUndefined();
    expect(mock.named("insertBefore")).toEqual([
      ["insertBefore", 1, 2, null],
      ["insertBefore", 1, 3, 2],
      ["insertBefore", 1, 3, null],
      ["insertBefore", 1, 3, null],
    ]);
    expect(mock.named("removeElement")).toEqual([["removeElement", 1, 2]]);
    expect(mock.named("replaceElement")).toEqual([["replaceElement", 3, 2]]);
  });

  it("reject anything but a live-branded handle object", () => {
    const view = __CreateView(0);
    for (const bad of [0, "not a handle", null, undefined, {}, Symbol("x")]) {
      expect(() => __AppendElement(bad, view)).toThrow(
        "__AppendElement expects an element handle for argument 0",
      );
    }
    expect(() => __AppendElement(view, 7)).toThrow(
      "__AppendElement expects an element handle for argument 1",
    );
  });

  it("reject a non-handle insertion reference but accept nullish ones", () => {
    const page = __CreatePage("card", 0);
    const view = __CreateView(0);
    expect(() => __InsertElementBefore(page, view, "x")).toThrow(
      "__InsertElementBefore expects an element handle, null, or undefined for argument 2",
    );
  });

  it("forward retired unique ids so the native side owns liveness errors", () => {
    const page = __CreatePage("card", 0);
    const dropped = __CreateView(0);
    __DropElement(dropped);
    __AppendElement(page, dropped);
    expect(mock.named("insertBefore")).toEqual([["insertBefore", 1, 2, null]]);
  });
});

describe("__DropElement", () => {
  it("retires the element natively and tolerates a second drop", () => {
    const view = __CreateView(0);
    expect(__DropElement(view)).toBeUndefined();
    expect(__DropElement(view)).toBeUndefined();
    expect(mock.named("dropElement")).toEqual([["dropElement", 2]]);
  });

  it("lets the native page rejection propagate", () => {
    const page = __CreatePage("card", 0);
    expect(() => __DropElement(page)).toThrow(
      "the page element cannot be removed",
    );
  });

});

describe("__FlushElementTree and delivery", () => {
  it("flushes through the native object", () => {
    __FlushElementTree();
    expect(mock.named("flushElementTree")).toHaveLength(1);
  });

  it("delivers nothing when no handle was collected", () => {
    __CreateView(0);
    mock.deliverPendingElementDrops?.();
    expect(mock.named("dropElement")).toEqual([]);
  });
});
