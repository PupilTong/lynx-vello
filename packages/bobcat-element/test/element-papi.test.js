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
 * @param {number[]} [issuedIds] Ids the native half hands out, in call order.
 *   Defaults to the real boundary's shape (2, 3, 4, ...); a test that needs to
 *   prove the runtime carries native's number through rather than numbering
 *   handles itself passes a sequence no counter would produce.
 * @returns {BobcatNative & { calls: unknown[][], named: (name: string) => unknown[][] }}
 */
function createMockBobcat(issuedIds) {
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
  // Mirrors the real boundary: the DOM reserves id 0, takes 1 for the
  // document node, and hands the page 2, so the first created element is 3.
  let nextNodeId = 3;
  let issued = 0;
  /** @returns {number} */
  const issueNodeId = () => {
    if (issuedIds !== undefined) {
      const id = issuedIds[issued];
      issued += 1;
      if (id === undefined) {
        throw new Error("the mock ran out of ids to issue");
      }
      return id;
    }
    const id = nextNodeId;
    nextNodeId += 1;
    return id;
  };
  /** @type {Map<number, number>} */
  const parents = new Map();
  /** @type {Map<number, Map<string, string>>} */
  const attributes = new Map();
  /** @type {Map<number, string>} */
  const tags = new Map([[2, "page"]]);
  return {
    calls,
    named,
    createPage: () => {
      calls.push(["createPage"]);
      return 2;
    },
    /** @param {string} tag */
    createElement: (tag) => {
      const node = issueNodeId();
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
     * @param {string} value
     */
    set_node_property: (node, name, value) => {
      const id = nodeId("set_node_property", node);
      if (typeof name !== "string" || typeof value !== "string") {
        throw new TypeError("set_node_property expects string name and value");
      }
      calls.push(["set_node_property", id, name, value]);
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

  it("does not install __SetCSSId: the scope it names has no consumer", () => {
    expect("__SetCSSId" in globalThis).toBe(false);
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
      ["insertBefore", 2, 3, null],
      ["insertBefore", 2, 4, null],
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
      ["setAttribute", 3, "text", "Hello, Lynx"],
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
      ["insertBefore", 2, 3, null],
      ["insertBefore", 2, 4, 3],
      ["insertBefore", 2, 4, null],
      ["insertBefore", 2, 4, null],
    ]);
    expect(mock.named("removeElement")).toEqual([["removeElement", 3]]);
    expect(mock.named("replaceElement")).toEqual([["replaceElement", 4, 3]]);
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
      ["insertBefore", 2, 3, null],
      ["insertBefore", 2, 4, null],
      ["insertBefore", 2, 3, null],
      ["insertBefore", 2, 3, null],
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
      ["removeElement", 4],
      ["parentNode", 3],
      ["insertBefore", 2, 5, 3],
      ["insertBefore", 2, 6, 3],
      ["removeElement", 3],
    ]);
  });

  it("does nothing when the first old child is detached, like replaceWith", () => {
    const page = __CreatePage("card", 0);
    const detached = __CreateView(0);
    const replacement = __CreateView(0);
    void page;
    mock.calls.length = 0;

    __ReplaceElements(page, replacement, detached);
    expect(mock.calls).toEqual([["parentNode", 3]]);
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
      ["parentNode", 3],
      ["parentNode", 4],
      ["swapElement", 3, 4],
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
      ["parentNode", 3],
      ["parentNode", 4],
      ["replaceElement", 4, 3],
    ]);
    mock.calls.length = 0;

    // The first swap left detachedA attached and `attached` detached, so
    // the roles flip: the attached operand is replaced again.
    __SwapElement(detachedA, attached);
    expect(mock.calls).toEqual([
      ["parentNode", 4],
      ["parentNode", 3],
      ["replaceElement", 3, 4],
    ]);
    mock.calls.length = 0;

    __SwapElement(detachedA, detachedB);
    expect(mock.calls).toEqual([
      ["parentNode", 4],
      ["parentNode", 5],
    ]);
  });
});

describe("__SetClasses", () => {
  it("sets the class attribute", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetClasses(view, "row bold");
    expect(mock.calls).toEqual([["setAttribute", 3, "class", "row bold"]]);
  });

  it("removes the attribute for every falsy class list", () => {
    const view = __CreateView(0);
    for (const empty of ["", null, undefined]) {
      mock.calls.length = 0;
      __SetClasses(view, empty);
      expect(mock.calls, String(empty)).toEqual([
        ["removeAttribute", 3, "class"],
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
    expect(mock.calls).toEqual([["removeAttribute", 3, "id"]]);
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
  it("reports the handle's node id, which is the id native issued", () => {
    const page = __CreatePage("card", 0);
    const view = __CreateView(0);
    expect(__GetElementUniqueID(page)).toBe(2);
    expect(__GetElementUniqueID(view)).toBe(3);
    expect(mock.named("createElement")).toHaveLength(1);
  });

  it("mints no id of its own: it reports back exactly what native issued", async () => {
    // Ids no counter on this side could produce: out of order, with gaps.
    // Anything the runtime derived itself would disagree with this sequence.
    const issued = [41, 7, 900];
    rstest.resetModules();
    globalThis.bobcat = createMockBobcat(issued);
    await import("../src/element-papi.js");

    const created = [
      __CreateView(0),
      __CreateText(0),
      __CreateElement("custom-widget", 0),
    ];

    expect(created.map((element) => __GetElementUniqueID(element)))
      .toStrictEqual(issued);
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
      ["setAttribute", 3, "style", "color:red;width:10px"],
    ]);
  });

  it("fans a record out into ordered single-property updates", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetInlineStyles(view, {
      backgroundColor: "red",
      borderTopLeftRadius: 4,
      color: undefined,
      width: null,
    });
    expect(mock.calls).toEqual([
      ["setAttribute", 3, "style", ""],
      ["set_node_property", 3, "background-color", "red"],
      ["set_node_property", 3, "border-top-left-radius", "4"],
    ]);
  });

  it("resets the complete declaration block before applying a record", () => {
    const view = __CreateView(0);
    __SetInlineStyles(view, "width:10px;color:red");
    mock.calls.length = 0;

    __SetInlineStyles(view, { height: "20px" });
    expect(mock.calls).toEqual([
      ["setAttribute", 3, "style", ""],
      ["set_node_property", 3, "height", "20px"],
    ]);
  });

  it("preserves the case-sensitive name of a custom property", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetInlineStyles(view, { "--accentColor": "tomato" });
    expect(mock.calls).toEqual([
      ["setAttribute", 3, "style", ""],
      ["set_node_property", 3, "--accentColor", "tomato"],
    ]);
  });

  it("skips nullish declarations but forwards invalid names for CSS to reject", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetInlineStyles(view, {
      color: null,
      width: undefined,
      definitelyNotAProperty: "value",
    });
    expect(mock.calls).toEqual([
      ["setAttribute", 3, "style", ""],
      ["set_node_property", 3, "definitely-not-a-property", "value"],
    ]);
  });

  it("keeps an empty style attribute for an empty or all-nullish record", () => {
    const view = __CreateView(0);
    for (const styles of [{}, { color: null, width: undefined }]) {
      mock.calls.length = 0;
      __SetInlineStyles(view, styles);
      expect(mock.calls).toEqual([["setAttribute", 3, "style", ""]]);
    }
  });

  it("removes the attribute for every falsy value", () => {
    const view = __CreateView(0);
    for (const empty of ["", null, undefined]) {
      mock.calls.length = 0;
      __SetInlineStyles(view, empty);
      expect(mock.calls, String(empty)).toEqual([
        ["removeAttribute", 3, "style"],
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
      ["setAttribute", 3, "text", "hello"],
      ["setAttribute", 3, "flex-grow", "1"],
      ["setAttribute", 3, "clip-radius", "true"],
    ]);
  });

  it("removes the attribute for null and undefined", () => {
    const view = __CreateView(0);
    for (const absent of [null, undefined]) {
      mock.calls.length = 0;
      __SetAttribute(view, "text", absent);
      expect(mock.calls, String(absent)).toEqual([
        ["removeAttribute", 3, "text"],
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
      ["setAttribute", 3, "id", "header"],
      ["setAttribute", 3, "class", "row"],
      ["setAttribute", 3, "style", "color:red"],
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
