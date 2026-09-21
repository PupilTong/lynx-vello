// Behavior tests for `bobcat:future` over a recording host mock.
//
// These pin the semantics that live in future.ts: what a `wait` answers and
// memoizes, what a timeout leaves behind, the one-way conversion to a Promise
// and the refusal that follows it, and what `__BobcatSettleFuture` does with
// a delivery. The table underneath is the host's and is stood in for here —
// there, a wait parks the job it runs in and a settle is a task of the
// realm's owner, neither of which a Node test can have. The real boundary
// runs in the crate's own tests, over a live MTS page and a live worker.

import { beforeEach, describe, expect, it, rstest } from "@rstest/core";

rstest.mockRequire("bobcat-internal:host", () => ({
  waitFuture: (id: number, timeoutMs: number): boolean =>
    host.wait(id, timeoutMs),
  takeFuture: (id: number): unknown => host.take(id),
  settleFuture: (id: number): undefined => host.settle(id),
}));

import { Future, TimeoutError, __BobcatSettleFuture } from "../src/future.ts";

/** One registered operation, as the mock host holds it. */
interface Entry {
  /** What it settles to, or the reason it will not. */
  outcome: { value: unknown } | { reason: string };
  /** Whether a wait on it answers at once or runs out its deadline. */
  slow: boolean;
  settled: boolean;
}

const host = {
  entries: new Map<number, Entry>(),
  /** Every id, in the order each member was asked for it. */
  waits: [] as number[],
  settles: [] as number[],
  next: 1,

  /** Files one operation that settles to `value` the moment it is waited on. */
  resolves(value: unknown, slow = false): number {
    const id = this.next++;
    this.entries.set(id, { outcome: { value }, slow, settled: false });
    return id;
  },
  rejects(reason: string): number {
    const id = this.next++;
    this.entries.set(id, { outcome: { reason }, slow: false, settled: false });
    return id;
  },
  wait(id: number, timeoutMs: number): boolean {
    this.waits.push(id);
    const entry = this.entries.get(id);
    if (entry === undefined) throw new Error(`future ${id} is not pending`);
    if (entry.slow && Number.isFinite(timeoutMs)) {
      // The operation is untouched: it stays pending, exactly as the host
      // puts it back.
      return false;
    }
    entry.settled = true;
    return true;
  },
  take(id: number): unknown {
    const entry = this.entries.get(id);
    if (entry === undefined || !entry.settled) {
      throw new Error(`future ${id} has not settled`);
    }
    this.entries.delete(id);
    if ("reason" in entry.outcome) throw new Error(entry.outcome.reason);
    return entry.outcome.value;
  },
  settle(id: number): undefined {
    if (!this.entries.has(id)) throw new Error(`future ${id} is not pending`);
    this.settles.push(id);
    return undefined;
  },
  /** What the owner's epilogue does once a requested settle has settled. */
  deliver(id: number): undefined {
    const entry = this.entries.get(id);
    if (entry === undefined) throw new Error(`future ${id} is not pending`);
    this.entries.delete(id);
    return "reason" in entry.outcome
      ? __BobcatSettleFuture(id, true, entry.outcome.reason)
      : __BobcatSettleFuture(id, false, entry.outcome.value);
  },
  reset(): void {
    this.entries.clear();
    this.waits.length = 0;
    this.settles.length = 0;
    this.next = 1;
  },
};

describe("one host-backed operation, waited on or awaited", () => {
  beforeEach(() => {
    host.reset();
  });

  it("waits for the value the host settled to", () => {
    const future = new Future(host.resolves(42));

    expect(future.wait()).toBe(42);
    expect(host.waits).toEqual([1]);
    // Memoized: the host's entry is spent, and a second read answers the same
    // way rather than asking for a future that is no longer there.
    expect(future.wait()).toBe(42);
    expect(host.waits).toEqual([1]);
  });

  it("waits without a timeout for a Future given none", () => {
    // `slow`, so a finite deadline would have timed out: the wait answers,
    // which is what says no deadline was named.
    const future = new Future(host.resolves("late", true));

    expect(future.wait()).toBe("late");
  });

  it("throws a rejection as an Error carrying the host's reason", () => {
    const future = new Future(host.rejects("the fetcher returned a stylesheet"));

    expect(() => future.wait()).toThrow("the fetcher returned a stylesheet");
    expect(() => future.wait()).toThrow(Error);
    // Memoized as well, so the second read never reached the host.
    expect(host.waits).toEqual([1]);
  });

  it("times out without cancelling, and answers a later wait", () => {
    const future = new Future(host.resolves("eventually", true));

    expect(() => future.wait(50)).toThrow(TimeoutError);
    expect(() => future.wait(50)).toThrow(
      "the Future did not settle within 50 ms",
    );
    // Still pending on the host's side, so the one without a deadline lands.
    expect(future.wait()).toBe("eventually");
  });

  it("converts to a Promise after a timed-out wait", async () => {
    const future = new Future(host.resolves("eventually", true));

    expect(() => future.wait(1)).toThrow(TimeoutError);
    const answer = future.then(value => value);
    expect(host.settles).toEqual([1]);
    host.deliver(1);

    await expect(answer).resolves.toBe("eventually");
  });

  it("refuses a NaN timeout", () => {
    const future = new Future(host.resolves(1));

    expect(() => future.wait(Number.NaN)).toThrow(TypeError);
    expect(() => future.wait("soon" as unknown as number)).toThrow(
      "Future.wait expects a number of milliseconds",
    );
    // Nothing was asked of the host, so the operation is untouched.
    expect(host.waits).toEqual([]);
    expect(future.wait()).toBe(1);
  });

  it("refuses a Future id that is not a number", () => {
    expect(() => new Future("1" as unknown as number)).toThrow(TypeError);
  });

  it("asks for one settle however many times it is then'd", async () => {
    const future = new Future(host.resolves(7));

    const first = future.then(value => value);
    const second = future.then(value => (value as number) + 1);
    expect(host.settles).toEqual([1]);
    host.deliver(1);

    await expect(first).resolves.toBe(7);
    await expect(second).resolves.toBe(8);
  });

  it("rejects the Promise with an Error carrying the reason", async () => {
    const future = new Future(host.rejects("no such file: app:///a.cjs"));

    const caught = future.then(undefined, error => error);
    host.deliver(1);

    const error = await caught;
    expect(error).toBeInstanceOf(Error);
    expect((error as Error).message).toBe("no such file: app:///a.cjs");
  });

  it("refuses a wait once it is a Promise", async () => {
    const future = new Future(host.resolves("value"));
    const answer = future.then(value => value);

    expect(() => future.wait()).toThrow(TypeError);
    expect(() => future.wait()).toThrow("this Future is already a Promise");
    // Nothing was asked of the host but the settle it already requested.
    expect(host.waits).toEqual([]);
    host.deliver(1);

    await expect(answer).resolves.toBe("value");
  });

  it("answers a then after a completed wait, without a second settle", async () => {
    const future = new Future(host.resolves(21));

    expect(future.wait()).toBe(21);

    await expect(future.then(value => value)).resolves.toBe(21);
    expect(host.settles).toEqual([]);
  });

  it("rejects a then after a failed wait, without a second settle", async () => {
    const future = new Future(host.rejects("the realm ended"));

    expect(() => future.wait()).toThrow("the realm ended");

    await expect(
      future.then(undefined, error => (error as Error).message),
    ).resolves.toBe("the realm ended");
    expect(host.settles).toEqual([]);
  });

  it("drops a delivery nothing is waiting on", () => {
    expect(__BobcatSettleFuture(9, false, "nobody asked")).toBeUndefined();
  });
});
