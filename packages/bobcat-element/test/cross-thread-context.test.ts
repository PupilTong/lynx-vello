import { beforeAll, describe, expect, it, rstest } from "@rstest/core";
import * as eventTarget from "../src/event-target.ts";
rstest.mockRequire("bobcat:event-target", () => eventTarget);
import type { ContextEvent } from "../src/cross-thread-context.ts";
import type * as contextModule from "../src/cross-thread-context.ts";
let createCrossThreadContext: typeof contextModule.createCrossThreadContext;
beforeAll(async () => {
  ({createCrossThreadContext} = await import("../src/cross-thread-context.ts"));
});

describe("native Lynx Context contract", () => {
  it("preserves FIFO and queued payload references until Worker JSON copies the send", () => {
    const context = createCrossThreadContext("CoreContext");
    const sent: ContextEvent[] = [];
    const heard: unknown[] = [];
    context.addEventListener("change", (e: unknown) => heard.push(e));
    const first = { type: "change", data: { value: 1 } };
    expect(context.dispatchEvent(first)).toBe(0);
    first.data.value = 2;
    context.dispatchEvent({ type: "change", data: null });
    context.connect(event => sent.push(JSON.parse(JSON.stringify(event))));
    first.data.value = 3;
    context.postMessage(undefined);
    expect(sent).toEqual([
      { type: "change", data: { value: 2 }, origin: "CoreContext" },
      { type: "change", data: null, origin: "CoreContext" },
      { type: "message", origin: "CoreContext" },
    ]);
    expect(heard).toEqual([]);
    context.receive(sent[1]!);
    expect(heard).toEqual([sent[1]]);
  });

  it("validates native arguments and ignores DOM listener options", () => {
    const context = createCrossThreadContext();
    for (const invalid of [null, {}, {type: 1, data: 0}, {type: "x"}]) {
      expect(() => context.dispatchEvent(invalid as ContextEvent)).toThrow();
    }
    expect(() => context.postMessage()).toThrow();
    expect(() => context.addEventListener(1, () => {})).toThrow();
    expect(() => context.addEventListener("x", {handleEvent() {}})).toThrow();
    expect(() => context.removeEventListener("x", null)).toThrow();
    const receivers: unknown[] = [];
    function listener(this: unknown) { receivers.push(this); }
    context.addEventListener("x", listener, { once: true, capture: true });
    context.addEventListener("x", listener, false);
    context.receive({type: "x", data: undefined, origin: "CoreContext"});
    context.receive({type: "x", data: null, origin: "CoreContext"});
    expect(receivers).toEqual([undefined, undefined]);
    context.removeEventListener("x", listener, false);
    context.receive({type: "x", data: 0});
    expect(receivers).toHaveLength(2);
  });

  it("honors removal in the current walk and defers new listeners", () => {
    const context = createCrossThreadContext();
    const calls: string[] = [];
    const removed = () => calls.push("removed");
    const added = () => calls.push("added");
    context.addEventListener("x", () => {
      calls.push("first");
      context.removeEventListener("x", removed);
      context.addEventListener("x", added);
    });
    context.addEventListener("x", removed);
    context.receive({type: "x", data: 0});
    context.receive({type: "x", data: 0});
    expect(calls).toEqual(["first", "first", "added"]);
  });

  it("uses ordinary JSON values and toJSON across the Worker boundary", () => {
    const context = createCrossThreadContext();
    const received: unknown[] = [];
    context.addEventListener("message", (event: unknown) => received.push(event));
    context.connect(event => context.receive(JSON.parse(JSON.stringify(event))));
    context.postMessage({missing: undefined, zero: -0, nan: NaN, list: [undefined, null]});
    context.postMessage({toJSON: () => "custom JSON"});
    context.postMessage(undefined);
    context.postMessage(null);
    expect(received).toEqual([
      {type: "message", data: {zero: 0, nan: null, list: [null, null]}, origin: "JSContext"},
      {type: "message", data: "custom JSON", origin: "JSContext"},
      {type: "message", data: undefined, origin: "JSContext"},
      {type: "message", data: null, origin: "JSContext"},
    ]);
    expect(() => context.postMessage(1n)).toThrow();
    const cycle: {self?: unknown} = {};
    cycle.self = cycle;
    expect(() => context.postMessage(cycle)).toThrow();
  });
});
