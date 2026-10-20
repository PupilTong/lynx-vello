// `bobcat:frame-boot`: the half of a frame's card that runs in its
// `ShadowRealm` child, over stand-ins for every module it imports, so what
// is pinned here is its own order — when `dispose` stops the card's exposure,
// disposes of the card's BTS and hands the frame back to its parent. The card booting under a real nested page, with a real BTS,
// runs in crates/bobcat-core/src/main/runtime/frame_tests.rs.

import { beforeEach, describe, expect, it, rstest } from "@rstest/core";
import type * as frameBootModule from "../src/frame-boot.ts";

/** Everything the stand-ins were asked, in order. */
const calls: unknown[][] = [];

/** Settles the card's BTS disposal, once `__BobcatDispose` started it. */
let acknowledge: (() => void) | undefined;

/** What the stand-in `__BobcatSwitchExposure` answers for a stop. */
let queued = 0;

/** What the stand-in `__BobcatSwitchExposure` throws, if anything. */
let switchFails: string | undefined;

/** The callback `__BobcatAfterExposureRecords` was handed, if any. */
let afterRecords: (() => void) | undefined;

/** The callback `__BobcatCloseFrames` was handed, if any. */
let framesClosed: (() => void) | undefined;

/** Whether the stand-in `__BobcatCloseFrames` calls its callback at once. */
let framesCloseAtOnce = true;

rstest.mockRequire("bobcat:runtime", () => ({
  _ReportError(error: unknown) {
    calls.push(["reportError", error instanceof Error ? error.message : String(error)]);
  },
  __BobcatConnectBackground(_worker: unknown, _data: unknown, entry: string) {
    calls.push(["connectBackground", entry]);
  },
  __BobcatDispose(): Promise<void> {
    calls.push(["disposeBackground"]);
    return new Promise(resolve => {
      acknowledge = resolve;
    });
  },
  __BobcatInitEntry(url: string) {
    calls.push(["initEntry", url]);
  },
  __BobcatInitializeMTS() {
    calls.push(["initializeMTS"]);
  },
  __BobcatProcessInitData(data: unknown) {
    return data;
  },
  __BobcatRenderPage() {
    calls.push(["renderPage"]);
  },
  __BobcatUpdateData() {},
  __BobcatUpdateGlobalProps() {},
  lynx: { __initData: {} },
}));

rstest.mockRequire("bobcat:element", () => ({
  __BobcatAfterExposureRecords(callback: () => void) {
    calls.push(["afterExposureRecords"]);
    afterRecords = callback;
  },
  // The realm's own switch, over the elements it registered.
  __BobcatSwitchExposure(on: boolean, sendEvent: boolean): number {
    calls.push(["switchExposure", on, sendEvent]);
    if (switchFails !== undefined) throw new Error(switchFails);
    return on ? 0 : queued;
  },
  __BobcatUsePageRoot(page: number) {
    calls.push(["usePageRoot", page]);
  },
  __FlushElementTree() {},
}));

rstest.mockRequire("bobcat:frame", () => ({
  __BobcatCloseFrames(onClosed: () => void) {
    calls.push(["closeFrames"]);
    if (framesCloseAtOnce) onClosed();
    else framesClosed = onClosed;
  },
}));

rstest.mockRequire("bobcat-internal", () => ({
  Worker: class {
    constructor(url: string) {
      calls.push(["worker", url]);
    }
  },
}));

rstest.mockRequire("bobcat-internal:host", () => ({
  adoptStyleSheet() {},
  closeRealm() {
    calls.push(["closeRealm"]);
  },
  // The frame's nested page: one past the frame's id.
  framePage(frame: number): number {
    return frame + 1;
  },
  hasStyleSheet: () => false,
  loadModuleSync() {},
  parentNode: (node: number) => node - 1,
}));

rstest.mockRequire("bobcat:section-url", () => ({
  backgroundThreadURL: (url: string) => `${url}#background-thread`,
  bundleStyleSheetURL: (url: string) => `${url}/style-info.css`,
  sectionURL: (name: string, url: string) => `${url}/${name}.js`,
}));

const CARD = "app:///child.web.bundle";

let frameBoot: typeof frameBootModule;

beforeEach(async () => {
  calls.length = 0;
  acknowledge = undefined;
  queued = 0;
  switchFails = undefined;
  afterRecords = undefined;
  framesClosed = undefined;
  framesCloseAtOnce = true;
  // Each test gets a fresh module.
  rstest.resetModules();
  frameBoot = await import("../src/frame-boot.ts");
});

/** Boots the card in frame 7, as `bobcat:frame` would. */
function boot(): void {
  expect(frameBoot.boot(7, CARD, "{}", "{}", "{}", "")).toBe(true);
  calls.length = 0;
}

/** The parent's `done`, recording its call. */
function done(): void {
  calls.push(["done"]);
}

describe("dispose", () => {
  it("does everything at once when the stop queued nothing", async () => {
    boot();
    frameBoot.dispose(done);
    // In this call: the card's exposure stopped, its own frames closed, its
    // BTS disposed of, and the frame handed back — nothing awaited.
    expect(calls).toEqual([
      ["switchExposure", false, true],
      ["closeFrames"],
      ["disposeBackground"],
      ["done"],
    ]);
    // The realm closes only once the BTS acknowledged.
    await Promise.resolve();
    expect(calls.at(-1)).toEqual(["done"]);
    acknowledge!();
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(calls.at(-1)).toEqual(["closeRealm"]);
  });

  it("waits for the flush of its last records before disposing of the BTS and handing the frame back", () => {
    boot();
    queued = 2;
    frameBoot.dispose(done);
    expect(calls).toEqual([
      ["switchExposure", false, true],
      ["closeFrames"],
      ["afterExposureRecords"],
    ]);

    // The host's next delivery dispatched the records, and the flush posted
    // the last list to the BTS.
    afterRecords!();
    expect(calls.slice(3)).toEqual([["disposeBackground"], ["done"]]);
  });

  it("waits for its own frames' cards to post their last lists too", () => {
    boot();
    queued = 1;
    framesCloseAtOnce = false;
    frameBoot.dispose(done);
    afterRecords!();
    expect(calls.filter(call => call[0] === "disposeBackground")).toEqual([]);
    framesClosed!();
    expect(calls.slice(-2)).toEqual([["disposeBackground"], ["done"]]);
  });

  it("lets go at once of a card that never booted, whose switch lists nothing", () => {
    frameBoot.dispose(done);
    expect(calls).toEqual([
      ["switchExposure", false, true],
      ["closeFrames"],
      ["disposeBackground"],
      ["done"],
    ]);
  });

  it("reports a switch that throws and goes on as if it queued nothing", () => {
    boot();
    queued = 2;
    switchFails = "the document is gone";
    frameBoot.dispose(done);
    expect(calls).toEqual([
      ["switchExposure", false, true],
      ["reportError", "the document is gone"],
      ["closeFrames"],
      ["disposeBackground"],
      ["done"],
    ]);
  });

  it("reports a done that throws across the boundary", () => {
    boot();
    frameBoot.dispose(() => {
      throw new TypeError("the parent is gone");
    });
    expect(calls.slice(-2)).toEqual([
      ["disposeBackground"],
      ["reportError", "the parent is gone"],
    ]);
  });
});
