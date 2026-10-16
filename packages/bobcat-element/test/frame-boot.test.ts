// `bobcat:frame-boot`: the half of a frame's card that runs in its
// `ShadowRealm` child, over stand-ins for every module it imports, so what
// is pinned here is its own order — what `dispose` sends the card's BTS
// before letting go of it, and what `sendExposure` and `frameConnection`
// hand to the card's own runtime and frames. The card booting under a real
// nested page, with a real BTS, runs in
// crates/bobcat-core/src/main/runtime/frame_tests.rs.

import { beforeEach, describe, expect, it, rstest } from "@rstest/core";
import type * as frameBootModule from "../src/frame-boot.ts";

/** Everything the stand-ins were asked, in order. */
const calls: unknown[][] = [];

/** Settles the card's BTS disposal, once `__BobcatDispose` started it. */
let acknowledge: (() => void) | undefined;

/** What the stand-in `bobcat:frame` answers for a frame, by id. */
const holding: Set<number> = new Set();

/** Set to make the stand-in runtime's exposure delivery throw. */
let exposureFails: string | undefined;

rstest.mockRequire("bobcat:runtime", () => ({
  _ReportError(error: unknown) {
    calls.push(["reportError", error instanceof Error ? error.message : String(error)]);
  },
  __BobcatConnectBackground(_worker: unknown, _data: unknown, entry: string) {
    calls.push(["connectBackground", entry]);
  },
  __BobcatDispose(): Promise<void> {
    calls.push(["dispose"]);
    return new Promise(resolve => {
      acknowledge = resolve;
    });
  },
  __BobcatInitEntry(url: string) {
    calls.push(["initEntry", url]);
  },
  __BobcatInitializeMTS(options: Record<string, unknown>) {
    calls.push(["initializeMTS", options["frame"]]);
  },
  __BobcatProcessInitData(data: unknown) {
    return data;
  },
  __BobcatRenderPage() {
    calls.push(["renderPage"]);
  },
  __BobcatSendExposure(kind: string, record: string) {
    calls.push(["sendExposure", kind, record]);
    if (exposureFails !== undefined) throw new TypeError(exposureFails);
  },
  __BobcatSendLastDisexposure() {
    calls.push(["sendLastDisexposure"]);
  },
  __BobcatUpdateData() {},
  __BobcatUpdateGlobalProps() {},
  lynx: { __initData: {} },
}));

rstest.mockRequire("bobcat:element", () => ({
  __BobcatDispatchCardEvent() {},
  __BobcatUsePageRoot(page: number) {
    calls.push(["usePageRoot", page]);
  },
  __FlushElementTree() {},
}));

rstest.mockRequire("bobcat:frame", () => ({
  __BobcatCloseFrames() {
    calls.push(["closeFrames"]);
  },
  __BobcatFrameConnected(frameId: number): boolean {
    calls.push(["frameConnected", frameId]);
    if (frameId < 0) throw new TypeError("no such frame");
    return holding.has(frameId);
  },
  __BobcatFrameDisconnected(frameId: number): boolean {
    calls.push(["frameDisconnected", frameId]);
    return holding.has(frameId);
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
  switchExposure(on: 0 | 1, sendEvent: 0 | 1, scope: number) {
    calls.push(["switchExposure", on, sendEvent, scope]);
  },
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
  holding.clear();
  acknowledge = undefined;
  exposureFails = undefined;
  // `boot` keeps the frame it rendered in: each test gets a fresh module.
  rstest.resetModules();
  frameBoot = await import("../src/frame-boot.ts");
});

/** Boots the card in frame 7, as `bobcat:frame` would. */
function boot(): void {
  expect(frameBoot.boot(7, CARD, "{}", "{}", "{}", "")).toBe(true);
  calls.length = 0;
}

describe("boot", () => {
  it("scopes the card's exposure switch to the frame it renders in", () => {
    frameBoot.boot(7, CARD, "{}", "{}", "{}", "");
    expect(calls.filter(call => call[0] === "initializeMTS")).toEqual([
      ["initializeMTS", 7],
    ]);
  });
});

describe("dispose", () => {
  it("sends the card's BTS its last disexposure list before disposing of it, all at once", async () => {
    boot();
    frameBoot.dispose();
    // In this call: the card's own frames first, then the last list, the
    // card's exposure switch let go of, and the BTS's `dispose` — nothing
    // awaited between them.
    expect(calls).toEqual([
      ["closeFrames"],
      ["sendLastDisexposure"],
      ["switchExposure", 1, 0, 7],
      ["dispose"],
    ]);
    // The realm closes only once the BTS acknowledged.
    await Promise.resolve();
    expect(calls.at(-1)).toEqual(["dispose"]);
    acknowledge!();
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(calls.at(-1)).toEqual(["closeRealm"]);
  });

  it("lets go of no exposure switch before a page was chosen", () => {
    frameBoot.dispose();
    expect(calls).toEqual([["closeFrames"], ["sendLastDisexposure"], ["dispose"]]);
  });
});

describe("sendExposure", () => {
  it("delivers the card's records as the page's own delivery does, and reports what throws", () => {
    boot();
    frameBoot.sendExposure("exposure", "1:91:a0:0:");
    exposureFails = "a read failed";
    frameBoot.sendExposure("disexposure", "1:91:a0:0:");
    expect(calls).toEqual([
      ["sendExposure", "exposure", "1:91:a0:0:"],
      ["sendExposure", "disexposure", "1:91:a0:0:"],
      ["reportError", "a read failed"],
    ]);
  });
});

describe("frameConnection", () => {
  it("runs the card's own bobcat:frame and answers whether it, or a card of its own, held the frame", () => {
    boot();
    holding.add(9);
    expect(frameBoot.frameConnection(9, false)).toBe(true);
    expect(frameBoot.frameConnection(9, true)).toBe(true);
    expect(frameBoot.frameConnection(10, true)).toBe(false);
    // What throws is the card's: reported here, and no answer.
    expect(frameBoot.frameConnection(-1, true)).toBe(false);
    expect(calls).toEqual([
      ["frameDisconnected", 9],
      ["frameConnected", 9],
      ["frameConnected", 10],
      ["frameConnected", -1],
      ["reportError", "no such frame"],
    ]);
  });
});
