// `bobcat:frame-boot`: the half of a frame's card that runs in its
// `ShadowRealm` child, over stand-ins for every module it imports, so what
// is pinned here is its own order — where `boot` dispatches the frame's
// `load`, and when `dispose` stops the card's exposure, disposes of the
// card's BTS and hands the frame back to its parent. The `load` is the real
// `bobcat:element`'s `__CreateEvent`, `__DispatchEvent` and
// `__BobcatDispatchEvent`, over a recording host `dispatchEvent` that calls
// this realm back as the host does, so the event's facts and a listener of
// the card's own are the runtime's. The card booting under a real nested page, with a real
// BTS, and the page realm hearing its `load`, run in
// crates/bobcat-core/src/main/runtime/frame_tests.rs.

import { beforeEach, describe, expect, it, rstest } from "@rstest/core";
import type * as elementPapiModule from "../src/element-papi.ts";
import type * as frameBootModule from "../src/frame-boot.ts";
import * as record from "../src/record.ts";

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

/** What the stand-in `framePage` throws, if anything. */
let framePageFails: string | undefined;

/**
 * What the stand-in host `dispatchEvent` throws, if anything: the bridge's
 * clone refusing the `detail`, before any realm is called.
 */
let dispatchFails: string | undefined;

/**
 * The listeners of the other realms the stand-in host `dispatchEvent` calls
 * before this one: the page realm's, which the real host calls first.
 */
let otherRealms: () => void = () => undefined;

/**
 * Whether the stand-in host `dispatchEvent` calls this realm back once the
 * other realms ran: a card whose page the parent freed meanwhile has no
 * registration left on the path and is not invited.
 */
let invited = true;

/** The card's `root`, which the stand-in `loadModuleSync` runs. */
let root: () => void = () => undefined;

/** `bobcat:element`'s detail kind for a synthetic event this realm dispatched. */
const DETAIL_DISPATCHED = 9;

/** What the stand-in `timelineMilliseconds` answers. */
const TIMELINE = 1234.5;

/**
 * The real `bobcat:element`, fresh for each test, to which the stand-in
 * delegates the members `boot`'s `load` and a card's listener use.
 */
let papi: typeof elementPapiModule;

// The reader of every record the host answers with, as the realm has it.
rstest.mockRequire("bobcat:record", () => record);

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
    return papi.__BobcatUsePageRoot(page);
  },
  __CreateEvent: (...args: Parameters<typeof papi.__CreateEvent>) => papi.__CreateEvent(...args),
  __DispatchEvent: (...args: Parameters<typeof papi.__DispatchEvent>) => papi.__DispatchEvent(...args),
  __FlushElementTree() {},
  __GetPageElement: () => papi.__GetPageElement(),
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
  __BobcatWorkerKey: () => 1,
}));

rstest.mockRequire("bobcat-internal:host", () => ({
  adoptStyleSheet() {},
  closeRealm() {
    calls.push(["closeRealm"]);
  },
  // The frame's nested page: one past the frame's id.
  framePage(frame: number): number {
    if (framePageFails !== undefined) throw new Error(framePageFails);
    return frame + 1;
  },
  hasStyleSheet: () => false,
  loadModuleSync(url: string) {
    calls.push(["loadModuleSync", url]);
    root();
  },
  markLoadTiming() {},
  parentNode: (node: number) => node - 1,
  // What the real `bobcat:element` asks for the `load` and a listener.
  // The host computes the path and calls every realm the listener table
  // invites, the page realm first, which frame_tests.rs runs; this runs the
  // other realms' stand-in and then calls this realm back over the nested
  // page alone, the one step of the path it holds, with `DETAIL_DISPATCHED`,
  // as `element-papi.test.ts`'s `callsBack` does. What this realm's walk
  // throws is the host's to report, as a listener failure.
  dispatchEvent(
    node: number,
    name: string,
    bubbles: boolean,
    composed: boolean,
    timestamp: number,
    detail: unknown,
  ): undefined {
    // `structuredClone` stands in for the bridge's clone of `detail`.
    calls.push(["dispatchEvent", node, name, bubbles, composed, timestamp, structuredClone(detail)]);
    if (dispatchFails !== undefined) throw new TypeError(dispatchFails);
    otherRealms();
    if (!invited) return undefined;
    try {
      papi.__BobcatDispatchEvent(name, bubbles, timestamp, 1, node, node, 0, DETAIL_DISPATCHED);
    } catch (error) {
      calls.push(["listenerFailed", error instanceof Error ? error.message : String(error)]);
    }
    return undefined;
  },
  getAttribute: () => null,
  listenerClosed() {},
  listenerOpened() {},
  timelineMilliseconds: () => TIMELINE,
}));

rstest.mockRequire("bobcat:section-url", () => ({
  backgroundThreadURL: (url: string) => `${url}#background-thread`,
  bundleStyleSheetURL: (url: string) => `${url}/style-info.css`,
  sectionURL: (name: string, url: string) => `${url}/${name}.js`,
}));

const CARD = "app:///child.web.bundle";

/** The frame's `src` as written, which resolved to [`CARD`]. */
const SRC = "child.web.bundle";

/** The nested page of frame 7, as the stand-in `framePage` answers it. */
const PAGE = 8;

/** The `detail` of the frame's `load`. */
const LOADED = { statusCode: 0, statusMessage: "success", url: SRC };

let frameBoot: typeof frameBootModule;

beforeEach(async () => {
  calls.length = 0;
  acknowledge = undefined;
  queued = 0;
  switchFails = undefined;
  afterRecords = undefined;
  framesClosed = undefined;
  framesCloseAtOnce = true;
  framePageFails = undefined;
  dispatchFails = undefined;
  otherRealms = () => undefined;
  invited = true;
  root = () => undefined;
  // Each test gets fresh modules: one realm's page root is chosen once.
  rstest.resetModules();
  papi = await import("../src/element-papi.ts");
  frameBoot = await import("../src/frame-boot.ts");
});

/** Boots the card in frame 7, as `bobcat:frame` would. */
function boot(): void {
  expect(frameBoot.boot(7, CARD, SRC, "{}", "{}", "{}", "")).toBe(true);
  calls.length = 0;
}

describe("boot", () => {
  it("dispatches the frame's load at the nested page once the root has loaded, before the card renders", () => {
    expect(frameBoot.boot(7, CARD, SRC, "{}", "{}", "{}", "")).toBe(true);
    expect(calls).toEqual([
      ["usePageRoot", PAGE],
      ["initializeMTS"],
      ["initEntry", CARD],
      ["loadModuleSync", `${CARD}/root.js`],
      // Bubbling and composed, stamped at `__CreateEvent`, with web-core's
      // success detail naming `src` as written.
      ["dispatchEvent", PAGE, "load", true, true, TIMELINE, LOADED],
      ["worker", "bobcat:bts"],
      ["connectBackground", `${CARD}#background-thread`],
      ["renderPage"],
    ]);
  });

  it("dispatches it for a root that threw, after the root's report", () => {
    root = () => {
      throw new Error("the root threw");
    };
    expect(frameBoot.boot(7, CARD, SRC, "{}", "{}", "{}", "")).toBe(true);
    expect(calls.slice(3, 6)).toEqual([
      ["loadModuleSync", `${CARD}/root.js`],
      ["reportError", "the root threw"],
      ["dispatchEvent", PAGE, "load", true, true, TIMELINE, LOADED],
    ]);
    expect(calls.at(-1)).toEqual(["renderPage"]);
  });

  it("dispatches nothing for a boot that fails before the root loaded", () => {
    framePageFails = "frames nest eight deep at most";
    expect(frameBoot.boot(7, CARD, SRC, "{}", "{}", "{}", "")).toBe(false);
    expect(calls).toEqual([["reportError", "frames nest eight deep at most"]]);
  });

  it("reports a dispatch that throws and goes on to render", () => {
    dispatchFails = "the detail does not clone";
    expect(frameBoot.boot(7, CARD, SRC, "{}", "{}", "{}", "")).toBe(true);
    expect(calls.slice(4)).toEqual([
      ["dispatchEvent", PAGE, "load", true, true, TIMELINE, LOADED],
      ["reportError", "the detail does not clone"],
      ["worker", "bobcat:bts"],
      ["connectBackground", `${CARD}#background-thread`],
      ["renderPage"],
    ]);
  });

  it("stops, reporting nothing, when a listener of the parent's lets go of the card during the dispatch", () => {
    // The parent's listener, called first, gives the frame a new `src`,
    // which disposes of this card at once, and the parent's reset frees
    // its page, with its registrations, so the host does not call it.
    otherRealms = () => {
      frameBoot.dispose(done);
      invited = false;
    };
    expect(frameBoot.boot(7, CARD, SRC, "{}", "{}", "{}", "")).toBe(false);
    expect(calls.slice(4)).toEqual([
      ["dispatchEvent", PAGE, "load", true, true, TIMELINE, LOADED],
      ["switchExposure", false, true],
      ["closeFrames"],
      ["disposeBackground"],
      ["done"],
    ]);
  });

  it("is heard by a listener the card's root filed on its page, at the target, inside the host's dispatch", () => {
    root = () => {
      papi.__AddEventListener(papi.__GetPageElement(), "load", (event: {
        type: string;
        eventPhase: number;
        target: { uid: number };
        detail: unknown;
      }) => {
        calls.push(["heard", event.type, event.eventPhase, event.target.uid, event.detail]);
      }, {});
    };
    expect(frameBoot.boot(7, CARD, SRC, "{}", "{}", "{}", "")).toBe(true);
    expect(calls.slice(4, 6)).toEqual([
      ["dispatchEvent", PAGE, "load", true, true, TIMELINE, LOADED],
      // `AT_TARGET`, the nested page itself.
      ["heard", "load", 2, PAGE, LOADED],
    ]);
  });

  it("goes on to render when the card's own listener throws, which the host reports", () => {
    root = () => {
      papi.__AddEventListener(papi.__GetPageElement(), "load", () => {
        throw new Error("the card's listener threw");
      }, {});
    };
    expect(frameBoot.boot(7, CARD, SRC, "{}", "{}", "{}", "")).toBe(true);
    expect(calls.slice(4)).toEqual([
      ["dispatchEvent", PAGE, "load", true, true, TIMELINE, LOADED],
      ["listenerFailed", "the card's listener threw"],
      ["worker", "bobcat:bts"],
      ["connectBackground", `${CARD}#background-thread`],
      ["renderPage"],
    ]);
  });
});

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

describe("__BobcatLoadMetrics", () => {
  /** A card's load entry, as the host writes it. */
  const ENTRY = { entryType: "pipeline", name: "loadBundle", loadBundleStart: 1.5 };

  it("dispatches the frame's loadmetrics at the nested page, with native's detail", () => {
    boot();
    frameBoot.__BobcatLoadMetrics(JSON.stringify(ENTRY));
    expect(calls).toEqual([
      // Bubbling and composed, as `load` is, naming `src` as written.
      [
        "dispatchEvent",
        PAGE,
        "loadmetrics",
        true,
        true,
        TIMELINE,
        { url: SRC, mode: "standard", entry: ENTRY },
      ],
    ]);
  });

  it("dispatches nothing for a card the frame let go of", () => {
    boot();
    frameBoot.dispose(done);
    calls.length = 0;
    frameBoot.__BobcatLoadMetrics(JSON.stringify(ENTRY));
    expect(calls).toEqual([]);
  });
});
