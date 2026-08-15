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
  /** @type {Map<number, number>} */
  const parents = new Map();
  /** @type {Map<number, Map<string, string>>} */
  const attributes = new Map();
  /** @type {Map<number, string>} */
  const tags = new Map([[1, "page"]]);
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
      tags.set(node, tag);
      calls.push(["createElement", tag]);
      return node;
    },
    /**
     * @param {unknown} node
     * @param {string} name
     * @param {string} value
     */
    setAttribute: (node, name, value) => {
      const id = nodeId("setAttribute", node);
      if (typeof value !== "string") {
        throw new TypeError("setAttribute expects a string for argument 2");
      }
      let element = attributes.get(id);
      if (element === undefined) {
        element = new Map();
        attributes.set(id, element);
      }
      element.set(name, value);
      calls.push(["setAttribute", id, name, value]);
    },
    /**
     * @param {unknown} node
     * @param {string} name
     */
    removeAttribute: (node, name) => {
      const id = nodeId("removeAttribute", node);
      attributes.get(id)?.delete(name);
      calls.push(["removeAttribute", id, name]);
    },
    /**
     * @param {unknown} node
     * @param {string} name
     */
    getAttribute: (node, name) => {
      const id = nodeId("getAttribute", node);
      calls.push(["getAttribute", id, name]);
      return attributes.get(id)?.get(name) ?? null;
    },
    /** @param {unknown} node */
    tagName: (node) => {
      const id = nodeId("tagName", node);
      calls.push(["tagName", id]);
      const tag = tags.get(id);
      if (tag === undefined) {
        throw new Error(`tagName: ${id} is not a live element`);
      }
      return tag;
    },
    /** @param {unknown} node */
    parentNode: (node) => {
      const id = nodeId("parentNode", node);
      calls.push(["parentNode", id]);
      return parents.get(id) ?? null;
    },
    /**
     * @param {unknown} parent
     * @param {unknown} child
     * @param {unknown} reference
     */
    insertBefore: (parent, child, reference) => {
      const parentId = nodeId("insertBefore", parent);
      const childId = nodeId("insertBefore", child);
      parents.set(childId, parentId);
      calls.push([
        "insertBefore",
        parentId,
        childId,
        reference === null ? null : nodeId("insertBefore", reference),
      ]);
    },
    /** @param {unknown} child */
    removeElement: (child) => {
      const childId = nodeId("removeElement", child);
      parents.delete(childId);
      calls.push(["removeElement", childId]);
    },
    /**
     * @param {unknown} newElement
     * @param {unknown} oldElement
     */
    replaceElement: (newElement, oldElement) => {
      const newId = nodeId("replaceElement", newElement);
      const oldId = nodeId("replaceElement", oldElement);
      const parent = parents.get(oldId);
      if (parent !== undefined) {
        parents.set(newId, parent);
        parents.delete(oldId);
      }
      calls.push(["replaceElement", newId, oldId]);
    },
    /**
     * @param {unknown} childA
     * @param {unknown} childB
     */
    swapElement: (childA, childB) => {
      const a = nodeId("swapElement", childA);
      const b = nodeId("swapElement", childB);
      const parentA = parents.get(a);
      const parentB = parents.get(b);
      if (parentA !== undefined) {
        parents.set(b, parentA);
      } else {
        parents.delete(b);
      }
      if (parentB !== undefined) {
        parents.set(a, parentB);
      } else {
        parents.delete(a);
      }
      calls.push(["swapElement", a, b]);
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
      ["__ReplaceElements", 3],
      ["__SwapElement", 2],
      ["__SetClasses", 2],
      ["__SetID", 2],
      ["__GetID", 1],
      ["__GetTag", 1],
      ["__GetElementUniqueID", 1],
      ["__SetInlineStyles", 2],
      ["__SetAttribute", 3],
      ["__SetCSSId", 3],
      ["__AddEvent", 4],
      ["__GetEvent", 3],
      ["__GetEvents", 1],
      ["__FlushElementTree", 0],
    ];
    for (const [name, arity] of arities) {
      const papi = /** @type {Record<string, unknown>} */ (globalThis)[name];
      expect(papi, name).toBeTypeOf("function");
      expect(/** @type {Function} */ (papi).length, name).toBe(arity);
    }
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

describe("__ReplaceElements", () => {
  it("appends when there are no old children, accepting both shapes", () => {
    const page = __CreatePage("card", 0);
    const first = __CreateView(0);
    const second = __CreateView(0);
    __ReplaceElements(page, [first, second]);
    __ReplaceElements(page, first, null);
    __ReplaceElements(page, first, []);
    expect(mock.named("insertBefore")).toEqual([
      ["insertBefore", 1, 2, null],
      ["insertBefore", 1, 3, null],
      ["insertBefore", 1, 2, null],
      ["insertBefore", 1, 2, null],
    ]);
  });

  it("detaches the tail old children and replaces the first in place", () => {
    const page = __CreatePage("card", 0);
    const oldA = __CreateView(0);
    const oldB = __CreateView(0);
    const newA = __CreateView(0);
    const newB = __CreateView(0);
    __AppendElement(page, oldA);
    __AppendElement(page, oldB);
    mock.calls.length = 0;

    __ReplaceElements(page, [newA, newB], [oldA, oldB]);
    expect(mock.calls).toEqual([
      ["removeElement", 3],
      ["parentNode", 2],
      ["insertBefore", 1, 4, 2],
      ["insertBefore", 1, 5, 2],
      ["removeElement", 2],
    ]);
  });

  it("does nothing when the first old child is detached, like replaceWith", () => {
    const page = __CreatePage("card", 0);
    const detached = __CreateView(0);
    const replacement = __CreateView(0);
    void page;
    mock.calls.length = 0;

    __ReplaceElements(page, replacement, detached);
    expect(mock.calls).toEqual([["parentNode", 2]]);
  });
});

describe("__SwapElement", () => {
  it("uses the native swap for two attached elements", () => {
    const page = __CreatePage("card", 0);
    const a = __CreateView(0);
    const b = __CreateView(0);
    __AppendElement(page, a);
    __AppendElement(page, b);
    mock.calls.length = 0;

    __SwapElement(a, b);
    expect(mock.calls).toEqual([
      ["parentNode", 2],
      ["parentNode", 3],
      ["swapElement", 2, 3],
    ]);
  });

  it("composes the degenerate patterns over the simple members", () => {
    const page = __CreatePage("card", 0);
    const attached = __CreateView(0);
    const detachedA = __CreateView(0);
    const detachedB = __CreateView(0);
    __AppendElement(page, attached);
    mock.calls.length = 0;

    __SwapElement(attached, attached);
    expect(mock.calls).toEqual([]);

    __SwapElement(attached, detachedA);
    expect(mock.calls).toEqual([
      ["parentNode", 2],
      ["parentNode", 3],
      ["replaceElement", 3, 2],
    ]);
    mock.calls.length = 0;

    // The first swap left detachedA attached and `attached` detached, so
    // the roles flip: the attached operand is replaced again.
    __SwapElement(detachedA, attached);
    expect(mock.calls).toEqual([
      ["parentNode", 3],
      ["parentNode", 2],
      ["replaceElement", 2, 3],
    ]);
    mock.calls.length = 0;

    __SwapElement(detachedA, detachedB);
    expect(mock.calls).toEqual([
      ["parentNode", 3],
      ["parentNode", 4],
    ]);
  });
});

describe("__SetClasses", () => {
  it("sets the class attribute", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetClasses(view, "row bold");
    expect(mock.calls).toEqual([["setAttribute", 2, "class", "row bold"]]);
  });

  it("removes the attribute for every falsy class list", () => {
    const view = __CreateView(0);
    for (const empty of ["", null, undefined]) {
      mock.calls.length = 0;
      __SetClasses(view, empty);
      expect(mock.calls, String(empty)).toEqual([
        ["removeAttribute", 2, "class"],
      ]);
    }
  });
});

describe("id", () => {
  it("sets, reads back, and removes the id attribute", () => {
    const view = __CreateView(0);
    expect(__GetID(view)).toBe(null);

    __SetID(view, "header");
    expect(__GetID(view)).toBe("header");

    __SetID(view, null);
    expect(__GetID(view)).toBe(null);
  });

  it("treats an empty id as a removal, like web-core", () => {
    const view = __CreateView(0);
    __SetID(view, "header");
    mock.calls.length = 0;

    __SetID(view, "");
    expect(mock.calls).toEqual([["removeAttribute", 2, "id"]]);
  });
});

describe("__GetTag", () => {
  it("reports the tag each element was created with", () => {
    const page = __CreatePage("card", 0);
    expect(__GetTag(page)).toBe("page");
    expect(__GetTag(__CreateView(0))).toBe("view");
    expect(__GetTag(__CreateText(0))).toBe("text");
    expect(__GetTag(__CreateImage(0))).toBe("image");
    expect(__GetTag(__CreateScrollView(0))).toBe("scroll-view");
    expect(__GetTag(__CreateWrapperElement(0))).toBe("wrapper");
    expect(__GetTag(__CreateRawText("x"))).toBe("raw-text");
    expect(__GetTag(__CreateList(0, () => {}, () => {}))).toBe("list");
    expect(__GetTag(__CreateElement("custom-widget", 0))).toBe("custom-widget");
  });
});

describe("__GetElementUniqueID", () => {
  it("reports the handle's node id", () => {
    const page = __CreatePage("card", 0);
    const view = __CreateView(0);
    expect(__GetElementUniqueID(page)).toBe(1);
    expect(__GetElementUniqueID(view)).toBe(2);
  });

  it("reports -1 for a falsy or foreign element instead of throwing", () => {
    for (const foreign of [null, undefined, {}, [], "x"]) {
      expect(__GetElementUniqueID(foreign), String(foreign)).toBe(-1);
    }
  });
});

describe("__SetInlineStyles", () => {
  it("sets a declaration string verbatim", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetInlineStyles(view, "color:red;width:10px");
    expect(mock.calls).toEqual([
      ["setAttribute", 2, "style", "color:red;width:10px"],
    ]);
  });

  it("hyphenates a record and skips null and undefined values", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetInlineStyles(view, {
      backgroundColor: "red",
      borderTopLeftRadius: 4,
      color: undefined,
      width: null,
    });
    expect(mock.calls).toEqual([[
      "setAttribute",
      2,
      "style",
      "background-color:red;border-top-left-radius:4;",
    ]]);
  });

  it("removes the attribute for every falsy value", () => {
    const view = __CreateView(0);
    for (const empty of ["", null, undefined]) {
      mock.calls.length = 0;
      __SetInlineStyles(view, empty);
      expect(mock.calls, String(empty)).toEqual([
        ["removeAttribute", 2, "style"],
      ]);
    }
  });
});

describe("__SetAttribute", () => {
  it("stringifies the value", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetAttribute(view, "text", "hello");
    __SetAttribute(view, "flex-grow", 1);
    __SetAttribute(view, "clip-radius", true);
    expect(mock.calls).toEqual([
      ["setAttribute", 2, "text", "hello"],
      ["setAttribute", 2, "flex-grow", "1"],
      ["setAttribute", 2, "clip-radius", "true"],
    ]);
  });

  it("removes the attribute for null and undefined", () => {
    const view = __CreateView(0);
    for (const absent of [null, undefined]) {
      mock.calls.length = 0;
      __SetAttribute(view, "text", absent);
      expect(mock.calls, String(absent)).toEqual([
        ["removeAttribute", 2, "text"],
      ]);
    }
  });

  it("hands id, class, and style to the native boundary unchanged", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetAttribute(view, "id", "header");
    __SetAttribute(view, "class", "row");
    __SetAttribute(view, "style", "color:red");
    expect(mock.calls).toEqual([
      ["setAttribute", 2, "id", "header"],
      ["setAttribute", 2, "class", "row"],
      ["setAttribute", 2, "style", "color:red"],
    ]);
  });

  it("refuses update-list-info rather than writing a command object", () => {
    const list = __CreateList(0, () => {}, () => {});
    mock.calls.length = 0;

    expect(() =>
      __SetAttribute(list, "update-list-info", {
        insertAction: [],
        removeAction: [],
      })
    ).toThrow("update-list-info");
    expect(mock.calls).toEqual([]);
  });
});

describe("__SetCSSId", () => {
  it("writes the scope id onto every element of the batch", () => {
    const first = __CreateView(0);
    const second = __CreateView(0);
    mock.calls.length = 0;

    __SetCSSId([first, second], 7);
    expect(mock.calls).toEqual([
      ["setAttribute", 2, "l-css-id", "7"],
      ["setAttribute", 3, "l-css-id", "7"],
    ]);
  });

  it("removes the scope id at 0 and null, ReactLynx's default scope", () => {
    const view = __CreateView(0);
    for (const empty of [0, null, undefined]) {
      mock.calls.length = 0;
      __SetCSSId([view], empty);
      expect(mock.calls, String(empty)).toEqual([
        ["removeAttribute", 2, "l-css-id"],
      ]);
    }
  });

  it("writes the entry name only when the bundle passes one", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetCSSId([view], 3, "lazy-entry");
    expect(mock.calls).toEqual([
      ["setAttribute", 2, "l-e-name", "lazy-entry"],
      ["setAttribute", 2, "l-css-id", "3"],
    ]);
    mock.calls.length = 0;

    __SetCSSId([view], 3);
    expect(mock.calls).toEqual([["setAttribute", 2, "l-css-id", "3"]]);
  });
});

describe("events", () => {
  it("records a background-thread handler name and reads it back", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __AddEvent(view, "bindEvent", "tap", "handler:1");
    expect(__GetEvent(view, "tap", "bindEvent")).toBe("handler:1");
    expect(__GetEvents(view)).toEqual([
      { type: "bindevent", name: "tap", function: "handler:1" },
    ]);
    // Registration is this runtime's own bookkeeping, not a DOM mutation.
    expect(mock.calls).toEqual([]);
  });

  it("lowercases the event type and name", () => {
    const view = __CreateView(0);
    __AddEvent(view, "CATCHEvent", "TouchStart", "handler:1");
    expect(__GetEvent(view, "touchstart", "catchevent")).toBe("handler:1");
    expect(__GetEvents(view)).toEqual([
      { type: "catchevent", name: "touchstart", function: "handler:1" },
    ]);
  });

  it("keeps the worklet slot separate from the background one", () => {
    const view = __CreateView(0);
    const worklet = { type: "worklet", value: { _wkltId: "1:2" } };

    __AddEvent(view, "bindEvent", "tap", worklet);
    // __GetEvent reads the background slot alone, as web-core's does.
    expect(__GetEvent(view, "tap", "bindEvent")).toBe(undefined);
    expect(__GetEvents(view)).toEqual([
      { type: "bindevent", name: "tap", function: worklet },
    ]);

    __AddEvent(view, "bindEvent", "tap", "handler:1");
    expect(__GetEvent(view, "tap", "bindEvent")).toBe("handler:1");
    expect(__GetEvents(view)).toEqual([
      { type: "bindevent", name: "tap", function: "handler:1" },
      { type: "bindevent", name: "tap", function: worklet },
    ]);
  });

  it("clears both slots for a null handler", () => {
    const view = __CreateView(0);
    __AddEvent(view, "bindEvent", "tap", "handler:1");
    __AddEvent(view, "bindEvent", "tap", { type: "worklet", value: {} });

    __AddEvent(view, "bindEvent", "tap", null);
    expect(__GetEvent(view, "tap", "bindEvent")).toBe(undefined);
    expect(__GetEvents(view)).toEqual([]);
  });

  it("keeps one registration per element, type, and name", () => {
    const first = __CreateView(0);
    const second = __CreateView(0);
    __AddEvent(first, "bindEvent", "tap", "handler:1");
    __AddEvent(first, "catchEvent", "tap", "handler:2");
    __AddEvent(second, "bindEvent", "tap", "handler:3");

    expect(__GetEvents(first)).toEqual([
      { type: "bindevent", name: "tap", function: "handler:1" },
      { type: "catchevent", name: "tap", function: "handler:2" },
    ]);
    expect(__GetEvents(second)).toEqual([
      { type: "bindevent", name: "tap", function: "handler:3" },
    ]);
  });

  it("reports no events for an element that never registered one", () => {
    const view = __CreateView(0);
    expect(__GetEvents(view)).toEqual([]);
    expect(__GetEvent(view, "tap", "bindEvent")).toBe(undefined);
  });
});

describe("__FlushElementTree", () => {
  it("flushes through the native object", () => {
    __FlushElementTree();
    expect(mock.named("flushElementTree")).toHaveLength(1);
  });
});
