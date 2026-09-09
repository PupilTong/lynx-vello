// @ts-check

import { beforeAll, describe, expect, it, rstest } from "@rstest/core";
import * as eventTarget from "../src/event-target.mjs";

rstest.mockRequire("bobcat:event-target", () => eventTarget);

/** @type {typeof import("../src/cross-thread-context.mjs").createCrossThreadContext} */
let createCrossThreadContext;
beforeAll(async () => {
  ({ createCrossThreadContext } = await import("../src/cross-thread-context.mjs"));
});

describe("Lynx cross-thread context", () => {
  it("retains queued event references until the Worker transport snapshots them", () => {
    const { context, connect } = createCrossThreadContext();
    /** @type {unknown[]} */
    const sent = [];
    const first = { type: "first", data: { value: 1 } };
    const second = { type: "second", data: { value: 2 } };

    expect(context.dispatchEvent(first)).toBe(3);
    context.dispatchEvent(second);
    first.data.value = 3;
    expect(sent).toEqual([]);

    connect((event) => sent.push(JSON.parse(JSON.stringify(event))));
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
    const { context, connect, receive } = createCrossThreadContext();
    /** @type {unknown[]} */
    const sent = [];
    /** @type {unknown[]} */
    const received = [];
    connect((event) => sent.push(event));
    context.addEventListener("update", /** @param {unknown} event */ (event) => {
      received.push(event);
    });

    const event = { type: "update", data: { value: 7 } };
    expect(context.dispatchEvent(event)).toBe(3);
    expect(sent).toEqual([event]);
    expect(received).toEqual([]);

    receive(event);
    expect(received).toEqual([event]);
    expect(sent).toEqual([event]);
  });

  it("receives by type and defaults only nullish data to an empty object", () => {
    const { context, receive } = createCrossThreadContext();
    /** @type {unknown[]} */
    const received = [];
    context.addEventListener("update", /** @param {unknown} event */ (event) => {
      received.push(event);
    });

    receive({ type: "unrelated", data: 1 });
    receive({ type: "update" });
    receive({ type: "update", data: null });
    receive({ type: "update", data: false });
    receive({ type: "update", data: 0 });
    receive({ type: "update", data: "" });

    expect(received).toEqual([
      { type: "update", data: {} },
      { type: "update", data: {} },
      { type: "update", data: false },
      { type: "update", data: 0 },
      { type: "update", data: "" },
    ]);
  });

  it("does not retain incoming events for listeners registered later", () => {
    const { context, receive } = createCrossThreadContext();
    /** @type {unknown[]} */
    const received = [];
    receive({ type: "update", data: "early" });
    context.addEventListener("update", /** @param {unknown} event */ (event) => {
      received.push(event);
    });
    receive({ type: "update", data: "now" });
    expect(received).toEqual([{ type: "update", data: "now" }]);
  });

  it("keeps callback identity, capture and once semantics with the context as this", () => {
    const { context, receive } = createCrossThreadContext();
    /** @type {unknown[]} */
    const receivers = [];
    /** @this {unknown} */
    function listener() {
      receivers.push(this);
    }
    context.addEventListener("update", listener, { once: true });
    context.addEventListener("update", listener);
    context.addEventListener("update", listener, true);

    receive({ type: "update" });
    expect(receivers).toEqual([context, context]);
    receive({ type: "update" });
    expect(receivers).toEqual([context, context, context]);
    context.removeEventListener("update", listener, true);
    receive({ type: "update" });
    expect(receivers).toEqual([context, context, context]);
  });

  it("honors listener removals during receive and defers additions to the next event", () => {
    const { context, receive } = createCrossThreadContext();
    /** @type {string[]} */
    const calls = [];
    const removed = () => calls.push("removed");
    const added = () => calls.push("added");
    context.addEventListener("update", () => {
      calls.push("first");
      context.removeEventListener("update", removed);
      context.addEventListener("update", added);
    });
    context.addEventListener("update", removed);

    receive({ type: "update" });
    expect(calls).toEqual(["first"]);
    receive({ type: "update" });
    expect(calls).toEqual(["first", "first", "added"]);
  });

  it("leaves postMessage unimplemented without sending or local delivery", () => {
    const { context, connect } = createCrossThreadContext();
    /** @type {unknown[]} */
    const sent = [];
    connect((event) => sent.push(event));
    expect(context.postMessage({ type: "update", data: 1 })).toBeUndefined();
    expect(sent).toEqual([]);
  });
});
