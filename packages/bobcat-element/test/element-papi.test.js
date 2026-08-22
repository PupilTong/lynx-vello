// @ts-check
// Behavior tests for the Element PAPI runtime over a recording native mock.
//
// These pin the semantics that live in element-papi.mjs: the PAPI surface and
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
  /** @type {BobcatNative & { calls: unknown[][], named: (name: string) => unknown[][] }} */
  const host = {
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
    /**
     * @param {unknown} node
     * @param {unknown} phase
     * @param {unknown} eventName
     */
    enableEventListener: (node, phase, eventName) => {
      calls.push(["enableEventListener", node, phase, eventName]);
    },
    /**
     * @param {unknown} node
     * @param {unknown} phase
     * @param {unknown} eventName
     */
    disableEventListener: (node, phase, eventName) => {
      calls.push(["disableEventListener", node, phase, eventName]);
    },
    stopPropagation: () => {
      calls.push(["stopPropagation"]);
    },
  };
  return host;
}

/** @type {ReturnType<typeof createMockBobcat>} */
let mock;
/** @type {typeof import("../src/element-papi.mjs")} */
let elementModule;

beforeEach(async () => {
  rstest.resetModules();
  mock = createMockBobcat();
  globalThis.bobcat = mock;
  // Installed by a card's own worklet runtime, never by this file; a test
  // that wants one puts it here itself.
  globalThis.runWorklet = undefined;
  elementModule = await import("../src/element-papi.mjs");
  // The rest of this legacy-shaped behavior suite calls PAPI names directly;
  // expose this test instance without making global installation a module
  // responsibility.
  Object.assign(globalThis, elementModule);
});

describe("installation", () => {
  it("exports every PAPI binding with the arity its reference declares", () => {
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
      ["__SetCSSId", 3],
      ["__SetAttribute", 3],
      ["__UpdateListCallbacks", 4],
      ["__AddEvent", 4],
      ["__GetEvent", 3],
      ["__GetEvents", 1],
      ["__SetEvents", 2],
      ["__AddEventListener", 4],
      ["__RemoveEventListener", 4],
      ["__StopPropagation", 1],
      ["__StopImmediatePropagation", 1],
      ["__FlushElementTree", 0],
    ];
    for (const [name, arity] of arities) {
      const papi = /** @type {Record<string, unknown>} */ (elementModule)[name];
      expect(papi, name).toBeTypeOf("function");
      expect(/** @type {Function} */ (papi).length, name).toBe(arity);
    }
    expect(Object.keys(elementModule).sort()).toEqual(
      arities.map(([name]) => name).sort(),
    );
  });

  it("does not install __DropElement: collection is the only release path", () => {
    expect("__DropElement" in globalThis).toBe(false);
  });

  it("accepts __SetCSSId and records nothing: the scope it names has no consumer", () => {
    const element = __CreateView(0);
    mock.calls.length = 0;
    expect(__SetCSSId([element], 7, "entry")).toBeUndefined();
    expect(__SetCSSId([element], null, undefined)).toBeUndefined();
    expect(mock.calls).toEqual([]);
  });

  it("fails loudly when the native object is missing", async () => {
    rstest.resetModules();
    // @ts-expect-error deliberately removing the native object
    globalThis.bobcat = undefined;
    await expect(import("../src/element-papi.mjs")).rejects.toThrow(
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
    const isolatedModule = await import("../src/element-papi.mjs");

    const created = [
      isolatedModule.__CreateView(0),
      isolatedModule.__CreateText(0),
      isolatedModule.__CreateElement("custom-widget", 0),
    ];

    expect(created.map((element) => isolatedModule.__GetElementUniqueID(element)))
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


describe("__FlushElementTree", () => {
  it("flushes through the native object", () => {
    __FlushElementTree();
    expect(mock.named("flushElementTree")).toHaveLength(1);
  });
});


/** Builds `page > outer > inner` and returns the three handles. */
function tree() {
  const page = __CreatePage("card", 0);
  const outer = __CreateView(0);
  const inner = __CreateView(0);
  __AppendElement(page, outer);
  __AppendElement(outer, inner);
  return { page, outer, inner };
}

let nextEventId = 0;

/** Delivers one node's turn as the only call of its own dispatch. */
function deliver(
  /** @type {object} */ node,
  /** @type {object} */ target,
  /** @type {number} */ phase,
  /** @type {string} */ name,
  /** @type {string} */ detailJson = "",
) {
  walk([{ node, target, phase }], name, detailJson);
}

/**
 * Delivers a whole path under one event id, the way the host does: one id
 * for the walk, `isLastCall` only on the final step.
 */
function walk(
  /** @type {{ node: object, target: object, phase: number }[]} */ steps,
  /** @type {string} */ name,
  /** @type {string} */ detailJson = "",
) {
  const eventId = nextEventId;
  nextEventId += 1;
  steps.forEach((step, index) => {
    mock.event_listener_callback?.(
      __GetElementUniqueID(step.node),
      __GetElementUniqueID(step.target),
      step.phase,
      name,
      detailJson,
      eventId,
      index === steps.length - 1,
    );
  });
}

const BUBBLE = 0;
const CAPTURE = 1;

describe("event listeners", () => {
  it("tells the host the first listener arrived, and only the first", () => {
    const { inner } = tree();
    const uid = __GetElementUniqueID(inner);

    __AddEventListener(inner, "tap", () => {}, {});
    __AddEventListener(inner, "tap", () => {}, {});

    expect(mock.named("enableEventListener")).toEqual([
      ["enableEventListener", uid, BUBBLE, "tap"],
    ]);
  });

  it("tells the host the last listener left, and only the last", () => {
    const { inner } = tree();
    const uid = __GetElementUniqueID(inner);
    const a = () => {};
    const b = () => {};
    __AddEventListener(inner, "tap", a, {});
    __AddEventListener(inner, "tap", b, {});

    __RemoveEventListener(inner, "tap", a, {});
    expect(mock.named("disableEventListener")).toEqual([]);

    __RemoveEventListener(inner, "tap", b, {});
    expect(mock.named("disableEventListener")).toEqual([
      ["disableEventListener", uid, BUBBLE, "tap"],
    ]);
  });

  it("counts each pass separately, which is what the phase argument is for", () => {
    const { inner } = tree();
    const uid = __GetElementUniqueID(inner);
    const handler = () => {};

    __AddEventListener(inner, "tap", handler, {});
    __AddEventListener(inner, "tap", handler, { capture: true });

    expect(mock.named("enableEventListener")).toEqual([
      ["enableEventListener", uid, BUBBLE, "tap"],
      ["enableEventListener", uid, CAPTURE, "tap"],
    ]);

    // Capture is part of the identity, so a bubble removal leaves the capture
    // registration — and the host still hears about this node.
    __RemoveEventListener(inner, "tap", handler, {});
    expect(mock.named("disableEventListener")).toEqual([
      ["disableEventListener", uid, BUBBLE, "tap"],
    ]);
  });

  it("runs the pass's own listeners in registration order", () => {
    const { inner } = tree();
    /** @type {string[]} */
    const order = [];
    __AddEventListener(inner, "tap", () => order.push("bubble-1"), {});
    __AddEventListener(inner, "tap", () => order.push("bubble-2"), {});
    __AddEventListener(inner, "tap", () => order.push("capture"), {
      capture: true,
    });

    deliver(inner, inner, CAPTURE, "tap");
    deliver(inner, inner, BUBBLE, "tap");

    expect(order).toEqual(["capture", "bubble-1", "bubble-2"]);
  });

  it("re-adding the same callback is ignored, options and all", () => {
    const { inner } = tree();
    let runs = 0;
    const handler = () => {
      runs += 1;
    };
    __AddEventListener(inner, "tap", handler, {});
    __AddEventListener(inner, "tap", handler, { once: true });

    deliver(inner, inner, BUBBLE, "tap");
    deliver(inner, inner, BUBBLE, "tap");

    expect(runs).toBe(2);
  });

  it("removes a once listener before running it", () => {
    const { inner } = tree();
    let runs = 0;
    __AddEventListener(inner, "tap", () => {
      runs += 1;
    }, { once: true });

    deliver(inner, inner, BUBBLE, "tap");
    deliver(inner, inner, BUBBLE, "tap");

    expect(runs).toBe(1);
    expect(mock.named("disableEventListener")).toHaveLength(1);
  });

  it("matches the event name case-insensitively on both sides", () => {
    const { inner } = tree();
    let runs = 0;
    const handler = () => {
      runs += 1;
    };
    __AddEventListener(inner, "TAP", handler, {});
    deliver(inner, inner, BUBBLE, "tap");
    expect(runs).toBe(1);

    __RemoveEventListener(inner, "Tap", handler, {});
    deliver(inner, inner, BUBBLE, "tap");
    expect(runs).toBe(1);
  });

  it("ignores a non-callable registration", () => {
    const { inner } = tree();
    __AddEventListener(inner, "tap", "handlerName", {});
    expect(mock.named("enableEventListener")).toEqual([]);
    expect(() => deliver(inner, inner, BUBBLE, "tap")).not.toThrow();
  });

  it("hands the callback an event carrying both identities and the detail", () => {
    const { outer, inner } = tree();
    __SetID(inner, "target-id");
    __SetID(outer, "current-id");
    /** @type {any} */
    let received;
    /** @type {any} */
    let currentTarget;
    __AddEventListener(outer, "tap", (/** @type {any} */ event) => {
      received = event;
      // Read here rather than after: the standard clears `currentTarget` when
      // the dispatch ends, so inside the listener is the only place it means
      // anything.
      currentTarget = event.currentTarget;
    }, {});

    deliver(outer, inner, BUBBLE, "tap", JSON.stringify({ x: 12, y: 30 }));

    expect(received.type).toBe("tap");
    expect(received.detail).toEqual({ x: 12, y: 30 });
    expect(received.target.uid).toBe(__GetElementUniqueID(inner));
    expect(currentTarget.uid).toBe(__GetElementUniqueID(outer));
    expect(received.target.id).toBe("target-id");
    expect(currentTarget.elementRefptr).toBe(outer);
  });

  it("reports the standard's at-target phase where the passes meet", () => {
    const { outer, inner } = tree();
    /** @type {number[]} */
    const phases = [];
    const record = (/** @type {any} */ event) => phases.push(event.eventPhase);
    __AddEventListener(inner, "tap", record, {});
    __AddEventListener(inner, "tap", record, { capture: true });
    __AddEventListener(outer, "tap", record, { capture: true });

    deliver(outer, inner, CAPTURE, "tap");
    deliver(inner, inner, CAPTURE, "tap");
    deliver(inner, inner, BUBBLE, "tap");

    // 1 capturing on the ancestor, 2 at the target in both passes.
    expect(phases).toEqual([CAPTURE, 2, 2]);
  });

  it("one event object serves the whole walk, so a listener can write to it", () => {
    const { page, outer, inner } = tree();
    /** @type {unknown[]} */
    const seen = [];
    __AddEventListener(page, "tap", (/** @type {any} */ event) => {
      event.marker = "from page";
      seen.push(event);
    }, { capture: true });
    __AddEventListener(inner, "tap", (/** @type {any} */ event) => {
      seen.push(event, event.marker);
    }, {});
    __AddEventListener(outer, "tap", (/** @type {any} */ event) => {
      seen.push(event);
    }, {});

    walk([
      { node: page, target: inner, phase: CAPTURE },
      { node: inner, target: inner, phase: BUBBLE },
      { node: outer, target: inner, phase: BUBBLE },
    ], "tap");

    const [first, second, marker, third] = seen;
    expect(second).toBe(first);
    expect(third).toBe(first);
    expect(marker).toBe("from page");
  });

  it("drops the event on the last call, so the next walk starts clean", () => {
    const { inner } = tree();
    /** @type {unknown[]} */
    const seen = [];
    __AddEventListener(inner, "tap", (/** @type {any} */ event) => {
      seen.push(event, event.marker);
      event.marker = "written";
    }, {});
    const uid = __GetElementUniqueID(inner);

    // The same id twice, which the host never does — that is the point. If
    // the last call did not drop the object, the second walk would find it.
    mock.event_listener_callback?.(uid, uid, BUBBLE, "tap", "", 66, true);
    mock.event_listener_callback?.(uid, uid, BUBBLE, "tap", "", 66, true);

    const [first, firstMarker, second, secondMarker] = seen;
    expect(firstMarker).toBeUndefined();
    expect(secondMarker).toBeUndefined();
    expect(second).not.toBe(first);
  });

  it("drops the event when a listener throws, since the host ends the walk", () => {
    const { inner } = tree();
    /** @type {unknown[]} */
    const seen = [];
    let shouldThrow = true;
    __AddEventListener(inner, "tap", (/** @type {any} */ event) => {
      seen.push(event.marker);
      event.marker = "written";
      if (shouldThrow) {
        throw new Error("listener failed");
      }
    }, {});
    const uid = __GetElementUniqueID(inner);

    // The host aborts on the throw, so no call ever carries `isLastCall` for
    // this id. Reusing the id is how a retained object would show itself.
    expect(() =>
      mock.event_listener_callback?.(uid, uid, BUBBLE, "tap", "", 77, false)
    ).toThrow("listener failed");
    shouldThrow = false;
    mock.event_listener_callback?.(uid, uid, BUBBLE, "tap", "", 77, true);

    expect(seen).toEqual([undefined, undefined]);
  });

  it("drops the event when a listener stops propagation mid-walk", () => {
    const { inner } = tree();
    /** @type {unknown[]} */
    const seen = [];
    __AddEventListener(inner, "tap", (/** @type {any} */ event) => {
      seen.push(event.marker);
      event.marker = "written";
      event.stopPropagation();
    }, {});
    const uid = __GetElementUniqueID(inner);

    mock.event_listener_callback?.(uid, uid, BUBBLE, "tap", "", 88, false);
    mock.event_listener_callback?.(uid, uid, BUBBLE, "tap", "", 88, true);

    expect(seen).toEqual([undefined, undefined]);
  });

  it("leaves a retained event reporting no current target once the walk ends", () => {
    const { page, inner } = tree();
    /** @type {any} */
    let retained;
    __AddEventListener(inner, "tap", (/** @type {any} */ event) => {
      retained = event;
    }, {});
    __AddEventListener(page, "tap", () => {}, {});

    walk([
      { node: inner, target: inner, phase: BUBBLE },
      { node: page, target: inner, phase: BUBBLE },
    ], "tap");

    // The standard's last dispatch step. Without it the object a listener kept
    // would still name `page`, the node the walk happened to stop on.
    expect(retained.currentTarget).toBeNull();
    expect(retained.eventPhase).toBe(0);
    // `target` outlives the walk, which the standard does not clear.
    expect(retained.target.uid).toBe(__GetElementUniqueID(inner));
  });

  it("keeps one target object across a walk, and swaps it only on retargeting", () => {
    const { page, outer, inner } = tree();
    /** @type {unknown[]} */
    const targets = [];
    for (const node of [inner, outer, page]) {
      __AddEventListener(node, "tap", (/** @type {any} */ event) => {
        targets.push(event.target);
      }, {});
    }

    walk([
      { node: inner, target: inner, phase: BUBBLE },
      { node: outer, target: inner, phase: BUBBLE },
      // What crossing a shadow boundary looks like from here: the host hands
      // the same walk a different target.
      { node: page, target: outer, phase: BUBBLE },
    ], "tap");

    expect(targets[1]).toBe(targets[0]);
    expect(targets[2]).not.toBe(targets[0]);
    expect(/** @type {any} */ (targets[2]).uid).toBe(
      __GetElementUniqueID(outer),
    );
  });

  it("ends the walk through the host when a listener stops propagation", () => {
    const { inner } = tree();
    __AddEventListener(inner, "tap", (/** @type {any} */ event) => event.stopPropagation(), {});

    deliver(inner, inner, BUBBLE, "tap");

    expect(mock.named("stopPropagation")).toHaveLength(1);
  });

  it("keeps stopImmediatePropagation inside this node, and still ends the walk", () => {
    const { inner } = tree();
    /** @type {string[]} */
    const order = [];
    __AddEventListener(inner, "tap", (/** @type {any} */ event) => {
      order.push("first");
      __StopImmediatePropagation(event);
    }, {});
    __AddEventListener(inner, "tap", () => order.push("second"), {});

    deliver(inner, inner, BUBBLE, "tap");

    expect(order).toEqual(["first"]);
    expect(mock.named("stopPropagation")).toHaveLength(1);
  });

  it("__StopPropagation does not skip the rest of this node", () => {
    const { inner } = tree();
    /** @type {string[]} */
    const order = [];
    __AddEventListener(inner, "tap", (/** @type {any} */ event) => {
      order.push("first");
      __StopPropagation(event);
    }, {});
    __AddEventListener(inner, "tap", () => order.push("second"), {});

    deliver(inner, inner, BUBBLE, "tap");

    expect(order).toEqual(["first", "second"]);
  });

  it("a listener added during dispatch does not run for this event", () => {
    const { inner } = tree();
    /** @type {string[]} */
    const order = [];
    __AddEventListener(inner, "tap", () => {
      order.push("first");
      __AddEventListener(inner, "tap", () => order.push("late"), {});
    }, {});

    deliver(inner, inner, BUBBLE, "tap");
    expect(order).toEqual(["first"]);

    deliver(inner, inner, BUBBLE, "tap");
    expect(order).toEqual(["first", "first", "late"]);
  });

  it("reports the standard's phase numbers, not the pass it was told", () => {
    const { outer, inner } = tree();
    /** @type {number[]} */
    const phases = [];
    const record = (/** @type {any} */ event) => phases.push(event.eventPhase);
    __AddEventListener(outer, "tap", record, { capture: true });
    __AddEventListener(inner, "tap", record, { capture: true });
    __AddEventListener(inner, "tap", record, {});
    __AddEventListener(outer, "tap", record, {});

    deliver(outer, inner, CAPTURE, "tap");
    deliver(inner, inner, CAPTURE, "tap");
    deliver(inner, inner, BUBBLE, "tap");
    deliver(outer, inner, BUBBLE, "tap");

    // CAPTURING_PHASE, AT_TARGET twice, BUBBLING_PHASE. The bubbling one is
    // the case the pass id gets wrong: `BUBBLE` is 0, which is `Event.NONE`.
    expect(phases).toEqual([1, 2, 2, 3]);
  });

  it("does not run a listener an earlier one removed", () => {
    const { inner } = tree();
    /** @type {string[]} */
    const order = [];
    const second = () => order.push("second");
    __AddEventListener(inner, "tap", () => {
      order.push("first");
      __RemoveEventListener(inner, "tap", second, {});
    }, {});
    __AddEventListener(inner, "tap", second, {});

    deliver(inner, inner, BUBBLE, "tap");

    expect(order).toEqual(["first"]);
  });

  it("delivers nothing for a node id no handle names", () => {
    tree();
    expect(() =>
      mock.event_listener_callback?.(9999, 9999, BUBBLE, "tap", "", 9000, true)
    ).not.toThrow();
  });

  it("listeners are scoped to their own element", () => {
    const { outer, inner } = tree();
    let runs = 0;
    __AddEventListener(inner, "tap", () => {
      runs += 1;
    }, {});

    deliver(outer, outer, BUBBLE, "tap");
    expect(runs).toBe(0);
    deliver(inner, inner, BUBBLE, "tap");
    expect(runs).toBe(1);
  });
});

describe("__AddEvent", () => {
  /**
   * A worklet handler over a plain callback, plus the `runWorklet` a card's
   * own worklet runtime would have installed to invoke it. This is the only
   * handler kind that runs in this realm.
   *
   * @param {(event: any) => void} body
   */
  function worklet(body) {
    globalThis.runWorklet = (value, params) => {
      /** @type {any} */ (value).body(params[0]);
    };
    return { type: "worklet", value: { body } };
  }

  it("files one handler per name and replaces whatever that name held", () => {
    const { inner } = tree();
    const first = "3:0:bindtap";
    const second = "3:1:catchtap";

    __AddEvent(inner, "bindEvent", "tap", first);
    __AddEvent(inner, "catchEvent", "tap", second);

    // The map is keyed by name alone, so the second call did not add a
    // registration beside the first: it took its place, type included.
    expect(__GetEvent(inner, "tap", "bindEvent")).toBeUndefined();
    expect(__GetEvent(inner, "tap", "catchEvent")).toBe(second);
    expect(__GetEvents(inner)).toEqual([
      { type: "catchevent", name: "tap", function: second },
    ]);
  });

  it("lowercases both halves, so a card reads back what it wrote", () => {
    const { inner } = tree();
    const handler = "3:0:bindtap";

    __AddEvent(inner, "bindEvent", "Tap", handler);

    expect(__GetEvent(inner, "TAP", "BINDEVENT")).toBe(handler);
    expect(__GetEvents(inner)).toEqual([
      { type: "bindevent", name: "tap", function: handler },
    ]);
  });

  it("removes on a nullish handler", () => {
    const { inner } = tree();
    const uid = __GetElementUniqueID(inner);
    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");

    __AddEvent(inner, "bindEvent", "tap", undefined);

    expect(__GetEvents(inner)).toEqual([]);
    expect(mock.named("disableEventListener")).toEqual([
      ["disableEventListener", uid, BUBBLE, "tap"],
    ]);
  });

  it("indexes the pass its type selects, and moves when the type moves", () => {
    const { inner } = tree();
    const uid = __GetElementUniqueID(inner);
    const handler = "3:0:bindtap";

    __AddEvent(inner, "bindEvent", "tap", handler);
    expect(mock.named("enableEventListener")).toEqual([
      ["enableEventListener", uid, BUBBLE, "tap"],
    ]);

    __AddEvent(inner, "capture-bind", "tap", handler);
    expect(mock.named("disableEventListener")).toEqual([
      ["disableEventListener", uid, BUBBLE, "tap"],
    ]);
    expect(mock.named("enableEventListener")).toEqual([
      ["enableEventListener", uid, BUBBLE, "tap"],
      ["enableEventListener", uid, CAPTURE, "tap"],
    ]);
  });

  it("shares the host index with __AddEventListener, and neither switches the other off", () => {
    const { inner } = tree();
    const uid = __GetElementUniqueID(inner);
    const callback = () => {};

    __AddEventListener(inner, "tap", callback, {});
    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");
    // One index entry covers both kinds, so the second registration says
    // nothing new.
    expect(mock.named("enableEventListener")).toEqual([
      ["enableEventListener", uid, BUBBLE, "tap"],
    ]);

    __RemoveEventListener(inner, "tap", callback, {});
    expect(mock.named("disableEventListener")).toEqual([]);

    __AddEvent(inner, "bindEvent", "tap", null);
    expect(mock.named("disableEventListener")).toEqual([
      ["disableEventListener", uid, BUBBLE, "tap"],
    ]);
  });

  it("ignores a callable outright, filing nothing and clearing nothing", () => {
    const { inner } = tree();
    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");
    mock.calls.length = 0;

    __AddEvent(inner, "bindEvent", "tap", () => {});

    // web-core's `__AddEvent` matches none of its branches on a callable, so
    // the call does nothing at all — including not clearing the name.
    expect(__GetEvent(inner, "tap", "bindEvent")).toBe("3:0:bindtap");
    expect(mock.calls).toEqual([]);
  });

  it("ignores a callable on an element that filed nothing", () => {
    const { inner } = tree();

    __AddEvent(inner, "bindEvent", "tap", () => {});

    expect(__GetEvents(inner)).toEqual([]);
    expect(mock.named("enableEventListener")).toEqual([]);
  });

  it("runs a worklet handler through the realm's own runWorklet", () => {
    const { inner } = tree();
    /** @type {unknown[]} */
    const runs = [];
    const value = { _wkltId: "abc" };
    // Read inside the call: the walk resets `currentTarget` when it ends, so
    // an event kept past it no longer names the node it was delivered to.
    globalThis.runWorklet = (worklet, params) => {
      const event = /** @type {any} */ (params[0]);
      runs.push(worklet, event.type, event.detail.x, event.currentTarget.uid);
    };
    __AddEvent(inner, "bindEvent", "tap", { type: "worklet", value });

    deliver(inner, inner, BUBBLE, "tap", JSON.stringify({ x: 12 }));

    // The worklet body reaches `runWorklet` unwrapped, as its `value`, with
    // the event as the single positional parameter.
    expect(runs).toEqual([value, "tap", 12, __GetElementUniqueID(inner)]);
  });

  it("does not fail a dispatch when no worklet runtime was installed", () => {
    const { inner } = tree();
    __AddEvent(inner, "bindEvent", "tap", { type: "worklet", value: {} });

    expect(() => deliver(inner, inner, BUBBLE, "tap")).not.toThrow();
  });

  it("files a background-thread handler name and never calls it", () => {
    const { inner } = tree();
    const uid = __GetElementUniqueID(inner);

    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");

    // Filed and reported, because a card may read it back — and indexed,
    // because the form still decides the walk.
    expect(__GetEvent(inner, "tap", "bindEvent")).toBe("3:0:bindtap");
    expect(mock.named("enableEventListener")).toEqual([
      ["enableEventListener", uid, BUBBLE, "tap"],
    ]);
    expect(() => deliver(inner, inner, BUBBLE, "tap")).not.toThrow();
  });

  it("ends the walk for a catch form whose handler cannot run here", () => {
    const { inner } = tree();
    __AddEvent(inner, "catchEvent", "tap", "3:0:catchtap");

    deliver(inner, inner, BUBBLE, "tap");

    // The form catches, not the handler: nothing ran and the walk still ends.
    expect(mock.named("stopPropagation")).toHaveLength(1);
  });

  it("ends the walk for a catch form after its handler ran", () => {
    const { inner } = tree();
    /** @type {string[]} */
    const order = [];
    __AddEvent(inner, "capture-catch", "tap", worklet(() => order.push("handler")));

    deliver(inner, inner, CAPTURE, "tap");

    expect(order).toEqual(["handler"]);
    expect(mock.named("stopPropagation")).toHaveLength(1);
  });

  it("runs before the __AddEventListener closures on the same node", () => {
    const { inner } = tree();
    /** @type {string[]} */
    const order = [];
    __AddEventListener(inner, "tap", () => order.push("closure"), {});
    __AddEvent(inner, "bindEvent", "tap", worklet(() => order.push("handler")));

    deliver(inner, inner, BUBBLE, "tap");

    expect(order).toEqual(["handler", "closure"]);
  });

  it("skips the closures when the handler stops immediate propagation", () => {
    const { inner } = tree();
    /** @type {string[]} */
    const order = [];
    __AddEventListener(inner, "tap", () => order.push("closure"), {});
    __AddEvent(
      inner,
      "bindEvent",
      "tap",
      worklet((/** @type {any} */ event) => {
        order.push("handler");
        event.stopImmediatePropagation();
      }),
    );

    deliver(inner, inner, BUBBLE, "tap");

    expect(order).toEqual(["handler"]);
  });

  it("is scoped to its own element and pass", () => {
    const { outer, inner } = tree();
    let runs = 0;
    __AddEvent(inner, "bindEvent", "tap", worklet(() => {
      runs += 1;
    }));

    deliver(outer, outer, BUBBLE, "tap");
    deliver(inner, inner, CAPTURE, "tap");
    expect(runs).toBe(0);

    deliver(inner, inner, BUBBLE, "tap");
    expect(runs).toBe(1);
  });

  it("files global-bindEvent apart, and never indexes it", () => {
    const { inner } = tree();
    let runs = 0;
    const handler = worklet(() => {
      runs += 1;
    });

    __AddEvent(inner, "global-bindEvent", "tap", handler);

    expect(__GetEvent(inner, "tap", "global-bindEvent")).toBe(handler);
    // Its own map: a global registration does not displace a path one.
    expect(__GetEvent(inner, "tap", "bindEvent")).toBeUndefined();
    // The host walks the event path and nothing else, so indexing it would
    // deliver a subset no reference implementation produces.
    expect(mock.named("enableEventListener")).toEqual([]);

    deliver(inner, inner, BUBBLE, "tap");
    expect(runs).toBe(0);
  });
});

describe("__GetEvents and __SetEvents", () => {
  it("reports nothing for an element that never filed a handler", () => {
    const { inner } = tree();
    expect(__GetEvents(inner)).toEqual([]);
    expect(__GetEvent(inner, "tap", "bindEvent")).toBeUndefined();
  });

  it("lists the path handlers before the global ones", () => {
    const { inner } = tree();
    __AddEvent(inner, "bindEvent", "tap", "a");
    __AddEvent(inner, "global-bindEvent", "scroll", "b");
    __AddEvent(inner, "capture-catch", "longpress", "c");

    expect(__GetEvents(inner)).toEqual([
      { type: "bindevent", name: "tap", function: "a" },
      { type: "capture-catch", name: "longpress", function: "c" },
      { type: "global-bindevent", name: "scroll", function: "b" },
    ]);
  });

  it("clears before it adds, so a name absent from the list is gone", () => {
    const { inner } = tree();
    const uid = __GetElementUniqueID(inner);
    __AddEvent(inner, "bindEvent", "tap", "old");

    __SetEvents(inner, [
      { type: "bindEvent", name: "longpress", function: "new" },
    ]);

    expect(__GetEvent(inner, "tap", "bindEvent")).toBeUndefined();
    expect(__GetEvent(inner, "longpress", "bindEvent")).toBe("new");
    expect(mock.named("disableEventListener")).toEqual([
      ["disableEventListener", uid, BUBBLE, "tap"],
    ]);
  });

  it("round-trips what __GetEvents reported", () => {
    const { inner, outer } = tree();
    __AddEvent(inner, "capture-bind", "tap", "a");
    __AddEvent(inner, "global-bindEvent", "scroll", "b");

    __SetEvents(outer, __GetEvents(inner));

    expect(__GetEvents(outer)).toEqual(__GetEvents(inner));
  });

  it("skips an entry that names no event", () => {
    const { inner } = tree();
    __SetEvents(inner, [
      { type: "bindEvent", function: "a" },
      { name: "tap", function: "b" },
      { type: 3, name: 4, function: "c" },
      { type: "bindEvent", name: "tap", function: "d" },
    ]);

    expect(__GetEvents(inner)).toEqual([
      { type: "bindevent", name: "tap", function: "d" },
    ]);
  });

  it("clears and stops when handed no list at all", () => {
    const { inner } = tree();
    __AddEvent(inner, "bindEvent", "tap", "old");

    __SetEvents(inner, undefined);

    expect(__GetEvents(inner)).toEqual([]);
  });
});

describe("list callbacks", () => {
  it("files a list's callbacks without telling the host anything", () => {
    const list = __CreateList(0, () => 0, () => {}, {}, () => []);
    mock.calls.length = 0;

    expect(__UpdateListCallbacks(list, () => 1, () => {}, () => [])).toBe(
      undefined,
    );
    expect(__UpdateListCallbacks(list, null, null, null)).toBe(undefined);

    // Storage only: their consumer needs the child at an index, which the
    // native boundary cannot answer.
    expect(mock.calls).toEqual([]);
  });

  it("still refuses update-list-info, naming what is missing", () => {
    const list = __CreateList(0, () => 0, () => {});
    __UpdateListCallbacks(list, () => 0, () => {}, () => []);

    expect(() =>
      __SetAttribute(list, "update-list-info", {
        insertAction: [],
        removeAction: [],
      })
    ).toThrow("indexed child access");
  });
});
