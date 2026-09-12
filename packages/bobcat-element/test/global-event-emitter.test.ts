import { describe, expect, it, rstest } from "@rstest/core";
import { GlobalEventEmitter } from "../src/global-event-emitter.ts";

describe("Lynx GlobalEventEmitter", () => {
  it("keeps duplicate registrations, their receivers, and removes only the first match", () => {
    const emitter = new GlobalEventEmitter();
    const context = {};
    const seen: unknown[] = [];
    function listener(this: unknown, ...args: unknown[]) { seen.push([this, args]); }
    emitter.addListener("event", listener, context);
    emitter.addListener("event", listener);
    emitter.emit("event", [1, 2]);
    emitter.removeListener("event", listener);
    emitter.emit("event", [3]);
    expect(seen).toEqual([[context, [1, 2]], [emitter, [1, 2]], [emitter, [3]]]);
  });

  it("distinguishes an argument-list emission from a single parsed trigger payload", () => {
    const emitter = new GlobalEventEmitter();
    const listener = rstest.fn();
    emitter.addListener("event", listener);
    emitter.emit("event", [1, 2]);
    emitter.trigger("event", "[1,2]");
    emitter.trigger("event", { value: 3 });
    expect(listener.mock.calls).toEqual([[1, 2], [[1, 2]], [{ value: 3 }]]);
    expect(() => emitter.trigger("unregistered", "invalid JSON")).not.toThrow();
    expect(() => emitter.trigger("event", "invalid JSON")).toThrow();
  });

  it("uses native live-array iteration and propagates a listener failure", () => {
    const emitter = new GlobalEventEmitter();
    const seen: number[] = [];
    const first = () => { seen.push(1); emitter.removeListener("event", first); };
    emitter.addListener("event", first);
    emitter.addListener("event", () => seen.push(2));
    emitter.addListener("event", () => { seen.push(3); throw Error("failure"); });
    emitter.addListener("event", () => seen.push(4));
    expect(() => emitter.emit("event", [])).toThrow("failure");
    expect(seen).toEqual([1, 3]);
    emitter.removeAllListeners("event");
    expect(() => emitter.emit("event", [])).not.toThrow();
  });
});
