import { describe, expect, it, rstest } from "@rstest/core";
import { GlobalEventEmitter } from "../src/global-event-emitter.ts";
import {
  PERFORMANCE_EVENT,
  createPerformanceObservers,
  createPipelineHooks,
  decodeEntries,
  pipelineNumber,
} from "../src/performance.ts";
import { splitRecord } from "../src/record.ts";

/** One `<utf16Length>:<text>` field, as the host writes it. */
function field(text: string): string {
  return `${text.length}:${text}`;
}

/** One entry as `takePerformanceEntries` writes it. */
function entryRecord(pairs: [string, string][]): string {
  return field(String(pairs.length)) + pairs.map(([key, value]) => field(key) + field(value)).join("");
}

describe("pipeline hooks", () => {
  it("unpack ReactLynx's options into primitive host arguments", () => {
    const host = {
      generatePipelineId: rstest.fn(() => 41),
      onPipelineStart: rstest.fn(),
      markPipelineTiming: rstest.fn(),
      bindPipelineTimingFlag: rstest.fn(),
    };
    const hooks = createPipelineHooks(host);
    const options = hooks._generatePipelineOptions();
    expect(options).toEqual({ pipelineID: "41", needTimestamps: false });
    hooks._onPipelineStart("41", {
      ...options,
      needTimestamps: true,
      pipelineOrigin: "updateTriggeredByBts",
      dsl: "reactLynx",
      stage: "update",
    });
    expect(host.onPipelineStart).toHaveBeenCalledWith(41, "updateTriggeredByBts", "reactLynx", "update", 1);
    // An SDK before 3.0 passes no options.
    hooks._onPipelineStart("41");
    expect(host.onPipelineStart).toHaveBeenLastCalledWith(41, "", "", "", 0);
    hooks._markTiming("41", "diffVdomStart");
    expect(host.markPipelineTiming).toHaveBeenCalledWith(41, "diffVdomStart");
    hooks._bindPipelineIdWithTimingFlag("41", "flag");
    expect(host.bindPipelineTimingFlag).toHaveBeenCalledWith(41, "flag");
  });

  it("names no pipeline for an id that is not one", () => {
    expect(pipelineNumber("7")).toBe(7);
    for (const id of [undefined, "", "x", "1.5", -1, 0]) expect(pipelineNumber(id)).toBe(0);
  });
});

describe("entries", () => {
  it("fold dotted keys into objects and read numbers back", () => {
    const record = entryRecord([
      ["entryType", "spipeline"],
      ["name", "sloadBundle"],
      ["paintEnd", "n1739594612307.429"],
      ["lynxFcp.name", "slynxFcp"],
      ["lynxFcp.duration", "n12.500"],
      ["hostPlatformTiming.hostPlatformType", "svello"],
    ]) + entryRecord([["entryType", "spipeline"], ["name", "supdateTriggeredByBts"]]);
    expect(decodeEntries(record, splitRecord)).toEqual([
      {
        entryType: "pipeline",
        name: "loadBundle",
        paintEnd: 1739594612307.429,
        lynxFcp: { name: "lynxFcp", duration: 12.5 },
        hostPlatformTiming: { hostPlatformType: "vello" },
      },
      { entryType: "pipeline", name: "updateTriggeredByBts" },
    ]);
  });
});

describe("observers", () => {
  function setup(record: string) {
    const host = {
      observePerformance: rstest.fn(),
      takePerformanceEntries: rstest.fn(() => record),
    };
    const emitter = new GlobalEventEmitter();
    return { host, emitter, observers: createPerformanceObservers(host, emitter, splitRecord) };
  }

  it("tell the host when the realm starts and stops observing", () => {
    const { host, observers } = setup("");
    const first = observers.createObserver(() => {});
    const second = observers.createObserver(() => {});
    first.observe(["pipeline"]);
    second.observe(["pipeline.loadBundle"]);
    expect(host.observePerformance.mock.calls).toEqual([[1]]);
    first.disconnect();
    expect(host.observePerformance.mock.calls).toEqual([[1]]);
    second.observe([]);
    expect(host.observePerformance.mock.calls).toEqual([[1], [0]]);
  });

  it("receive the entries whose type or type and name they observe", () => {
    const record = entryRecord([["entryType", "spipeline"], ["name", "sloadBundle"]])
      + entryRecord([["entryType", "spipeline"], ["name", "supdateTriggeredByBts"]]);
    const { emitter, observers } = setup(record);
    const all: unknown[] = [];
    const loads: unknown[] = [];
    observers.createObserver(entry => all.push(entry.name)).observe(["pipeline"]);
    observers.createObserver(entry => loads.push(entry.name)).observe(["pipeline.loadBundle"]);
    const heard: unknown[] = [];
    emitter.addListener(PERFORMANCE_EVENT, entry => heard.push((entry as { name: string }).name));
    observers.deliver();
    expect(all).toEqual(["loadBundle", "updateTriggeredByBts"]);
    expect(loads).toEqual(["loadBundle"]);
    expect(heard).toEqual(["loadBundle", "updateTriggeredByBts"]);
  });

  it("hear nothing once disconnected", () => {
    const { observers } = setup(entryRecord([["entryType", "spipeline"], ["name", "sloadBundle"]]));
    const heard: unknown[] = [];
    const observer = observers.createObserver(entry => heard.push(entry));
    observer.observe(["pipeline"]);
    observer.disconnect();
    observers.deliver();
    expect(heard).toEqual([]);
  });
});
