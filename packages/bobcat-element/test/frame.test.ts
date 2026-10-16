// `bobcat:frame`: what a frame's `src`, `data`, `global-props`, connection and
// disconnection do in the realm that holds its handle, and how an event's
// steps reach a booted card, over stand-ins for the host members it reaches
// (`resolveUrl`, `fetchResource`, `resetFrame`, `parentNode`,
// `callElementMethod` and the `Future` trio), for the diagnostics pair, and
// for `ShadowRealm`.
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

/** The view's base URL the stand-in `resolveUrl` resolves against. */
const BASE = "app:///cards/";

/** The page root `__GetPageElement` answers, by node id. */
const PAGE = 1;

/**
 * Each node's parent, which the stand-in `parentNode` answers: every frame a
 * test makes is a child of [`PAGE`] unless the test moves it.
 */
const parents: Map<number, number | null> = new Map();

/** The border-box origin `boundingClientRect` answers, by node id. */
const origins: Map<number, [number, number]> = new Map();

rstest.mockRequire("bobcat-internal:host", () => ({
  // The host's: URL rules against the view's base, throwing for a URL that
  // does not resolve, as `HostOutbox::resolve` does.
  resolveUrl(url: string): string {
    calls.push(["resolveUrl", url]);
    return new URL(url, BASE).href;
  },
  fetchResource(url: string): number | true {
    calls.push(["fetchResource", url]);
    if (already.has(url)) return true;
    return nextFuture++;
  },
  resetFrame(frame: number): undefined {
    calls.push(["resetFrame", frame]);
    return undefined;
  },
  parentNode(node: number): number | null {
    return parents.get(node) ?? null;
  },
  // The base set's record: the code, then `left`, `top`, `width`, `height`.
  callElementMethod(node: number, method: string, params: string): string | number {
    if (method !== "boundingClientRect" || params !== "") return 3;
    const [left, top] = origins.get(node) ?? [0, 0];
    return [0, left, top, 100, 50].map(field => `${String(field).length}:${field}`).join("");
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
import * as record from "../src/record.ts";
rstest.mockRequire("bobcat:record", () => record);

rstest.mockRequire("bobcat:diagnostics", () => ({
  reportError(error: unknown) {
    calls.push(["reportError", error instanceof Error ? error.message : String(error)]);
  },
}));

/** The handles `bobcat:element`'s table resolves, by node id. */
const handles: Map<number, object> = new Map();

/** The page root's handle, which `__GetPageElement` answers. */
const pageHandle = { page: PAGE };

/** A frame handle this realm holds under `frameId`, a child of the page. */
function frameHandle(frameId: number): object {
  const handle = {};
  handles.set(frameId, handle);
  parents.set(frameId, PAGE);
  return handle;
}

rstest.mockRequire("bobcat:element", () => ({
  __BobcatHandleOf(nodeId: number): object | undefined {
    return handles.get(nodeId);
  },
  __GetPageElement(): object {
    return pageHandle;
  },
  __GetElementUniqueID(handle: object): number {
    return handle === pageHandle ? PAGE : -1;
  },
}));

/** The view's native module table, as the page realm holds it. */
const MODULE_TABLE = "4:Echo4:ping";

rstest.mockRequire("bobcat:runtime", () => ({
  SystemInfo: { platform: "headless", pixelRatio: 2 },
  __BobcatNativeModuleTable: () => MODULE_TABLE,
}));

/**
 * A stand-in for the host's `ShadowRealm`: its child's `boot`, `dispose`,
 * `updateData`, `updateGlobalProps`, `dispatchEvent`, `sendExposure` and
 * `frameConnection` record their calls, each but `boot` with the number of
 * its realm; `frameConnection` answers whether the realm's number is among
 * those [`FakeShadowRealm.holding`] names for the frame.
 */
class FakeShadowRealm {
  /** Which child realms hold which frames, by frame id. */
  static holding: Map<number, number> = new Map();
  static created = 0;
  /** What the next `importValue` settles with, if not a recording export. */
  static importFails: string | undefined;
  /** What the child's `boot` answers: `false` is a boot that failed. */
  static bootAnswers = true;
  /** Set to make the child's update functions throw this, after recording. */
  static updateFails: string | undefined;
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
    const exported = name === "boot"
      ? (...args: unknown[]) => {
        calls.push(["boot", ...args]);
        return FakeShadowRealm.bootAnswers;
      }
      : name === "frameConnection"
      ? (...args: unknown[]) => {
        calls.push([name, this.number, ...args]);
        return FakeShadowRealm.holding.get(args[0] as number) === this.number;
      }
      : (...args: unknown[]) => {
        calls.push([name, this.number, ...args]);
        const failure = FakeShadowRealm.updateFails;
        if (failure !== undefined && name !== "dispose") throw new TypeError(failure);
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
  forwardEvent,
  forwardExposure,
  frameCardOf,
  frameCards,
  frameDataChanged,
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
  parents.clear();
  origins.clear();
  FakeShadowRealm.created = 0;
  FakeShadowRealm.importFails = undefined;
  FakeShadowRealm.bootAnswers = true;
  FakeShadowRealm.updateFails = undefined;
  FakeShadowRealm.hold = false;
  FakeShadowRealm.held = [];
  FakeShadowRealm.holding.clear();
  (globalThis as Record<string, unknown>)["ShadowRealm"] = FakeShadowRealm;
});

describe("frameSourceChanged", () => {
  it("resolves src against the view's base once, fetches that, and boots it in a fresh realm once the fetch settles", async () => {
    const frame = {};
    frameSourceChanged(frame, 3, "child.web.bundle");
    const [id] = futures();
    // A frame with no card yet has nothing to release.
    expect(calls).toEqual([
      ["resolveUrl", "child.web.bundle"],
      ["fetchResource", "app:///cards/child.web.bundle"],
      ["settleFuture", id],
    ]);
    await drain();
    expect(FakeShadowRealm.created).toBe(0);

    future.__BobcatSettleFuture(id!, false, undefined);
    await drain();
    expect(FakeShadowRealm.created).toBe(1);
    // The card boots at the resolved URL, with the view's module table for
    // its BTS.
    expect(calls.slice(3)).toEqual([
      ["importValue", "bobcat:frame-boot", "boot"],
      ["importValue", "bobcat:frame-boot", "dispose"],
      ["importValue", "bobcat:frame-boot", "updateData"],
      ["importValue", "bobcat:frame-boot", "updateGlobalProps"],
      ["importValue", "bobcat:frame-boot", "dispatchEvent"],
      ["importValue", "bobcat:frame-boot", "sendExposure"],
      ["importValue", "bobcat:frame-boot", "frameConnection"],
      ["boot", 3, "app:///cards/child.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE],
    ]);

    // The frame keeps `src` as written: the same string again is unchanged.
    calls.length = 0;
    frameSourceChanged(frame, 3, "child.web.bundle");
    expect(calls).toEqual([]);
  });

  it("reports a src that does not resolve once, naming it, and fetches nothing", async () => {
    frameSourceChanged({}, 3, "http://[");
    await drain();
    expect(calls.filter(call => call[0] === "fetchResource")).toEqual([]);
    expect(FakeShadowRealm.created).toBe(0);
    const reports = calls.filter(call => call[0] === "reportError");
    expect(reports).toHaveLength(1);
    expect(reports[0]![1]).toMatch(/^frame http:\/\/\[: /);
  });

  it("defers a probe hit to a microtask", async () => {
    already.add("app:///cached.web.bundle");
    frameSourceChanged({}, 3, "app:///cached.web.bundle");
    expect(calls).toEqual([
      ["resolveUrl", "app:///cached.web.bundle"],
      ["fetchResource", "app:///cached.web.bundle"],
    ]);
    expect(FakeShadowRealm.created).toBe(0);
    await drain();
    expect(FakeShadowRealm.created).toBe(1);
    expect(calls.at(-1)).toEqual(["boot", 3, "app:///cached.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE]);
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
      ["boot", 3, "app:///second.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE],
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
      ["boot", 3, "app:///child.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE],
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
      ["resolveUrl", "app:///second.web.bundle"],
      ["fetchResource", "app:///second.web.bundle"],
    ]);
    await drain();
    expect(FakeShadowRealm.created).toBe(2);
    expect(calls.at(-1)).toEqual(["boot", 3, "app:///second.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE]);
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
    expect(calls.slice(0, 2)).toEqual([
      ["resolveUrl", "app:///child.web.bundle"],
      ["fetchResource", "app:///child.web.bundle"],
    ]);
    expect(calls.at(-1)).toEqual(["boot", 3, "app:///child.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE]);
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
    // Nobody holds 41: no card of this realm's, which has none, either.
    expect(__BobcatFrameConnected(41)).toBe(false);
    expect(__BobcatFrameDisconnected(41)).toBe(false);
    frameSourceChanged(frameHandle(4), 4, null);
    // This realm's, with nothing to load: taken all the same.
    expect(__BobcatFrameConnected(4)).toBe(true);
    // A handle this realm does not hold, though the frame has a card.
    frameSourceChanged({}, 5, "app:///child.web.bundle");
    calls.length = 0;
    __BobcatFrameConnected(5);
    __BobcatFrameDisconnected(5);
    __BobcatFrameDisconnected(4);
    expect(calls).toEqual([]);
  });

  it("hands a frame it does not hold to each booted card in turn, until one takes it", async () => {
    already.add("app:///a.web.bundle");
    already.add("app:///b.web.bundle");
    frameSourceChanged(frameHandle(3), 3, "app:///a.web.bundle");
    frameSourceChanged(frameHandle(4), 4, "app:///b.web.bundle");
    await drain();
    // Frame 9 is the second card's: a frame it made in its own tree.
    FakeShadowRealm.holding.set(9, 2);
    calls.length = 0;

    expect(__BobcatFrameDisconnected(9)).toBe(true);
    expect(__BobcatFrameConnected(9)).toBe(true);
    expect(calls).toEqual([
      ["frameConnection", 1, 9, false],
      ["frameConnection", 2, 9, false],
      ["frameConnection", 1, 9, true],
      ["frameConnection", 2, 9, true],
    ]);

    // One no card holds is asked of every card, and nobody's.
    calls.length = 0;
    expect(__BobcatFrameDisconnected(10)).toBe(false);
    expect(calls).toEqual([
      ["frameConnection", 1, 10, false],
      ["frameConnection", 2, 10, false],
    ]);

    // A frame of this realm's own is never handed on.
    calls.length = 0;
    expect(__BobcatFrameDisconnected(3)).toBe(true);
    expect(calls).toEqual([["dispose", 1], ["resetFrame", 3]]);
  });

  it("reports a card's answer that throws across the boundary, naming its src, and asks the next", async () => {
    already.add("app:///a.web.bundle");
    already.add("app:///b.web.bundle");
    frameSourceChanged(frameHandle(3), 3, "app:///a.web.bundle");
    await drain();
    // Its boot answered after the first: its realm is the second.
    frameSourceChanged(frameHandle(4), 4, "app:///b.web.bundle");
    await drain();
    FakeShadowRealm.holding.set(9, 2);
    const first = frameCardOf(3) as { frameConnection: unknown };
    Object.defineProperty(first, "frameConnection", {
      value: () => {
        throw new TypeError("the realm is gone");
      },
    });
    calls.length = 0;

    expect(__BobcatFrameConnected(9)).toBe(true);
    expect(calls).toEqual([
      ["reportError", "frame app:///a.web.bundle: the realm is gone"],
      ["frameConnection", 2, 9, true],
    ]);
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

describe("data and global-props", () => {
  const CHILD = "app:///child.web.bundle";
  const SECOND = "app:///second.web.bundle";

  /** The `boot` calls so far. */
  function boots(): unknown[][] {
    return calls.filter(call => call[0] === "boot");
  }

  /** The calls of either update function so far. */
  function updates(): unknown[][] {
    return calls.filter(call => call[0] === "updateData" || call[0] === "updateGlobalProps");
  }

  it("boots the first card with the data and global-props written before src", async () => {
    already.add(CHILD);
    const frame = frameHandle(3);
    frameDataChanged(frame, "data", '{"label":"first"}');
    frameDataChanged(frame, "global-props", '{"theme":"dark"}');
    // There is no card yet to hand them to.
    expect(calls).toEqual([]);
    frameSourceChanged(frame, 3, CHILD);
    await drain();
    expect(boots()).toEqual([
      ["boot", 3, CHILD, '{"label":"first"}', '{"theme":"dark"}', SYSTEM_INFO, MODULE_TABLE],
    ]);
    expect(updates()).toEqual([]);
  });

  for (const [name, update] of [["data", "updateData"], ["global-props", "updateGlobalProps"]] as const) {
    describe(name, () => {
      /** `boot`'s call for `src` with `json` as this attribute and `{}` as the other. */
      function booted(src: string, json: string): unknown[] {
        return name === "data"
          ? ["boot", 3, src, json, "{}", SYSTEM_INFO, MODULE_TABLE]
          : ["boot", 3, src, "{}", json, SYSTEM_INFO, MODULE_TABLE];
      }

      it("hands every write after boot to the card's update, equal or not, and boots nothing", async () => {
        already.add(CHILD);
        const frame = frameHandle(3);
        frameSourceChanged(frame, 3, CHILD);
        await drain();
        calls.length = 0;
        frameDataChanged(frame, name, '{"label":"second"}');
        frameDataChanged(frame, name, '{"label":"second"}');
        expect(calls).toEqual([
          [update, 1, '{"label":"second"}'],
          [update, 1, '{"label":"second"}'],
        ]);
        await drain();
        expect(FakeShadowRealm.created).toBe(1);
        expect(boots()).toEqual([]);
      });

      it("only keeps a write while the fetch is in flight, which boot then reads", async () => {
        const frame = frameHandle(3);
        frameSourceChanged(frame, 3, CHILD);
        frameDataChanged(frame, name, '{"label":"late"}');
        future.__BobcatSettleFuture(futures()[0]!, false, undefined);
        await drain();
        expect(boots()).toEqual([booted(CHILD, '{"label":"late"}')]);
        expect(updates()).toEqual([]);
      });

      it("only keeps a write while the import is in flight, which boot then reads", async () => {
        already.add(CHILD);
        FakeShadowRealm.hold = true;
        const frame = frameHandle(3);
        frameSourceChanged(frame, 3, CHILD);
        await drain();
        expect(FakeShadowRealm.created).toBe(1);
        frameDataChanged(frame, name, '{"label":"late"}');
        FakeShadowRealm.release();
        await drain();
        expect(boots()).toEqual([booted(CHILD, '{"label":"late"}')]);
        expect(updates()).toEqual([]);
      });

      it("leaves the running card alone on removal, and boots the next card with {}", async () => {
        already.add(CHILD);
        already.add(SECOND);
        const frame = frameHandle(3);
        frameDataChanged(frame, name, '{"label":"first"}');
        frameSourceChanged(frame, 3, CHILD);
        await drain();
        calls.length = 0;
        frameDataChanged(frame, name, null);
        expect(calls).toEqual([]);
        frameSourceChanged(frame, 3, SECOND);
        await drain();
        expect(boots()).toEqual([booted(SECOND, "{}")]);
      });

      it("ignores anything but a plain object, keeping what it had", async () => {
        already.add(CHILD);
        const frame = frameHandle(3);
        frameDataChanged(frame, name, '{"label":"kept"}');
        frameSourceChanged(frame, 3, CHILD);
        await drain();
        calls.length = 0;
        for (const ignored of ["3", "true", "null", '"text"', "[1]", "nope", "", "[object Object]"]) {
          frameDataChanged(frame, name, ignored);
        }
        expect(calls).toEqual([]);
        // Nor did any of them replace what the next boot reads.
        __BobcatFrameDisconnected(3);
        __BobcatFrameConnected(3);
        await drain();
        expect(boots()).toEqual([booted(CHILD, '{"label":"kept"}')]);
      });

      it("boots a new src with the last value written", async () => {
        already.add(CHILD);
        already.add(SECOND);
        const frame = frameHandle(3);
        frameDataChanged(frame, name, '{"label":"first"}');
        frameSourceChanged(frame, 3, CHILD);
        await drain();
        frameDataChanged(frame, name, '{"label":"second"}');
        calls.length = 0;
        frameSourceChanged(frame, 3, SECOND);
        await drain();
        expect(calls[0]).toEqual(["dispose", 1]);
        expect(boots()).toEqual([booted(SECOND, '{"label":"second"}')]);
        // The second card is the one a write now reaches.
        calls.length = 0;
        frameDataChanged(frame, name, '{"label":"third"}');
        expect(calls).toEqual([[update, 2, '{"label":"third"}']]);
      });

      it("boots a reconnected frame with the last value written, and updates no card while it has none", async () => {
        already.add(CHILD);
        const frame = frameHandle(3);
        frameSourceChanged(frame, 3, CHILD);
        await drain();
        __BobcatFrameDisconnected(3);
        calls.length = 0;
        frameDataChanged(frame, name, '{"label":"while away"}');
        expect(calls).toEqual([]);
        __BobcatFrameConnected(3);
        await drain();
        expect(boots()).toEqual([booted(CHILD, '{"label":"while away"}')]);
        expect(updates()).toEqual([]);
      });

      it("updates no card whose boot failed", async () => {
        already.add(CHILD);
        FakeShadowRealm.bootAnswers = false;
        const frame = frameHandle(3);
        frameSourceChanged(frame, 3, CHILD);
        await drain();
        calls.length = 0;
        frameDataChanged(frame, name, '{"label":"second"}');
        expect(calls).toEqual([]);
      });

      it("reports an update that throws across the boundary once, naming src", async () => {
        already.add(CHILD);
        const frame = frameHandle(3);
        frameSourceChanged(frame, 3, CHILD);
        await drain();
        calls.length = 0;
        FakeShadowRealm.updateFails = "the realm is gone";
        frameDataChanged(frame, name, '{"label":"second"}');
        expect(calls).toEqual([
          [update, 1, '{"label":"second"}'],
          ["reportError", `frame ${CHILD}: the realm is gone`],
        ]);
      });
    });
  }
});

describe("events", () => {
  const CHILD = "app:///child.web.bundle";
  const SECOND = "app:///second.web.bundle";

  /** The steps and the event a test hands a card, as `bobcat:element` would. */
  const PAYLOAD = [0, 0, 120, 70, undefined, undefined, 0, 0, 1, "touch"];

  function forward(card: object, nodes = "9,8", hosts = "3,3"): void {
    forwardEvent(card, nodes, "9,9", hosts, "tap", true, 12.5, PAYLOAD[0], PAYLOAD.slice(1));
  }

  /** The calls of the child's `dispatchEvent` so far. */
  function dispatched(): unknown[][] {
    return calls.filter(call => call[0] === "dispatchEvent");
  }

  /** Boots `src` in the frame `frameId`, which this realm holds. */
  async function booted(frameId: number, src = CHILD): Promise<object> {
    already.add(src);
    const frame = handles.get(frameId) ?? frameHandle(frameId);
    frameSourceChanged(frame, frameId, src);
    await drain();
    return frame;
  }

  it("has no card for a frame until its card booted, nor for one whose boot failed", async () => {
    FakeShadowRealm.hold = true;
    already.add(CHILD);
    frameSourceChanged(frameHandle(3), 3, CHILD);
    await drain();
    // Fetched and importing: no card yet.
    expect(frameCardOf(3)).toBeUndefined();
    expect(frameCards()).toEqual([]);
    FakeShadowRealm.release();
    await drain();
    expect(frameCardOf(3)).toBeDefined();
    expect(frameCards()).toEqual([frameCardOf(3)]);

    FakeShadowRealm.hold = false;
    FakeShadowRealm.bootAnswers = false;
    await booted(4);
    expect(frameCardOf(4)).toBeUndefined();
    // Nor is a node this realm holds no frame handle for anything's.
    expect(frameCardOf(5)).toBeUndefined();
  });

  it("hands a card its steps through the child's dispatchEvent, with the frame's border-box origin", async () => {
    await booted(3);
    origins.set(3, [20, 30]);
    const card = frameCardOf(3)!;
    calls.length = 0;
    forward(card);
    // Every argument a primitive: the origin, the three id lists, the event
    // and its payload as this realm received them.
    expect(dispatched()).toEqual([
      ["dispatchEvent", 1, 20, 30, "9,8", "9,9", "3,3", "tap", true, 12.5, ...PAYLOAD],
    ]);
    expect(calls.filter(call => call[0] === "reportError")).toEqual([]);
  });

  it("hands nothing to a card the frame let go of since the event was split", async () => {
    const frame = await booted(3);
    const card = frameCardOf(3)!;
    // A listener wrote a new `src`: the first card is torn down.
    already.add(SECOND);
    frameSourceChanged(frame, 3, SECOND);
    calls.length = 0;
    forward(card);
    expect(dispatched()).toEqual([]);
    // The new card is another record, which hears what it is handed.
    await drain();
    const second = frameCardOf(3)!;
    expect(second).not.toBe(card);
    forward(second);
    expect(dispatched()).toHaveLength(1);
    expect(dispatched()[0]![1]).toBe(2);

    // A disconnection lets go of it too.
    __BobcatFrameDisconnected(3);
    calls.length = 0;
    forward(second);
    expect(dispatched()).toEqual([]);
    expect(frameCardOf(3)).toBeUndefined();
    expect(frameCards()).toEqual([]);
  });

  it("hands nothing to the card of a frame no longer in this realm's tree, without a report", async () => {
    await booted(3);
    // Under a wrapper that a listener took out of the page: the host
    // delivers the disconnection in a later entry, so the card still runs.
    parents.set(3, 7);
    parents.set(7, null);
    const card = frameCardOf(3)!;
    calls.length = 0;
    forward(card);
    expect(dispatched()).toEqual([]);
    expect(calls).toEqual([]);
    // Back under the page through the wrapper, it hears again.
    parents.set(7, PAGE);
    forward(card);
    expect(dispatched()).toHaveLength(1);
  });

  it("lists every booted card for a key event, in load order", async () => {
    await booted(3);
    await booted(4, SECOND);
    expect(frameCards()).toEqual([frameCardOf(3), frameCardOf(4)]);
    calls.length = 0;
    for (const card of frameCards()) {
      forwardEvent(card, "", "", "", "keydown", true, 0, 0, [4, "a", "KeyA", false, 0]);
    }
    expect(dispatched()).toEqual([
      ["dispatchEvent", 1, 0, 0, "", "", "", "keydown", true, 0, 0, 4, "a", "KeyA", false, 0],
      ["dispatchEvent", 2, 0, 0, "", "", "", "keydown", true, 0, 0, 4, "a", "KeyA", false, 0],
    ]);
  });

  it("reports a hand-over that throws across the boundary once, naming src", async () => {
    await booted(3);
    const card = frameCardOf(3)!;
    calls.length = 0;
    FakeShadowRealm.updateFails = "the realm is gone";
    forward(card);
    expect(calls.filter(call => call[0] === "reportError")).toEqual([
      ["reportError", `frame ${CHILD}: the realm is gone`],
    ]);
  });
});

describe("exposure", () => {
  const CHILD = "app:///child.web.bundle";
  const SECOND = "app:///second.web.bundle";

  /** The calls of the child's `sendExposure` so far. */
  function sent(): unknown[][] {
    return calls.filter(call => call[0] === "sendExposure");
  }

  it("hands a booted card its records through the child's sendExposure, whether or not its frame is in this realm's tree", async () => {
    already.add(CHILD);
    frameSourceChanged(frameHandle(3), 3, CHILD);
    await drain();
    const card = frameCardOf(3)!;
    calls.length = 0;
    forwardExposure(card, "exposure", "1:91:a0:0:");
    // Out of the tree: its leaving's disexposures come before its
    // disconnection, and still reach its card.
    parents.set(3, null);
    forwardExposure(card, "disexposure", "1:91:a0:0:");
    expect(sent()).toEqual([
      ["sendExposure", 1, "exposure", "1:91:a0:0:"],
      ["sendExposure", 1, "disexposure", "1:91:a0:0:"],
    ]);
    expect(calls.filter(call => call[0] === "reportError")).toEqual([]);
  });

  it("hands nothing to a card the frame let go of", async () => {
    already.add(CHILD);
    already.add(SECOND);
    const frame = frameHandle(3);
    frameSourceChanged(frame, 3, CHILD);
    await drain();
    const card = frameCardOf(3)!;
    frameSourceChanged(frame, 3, SECOND);
    await drain();
    calls.length = 0;
    forwardExposure(card, "disexposure", "1:91:a0:0:");
    expect(sent()).toEqual([]);
    // The new card is another record.
    forwardExposure(frameCardOf(3)!, "disexposure", "1:91:a0:0:");
    expect(sent()).toEqual([["sendExposure", 2, "disexposure", "1:91:a0:0:"]]);
  });

  it("reports a hand-over that throws across the boundary once, naming src", async () => {
    already.add(CHILD);
    frameSourceChanged(frameHandle(3), 3, CHILD);
    await drain();
    calls.length = 0;
    FakeShadowRealm.updateFails = "the realm is gone";
    forwardExposure(frameCardOf(3)!, "exposure", "1:91:a0:0:");
    expect(calls.filter(call => call[0] === "reportError")).toEqual([
      ["reportError", `frame ${CHILD}: the realm is gone`],
    ]);
  });
});
