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
    listenerOpened: native.listenerOpened,
    listenerClosed: native.listenerClosed,
    timelineMilliseconds: native.timelineMilliseconds,
    dispatchEvent: native.dispatchEvent,
    switchExposure: native.switchExposure,
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
  // The exposure lists `bobcat:element` builds, as they would be sent:
  // the transport's copy, without the timestamp the sender writes.
  __BobcatSendExposureRecords(exposure: unknown[], disexposure: unknown[]) {
    const native =
      globalThis.__bobcatTestHost as ReturnType<typeof createMockBobcat>;
    native.calls.push([
      "sendExposureRecords",
      structuredClone(exposure),
      structuredClone(disexposure),
    ]);
  },
}));

// A frame's `src` load and its `data`/`global-props` are `bobcat:frame`'s,
// tested in frame.test.ts; here each is the one call `__SetAttribute` makes,
// recorded with the mock's calls.
rstest.mockRequire("bobcat:frame", () => ({
  frameSourceChanged(handle: object, frameId: number, src: string | null) {
    const native =
      globalThis.__bobcatTestHost as ReturnType<typeof createMockBobcat>;
    native.calls.push(["frameSourceChanged", handle, frameId, src]);
  },
  frameDataChanged(handle: object, name: string, json: string | null) {
    const native =
      globalThis.__bobcatTestHost as ReturnType<typeof createMockBobcat>;
    native.calls.push(["frameDataChanged", handle, name, json]);
  },
  frameConnectionChanged(handle: object, frameId: number, connected: boolean) {
    const native =
      globalThis.__bobcatTestHost as ReturnType<typeof createMockBobcat>;
    native.calls.push(["frameConnectionChanged", handle, frameId, connected]);
  },
}));

/**
 * What the mock's second `createDocument` throws, so a test can assert that
 * the host's refusal reaches the caller by identity — the native member
 * refuses a second document, and this file adds no check of its own.
 */
const DOCUMENT_REFUSAL = new Error("the realm already created its document");

/** The document node's id, which `createDocument` answers. */
const DOCUMENT_NODE_ID = 1;

/**
 * The page configuration a boot module is written with and hands to the
 * constructor. Its five switches are the host's; nothing in this file reads
 * them, and the constructor's only job is to pass them on, in order.
 */
const PAGE_CONFIG: elementPapi.PageConfig = {
  defaultDisplayLinear: true,
  defaultOverflowVisible: false,
  enableCssSelector: false,
  enableJSDataProcessor: true,
  enableCssInheritance: true,
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
  answerElementMethod: (nodeId: number, method: string, params: string) => string | number;
  answerComputedStyle: (
    nodeId: number,
    properties: string,
    resolved: 0 | 1,
  ) => string;
  /**
   * What a synthetic dispatch does in the host besides being recorded. The
   * real host computes the path from the tree, runs the engine components'
   * walk and calls each invited realm, all of which
   * crates/bobcat-core/src/main/runtime/tests.rs covers; the default calls
   * no realm, and `callsBack` calls this one as the host does.
   */
  answerDispatch: (
    nodeId: number,
    eventName: string,
    bubbles: boolean,
    composed: boolean,
    timestamp: number,
  ) => void;
  /** What `timelineMilliseconds` answers, which a test sets. */
  timeline: number;
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
  // What `dropElement` freed. The real boundary refuses every read of a freed
  // element, and the two attribute reads refuse here too.
  const freed: Set<number> = new Set();
  const live = (name: string, id: number): number => {
    if (freed.has(id)) {
      throw new TypeError(`${name} received a stale element id`);
    }
    return id;
  };
  // One document per realm for the life of the realm, as the native slot
  // enforces it: the ingredients a construction spends are never restored.
  let documentSpent = false;

  const host: MockBobcat = {
    calls,
    named,
    // Arguments are recorded rather than ignored: a test asserts the five
    // switches arrive as booleans, in `PageConfig`'s order.
    createDocument: (...args: unknown[]) => {
      calls.push(["createDocument", ...args]);
      if (documentSpent) {
        throw DOCUMENT_REFUSAL;
      }
      documentSpent = true;
      // The document node's id, as the host answers it.
      return DOCUMENT_NODE_ID;
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
      const id = live("getAttribute", nodeId("getAttribute", node));
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
    answerElementMethod: () => 3,
    callElementMethod: (node: unknown, method: unknown, params: unknown) => {
      const id = nodeId("callElementMethod", node);
      calls.push(["callElementMethod", id, method, params]);
      return host.answerElementMethod(id, method as string, params as string);
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
      const id = live("attributeNames", nodeId("attributeNames", node));
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
      const id = nodeId("dropElement", node);
      freed.add(id);
      calls.push(["dropElement", id]);
    },
    flushElementTree: () => {
      calls.push(["flushElementTree"]);
    },
    listenerOpened: (node: unknown, eventName: unknown, kinds: unknown) => {
      calls.push(["listenerOpened", nodeId("listenerOpened", node), eventName, kinds]);
    },
    listenerClosed: (node: unknown, eventName: unknown) => {
      calls.push(["listenerClosed", nodeId("listenerClosed", node), eventName]);
    },
    timeline: 0,
    timelineMilliseconds: () => host.timeline,
    // What the host does inside `dispatchEvent` besides recording it: by
    // default nothing, a dispatch no realm's listener table invites this
    // realm to; `callsBack` has it call this realm the way the host does.
    answerDispatch: (
      _node: number,
      _eventName: string,
      _bubbles: boolean,
      _composed: boolean,
      _timestamp: number,
    ): void => {},
    dispatchEvent: (
      node: unknown,
      eventName: unknown,
      bubbles: unknown,
      composed: unknown,
      timestamp: unknown,
      detail: unknown,
    ): undefined => {
      const id = nodeId("dispatchEvent", node);
      calls.push(["dispatchEvent", id, eventName, bubbles, composed, timestamp, detail]);
      host.answerDispatch(
        id,
        eventName as string,
        bubbles as boolean,
        composed as boolean,
        timestamp as number,
      );
      return undefined;
    },
    // A stop with `sendEvent` answers one record per listed id, as if every
    // listed element were exposed under an `exposure-id`.
    switchExposure: (on: 0 | 1, sendEvent: 0 | 1, ids: string) => {
      calls.push(["switchExposure", on, sendEvent, ids]);
      return on === 0 && sendEvent === 1 && ids !== "" ? ids.split(",").length : 0;
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
      ["__CreateFrame", 1],
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
      ["__CreateEvent", 4],
      ["__DispatchEvent", 2],
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
        // constructs, which `MTS_CHUNK_PREAMBLE` does not import.
        "Document",
        // Nor these four: `bobcat:frame-boot` chooses a frame realm's page
        // root, stops its exposure and waits for its last exposure records
        // with them, `bobcat:runtime`'s `lynx` switches the realm's exposure
        // with the second, and the host's tests find a handle by its id.
        "__BobcatUsePageRoot",
        "__BobcatSwitchExposure",
        "__BobcatAfterExposureRecords",
        "__BobcatHandleOf",
      ].sort(),
    );
    // The four every event carries; the path, the extras and the detail
    // numbers are a rest parameter, which `length` does not count.
    expect(elementModule.__BobcatDispatchEvent).toHaveLength(4);
  });

  it("creates the realm's document once, over the config it is given", () => {
    void new elementModule.Document(PAGE_CONFIG);
    expect(mock.named("createDocument")).toEqual([
      // `PageConfig`'s order: display, overflow, selectors, processor,
      // inheritance.
      ["createDocument", true, false, false, true, true],
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

  it("makes the document the handle of the document node, an event target", () => {
    const document = new elementModule.Document(PAGE_CONFIG);
    expect(elementModule.__BobcatHandleOf(DOCUMENT_NODE_ID)).toBe(document);
    expect(__GetElementUniqueID(document)).toBe(DOCUMENT_NODE_ID);

    const seen: unknown[] = [];
    const listener = (event: { type: string; target: unknown; currentTarget: unknown }) => {
      seen.push([event.type, event.target, event.currentTarget]);
    };
    document.addEventListener("selectionchange", listener);
    // The host is told as for any handle: the document node, the bubble pass.
    expect(mock.named("listenerOpened")).toEqual([
      ["listenerOpened", DOCUMENT_NODE_ID, "selectionchange", 1],
    ]);
    // The host's W3C `selectionchange`: a path of the document node alone,
    // not bubbling, with nothing in its detail.
    dispatch([document], "selectionchange", { empty: true }, undefined, false);
    const descriptor = {
      dataset: {},
      id: null,
      uid: DOCUMENT_NODE_ID,
      elementRefptr: document,
    };
    expect(seen).toEqual([["selectionchange", descriptor, descriptor]]);
    // The document node has no attributes to ask the host for.
    expect(mock.named("getAttribute")).toEqual([]);
    expect(mock.named("attributeNames")).toEqual([]);

    document.removeEventListener("selectionchange", listener);
    expect(mock.named("listenerClosed")).toEqual([
      ["listenerClosed", DOCUMENT_NODE_ID, "selectionchange"],
    ]);
    dispatch([document], "selectionchange", { empty: true }, undefined, false);
    expect(seen).toHaveLength(1);
  });

  it("does not install __DropElement: collection is the only release path", () => {
    expect("__DropElement" in globalThis).toBe(false);
  });

  it("__SetCSSId writes l-css-id per element and removes it for 0 or a nullish id", () => {
    const first = __CreateView(0);
    const second = __CreateView(0);
    mock.calls.length = 0;
    // The entry name is not written: no lazy bundle's StyleInfo is mounted.
    expect(__SetCSSId([first, second], 7, "entry")).toBeUndefined();
    expect(mock.calls).toEqual([
      ["setAttribute", 3, "l-css-id", "7"],
      ["setAttribute", 4, "l-css-id", "7"],
    ]);
    expect(__GetAttributeByName(first, "l-css-id")).toBe("7");
    for (const cleared of [0, null, undefined]) {
      mock.calls.length = 0;
      expect(__SetCSSId([first], cleared, undefined)).toBeUndefined();
      expect(mock.calls, String(cleared)).toEqual([
        ["removeAttribute", 3, "l-css-id"],
      ]);
    }
    expect(__GetAttributeByName(first, "l-css-id")).toBe(null);
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

  it("refuses a page root once the realm's page is chosen", () => {
    const page = __CreatePage("card", 0);
    expect(() => elementModule.__BobcatUsePageRoot(99)).toThrow(TypeError);
    expect(__CreatePage("card", 0)).toBe(page);
    expect(elementModule.__BobcatHandleOf(elementModule.__GetElementUniqueID(page) as number)).toBe(page);
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
    __CreateFrame(0);
    expect(mock.named("createElement").map((call) => call[1])).toEqual([
      "custom-widget",
      "wrapper",
      "text",
      "image",
      "view",
      "scroll-view",
      "list",
      "frame",
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

  it("writes a markdown's structured markdown-style as JSON text, as web-core's property does", () => {
    // web-core assigns a non-primitive value to `x-markdown`'s
    // `markdown-style`/`markdownStyle` property, whose setter writes its JSON
    // text to the `markdown-style` attribute; `content-range` has no
    // property, so an array there is `String(value)`, the "start,end" form.
    const style = {link: {color: "ff0000"}, inlineCode: {color: "0000ff"}};
    const json = JSON.stringify(style);
    for (const tag of ["x-markdown", "markdown"]) {
      const markdown = __CreateElement(tag, 0);
      mock.calls.length = 0;
      __SetAttribute(markdown, "markdown-style", style);
      __SetAttribute(markdown, "markdownStyle", style);
      __SetAttribute(markdown, "markdown-style", json);
      __SetAttribute(markdown, "content-range", [0, 10]);
      __SetAttribute(markdown, "content-id", "case-1");
      expect(mock.named("setAttribute").map(call => call.slice(2)), tag).toEqual([
        ["markdown-style", json],
        ["markdown-style", json],
        ["markdown-style", json],
        ["content-range", "0,10"],
        ["content-id", "case-1"],
      ]);
      expect(elementModule.__GetAttributeByName(markdown, "markdown-style"), tag)
        .toEqual(json);
    }
    // Any other element has no such property: the object is its string.
    const view = __CreateView(0);
    mock.calls.length = 0;
    __SetAttribute(view, "markdown-style", style);
    expect(mock.named("setAttribute").map(call => call.slice(2))).toEqual([
      ["markdown-style", "[object Object]"],
    ]);
    // A value the property's `JSON.stringify` refuses throws, as there.
    const cyclic: {self: unknown} = {self: null};
    cyclic.self = cyclic;
    expect(() => __SetAttribute(__CreateElement("x-markdown", 0), "markdown-style", cyclic))
      .toThrow(TypeError);
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

  it("hands a frame's src to bobcat:frame after the native write", () => {
    const frame = __CreateFrame(0);
    const named = __CreateElement("frame", 0);
    const view = __CreateView(0);
    mock.calls.length = 0;

    __SetAttribute(frame, "src", "app:///child.web.bundle");
    __SetAttribute(named, "src", 7);
    __SetAttribute(frame, "src", null);
    // Only a frame's `src` is a load; another name, or another tag, is not.
    __SetAttribute(frame, "title", "x");
    __SetAttribute(view, "src", "app:///child.web.bundle");
    expect(mock.calls).toEqual([
      ["setAttribute", 3, "src", "app:///child.web.bundle"],
      ["frameSourceChanged", frame, 3, "app:///child.web.bundle"],
      ["setAttribute", 4, "src", "7"],
      ["frameSourceChanged", named, 4, "7"],
      ["removeAttribute", 3, "src"],
      ["frameSourceChanged", frame, 3, null],
      ["setAttribute", 3, "title", "x"],
      ["setAttribute", 5, "src", "app:///child.web.bundle"],
    ]);
  });

  it("files the frame connection listener on every frame, where no app can list or remove it", () => {
    mock.calls.length = 0;
    const frame = __CreateFrame(0);
    const named = __CreateElement("frame", 0);
    __CreateView(0);
    const rows = mock.calls.filter(([name]) => String(name).startsWith("listener"));
    expect(rows).toEqual([
      ["listenerOpened", 3, "bobcat:frameconnected", 1],
      ["listenerOpened", 3, "bobcat:framedisconnected", 1],
      ["listenerOpened", 4, "bobcat:frameconnected", 1],
      ["listenerOpened", 4, "bobcat:framedisconnected", 1],
    ]);

    mock.calls.length = 0;
    const listener = () => {};
    __AddEventListener(frame, "bobcat:frameconnected", listener, {});
    __RemoveEventListener(frame, "bobcat:frameconnected", listener, {});
    // An app removal under the internal name names no listener of the
    // app's, and takes nothing away.
    __RemoveEventListener(named, "bobcat:framedisconnected", listener, {});
    expect(mock.calls.filter(([name]) => String(name).startsWith("listener"))).toEqual([]);
    expect(__GetEvents(frame)).toEqual([]);
  });

  it("hands a frame's connection events to bobcat:frame, a detached frame's included", () => {
    const page = __CreatePage("card", 0);
    const frame = __CreateFrame(0);
    __AppendElement(page, frame);
    const id = __GetElementUniqueID(frame);
    mock.calls.length = 0;

    // As the host dispatches them: at the frame, not bubbling, no detail.
    dispatch([frame, page], "bobcat:frameconnected", { empty: true }, undefined, false);
    __RemoveElement(page, frame);
    dispatch([frame], "bobcat:framedisconnected", { empty: true }, undefined, false);
    expect(mock.named("frameConnectionChanged")).toEqual([
      ["frameConnectionChanged", frame, id, true],
      ["frameConnectionChanged", frame, id, false],
    ]);
  });

  it("writes a frame's data and global-props as JSON text and hands that text to bobcat:frame", () => {
    // ReactLynx passes each as the object itself; web-core's nested
    // `lynx-view` takes the object through its `data`/`globalProps`
    // properties, and here the attribute holds its JSON text.
    const page = __CreatePage("card", 0);
    const frame = __CreateFrame(0);
    const view = __CreateView(0);
    __AppendElement(page, frame);
    const data = {label: "x", nested: {count: 1}};
    const json = JSON.stringify(data);
    for (const name of ["data", "global-props"]) {
      mock.calls.length = 0;
      __SetAttribute(frame, name, data);
      __SetAttribute(frame, name, '{"label":"as written"}');
      // Anything else is handed over as its text, for `bobcat:frame` to refuse.
      __SetAttribute(frame, name, 3);
      __SetAttribute(frame, name, [1]);
      __SetAttribute(frame, name, null);
      expect(mock.calls.filter(call => call[0] !== "tagName"), name).toEqual([
        ["setAttribute", 3, name, json],
        ["frameDataChanged", frame, name, json],
        ["setAttribute", 3, name, '{"label":"as written"}'],
        ["frameDataChanged", frame, name, '{"label":"as written"}'],
        ["setAttribute", 3, name, "3"],
        ["frameDataChanged", frame, name, "3"],
        ["setAttribute", 3, name, "[1]"],
        ["frameDataChanged", frame, name, "[1]"],
        ["removeAttribute", 3, name],
        ["frameDataChanged", frame, name, null],
      ]);
      __SetAttribute(frame, name, data);
      expect(elementModule.__GetAttributeByName(frame, name), name).toBe(json);
    }
    // The typed copy a node query's `attribute` field reads is the object.
    const answer = elementModule.__BobcatQueryNodes({
      bobcat: "runtime",
      method: "nodeQuery",
      operation: "fields",
      token: {
        type: 2,
        identifier: String(__GetElementUniqueID(frame)),
        component_id: "",
        first_only: true,
        root_unique_id: undefined,
      },
      params: ["attribute"],
    });
    expect(answer).toEqual({
      status: { code: 0, data: "success" },
      data: { attribute: { data, "global-props": data } },
    });
    // Any other element has no such property, and no frame to tell.
    mock.calls.length = 0;
    __SetAttribute(view, "data", data);
    __SetAttribute(view, "global-props", data);
    expect(mock.calls.filter(call => call[0] !== "tagName")).toEqual([
      ["setAttribute", 4, "data", "[object Object]"],
      ["setAttribute", 4, "global-props", "[object Object]"],
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
    name,
    bubbles,
    payload.timestamp ?? 0,
    nodes.length,
    ...nodes.flatMap((node, index) => [node, targeted[index] ?? node]),
    ...extraArguments(payload),
    ...detailArguments(payload),
  );
}

/** The host's detail kinds, mirrored from `main/runtime/lib.rs`. */
const DETAIL_POSITION = 0;
const DETAIL_EMPTY = 2;
const DETAIL_EXPOSURE = 3;
const DETAIL_KEY = 4;
const DETAIL_INPUT = 5;
const DETAIL_COMPOSITION = 6;
const DETAIL_FOCUS = 7;
const DETAIL_CLONED = 8;
const DETAIL_DISPATCHED = 9;

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
  /** A pointer event's W3C `button`, `buttons`, `pointerId`, `pointerType`. */
  button?: number;
  buttons?: number;
  pointerId?: number;
  pointerType?: string;
  /** The extras channel: name/value pairs the realm makes the `detail` of. */
  extras?: [string, unknown][];
  touchNumbers?: number[];
  timestamp?: number;
  /**
   * An image `load`'s intrinsic size: extras `width` and `height`, and the
   * `DETAIL_EMPTY` kind, as the host sends it.
   */
  width?: number;
  height?: number;
  /** An image `error`: the `DETAIL_EMPTY` kind, which spends no numbers. */
  empty?: boolean;
  /** A `uiappear`/`uidisappear`: the `DETAIL_EXPOSURE` kind, no numbers. */
  exposure?: boolean;
  /**
   * A `keydown`/`keyup`: the `DETAIL_KEY` kind — the two strings, `repeat`
   * and the modifier bitmask (alt 1, ctrl 2, meta 4, shift 8).
   */
  key?: { key: string; code: string; repeat: boolean; modifiers: number };
  /**
   * A `beforeinput`/`input`: the `DETAIL_INPUT` kind — `inputType`, `data`
   * (a string or null) and `isComposing`.
   */
  input?: { inputType: string; data: string | null; isComposing: boolean };
  /** A `composition*` event: the `DETAIL_COMPOSITION` kind — `data`. */
  composition?: string;
  /**
   * A focus event: the `DETAIL_FOCUS` kind — the related element's node id,
   * or null.
   */
  related?: number | null;
}

/**
 * The extras channel as the host sends it: the count, then each name and its
 * value. An image `load`'s size crosses here.
 */
function extraArguments(payload: DispatchPayload): [number, ...unknown[]] {
  const extras: [string, unknown][] = payload.extras ??
    (payload.width === undefined
      ? []
      : [["width", payload.width], ["height", payload.height]]);
  return [extras.length, ...extras.flat()];
}

/**
 * The kind and the numbers behind it, as the host sends them. A tuple, so the
 * spread at the call site still fills the `detailKind` parameter.
 */
function detailArguments(payload: DispatchPayload): [number, ...unknown[]] {
  if (payload.empty === true) {
    return [DETAIL_EMPTY];
  }
  if (payload.exposure === true) {
    return [DETAIL_EXPOSURE];
  }
  if (payload.width !== undefined || payload.extras !== undefined) {
    return [DETAIL_EMPTY];
  }
  if (payload.key !== undefined) {
    const { key, code, repeat, modifiers } = payload.key;
    return [DETAIL_KEY, key, code, repeat, modifiers];
  }
  if (payload.input !== undefined) {
    const { inputType, data, isComposing } = payload.input;
    return [DETAIL_INPUT, inputType, data, isComposing];
  }
  if (payload.composition !== undefined) {
    return [DETAIL_COMPOSITION, payload.composition];
  }
  if (payload.related !== undefined) {
    return [DETAIL_FOCUS, payload.related];
  }
  return [
    DETAIL_POSITION,
    payload.x ?? 0,
    payload.y ?? 0,
    payload.deltaX,
    payload.deltaY,
    payload.button,
    payload.buttons,
    payload.pointerId,
    payload.pointerType,
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
  it("tells the host each element's first registration for a name, once", () => {
    const { outer, inner } = tree();

    __AddEventListener(inner, "tap", () => {}, {});
    __AddEventListener(inner, "tap", () => {}, {});
    __AddEventListener(outer, "tap", () => {}, {});

    // Per (element, name), with the passes it runs in: a second bubble
    // closure on `inner` changes nothing the host was told.
    expect(mock.named("listenerOpened")).toEqual([
      ["listenerOpened", __GetElementUniqueID(inner), "tap", 1],
      ["listenerOpened", __GetElementUniqueID(outer), "tap", 1],
    ]);
  });

  it("closes an element's name when its own last registration for it goes", () => {
    const { outer, inner } = tree();
    const a = () => {};
    const b = () => {};
    __AddEventListener(inner, "tap", a, {});
    __AddEventListener(inner, "tap", b, {});
    __AddEventListener(outer, "tap", a, {});

    __RemoveEventListener(inner, "tap", a, {});
    expect(mock.named("listenerClosed")).toEqual([]);
    __RemoveEventListener(inner, "tap", b, {});
    expect(mock.named("listenerClosed")).toEqual([
      ["listenerClosed", __GetElementUniqueID(inner), "tap"],
    ]);

    __RemoveEventListener(outer, "tap", a, {});
    expect(mock.named("listenerClosed")).toEqual([
      ["listenerClosed", __GetElementUniqueID(inner), "tap"],
      ["listenerClosed", __GetElementUniqueID(outer), "tap"],
    ]);
  });

  it("reports the union of the passes, and reports it again when it changes", () => {
    const { inner } = tree();
    const id = __GetElementUniqueID(inner);
    const handler = () => {};

    __AddEventListener(inner, "tap", handler, {});
    __AddEventListener(inner, "tap", handler, { capture: true });
    // Capture is part of the identity, so a bubble removal leaves the capture
    // registration: the kinds narrow to the capture pass.
    __RemoveEventListener(inner, "tap", handler, {});
    expect(mock.named("listenerOpened")).toEqual([
      ["listenerOpened", id, "tap", 1],
      ["listenerOpened", id, "tap", 3],
      ["listenerOpened", id, "tap", 2],
    ]);
    expect(mock.named("listenerClosed")).toEqual([]);

    __RemoveEventListener(inner, "tap", handler, { capture: true });
    expect(mock.named("listenerClosed")).toEqual([
      ["listenerClosed", id, "tap"],
    ]);
  });

  it("files each __AddEvent form under its kind: bind and catch 1, capture 2, global 4", () => {
    const { inner } = tree();
    const id = __GetElementUniqueID(inner);
    const forms: [string, number][] = [
      ["bindEvent", 1],
      ["catchEvent", 1],
      ["capture-bind", 2],
      ["capture-catch", 2],
      ["global-bindEvent", 4],
    ];
    for (const [form, kinds] of forms) {
      mock.calls.length = 0;
      __AddEvent(inner, form, "tap", "3:0:handler");
      expect(mock.named("listenerOpened"), form).toEqual([
        ["listenerOpened", id, "tap", kinds],
      ]);
      __AddEvent(inner, form, "tap", null);
      expect(mock.named("listenerClosed"), form).toEqual([
        ["listenerClosed", id, "tap"],
      ]);
    }

    // The forms or together across the two slots and the closures: a worklet
    // capture, a global string beside it, and a bubble closure.
    mock.calls.length = 0;
    __AddEvent(inner, "capture-bind", "tap", { type: "worklet", value: {} });
    __AddEvent(inner, "global-bindEvent", "tap", "3:0:global");
    __AddEventListener(inner, "tap", () => {}, {});
    expect(mock.named("listenerOpened")).toEqual([
      ["listenerOpened", id, "tap", 2],
      ["listenerOpened", id, "tap", 6],
      ["listenerOpened", id, "tap", 7],
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
    expect(mock.named("listenerClosed")).toEqual([
      ["listenerClosed", __GetElementUniqueID(inner), "tap"],
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
    expect(mock.named("listenerOpened")).toEqual([]);
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
      "tap",
      true,
      0,
      4,
      ...[inner, outer, 9999, page].flatMap((step) => [
        typeof step === "number" ? step : __GetElementUniqueID(step),
        __GetElementUniqueID(inner),
      ]),
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
        "tap",
        true,
        0,
        1,
        uid,
        999,
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

    // Nothing about the stop crosses the boundary: the walk it ends is here,
    // and all the host ever heard is the two registrations.
    expect(order).toEqual(["inner"]);
    expect(mock.calls.filter(([name]) => String(name).startsWith("listener")))
      .toHaveLength(2);
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
    const id = papi.__GetElementUniqueID(view);
    expect(mock.named("listenerOpened")).toEqual([
      ["listenerOpened", id, "tap", 1],
      ["listenerOpened", id, "swipe", 4],
    ]);
    const held = registered.at(-1)?.held;

    // What a collection does: the cleanup gets the record the handle left.
    // It tells the host nothing about the registrations — the host drops a
    // freed element's rows itself — and frees the element.
    cleanup(held as never);

    expect(mock.named("listenerClosed")).toEqual([]);
    expect(mock.named("dropElement")).toEqual([["dropElement", id]]);
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
    expect(mock.named("listenerClosed")).toEqual([
      ["listenerClosed", __GetElementUniqueID(inner), "tap"],
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
    // the host hears the element's new kinds, with no close between.
    order.length = 0;
    __AddEvent(outer, "bindEvent", "tap", worklet(() => order.push("outer")));

    dispatch([inner, outer], "tap");
    expect(order).toEqual(["inner", "outer"]);
    expect(mock.named("listenerOpened")).toEqual([
      ["listenerOpened", __GetElementUniqueID(inner), "tap", 1],
      ["listenerOpened", __GetElementUniqueID(outer), "tap", 2],
      ["listenerOpened", __GetElementUniqueID(outer), "tap", 1],
    ]);
    expect(mock.named("listenerClosed")).toEqual([]);
  });

  it("shares an element's report with __AddEventListener, and neither closes it for the other", () => {
    const { inner } = tree();
    const id = __GetElementUniqueID(inner);
    const callback = () => {};

    __AddEventListener(inner, "tap", callback, {});
    __AddEvent(inner, "bindEvent", "tap", "3:0:bindtap");
    // Both run on the bubble pass, so the second registration changes
    // nothing the host was told.
    expect(mock.named("listenerOpened")).toEqual([
      ["listenerOpened", id, "tap", 1],
    ]);

    __RemoveEventListener(inner, "tap", callback, {});
    expect(mock.named("listenerClosed")).toEqual([]);

    __AddEvent(inner, "bindEvent", "tap", null);
    expect(mock.named("listenerClosed")).toEqual([
      ["listenerClosed", id, "tap"],
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
    expect(mock.named("listenerOpened")).toEqual([]);
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
    expect(mock.named("listenerOpened")).toEqual([
      ["listenerOpened", uid, "tap", 1],
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
    // One report covers both kinds: both run on the bubble pass.
    expect(mock.named("listenerOpened")).toEqual([
      ["listenerOpened", __GetElementUniqueID(inner), "tap", 1],
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
    // passes, which is what the host is told.
    expect(__GetEvent(inner, "tap", "bindEvent")).toBe("3:0:bindtap");
    expect(__GetEvents(inner)).toEqual([
      { type: "bindevent", name: "tap", function: "3:0:bindtap" },
      { type: "capture-bind", name: "tap", function: handler },
    ]);
    expect(mock.named("listenerOpened")).toEqual([
      ["listenerOpened", __GetElementUniqueID(inner), "tap", 2],
      ["listenerOpened", __GetElementUniqueID(inner), "tap", 3],
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
    expect(mock.named("listenerClosed")).toEqual([
      ["listenerClosed", __GetElementUniqueID(inner), "tap"],
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
    // A global registration alone is reported, as its own kind: the host
    // calls this realm for every event of the name, whatever its path.
    expect(mock.named("listenerOpened")).toEqual([
      ["listenerOpened", __GetElementUniqueID(inner), "tap", 4],
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
    expect(mock.named("listenerOpened")).toEqual([
      ["listenerOpened", __GetElementUniqueID(inner), "tap", 4],
    ]);

    __AddEvent(inner, "global-bindEvent", "tap", undefined);

    expect(__GetEvents(inner)).toEqual([]);
    expect(mock.named("listenerClosed")).toEqual([
      ["listenerClosed", __GetElementUniqueID(inner), "tap"],
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

  // Native's shape: a wheel's delta and position are the event's own
  // members, as `WheelEvent`'s are, and its `detail` is empty.
  it("gives a wheel an empty detail and its members on the event", () => {
    const { inner } = tree();
    let seen: Record<string, unknown> | undefined;
    __AddEventListener(inner, "wheel", (event: Record<string, unknown>) => {
      seen = event;
    }, {});

    dispatch([inner], "wheel", { x: 5, y: 6, deltaX: 0, deltaY: 30 });

    expect(seen!["detail"]).toEqual({});
    for (
      const [member, value] of Object.entries({
        deltaX: 0,
        deltaY: 30,
        x: 5,
        y: 6,
        clientX: 5,
        clientY: 6,
        pageX: 5,
        pageY: 6,
      })
    ) {
      expect(seen![member], member).toBe(value);
    }
    expect("button" in seen!).toBe(false);
  });

  // The host's extras are the Lynx `detail`, whatever their names: an
  // `<image>` `load`'s `{width, height}`, web-core's
  // `naturalWidth`/`naturalHeight` and not the box the bitmap drew into.
  it("makes the host's extras the detail", () => {
    const { inner } = tree();

    const detail = detailOf(inner, "load", { width: 40, height: 20 });

    expect(detail).toEqual({ width: 40, height: 20 });
    expect(Object.keys(detail)).toEqual(["width", "height"]);
  });

  it("makes any extras the detail, numbers and booleans alike, over the payload", () => {
    const { inner } = tree();

    const detail = detailOf(inner, "scroll", {
      extras: [["scrollTop", 120], ["deltaY", 20], ["isDragging", false]],
    });
    expect(detail).toEqual({ scrollTop: 120, deltaY: 20, isDragging: false });

    // A position payload gives way to them too: the extras are the detail.
    const positioned = detailOf(inner, "tap", {
      x: 1,
      y: 2,
      extras: [["custom", 7]],
    });
    expect(positioned).toEqual({ custom: 7 });
  });

  // A component's string detail (`<x-markdown>`'s `link`) crosses as string
  // extras, and a key the component had no value for is not sent at all, so
  // the detail has no such key rather than an `undefined`-valued one.
  it("makes string extras the detail and has no key the host did not send", () => {
    const { inner } = tree();

    const link = detailOf(inner, "link", {
      extras: [["url", "https://lynxjs.org/"], ["content", "Lynx"], ["contentId", "doc-1"]],
    });
    expect(link).toEqual({ url: "https://lynxjs.org/", content: "Lynx", contentId: "doc-1" });

    const bare = detailOf(inner, "link", {
      extras: [["url", ""], ["content", ""]],
    });
    expect(Object.keys(bare)).toEqual(["url", "content"]);
  });

  // An `x-input`'s `input`: a string extra crosses as itself, beside the
  // numbers and the boolean, in the host's order.
  it("makes string extras part of the detail", () => {
    const { inner } = tree();

    const detail = detailOf(inner, "input", {
      extras: [
        ["value", "a,b:c"],
        ["textLength", 5],
        ["cursor", 5],
        ["isComposing", false],
        ["selectionStart", 5],
        ["selectionEnd", 5],
      ],
    });
    expect(detail).toStrictEqual({
      value: "a,b:c",
      textLength: 5,
      cursor: 5,
      isComposing: false,
      selectionStart: 5,
      selectionEnd: 5,
    });
    expect(Object.keys(detail)).toEqual([
      "value", "textLength", "cursor", "isComposing", "selectionStart", "selectionEnd",
    ]);
  });

  // web-core's `error` detail exactly: no keys, and no numbers spent to say
  // so.
  it("builds an empty detail out of the kind that spends no numbers", () => {
    const { inner } = tree();

    expect(detailOf(inner, "error", { empty: true })).toEqual({});
  });

  // web-core's `ExposureEventDetail` (`ExposureServices.ts:202-208`), read off
  // the event's own target: the kind spends no numbers.
  it("builds an exposure detail out of the target's own attributes", () => {
    const { outer, inner } = tree();
    __SetAttribute(inner, "exposure-id", "card-7");
    __SetAttribute(inner, "exposure-scene", "feed");

    expect(detailOf(inner, "uiappear", { exposure: true })).toEqual({
      "unique-id": __GetElementUniqueID(inner),
      exposureID: "card-7",
      exposureScene: "feed",
      "exposure-id": "card-7",
      "exposure-scene": "feed",
    });
    // An element with only a listener has no id, and an absent scene is "".
    expect(detailOf(outer, "uidisappear", { exposure: true })).toEqual({
      "unique-id": __GetElementUniqueID(outer),
      exposureID: null,
      exposureScene: "",
      "exposure-id": null,
      "exposure-scene": "",
    });
  });

  it("publishes the exposure detail to a background handler", () => {
    const { inner } = tree();
    __SetAttribute(inner, "exposure-id", "card-7");
    __AddEvent(inner, "bindEvent", "uiappear", "3:0:binduiappear");

    dispatch([inner], "uiappear", { exposure: true }, undefined, false);

    const published = mock.named("publishEvent");
    expect(published).toHaveLength(1);
    expect((published[0]![3] as { detail: unknown }).detail).toEqual({
      "unique-id": __GetElementUniqueID(inner),
      exposureID: "card-7",
      exposureScene: "",
      "exposure-id": "card-7",
      "exposure-scene": "",
    });
  });
});

describe("pointer, mouse and click events", () => {
  /** The W3C members a pointer-shaped event carries on itself. */
  type MouseEventValues = Record<string, unknown> & {
    detail: Record<string, unknown>;
  };

  function delivered(name: string, payload: DispatchPayload): MouseEventValues {
    const { inner } = tree();
    let seen: MouseEventValues | undefined;
    __AddEventListener(inner, name, (event: MouseEventValues) => {
      seen = event;
    }, {});
    dispatch([inner], name, payload);
    expect(seen, "the listener ran").toBeDefined();
    return seen!;
  }

  it("puts web-core's mouse members on a compatibility mouse event", () => {
    const event = delivered("mousedown", { x: 12, y: 30, button: 0, buttons: 1 });

    // The `detail` keeps the `{x, y}` every pointer event has; the buttons
    // never enter it.
    expect(event.detail).toEqual({ x: 12, y: 30 });
    expect(event["button"]).toBe(0);
    expect(event["buttons"]).toBe(1);
    expect(event["x"]).toBe(12);
    expect(event["y"]).toBe(30);
    expect(event["clientX"]).toBe(12);
    expect(event["clientY"]).toBe(30);
    expect(event["pageX"]).toBe(12);
    expect(event["pageY"]).toBe(30);
  });

  it("gives every pointer-shaped event the mouse members, by shape not name", () => {
    for (const name of ["click", "dblclick", "tap", "longpress", "mouseup"]) {
      const event = delivered(name, {
        x: 4,
        y: 5,
        button: 0,
        buttons: 0,
        pointerId: 3,
        pointerType: "touch",
      });

      expect(event.detail, name).toEqual({ x: 4, y: 5 });
      expect(event["button"], name).toBe(0);
      expect(event["buttons"], name).toBe(0);
      expect(event["x"], name).toBe(4);
      expect(event["clientX"], name).toBe(4);
      expect(event["pageY"], name).toBe(5);
      expect("pointerId" in event, `${name} is no PointerEvent`).toBe(false);
    }
  });

  it("gives a pointer event its pointerId and pointerType as well", () => {
    const event = delivered("pointerdown", {
      x: 1,
      y: 2,
      button: 0,
      buttons: 1,
      pointerId: 7,
      pointerType: "pen",
    });

    expect(event.detail).toEqual({ x: 1, y: 2 });
    expect(event["pointerId"]).toBe(7);
    expect(event["pointerType"]).toBe("pen");
    expect(event["buttons"]).toBe(1);
    expect(event["clientY"]).toBe(2);
  });

  it("gives a touch event its lists and no mouse members", () => {
    const event = delivered("touchstart", {
      x: 1,
      y: 2,
      touchNumbers: [1, 1, 2, 7],
    });

    expect(event.detail).toEqual({ x: 1, y: 2 });
    expect(event["touches"]).toHaveLength(1);
    for (const member of ["button", "clientX", "pointerId"]) {
      expect(member in event, member).toBe(false);
    }
  });

  it("publishes a click's mouse members to a background handler", () => {
    const { inner } = tree();
    __AddEvent(inner, "bindEvent", "click", "3:0:bindclick");

    dispatch([inner], "click", { x: 7, y: 8, button: 0, buttons: 0 });

    const published = mock.named("publishEvent");
    expect(published).toHaveLength(1);
    const event = published[0]![3] as Record<string, unknown>;
    expect(event["detail"]).toEqual({ x: 7, y: 8 });
    expect(event["button"]).toBe(0);
    expect(event["clientX"]).toBe(7);
  });
});

describe("key events", () => {
  /** The `KeyboardEvent` members a key event carries on itself. */
  interface KeyEvent {
    detail: Record<string, unknown>;
    key?: string;
    code?: string;
    repeat?: boolean;
    altKey?: boolean;
    ctrlKey?: boolean;
    metaKey?: boolean;
    shiftKey?: boolean;
  }

  it("puts the key members on the event and leaves the detail empty", () => {
    const { inner } = tree();
    const seen: KeyEvent[] = [];
    __AddEventListener(inner, "keydown", (event: KeyEvent) => {
      seen.push(event);
    }, {});

    dispatch([inner], "keydown", {
      key: { key: "A", code: "KeyA", repeat: true, modifiers: 2 | 8 },
    });

    expect(seen).toHaveLength(1);
    const event = seen[0]!;
    expect(event.detail).toEqual({});
    expect(event.key).toBe("A");
    expect(event.code).toBe("KeyA");
    expect(event.repeat).toBe(true);
    expect(event.altKey).toBe(false);
    expect(event.ctrlKey).toBe(true);
    expect(event.metaKey).toBe(false);
    expect(event.shiftKey).toBe(true);
  });

  it("gives an event of another kind none of the key members", () => {
    const { inner } = tree();
    let seen: object | undefined;
    __AddEventListener(inner, "tap", (event: object) => {
      seen = event;
    }, {});

    dispatch([inner], "tap", { x: 1, y: 2 });

    expect(seen).toBeDefined();
    for (const member of ["key", "code", "repeat", "altKey", "shiftKey"]) {
      expect(member in seen!, member).toBe(false);
    }
  });

  it("publishes the key members to a background handler", () => {
    const { inner } = tree();
    __AddEvent(inner, "bindEvent", "keyup", "3:0:bindkeyup");

    dispatch([inner], "keyup", {
      key: { key: "Enter", code: "Enter", repeat: false, modifiers: 4 },
    });

    const published = mock.named("publishEvent");
    expect(published).toHaveLength(1);
    const event = published[0]![3] as Record<string, unknown>;
    expect(event["type"]).toBe("keyup");
    expect(event["detail"]).toEqual({});
    expect(event["key"]).toBe("Enter");
    expect(event["code"]).toBe("Enter");
    expect(event["repeat"]).toBe(false);
    expect(event["metaKey"]).toBe(true);
    expect(event["altKey"]).toBe(false);
  });
});

describe("editing events", () => {
  /** The members the editing events carry on themselves. */
  interface EditingEvent {
    type: string;
    detail: Record<string, unknown>;
    inputType?: string;
    data?: string | null;
    isComposing?: boolean;
    relatedTarget?: { uid: number } | null;
  }

  it("puts InputEvent's members on a beforeinput and leaves the detail empty", () => {
    const { inner } = tree();
    const seen: EditingEvent[] = [];
    __AddEventListener(inner, "beforeinput", (event: EditingEvent) => {
      seen.push(event);
    }, {});

    dispatch([inner], "beforeinput", {
      input: { inputType: "insertText", data: "a", isComposing: false },
    });
    dispatch([inner], "beforeinput", {
      input: { inputType: "deleteContentBackward", data: null, isComposing: true },
    });

    expect(seen).toHaveLength(2);
    expect(seen[0]!.detail).toEqual({});
    expect(seen[0]!.inputType).toBe("insertText");
    expect(seen[0]!.data).toBe("a");
    expect(seen[0]!.isComposing).toBe(false);
    expect(seen[1]!.inputType).toBe("deleteContentBackward");
    expect(seen[1]!.data).toBeNull();
    expect(seen[1]!.isComposing).toBe(true);
  });

  it("puts CompositionEvent.data on a composition event", () => {
    const { inner } = tree();
    const seen: EditingEvent[] = [];
    __AddEventListener(inner, "compositionupdate", (event: EditingEvent) => {
      seen.push(event);
    }, {});

    dispatch([inner], "compositionupdate", { composition: "にほ" });

    expect(seen).toHaveLength(1);
    expect(seen[0]!.data).toBe("にほ");
    expect(seen[0]!.detail).toEqual({});
    expect("inputType" in seen[0]!).toBe(false);
  });

  it("resolves a focus event's relatedTarget to its descriptor, or null", () => {
    const { outer, inner } = tree();
    const seen: EditingEvent[] = [];
    __AddEventListener(inner, "focus", (event: EditingEvent) => {
      seen.push(event);
    }, {});

    dispatch([inner], "focus", { related: __GetElementUniqueID(outer) }, undefined, false);
    dispatch([inner], "focus", { related: null }, undefined, false);
    dispatch([inner], "focus", { related: 999_999 }, undefined, false);

    expect(seen.map((event) => event.relatedTarget?.uid ?? null)).toEqual([
      __GetElementUniqueID(outer),
      null,
      null,
    ]);
    expect(seen[0]!.detail).toEqual({});
  });

  // The 2026-08-17 ruling stands: no event is cancelable from script.
  it("carries no preventDefault, cancelable or defaultPrevented", () => {
    const { inner } = tree();
    const seen: Record<string, unknown>[] = [];
    __AddEventListener(inner, "beforeinput", (event: Record<string, unknown>) => {
      seen.push(event);
    }, {});

    const answer = elementModule.__BobcatDispatchEvent(
      "beforeinput",
      true,
      0,
      1,
      __GetElementUniqueID(inner),
      __GetElementUniqueID(inner),
      0,
      DETAIL_INPUT,
      "insertText",
      "x",
      false,
    );

    expect(answer).toBeUndefined();
    expect(seen).toHaveLength(1);
    for (const member of ["preventDefault", "cancelable", "defaultPrevented"]) {
      expect(member in seen[0]!, member).toBe(false);
    }
    expect(seen[0]!["inputType"]).toBe("insertText");
  });

  it("publishes the editing members to a background handler", () => {
    const { outer, inner } = tree();
    __AddEvent(inner, "bindEvent", "input", "3:0:bindinput");
    __AddEvent(inner, "bindEvent", "focus", "3:0:bindfocus");

    dispatch([inner], "input", {
      input: { inputType: "insertText", data: "x", isComposing: true },
    });
    dispatch([inner], "focus", { related: __GetElementUniqueID(outer) }, undefined, false);

    const published = mock.named("publishEvent");
    expect(published).toHaveLength(2);
    const input = published[0]![3] as Record<string, unknown>;
    expect(input["inputType"]).toBe("insertText");
    expect(input["data"]).toBe("x");
    expect(input["isComposing"]).toBe(true);
    for (const member of ["preventDefault", "cancelable", "defaultPrevented"]) {
      expect(member in input, member).toBe(false);
    }
    const focus = published[1]![3] as Record<string, unknown>;
    expect(focus["relatedTarget"]).toEqual({
      dataset: {},
      id: null,
      uid: __GetElementUniqueID(outer),
    });
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
      "tap",
      undefined,
      0,
      1,
      __GetElementUniqueID(inner),
      __GetElementUniqueID(inner),
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

/**
 * Has the mock's `dispatchEvent` do what the host does when the listener
 * table invites this realm to its own synthetic event: call
 * `__BobcatDispatchEvent` once, with the event's name, `bubbles` and
 * timestamp, the path `path` (target first) and, position for position,
 * `targets` — every step its own target unless given — no extras, and the
 * `DETAIL_DISPATCHED` kind, which walks the event `__DispatchEvent` left
 * pending. `before` runs first, inside the host's call, standing for the
 * realms the host calls before this one. What the walk throws is kept in
 * the answer rather than thrown, as the host reports a listener's failure
 * instead of throwing it into the dispatching realm.
 */
function callsBack(path: object[], targets?: object[], before?: () => void): unknown[] {
  const nodes = path.map((handle) => __GetElementUniqueID(handle));
  const targeted = targets === undefined
    ? nodes.map(() => nodes[0])
    : targets.map((handle) => __GetElementUniqueID(handle));
  const reported: unknown[] = [];
  mock.answerDispatch = (_node, eventName, bubbles, _composed, timestamp) => {
    before?.();
    try {
      elementModule.__BobcatDispatchEvent(
        eventName,
        bubbles,
        timestamp,
        nodes.length,
        ...nodes.flatMap((node, index) => [node, targeted[index] ?? node]),
        0,
        DETAIL_DISPATCHED,
      );
    } catch (error) {
      reported.push(error);
    }
  };
  return reported;
}

describe("__CreateEvent and __DispatchEvent", () => {
  it("creates an event in no dispatch yet, stamped on the view's timeline", () => {
    mock.timeline = 1234.5;
    const detail = { item: 3 };
    const event = __CreateEvent(0, "MyEvent", {}, detail) as {
      type: string;
      eventPhase: number;
      target: unknown;
      currentTarget: unknown;
      detail: unknown;
      timestamp: number;
      params: unknown;
      stopPropagation: unknown;
      stopImmediatePropagation: unknown;
    };

    // The shape every listener here is handed, with nothing else
    // enumerable: the dispatch state is hidden.
    expect(Object.keys(event)).toEqual([
      "type",
      "eventPhase",
      "target",
      "currentTarget",
      "detail",
      "timestamp",
      "params",
      "stopPropagation",
      "stopImmediatePropagation",
    ]);
    expect(event.type).toBe("myevent");
    expect(event.eventPhase).toBe(0);
    expect(event.target).toBeNull();
    expect(event.currentTarget).toBeNull();
    expect(event.detail).toBe(detail);
    expect(event.timestamp).toBe(1234.5);
    expect(event.params).toEqual({});
    expect(event.stopPropagation).toBeTypeOf("function");
    expect(event.stopImmediatePropagation).toBeTypeOf("function");
    // Asked once, at creation, and nothing else of the host.
    expect(mock.calls).toEqual([]);
  });

  it("gives a nullish detail an empty object and refuses one that is no object", () => {
    expect((__CreateEvent(0, "a", undefined, undefined) as { detail: unknown }).detail)
      .toEqual({});
    expect((__CreateEvent(0, "a", null, null) as { detail: unknown }).detail).toEqual({});
    expect(() => __CreateEvent(0, "a", {}, 3)).toThrow(TypeError);
    expect(() => __CreateEvent(0, "a", {}, "detail")).toThrow(TypeError);
    expect(() => __CreateEvent(0, "a", true, {})).toThrow(TypeError);
  });

  it("hands the host the lowercased name, both flags and the creation timestamp", () => {
    const { inner } = tree();
    mock.timeline = 50;
    const detail = { item: 3 };
    const bubbling = __CreateEvent(1, "Custom", { bubbles: true, composed: true }, detail);
    // `capture` and `cancelable` are accepted and change nothing.
    const plain = __CreateEvent(0, "custom", { capture: true, cancelable: true }, null);
    mock.timeline = 99;

    expect(__DispatchEvent(inner, bubbling)).toBe(true);
    expect(__DispatchEvent(inner, plain)).toBe(true);

    const uid = __GetElementUniqueID(inner);
    // The `detail` goes as the sixth argument, the event's own object, for
    // the host to clone into the other realms it calls.
    expect(mock.named("dispatchEvent")).toEqual([
      ["dispatchEvent", uid, "custom", true, true, 50, detail],
      ["dispatchEvent", uid, "custom", false, false, 50, {}],
    ]);
    expect(mock.named("dispatchEvent")[0]?.[6]).toBe(detail);
  });

  it("hands the host the detail the event holds when it is dispatched", () => {
    const { inner } = tree();
    const event = __CreateEvent(0, "custom", {}, { first: true }) as ListenerEvent;
    const replaced = { second: true };
    (event as { detail: unknown }).detail = replaced;

    __DispatchEvent(inner, event);

    expect(mock.named("dispatchEvent")[0]?.[6]).toBe(replaced);
  });

  it("lets a throw at the host's call propagate, before any listener runs", () => {
    const { inner } = tree();
    const runs: string[] = [];
    __AddEventListener(inner, "custom", () => runs.push("inner"), {});
    callsBack([inner]);
    const callBack = mock.answerDispatch;
    // What the bridge does with a value its structured clone refuses.
    mock.answerDispatch = () => {
      throw new TypeError("unsupported object class");
    };
    const event = __CreateEvent(0, "custom", {}, { callback: () => 1 }) as ListenerEvent;

    expect(() => __DispatchEvent(inner, event)).toThrow(TypeError);
    expect(runs).toEqual([]);
    expect(event.eventPhase).toBe(0);
    // Nothing was left pending: a dispatch of another event walks that one.
    mock.answerDispatch = callBack;
    const next = __CreateEvent(0, "custom", {}, {});
    __AddEventListener(inner, "custom", (received: unknown) => {
      runs.push(received === next ? "next" : "stale");
    }, {});
    __DispatchEvent(inner, next);
    expect(runs).toEqual(["inner", "next"]);
  });

  it("makes another realm's cloned detail the event's detail, as it came", () => {
    const { page, outer, inner } = tree();
    const clone = { item: 3, nested: { list: [1, 2] } };
    const seen: unknown[] = [];
    __AddEventListener(outer, "custom", (event: ListenerEvent) => {
      seen.push(event.detail, event.target.uid, event.eventPhase);
    }, {});
    const nodes = [inner, outer, page].map((handle) => __GetElementUniqueID(handle));

    elementModule.__BobcatDispatchEvent(
      "custom",
      true,
      5,
      nodes.length,
      ...nodes.flatMap((node) => [node, nodes[0]]),
      0,
      DETAIL_CLONED,
      clone,
    );

    expect(seen).toEqual([clone, nodes[0], BUBBLING_PHASE]);
    expect(seen[0]).toBe(clone);
  });

  it("runs the capture, bubble and global passes over both registration forms", () => {
    const { page, outer, inner } = tree();
    const innerUid = __GetElementUniqueID(inner);
    const outerUid = __GetElementUniqueID(outer);
    const detail = { item: 3 };
    const order: string[] = [];
    __AddEventListener(page, "custom", (event: ListenerEvent) => {
      order.push(`page-capture:${event.eventPhase}`);
    }, { capture: true });
    __AddEventListener(inner, "custom", (event: ListenerEvent) => {
      order.push(`inner:${event.eventPhase}:${(event.detail as unknown) === detail}`);
    }, {});
    __AddEventListener(outer, "custom", (event: ListenerEvent) => {
      order.push(`outer:${event.eventPhase}:${event.target.uid}`);
    }, {});
    __AddEvent(outer, "bindEvent", "custom", "outer:custom");
    __AddEvent(page, "global-bindEvent", "custom", "page:global");
    mock.timeline = 7;
    const event = __CreateEvent(0, "custom", { bubbles: true }, detail) as ListenerEvent;
    callsBack([inner, outer, page]);
    mock.calls.length = 0;

    expect(__DispatchEvent(inner, event)).toBe(true);

    expect(order).toEqual([
      `page-capture:${CAPTURING_PHASE}`,
      `inner:${AT_TARGET}:true`,
      `outer:${BUBBLING_PHASE}:${innerUid}`,
    ]);
    // The string handlers publish the event's snapshot: the bubble one on
    // its step, before that node's closures, then the global one with no
    // phase.
    const target = { dataset: {}, id: null, uid: innerUid };
    expect(mock.named("publishEvent")).toEqual([
      ["publishEvent", undefined, "outer:custom", {
        type: "custom",
        eventPhase: BUBBLING_PHASE,
        target,
        currentTarget: { dataset: {}, id: null, uid: outerUid },
        detail,
        timestamp: 7,
        params: {},
      }],
      ["publishEvent", undefined, "page:global", {
        type: "custom",
        eventPhase: 0,
        target,
        currentTarget: { dataset: {}, id: null, uid: __GetElementUniqueID(page) },
        detail,
        timestamp: 7,
        params: {},
      }],
    ]);
    // The standard's last dispatch step: the target stays, the rest is reset.
    expect(event.eventPhase).toBe(0);
    expect(event.currentTarget).toBeNull();
    expect(event.target.uid).toBe(innerUid);
    expect(event.detail as unknown).toBe(detail);
  });

  it("narrows a non-bubbling event to its at-target steps and runs no global pass", () => {
    const { page, outer, inner } = tree();
    const order: string[] = [];
    __AddEventListener(page, "custom", () => order.push("page-capture"), { capture: true });
    __AddEventListener(inner, "custom", () => order.push("inner"), {});
    __AddEventListener(outer, "custom", () => order.push("outer"), {});
    __AddEvent(page, "global-bindEvent", "custom", "page:global");
    callsBack([inner, outer, page]);

    __DispatchEvent(inner, __CreateEvent(0, "custom", {}, {}));

    expect(order).toEqual(["page-capture", "inner"]);
    expect(mock.named("publishEvent")).toEqual([]);
  });

  it("honors the two stop members, and a re-dispatch starts with them clear", () => {
    const { outer, inner } = tree();
    const order: string[] = [];
    let stop: "propagation" | "immediate" | undefined = "propagation";
    __AddEventListener(inner, "custom", (event: ListenerEvent) => {
      order.push("inner-first");
      if (stop === "propagation") {
        __StopPropagation(event);
      } else if (stop === "immediate") {
        __StopImmediatePropagation(event);
      }
    }, {});
    __AddEventListener(inner, "custom", () => order.push("inner-second"), {});
    __AddEventListener(outer, "custom", () => order.push("outer"), {});
    callsBack([inner, outer]);
    const event = __CreateEvent(0, "custom", { bubbles: true }, {});

    __DispatchEvent(inner, event);
    expect(order).toEqual(["inner-first", "inner-second"]);

    order.length = 0;
    stop = "immediate";
    __DispatchEvent(inner, event);
    expect(order).toEqual(["inner-first"]);

    order.length = 0;
    stop = undefined;
    __DispatchEvent(inner, event);
    expect(order).toEqual(["inner-first", "inner-second", "outer"]);
  });

  it("refuses an event whose dispatch is running, and dispatches it again once it ends", () => {
    const { outer, inner } = tree();
    const thrown: unknown[] = [];
    let runs = 0;
    __AddEventListener(inner, "custom", (event: ListenerEvent) => {
      runs += 1;
      try {
        __DispatchEvent(outer, event);
      } catch (error) {
        thrown.push(error);
      }
    }, {});
    callsBack([inner]);
    const event = __CreateEvent(0, "custom", { bubbles: true }, {});

    __DispatchEvent(inner, event);
    expect(runs).toBe(1);
    expect(thrown).toHaveLength(1);
    expect(thrown[0]).toBeInstanceOf(TypeError);
    // The nested refusal asked the host for nothing.
    expect(mock.named("dispatchEvent")).toHaveLength(1);

    __DispatchEvent(inner, event);
    expect(runs).toBe(2);
    expect(mock.named("dispatchEvent")).toHaveLength(2);
  });

  it("ends the walk when a listener throws, which the host reports, and dispatches again", () => {
    const { inner } = tree();
    let fail = true;
    __AddEventListener(inner, "custom", () => {
      if (fail) {
        throw new Error("listener failed");
      }
    }, {});
    const reported = callsBack([inner]);
    const event = __CreateEvent(0, "custom", {}, {}) as ListenerEvent;

    // The throw leaves this realm's walk through the host's call, which
    // reports it; `__DispatchEvent` is not where it lands.
    expect(__DispatchEvent(inner, event)).toBe(true);
    expect(reported).toHaveLength(1);
    expect(String(reported[0])).toContain("listener failed");
    expect(event.eventPhase).toBe(0);
    expect(event.currentTarget).toBeNull();

    fail = false;
    expect(__DispatchEvent(inner, event)).toBe(true);
    expect(reported).toHaveLength(1);
  });

  it("does not walk the event itself: only the host's call for it runs the listeners", () => {
    const { outer, inner } = tree();
    const runs: string[] = [];
    __AddEventListener(inner, "custom", () => runs.push("inner"), {});
    const event = __CreateEvent(0, "custom", { bubbles: true }, {}) as ListenerEvent;

    // A host that did not call this realm: no registration of its on the
    // path, as the listener table says.
    expect(__DispatchEvent(inner, event)).toBe(true);
    expect(runs).toEqual([]);
    expect(event.target).toBeNull();
    expect(event.eventPhase).toBe(0);

    // Nothing is left pending: a `DETAIL_DISPATCHED` call with no dispatch
    // of this realm's running walks nothing.
    const uid = __GetElementUniqueID(inner);
    elementModule.__BobcatDispatchEvent(
      "custom",
      true,
      0,
      2,
      uid,
      uid,
      __GetElementUniqueID(outer),
      uid,
      0,
      DETAIL_DISPATCHED,
    );
    expect(runs).toEqual([]);
  });

  it("walks the very object it was given, so a stop on it ends that walk", () => {
    const { page, outer, inner } = tree();
    const seen: unknown[] = [];
    const event = __CreateEvent(0, "custom", { bubbles: true }, { item: 1 }) as ListenerEvent;
    __AddEventListener(inner, "custom", (received: ListenerEvent) => {
      seen.push(received === event, received.target.uid, received.eventPhase);
      __StopPropagation(event);
    }, {});
    __AddEventListener(outer, "custom", () => seen.push("outer"), {});
    callsBack([inner, outer, page]);

    expect(__DispatchEvent(inner, event)).toBe(true);

    expect(seen).toEqual([true, __GetElementUniqueID(inner), AT_TARGET]);
    expect(event.target.uid).toBe(__GetElementUniqueID(inner));
  });

  it("walks a nested dispatch's own object, and keeps the outer event's", () => {
    const { page, outer, inner } = tree();
    const seen: string[] = [];
    const outerEvent = __CreateEvent(0, "first", { bubbles: true }, { n: 1 });
    const nestedEvent = __CreateEvent(0, "second", { bubbles: true }, { n: 2 });
    __AddEventListener(inner, "first", (received: unknown) => {
      seen.push(`first-inner:${received === outerEvent}`);
      __DispatchEvent(inner, nestedEvent);
    }, {});
    __AddEventListener(inner, "second", (received: unknown) => {
      seen.push(`second-inner:${received === nestedEvent}`);
    }, {});
    __AddEventListener(outer, "first", (received: unknown) => {
      seen.push(`first-outer:${received === outerEvent}`);
    }, {});
    callsBack([inner, outer, page]);

    expect(__DispatchEvent(inner, outerEvent)).toBe(true);

    expect(seen).toEqual([
      "first-inner:true",
      "second-inner:true",
      "first-outer:true",
    ]);
  });

  it("keeps its pending event across another realm's call and a dispatch made before its own call", () => {
    const { page, outer, inner } = tree();
    const seen: string[] = [];
    const outerEvent = __CreateEvent(0, "first", { bubbles: true }, { n: 1 });
    const nestedEvent = __CreateEvent(0, "second", { bubbles: true }, { n: 2 });
    const uids = [inner, outer, page].map((handle) => __GetElementUniqueID(handle));
    __AddEventListener(inner, "first", (received: unknown) => {
      seen.push(`first:${received === outerEvent}`);
    }, {});
    __AddEventListener(inner, "second", (received: unknown) => {
      seen.push(`second:${received === nestedEvent}`);
    }, {});
    __AddEventListener(inner, "keyup", (received: ListenerEvent) => {
      seen.push(`keyup:${received !== outerEvent}:${JSON.stringify(received.detail)}`);
    }, {});
    let nesting = true;
    callsBack([inner, outer, page], undefined, () => {
      if (!nesting) {
        return;
      }
      nesting = false;
      // The realms the host calls before this one: one dispatches a `keyup`
      // that reaches this realm with a clone, and one reaches this realm's
      // code, which dispatches an event of its own.
      elementModule.__BobcatDispatchEvent(
        "keyup",
        true,
        0,
        uids.length,
        ...uids.flatMap((uid) => [uid, uids[0]]),
        0,
        DETAIL_CLONED,
        { k: 1 },
      );
      __DispatchEvent(inner, nestedEvent);
      nesting = true;
    });

    expect(__DispatchEvent(inner, outerEvent)).toBe(true);

    expect(seen).toEqual([
      'keyup:true:{"k":1}',
      "second:true",
      "first:true",
    ]);
  });

  it("refuses a value that is no handle, and an event __CreateEvent did not make", () => {
    const { inner } = tree();
    let routed: unknown;
    __AddEventListener(inner, "tap", (event: unknown) => {
      routed = event;
    }, {});
    dispatch([inner], "tap");
    const event = __CreateEvent(0, "custom", {}, {});

    expect(() => __DispatchEvent({}, event)).toThrow(TypeError);
    expect(() => __DispatchEvent(null, event)).toThrow(TypeError);
    expect(() => __DispatchEvent(inner, {})).toThrow(TypeError);
    // An event the host dispatched: the realm was never told whether it is
    // composed.
    expect(() => __DispatchEvent(inner, routed)).toThrow(TypeError);
    expect(mock.named("dispatchEvent")).toEqual([]);
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
    expect(mock.named("listenerClosed")).toEqual([
      ["listenerClosed", __GetElementUniqueID(inner), "tap"],
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

describe("exposure", () => {
  /** The listener table rows the realm reported since the calls were cleared. */
  const rows = () =>
    mock.calls.filter(([name]) => String(name).startsWith("listener"));

  /**
   * One exposure record as the host dispatches it: `bobcat:exposure` at the
   * element, not bubbling, with the id and scene the element was exposed
   * under and whether it appeared as extras.
   */
  const record = (
    element: object,
    exposureId: string,
    scene: string,
    appeared: boolean,
  ) =>
    dispatch([element], "bobcat:exposure", {
      extras: [
        ["exposureID", exposureId],
        ["exposureScene", scene],
        ["appeared", appeared],
      ],
    }, undefined, false);

  /** What `bobcat:runtime`'s exposure sender received, one entry per flush. */
  const sent = () =>
    mock.named("sendExposureRecords").map(([, exposure, disexposure]) => [
      exposure,
      disexposure,
    ]);

  /** Lets the promise job the first record scheduled run. */
  const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

  it("files the record listener beside an exposure handler, and closes it with the last", () => {
    const { outer, inner } = tree();
    const id = __GetElementUniqueID(inner);

    __AddEvent(inner, "bindEvent", "uiappear", "3:0:appear");
    // A handler of the other name, a worklet beside the string, and a
    // replacement in another form of the same kind.
    __AddEvent(inner, "bindEvent", "uidisappear", "3:0:disappear");
    __AddEvent(inner, "bindEvent", "uiappear", { type: "worklet", value: {} });
    __AddEvent(inner, "catchEvent", "uiappear", "3:0:replaced");
    __AddEvent(inner, "bindEvent", "uidisappear", null);
    __AddEvent(outer, "bindEvent", "tap", "3:0:tap");
    __AddEvent(inner, "bindEvent", "uiappear", undefined);

    // Whether the element is detected is the host's, read off its listener
    // table: the `bobcat:exposure` row registers it, and the `uiappear` row
    // has it hear its element events.
    expect(rows()).toEqual([
      ["listenerOpened", id, "uiappear", 1],
      ["listenerOpened", id, "bobcat:exposure", 1],
      ["listenerOpened", id, "uidisappear", 1],
      ["listenerClosed", id, "uidisappear"],
      ["listenerOpened", __GetElementUniqueID(outer), "tap", 1],
      ["listenerClosed", id, "uiappear"],
      ["listenerClosed", id, "bobcat:exposure"],
    ]);
  });

  it("files it for an __AddEventListener closure and a global form, in any case", () => {
    const { inner } = tree();
    const id = __GetElementUniqueID(inner);
    const listener = () => {};

    __AddEventListener(inner, "UIAppear", listener, {});
    __AddEvent(inner, "global-bindEvent", "uidisappear", "3:0:global");
    __RemoveEventListener(inner, "uiappear", listener, {});
    // An app removal under the record listener's own name names no
    // listener of the app's, and takes nothing away.
    __RemoveEventListener(inner, "bobcat:exposure", listener, {});
    __AddEvent(inner, "global-bindEvent", "uidisappear", null);

    expect(rows()).toEqual([
      ["listenerOpened", id, "uiappear", 1],
      ["listenerOpened", id, "bobcat:exposure", 1],
      ["listenerOpened", id, "uidisappear", 4],
      ["listenerClosed", id, "uiappear"],
      ["listenerClosed", id, "uidisappear"],
      ["listenerClosed", id, "bobcat:exposure"],
    ]);
  });

  it("tells nothing when __SetEvents hands an exposure handler back, and closes it when it drops the last", () => {
    const { inner } = tree();
    const id = __GetElementUniqueID(inner);
    __AddEvent(inner, "bindEvent", "uiappear", "3:0:appear");
    __AddEvent(inner, "bindEvent", "tap", "3:0:tap");
    mock.calls.length = 0;

    // The element before and after the call is what is reconciled, so a
    // round trip neither closes nor reopens anything.
    __SetEvents(inner, __GetEvents(inner));
    expect(mock.calls).toEqual([]);

    __SetEvents(inner, [{ type: "bindEvent", name: "tap", function: "3:0:tap" }]);
    expect(mock.named("listenerClosed")).toEqual([
      ["listenerClosed", id, "uiappear"],
      ["listenerClosed", id, "bobcat:exposure"],
    ]);
    expect(mock.named("listenerOpened")).toEqual([]);
    // Nothing an app reads lists the record listener.
    expect(__GetEvents(inner)).toEqual([
      { type: "bindevent", name: "tap", function: "3:0:tap" },
    ]);
  });

  it("files it while the exposure-id is non-empty, and closes it when the id goes", () => {
    const { inner } = tree();
    const id = __GetElementUniqueID(inner);

    __SetAttribute(inner, "exposure-id", "card-7");
    // A second write and a scene file nothing more.
    __SetAttribute(inner, "exposure-id", "card-8");
    __SetAttribute(inner, "exposure-scene", "feed");
    expect(rows()).toEqual([["listenerOpened", id, "bobcat:exposure", 1]]);

    __SetAttribute(inner, "exposure-id", "");
    expect(rows()).toEqual([
      ["listenerOpened", id, "bobcat:exposure", 1],
      ["listenerClosed", id, "bobcat:exposure"],
    ]);

    __SetAttribute(inner, "exposure-id", "card-9");
    __SetAttribute(inner, "exposure-id", null);
    expect(rows().slice(2)).toEqual([
      ["listenerOpened", id, "bobcat:exposure", 1],
      ["listenerClosed", id, "bobcat:exposure"],
    ]);
  });

  it("keeps the row of an exposed element until its disexposure arrives", async () => {
    const { inner } = tree();
    const id = __GetElementUniqueID(inner);
    __SetAttribute(inner, "exposure-id", "card-7");
    record(inner, "card-7", "", true);
    mock.calls.length = 0;

    // The host still has the element registered, and owes the disexposure
    // under the old id: the row stays for it.
    __SetAttribute(inner, "exposure-id", null);
    expect(rows()).toEqual([]);

    record(inner, "card-7", "", false);
    expect(rows()).toEqual([["listenerClosed", id, "bobcat:exposure"]]);
    await flush();
    expect(sent().map(([exposure, disexposure]) => [
      (exposure as unknown[]).length,
      (disexposure as { "exposure-id": string }[]).map((entry) => entry["exposure-id"]),
    ])).toEqual([[1, ["card-7"]]]);
  });

  it("keeps the row of an exposed element whose last handler goes, until its disexposure", () => {
    const { inner } = tree();
    const id = __GetElementUniqueID(inner);
    __AddEvent(inner, "bindEvent", "uiappear", "3:0:appear");
    record(inner, "card-7", "", true);
    mock.calls.length = 0;

    __AddEvent(inner, "bindEvent", "uiappear", null);
    expect(rows()).toEqual([["listenerClosed", id, "uiappear"]]);

    record(inner, "card-7", "", false);
    expect(rows()).toEqual([
      ["listenerClosed", id, "uiappear"],
      ["listenerClosed", id, "bobcat:exposure"],
    ]);
  });

  it("keeps the row through a disexposure of an element that still wants it", () => {
    const { inner } = tree();
    __SetAttribute(inner, "exposure-id", "card-7");
    record(inner, "card-7", "", true);
    mock.calls.length = 0;

    // `lynx.stopExposure()`: the element is no longer exposed, and still
    // wants its records.
    record(inner, "card-7", "", false);
    expect(rows()).toEqual([]);
  });

  it("sends one delivery's records as one exposure list, then one disexposure list", async () => {
    const { outer, inner } = tree();
    const outerUid = __GetElementUniqueID(outer);
    const innerUid = __GetElementUniqueID(inner);
    __SetID(inner, "row");
    __SetAttribute(inner, "data-row-index", "4");
    __AddDataset(inner, "typed", { nested: 1 });
    __SetAttribute(inner, "exposure-id", "card-7");
    __SetAttribute(outer, "exposure-id", "outer");
    record(outer, "outer", "", true);
    await flush();
    mock.calls.length = 0;

    // One delivery: a disexposure first, then an exposure. The id and scene
    // are the record's, not the element's attributes now.
    record(outer, "outer", "", false);
    record(inner, "card-7", "feed", true);
    // Nothing leaves before the delivery's checkpoint.
    expect(sent()).toEqual([]);
    await flush();

    const dataset = { rowIndex: "4", typed: { nested: 1 } };
    const target = { dataset, id: "row", uid: innerUid };
    const outerTarget = { dataset: {}, id: null, uid: outerUid };
    expect(sent()).toEqual([
      [
        [
          {
            dataset,
            "unique-id": innerUid,
            exposureID: "card-7",
            exposureScene: "feed",
            "exposure-id": "card-7",
            "exposure-scene": "feed",
            type: "exposure",
            target,
            currentTarget: target,
            detail: {
              "unique-id": 0,
              exposureID: "card-7",
              exposureScene: "feed",
              "exposure-id": "card-7",
              "exposure-scene": "feed",
            },
          },
        ],
        [
          {
            dataset: {},
            "unique-id": outerUid,
            exposureID: "outer",
            exposureScene: "",
            "exposure-id": "outer",
            "exposure-scene": "",
            type: "disexposure",
            target: outerTarget,
            currentTarget: outerTarget,
            detail: {
              "unique-id": 0,
              exposureID: "outer",
              exposureScene: "",
              "exposure-id": "outer",
              "exposure-scene": "",
            },
          },
        ],
      ],
    ]);
  });

  it("runs an after-records callback once the next flush scheduled after it has sent its lists", async () => {
    const { outer, inner } = tree();
    __SetAttribute(inner, "exposure-id", "card-7");
    __SetAttribute(outer, "exposure-id", "outer");
    const order: string[] = [];
    const sentSoFar = () => String(sent().length);

    // Nothing is queued: the callback waits for a flush that has not been
    // scheduled yet, and no time passing runs it.
    elementModule.__BobcatAfterExposureRecords(() => order.push(`first after ${sentSoFar()}`));
    await flush();
    expect(order).toEqual([]);

    // A flush already scheduled when a callback is filed is not the one it
    // waits for: that is the next one.
    record(outer, "outer", "", true);
    elementModule.__BobcatAfterExposureRecords(() => order.push(`second after ${sentSoFar()}`));
    await flush();
    expect(order).toEqual(["first after 1"]);

    record(inner, "card-7", "", false);
    await flush();
    expect(order).toEqual(["first after 1", "second after 2"]);
    // Each ran once.
    record(outer, "outer", "", false);
    await flush();
    expect(order).toHaveLength(2);
  });

  it("builds a detached element's record from its live handle", async () => {
    const { outer, inner } = tree();
    const uid = __GetElementUniqueID(inner);
    __SetID(inner, "gone");
    __AddDataset(inner, "key", "k");
    __SetAttribute(inner, "exposure-id", "card-7");
    __RemoveElement(outer, inner);

    record(inner, "card-7", "", false);
    await flush();

    const target = { dataset: { key: "k" }, id: "gone", uid };
    expect(sent()).toEqual([[[], [expect.objectContaining({
      dataset: { key: "k" },
      type: "disexposure",
      target,
      currentTarget: target,
    })]]]);
  });

  it("switches the elements this realm registered, by id, in the order it registered them", () => {
    const { page, outer, inner } = tree();
    __SetAttribute(inner, "exposure-id", "card-7");
    __AddEvent(outer, "bindEvent", "uiappear", "3:0:appear");
    // Neither an element with no exposure want nor the page is registered.
    __AddEvent(page, "bindEvent", "tap", "3:0:tap");
    const ids = `${__GetElementUniqueID(inner)},${__GetElementUniqueID(outer)}`;
    mock.calls.length = 0;

    // The host's answer, the records the stop queued, is the switch's.
    expect(elementModule.__BobcatSwitchExposure(false, true)).toBe(2);
    expect(elementModule.__BobcatSwitchExposure(false, false)).toBe(0);
    // A resume reads no `sendEvent`.
    expect(elementModule.__BobcatSwitchExposure(true, true)).toBe(0);
    expect(mock.named("switchExposure")).toEqual([
      ["switchExposure", 0, 1, ids],
      ["switchExposure", 0, 0, ids],
      ["switchExposure", 1, 0, ids],
    ]);

    // An element whose record listener went is no longer this realm's.
    __SetAttribute(inner, "exposure-id", null);
    mock.calls.length = 0;
    elementModule.__BobcatSwitchExposure(false, true);
    expect(mock.named("switchExposure")).toEqual([
      ["switchExposure", 0, 1, String(__GetElementUniqueID(outer))],
    ]);
  });

  it("stops an element registered while the realm is stopped right after filing its listener", () => {
    const { outer, inner } = tree();
    __SetAttribute(outer, "exposure-id", "outer");
    elementModule.__BobcatSwitchExposure(false, true);
    mock.calls.length = 0;

    __SetAttribute(inner, "exposure-id", "card-7");
    const id = __GetElementUniqueID(inner);
    // Registered with the host first, then stopped, before any update can
    // report it.
    expect(mock.calls.filter(([name]) =>
      name === "listenerOpened" || name === "switchExposure")).toEqual([
      ["listenerOpened", id, "bobcat:exposure", 1],
      ["switchExposure", 0, 0, String(id)],
    ]);

    // The resume lists it with the rest; one registered after it runs.
    elementModule.__BobcatSwitchExposure(true, false);
    expect(mock.named("switchExposure").at(-1)).toEqual(
      ["switchExposure", 1, 0, `${__GetElementUniqueID(outer)},${id}`],
    );
    mock.calls.length = 0;
    __AddEvent(__CreateView(0), "bindEvent", "uidisappear", "3:0:gone");
    expect(mock.named("switchExposure")).toEqual([]);
  });
});

describe("the dataset's data-* attributes", () => {
  /** The attribute writes the host received since the calls were cleared. */
  function attributeWrites() {
    return mock.calls.filter(
      (call) => call[0] === "setAttribute" || call[0] === "removeAttribute",
    );
  }

  it("reflects an __AddDataset entry as data-<key>", () => {
    const view = __CreateView(0);
    const id = __GetElementUniqueID(view);
    mock.calls.length = 0;

    __AddDataset(view, "test", "x");

    expect(attributeWrites()).toEqual([["setAttribute", id, "data-test", "x"]]);
    expect(__GetAttributeByName(view, "data-test")).toBe("x");
  });

  it("reflects every key of a __SetDataset and keeps the keys it does not name", () => {
    const view = __CreateView(0);
    const id = __GetElementUniqueID(view);
    mock.calls.length = 0;

    __SetDataset(view, { test: "x", kind: "row" });
    // Native's merge: `test` stays in the typed store, so its attribute
    // stays too. web-core replaces the dataset and would remove it.
    __SetDataset(view, { kind: "cell" });

    expect(attributeWrites()).toEqual([
      ["setAttribute", id, "data-test", "x"],
      ["setAttribute", id, "data-kind", "row"],
      ["setAttribute", id, "data-kind", "cell"],
    ]);
    expect(__GetDataset(view)).toEqual({ test: "x", kind: "cell" });
  });

  it("writes a non-string value as its string and an object as its JSON text", () => {
    const view = __CreateView(0);
    const id = __GetElementUniqueID(view);
    mock.calls.length = 0;

    __AddDataset(view, "count", 7);
    __AddDataset(view, "on", true);
    __SetDataset(view, { item: { id: 3, tags: ["a"] }, list: [1, 2] });

    expect(attributeWrites()).toEqual([
      ["setAttribute", id, "data-count", "7"],
      ["setAttribute", id, "data-on", "true"],
      ["setAttribute", id, "data-item", '{"id":3,"tags":["a"]}'],
      ["setAttribute", id, "data-list", "[1,2]"],
    ]);
    // The typed store is untouched by the reflection.
    expect(__GetDataset(view)).toEqual({
      count: 7,
      on: true,
      item: { id: 3, tags: ["a"] },
      list: [1, 2],
    });
  });

  it("removes the attribute of a null, undefined or otherwise falsy value", () => {
    for (const value of [null, undefined, false, 0, Number.NaN, ""]) {
      const added = __CreateView(0);
      const set = __CreateView(0);
      __AddDataset(added, "test", "x");
      __SetDataset(set, { test: "x" });
      mock.calls.length = 0;

      __AddDataset(added, "test", value);
      __SetDataset(set, { test: value });

      expect(attributeWrites()).toEqual([
        ["removeAttribute", __GetElementUniqueID(added), "data-test"],
        ["removeAttribute", __GetElementUniqueID(set), "data-test"],
      ]);
      expect(__GetAttributeByName(added, "data-test")).toBeNull();
      // The key itself stays, with the value it was given.
      expect(__GetDataset(added)).toEqual({ test: value });
      expect(Object.hasOwn(__GetDataset(set), "test")).toBe(true);
    }
  });

  it("crosses to the host only when the reflected text changed", () => {
    const view = __CreateView(0);
    const id = __GetElementUniqueID(view);
    __SetDataset(view, { test: "x", count: 1, item: { id: 3 }, off: null });
    mock.calls.length = 0;

    // ReactLynx's spread update hands over the whole dataset every time.
    __SetDataset(view, { test: "x", count: 1, item: { id: 3 }, off: undefined });
    __AddDataset(view, "test", "x");
    expect(attributeWrites()).toEqual([]);

    __SetDataset(view, { test: "y", count: 1, item: { id: 4 }, off: null });
    expect(attributeWrites()).toEqual([
      ["setAttribute", id, "data-test", "y"],
      ["setAttribute", id, "data-item", '{"id":4}'],
    ]);
  });

  it("always crosses for a key's first write, so a data-* attribute of that name is replaced", () => {
    const view = __CreateView(0);
    const id = __GetElementUniqueID(view);
    __SetAttribute(view, "data-test", "attribute");
    mock.calls.length = 0;

    __AddDataset(view, "test", undefined);

    expect(attributeWrites()).toEqual([["removeAttribute", id, "data-test"]]);
  });

  it("lowers ASCII uppercase in the attribute name and converts nothing else", () => {
    const view = __CreateView(0);
    const id = __GetElementUniqueID(view);
    mock.calls.length = 0;

    __SetDataset(view, { fooBar: "a", "foo-bar": "b", "É": "c" });

    expect(attributeWrites()).toEqual([
      ["setAttribute", id, "data-foobar", "a"],
      ["setAttribute", id, "data-foo-bar", "b"],
      ["setAttribute", id, "data-É", "c"],
    ]);
  });

  it("keeps a value JSON cannot carry, as an attribute with no text", () => {
    const view = __CreateView(0);
    const id = __GetElementUniqueID(view);
    const cycle: Record<string, unknown> = {};
    cycle["self"] = cycle;
    mock.calls.length = 0;

    expect(() => __AddDataset(view, "cycle", cycle)).not.toThrow();

    expect(attributeWrites()).toEqual([["setAttribute", id, "data-cycle", ""]]);
    const stored = __GetDataset(view)["cycle"] as Record<string, unknown>;
    expect(stored["self"]).toBe(stored);
  });

  it("answers an event's dataset with the typed values alone, under the keys they were filed with", () => {
    const { inner } = tree();
    __SetAttribute(inner, "data-item-name", "row");
    __SetDataset(inner, { fooBar: 1, "foo-bar": { nested: true }, plain: "text", off: false });
    const seen: unknown[] = [];
    __AddEventListener(inner, "tap", (event: ListenerEvent) => {
      seen.push(event.target.dataset);
    }, {});

    dispatch([inner], "tap");

    // Neither `foobar: "1"` (the attribute `data-foobar`) nor a string
    // `fooBar` (the attribute `data-foo-bar`, camelCased) is in it.
    expect(seen).toEqual([{
      itemName: "row",
      fooBar: 1,
      "foo-bar": { nested: true },
      plain: "text",
      off: false,
    }]);
  });

  it("keeps data-* attributes out of the attribute field of a node query", () => {
    const page = __CreatePage("card", 0);
    const view = __CreateView(0);
    __AppendElement(page, view);
    __SetAttribute(view, "role", "row");
    __SetDataset(view, { test: "x", count: 7 });
    const uid = __GetElementUniqueID(view);

    const answer = elementModule.__BobcatQueryNodes({
      bobcat: "runtime",
      method: "nodeQuery",
      operation: "fields",
      token: {
        type: 2,
        identifier: String(uid),
        component_id: "",
        first_only: true,
        root_unique_id: undefined,
      },
      params: ["attribute", "dataset"],
    });

    expect(answer).toEqual({
      status: { code: 0, data: "success" },
      data: { attribute: { role: "row" }, dataset: { test: "x", count: 7 } },
    });
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
    mock.answerElementMethod = () => styleRecord("0", "20", "0", "100", "50");
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
    // `boundingClientRect` reads no params, so none are serialized.
    expect(mock.named("callElementMethod")).toEqual([
      ["callElementMethod", __GetElementUniqueID(view), "boundingClientRect", ""],
    ]);
  });

  it("tells each status and a record apart", () => {
    const view = __CreateView(0);
    const answers: unknown[] = [];
    for (const answer of [4, 7, 8, 0, "1:0"]) {
      mock.answerElementMethod = () => answer;
      __InvokeUIMethod(view, "selectTab", {}, (result: unknown) => answers.push(result));
    }
    mock.answerElementMethod = () => styleRecord("0", "1", "2", "3", "4");
    __InvokeUIMethod(view, "boundingClientRect", {}, (result: unknown) => answers.push(result));
    expect(answers[0]).toStrictEqual({ code: 4, data: undefined });
    expect(answers[1]).toStrictEqual({ code: 7, data: undefined });
    expect(answers[2]).toStrictEqual({ code: 8, data: undefined });
    expect(answers[3]).toStrictEqual({ code: 0, data: undefined });
    // A string is always a record: the code field and here no data fields.
    expect(answers[4]).toStrictEqual({ code: 0, data: [] });
    // `boundingClientRect`'s record is the same envelope, its four fields
    // after the code.
    expect(answers[5]).toMatchObject({
      code: 0,
      data: { left: 1, top: 2, width: 3, height: 4, right: 4, bottom: 6 },
    });
  });

  it("reads a record's first field as the code and hands over the rest", () => {
    const view = __CreateView(0);
    // Fields that contain the delimiter and a character outside the BMP,
    // which the host counts in UTF-16 units as `splitRecord` reads them.
    mock.answerElementMethod = () => "1:0" + "3:a,b" + "0:" + "2:\u{1F600}";
    const callback = rstest.fn();

    __InvokeUIMethod(view, "getDetail", {}, callback);

    // `getDetail` has no decoder arm, so the fields arrive as they were
    // written.
    expect(callback.mock.calls[0]?.[0]).toStrictEqual({
      code: 0,
      data: ["a,b", "", "\u{1F600}"],
    });
  });

  it("builds a scroll-view's scrollBy and getScrollInfo data from their records", () => {
    const view = __CreateElement("scroll-view", 0);
    const answers: unknown[] = [];
    mock.answerElementMethod = () => "1:0" + "1:0" + "2:20" + "2:50" + "4:30.5";
    __InvokeUIMethod(view, "scrollBy", { offset: 50 }, (result: unknown) => answers.push(result));
    mock.answerElementMethod = () => "1:0" + "2:15" + "4:70.5" + "3:120" + "3:120";
    __InvokeUIMethod(view, "getScrollInfo", {}, (result: unknown) => answers.push(result));

    expect(answers[0]).toStrictEqual({
      code: 0,
      data: { consumedX: 0, consumedY: 20, unconsumedX: 50, unconsumedY: 30.5 },
    });
    expect(answers[1]).toStrictEqual({
      code: 0,
      data: { scrollX: 15, scrollY: 70.5, scrollRange: 120, maxScrollOffset: 120 },
    });
  });

  it("builds a list's getScrollContainerInfo and getVisibleCells data from their records", () => {
    const list = __CreateElement("list", 0);
    const answers: unknown[] = [];
    mock.answerElementMethod = () => "1:0" + "4:70.5" + "1:0" + "3:470" + "3:180";
    __InvokeUIMethod(list, "getScrollContainerInfo", {}, (result: unknown) => answers.push(result));
    mock.answerElementMethod = () =>
      "1:0" + "1:2"
      + "3:a,b" + "2:k0" + "1:0" + "1:0" + "3:-50" + "3:180" + "2:30"
      + "0:" + "0:" + "1:2" + "1:0" + "2:30" + "3:180" + "2:70";
    __InvokeUIMethod(list, "getVisibleCells", {}, (result: unknown) => answers.push(result));
    mock.answerElementMethod = () => "1:0" + "1:0";
    __InvokeUIMethod(list, "getVisibleCells", {}, (result: unknown) => answers.push(result));

    expect(answers[0]).toStrictEqual({
      code: 0,
      data: { scrollTop: 70.5, scrollLeft: 0, scrollHeight: 470, scrollWidth: 180 },
    });
    // The count, then seven fields per cell; a field holding the record's
    // own delimiter and an empty one both survive as strings.
    expect(answers[1]).toStrictEqual({
      code: 0,
      data: [
        { id: "a,b", itemKey: "k0", index: 0, left: 0, top: -50, right: 180, bottom: 30 },
        { id: "", itemKey: "", index: 2, left: 0, top: 30, right: 180, bottom: 70 },
      ],
    });
    expect(answers[2]).toStrictEqual({ code: 0, data: [] });
  });

  it("builds an input's getValue data with web-core's selectionBegin beside the typings' members", () => {
    const input = __CreateElement("x-input", 0);
    const answers: unknown[] = [];
    mock.answerElementMethod = () => "1:0" + "5:h:llo" + "1:1" + "1:3" + "1:1";
    __InvokeUIMethod(input, "getValue", {}, (result: unknown) => answers.push(result));
    mock.answerElementMethod = () => "1:0" + "0:" + "1:0" + "1:0" + "1:0";
    __InvokeUIMethod(input, "getValue", {}, (result: unknown) => answers.push(result));

    expect(answers[0]).toStrictEqual({
      code: 0,
      data: { value: "h:llo", selectionStart: 1, selectionEnd: 3, isComposing: true, selectionBegin: 1 },
    });
    expect(answers[1]).toStrictEqual({
      code: 0,
      data: { value: "", selectionStart: 0, selectionEnd: 0, isComposing: false, selectionBegin: 0 },
    });
  });

  it("builds a text's getTextBoundingRect data with the union of its boxes", () => {
    const text = __CreateText(0);
    const answers: unknown[] = [];
    mock.answerElementMethod = () =>
      "1:0" + "1:2"
      + "2:47" + "2:37" + "2:40" + "2:21"
      + "2:27" + "2:58" + "4:40.5" + "2:21";
    __InvokeUIMethod(text, "getTextBoundingRect", { start: 1, end: 6 }, (result: unknown) => answers.push(result));
    mock.answerElementMethod = () => "1:0" + "1:1" + "2:27" + "2:37" + "1:0" + "2:21";
    __InvokeUIMethod(text, "getTextBoundingRect", { start: 5, end: 5 }, (result: unknown) => answers.push(result));
    mock.answerElementMethod = () => 8;
    __InvokeUIMethod(text, "getTextBoundingRect", { start: 0, end: 99 }, (result: unknown) => answers.push(result));

    expect(mock.named("callElementMethod").slice(-1)[0]?.[3]).toBe('{"start":0,"end":99}');
    expect(answers[0]).toStrictEqual({
      code: 0,
      data: {
        boundingRect: { left: 27, top: 37, right: 87, bottom: 79, width: 60, height: 42 },
        boxes: [
          { left: 47, top: 37, right: 87, bottom: 58, width: 40, height: 21 },
          { left: 27, top: 58, right: 67.5, bottom: 79, width: 40.5, height: 21 },
        ],
      },
    });
    // One box is its own union, and the empty range's zero box is kept.
    expect(answers[1]).toStrictEqual({
      code: 0,
      data: {
        boundingRect: { left: 27, top: 37, right: 27, bottom: 58, width: 0, height: 21 },
        boxes: [{ left: 27, top: 37, right: 27, bottom: 58, width: 0, height: 21 }],
      },
    });
    expect(answers[2]).toStrictEqual({ code: 8, data: undefined });
  });

  it("builds a text's setTextSelection data with its boxes and two handles", () => {
    const text = __CreateText(0);
    const answers: unknown[] = [];
    mock.answerElementMethod = () =>
      "1:0" + "1:2"
      + "2:47" + "2:37" + "2:40" + "2:21"
      + "2:27" + "2:58" + "2:40" + "2:21"
      + "2:47" + "2:58" + "2:50"
      + "2:67" + "2:79" + "2:50";
    __InvokeUIMethod(
      text,
      "setTextSelection",
      { startX: 32, startY: 17, endX: 52, endY: 38, showStartHandle: true },
      (result: unknown) => answers.push(result),
    );
    mock.answerElementMethod = () => 0;
    __InvokeUIMethod(
      text,
      "setTextSelection",
      { startX: -1, startY: 0, endX: 0, endY: 0 },
      (result: unknown) => answers.push(result),
    );
    mock.answerElementMethod = () => 8;
    __InvokeUIMethod(
      text,
      "setTextSelection",
      { startX: 0, startY: 0, endX: 0, endY: 0 },
      (result: unknown) => answers.push(result),
    );

    expect(mock.named("callElementMethod").slice(-3)[0]?.[3]).toBe(
      '{"startX":32,"startY":17,"endX":52,"endY":38,"showStartHandle":true}',
    );
    expect(answers[0]).toStrictEqual({
      code: 0,
      data: {
        boundingRect: { left: 27, top: 37, right: 87, bottom: 79, width: 60, height: 42 },
        boxes: [
          { left: 47, top: 37, right: 87, bottom: 58, width: 40, height: 21 },
          { left: 27, top: 58, right: 67, bottom: 79, width: 40, height: 21 },
        ],
        handles: [
          { x: 47, y: 58, radius: 50 },
          { x: 67, y: 79, radius: 50 },
        ],
      },
    });
    // A negative coordinate clears the selection and answers no data.
    expect(answers[1]).toStrictEqual({ code: 0, data: undefined });
    expect(answers[2]).toStrictEqual({ code: 8, data: undefined });
  });

  it("builds a text's getSelectedText data from its one field", () => {
    const text = __CreateText(0);
    const answers: unknown[] = [];
    mock.answerElementMethod = () => "1:0" + "4:b:c\uFFFC";
    __InvokeUIMethod(text, "getSelectedText", {}, (result: unknown) => answers.push(result));
    mock.answerElementMethod = () => "1:0" + "0:";
    __InvokeUIMethod(text, "getSelectedText", {}, (result: unknown) => answers.push(result));

    expect(answers[0]).toStrictEqual({ code: 0, data: { selectedText: "b:c\uFFFC" } });
    expect(answers[1]).toStrictEqual({ code: 0, data: { selectedText: "" } });
  });

  it("sends missing params as an empty object and uncarriable ones as null", () => {
    const view = __CreateView(0);
    const cyclic: Record<string, unknown> = {};
    cyclic['self'] = cyclic;
    mock.calls.length = 0;

    __InvokeUIMethod(view, "selectTab", undefined, () => {});
    __InvokeUIMethod(view, "selectTab", { index: 2, smooth: false }, () => {});
    __InvokeUIMethod(view, "selectTab", cyclic, () => {});
    __InvokeUIMethod(view, "selectTab", () => {}, () => {});

    expect(mock.named("callElementMethod").map((call) => call[3])).toEqual([
      "{}",
      '{"index":2,"smooth":false}',
      "null",
      "null",
    ]);
  });

  it("reports an element carrying no id as the empty string", () => {
    const view = __CreateView(0);
    mock.answerElementMethod = () => styleRecord("0", "0", "0", "0", "0");
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
    mock.answerElementMethod = () => styleRecord("0", "0", "0", "0", "0");
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

describe("a frame's card", () => {
  /** One `bobcat:record` field, as the host writes it. */
  const field = (text: string) => `${text.length}:${text}`;

  /**
   * This realm as a frame's card: a frame element whose shadow root holds
   * the nested page, made this realm's page root as `bobcat:frame-boot`
   * makes it, with a `view` under that page. The frame's border box starts
   * at (20, 30) in the viewport.
   */
  function card() {
    const frameId = mock.createElement("frame");
    const shadowId = mock.createElement("shadow-root");
    const pageId = mock.createElement("page");
    mock.insertBefore(frameId, shadowId, null);
    mock.insertBefore(shadowId, pageId, null);
    mock.answerElementMethod = (id: number, method: string) =>
      id === frameId && method === "boundingClientRect"
        ? field("0") + field("20") + field("30") + field("200") + field("100")
        : 3;
    elementModule.__BobcatUsePageRoot(pageId);
    const page = __CreatePage("0", 0);
    const view = __CreateView(0);
    __AppendElement(page, view);
    mock.calls.length = 0;
    return { frameId, page, view };
  }

  it("makes x and y relative to its frame's border box, and keeps client and page coordinates the viewport's", () => {
    const { frameId, page, view } = card();
    let received!: ListenerEvent & Record<string, unknown>;
    __AddEventListener(view, "tap", (event: ListenerEvent & Record<string, unknown>) => {
      received = {
        ...event,
        touches: structuredClone(event["touches"]),
      } as ListenerEvent & Record<string, unknown>;
    }, {});

    dispatch([view, page], "tap", {
      x: 120,
      y: 60,
      button: 0,
      buttons: 0,
      touchNumbers: [1, 120, 60, 1],
    });

    expect(received.detail).toEqual({ x: 100, y: 30 });
    expect([received["x"], received["y"]]).toEqual([100, 30]);
    expect([received["clientX"], received["clientY"]]).toEqual([120, 60]);
    expect([received["pageX"], received["pageY"]]).toEqual([120, 60]);
    expect(received["touches"]).toEqual([
      { identifier: 1, x: 100, y: 30, pageX: 120, pageY: 60, clientX: 120, clientY: 60 },
    ]);
    // One readback per dispatch, of the frame — the shadow root's host.
    expect(mock.named("callElementMethod")).toEqual([
      ["callElementMethod", frameId, "boundingClientRect", ""],
    ]);
  });

  it("makes a touch event's detail and every point's x and y relative to its frame's border box", () => {
    const { page, view } = card();
    let received!: ListenerEvent & Record<string, unknown>;
    __AddEventListener(page, "touchstart", (event: ListenerEvent & Record<string, unknown>) => {
      received = {
        ...event,
        touches: structuredClone(event["touches"]),
        changedTouches: structuredClone(event["changedTouches"]),
      } as ListenerEvent & Record<string, unknown>;
    }, {});

    dispatch([view, page], "touchstart", { x: 130, y: 60, touchNumbers: [1, 130, 60, 7] });

    expect(received.detail).toEqual({ x: 110, y: 30 });
    const point = { identifier: 1, x: 110, y: 30, pageX: 130, pageY: 60, clientX: 130, clientY: 60 };
    expect(received["touches"]).toEqual([point]);
    expect(received["changedTouches"]).toEqual([point]);
  });

  it("ends its own path passes at its own catch, and still runs its global pass, frame-local", () => {
    const { page, view } = card();
    const seen: string[] = [];
    globalThis.runWorklet = (value, params) => {
      const event = params[0] as ListenerEvent & { detail: { x: number; y: number } };
      seen.push(`${(value as { label: string }).label}:${event.target.uid}:${event.detail.x},${event.detail.y}`);
    };
    __AddEvent(view, "catchEvent", "tap", { type: "worklet", value: { label: "view-catch" } });
    __AddEvent(page, "bindEvent", "tap", { type: "worklet", value: { label: "page-bind" } });
    __AddEvent(page, "global-bindEvent", "tap", { type: "worklet", value: { label: "page-global" } });
    const uid = __GetElementUniqueID(view);

    dispatch([view, page], "tap", { x: 120, y: 60, button: 0, buttons: 0 });

    // The page's bubble step is this realm's own walk, which the catch
    // ended; the global pass is no step of the path. Another realm's walk
    // is the host's to call, and nothing here reaches it.
    expect(seen).toEqual([`view-catch:${uid}:100,30`, `page-global:${uid}:100,30`]);
  });

  it("still refuses a target of its own that no handle names", () => {
    const { view } = card();
    const uid = __GetElementUniqueID(view);

    expect(() =>
      elementModule.__BobcatDispatchEvent(
        "tap",
        true,
        0,
        1,
        uid,
        999,
        0,
        DETAIL_POSITION,
        0,
        0,
        undefined,
        undefined,
      )
    ).toThrow("ownership graph");
  });

  it("reads no origin for a dispatch without a position", () => {
    const { view } = card();
    let runs = 0;
    __AddEventListener(view, "load", () => {
      runs += 1;
    }, {});

    dispatch([view], "load", { empty: true }, undefined, false);

    expect(runs).toBe(1);
    expect(mock.named("callElementMethod")).toEqual([]);
  });

  it("skips the steps it holds no handle for, and aims at the first one it holds", () => {
    const { frameId, page, view } = card();
    const outer = mock.createElement("page");
    const seen: unknown[] = [];
    __AddEventListener(page, "tap", (event: ListenerEvent) => {
      seen.push(["page", event.target.uid, event.currentTarget.uid, event.eventPhase]);
    }, { capture: true });
    __AddEventListener(view, "tap", (event: ListenerEvent) => {
      seen.push(["view", event.target.uid, event.currentTarget.uid, event.eventPhase]);
    }, {});
    const uid = __GetElementUniqueID(view);
    const pageUid = __GetElementUniqueID(page);

    // The whole path, as the host hands it to every realm: the card's own
    // steps, then the frame and the page realm's page, which this realm
    // holds no handle for, retargeted to the frame.
    elementModule.__BobcatDispatchEvent(
      "tap",
      true,
      0,
      4,
      uid,
      uid,
      pageUid,
      uid,
      frameId,
      frameId,
      outer,
      frameId,
      0,
      DETAIL_EMPTY,
    );

    expect(seen).toEqual([
      ["page", uid, pageUid, 1],
      ["view", uid, uid, 2],
    ]);
  });

  it("takes its target from the first step it holds when the steps before it are another realm's", () => {
    const { frameId, view } = card();
    const shadowContent = mock.createElement("view");
    let target: unknown;
    __AddEventListener(view, "tap", (event: ListenerEvent) => {
      target = event.target.uid;
    }, {});
    const uid = __GetElementUniqueID(view);

    elementModule.__BobcatDispatchEvent(
      "tap",
      true,
      0,
      3,
      shadowContent,
      shadowContent,
      uid,
      uid,
      frameId,
      frameId,
      0,
      DETAIL_EMPTY,
    );

    expect(target).toBe(uid);
  });

  it("runs its global pass alone when it holds no step of the path, aimed at the element that registered", () => {
    const { page, view } = card();
    const documentElement = mock.createElement("page");
    const seen: unknown[] = [];
    globalThis.runWorklet = (value, params) => {
      (value as { body: (event: unknown) => void }).body(params[0]);
    };
    const body = (event: ListenerEvent & { key?: string }) =>
      seen.push([event.type, event.target.uid, event.currentTarget.uid, event.eventPhase, event.key]);
    __AddEvent(view, "global-bindEvent", "keydown", { type: "worklet", value: { body } });
    __AddEvent(view, "bindEvent", "keydown", { type: "worklet", value: { body: () => seen.push("bind") } });
    // A second element of the card, filed after the first: delivered after
    // it, and aimed at itself.
    const other = __CreateView(0);
    __AppendElement(page, other);
    __AddEvent(other, "global-bindEvent", "keydown", { type: "worklet", value: { body } });
    const uid = __GetElementUniqueID(view);
    const otherUid = __GetElementUniqueID(other);

    // A key's target is the document element, which is no node of a card's
    // tree: the host hands the card the page's path, none of which it holds.
    elementModule.__BobcatDispatchEvent(
      "keydown",
      true,
      0,
      1,
      documentElement,
      documentElement,
      0,
      DETAIL_KEY,
      "a",
      "KeyA",
      false,
      0,
    );

    expect(seen).toEqual([
      ["keydown", uid, uid, 0, "a"],
      ["keydown", otherUid, otherUid, 0, "a"],
    ]);
  });

  it("runs its global pass alone when handed no step", () => {
    const { view } = card();
    let target: unknown;
    globalThis.runWorklet = (value, params) => {
      (value as { body: (event: unknown) => void }).body(params[0]);
    };
    __AddEvent(view, "global-bindEvent", "keydown", {
      type: "worklet",
      value: { body: (event: ListenerEvent) => (target = event.target.uid) },
    });

    elementModule.__BobcatDispatchEvent("keydown", true, 0, 0, 0, DETAIL_KEY, "a", "KeyA", false, 0);

    expect(target).toBe(__GetElementUniqueID(view));
  });

  it("runs nothing for an event that does not bubble when it holds no step", () => {
    const { frameId, view } = card();
    __AddEvent(view, "global-bindEvent", "load", "3:0:load");

    elementModule.__BobcatDispatchEvent("load", false, 0, 0, 0, DETAIL_EMPTY);
    elementModule.__BobcatDispatchEvent(
      "load",
      false,
      0,
      1,
      frameId,
      frameId,
      0,
      DETAIL_EMPTY,
    );

    // A non-bubbling event runs no global pass, so there is nothing to run.
    expect(mock.named("publishEvent")).toEqual([]);
  });
});

describe("the page realm", () => {
  it("shapes positions in the viewport and reads no origin", () => {
    const { inner } = tree();
    let detail: unknown;
    let x: unknown;
    __AddEventListener(inner, "tap", (event: ListenerEvent & { x?: number }) => {
      detail = event.detail;
      x = event.x;
    }, {});
    mock.calls.length = 0;

    dispatch([inner], "tap", { x: 120, y: 60, button: 0, buttons: 0 });

    expect(detail).toEqual({ x: 120, y: 60 });
    expect(x).toBe(120);
    expect(mock.named("callElementMethod")).toEqual([]);
    expect(mock.named("parentNode")).toEqual([]);
  });
});

describe("layout events", () => {
  /** The detail every dispatch handed a closure on `element`, in order. */
  const details = (element: object, name: string) => {
    const seen: unknown[] = [];
    __AddEventListener(element, name, (event: { detail: unknown }) => {
      seen.push(event.detail);
    }, {});
    return seen;
  };

  it("builds layoutchange's detail from the four numbers and the target", () => {
    const { inner } = tree();
    __SetID(inner, "box");
    __AddDataset(inner, "row", 7);
    const seen = details(inner, "layoutchange");

    dispatch([inner], "layoutchange", {
      extras: [["left", 50], ["top", 60], ["width", 100], ["height", 40]],
    }, undefined, false);

    expect(seen).toEqual([{
      id: "box",
      dataset: { row: 7 },
      width: 100,
      height: 40,
      left: 50,
      top: 60,
      right: 150,
      bottom: 100,
    }]);
    expect(Object.keys(seen[0] as object)).toEqual([
      "id",
      "dataset",
      "width",
      "height",
      "left",
      "top",
      "right",
      "bottom",
    ]);
  });

  it("answers an empty id for an element that carries none", () => {
    const { inner } = tree();
    const seen = details(inner, "layoutchange");

    dispatch([inner], "layoutchange", {
      extras: [["left", 0], ["top", 0], ["width", 0], ["height", 0]],
    }, undefined, false);

    expect(seen).toEqual([{
      id: "",
      dataset: {},
      width: 0,
      height: 0,
      left: 0,
      top: 0,
      right: 0,
      bottom: 0,
    }]);
  });

  it("publishes the layoutchange detail to a background handler as values", () => {
    const { inner } = tree();
    __AddDataset(inner, "row", 7);
    __AddEvent(inner, "bindEvent", "layoutchange", "3:0:bindlayoutchange");

    dispatch([inner], "layoutchange", {
      extras: [["left", 1], ["top", 2], ["width", 3], ["height", 4]],
    }, undefined, false);

    const published = mock.named("publishEvent");
    expect(published).toHaveLength(1);
    expect(published[0]![2]).toBe("3:0:bindlayoutchange");
    expect((published[0]![3] as { detail: unknown }).detail).toEqual({
      id: "",
      dataset: { row: 7 },
      width: 3,
      height: 4,
      left: 1,
      top: 2,
      right: 4,
      bottom: 6,
    });
  });

  it("builds a text's layout detail from the lines' JSON text", () => {
    const { inner } = tree();
    const seen = details(inner, "layout");

    dispatch([inner], "layout", {
      extras: [
        ["lineCount", 2],
        ["lines", "[0,5,0,6,11,3]"],
        ["width", 100],
        ["height", 42],
      ],
    }, undefined, false);

    expect(seen).toEqual([{
      lineCount: 2,
      lines: [
        { start: 0, end: 5, ellipsisCount: 0 },
        { start: 6, end: 11, ellipsisCount: 3 },
      ],
      size: { width: 100, height: 42 },
    }]);
  });

  it("leaves another event's extras as the detail", () => {
    const { inner } = tree();
    const seen = details(inner, "load");

    dispatch([inner], "load", { width: 40, height: 20 }, undefined, false);

    expect(seen).toEqual([{ width: 40, height: 20 }]);
  });
});
