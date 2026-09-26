// Behavior tests for the Element PAPI runtime over a recording native mock.
//
// These pin the semantics that live in element-papi.ts: the PAPI surface and
// arities, tag vocabulary, handle-to-NodeId mapping, return identity, and
// drop bookkeeping. The mock mirrors the real boundary's shape: it returns
// sequential node ids and rejects non-number ids the way the native number
// extraction does. Structural behavior and the collection-driven release path
// run against the real native side in
// crates/bobcat-core/tests/main_thread.rs.

import { beforeEach, describe, expect, it, rstest } from "@rstest/core";
import type * as elementPapi from "../src/element-papi.ts";
import * as record from "../src/record.ts";

// The reader of every record the mock answers with, as the realm has it.
rstest.mockRequire("bobcat:record", () => record);

rstest.mockRequire("bobcat-internal:host", () => {
  const native = globalThis.__bobcatTestHost;
  if (native === undefined) {
    throw new Error("the Element PAPI test native host is not installed");
  }
  return {
    createDocument: native.createDocument,
    createPage: native.createPage,
    createElement: native.createElement,
    setAttribute: native.setAttribute,
    setInlineStyles: native.setInlineStyles,
    setInlineStyleProperty: native.setInlineStyleProperty,
    supportsStyleProperty: native.supportsStyleProperty,
    queryElementIds: native.queryElementIds,
    removeAttribute: native.removeAttribute,
    getAttribute: native.getAttribute,
    tagName: native.tagName,
    attributeNames: native.attributeNames,
    callElementMethod: native.callElementMethod,
    getComputedStyleMap: native.getComputedStyleMap,
    childElementIds: native.childElementIds,
    parentNode: native.parentNode,
    insertBefore: native.insertBefore,
    removeElement: native.removeElement,
    replaceElement: native.replaceElement,
    swapElement: native.swapElement,
    dropElement: native.dropElement,
    flushElementTree: native.flushElementTree,
    listenerNameOpened: native.listenerNameOpened,
    listenerNameClosed: native.listenerNameClosed,
    setTimer: native.setTimer,
    clearTimer: native.clearTimer,
  };
});

rstest.mockRequire("bobcat:runtime", () => ({
  __BobcatPublishEvent(
    componentId: string | undefined,
    handlerName: string,
    event: Record<string, unknown>,
  ) {
    const native =
      globalThis.__bobcatTestHost as ReturnType<typeof createMockBobcat>;
    // `structuredClone` stands in for the Worker transport, which is what
    // takes the copy now that element-papi hands over the values themselves.
    native.calls.push([
      "publishEvent",
      componentId,
      handlerName,
      structuredClone(event),
    ]);
  },
}));

/**
 * What the mock's second `createDocument` throws, so a test can assert that
 * the host's refusal reaches the caller by identity — the native member
 * refuses a second document, and this file adds no check of its own.
 */
const DOCUMENT_REFUSAL = new Error("the realm already created its document");

/**
 * The page configuration a boot module is written with and hands to the
 * constructor. Its four switches are the host's; nothing in this file reads
 * them, and the constructor's only job is to pass them on, in order.
 */
const PAGE_CONFIG: elementPapi.PageConfig = {
  defaultDisplayLinear: true,
  defaultOverflowVisible: false,
  enableCssSelector: false,
  enableJSDataProcessor: true,
};

/**
 * Every native member, plus the recorded calls, a filter over them, and the
 * selector answer a test installs.
 *
 * The runtime captures the module's bindings at import, so a test cannot
 * replace `queryElementIds` itself; `answerQuery` is the hook the recorded
 * member delegates to, read at call time. Its default refuses, because the
 * selector engine is the real DOM's and lives in
 * crates/bobcat-core/src/main/runtime/tests.rs.
 */
type MockBobcat = BobcatNative & {
  calls: unknown[][];
  named: (name: string) => unknown[][];
  answerQuery: (
    root: number,
    selector: string,
    firstOnly: 0 | 1,
    includeRoot: 0 | 1,
  ) => string;
  /**
   * The UI-method and computed-style answers, scripted the same way: both
   * members read geometry and style out of a document this file does not
   * have, so a test states the answer and the member hands it over. The
   * defaults are the two empty ones — a method the engine does not have, and
   * an element that has not been through a flush.
   */
  answerElementMethod: (nodeId: number, method: string) => string | null;
  answerComputedStyle: (
    nodeId: number,
    properties: string,
    resolved: 0 | 1,
  ) => string;
};

/**
 * @param issuedIds Ids the native half hands out, in call order.
 *   Defaults to the real boundary's shape (2, 3, 4, ...); a test that needs to
 *   prove the runtime carries native's number through rather than numbering
 *   handles itself passes a sequence no counter would produce.
 */
function createMockBobcat(issuedIds?: number[]): MockBobcat {
  const calls: unknown[][] = [];
  const named = (name: string) => calls.filter((call) => call[0] === name);
  const nodeId = (name: string, value: unknown): number => {
    if (typeof value !== "number") {
      throw new TypeError(`${name} expects a number`);
    }
    return value;
  };
  // Mirrors the real boundary: the DOM reserves id 0, takes 1 for the
  // document node, and hands the page 2, so the first created element is 3.
  let nextNodeId = 3;
  let issued = 0;
  const issueNodeId = (): number => {
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
  const parents: Map<number, number> = new Map();
  // The real boundary keeps its own child order and reports element children
  // in it, so a mock that only knew parent links could not stand in for the
  // one member whose whole contract is that order.
  const childOrder: Map<number, number[]> = new Map();
  const siblingsOf = (parent: number): number[] => {
    let list = childOrder.get(parent);
    if (list === undefined) {
      list = [];
      childOrder.set(parent, list);
    }
    return list;
  };
  const unlink = (child: number) => {
    const parent = parents.get(child);
    if (parent === undefined) {
      return;
    }
    const list = siblingsOf(parent);
    const at = list.indexOf(child);
    if (at !== -1) {
      list.splice(at, 1);
    }
  };
  const positionOf = (node: number): number => {
    const parent = parents.get(node);
    return parent === undefined ? -1 : siblingsOf(parent).indexOf(node);
  };
  const attributes: Map<number, Map<string, string>> = new Map();
  const tags: Map<number, string> = new Map([[2, "page"]]);
  // One document per realm for the life of the realm, as the native slot
  // enforces it: the ingredients a construction spends are never restored.
  let documentSpent = false;

  const host: MockBobcat = {
    calls,
    named,
    // Arguments are recorded rather than ignored: a test asserts the four
    // switches arrive as booleans, in `PageConfig`'s order.
    createDocument: (...args: unknown[]) => {
      calls.push(["createDocument", ...args]);
      if (documentSpent) {
        throw DOCUMENT_REFUSAL;
      }
      documentSpent = true;
    },
    createPage: () => {
      calls.push(["createPage"]);
      return 2;
    },
    createElement: (tag: string) => {
      const node = issueNodeId();
      tags.set(node, tag);
      calls.push(["createElement", tag]);
      return node;
    },
    setAttribute: (node: unknown, name: string, value: string) => {
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
    setInlineStyleProperty: (node: unknown, name: string, value: string) => {
      calls.push(["setInlineStyleProperty", nodeId("setInlineStyleProperty", node), name, value]);
    },
    supportsStyleProperty: (name: string) => name === "background-color" || name === "width",
    answerQuery: () => {
      throw new Error("selectors are tested against the real DOM");
    },
    queryElementIds: (
      root: unknown,
      selector: unknown,
      firstOnly: unknown,
      includeRoot: unknown,
    ) => {
      calls.push([
        "queryElementIds",
        nodeId("queryElementIds", root),
        selector,
        firstOnly,
        includeRoot,
      ]);
      return host.answerQuery(
        root as number,
        selector as string,
        firstOnly as 0 | 1,
        includeRoot as 0 | 1,
      );
    },
    /**
     * Decodes the record payload the way the native side does, so the
     * expectations below read as declarations rather than as wire text — and
     * so a length that disagreed with its field would surface here.
     */
    setInlineStyles: (node: unknown, record: string) => {
      const id = nodeId("setInlineStyles", node);
      if (typeof record !== "string") {
        throw new TypeError("setInlineStyles expects a string record");
      }
      const declarations: [string, string][] = [];
      let fields: string[] = [];
      let rest = record;
      while (rest.length > 0) {
        const separator = rest.indexOf(":");
        if (separator < 0) {
          throw new TypeError("setInlineStyles received a malformed record");
        }
        const units = Number(rest.slice(0, separator));
        if (!Number.isInteger(units) || units < 0) {
          throw new TypeError("setInlineStyles received a malformed length");
        }
        const body = rest.slice(separator + 1);
        if (body.length < units) {
          throw new TypeError("setInlineStyles received a truncated field");
        }
        fields.push(body.slice(0, units));
        rest = body.slice(units);
        const [name, value] = fields;
        if (name !== undefined && value !== undefined) {
          declarations.push([name, value]);
          fields = [];
        }
      }
      if (fields.length !== 0) {
        throw new TypeError("setInlineStyles received an odd field count");
      }
      calls.push(["setInlineStyles", id, declarations]);
    },
    removeAttribute: (node: unknown, name: string) => {
      const id = nodeId("removeAttribute", node);
      attributes.get(id)?.delete(name);
      calls.push(["removeAttribute", id, name]);
    },
    getAttribute: (node: unknown, name: string) => {
      const id = nodeId("getAttribute", node);
      calls.push(["getAttribute", id, name]);
      return attributes.get(id)?.get(name) ?? null;
    },
    tagName: (node: unknown) => {
      const id = nodeId("tagName", node);
      calls.push(["tagName", id]);
      const tag = tags.get(id);
      if (tag === undefined) {
        throw new Error(`tagName: ${id} is not a live element`);
      }
      return tag;
    },
    answerElementMethod: () => null,
    callElementMethod: (node: unknown, method: unknown) => {
      const id = nodeId("callElementMethod", node);
      calls.push(["callElementMethod", id, method]);
      return host.answerElementMethod(id, method as string);
    },
    answerComputedStyle: () => "",
    getComputedStyleMap: (
      node: unknown,
      properties: unknown,
      resolved: unknown,
    ) => {
      const id = nodeId("getComputedStyleMap", node);
      calls.push(["getComputedStyleMap", id, properties, resolved]);
      return host.answerComputedStyle(
        id,
        properties as string,
        resolved as 0 | 1,
      );
    },
    attributeNames: (node: unknown) => {
      const id = nodeId("attributeNames", node);
      calls.push(["attributeNames", id]);
      let record = "";
      for (const name of attributes.get(id)?.keys() ?? []) {
        record += `${name.length}:${name}`;
      }
      return record;
    },
    childElementIds: (node: unknown) => {
      const id = nodeId("childElementIds", node);
      calls.push(["childElementIds", id]);
      // No filtering: this mock has no node kind but the element, which is
      // exactly why the text node a `raw-text` reflects cannot be pinned
      // here — crates/bobcat-core/tests/main_thread.rs covers that.
      return (childOrder.get(id) ?? []).join(",");
    },
    parentNode: (node: unknown) => {
      const id = nodeId("parentNode", node);
      calls.push(["parentNode", id]);
      return parents.get(id) ?? null;
    },
    insertBefore: (parent: unknown, child: unknown, reference: unknown) => {
      const parentId = nodeId("insertBefore", parent);
      const childId = nodeId("insertBefore", child);
      unlink(childId);
      parents.set(childId, parentId);
      const list = siblingsOf(parentId);
      const at = reference === null
        ? -1
        : list.indexOf(nodeId("insertBefore", reference));
      if (at === -1) {
        list.push(childId);
      } else {
        list.splice(at, 0, childId);
      }
      calls.push([
        "insertBefore",
        parentId,
        childId,
        reference === null ? null : nodeId("insertBefore", reference),
      ]);
    },
    removeElement: (child: unknown) => {
      const childId = nodeId("removeElement", child);
      unlink(childId);
      parents.delete(childId);
      calls.push(["removeElement", childId]);
    },
    replaceElement: (newElement: unknown, oldElement: unknown) => {
      const newId = nodeId("replaceElement", newElement);
      const oldId = nodeId("replaceElement", oldElement);
      const parent = parents.get(oldId);
      if (parent !== undefined) {
        const at = positionOf(oldId);
        unlink(newId);
        unlink(oldId);
        parents.set(newId, parent);
        parents.delete(oldId);
        siblingsOf(parent).splice(at, 0, newId);
      }
      calls.push(["replaceElement", newId, oldId]);
    },
    swapElement: (childA: unknown, childB: unknown) => {
      const a = nodeId("swapElement", childA);
      const b = nodeId("swapElement", childB);
      const parentA = parents.get(a);
      const parentB = parents.get(b);
      const positionA = positionOf(a);
      const positionB = positionOf(b);
      if (parentA !== undefined) {
        parents.set(b, parentA);
        siblingsOf(parentA)[positionA] = b;
      } else {
        parents.delete(b);
      }
      if (parentB !== undefined) {
        parents.set(a, parentB);
        siblingsOf(parentB)[positionB] = a;
      } else {
        parents.delete(a);
      }
      calls.push(["swapElement", a, b]);
    },
    dropElement: (node: unknown) => {
      calls.push(["dropElement", nodeId("dropElement", node)]);
    },
    flushElementTree: () => {
      calls.push(["flushElementTree"]);
    },
    listenerNameOpened: (eventName: unknown) => {
      calls.push(["listenerNameOpened", eventName]);
    },
    listenerNameClosed: (eventName: unknown) => {
      calls.push(["listenerNameClosed", eventName]);
    },
    // The Element PAPI reaches none of these; they are here because the
    // mock stands in for the whole native module, not part of it.
    setTimer: (delayMilliseconds: number, repeats: boolean) => {
      calls.push(["setTimer", delayMilliseconds, repeats]);
      return 1;
    },
    clearTimer: (id: number) => {
      calls.push(["clearTimer", id]);
    },
    initData: () => undefined,
    globalProps: () => undefined,
    nativeModuleTable: () => "",
  };
  return host;
}

let mock: ReturnType<typeof createMockBobcat>;
let elementModule: typeof elementPapi;

beforeEach(async () => {
  rstest.resetModules();
  mock = createMockBobcat();
  globalThis.__bobcatTestHost = mock;
  Reflect.deleteProperty(globalThis, "bobcat");
  // Installed by a card's own worklet runtime, never by this file; a test
  // that wants one puts it here itself.
  globalThis.runWorklet = undefined;
  elementModule = await import("../src/element-papi.ts");
  // The rest of this legacy-shaped behavior suite calls PAPI names directly;
  // expose this test instance without making global installation a module
  // responsibility.
  Object.assign(globalThis, elementModule);
});

describe("installation", () => {
  it("exports every PAPI binding with the arity its reference declares", () => {
    const arities: [string, number][] = [
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
      ["__GetChildren", 1],
      ["__GetAttributeByName", 2],
      ["__GetAttributeNames", 1],
      ["__GetElementUniqueID", 1],
      ["__SetInlineStyles", 2],
      ["__AddInlineStyle", 3],
      ["__SetDataset", 2],
      ["__GetDataset", 1],
      ["__AddDataset", 3],
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
      ["__GetPageElement", 0],
      ["__QuerySelector", 3],
      ["__QuerySelectorAll", 3],
      ["__InvokeUIMethod", 4],
      ["__GetComputedStyleByKey", 2],
      ["__FlushElementTree", 0],
    ];
    for (const [name, arity] of arities) {
      const papi = (elementModule as Record<string, unknown>)[name];
      expect(papi, name).toBeTypeOf("function");
      expect((papi as Function).length, name).toBe(arity);
    }
    expect(Object.keys(elementModule).sort()).toEqual(
      [
        ...arities.map(([name]) => name),
        "__BobcatQueryNodes",
        "__BobcatDispatchEvent",
        // Neither is a PAPI member: the computed-style map is the Typed OM
        // readback the realm reaches by name, and the value class is the
        // type its entries carry.
        "__BobcatComputedStyleMap",
        "CSSStyleValue",
        // Not a PAPI member: the lifecycle export the boot module
        // constructs, which no entry preamble imports.
        "Document",
      ].sort(),
    );
    // The six every event carries; the detail numbers are a rest parameter,
    // which `length` does not count.
    expect(elementModule.__BobcatDispatchEvent).toHaveLength(6);
  });

  it("creates the realm's document once, over the config it is given", () => {
    void new elementModule.Document(PAGE_CONFIG);
    expect(mock.named("createDocument")).toEqual([
      // `PageConfig`'s order: display, overflow, selectors, processor.
      ["createDocument", true, false, false, true],
    ]);
  });

  it("tags a document the way the standard's own exotic objects are tagged", () => {
    expect(
      Object.prototype.toString.call(new elementModule.Document(PAGE_CONFIG)),
    ).toBe("[object Document]");
  });

  it("lets the host refuse a second document rather than refusing it here", () => {
    const first = new elementModule.Document(PAGE_CONFIG);
    let thrown: unknown;
    try {
      void new elementModule.Document(PAGE_CONFIG);
    } catch (error) {
      thrown = error;
    }
    // By identity: the refusal is the host's, carried out of the constructor
    // untouched rather than re-thrown or replaced by a check of this file's.
    expect(thrown).toBe(DOCUMENT_REFUSAL);
    expect(mock.named("createDocument")).toHaveLength(2);
    expect(first).toBeInstanceOf(elementModule.Document);
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

  it("does not publish the native host on globalThis", () => {
    expect("bobcat" in globalThis).toBe(false);
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

  it("follows the old child to whichever parent last took it", () => {
    const page = __CreatePage("card", 0);
    const wrapper = __CreateView(0);
    const moved = __CreateView(0);
    const replacement = __CreateView(0);
    __AppendElement(page, wrapper);
    // Appended to the page first, then moved: the second append is what the
    // ownership graph has to end up recording.
    __AppendElement(page, moved);
    __AppendElement(wrapper, moved);
    mock.calls.length = 0;

    __ReplaceElements(page, replacement, moved);
    expect(mock.calls).toEqual([
      ["parentNode", 4],
      ["insertBefore", 3, 5, 4],
      ["removeElement", 4],
    ]);
  });

  it("treats a child whose parent let go of it as detached", () => {
    const page = __CreatePage("card", 0);
    const removed = __CreateView(0);
    const replacement = __CreateView(0);
    __AppendElement(page, removed);
    __RemoveElement(page, removed);
    mock.calls.length = 0;

    __ReplaceElements(page, replacement, removed);
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

  it("leaves both operands naming the parent the swap gave them", () => {
    const page = __CreatePage("card", 0);
    const wrapper = __CreateView(0);
    const a = __CreateView(0);
    const inner = __CreateView(0);
    const replacement = __CreateView(0);
    __AppendElement(page, wrapper);
    __AppendElement(page, a);
    __AppendElement(wrapper, inner);
    // A cross-parent swap: `a` lands under the wrapper, `inner` under the
    // page. Both parents have to be what the graph names afterwards.
    __SwapElement(a, inner);
    mock.calls.length = 0;

    __ReplaceElements(page, replacement, a);
    expect(mock.calls).toEqual([
      ["parentNode", 4],
      ["insertBefore", 3, 6, 4],
      ["removeElement", 4],
    ]);
    mock.calls.length = 0;

    // And `a`, detached by that replace, is the operand that moves.
    __SwapElement(a, inner);
    expect(mock.calls).toEqual([
      ["parentNode", 4],
      ["parentNode", 5],
      ["replaceElement", 4, 5],
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

describe("__GetAttributeByName", () => {
  it("reads one attribute back, and null for one never set", () => {
    const view = __CreateView(0);
    expect(__GetAttributeByName(view, "role")).toBe(null);
    __SetAttribute(view, "role", "button");
    expect(__GetAttributeByName(view, "role")).toBe("button");
  });

  it("names the attribute by its string form, as __SetAttribute does", () => {
    const view = __CreateView(0);
    __SetAttribute(view, 7, "seven");
    expect(__GetAttributeByName(view, 7)).toBe("seven");
    expect(__GetAttributeByName(view, "7")).toBe("seven");
  });

  it("agrees with __GetID on the id attribute", () => {
    const view = __CreateView(0);
    __SetID(view, "header");
    expect(__GetAttributeByName(view, "id")).toBe(__GetID(view));
  });
});

describe("__GetAttributeNames", () => {
  it("is empty for an element carrying nothing", () => {
    expect(__GetAttributeNames(__CreateView(0))).toEqual([]);
  });

  it("reports every name once, in the order the element acquired them", () => {
    const view = __CreateView(0);
    __SetAttribute(view, "role", "button");
    __SetAttribute(view, "aria-label", "Add one");
    __SetAttribute(view, "role", "link");
    expect(__GetAttributeNames(view)).toEqual(["role", "aria-label"]);
  });

  it("carries a name through whatever it contains", () => {
    const view = __CreateView(0);
    // The length prefix is the whole reason the record can hold these: a
    // delimiter, and an astral character whose UTF-16 length is not its
    // code-point count.
    __SetAttribute(view, "a:b,c", "1");
    __SetAttribute(view, "d\u{1F600}e", "2");
    expect(__GetAttributeNames(view)).toEqual(["a:b,c", "d\u{1F600}e"]);
  });
});

describe("__GetChildren", () => {
  it("is empty for an element with no children", () => {
    expect(__GetChildren(__CreateView(0))).toEqual([]);
  });

  it("returns the same handles the constructors did, in tree order", () => {
    const parent = __CreateView(0);
    const first = __CreateView(0);
    const second = __CreateText(0);
    __AppendElement(parent, first);
    __AppendElement(parent, second);
    expect(__GetChildren(parent)).toEqual([first, second]);
  });

  it("follows an insert before an existing child", () => {
    const parent = __CreateView(0);
    const first = __CreateView(0);
    const second = __CreateView(0);
    const middle = __CreateView(0);
    __AppendElement(parent, first);
    __AppendElement(parent, second);
    __InsertElementBefore(parent, middle, second);
    expect(__GetChildren(parent)).toEqual([first, middle, second]);
  });

  it("follows a removal and a reparent", () => {
    const parent = __CreateView(0);
    const other = __CreateView(0);
    const moved = __CreateView(0);
    const staying = __CreateView(0);
    __AppendElement(parent, moved);
    __AppendElement(parent, staying);
    __RemoveElement(parent, moved);
    expect(__GetChildren(parent)).toEqual([staying]);
    __AppendElement(other, moved);
    expect(__GetChildren(other)).toEqual([moved]);
  });

  it("follows a swap of two siblings", () => {
    const parent = __CreateView(0);
    const a = __CreateView(0);
    const b = __CreateView(0);
    __AppendElement(parent, a);
    __AppendElement(parent, b);
    __SwapElement(a, b);
    expect(__GetChildren(parent)).toEqual([b, a]);
  });

  it("reports the tree, not the order handles were adopted", () => {
    // Adoption order and tree order diverge here: `late` is adopted last and
    // placed first. Reading the JavaScript-side child set instead of the
    // native one would answer [early, late].
    const parent = __CreateView(0);
    const early = __CreateView(0);
    const late = __CreateView(0);
    __AppendElement(parent, early);
    __InsertElementBefore(parent, late, early);
    expect(__GetChildren(parent)).toEqual([late, early]);
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
    globalThis.__bobcatTestHost = createMockBobcat(issued);
    const isolatedModule = await import("../src/element-papi.ts");

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

  it("sends a record as one ordered batch", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetInlineStyles(view, {
      backgroundColor: "red",
      borderTopLeftRadius: 4,
      color: undefined,
      width: null,
    });
    expect(mock.calls).toEqual([
      ["setInlineStyles", 3, [
        ["background-color", "red"],
        ["border-top-left-radius", "4"],
      ]],
    ]);
  });

  it("resets the complete declaration block before applying a record", () => {
    const view = __CreateView(0);
    __SetInlineStyles(view, "width:10px;color:red");
    mock.calls.length = 0;

    __SetInlineStyles(view, { height: "20px" });
    expect(mock.calls).toEqual([
      ["setInlineStyles", 3, [["height", "20px"]]],
    ]);
  });

  it("preserves the case-sensitive name of a custom property", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetInlineStyles(view, { "--accentColor": "tomato" });
    expect(mock.calls).toEqual([
      ["setInlineStyles", 3, [["--accentColor", "tomato"]]],
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
      ["setInlineStyles", 3, [["definitely-not-a-property", "value"]]],
    ]);
  });

  it("keeps an empty style attribute for an empty or all-nullish record", () => {
    const view = __CreateView(0);
    for (const styles of [{}, { color: null, width: undefined }]) {
      mock.calls.length = 0;
      __SetInlineStyles(view, styles);
      expect(mock.calls).toEqual([["setInlineStyles", 3, []]]);
    }
  });

  it("carries values containing delimiters and non-BMP text intact", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetInlineStyles(view, {
      fontFamily: 'a;b:c", 3:x',
      content: "🦀:1",
      "--empty": "",
    });
    expect(mock.calls).toEqual([
      ["setInlineStyles", 3, [
        ["font-family", 'a;b:c", 3:x'],
        ["content", "🦀:1"],
        ["--empty", ""],
      ]],
    ]);
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
  it("keeps native dataset values, merges keys and returns independent copies", () => {
    const view = __CreateView(0);
    const input = {count:7, nested:{value:1}, nil:null};
    elementModule.__SetDataset(view, input);
    input.nested.value = 2;
    elementModule.__SetDataset(view, {next:undefined});
    const result = elementModule.__GetDataset(view) as typeof input;
    expect(result).toEqual({count:7, nested:{value:1}, nil:null, next:undefined});
    expect(Object.hasOwn(result, 'next')).toBe(true);
    result.nested.value = 3;
    expect((elementModule.__GetDataset(view) as typeof input).nested.value).toBe(1);
    elementModule.__AddDataset(view, 'count', false);
    expect(elementModule.__GetDataset(view)["count"]).toBe(false);
  });
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

  it("does not impose the cross-thread value domain on attribute assignment", () => {
    const view = __CreateView(0);
    const cyclic: {self: unknown} = {self:null};
    cyclic.self = cyclic;
    const values = [7n, Symbol('local'), cyclic, {nested: {callback() {}}}];
    for (const value of values) {
      __SetAttribute(view, 'value', value);
      expect(elementModule.__GetAttributeByName(view, 'value')).toBe(String(value));
    }
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

  it("routes update-list-info to the list protocol instead of the element", async () => {
    const list = __CreateList(0, () => {}, () => {});
    mock.calls.length = 0;

    // Not an attribute at all: nothing is written onto the element, and an
    // empty batch reaches the host with nothing to do.
    expect(
      __SetAttribute(list, "update-list-info", {
        insertAction: [],
        removeAction: [],
      }),
    ).toBe(undefined);
    expect(mock.calls).toEqual([]);

    await drain();
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

/**
 * Dispatches one event the way the host does: one call carrying the whole
 * path as two comma-joined id strings, target-first and root-last, and every
 * other fact as a number.
 *
 * `targets` is that step's own target — the same node for every step unless a
 * test is exercising shadow retargeting — so it defaults to the path's first
 * entry, the node the event happened at. The payload defaults to the origin
 * with no wheel delta and no touch points, which is what a test that is not
 * about the payload wants.
 *
 * `bubbles` is the event's own, and the default is what every routed Lynx
 * event is: the two that are not are an `<image>`'s `load` and `error`.
 */
function dispatch(
  path: object[],
  name: string,
  payload: DispatchPayload = {},
  targets?: object[],
  bubbles: boolean = true,
) {
  const nodes = path.map((handle) => __GetElementUniqueID(handle));
  const targeted = targets === undefined
    ? nodes.map(() => nodes[0])
    : targets.map((handle) => __GetElementUniqueID(handle));
  elementModule.__BobcatDispatchEvent(
    nodes.join(","),
    targeted.join(","),
    name,
    bubbles,
    payload.timestamp ?? 0,
    ...detailArguments(payload),
  );
}

/** The host's detail kinds, mirrored from `main/runtime/lib.rs`. */
const DETAIL_POSITION = 0;
const DETAIL_SIZE = 1;
const DETAIL_EMPTY = 2;

/**
 * What a test asks one dispatch's `detail` to be made of.
 *
 * A position detail unless it names otherwise, because that is what every
 * routed input event carries and what a test that is not about the detail
 * wants.
 */
interface DispatchPayload {
  x?: number;
  y?: number;
  deltaX?: number;
  deltaY?: number;
  touchNumbers?: number[];
  timestamp?: number;
  /** An image `load`'s intrinsic size: the `DETAIL_SIZE` kind. */
  width?: number;
  height?: number;
  /** An image `error`: the `DETAIL_EMPTY` kind, which spends no numbers. */
  empty?: boolean;
}

/**
 * The kind and the numbers behind it, as the host sends them. A tuple, so the
 * spread at the call site still fills the `detailKind` parameter.
 */
function detailArguments(payload: DispatchPayload): [number, ...unknown[]] {
  if (payload.empty === true) {
    return [DETAIL_EMPTY];
  }
  if (payload.width !== undefined) {
    return [DETAIL_SIZE, payload.width, payload.height];
  }
  return [
    DETAIL_POSITION,
    payload.x ?? 0,
    payload.y ?? 0,
    payload.deltaX,
    payload.deltaY,
    ...(payload.touchNumbers ?? []),
  ];
}

/** The standard's `Event.eventPhase` values. */
const CAPTURING_PHASE = 1;
const AT_TARGET = 2;
const BUBBLING_PHASE = 3;

/** An event's `target` and `currentTarget` descriptor. */
interface TargetInfo {
  dataset: Record<string, unknown>;
  id: string | null;
  uid: number;
  elementRefptr: object;
}

/**
 * The event object as a listener sees it during delivery, typed as these
 * tests read and write it: the fields the runtime sets, the `marker` a test
 * writes to see one object serve a whole dispatch, and the `detail` the
 * runtime builds from the numbers the host sent.
 */
interface ListenerEvent {
  type: string;
  eventPhase: number;
  target: TargetInfo;
  currentTarget: TargetInfo;
  detail: { x: number; y: number; deltaX?: number; deltaY?: number };
  marker?: string;
  stopPropagation(): void;
  stopImmediatePropagation(): void;
}

describe("event listeners", () => {
  it("opens the name with the host once, however many handles want it", () => {
    const { outer, inner } = tree();

    __AddEventListener(inner, "tap", () => {}, {});
    __AddEventListener(inner, "tap", () => {}, {});
    __AddEventListener(outer, "tap", () => {}, {});

    // The host keeps no per-element index: all it hears is that the name
    // went from wanted by nobody to wanted by somebody.
    expect(mock.named("listenerNameOpened")).toEqual([
      ["listenerNameOpened", "tap"],
    ]);
  });

  it("closes the name when the last handle anywhere gives it up", () => {
    const { outer, inner } = tree();
    const a = () => {};
    const b = () => {};
    __AddEventListener(inner, "tap", a, {});
    __AddEventListener(inner, "tap", b, {});
    __AddEventListener(outer, "tap", a, {});

    __RemoveEventListener(inner, "tap", a, {});
    expect(mock.named("listenerNameClosed")).toEqual([]);
    __RemoveEventListener(inner, "tap", b, {});
    expect(mock.named("listenerNameClosed")).toEqual([]);

    __RemoveEventListener(outer, "tap", a, {});
    expect(mock.named("listenerNameClosed")).toEqual([
      ["listenerNameClosed", "tap"],
    ]);
  });

  it("counts a handle once per name, whichever passes it registers in", () => {
    const { inner } = tree();
    const handler = () => {};

    __AddEventListener(inner, "tap", handler, {});
    __AddEventListener(inner, "tap", handler, { capture: true });

    expect(mock.named("listenerNameOpened")).toEqual([
      ["listenerNameOpened", "tap"],
    ]);

    // Capture is part of the identity, so a bubble removal leaves the capture
    // registration — and that one still wants the name.
    __RemoveEventListener(inner, "tap", handler, {});
    expect(mock.named("listenerNameClosed")).toEqual([]);

    __RemoveEventListener(inner, "tap", handler, { capture: true });
    expect(mock.named("listenerNameClosed")).toEqual([
      ["listenerNameClosed", "tap"],
    ]);
  });

  it("runs the capture pass before the bubble one, each in registration order", () => {
    const { inner } = tree();
    const order: string[] = [];
    __AddEventListener(inner, "tap", () => order.push("bubble-1"), {});
    __AddEventListener(inner, "tap", () => order.push("bubble-2"), {});
    __AddEventListener(inner, "tap", () => order.push("capture"), {
      capture: true,
    });

    dispatch([inner], "tap");

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

    dispatch([inner], "tap");
    dispatch([inner], "tap");

    expect(runs).toBe(2);
  });

  it("removes a once listener before running it", () => {
    const { inner } = tree();
    let runs = 0;
    __AddEventListener(inner, "tap", () => {
      runs += 1;
    }, { once: true });

    dispatch([inner], "tap");
    dispatch([inner], "tap");

    expect(runs).toBe(1);
    expect(mock.named("listenerNameClosed")).toEqual([
      ["listenerNameClosed", "tap"],
    ]);
  });

  it("matches the event name case-insensitively on both sides", () => {
    const { inner } = tree();
    let runs = 0;
    const handler = () => {
      runs += 1;
    };
    __AddEventListener(inner, "TAP", handler, {});
    dispatch([inner], "tap");
    expect(runs).toBe(1);

    __RemoveEventListener(inner, "Tap", handler, {});
    dispatch([inner], "tap");
    expect(runs).toBe(1);
  });

  it("ignores a non-callable registration", () => {
    const { inner } = tree();
    __AddEventListener(inner, "tap", "handlerName", {});
    expect(mock.named("listenerNameOpened")).toEqual([]);
    expect(() => dispatch([inner], "tap")).not.toThrow();
  });

  it("hands the callback an event carrying both identities and the detail", () => {
    const { outer, inner } = tree();
    __SetID(inner, "target-id");
    __SetID(outer, "current-id");
    let received!: ListenerEvent;
    let currentTarget!: TargetInfo;
    __AddEventListener(outer, "tap", (event: ListenerEvent) => {
      received = event;
      // Read here rather than after: the standard clears `currentTarget` when
      // the dispatch ends, so inside the listener is the only place it means
      // anything.
      currentTarget = event.currentTarget;
    }, {});

    dispatch([inner, outer], "tap", { x: 12, y: 30 });

    expect(received.type).toBe("tap");
    expect(received.detail).toEqual({ x: 12, y: 30 });
    expect(received.target.uid).toBe(__GetElementUniqueID(inner));
    expect(currentTarget.uid).toBe(__GetElementUniqueID(outer));
    expect(received.target.id).toBe("target-id");
    expect(currentTarget.elementRefptr).toBe(outer);
  });

  it("reports the standard's phase numbers for every step it visits", () => {
    const { outer, inner } = tree();
    const phases: number[] = [];
    const record = (event: ListenerEvent) => phases.push(event.eventPhase);
    __AddEventListener(outer, "tap", record, { capture: true });
    __AddEventListener(inner, "tap", record, { capture: true });
    __AddEventListener(inner, "tap", record, {});
    __AddEventListener(outer, "tap", record, {});

    dispatch([inner, outer], "tap");

    // Capturing at the ancestor, at-target in both passes on the target
    // itself — a step whose target is itself is at-target whichever pass
    // reached it — then bubbling back out.
    expect(phases).toEqual([
      CAPTURING_PHASE,
      AT_TARGET,
      AT_TARGET,
      BUBBLING_PHASE,
    ]);
  });

  it("skips a step that registered nothing, and one whose handle is gone", () => {
    const { page, outer, inner } = tree();
    const order: string[] = [];
    __AddEventListener(page, "tap", () => order.push("page"), {});
    __AddEventListener(inner, "tap", () => order.push("inner"), {});

    // `outer` is on the path and registered nothing; the id past it names no
    // handle at all, which is what a step whose handle was collected before
    // its element was freed looks like from here.
    elementModule.__BobcatDispatchEvent(
      `${__GetElementUniqueID(inner)},${__GetElementUniqueID(outer)},9999,${
        __GetElementUniqueID(page)
      }`,
      new Array(4).fill(__GetElementUniqueID(inner)).join(","),
      "tap",
      true,
      0,
      DETAIL_POSITION,
      0,
      0,
      undefined,
      undefined,
    );

    expect(order).toEqual(["inner", "page"]);
  });

  it("one event object serves the whole dispatch, so a listener can write to it", () => {
    const { page, outer, inner } = tree();
    const seen: unknown[] = [];
    __AddEventListener(page, "tap", (event: ListenerEvent) => {
      event.marker = "from page";
      seen.push(event);
    }, { capture: true });
    __AddEventListener(inner, "tap", (event: ListenerEvent) => {
      seen.push(event, event.marker);
    }, {});
    __AddEventListener(outer, "tap", (event: ListenerEvent) => {
      seen.push(event);
    }, {});

    dispatch([inner, outer, page], "tap");

    const [first, second, marker, third] = seen;
    expect(second).toBe(first);
    expect(third).toBe(first);
    expect(marker).toBe("from page");
  });

  it("mints one event per dispatch, so the next one starts clean", () => {
    const { inner } = tree();
    const seen: unknown[] = [];
    __AddEventListener(inner, "tap", (event: ListenerEvent) => {
      seen.push(event, event.marker);
      event.marker = "written";
    }, {});

    dispatch([inner], "tap");
    dispatch([inner], "tap");

    const [first, firstMarker, second, secondMarker] = seen;
    expect(firstMarker).toBeUndefined();
    expect(secondMarker).toBeUndefined();
    expect(second).not.toBe(first);
  });

  it("ends the dispatch even when a listener throws its way out of it", () => {
    const { outer, inner } = tree();
    let retained!: ListenerEvent;
    __AddEventListener(inner, "tap", (event: ListenerEvent) => {
      retained = event;
      throw new Error("listener failed");
    }, {});
    let ancestorRan = false;
    __AddEventListener(outer, "tap", () => {
      ancestorRan = true;
    }, {});

    // The host reports it as a failed listener; the walk does not resume.
    expect(() => dispatch([inner, outer], "tap")).toThrow("listener failed");

    expect(ancestorRan).toBe(false);
    // The standard's last dispatch step still ran, so the object the
    // listener kept does not go on naming the node it threw at.
    expect(retained.currentTarget).toBeNull();
    expect(retained.eventPhase).toBe(0);
  });

  it("refuses a target no handle names instead of delivering the event", () => {
    const { inner } = tree();
    const seen: unknown[] = [];
    __AddEventListener(inner, "tap", (event: ListenerEvent) => {
      seen.push(event.target.uid);
    }, {});
    const uid = __GetElementUniqueID(inner);

    // An id no handle was ever minted for. A connected element always has
    // one — its parent's handle holds it — so this is the ownership graph
    // and the tree disagreeing, and it is reported rather than swallowed.
    expect(() =>
      elementModule.__BobcatDispatchEvent(
        String(uid),
        "999",
        "tap",
        true,
        0,
        DETAIL_POSITION,
        0,
        0,
        undefined,
        undefined,
      )
    ).toThrow("ownership graph");
    expect(seen).toEqual([]);

    // And it left nothing behind: the next dispatch is a fresh one.
    dispatch([inner], "tap");
    expect(seen).toEqual([uid]);
  });

  it("leaves a retained event reporting no current target once it ends", () => {
    const { page, inner } = tree();
    let retained!: ListenerEvent;
    __AddEventListener(inner, "tap", (event: ListenerEvent) => {
      retained = event;
    }, {});
    __AddEventListener(page, "tap", () => {}, {});

    dispatch([inner, page], "tap");

    // The standard's last dispatch step. Without it the object a listener kept
    // would still name `page`, the node the walk happened to stop on.
    expect(retained.currentTarget).toBeNull();
    expect(retained.eventPhase).toBe(0);
    // `target` outlives the dispatch, which the standard does not clear.
    expect(retained.target.uid).toBe(__GetElementUniqueID(inner));
  });

  it("keeps one target object across a walk, and swaps it only on retargeting", () => {
    const { page, outer, inner } = tree();
    const targets: unknown[] = [];
    for (const node of [inner, outer, page]) {
      __AddEventListener(node, "tap", (event: ListenerEvent) => {
        targets.push(event.target);
      }, {});
    }

    // What crossing a shadow boundary looks like from here: the step above it
    // is told a different target than the steps below.
    dispatch([inner, outer, page], "tap", {}, [inner, inner, outer]);

    expect(targets[1]).toBe(targets[0]);
    expect(targets[2]).not.toBe(targets[0]);
    expect((targets[2] as TargetInfo).uid).toBe(
      __GetElementUniqueID(outer),
    );
  });

  it("ends the remaining steps when a listener stops propagation", () => {
    const { outer, inner } = tree();
    const order: string[] = [];
    __AddEventListener(inner, "tap", (event: ListenerEvent) => {
      order.push("inner");
      event.stopPropagation();
    }, {});
    __AddEventListener(outer, "tap", () => order.push("outer"), {});

    dispatch([inner, outer], "tap");

    // Nothing about the stop crosses the boundary: the walk it ends is here.
    expect(order).toEqual(["inner"]);
    expect(mock.calls.filter(([name]) => String(name).startsWith("listenerName")))
      .toHaveLength(1);
  });

  it("stops a capture-pass listener from reaching the bubble pass at all", () => {
    const { outer, inner } = tree();
    const order: string[] = [];
    __AddEventListener(outer, "tap", (event: ListenerEvent) => {
      order.push("outer-capture");
      __StopPropagation(event);
    }, { capture: true });
    __AddEventListener(inner, "tap", () => order.push("inner-bubble"), {});

    dispatch([inner, outer], "tap");

    expect(order).toEqual(["outer-capture"]);
  });

  it("keeps stopImmediatePropagation inside this node, and still ends the walk", () => {
    const { outer, inner } = tree();
    const order: string[] = [];
    __AddEventListener(inner, "tap", (event: ListenerEvent) => {
      order.push("first");
      __StopImmediatePropagation(event);
    }, {});
    __AddEventListener(inner, "tap", () => order.push("second"), {});
    __AddEventListener(outer, "tap", () => order.push("outer"), {});

    dispatch([inner, outer], "tap");

    expect(order).toEqual(["first"]);
  });

  it("__StopPropagation does not skip the rest of this node", () => {
    const { outer, inner } = tree();
    const order: string[] = [];
    __AddEventListener(inner, "tap", (event: ListenerEvent) => {
      order.push("first");
      __StopPropagation(event);
    }, {});
    __AddEventListener(inner, "tap", () => order.push("second"), {});
    __AddEventListener(outer, "tap", () => order.push("outer"), {});

    dispatch([inner, outer], "tap");

    expect(order).toEqual(["first", "second"]);
  });

  it("a listener added during dispatch does not run for this event", () => {
    const { inner } = tree();
    const order: string[] = [];
    __AddEventListener(inner, "tap", () => {
      order.push("first");
      __AddEventListener(inner, "tap", () => order.push("late"), {});
    }, {});

    dispatch([inner], "tap");
    expect(order).toEqual(["first"]);

    dispatch([inner], "tap");
    expect(order).toEqual(["first", "first", "late"]);
  });

  it("does not run a listener an earlier one removed", () => {
    const { inner } = tree();
    const order: string[] = [];
    const second = () => order.push("second");
    __AddEventListener(inner, "tap", () => {
      order.push("first");
      __RemoveEventListener(inner, "tap", second, {});
    }, {});
    __AddEventListener(inner, "tap", second, {});

    dispatch([inner], "tap");

    expect(order).toEqual(["first"]);
  });

  it("listeners are scoped to their own element", () => {
    const { outer, inner } = tree();
    let runs = 0;
    __AddEventListener(inner, "tap", () => {
      runs += 1;
    }, {});

    // A path that does not visit `inner` never reaches its listeners.
    dispatch([outer], "tap");
    expect(runs).toBe(0);
    dispatch([inner, outer], "tap");
    expect(runs).toBe(1);
  });

  // web-core's `common_event_handler` (event_apis.rs:413-432) narrows exactly
  // two of the three passes for a non-bubbling event: the capture pass runs
  // over the whole path either way, the bind pass over the target alone, and
  // the `global-bindEvent` pass not at all. An `<image>`'s `load` and `error`
  // are the events that arrive this way — web-core mints both with
  // `bubbles: false` (`commonEventInitConfiguration.ts`).
  it("captures down the whole path for a non-bubbling event", () => {
    const { page, outer, inner } = tree();
    const order: string[] = [];
    for (const [label, handle] of [
      ["page", page],
      ["outer", outer],
      ["inner", inner],
    ] as const) {
      __AddEventListener(handle, "load", () => order.push(label), {
        capture: true,
      });
    }

    const size = { width: 40, height: 20 };
    dispatch([inner, outer, page], "load", size, undefined, false);

    expect(order).toEqual(["page", "outer", "inner"]);
  });

  it("binds on the target alone for a non-bubbling event", () => {
    const { page, outer, inner } = tree();
    const order: string[] = [];
    for (const [label, handle] of [
      ["inner", inner],
      ["outer", outer],
      ["page", page],
    ] as const) {
      __AddEventListener(handle, "load", () => order.push(label), {});
      // The other registration form files on the same pass, and narrows with
      // it: a ReactLynx `bindload` is one of these, not a closure.
      __AddEvent(handle, "bindEvent", "load", `${label}:load`);
    }

    const size = { width: 40, height: 20 };
    dispatch([inner, outer, page], "load", size, undefined, false);

    expect(order).toEqual(["inner"]);
    expect(mock.named("publishEvent").map((call) => call[2])).toEqual([
      "inner:load",
    ]);
  });

  it("runs no global-bindEvent pass for a non-bubbling event", () => {
    const { page, outer, inner } = tree();
    __AddEvent(outer, "global-bindEvent", "load", "outer:global");

    const size = { width: 40, height: 20 };
    dispatch([inner, outer, page], "load", size, undefined, false);
    expect(mock.named("publishEvent")).toEqual([]);

    // The same registration reached by a bubbling event of the same name:
    // what the flag suppresses is the pass, not the registration.
    dispatch([inner, outer, page], "load");
    expect(mock.named("publishEvent").map((call) => call[2])).toEqual([
      "outer:global",
    ]);
  });

  it("keeps a retargeted at-target step in a non-bubbling bind pass", () => {
    const { page, outer, inner } = tree();
    const order: string[] = [];
    __AddEventListener(inner, "load", () => order.push("inner"), {});
    __AddEventListener(outer, "load", () => order.push("outer"), {});
    __AddEventListener(page, "load", () => order.push("page"), {});

    // `outer` stands in for `inner` above a shadow boundary: its step is its
    // own target, which is what makes it at-target in both passes. `page`
    // sees `outer` and is not.
    const size = { width: 40, height: 20 };
    dispatch([inner, outer, page], "load", size, [inner, outer, outer], false);

    expect(order).toEqual(["inner", "outer"]);
  });

  it("closes the names a collected handle held, and prunes its global registration", async () => {
    // The registry the module captures at import, replaced with one this test
    // can drive: a collection is what calls the cleanup, and no test can
    // schedule one.
    rstest.resetModules();
    const registered: { held: unknown }[] = [];
    let cleanup!: (held: never) => void;
    const real = globalThis.FinalizationRegistry;
    globalThis.FinalizationRegistry = class {
      constructor(callback: (held: never) => void) {
        cleanup = callback;
      }
      register(_target: object, held: unknown) {
        registered.push({ held });
      }
      unregister() {}
    } as unknown as typeof FinalizationRegistry;
    let papi: typeof elementPapi;
    try {
      papi = await import("../src/element-papi.ts");
    } finally {
      globalThis.FinalizationRegistry = real;
    }

    const page = papi.__CreatePage("card", 0);
    const view = papi.__CreateView(0);
    papi.__AppendElement(page, view);
    papi.__AddEventListener(view, "tap", () => {}, {});
    papi.__AddEvent(view, "global-bindEvent", "swipe", "3:0:swipe");
    expect(mock.named("listenerNameOpened")).toEqual([
      ["listenerNameOpened", "tap"],
      ["listenerNameOpened", "swipe"],
    ]);
    const held = registered.at(-1)?.held;

    // What a collection does: the cleanup gets the record the handle left,
    // which is the only place its registrations are still named.
    cleanup(held as never);

    expect(mock.named("listenerNameClosed")).toEqual([
      ["listenerNameClosed", "tap"],
      ["listenerNameClosed", "swipe"],
    ]);
    expect(mock.named("dropElement")).toEqual([
      ["dropElement", papi.__GetElementUniqueID(view)],
    ]);
  });
});

describe("__AddEvent", () => {
  /**
   * A worklet handler over a plain callback, plus the `runWorklet` a card's
   * own worklet runtime would have installed to invoke it. This is the only
   * handler kind that runs in this realm.
   */
  function worklet(body: (event: ListenerEvent) => void) {
    globalThis.runWorklet = (value, params) => {
      (value as { body: (event: unknown) => void }).body(params[0]);
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

  it("removes on a nullish handler, closing the name it held open", () => {
    const { inner } = tree();
    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");

    __AddEvent(inner, "bindEvent", "tap", undefined);

    expect(__GetEvents(inner)).toEqual([]);
    expect(mock.named("listenerNameClosed")).toEqual([
      ["listenerNameClosed", "tap"],
    ]);
  });

  it("delivers in the pass its type selects, and moves when the type moves", () => {
    const { outer, inner } = tree();
    const order: string[] = [];
    __AddEvent(inner, "bindEvent", "tap", worklet(() => order.push("inner")));
    __AddEvent(outer, "capture-bind", "tap", worklet(() => order.push("outer")));

    dispatch([inner, outer], "tap");
    expect(order).toEqual(["outer", "inner"]);

    // The same name, the other form: the entry moves to the bubble pass, and
    // the host hears nothing, because the name it holds open is the same one.
    order.length = 0;
    __AddEvent(outer, "bindEvent", "tap", worklet(() => order.push("outer")));

    dispatch([inner, outer], "tap");
    expect(order).toEqual(["inner", "outer"]);
    expect(mock.named("listenerNameOpened")).toEqual([
      ["listenerNameOpened", "tap"],
    ]);
    expect(mock.named("listenerNameClosed")).toEqual([]);
  });

  it("shares the name count with __AddEventListener, and neither closes it for the other", () => {
    const { inner } = tree();
    const callback = () => {};

    __AddEventListener(inner, "tap", callback, {});
    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");
    // One count per handle and name, so the second registration says nothing
    // new.
    expect(mock.named("listenerNameOpened")).toEqual([
      ["listenerNameOpened", "tap"],
    ]);

    __RemoveEventListener(inner, "tap", callback, {});
    expect(mock.named("listenerNameClosed")).toEqual([]);

    __AddEvent(inner, "bindEvent", "tap", null);
    expect(mock.named("listenerNameClosed")).toEqual([
      ["listenerNameClosed", "tap"],
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
    expect(mock.named("listenerNameOpened")).toEqual([]);
  });

  it("runs a worklet handler through the realm's own runWorklet", () => {
    const { inner } = tree();
    const runs: unknown[] = [];
    const value = { _wkltId: "abc" };
    // Read inside the call: the dispatch resets `currentTarget` when it ends,
    // so an event kept past it no longer names the node it was delivered to.
    globalThis.runWorklet = (worklet, params) => {
      const event = params[0] as ListenerEvent;
      runs.push(worklet, event.type, event.detail.x, event.currentTarget.uid);
    };
    __AddEvent(inner, "bindEvent", "tap", { type: "worklet", value });

    dispatch([inner], "tap", { x: 12, y: 30 });

    // The worklet body reaches `runWorklet` unwrapped, as its `value`, with
    // the event as the single positional parameter.
    expect(runs).toEqual([value, "tap", 12, __GetElementUniqueID(inner)]);
  });

  it("does not fail a dispatch when no worklet runtime was installed", () => {
    const { inner } = tree();
    __AddEvent(inner, "bindEvent", "tap", { type: "worklet", value: {} });

    expect(() => dispatch([inner], "tap")).not.toThrow();
  });

  it("publishes an opaque background-thread handler name with its event", () => {
    const { inner } = tree();
    const uid = __GetElementUniqueID(inner);

    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");

    expect(__GetEvent(inner, "tap", "bindEvent")).toBe("3:0:bindtap");
    expect(mock.named("listenerNameOpened")).toEqual([
      ["listenerNameOpened", "tap"],
    ]);
    dispatch([inner], "tap", { x: 12, y: 30 });

    const target = { dataset: {}, id: null, uid };
    expect(mock.named("publishEvent")).toEqual([
      ["publishEvent", undefined, "3:0:bindtap", {
        type: "tap",
        eventPhase: 2,
        target,
        currentTarget: target,
        detail: { x: 12, y: 30 },
        timestamp: 0,
        params: {},
      }],
    ]);
  });

  it("publishes an empty string handler without treating it as removal", () => {
    const { inner } = tree();
    __AddEvent(inner, "bindEvent", "tap", "");

    dispatch([inner], "tap");

    expect(__GetEvent(inner, "tap", "bindEvent")).toBe("");
    expect(mock.named("publishEvent")).toHaveLength(1);
    expect(mock.named("publishEvent")[0]?.[2]).toBe("");
  });

  it("snapshots targets, datasets and detail before the local walk mutates them", () => {
    const { outer, inner } = tree();
    const outerUid = __GetElementUniqueID(outer);
    const innerUid = __GetElementUniqueID(inner);
    __SetID(inner, "button");
    __SetAttribute(inner, "data-item-name", "before");
    __SetAttribute(inner, "data-count", "2");
    __SetAttribute(outer, "data-section", "actions");
    __AddEvent(inner, "bindEvent", "tap", "inner:tap");
    __AddEvent(outer, "bindEvent", "tap", "outer:tap");
    let retained!: ListenerEvent;
    __AddEventListener(inner, "tap", (event: ListenerEvent) => {
      retained = event;
      event.detail.x = 99;
      __SetAttribute(inner, "data-item-name", "after");
    }, {});

    dispatch([inner, outer], "tap", { x: 12, y: 30 });

    expect(retained.currentTarget).toBeNull();
    expect(retained.target.elementRefptr).toBe(inner);
    expect(mock.named("publishEvent")).toEqual([
      ["publishEvent", undefined, "inner:tap", {
        type: "tap",
        eventPhase: 2,
        target: {
          dataset: { itemName: "before", count: "2" },
          id: "button",
          uid: innerUid,
        },
        currentTarget: {
          dataset: { itemName: "before", count: "2" },
          id: "button",
          uid: innerUid,
        },
        detail: { x: 12, y: 30 },
        timestamp: 0,
        params: {},
      }],
      ["publishEvent", undefined, "outer:tap", {
        type: "tap",
        eventPhase: 3,
        target: {
          dataset: { itemName: "after", count: "2" },
          id: "button",
          uid: innerUid,
        },
        currentTarget: {
          dataset: { section: "actions" },
          id: null,
          uid: outerUid,
        },
        detail: { x: 99, y: 30 },
        timestamp: 0,
        params: {},
      }],
    ]);
  });

  it("stops a catch form before publishing and then runs its local closures", () => {
    const { outer, inner } = tree();
    __AddEventListener(inner, "tap", () => mock.calls.push(["closure"]), {});
    __AddEvent(inner, "catchEvent", "tap", "3:0:catchtap");
    __AddEvent(outer, "bindEvent", "tap", "3:0:bindtap");

    dispatch([inner, outer], "tap");

    // The catch stopped the walk before its own handler was published, and
    // everything on its own node still ran; the ancestor never did.
    expect(
      mock.calls
        .filter(([name]) => ["publishEvent", "closure"].includes(String(name)))
        .map(([name, , handler]) => name === "closure" ? name : handler),
    ).toEqual(["3:0:catchtap", "closure"]);
  });

  it("ends the walk for a catch form after its handler ran", () => {
    const { outer, inner } = tree();
    const order: string[] = [];
    __AddEvent(inner, "capture-catch", "tap", worklet(() => order.push("handler")));
    __AddEvent(outer, "bindEvent", "tap", worklet(() => order.push("outer")));

    dispatch([inner, outer], "tap");

    // The capture pass reached the target and stopped there, so the bubble
    // pass never ran at all.
    expect(order).toEqual(["handler"]);
  });

  it("runs before the __AddEventListener closures on the same node", () => {
    const { inner } = tree();
    const order: string[] = [];
    __AddEventListener(inner, "tap", () => order.push("closure"), {});
    __AddEvent(inner, "bindEvent", "tap", worklet(() => order.push("handler")));

    dispatch([inner], "tap");

    expect(order).toEqual(["handler", "closure"]);
  });

  it("skips the closures when the handler stops immediate propagation", () => {
    const { inner } = tree();
    const order: string[] = [];
    __AddEventListener(inner, "tap", () => order.push("closure"), {});
    __AddEvent(
      inner,
      "bindEvent",
      "tap",
      worklet((event: ListenerEvent) => {
        order.push("handler");
        event.stopImmediatePropagation();
      }),
    );

    dispatch([inner], "tap");

    expect(order).toEqual(["handler"]);
  });

  it("is scoped to its own element", () => {
    const { outer, inner } = tree();
    let runs = 0;
    __AddEvent(inner, "bindEvent", "tap", worklet(() => {
      runs += 1;
    }));

    dispatch([outer], "tap");
    expect(runs).toBe(0);

    dispatch([inner, outer], "tap");
    expect(runs).toBe(1);
  });

  it("files a string and a worklet for one name side by side, and runs both", () => {
    const { inner } = tree();
    const order: string[] = [];
    globalThis.runWorklet = (value) => {
      order.push("worklet");
      void value;
    };

    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");
    __AddEvent(inner, "bindEvent", "tap", { type: "worklet", value: {} });

    // Two maps, one per kind: neither filing displaced the other.
    expect(__GetEvent(inner, "tap", "bindEvent")).toBe("3:0:bindtap");
    expect(__GetEvents(inner)).toEqual([
      { type: "bindevent", name: "tap", function: "3:0:bindtap" },
      {
        type: "bindevent",
        name: "tap",
        function: { type: "worklet", value: {} },
      },
    ]);
    // One name count covers both kinds.
    expect(mock.named("listenerNameOpened")).toEqual([
      ["listenerNameOpened", "tap"],
    ]);

    __AddEventListener(inner, "tap", () => order.push("closure"), {});
    dispatch([inner], "tap");

    // web-core's per-node order: the cross-thread handler, then the worklet,
    // then the `__AddEventListener` closures.
    expect(
      mock.calls
        .filter(([name]) => name === "publishEvent")
        .map(() => "publish"),
    ).toEqual(["publish"]);
    expect(order).toEqual(["worklet", "closure"]);
  });

  it("files the two kinds under different forms without either clearing the other", () => {
    const { inner } = tree();
    const order: string[] = [];
    const handler = worklet(() => order.push("worklet"));

    __AddEvent(inner, "capture-bind", "tap", handler);
    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");

    // Each kind keeps its own form, so this node is delivered to in both
    // passes — and holds the one name open once.
    expect(__GetEvent(inner, "tap", "bindEvent")).toBe("3:0:bindtap");
    expect(__GetEvents(inner)).toEqual([
      { type: "bindevent", name: "tap", function: "3:0:bindtap" },
      { type: "capture-bind", name: "tap", function: handler },
    ]);
    expect(mock.named("listenerNameOpened")).toEqual([
      ["listenerNameOpened", "tap"],
    ]);

    dispatch([inner], "tap");

    expect(order).toEqual(["worklet"]);
    expect(mock.named("publishEvent")).toHaveLength(1);
  });

  it("__GetEvent answers the string kind only", () => {
    const { inner } = tree();
    const handler = { type: "worklet", value: {} };

    __AddEvent(inner, "bindEvent", "tap", handler);

    // web-core's `get_event` reads its cross-thread map and never its
    // worklet one, so a name carrying only a worklet answers nothing.
    expect(__GetEvent(inner, "tap", "bindEvent")).toBeUndefined();
    expect(__GetEvents(inner)).toEqual([
      { type: "bindevent", name: "tap", function: handler },
    ]);
  });

  it("clears both kinds on a nullish handler, and closes the name", () => {
    const { inner } = tree();
    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");
    __AddEvent(inner, "bindEvent", "tap", { type: "worklet", value: {} });

    __AddEvent(inner, "bindEvent", "tap", null);

    expect(__GetEvents(inner)).toEqual([]);
    expect(mock.named("listenerNameClosed")).toEqual([
      ["listenerNameClosed", "tap"],
    ]);
  });

  it("ends the walk when either kind carries the catch form", () => {
    const { outer, inner } = tree();
    const order: string[] = [];

    // The catch is the worklet's form; the string beside it is a plain bind.
    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");
    __AddEvent(inner, "catchEvent", "tap", worklet(() => order.push("worklet")));
    __AddEvent(outer, "bindEvent", "tap", "3:0:outer");

    dispatch([inner, outer], "tap");

    // Both of the target's own kinds ran — the form decides the stop, not the
    // handler — and the ancestor's did not.
    expect(order).toEqual(["worklet"]);
    expect(mock.named("publishEvent").map((call) => call[2])).toEqual([
      "3:0:bindtap",
    ]);
  });

  it("files global-bindEvent apart, and opens the name from its own slot", () => {
    const { inner } = tree();
    let runs = 0;
    const handler = worklet(() => {
      runs += 1;
    });

    __AddEvent(inner, "global-bindEvent", "tap", handler);

    expect(__GetEvent(inner, "tap", "global-bindEvent")).toBeUndefined();
    // Its own slot: a global registration does not displace a path one.
    expect(__GetEvent(inner, "tap", "bindEvent")).toBeUndefined();
    expect(__GetEvents(inner)).toEqual([
      { type: "global-bindevent", name: "tap", function: handler },
    ]);
    // A global registration alone is enough to make the painting side route
    // the name, because the event it wants took no particular path.
    expect(mock.named("listenerNameOpened")).toEqual([
      ["listenerNameOpened", "tap"],
    ]);

    // Not a step of the path, and not filtered by one either: the delivery
    // happens whether or not the registered element is on it, and exactly
    // once per dispatch.
    dispatch([inner], "tap");
    expect(runs).toBe(1);
  });

  it("closes the name when the last global handler goes", () => {
    const { inner } = tree();
    __AddEvent(inner, "global-bindEvent", "tap", "3:0:globaltap");
    __AddEvent(inner, "global-bindEvent", "tap", { type: "worklet", value: {} });
    expect(mock.named("listenerNameOpened")).toEqual([
      ["listenerNameOpened", "tap"],
    ]);

    __AddEvent(inner, "global-bindEvent", "tap", undefined);

    expect(__GetEvents(inner)).toEqual([]);
    expect(mock.named("listenerNameClosed")).toEqual([
      ["listenerNameClosed", "tap"],
    ]);

    // And nothing is delivered for it afterwards.
    let runs = 0;
    globalThis.runWorklet = () => {
      runs += 1;
    };
    dispatch([inner], "tap");
    expect(runs).toBe(0);
  });

  it("runs both global handler kinds with no phase and its own currentTarget", () => {
    const { outer, inner } = tree();
    const seen: { uid: number; phase: number }[] = [];
    globalThis.runWorklet = (_value, params) => {
      const event = params[0] as ListenerEvent;
      seen.push({ uid: event.currentTarget.uid, phase: event.eventPhase });
    };
    __AddEvent(outer, "global-bindEvent", "tap", "outer:global");
    __AddEvent(outer, "global-bindEvent", "tap", {
      type: "worklet",
      value: {},
    });

    // The registered node is not the path: a global delivery is not a step
    // of the path the event took, and here `outer` is not even on it.
    dispatch([inner], "tap", { x: 1, y: 2 });

    const published = mock.named("publishEvent");
    expect(published).toHaveLength(1);
    expect(published[0]?.[2]).toBe("outer:global");
    const event = published[0]?.[3] as {
      eventPhase: number;
      target: { uid: number };
      currentTarget: { uid: number };
    };
    expect(event.eventPhase).toBe(0);
    expect(event.target.uid).toBe(__GetElementUniqueID(inner));
    expect(event.currentTarget.uid).toBe(__GetElementUniqueID(outer));
    // The string first, then the worklet, as on the path.
    expect(seen).toEqual([
      { uid: __GetElementUniqueID(outer), phase: 0 },
    ]);
  });

  it("runs the global pass after a catch ended the walk over the path", () => {
    const { page, outer, inner } = tree();
    const order: string[] = [];
    __AddEvent(inner, "catchEvent", "tap", worklet(() => order.push("catch")));
    __AddEvent(outer, "bindEvent", "tap", worklet(() => order.push("outer")));
    __AddEvent(page, "global-bindEvent", "tap", worklet(() => order.push("global")));

    dispatch([inner, outer, page], "tap");

    // web-core's `common_event_handler` calls `dispatch_global_bind_event`
    // unconditionally: the catch ends the path, not the pass after it.
    expect(order).toEqual(["catch", "global"]);
  });

  it("delivers global registrations in registration order, re-filing in place", () => {
    const { page, outer, inner } = tree();
    const order: string[] = [];
    __AddEvent(outer, "global-bindEvent", "tap", worklet(() => order.push("outer")));
    __AddEvent(page, "global-bindEvent", "tap", worklet(() => order.push("page")));
    // Re-filing a handler on an element already registered must not move it
    // behind the ones that followed it.
    __AddEvent(outer, "global-bindEvent", "tap", worklet(() => order.push("outer")));

    dispatch([inner], "tap");

    expect(order).toEqual(["outer", "page"]);
  });

  it("carries the dataset on both descriptors a worklet and a closure see", () => {
    const { inner } = tree();
    __SetAttribute(inner, "data-item-name", "row");
    __SetDataset(inner, { typed: 7 });
    const seen: unknown[] = [];
    __AddEvent(
      inner,
      "bindEvent",
      "tap",
      worklet((event: ListenerEvent) => {
        seen.push(event.target.dataset, event.currentTarget.dataset);
      }),
    );
    __AddEventListener(inner, "tap", (event: ListenerEvent) => {
      seen.push(event.target.dataset, event.currentTarget.dataset);
    }, {});

    dispatch([inner], "tap");

    // web-core's `generateTargetObject` gives every descriptor a `dataset`,
    // the `data-*` attributes camelCased with the typed values merged over.
    const dataset = { itemName: "row", typed: 7 };
    expect(seen).toEqual([dataset, dataset, dataset, dataset]);
  });
});

describe("the event detail", () => {
  /** Delivers one event and returns the `detail` its listener was handed. */
  function detailOf(
    element: object,
    name: string,
    payload: Parameters<typeof dispatch>[2],
  ): Record<string, unknown> {
    let seen: Record<string, unknown> | undefined;
    __AddEventListener(element, name, (event: ListenerEvent) => {
      seen = event.detail as unknown as Record<string, unknown>;
    }, {});
    dispatch([element], name, payload);
    expect(seen, "the listener ran").toBeDefined();
    return seen as Record<string, unknown>;
  }

  it("reports the position alone for an event with no wheel delta", () => {
    const { inner } = tree();

    const detail = detailOf(inner, "tap", { x: 12, y: 30 });

    // Exactly two keys: the two delta ones are absent, not `undefined`-valued,
    // because the transport carries an `undefined`-valued key as one.
    expect(detail).toEqual({ x: 12, y: 30 });
    expect(Object.keys(detail)).toEqual(["x", "y"]);
  });

  it("adds the delta for the one event that carries one", () => {
    const { inner } = tree();

    const detail = detailOf(inner, "wheel", {
      x: 5,
      y: 6,
      deltaX: 0,
      deltaY: 30,
    });

    expect(detail).toEqual({ x: 5, y: 6, deltaX: 0, deltaY: 30 });
    expect(Object.keys(detail)).toEqual(["x", "y", "deltaX", "deltaY"]);
  });

  // The host names a detail *kind*, not an event: the numbers behind
  // `DETAIL_SIZE` make an `<image>` `load`'s `{width, height}`, web-core's
  // `naturalWidth`/`naturalHeight` and not the box the bitmap drew into.
  it("builds a size detail out of the kind the host named", () => {
    const { inner } = tree();

    const detail = detailOf(inner, "load", { width: 40, height: 20 });

    expect(detail).toEqual({ width: 40, height: 20 });
    expect(Object.keys(detail)).toEqual(["width", "height"]);
  });

  // web-core's `error` detail exactly: no keys, and no numbers spent to say
  // so.
  it("builds an empty detail out of the kind that spends no numbers", () => {
    const { inner } = tree();

    expect(detailOf(inner, "error", { empty: true })).toEqual({});
  });
});

describe("timestamp and params", () => {
  /** The two members every dispatched event carries, whatever its type. */
  interface StampedEvent {
    timestamp: number;
    params: Record<string, unknown>;
  }

  it("gives every event the host's timestamp and a fresh empty params", () => {
    const { inner } = tree();
    const seen: StampedEvent[] = [];
    __AddEventListener(inner, "tap", (event: StampedEvent) => {
      seen.push(event);
    }, {});

    dispatch([inner], "tap", { timestamp: 1234.5 });

    expect(seen).toHaveLength(1);
    expect(seen[0]?.timestamp).toBe(1234.5);
    expect(seen[0]?.params).toEqual({});
  });

  it("reports the time origin for a reading the host could not take", () => {
    const { inner } = tree();
    const seen: number[] = [];
    __AddEventListener(inner, "tap", (event: StampedEvent) => {
      seen.push(event.timestamp);
    }, {});

    elementModule.__BobcatDispatchEvent(
      String(__GetElementUniqueID(inner)),
      String(__GetElementUniqueID(inner)),
      "tap",
      undefined,
      0,
      0,
      undefined,
      undefined,
    );

    expect(seen).toEqual([0]);
  });

  it("mints a new params for each dispatch", () => {
    const { inner } = tree();
    const seen: Record<string, unknown>[] = [];
    __AddEventListener(inner, "tap", (event: StampedEvent) => {
      seen.push(event.params);
    }, {});

    dispatch([inner], "tap");
    dispatch([inner], "tap");

    expect(seen).toHaveLength(2);
    // One object per dispatch, like the event that carries it: what a
    // listener wrote into one event's `params` is not in the next one's.
    expect(seen[0]).not.toBe(seen[1]);
  });

  it("publishes both to a background-thread handler", () => {
    const { inner } = tree();
    const uid = __GetElementUniqueID(inner);
    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");

    dispatch([inner], "tap", { timestamp: 42 });

    const target = { dataset: {}, id: null, uid };
    expect(mock.named("publishEvent")).toEqual([
      ["publishEvent", undefined, "3:0:bindtap", {
        type: "tap",
        eventPhase: 2,
        target,
        currentTarget: target,
        detail: { x: 0, y: 0 },
        timestamp: 42,
        params: {},
      }],
    ]);
  });
});

describe("touch events", () => {
  /** One entry of a decoded touch list, as these tests read it. */
  interface TouchPoint {
    identifier: number;
    x: number;
    y: number;
    pageX: number;
    pageY: number;
    clientX: number;
    clientY: number;
  }

  /** The event a touch listener sees: the three lists it alone carries. */
  interface TouchEvent {
    type: string;
    touches?: TouchPoint[];
    targetTouches?: TouchPoint[];
    changedTouches?: TouchPoint[];
  }

  /** Delivers one event and returns what the listener was handed. */
  function deliver(
    element: object,
    name: string,
    touchNumbers: number[],
  ): TouchEvent {
    let seen: TouchEvent | undefined;
    __AddEventListener(element, name, (event: TouchEvent) => {
      seen = {
        type: event.type,
        ...(event.touches === undefined ? {} : { touches: event.touches }),
        ...(event.targetTouches === undefined
          ? {}
          : { targetTouches: event.targetTouches }),
        ...(event.changedTouches === undefined
          ? {}
          : { changedTouches: event.changedTouches }),
      };
    }, {});
    dispatch([element], name, { touchNumbers });
    expect(seen, "the listener ran").toBeDefined();
    return seen as TouchEvent;
  }

  it("sorts the points into the three lists by their flags", () => {
    const { inner } = tree();

    // Two fingers: the first is down elsewhere (active only), the second is
    // this event's own, on this target (active, target, changed).
    const event = deliver(inner, "touchmove", [1, 10, 20, 1, 2, 30.5, 40, 7]);

    const first = {
      identifier: 1,
      x: 10,
      y: 20,
      pageX: 10,
      pageY: 20,
      clientX: 10,
      clientY: 20,
    };
    const second = {
      identifier: 2,
      x: 30.5,
      y: 40,
      pageX: 30.5,
      pageY: 40,
      clientX: 30.5,
      clientY: 40,
    };
    expect(event.touches).toEqual([first, second]);
    expect(event.targetTouches).toEqual([second]);
    expect(event.changedTouches).toEqual([second]);
  });

  it("leaves a lifted finger out of every list but the changed one", () => {
    const { inner } = tree();

    const event = deliver(inner, "touchend", [1, 12, 20, 4]);

    expect(event.touches).toEqual([]);
    expect(event.targetTouches).toEqual([]);
    expect(event.changedTouches).toEqual([{
      identifier: 1,
      x: 12,
      y: 20,
      pageX: 12,
      pageY: 20,
      clientX: 12,
      clientY: 20,
    }]);
  });

  it("gives an event with no touch points no such keys at all", () => {
    const { inner } = tree();

    const event = deliver(inner, "tap", []);

    // Absent, not `undefined`-valued: the transport carries an
    // `undefined`-valued key as one rather than dropping it.
    expect(Object.keys(event)).toEqual(["type"]);
  });

  it("carries the lists to a background-thread handler", () => {
    const { inner } = tree();
    const uid = __GetElementUniqueID(inner);
    __AddEvent(inner, "bindEvent", "touchend", "3:0:bindtouchend");

    dispatch([inner], "touchend", { touchNumbers: [1, 12, 20, 4] });

    const target = { dataset: {}, id: null, uid };
    const point = {
      identifier: 1,
      x: 12,
      y: 20,
      pageX: 12,
      pageY: 20,
      clientX: 12,
      clientY: 20,
    };
    expect(mock.named("publishEvent")).toEqual([
      ["publishEvent", undefined, "3:0:bindtouchend", {
        type: "touchend",
        eventPhase: 2,
        target,
        currentTarget: target,
        detail: { x: 0, y: 0 },
        timestamp: 0,
        params: {},
        touches: [],
        targetTouches: [],
        changedTouches: [point],
      }],
    ]);
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
    __AddEvent(inner, "bindEvent", "tap", "old");

    __SetEvents(inner, [
      { type: "bindEvent", name: "longpress", function: "new" },
    ]);

    expect(__GetEvent(inner, "tap", "bindEvent")).toBeUndefined();
    expect(__GetEvent(inner, "longpress", "bindEvent")).toBe("new");
    expect(mock.named("listenerNameClosed")).toEqual([
      ["listenerNameClosed", "tap"],
    ]);
  });

  it("round-trips what __GetEvents reported", () => {
    const { inner, outer } = tree();
    __AddEvent(inner, "capture-bind", "tap", "a");
    __AddEvent(inner, "global-bindEvent", "scroll", "b");
    // Both kinds of one name, which is the case the round trip could only
    // carry once the two stopped sharing a slot.
    __AddEvent(inner, "capture-bind", "tap", { type: "worklet", value: {} });

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

describe("__GetPageElement", () => {
  it("is undefined before __CreatePage and the page handle after it", () => {
    expect(__GetPageElement()).toBeUndefined();
    const page = __CreatePage("card", 0);
    // web-core's is `() => page`, the binding its own `__CreatePage` assigns.
    expect(__GetPageElement()).toBe(page);
  });
});

describe("__QuerySelector and __QuerySelectorAll", () => {
  it("ask the host for a root-exclusive match and map the ids to handles", () => {
    const { page, outer, inner } = tree();
    mock.answerQuery = (_root, _selector, firstOnly) =>
      firstOnly
        ? String(__GetElementUniqueID(outer))
        : `${__GetElementUniqueID(outer)},${__GetElementUniqueID(inner)}`;

    expect(__QuerySelector(page, ".row", {})).toBe(outer);
    expect(__QuerySelectorAll(page, ".row", {})).toEqual([outer, inner]);

    // `includeRoot` 0: these two are `Element.querySelector`'s scope, which
    // never answers the element it was asked on — unlike SelectorQuery's.
    expect(mock.named("queryElementIds")).toEqual([
      ["queryElementIds", __GetElementUniqueID(page), ".row", 1, 0],
      ["queryElementIds", __GetElementUniqueID(page), ".row", 0, 0],
    ]);
  });

  it("stringifies the selector and ignores the params object", () => {
    const { page } = tree();
    mock.answerQuery = () => "";

    expect(__QuerySelector(page, 7, { onlyCurrentComponent: true }))
      .toBeUndefined();
    expect(__QuerySelectorAll(page, 7)).toEqual([]);

    expect(mock.named("queryElementIds").map((call) => call[2])).toEqual([
      "7",
      "7",
    ]);
  });

  it("lets a selector the host refuses throw", () => {
    const { page } = tree();
    const refusal = new Error("'!' is not a valid selector");
    mock.answerQuery = () => {
      throw refusal;
    };

    // The DOM throws a SyntaxError for an unparsable selector and so does
    // web-core's; nothing here catches the host's refusal.
    expect(() => __QuerySelector(page, "!", {})).toThrow(refusal);
    expect(() => __QuerySelectorAll(page, "!", {})).toThrow(refusal);
  });

  it("refuses a match no live handle names", () => {
    const { page } = tree();
    mock.answerQuery = () => "4242";

    expect(() => __QuerySelectorAll(page, ".row", {})).toThrow(
      "a queried live node has no handle",
    );
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

    // Storage only: filing a callback is not itself a host call. What reads
    // them is `__SetAttribute(list, "update-list-info", …)`, below.
    expect(mock.calls).toEqual([]);
  });
});

/**
 * A list, the cells a `componentAtIndex` will hand it, and the recorder the
 * assertions read.
 *
 * `componentAtIndex` stands in for ReactLynx's: it appends the cell for an
 * index before answering its sign, which is what makes the "already in
 * place" check below a real case rather than a hypothetical one.
 */
function listFixture(cellCount: number) {
  const list = __CreateList(0, null, null) as object;
  const cells = Array.from({ length: cellCount }, () => __CreateView(0));
  const calls: unknown[][] = [];
  const componentAtIndex = (
    ...args: unknown[]
  ): number => {
    calls.push(["componentAtIndex", ...args.slice(1)]);
    const cell = cells[args[2] as number]!;
    __AppendElement(list, cell);
    return __GetElementUniqueID(cell);
  };
  const enqueueComponent = (...args: unknown[]): undefined => {
    calls.push(["enqueueComponent", ...args.slice(1)]);
    return undefined;
  };
  return { list, cells, calls, componentAtIndex, enqueueComponent };
}

/** The node ids of a list's element children, in tree order. */
function childIds(list: object): number[] {
  return __GetChildren(list).map((child) => __GetElementUniqueID(child));
}

/**
 * Lets the update-list-info microtask run.
 *
 * The runtime schedules it with `Promise.resolve().then`, because the MTS
 * realm has no `queueMicrotask`; awaiting one promise tick is therefore
 * exactly one drain of the job it queued.
 */
function drain(): Promise<void> {
  return Promise.resolve();
}

/**
 * The rejections `run` leaves behind.
 *
 * The microtask is fire-and-forget, so a throw inside it becomes an
 * unhandled rejection rather than something a caller can catch — which is
 * exactly what the realm's rejection tracker reports it as. The runner's own
 * listener is stood down for the duration and put back afterwards, because
 * an expected rejection is not a failed test file; the timer is the turn
 * Node reports rejections on, which is after the microtask queue drains.
 */
async function collectRejections(
  run: () => Promise<void>,
): Promise<unknown[]> {
  const failures: unknown[] = [];
  const record = (error: unknown) => failures.push(error);
  const runners = process.listeners("unhandledRejection");
  process.removeAllListeners("unhandledRejection");
  process.on("unhandledRejection", record);
  try {
    await run();
    await new Promise((resolve) => setTimeout(resolve, 0));
  } finally {
    process.off("unhandledRejection", record);
    for (const listener of runners) {
      process.on("unhandledRejection", listener);
    }
  }
  return failures;
}

describe("update-list-info", () => {
  it("inserts the cells componentAtIndex builds, in position order", async () => {
    const { list, cells, calls, componentAtIndex, enqueueComponent } =
      listFixture(3);
    __SetAttribute(list, "update-list-info", {
      insertAction: [{ position: 0 }, { position: 1 }, { position: 2 }],
      removeAction: [],
      updateAction: [],
    });
    __UpdateListCallbacks(list, componentAtIndex, enqueueComponent, () => []);

    // Nothing before the microtask: the callbacks are filed after the write.
    expect(calls).toEqual([]);
    expect(childIds(list)).toEqual([]);

    await drain();
    expect(calls).toEqual([
      ["componentAtIndex", __GetElementUniqueID(list), 0, 0, false],
      ["componentAtIndex", __GetElementUniqueID(list), 1, 0, false],
      ["componentAtIndex", __GetElementUniqueID(list), 2, 0, false],
    ]);
    expect(childIds(list)).toEqual(cells.map(__GetElementUniqueID));
  });

  it("reads the callbacks the flush filed after the write, not the ones before it", async () => {
    const { list, cells, calls, componentAtIndex, enqueueComponent } =
      listFixture(1);
    const stale = () => {
      throw new Error("the stale componentAtIndex ran");
    };
    __UpdateListCallbacks(list, stale, stale, null);

    __SetAttribute(list, "update-list-info", {
      insertAction: [{ position: 0 }],
      removeAction: [],
    });
    // ReactLynx's `flush()` order: the operations, then the pair that serves
    // them. The microtask is what makes the two one operation.
    __UpdateListCallbacks(list, componentAtIndex, enqueueComponent, () => []);

    await drain();
    expect(calls).toEqual([
      ["componentAtIndex", __GetElementUniqueID(list), 0, 0, false],
    ]);
    expect(childIds(list)).toEqual([__GetElementUniqueID(cells[0]!)]);
  });

  it("does not move a cell componentAtIndex already appended in place", async () => {
    const { list, componentAtIndex, enqueueComponent } = listFixture(2);
    __SetAttribute(list, "update-list-info", {
      insertAction: [{ position: 0 }, { position: 1 }],
      removeAction: [],
    });
    __UpdateListCallbacks(list, componentAtIndex, enqueueComponent, null);
    await drain();

    // Every append landed at its own position, so the protocol made no tree
    // call of its own: only the two `insertBefore`s the appends were.
    expect(mock.named("insertBefore")).toHaveLength(2);
    expect(mock.named("insertBefore").map((call) => call[3])).toEqual([
      null,
      null,
    ]);
  });

  it("moves a cell appended past its position back to it", async () => {
    const { list, cells, componentAtIndex, enqueueComponent } = listFixture(2);
    // The list already holds the second cell, so building the first appends
    // it *after* that one and it has to be moved before it.
    __AppendElement(list, cells[1]!);
    __SetAttribute(list, "update-list-info", {
      insertAction: [{ position: 0 }],
      removeAction: [],
    });
    __UpdateListCallbacks(list, componentAtIndex, enqueueComponent, null);
    await drain();

    expect(childIds(list)).toEqual([
      __GetElementUniqueID(cells[0]!),
      __GetElementUniqueID(cells[1]!),
    ]);
  });

  it("shifts each removal by the removals before it", async () => {
    const { list, cells, calls, componentAtIndex, enqueueComponent } =
      listFixture(5);
    for (const cell of cells) {
      __AppendElement(list, cell);
    }
    const listId = __GetElementUniqueID(list);

    // Old indices 1 and 3, ascending: after the first removal the child at
    // old index 3 sits at current index 2, which is `position - i`.
    __SetAttribute(list, "update-list-info", {
      insertAction: [],
      removeAction: [1, 3],
    });
    __UpdateListCallbacks(list, componentAtIndex, enqueueComponent, null);
    await drain();

    expect(calls).toEqual([
      ["enqueueComponent", listId, __GetElementUniqueID(cells[1]!)],
      ["enqueueComponent", listId, __GetElementUniqueID(cells[3]!)],
    ]);
    expect(childIds(list)).toEqual([
      __GetElementUniqueID(cells[0]!),
      __GetElementUniqueID(cells[2]!),
      __GetElementUniqueID(cells[4]!),
    ]);
  });

  it("removes before it inserts, and skips a removal past the end", async () => {
    const { list, cells, calls, componentAtIndex, enqueueComponent } =
      listFixture(3);
    __AppendElement(list, cells[2]!);
    __SetAttribute(list, "update-list-info", {
      insertAction: [{ position: 0 }],
      // Index 1 does not exist: web-core's `if (removedEle)` skips it.
      removeAction: [0, 1],
    });
    __UpdateListCallbacks(list, componentAtIndex, enqueueComponent, null);
    await drain();

    expect(calls).toEqual([
      [
        "enqueueComponent",
        __GetElementUniqueID(list),
        __GetElementUniqueID(cells[2]!),
      ],
      ["componentAtIndex", __GetElementUniqueID(list), 0, 0, false],
    ]);
    expect(childIds(list)).toEqual([__GetElementUniqueID(cells[0]!)]);
  });

  it("ignores updateAction", async () => {
    const { list, calls, componentAtIndex, enqueueComponent } = listFixture(1);
    __SetAttribute(list, "update-list-info", {
      insertAction: [],
      removeAction: [],
      updateAction: [{ from: 0, to: 0, type: "x", flush: false }],
    });
    __UpdateListCallbacks(list, componentAtIndex, enqueueComponent, null);
    await drain();

    expect(calls).toEqual([]);
    expect(childIds(list)).toEqual([]);
  });

  it("never calls the componentAtIndexes batch callback", async () => {
    const { list, componentAtIndex, enqueueComponent } = listFixture(1);
    const batch = () => {
      throw new Error("componentAtIndexes ran");
    };
    __SetAttribute(list, "update-list-info", {
      insertAction: [{ position: 0 }],
      removeAction: [],
    });
    __UpdateListCallbacks(list, componentAtIndex, enqueueComponent, batch);
    await drain();

    expect(childIds(list)).toHaveLength(1);
  });

  it("inserts nothing once the callbacks are cleared", async () => {
    const { list, componentAtIndex, enqueueComponent } = listFixture(1);
    __UpdateListCallbacks(list, componentAtIndex, enqueueComponent, null);
    __SetAttribute(list, "update-list-info", {
      insertAction: [{ position: 0 }],
      removeAction: [],
    });
    // ReactLynx clears all three when it tears a list down.
    __UpdateListCallbacks(list, null, null, null);
    mock.calls.length = 0;
    await drain();

    // No `componentAtIndex` is no sign, so nothing is built and nothing is
    // moved — web-core's `componentAtIndex?.(…)` answering undefined.
    expect(childIds(list)).toEqual([]);
    expect(mock.named("insertBefore")).toEqual([]);
  });

  it("still removes with a cleared enqueueComponent, as web-core's ?. does", async () => {
    const { list, cells } = listFixture(1);
    __AppendElement(list, cells[0]!);
    __SetAttribute(list, "update-list-info", {
      insertAction: [],
      removeAction: [0],
    });
    __UpdateListCallbacks(list, null, null, null);
    await drain();

    // web-core reaches the callback through `?.` and removes the child
    // either way; only the notification is optional.
    expect(childIds(list)).toEqual([]);
  });

  it("is a no-op for a payload that is not an object", async () => {
    const { list, cells } = listFixture(1);
    __AppendElement(list, cells[0]!);
    mock.calls.length = 0;

    // web-core destructures the value at the call and throws a TypeError
    // naming neither the list nor the member; this is the recorded
    // deviation.
    expect(__SetAttribute(list, "update-list-info", null)).toBe(undefined);
    expect(__SetAttribute(list, "update-list-info", "x")).toBe(undefined);
    await drain();

    expect(mock.calls).toEqual([]);
    expect(childIds(list)).toHaveLength(1);
  });

  it("refuses a sign no live handle names, and stops the batch", async () => {
    const { list, cells, enqueueComponent } = listFixture(1);
    // 4242 is no element of this document: the ownership graph and the tree
    // would disagree about a cell this very call built. web-core skips such
    // a sign; this throws, which is the recorded deviation.
    const signs = [4242, __GetElementUniqueID(cells[0]!)];
    __SetAttribute(list, "update-list-info", {
      insertAction: [{ position: 0 }, { position: 1 }],
      removeAction: [],
    });
    __UpdateListCallbacks(list, () => signs.shift(), enqueueComponent, null);

    const failures = await collectRejections(async () => {
      await drain();
    });

    expect(String(failures[0])).toContain("4242");
    expect(String(failures[0])).toContain("no live handle names");
    // The throw ended the microtask, so the second insertion never ran.
    expect(signs).toEqual([__GetElementUniqueID(cells[0]!)]);
    expect(childIds(list)).toEqual([]);
  });
});

/**
 * Encodes fields the way the native side writes a record payload back, so an
 * expectation below can be written as the values it stands for.
 */
function styleRecord(...fields: string[]): string {
  return fields.map((field) => `${field.length}:${field}`).join("");
}

describe("__InvokeUIMethod", () => {
  it("reports a method the engine does not have as code 3", () => {
    const view = __CreateView(0);
    const callback = rstest.fn();
    mock.calls.length = 0;

    expect(__InvokeUIMethod(view, "scrollIntoView", {}, callback)).toBe(
      undefined,
    );

    // Already called by the time the PAPI returned: the callback is
    // synchronous, so a card can measure and act in one job.
    expect(callback).toHaveBeenCalledTimes(1);
    expect(callback.mock.calls[0]?.[0]).toStrictEqual({
      code: 3,
      data: undefined,
    });
  });

  it("derives right and bottom, and carries the id and dataset", () => {
    const view = __CreateView(0);
    __SetID(view, "target");
    __SetDataset(view, { index: 2 });
    mock.answerElementMethod = () => "20,0,100,50";
    mock.calls.length = 0;
    const callback = rstest.fn();

    __InvokeUIMethod(view, "boundingClientRect", { relativeTo: 7 }, callback);

    expect(callback.mock.calls[0]?.[0]).toEqual({
      code: 0,
      data: {
        id: "target",
        dataset: { index: 2 },
        left: 20,
        top: 0,
        right: 120,
        bottom: 50,
        width: 100,
        height: 50,
      },
    });
    // The host takes the element and the method name and nothing else:
    // `params` names behavior this engine does not have, so it is dropped
    // here rather than carried to a boundary that would ignore it.
    expect(mock.named("callElementMethod")).toEqual([
      ["callElementMethod", __GetElementUniqueID(view), "boundingClientRect"],
    ]);
  });

  it("reports an element carrying no id as the empty string", () => {
    const view = __CreateView(0);
    mock.answerElementMethod = () => "0,0,0,0";
    const callback = rstest.fn();

    __InvokeUIMethod(view, "boundingClientRect", undefined, callback);

    const { data } = callback.mock.calls[0]?.[0] as { data: Record<string, unknown> };
    // `element.id`'s answer, not the attribute's absence.
    expect(data['id']).toBe("");
    expect(data['dataset']).toEqual({});
  });

  it("hands over a dataset copy the caller owns", () => {
    const view = __CreateView(0);
    __SetDataset(view, { index: 2 });
    mock.answerElementMethod = () => "0,0,0,0";
    const callback = rstest.fn();

    __InvokeUIMethod(view, "boundingClientRect", {}, callback);

    const { data } = callback.mock.calls[0]?.[0] as { data: Record<string, unknown> };
    (data['dataset'] as Record<string, unknown>)['index'] = 9;
    expect(__GetDataset(view)).toEqual({ index: 2 });
  });
});

describe("__GetComputedStyleByKey", () => {
  it("answers the value field of the record the host writes back", () => {
    const view = __CreateView(0);
    mock.answerComputedStyle = () => styleRecord("margin-top", "12.5px");
    mock.calls.length = 0;

    expect(__GetComputedStyleByKey(view, "margin-top")).toBe("12.5px");
    // Resolved values, which is what CSSOM's getComputedStyle reports.
    expect(mock.named("getComputedStyleMap")).toEqual([
      ["getComputedStyleMap", __GetElementUniqueID(view), "margin-top", 1],
    ]);
  });

  it("answers the empty string for an empty record", () => {
    const view = __CreateView(0);

    // The host's answer before the first flush, and for a name it has no
    // property for.
    expect(__GetComputedStyleByKey(view, "color")).toBe("");
    expect(__GetComputedStyleByKey(view, "not-a-property")).toBe("");
  });

  it("passes the key verbatim, so an IDL name answers nothing", () => {
    const view = __CreateView(0);
    mock.answerComputedStyle = (_node, properties) =>
      properties === "margin-top" ? styleRecord("margin-top", "4px") : "";
    mock.calls.length = 0;

    expect(__GetComputedStyleByKey(view, "marginTop")).toBe("");
    expect(mock.named("getComputedStyleMap")[0]).toEqual([
      "getComputedStyleMap",
      __GetElementUniqueID(view),
      "marginTop",
      1,
    ]);
  });

  it("answers the empty string for a key that is not a string, unasked", () => {
    const view = __CreateView(0);
    mock.calls.length = 0;

    expect(__GetComputedStyleByKey(view, undefined)).toBe("");
    expect(__GetComputedStyleByKey(view, Symbol("color"))).toBe("");
    expect(mock.named("getComputedStyleMap")).toEqual([]);
  });
});

describe("__BobcatComputedStyleMap", () => {
  const style = () =>
    styleRecord(
      "color",
      "rgb(255, 0, 0)",
      "margin-top",
      "0px",
      "--brand",
      "blue",
    );

  it("asks for the whole computed style, unresolved", () => {
    const view = __CreateView(0);
    mock.answerComputedStyle = style;
    mock.calls.length = 0;

    const map = __BobcatComputedStyleMap(view);

    expect(mock.named("getComputedStyleMap")).toEqual([
      ["getComputedStyleMap", __GetElementUniqueID(view), "", 0],
    ]);
    expect(map.size).toBe(3);
  });

  it("answers one value per property, by ASCII-lowercased name", () => {
    const view = __CreateView(0);
    mock.answerComputedStyle = style;
    const map = __BobcatComputedStyleMap(view);

    expect(map.get("color")).toBeInstanceOf(elementModule.CSSStyleValue);
    expect(String(map.get("color"))).toBe("rgb(255, 0, 0)");
    expect(String(map.get("COLOR"))).toBe("rgb(255, 0, 0)");
    expect(map.getAll("margin-top").map(String)).toEqual(["0px"]);
    expect(map.has("--brand")).toBe(true);
    expect(String(map.get("--brand"))).toBe("blue");
  });

  it("throws for a name no property has, and misses an absent custom one", () => {
    const view = __CreateView(0);
    mock.answerComputedStyle = style;
    const map = __BobcatComputedStyleMap(view);

    // Every author-facing longhand is in the map, so absence is invalidity
    // — the standard's TypeError — everywhere but a custom property, which
    // is a valid name whether or not the element carries it.
    expect(() => map.get("not-a-property")).toThrow(TypeError);
    expect(() => map.getAll("not-a-property")).toThrow(TypeError);
    expect(() => map.has("not-a-property")).toThrow(TypeError);
    expect(map.get("--missing")).toBe(undefined);
    expect(map.getAll("--missing")).toEqual([]);
    expect(map.has("--missing")).toBe(false);
  });

  it("iterates in the record's order, as name and value list", () => {
    const view = __CreateView(0);
    mock.answerComputedStyle = style;
    const map = __BobcatComputedStyleMap(view);

    expect([...map].map(([name, values]) => [name, values.map(String)]))
      .toEqual([
        ["color", ["rgb(255, 0, 0)"]],
        ["margin-top", ["0px"]],
        ["--brand", ["blue"]],
      ]);
    expect([...map.keys()]).toEqual(["color", "margin-top", "--brand"]);
    expect([...map.values()].map((values) => values.map(String))).toEqual([
      ["rgb(255, 0, 0)"],
      ["0px"],
      ["blue"],
    ]);
    expect([...map.entries()]).toEqual([...map]);
    const seen: unknown[][] = [];
    const thisArg = { marker: true };
    map.forEach(function(this: unknown, values, name, forEached) {
      seen.push([name, values.map(String), forEached === map, this]);
    }, thisArg);
    // WebIDL's maplike order: the value, the key, then the map itself.
    expect(seen).toEqual([
      ["color", ["rgb(255, 0, 0)"], true, thisArg],
      ["margin-top", ["0px"], true, thisArg],
      ["--brand", ["blue"], true, thisArg],
    ]);
  });

  it("is a snapshot: a later host answer does not reach a built map", () => {
    const view = __CreateView(0);
    mock.answerComputedStyle = style;
    const map = __BobcatComputedStyleMap(view);
    mock.answerComputedStyle = () => styleRecord("color", "rgb(0, 0, 255)");

    // The standard's map is [SameObject] and live; this one is one host
    // answer, decoded once.
    expect(String(map.get("color"))).toBe("rgb(255, 0, 0)");
    expect(String(__BobcatComputedStyleMap(view).get("color")))
      .toBe("rgb(0, 0, 255)");
  });

  it("answers an empty map before the first flush", () => {
    const view = __CreateView(0);
    const map = __BobcatComputedStyleMap(view);

    expect(map.size).toBe(0);
    expect([...map]).toEqual([]);
    // Nothing is in it, so nothing non-custom is a valid name to it.
    expect(() => map.get("color")).toThrow(TypeError);
  });
});
