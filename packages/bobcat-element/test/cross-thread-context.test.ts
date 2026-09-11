import { beforeAll, describe, expect, it, rstest } from "@rstest/core";
import * as eventTarget from "../src/event-target.ts";
import type * as contextModule from "../src/cross-thread-context.ts";

rstest.mockRequire("bobcat:event-target", () => eventTarget);

let createCrossThreadContext: typeof contextModule.createCrossThreadContext;
beforeAll(async () => {
  ({ createCrossThreadContext } = await import("../src/cross-thread-context.ts"));
});

describe("Lynx cross-thread context", () => {
  it("returns an EventTarget subclass with inherited listener methods", () => {
    const context = createCrossThreadContext();
    expect(context).toBeInstanceOf(eventTarget.EventTarget);
    expect(Object.getPrototypeOf(Object.getPrototypeOf(context))).toBe(
      eventTarget.EventTarget.prototype,
    );
    expect(context.addEventListener).toBe(
      eventTarget.EventTarget.prototype.addEventListener,
    );
    expect(context.removeEventListener).toBe(
      eventTarget.EventTarget.prototype.removeEventListener,
    );
    expect(Object.hasOwn(context, "addEventListener")).toBe(false);
    expect(Object.hasOwn(context, "removeEventListener")).toBe(false);
  });

  it("retains queued event references until the Worker transport snapshots them", () => {
    const context = createCrossThreadContext();
    const sent: unknown[] = [];
    const first = { type: "first", data: { value: 1 } };
    const second = { type: "second", data: { value: 2 } };

    expect(context.dispatchEvent(first)).toBe(3);
    context.dispatchEvent(second);
    first.data.value = 3;
    expect(sent).toEqual([]);

    context.connect((event) => sent.push(JSON.parse(JSON.stringify(event))));
    first.data.value = 4;
    const third = { type: "third", data: { value: 5 } };
    context.dispatchEvent(third);
    third.data.value = 6;

    expect(sent).toEqual([
      { type: "first", data: { value: 3 } },
      { type: "second", data: { value: 2 } },
      { type: "third", data: { value: 5 } },
    ]);
  });

  it("sends typed events without local echo and returns the ContextProxy result", () => {
    const context = createCrossThreadContext();
    const sent: unknown[] = [];
    const received: unknown[] = [];
    context.connect((event) => sent.push(event));
    context.addEventListener("update", (event: unknown) => {
      received.push(event);
    });

    const event = { type: "update", data: { value: 7 } };
    expect(context.dispatchEvent(event)).toBe(3);
    expect(sent).toEqual([event]);
    expect(received).toEqual([]);

    context.receive(event);
    expect(received).toEqual([event]);
    expect(sent).toEqual([event]);
  });

  it("receives by type and defaults only nullish data to an empty object", () => {
    const context = createCrossThreadContext();
    const received: unknown[] = [];
    context.addEventListener("update", (event: unknown) => {
      received.push(event);
    });

    context.receive({ type: "unrelated", data: 1 });
    context.receive({ type: "update" });
    context.receive({ type: "update", data: null });
    context.receive({ type: "update", data: false });
    context.receive({ type: "update", data: 0 });
    context.receive({ type: "update", data: "" });

    expect(received).toEqual([
      { type: "update", data: {} },
      { type: "update", data: {} },
      { type: "update", data: false },
      { type: "update", data: 0 },
      { type: "update", data: "" },
    ]);
  });

  it("does not retain incoming events for listeners registered later", () => {
    const context = createCrossThreadContext();
    const received: unknown[] = [];
    context.receive({ type: "update", data: "early" });
    context.addEventListener("update", (event: unknown) => {
      received.push(event);
    });
    context.receive({ type: "update", data: "now" });
    expect(received).toEqual([{ type: "update", data: "now" }]);
  });

  it("keeps callback identity, capture and once semantics with the context as this", () => {
    const context = createCrossThreadContext();
    const receivers: unknown[] = [];
    function listener(this: unknown) {
      receivers.push(this);
    }
    context.addEventListener("update", listener, { once: true });
    context.addEventListener("update", listener);
    context.addEventListener("update", listener, true);

    context.receive({ type: "update" });
    expect(receivers).toEqual([context, context]);
    context.receive({ type: "update" });
    expect(receivers).toEqual([context, context, context]);
    context.removeEventListener("update", listener, true);
    context.receive({ type: "update" });
    expect(receivers).toEqual([context, context, context]);
  });

  it("honors listener removals during receive and defers additions to the next event", () => {
    const context = createCrossThreadContext();
    const calls: string[] = [];
    const removed = () => calls.push("removed");
    const added = () => calls.push("added");
    context.addEventListener("update", () => {
      calls.push("first");
      context.removeEventListener("update", removed);
      context.addEventListener("update", added);
    });
    context.addEventListener("update", removed);

    context.receive({ type: "update" });
    expect(calls).toEqual(["first"]);
    context.receive({ type: "update" });
    expect(calls).toEqual(["first", "first", "added"]);
  });

  it("leaves postMessage unimplemented without sending or local delivery", () => {
    const context = createCrossThreadContext();
    const sent: unknown[] = [];
    context.connect((event) => sent.push(event));
    expect(context.postMessage({ type: "update", data: 1 })).toBeUndefined();
    expect(sent).toEqual([]);
  });
});


describe("BTS message values over the Worker JSON channel", () => {
  it("preserves undefined and special numbers without colliding with user objects", async () => {
    const {packBtsMessage, unpackBtsMessage} = await import("../src/cross-thread-context.ts");
    const data = {missing: undefined, negativeZero: -0, nan: NaN, infinity: Infinity,
      object: {bobcat: "value", value: ["undefined"]}, array: [undefined, null]};
    const received = unpackBtsMessage(JSON.parse(JSON.stringify(packBtsMessage(data)))) as typeof data;
    expect(received).toEqual(data);
    expect(Object.hasOwn(received, "missing")).toBe(true);
    expect(Object.is(received.negativeZero, -0)).toBe(true);
  });
});
