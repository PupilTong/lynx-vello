// `bobcat:animation-frame` over a recording stand-in for the one host member
// it is written over, `requestScriptFrame`.
//
// What is pinned here is what the module decides: which callbacks one frame
// runs, which wait for the next, when the host is told a frame is wanted, and
// where a callback's exception goes. Which realm the host delivers the vsync
// to is the host's; the crate's own tests run that over live realms.

import { afterEach, describe, expect, it, rstest } from "@rstest/core";
import * as eventTarget from "../src/event-target.ts";

rstest.mockRequire("bobcat:event-target", () => eventTarget);
rstest.mockRequire("bobcat-internal:host", () => ({
  requestScriptFrame: (pending: boolean): undefined => {
    host.demands.push(pending);
    return undefined;
  },
}));

import {
  __BobcatBeginFrame,
  cancelAnimationFrame,
  clearAnimationFrames,
  requestAnimationFrame,
} from "../src/animation-frame.ts";

/** Every demand the module handed the host, in order. */
const host = { demands: [] as boolean[] };

/** What the realm's installed reporter was handed, in order. */
const reports: unknown[] = [];
eventTarget.installExceptionReporter(error => { reports.push(error); });

afterEach(() => {
  // One realm's table for the whole file: each test leaves it empty.
  clearAnimationFrames();
  host.demands.length = 0;
  reports.length = 0;
});

describe("animation frames", () => {
  it("runs one frame's callbacks with cancellation, nested requests and errors kept on their own frame", () => {
    const calls: [string, number][] = [];
    let cancelled = 0;
    requestAnimationFrame(time => {
      calls.push(["first", time]);
      cancelAnimationFrame(cancelled);
      requestAnimationFrame(time => calls.push(["nested", time]));
      throw undefined;
    });
    cancelled = requestAnimationFrame(() => { throw Error("cancelled callback ran"); });
    requestAnimationFrame(time => calls.push(["third", time]));
    expect(host.demands).toEqual([true]);

    __BobcatBeginFrame(1250);
    // The nested request waits for the next frame, and the throw goes to the
    // installed reporter without stopping the callback behind it.
    expect(calls).toEqual([["first", 1250], ["third", 1250]]);
    expect(reports).toEqual([undefined]);
    // The vsync consumed the demand, so the nested request raised it again.
    expect(host.demands).toEqual([true, true]);

    __BobcatBeginFrame(1500);
    expect(calls).toEqual([["first", 1250], ["third", 1250], ["nested", 1500]]);
    expect(reports).toEqual([undefined]);
    expect(host.demands).toEqual([true, true]);
  });

  it("coalesces the demand and withdraws it when the last callback is cancelled", () => {
    const callback = rstest.fn();
    const first = requestAnimationFrame(callback);
    const last = requestAnimationFrame(callback);
    expect(host.demands).toEqual([true]);
    cancelAnimationFrame(first);
    expect(host.demands).toEqual([true]);
    cancelAnimationFrame(last);
    expect(host.demands).toEqual([true, false]);
    __BobcatBeginFrame(1750);
    expect(callback).not.toHaveBeenCalled();
    expect(host.demands).toEqual([true, false]);
  });

  it("drops every waiting callback and withdraws the demand when cleared", () => {
    const callback = rstest.fn();
    requestAnimationFrame(callback);
    requestAnimationFrame(callback);
    clearAnimationFrames();
    expect(host.demands).toEqual([true, false]);
    __BobcatBeginFrame(2000);
    expect(callback).not.toHaveBeenCalled();
  });

  it("refuses a callback that is not a function and files nothing", () => {
    expect(() => Reflect.apply(requestAnimationFrame, undefined, [null])).toThrow(TypeError);
    expect(host.demands).toEqual([]);
  });
});
