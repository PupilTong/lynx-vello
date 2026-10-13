// `bobcat:frame`: what a frame's `src` does in the realm that wrote it, over
// stand-ins for the host members it reaches (`fetchResource`, `releaseFrame`
// and the `Future` trio), for the diagnostics pair, and for `ShadowRealm`.
//
// The `Future` class is the real one; its settle is driven through its own
// `__BobcatSettleFuture`, as the host's epilogue drives it. The child half —
// `bobcat:frame-boot` binding the frame and rendering the card — runs
// against the real realm in crates/bobcat-core/src/main/runtime/tests.rs.

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
  releaseFrame(frame: number): undefined {
    calls.push(["releaseFrame", frame]);
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

rstest.mockRequire("bobcat:runtime", () => ({
  SystemInfo: { platform: "headless", pixelRatio: 2 },
}));

/** A stand-in for the host's `ShadowRealm`: its child's `boot` records its call. */
class FakeShadowRealm {
  static created = 0;
  /** What the next `importValue` settles with, if not a recording `boot`. */
  static importFails: string | undefined;
  /** Set to hold the next `importValue` until the test calls what it leaves here. */
  static hold = false;
  static release: (() => void) | undefined;
  constructor() {
    FakeShadowRealm.created += 1;
  }
  evaluate(): unknown {
    throw new Error("bobcat:frame evaluates nothing");
  }
  importValue(specifier: string, name: string): Promise<unknown> {
    calls.push(["importValue", specifier, name]);
    const failure = FakeShadowRealm.importFails;
    if (failure !== undefined) return Promise.reject(new TypeError(failure));
    const boot = (...args: unknown[]) => {
      calls.push(["boot", ...args]);
      return true;
    };
    if (!FakeShadowRealm.hold) return Promise.resolve(boot);
    return new Promise(resolve => {
      FakeShadowRealm.release = () => resolve(boot);
    });
  }
}

import { frameSourceChanged } from "../src/frame.ts";

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
  calls.length = 0;
  already.clear();
  FakeShadowRealm.created = 0;
  FakeShadowRealm.importFails = undefined;
  FakeShadowRealm.hold = false;
  FakeShadowRealm.release = undefined;
  (globalThis as Record<string, unknown>)["ShadowRealm"] = FakeShadowRealm;
});

describe("frameSourceChanged", () => {
  it("releases, fetches src as written, and boots it in a fresh realm once the fetch settles", async () => {
    const frame = {};
    frameSourceChanged(frame, 3, "child.web.bundle");
    const [id] = futures();
    expect(calls).toEqual([
      ["releaseFrame", 3],
      ["fetchResource", "child.web.bundle"],
      ["settleFuture", id],
    ]);
    await drain();
    expect(FakeShadowRealm.created).toBe(0);

    future.__BobcatSettleFuture(id!, false, undefined);
    await drain();
    expect(FakeShadowRealm.created).toBe(1);
    expect(calls.slice(3)).toEqual([
      ["importValue", "bobcat:frame-boot", "boot"],
      ["boot", 3, "child.web.bundle", "{}", "{}", SYSTEM_INFO],
    ]);
  });

  it("defers a probe hit to a microtask", async () => {
    already.add("app:///cached.web.bundle");
    frameSourceChanged({}, 3, "app:///cached.web.bundle");
    expect(calls).toEqual([
      ["releaseFrame", 3],
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
    frameSourceChanged(frame, 3, null);
    FakeShadowRealm.release!();
    await drain();
    expect(calls.filter(call => call[0] === "boot")).toEqual([]);
  });

  it("releases and starts nothing for a null or empty src", async () => {
    const frame = {};
    frameSourceChanged(frame, 3, "app:///child.web.bundle");
    const [first] = futures();
    calls.length = 0;
    frameSourceChanged(frame, 3, null);
    expect(calls).toEqual([["releaseFrame", 3]]);

    const other = {};
    frameSourceChanged(other, 4, "app:///child.web.bundle");
    const [second] = futures();
    calls.length = 0;
    frameSourceChanged(other, 4, "");
    expect(calls).toEqual([["releaseFrame", 4]]);

    future.__BobcatSettleFuture(first!, false, undefined);
    future.__BobcatSettleFuture(second!, false, undefined);
    await drain();
    expect(FakeShadowRealm.created).toBe(0);
  });

  it("does nothing for the src the frame already has, as native and web-core skip it", () => {
    const frame = {};
    frameSourceChanged(frame, 3, "app:///child.web.bundle");
    calls.length = 0;
    frameSourceChanged(frame, 3, "app:///child.web.bundle");
    expect(calls).toEqual([]);
    // Nor for a removal of a `src` the frame never had.
    frameSourceChanged({}, 4, null);
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
    frameSourceChanged(frame, 3, null);
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
