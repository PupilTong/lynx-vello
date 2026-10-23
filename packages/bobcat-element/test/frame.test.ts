// `bobcat:frame`: what a frame's `src`, `data`, `global-props`, connection and
// disconnection do in the realm that holds its handle, over stand-ins for the
// host members it reaches (`resolveUrl`, `fetchResource`, `resetFrame` and
// the `Future` trio), for the diagnostics pair, and for `ShadowRealm`.
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

/** The frame handles this realm holds, by node id. */
const handles: Map<number, object> = new Map();

/** A frame handle this realm holds under `frameId`. */
function frameHandle(frameId: number): object {
  const handle = {};
  handles.set(frameId, handle);
  return handle;
}

/**
 * The host's connection event at the frame `frameId`, as `bobcat:element`'s
 * listener hands it over: with the handle this realm holds.
 */
function connection(frameId: number, connected: boolean): void {
  frameConnectionChanged(handles.get(frameId)!, frameId, connected);
}

/** The view's native module table, as the page realm holds it. */
const MODULE_TABLE = "4:Echo4:ping";

rstest.mockRequire("bobcat:runtime", () => ({
  SystemInfo: { platform: "headless", pixelRatio: 2 },
  __BobcatNativeModuleTable: () => MODULE_TABLE,
}));

/**
 * A stand-in for the host's `ShadowRealm`: its child's `boot`, `dispose`,
 * `updateData` and `updateGlobalProps` record their calls, each but `boot`
 * with the number of its realm. Its `dispose` calls the parent's `done` at
 * once, as a card whose exposure stop queued nothing does, unless
 * `holdDone` is set: then `done` waits in `dones` for the test, as a card
 * waiting for its last records' delivery.
 */
class FakeShadowRealm {
  static created = 0;
  /** Set to keep each child's `done` for the test to call. */
  static holdDone = false;
  static dones: (() => void)[] = [];
  /** Set to make the child's `dispose` throw this, after recording. */
  static disposeFails: string | undefined;
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
      : name === "dispose"
      ? (done: () => void) => {
        calls.push(["dispose", this.number]);
        if (FakeShadowRealm.disposeFails !== undefined) {
          throw new TypeError(FakeShadowRealm.disposeFails);
        }
        if (FakeShadowRealm.holdDone) FakeShadowRealm.dones.push(done);
        else done();
      }
      : (...args: unknown[]) => {
        calls.push([name, this.number, ...args]);
        const failure = FakeShadowRealm.updateFails;
        if (failure !== undefined) throw new TypeError(failure);
      };
    if (!FakeShadowRealm.hold) return Promise.resolve(exported);
    return new Promise(resolve => {
      FakeShadowRealm.held.push(() => resolve(exported));
    });
  }
}

import {
  __BobcatCloseFrames,
  frameConnectionChanged,
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
  // What an earlier test left loaded or waiting is this module's, which
  // every test shares.
  for (const done of FakeShadowRealm.dones.splice(0)) done();
  FakeShadowRealm.holdDone = false;
  FakeShadowRealm.disposeFails = undefined;
  __BobcatCloseFrames();
  calls.length = 0;
  already.clear();
  handles.clear();
  FakeShadowRealm.created = 0;
  FakeShadowRealm.importFails = undefined;
  FakeShadowRealm.bootAnswers = true;
  FakeShadowRealm.updateFails = undefined;
  FakeShadowRealm.hold = false;
  FakeShadowRealm.held = [];
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
    // The card boots at the resolved URL, with `src` as written for the
    // `url` of its `load`, and the view's module table for its BTS.
    expect(calls.slice(3)).toEqual([
      ["importValue", "bobcat:frame-boot", "boot"],
      ["importValue", "bobcat:frame-boot", "dispose"],
      ["importValue", "bobcat:frame-boot", "updateData"],
      ["importValue", "bobcat:frame-boot", "updateGlobalProps"],
      ["boot", 3, "app:///cards/child.web.bundle", "child.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE],
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
    expect(calls.at(-1)).toEqual(["boot", 3, "app:///cached.web.bundle", "app:///cached.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE]);
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
      ["boot", 3, "app:///second.web.bundle", "app:///second.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE],
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
      ["boot", 3, "app:///child.web.bundle", "app:///child.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE],
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
    // The old card posted its last list at once: its page is reset before
    // the new load starts.
    expect(calls).toEqual([
      ["dispose", 1],
      ["resetFrame", 3],
      ["resolveUrl", "app:///second.web.bundle"],
      ["fetchResource", "app:///second.web.bundle"],
    ]);
    await drain();
    expect(FakeShadowRealm.created).toBe(2);
    expect(calls.at(-1)).toEqual(["boot", 3, "app:///second.web.bundle", "app:///second.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE]);
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
    connection(3, true);
    await drain();
    expect(FakeShadowRealm.created).toBe(1);

    calls.length = 0;
    connection(3, false);
    const reset = [["dispose", 1], ["resetFrame", 3]];
    expect(calls).toEqual(reset);
    // A second disconnection has nothing left to let go of.
    connection(3, false);
    expect(calls).toEqual(reset);

    calls.length = 0;
    connection(3, true);
    await drain();
    expect(FakeShadowRealm.created).toBe(2);
    expect(calls.slice(0, 2)).toEqual([
      ["resolveUrl", "app:///child.web.bundle"],
      ["fetchResource", "app:///child.web.bundle"],
    ]);
    expect(calls.at(-1)).toEqual(["boot", 3, "app:///child.web.bundle", "app:///child.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE]);
  });

  it("does not start a second load for a frame connected while its first is in flight", async () => {
    frameSourceChanged(frameHandle(3), 3, "app:///child.web.bundle");
    connection(3, true);
    expect(calls.filter(call => call[0] === "fetchResource")).toHaveLength(1);
    future.__BobcatSettleFuture(futures()[0]!, false, undefined);
    await drain();
    expect(FakeShadowRealm.created).toBe(1);
  });

  it("makes a load in flight stale on disconnect", async () => {
    frameSourceChanged(frameHandle(3), 3, "app:///child.web.bundle");
    connection(3, false);
    expect(calls.at(-1)).toEqual(["resetFrame", 3]);
    future.__BobcatSettleFuture(futures()[0]!, false, undefined);
    await drain();
    expect(FakeShadowRealm.created).toBe(0);
  });

  it("ignores a frame that never had a src", () => {
    frameHandle(41);
    connection(41, true);
    connection(41, false);
    frameSourceChanged(frameHandle(4), 4, null);
    connection(4, true);
    connection(4, false);
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
    connection(3, false);
    expect(calls).toEqual([]);
  });
});

describe("the last exposure list", () => {
  const FIRST = "app:///first.web.bundle";
  const SECOND = "app:///second.web.bundle";

  /** Boots `src` in frame 3 and answers the frame's handle. */
  async function booted(src: string): Promise<object> {
    already.add(src);
    const frame = frameHandle(3);
    frameSourceChanged(frame, 3, src);
    await drain();
    return frame;
  }

  it("resets the frame only once the card let go of has posted its last list", async () => {
    await booted(FIRST);
    FakeShadowRealm.holdDone = true;
    calls.length = 0;
    connection(3, false);
    await drain();
    expect(calls).toEqual([["dispose", 1]]);

    FakeShadowRealm.dones.shift()!();
    expect(calls).toEqual([["dispose", 1], ["resetFrame", 3]]);
    // Once only, however often the card calls it.
    expect(calls.filter(call => call[0] === "resetFrame")).toHaveLength(1);
  });

  it("boots a new src only after the old card's reset, into the fresh page", async () => {
    const frame = await booted(FIRST);
    already.add(SECOND);
    FakeShadowRealm.holdDone = true;
    calls.length = 0;
    frameSourceChanged(frame, 3, SECOND);
    await drain();
    // Fetched already: its start came in a microtask, and waits.
    expect(FakeShadowRealm.created).toBe(1);
    expect(calls).toEqual([
      ["dispose", 1],
      ["resolveUrl", SECOND],
      ["fetchResource", SECOND],
    ]);

    FakeShadowRealm.dones.shift()!();
    await drain();
    expect(FakeShadowRealm.created).toBe(2);
    expect(calls.slice(3)).toEqual([
      ["resetFrame", 3],
      ["importValue", "bobcat:frame-boot", "boot"],
      ["importValue", "bobcat:frame-boot", "dispose"],
      ["importValue", "bobcat:frame-boot", "updateData"],
      ["importValue", "bobcat:frame-boot", "updateGlobalProps"],
      ["boot", 3, SECOND, SECOND, "{}", "{}", SYSTEM_INFO, MODULE_TABLE],
    ]);
  });

  it("resets once for teardowns that pile up, and boots only the last src", async () => {
    const frame = await booted(FIRST);
    already.add(SECOND);
    already.add("app:///third.web.bundle");
    FakeShadowRealm.holdDone = true;
    calls.length = 0;
    frameSourceChanged(frame, 3, SECOND);
    await drain();
    // The second card never started, so it has nothing to dispose of.
    frameSourceChanged(frame, 3, "app:///third.web.bundle");
    connection(3, false);
    connection(3, true);
    await drain();
    expect(calls.filter(call => call[0] === "resetFrame")).toEqual([]);
    expect(FakeShadowRealm.created).toBe(1);

    FakeShadowRealm.dones.shift()!();
    await drain();
    expect(calls.filter(call => call[0] === "resetFrame")).toEqual([["resetFrame", 3]]);
    expect(calls.filter(call => call[0] === "boot")).toEqual([
      ["boot", 3, "app:///third.web.bundle", "app:///third.web.bundle", "{}", "{}", SYSTEM_INFO, MODULE_TABLE],
    ]);
  });

  it("closes this realm's frames once every card, and every reset still waiting, is done", async () => {
    already.add("app:///a.web.bundle");
    already.add("app:///b.web.bundle");
    frameSourceChanged(frameHandle(3), 3, "app:///a.web.bundle");
    frameSourceChanged(frameHandle(4), 4, "app:///b.web.bundle");
    await drain();
    FakeShadowRealm.holdDone = true;
    // Frame 3 let go of its card before the realm closes, still waiting.
    connection(3, false);
    let closed = 0;
    __BobcatCloseFrames(() => {
      closed += 1;
    });
    expect(closed).toBe(0);
    // Frame 4's card, then frame 3's: its reset runs, and then the callback.
    FakeShadowRealm.dones[1]!();
    expect(closed).toBe(0);
    FakeShadowRealm.dones[0]!();
    expect(closed).toBe(1);
    FakeShadowRealm.dones.length = 0;

    // Nothing to wait for: at once.
    __BobcatCloseFrames(() => {
      closed += 1;
    });
    expect(closed).toBe(2);
  });

  it("resets all the same when the card's dispose throws, and reports it", async () => {
    await booted(FIRST);
    FakeShadowRealm.disposeFails = "the realm is gone";
    calls.length = 0;
    connection(3, false);
    expect(calls).toEqual([
      ["dispose", 1],
      ["reportError", `frame ${FIRST}: the realm is gone`],
      ["resetFrame", 3],
    ]);
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
      ["boot", 3, CHILD, CHILD, '{"label":"first"}', '{"theme":"dark"}', SYSTEM_INFO, MODULE_TABLE],
    ]);
    expect(updates()).toEqual([]);
  });

  for (const [name, update] of [["data", "updateData"], ["global-props", "updateGlobalProps"]] as const) {
    describe(name, () => {
      /** `boot`'s call for `src` with `json` as this attribute and `{}` as the other. */
      function booted(src: string, json: string): unknown[] {
        return name === "data"
          ? ["boot", 3, src, src, json, "{}", SYSTEM_INFO, MODULE_TABLE]
          : ["boot", 3, src, src, "{}", json, SYSTEM_INFO, MODULE_TABLE];
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
        connection(3, false);
        connection(3, true);
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
        connection(3, false);
        calls.length = 0;
        frameDataChanged(frame, name, '{"label":"while away"}');
        expect(calls).toEqual([]);
        connection(3, true);
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
