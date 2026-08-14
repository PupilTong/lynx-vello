// @ts-check
// Behavior tests for the Element PAPI runtime over a recording native mock.
//
// These pin the semantics that live in element-papi.js: the PAPI surface and
// arities, tag vocabulary, handle-to-NodeId mapping, return identity, and
// drop bookkeeping. The mock mirrors the real boundary's shape: it returns
// sequential node ids and rejects non-number ids the way the native number
// extraction does. Structural behavior and the collection-driven drop path
// run against the real native side in
// crates/bobcat-core/tests/main_thread.rs.

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
   * @param {unknown} value
   * @returns {number}
   */
  const nodeId = (name, value) => {
    if (typeof value !== "number") {
      throw new TypeError(`${name} expects a number`);
    }
    return value;
  };
  let nextNodeId = 2;
  return {
    calls,
    named,
    createPage: () => {
      calls.push(["createPage"]);
      return 1;
    },
    /** @param {string} tag */
    createElement: (tag) => {
      const node = nextNodeId;
      nextNodeId += 1;
      calls.push(["createElement", tag]);
      return node;
    },
    /**
     * @param {unknown} node
     * @param {string} name
     * @param {string} value
     */
    setAttribute: (node, name, value) => {
      calls.push(["setAttribute", nodeId("setAttribute", node), name, value]);
    },
    /**
     * @param {unknown} parent
     * @param {unknown} child
     * @param {unknown} reference
     */
    insertBefore: (parent, child, reference) => {
      calls.push([
        "insertBefore",
        nodeId("insertBefore", parent),
        nodeId("insertBefore", child),
        reference === null ? null : nodeId("insertBefore", reference),
      ]);
    },
    /** @param {unknown} child */
    removeElement: (child) => {
      calls.push(["removeElement", nodeId("removeElement", child)]);
    },
    /**
     * @param {unknown} newElement
     * @param {unknown} oldElement
     */
    replaceElement: (newElement, oldElement) => {
      calls.push([
        "replaceElement",
        nodeId("replaceElement", newElement),
        nodeId("replaceElement", oldElement),
      ]);
    },
    /** @param {unknown} node */
    dropElement: (node) => {
      calls.push(["dropElement", nodeId("dropElement", node)]);
    },
    flushElementTree: () => {
      calls.push(["flushElementTree"]);
    },
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

  it("does not install __DropElement: collection is the only release path", () => {
    expect("__DropElement" in globalThis).toBe(false);
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
    expect(mock.named("createElement")).toEqual([["createElement", "view"]]);
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

  it("ignores its arguments entirely", () => {
    expect(() => __CreatePage()).not.toThrow();
    expect(() => __CreatePage(5, {})).not.toThrow();
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

  it("return distinct opaque object handles bound to native node ids", () => {
    const page = __CreatePage("card", 0);
    const first = __CreateView(0);
    const second = __CreateView(0);
    expect(first).toBeTypeOf("object");
    expect(second).not.toBe(first);
    __AppendElement(page, first);
    __AppendElement(page, second);
    expect(mock.named("insertBefore")).toEqual([
      ["insertBefore", 1, 2, null],
      ["insertBefore", 1, 3, null],
    ]);
  });

  it("ignore the parent component id entirely", () => {
    for (const anything of [0, 4294967295, "x", 1.5, {}, undefined]) {
      expect(() => __CreateView(anything)).not.toThrow();
    }
  });

  it("store raw text through setAttribute", () => {
    __CreateRawText("Hello, Lynx");
    expect(mock.named("setAttribute")).toEqual([
      ["setAttribute", 2, "text", "Hello, Lynx"],
    ]);
  });
});

describe("tree mutations", () => {
  it("forward node ids and return the child handle", () => {
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
    expect(mock.named("removeElement")).toEqual([["removeElement", 2]]);
    expect(mock.named("replaceElement")).toEqual([["replaceElement", 3, 2]]);
  });

  it("crash on foreign, primitive, or nullish handles", () => {
    const view = __CreateView(0);
    for (const bad of [0, "not a handle", {}]) {
      expect(() => __AppendElement(bad, view)).toThrow("expects a number");
    }
    for (const bad of [null, undefined]) {
      expect(() => __AppendElement(bad, view)).toThrow(TypeError);
    }
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
