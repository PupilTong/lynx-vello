// `lynx.fetchBundle`'s handle, over a stand-in for the host members
// `crates/bobcat-core/src/fetch.rs` and `crates/bobcat-core/src/future.rs`
// install.
//
// What is stood in for is exactly those: `fetchResource` starts one plain
// fetch and answers a future id, and the three `Future` members park, take
// and settle that future. The `Future` class itself is the real one — the
// handle's `wait` and `then` are its, and the refusal a `wait` after a `then`
// gets comes from there. What core does with the bytes is nothing at all;
// whether a fetched container's sections became loadable is the fetcher's,
// and the real boundary runs in crates/bobcat-core/tests/fetch_bundle.rs.

import { beforeEach, describe, expect, it, rstest } from "@rstest/core";

rstest.mockRequire("bobcat-internal:host", () => ({
  fetchResource: (url: string): number | true => host.fetch(url),
  waitFuture: (id: number, timeoutMs: number): boolean => host.wait(id, timeoutMs),
  takeFuture: (id: number): unknown => host.take(id),
  settleFuture: (id: number): undefined => host.settle(id),
}));

import * as future from "../src/future.ts";
rstest.mockRequire("bobcat:future", () => future);

import { type BundleInfo, createBundleFetches } from "../src/bundle-fetch.ts";

/** One fetch the host is carrying, by the future id it settles through. */
interface Load {
  url: string;
  /** Set once the test has finished the fetch: `undefined` is a success. */
  outcome?: { reason: string } | { done: true };
  /** Whether a wait has parked the outcome for the realm to take. */
  parked: boolean;
}

const host = {
  /** URLs this "view"'s fetcher already holds, which answer `true` at once. */
  already: new Set<string>(),
  /** Outstanding fetches, by the id the future table handed out. */
  loads: new Map<number, Load>(),
  /** Every URL `fetchResource` was asked for, in order. */
  fetches: [] as string[],
  /** Every `(id, timeoutMs)` a `wait` asked for, in order. */
  waits: [] as [number, number][],
  /** Every id `.then` asked to settle asynchronously, in order. */
  settles: [] as number[],
  next: 1,

  fetch(url: string): number | true {
    this.fetches.push(url);
    // The fetcher's probe: already fetched, so nothing is requested.
    if (this.already.has(url)) return true;
    const id = this.next++;
    this.loads.set(id, {url, parked: false});
    return id;
  },
  wait(id: number, timeoutMs: number): boolean {
    this.waits.push([id, timeoutMs]);
    const load = this.loads.get(id);
    if (load === undefined) throw new Error(`future ${id} is not pending`);
    // The stand-in for the park: a fetch the test has already finished
    // answers, and one it has not runs out its deadline. Nothing is
    // cancelled either way.
    if (load.outcome === undefined) return false;
    load.parked = true;
    return true;
  },
  take(id: number): unknown {
    const load = this.loads.get(id);
    if (load === undefined || !load.parked) {
      throw new Error(`future ${id} has not settled`);
    }
    this.loads.delete(id);
    if (load.outcome !== undefined && "reason" in load.outcome) {
      throw new Error(load.outcome.reason);
    }
    return undefined;
  },
  settle(id: number): undefined {
    if (!this.loads.has(id)) throw new Error(`future ${id} is not pending`);
    this.settles.push(id);
    return undefined;
  },

  /** The host's fetch finishing, which is what a later `wait` reads. */
  finish(id: number, reason?: string): void {
    const load = this.loads.get(id);
    if (load === undefined) throw new Error(`nothing is outstanding under ${id}`);
    load.outcome = reason === undefined ? {done: true} : {reason};
  },
  /** What the owner's epilogue does once a requested settle has settled. */
  deliver(id: number, reason?: string): void {
    this.finish(id, reason);
    this.loads.delete(id);
    future.__BobcatSettleFuture(id, reason !== undefined, reason);
  },
  reset(): void {
    this.already.clear();
    this.loads.clear();
    this.fetches.length = 0;
    this.waits.length = 0;
    this.settles.length = 0;
    this.next = 1;
  },
};

/** Every microtask the delivery queued, run out. */
function flush(): Promise<void> {
  return new Promise<void>(resolve => { setTimeout(resolve, 0); });
}

const reported: unknown[] = [];
/** What `later` was handed, for the tests that assert a callback is posted. */
const posted: (() => void)[] = [];

/** MTS's realm: a callback on a settled handle runs inline. */
function inlineRealm() {
  return createBundleFetches({
    report: error => { reported.push(error); },
    later: run => { run(); },
  });
}

/** BTS's realm: the same callback is a posted task. */
function postingRealm() {
  return createBundleFetches({
    report: error => { reported.push(error); },
    later: run => { posted.push(run); },
  });
}

const LAZY = "https://cdn.test/lazy.bundle";

describe("the handle lynx.fetchBundle answers with", () => {
  beforeEach(() => {
    host.reset();
    reported.length = 0;
    posted.length = 0;
  });

  it("asks the host every time, remembering no URL of its own", () => {
    const fetches = inlineRealm();

    fetches.fetchBundle(LAZY);
    fetches.fetchBundle(LAZY);

    // Nothing here memoizes: both calls reach the host, which is what lets
    // the *fetcher* be the one thing that knows what it has.
    expect(host.fetches).toEqual([LAZY, LAZY]);
  });

  it("settles at once for a URL the fetcher already holds, and runs .then through later", () => {
    host.already.add(LAZY);
    const fetches = inlineRealm();
    const order: string[] = [];

    const handle = fetches.fetchBundle(LAZY);
    handle.then(info => order.push(`then ${info.code}`));
    order.push("after then");

    // Inline, which is what `rLynxPrepareLazyBundleMTS` depends on: its
    // `loadScript` and `__LoadStyleSheet` run before the call that registered
    // the callback returns.
    expect(order).toEqual(["then 0", "after then"]);
    // No future was registered, so none was waited on or settled.
    expect(host.loads.size).toBe(0);
    expect(host.waits).toEqual([]);
    expect(host.settles).toEqual([]);
    // `wait` answers the same record, and still asks the host nothing.
    expect(handle.wait(5)).toEqual({url: LAZY, code: 0, error_msg: ""});
    expect(host.waits).toEqual([]);
  });

  it("posts an already-held URL's callback on the background thread", () => {
    host.already.add(LAZY);
    const order: string[] = [];

    postingRealm().fetchBundle(LAZY)
      .then(info => order.push(`then ${info.code}`));
    order.push("after then");

    expect(order).toEqual(["after then"]);
    expect(posted).toHaveLength(1);
    posted[0]!();
    expect(order).toEqual(["after then", "then 0"]);
  });

  it("waits out a fetch in seconds and answers native's installed record", () => {
    const fetches = inlineRealm();
    const handle = fetches.fetchBundle(LAZY);

    host.finish(1);
    expect(handle.wait(5)).toEqual({url: LAZY, code: 0, error_msg: ""});
    // Seconds here, milliseconds at the boundary.
    expect(host.waits).toEqual([[1, 5000]]);
    // The outcome is this realm's now: a second `wait` asks the host nothing.
    expect(handle.wait(5).code).toBe(0);
    expect(host.waits).toEqual([[1, 5000]]);
  });

  it("answers a timeout with native's record and leaves the fetch running", () => {
    const fetches = inlineRealm();
    const handle = fetches.fetchBundle("https://cdn.test/slow.bundle");

    // Nothing has settled: the future runs out its deadline, and the record
    // says so in native's own words. The fetch is not cancelled by it.
    expect(handle.wait(0.05)).toEqual({
      url: "https://cdn.test/slow.bundle",
      code: -2,
      error_msg:
        "ResponsePromise wait timeout after 0.05 seconds for url: https://cdn.test/slow.bundle",
    });
    expect(host.waits).toEqual([[1, 50]]);

    // Still pending on the host's side, so the next wait lands.
    host.finish(1);
    expect(handle.wait(5).code).toBe(0);
  });

  it("answers a rejected fetch as a -1 record carrying the host's reason", () => {
    const fetches = inlineRealm();
    const handle = fetches.fetchBundle("https://cdn.test/gone.bundle");

    host.finish(1, "this page has no such file");
    expect(handle.wait(5)).toEqual({
      url: "https://cdn.test/gone.bundle",
      code: -1,
      error_msg: "this page has no such file",
    });
    // Settled, so a `then` on it runs through `later` rather than converting.
    const seen: number[] = [];
    handle.then(info => seen.push(info.code));
    expect(seen).toEqual([-1]);
    expect(host.settles).toEqual([]);
  });

  it("runs a then after a successful wait through later, and never converts the future", () => {
    const fetches = inlineRealm();
    const handle = fetches.fetchBundle(LAZY);
    const order: string[] = [];

    host.finish(1);
    expect(handle.wait(5).code).toBe(0);

    // The `wait` is what brought the outcome in, so this handle is settled
    // and the callback goes through `later` — inline here, as on MTS.
    // Native's `LynxActor::Act` acts on the value being there.
    handle.then(info => order.push(`then ${info.code}`));
    expect(order).toEqual(["then 0"]);
    expect(host.settles).toEqual([]);
  });

  it("runs the callbacks registered before the settle once, as the Promise resolves", async () => {
    const fetches = inlineRealm();
    const handle = fetches.fetchBundle(LAZY);
    const seen: BundleInfo[] = [];

    // Pushed, not run: nothing has the outcome yet, so the future's Promise
    // is what runs these. Only the first callback converts it.
    handle.then(info => seen.push(info));
    handle.then(info => seen.push(info));
    expect(seen).toEqual([]);
    expect(host.settles).toEqual([1]);

    host.deliver(1);
    await flush();
    expect(seen).toHaveLength(2);
    expect(seen[0]).toEqual({url: LAZY, code: 0, error_msg: ""});

    // Registered afterwards, the callback runs through `later` instead.
    handle.then(info => seen.push(info));
    expect(seen).toHaveLength(3);
    expect(host.settles).toEqual([1]);
  });

  it("delivers a rejected fetch to a then as a -1 record", async () => {
    const fetches = inlineRealm();
    const handle = fetches.fetchBundle("https://cdn.test/gone.bundle");
    let settled: BundleInfo | undefined;
    handle.then(info => { settled = info; });

    host.deliver(1, "this page has no such file");
    await flush();

    expect(settled).toEqual({
      url: "https://cdn.test/gone.bundle",
      code: -1,
      error_msg: "this page has no such file",
    });
    // Nothing was reported: a failed fetch is a record, not a thrown error.
    expect(reported).toEqual([]);
  });

  it("posts a settled handle's callback on the background thread", async () => {
    const fetches = postingRealm();
    const handle = fetches.fetchBundle(LAZY);
    const order: string[] = [];

    host.finish(1);
    handle.wait(5);
    handle.then(info => order.push(`then ${info.code}`));
    order.push("after then");

    expect(order).toEqual(["after then"]);
    expect(posted).toHaveLength(1);
    posted[0]!();
    expect(order).toEqual(["after then", "then 0"]);
  });

  it("reports a callback that throws and runs the ones behind it", async () => {
    const fetches = inlineRealm();
    const handle = fetches.fetchBundle(LAZY);
    const seen: number[] = [];
    const failure = new Error("the callback threw");

    handle.then(() => { throw failure; });
    handle.then(info => seen.push(info.code));
    host.deliver(1);
    await flush();

    expect(reported).toEqual([failure]);
    expect(seen).toEqual([0]);
  });

  it("refuses a wait once a then has converted the future", () => {
    const fetches = inlineRealm();
    const handle = fetches.fetchBundle(LAZY);

    handle.then(() => undefined);
    // `bobcat:future`'s structural refusal, propagated: the delivery is a
    // job, and a job cannot run inside another job's wait. Native's
    // `shared_future` allows both on one handle.
    expect(() => handle.wait(5)).toThrow(TypeError);
    expect(() => handle.wait(5)).toThrow("this Future is already a Promise");
    expect(host.waits).toEqual([]);
  });

  it("hands out copies, so a caller cannot reach the record", () => {
    const fetches = inlineRealm();
    const handle = fetches.fetchBundle(LAZY);

    host.finish(1);
    const first = handle.wait(0);
    first.url = "rewritten";
    expect(handle.wait(0).url).toBe(LAZY);
    handle.then(info => { expect(info.url).toBe(LAZY); });
  });

  it("is native's two members and nothing else", () => {
    const handle = inlineRealm().fetchBundle(LAZY);

    expect(Object.keys(handle).sort()).toEqual(["then", "wait"]);
    // Not a Promise: `.then` answers nothing, so there is nothing to chain.
    expect(handle.then(() => undefined)).toBeUndefined();
  });

  it("refuses arguments native's own checks refuse", () => {
    const fetches = inlineRealm();

    expect(() => fetches.fetchBundle(7 as unknown as string)).toThrow(TypeError);
    const handle = fetches.fetchBundle(LAZY);
    expect(() => handle.wait("5" as unknown as number)).toThrow(TypeError);
    expect(() => handle.then(undefined as unknown as () => void)).toThrow(TypeError);
  });
});
