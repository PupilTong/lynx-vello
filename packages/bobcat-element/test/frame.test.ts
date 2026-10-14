// `bobcat:frame`: what a frame's `src`, connection and disconnection do in
// the realm that holds its handle, over stand-ins for the host members it
// reaches (`fetchResource`, `resetFrame` and the `Future` trio), for the
// diagnostics pair, and for `ShadowRealm`.
//
// The `Future` class is the real one; its settle is driven through its own
// `__BobcatSettleFuture`, as the host's epilogue drives it. The child half —
// `bobcat:frame-boot` rendering the card under the nested page — runs
// against the real realm in crates/bobcat-core/src/main/runtime/tests.rs and
// frame_tests.rs. The collection path (a collected frame handle disposes of
// its card) needs a collector this suite cannot drive; frame_tests.rs runs it.

import { beforeEach, describe, expect, it, rstest } from "@rstest/core";

/** Everything the stand-ins were asked, in order. */
const calls: unknown[][] = [];
/** URLs the fetcher's probe already holds, which answer `true`. */
const already = new Set<string>();
let nextFuture = 1;

rstest.mockRequire("bobcat-internal:host", () => ({
  fetchResource(url: string): number | true {
    calls.push(["fetchResource", url]);
    if (already.has(url)) return true;
    return nextFuture++;
  },
  resetFrame(frame: number): undefined {
    calls.push(["resetFrame", frame]);
    return undefined;
  },
  settleFuture(id: number): undefined {
    calls.push(["settleFuture", id]);
    return undefined;
  },
  waitFuture(): boolean {
    throw new Error("bobcat:frame never waits");
  },
  takeFuture(): unknown {
    throw new Error("bobcat:frame never waits");
  },
}));

import * as future from "../src/future.ts";
rstest.mockRequire("bobcat:future", () => future);

rstest.mockRequire("bobcat:diagnostics", () => ({
  reportError(error: unknown) {
    calls.push(["reportError", error instanceof Error ? error.message : String(error)]);
  },
}));

/** The handles `bobcat:element`'s table resolves, by node id. */
const handles: Map<number, object> = new Map();

/** A frame handle this realm holds under `frameId`. */
function frameHandle(frameId: number): object {
  const handle = {};
  handles.set(frameId, handle);
  return handle;
}

rstest.mockRequire("bobcat:element", () => ({
  __BobcatHandleOf(nodeId: number): object | undefined {
    return handles.get(nodeId);
  },
}));

rstest.mockRequire("bobcat:runtime", () => ({
  SystemInfo: { platform: "headless", pixelRatio: 2 },
}));

/**
 * A stand-in for the host's `ShadowRealm`: its child's `boot` and `dispose`
 * record their calls, `dispose` with the number of the realm it closes.
 */
class FakeShadowRealm {
  static created = 0;
  /** What the next `importValue` settles with, if not a recording export. */
  static importFails: string | undefined;
  /** Set to hold every `importValue` until the test calls `release`. */
  static hold = false;
  static held: (() => void)[] = [];
  static release(): void {
    for (const settle of FakeShadowRealm.held.splice(0)) settle();
  }
  readonly number: number;
  constructor() {
    FakeShadowRealm.created += 1;
    this.number = FakeShadowRealm.created;
  }
  evaluate(): unknown {
    throw new Error("bobcat:frame evaluates nothing");
  }
  importValue(specifier: string, name: string): Promise<unknown> {
    calls.push(["importValue", specifier, name]);
    const failure = FakeShadowRealm.importFails;
    if (failure !== undefined) return Promise.reject(new TypeError(failure));
    const exported = name === "dispose"
      ? () => {
        calls.push(["dispose", this.number]);
      }
      : (...args: unknown[]) => {
        calls.push(["boot", ...args]);
        return true;
      };
    if (!FakeShadowRealm.hold) return Promise.resolve(exported);
    return new Promise(resolve => {
      FakeShadowRealm.held.push(() => resolve(exported));
    });
  }
}

import {
  __BobcatCloseFrames,
  __BobcatFrameConnected,
  __BobcatFrameDisconnected,
  frameSourceChanged,
} from "../src/frame.ts";

/**
 * The future ids the `Future`s converted so far were settled under, in
 * order. Ids run on across tests, because the real `Future` module, and the
 * table it keeps them in, does.
 */
function futures(): number[] {
  return calls.filter(call => call[0] === "settleFuture").map(call => call[1] as number);
}

/** Lets every queued promise job run. */
async function drain() {
  for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
}

const SYSTEM_INFO = JSON.stringify({ platform: "headless", pixelRatio: 2 });

beforeEach(() => {
  // What an earlier test left loaded is this module's, which every test shares.
  __BobcatCloseFrames();
  calls.length = 0;
  already.clear();
  handles.clear();
  FakeShadowRealm.created = 0;
  FakeShadowRealm.importFails = undefined;
  FakeShadowRealm.hold = false;
  FakeShadowRealm.held = [];
  (globalThis as Record<string, unknown>)["ShadowRealm"] = FakeShadowRealm;
});

describe("frameSourceChanged", () => {
  it("fetches src as written, and boots it in a fresh realm once the fetch settles", async () => {
    const frame = {};
    frameSourceChanged(frame, 3, "child.web.bundle");
    const [id] = futures();
    // A frame with no card yet has nothing to release.
    expect(calls).toEqual([
      ["fetchResource", "child.web.bundle"],
      ["settleFuture", id],
    ]);
    await drain();
    expect(FakeShadowRealm.created).toBe(0);

    future.__BobcatSettleFuture(id!, false, undefined);
    await drain();
    expect(FakeShadowRealm.created).toBe(1);
    expect(calls.slice(2)).toEqual([
      ["importValue", "bobcat:frame-boot", "boot"],
      ["importValue", "bobcat:frame-boot", "dispose"],
      ["boot", 3, "child.web.bundle", "{}", "{}", SYSTEM_INFO],
    ]);
  });

  it("defers a probe hit to a microtask", async () => {
    already.add("app:///cached.web.bundle");
    frameSourceChanged({}, 3, "app:///cached.web.bundle");
    expect(calls).toEqual([
      ["fetchResource", "app:///cached.web.bundle"],
    ]);
    expect(FakeShadowRealm.created).toBe(0);
    await drain();
    expect(FakeShadowRealm.created).toBe(1);
    expect(calls.at(-1)).toEqual(["boot", 3, "app:///cached.web.bundle", "{}", "{}", SYSTEM_INFO]);
  });

  it("starts nothing for a stale generation", async () => {
    const frame = {};
    frameSourceChanged(frame, 3, "app:///first.web.bundle");
    frameSourceChanged(frame, 3, "app:///second.web.bundle");
    const [first, second] = futures();
    // The first fetch settles after the second `src` was written.
    future.__BobcatSettleFuture(first!, false, undefined);
    await drain();
    expect(FakeShadowRealm.created).toBe(0);
    future.__BobcatSettleFuture(second!, false, undefined);
    await drain();
    expect(FakeShadowRealm.created).toBe(1);
    expect(calls.filter(call => call[0] === "boot")).toEqual([
      ["boot", 3, "app:///second.web.bundle", "{}", "{}", SYSTEM_INFO],
    ]);
  });

  it("does not boot a realm whose src changed while it imported", async () => {
    already.add("app:///first.web.bundle");
    FakeShadowRealm.hold = true;
    const frame = {};
    frameSourceChanged(frame, 3, "app:///first.web.bundle");
    await drain();
    expect(FakeShadowRealm.created).toBe(1);
    // The `src` changes before the realm's import settles.
    frameSourceChanged(frame, 3, "app:///second.web.bundle");
    FakeShadowRealm.release();
    await drain();
    expect(calls.filter(call => call[0] === "boot")).toEqual([]);
    // Nor is there a `dispose` to call: the child never booted, and nothing
    // refers to it once its import settled.
    expect(calls.filter(call => call[0] === "dispose")).toEqual([]);
  });

  it("does nothing for a null or empty src: the frame keeps its card, as native and web-core do", async () => {
    const frame = {};
    frameSourceChanged(frame, 3, "app:///child.web.bundle");
    const [first] = futures();
    calls.length = 0;
    frameSourceChanged(frame, 3, null);
    frameSourceChanged(frame, 3, "");
    expect(calls).toEqual([]);

    // The load in flight is still the frame's, and boots.
    future.__BobcatSettleFuture(first!, false, undefined);
    await drain();
    expect(FakeShadowRealm.created).toBe(1);
    expect(calls.filter(call => call[0] === "boot")).toEqual([
      ["boot", 3, "app:///child.web.bundle", "{}", "{}", SYSTEM_INFO],
    ]);

    // Nor does a removal undo what the next write compares against: the
    // same `src` again is still unchanged.
    calls.length = 0;
    frameSourceChanged(frame, 3, null);
    frameSourceChanged(frame, 3, "app:///child.web.bundle");
    expect(calls).toEqual([]);

    // A frame that never had a card ignores both, too.
    frameSourceChanged({}, 4, null);
    frameSourceChanged({}, 4, "");
    expect(calls).toEqual([]);
  });

  it("disposes of the old card and resets the frame first when a new src replaces it", async () => {
    already.add("app:///first.web.bundle");
    already.add("app:///second.web.bundle");
    const frame = {};
    frameSourceChanged(frame, 3, "app:///first.web.bundle");
    await drain();
    calls.length = 0;
    frameSourceChanged(frame, 3, "app:///second.web.bundle");
    expect(calls).toEqual([
      ["dispose", 1],
      ["resetFrame", 3],
      ["fetchResource", "app:///second.web.bundle"],
    ]);
    await drain();
    expect(FakeShadowRealm.created).toBe(2);
    expect(calls.at(-1)).toEqual(["boot", 3, "app:///second.web.bundle", "{}", "{}", SYSTEM_INFO]);
  });

  it("does nothing for the src the frame already has, as native and web-core skip it", () => {
    const frame = {};
    frameSourceChanged(frame, 3, "app:///child.web.bundle");
    calls.length = 0;
    frameSourceChanged(frame, 3, "app:///child.web.bundle");
    expect(calls).toEqual([]);
  });

  it("reports a failed fetch once, naming src, and starts nothing", async () => {
    frameSourceChanged({}, 3, "app:///nope.web.bundle");
    future.__BobcatSettleFuture(futures()[0]!, true, "not found");
    await drain();
    expect(FakeShadowRealm.created).toBe(0);
    expect(calls.filter(call => call[0] === "reportError")).toEqual([
      ["reportError", "frame app:///nope.web.bundle: not found"],
    ]);
  });

  it("reports nothing for a stale fetch that failed", async () => {
    const frame = {};
    frameSourceChanged(frame, 3, "app:///nope.web.bundle");
    frameSourceChanged(frame, 3, "app:///other.web.bundle");
    future.__BobcatSettleFuture(futures()[0]!, true, "not found");
    await drain();
    expect(calls.filter(call => call[0] === "reportError")).toEqual([]);
  });

  it("reports a failed import of the boot module, naming src", async () => {
    already.add("app:///child.web.bundle");
    FakeShadowRealm.importFails = "no such module";
    frameSourceChanged({}, 3, "app:///child.web.bundle");
    await drain();
    expect(calls.filter(call => call[0] === "reportError")).toEqual([
      ["reportError", "frame app:///child.web.bundle: no such module"],
    ]);
  });
});

describe("connection", () => {
  it("disposes of the card on disconnect and loads src again on connect", async () => {
    already.add("app:///child.web.bundle");
    frameSourceChanged(frameHandle(3), 3, "app:///child.web.bundle");
    await drain();
    // The connection that follows the first write finds the load present.
    __BobcatFrameConnected(3);
    await drain();
    expect(FakeShadowRealm.created).toBe(1);

    calls.length = 0;
    __BobcatFrameDisconnected(3);
    expect(calls).toEqual([["dispose", 1], ["resetFrame", 3]]);
    // A second disconnection has nothing left to let go of.
    __BobcatFrameDisconnected(3);
    expect(calls).toEqual([["dispose", 1], ["resetFrame", 3]]);

    calls.length = 0;
    __BobcatFrameConnected(3);
    await drain();
    expect(FakeShadowRealm.created).toBe(2);
    expect(calls[0]).toEqual(["fetchResource", "app:///child.web.bundle"]);
    expect(calls.at(-1)).toEqual(["boot", 3, "app:///child.web.bundle", "{}", "{}", SYSTEM_INFO]);
  });

  it("does not start a second load for a frame connected while its first is in flight", async () => {
    frameSourceChanged(frameHandle(3), 3, "app:///child.web.bundle");
    __BobcatFrameConnected(3);
    expect(calls.filter(call => call[0] === "fetchResource")).toHaveLength(1);
    future.__BobcatSettleFuture(futures()[0]!, false, undefined);
    await drain();
    expect(FakeShadowRealm.created).toBe(1);
  });

  it("makes a load in flight stale on disconnect", async () => {
    frameSourceChanged(frameHandle(3), 3, "app:///child.web.bundle");
    __BobcatFrameDisconnected(3);
    expect(calls.at(-1)).toEqual(["resetFrame", 3]);
    future.__BobcatSettleFuture(futures()[0]!, false, undefined);
    await drain();
    expect(FakeShadowRealm.created).toBe(0);
  });

  it("ignores a frame that never had a src, or whose handle is another realm's", () => {
    __BobcatFrameConnected(41);
    __BobcatFrameDisconnected(41);
    frameSourceChanged(frameHandle(4), 4, null);
    __BobcatFrameConnected(4);
    // A handle this realm does not hold, though the frame has a card.
    frameSourceChanged({}, 5, "app:///child.web.bundle");
    calls.length = 0;
    __BobcatFrameConnected(5);
    __BobcatFrameDisconnected(5);
    __BobcatFrameDisconnected(4);
    expect(calls).toEqual([]);
  });

  it("closes every card this realm's frames hold, without touching the document", async () => {
    already.add("app:///a.web.bundle");
    already.add("app:///b.web.bundle");
    frameSourceChanged(frameHandle(3), 3, "app:///a.web.bundle");
    frameSourceChanged(frameHandle(4), 4, "app:///b.web.bundle");
    await drain();
    calls.length = 0;
    __BobcatCloseFrames();
    expect(calls).toEqual([["dispose", 1], ["dispose", 2]]);
    // Closed is closed: a disconnection after it disposes of nothing more.
    calls.length = 0;
    __BobcatFrameDisconnected(3);
    expect(calls).toEqual([]);
  });
});
