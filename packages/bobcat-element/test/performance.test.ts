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

describe("memory entries", () => {
  it("fold a key path of any depth into nested objects", () => {
    // As the host writes a `memory` entry: one object per category under
    // `detail`, each with its own string `detail`, then the total.
    const record = entryRecord([
      ["entryType", "smemory"],
      ["name", "smemory"],
      ["detail.mainThreadScriptingEngine.category", "smainThreadScriptingEngine"],
      ["detail.mainThreadScriptingEngine.sizeBytes", "n4096"],
      ["detail.mainThreadScriptingEngine.instanceCount", "n1"],
      ["detail.mainThreadScriptingEngine.detail.obj_count", "s12"],
      ["detail.mainThreadScriptingEngine.detail.sharedRuntime", "sthread"],
      ["detail.lynxTasmElement.category", "slynxTasmElement"],
      ["detail.lynxTasmElement.sizeBytes", "n640"],
      ["detail.lynxTasmElement.instanceCount", "n2"],
      ["detail.lynxTasmElement.detail.elementCount", "s2"],
      ["detail.lynxTasmElement.detail.singleElementSizeBytes", "s320"],
      ["sizeBytes", "n4736"],
    ]);
    expect(decodeEntries(record, splitRecord)).toEqual([
      {
        entryType: "memory",
        name: "memory",
        detail: {
          mainThreadScriptingEngine: {
            category: "mainThreadScriptingEngine",
            sizeBytes: 4096,
            instanceCount: 1,
            detail: { obj_count: "12", sharedRuntime: "thread" },
          },
          lynxTasmElement: {
            category: "lynxTasmElement",
            sizeBytes: 640,
            instanceCount: 2,
            detail: { elementCount: "2", singleElementSizeBytes: "320" },
          },
        },
        sizeBytes: 4736,
      },
    ]);
  });

  it("keep an escaped dot or backslash inside one name", () => {
    const record = entryRecord([
      ["entryType", "spipeline"],
      ["frameworkRenderingTiming.mark\\.with\\.dots", "n1.500"],
      ["frameworkRenderingTiming.back\\\\slash", "n2"],
    ]);
    expect(decodeEntries(record, splitRecord)).toEqual([
      {
        entryType: "pipeline",
        frameworkRenderingTiming: { "mark.with.dots": 1.5, "back\\slash": 2 },
      },
    ]);
  });
});

describe("resource entries", () => {
  it("read booleans back and keep a lazy bundle's keys flat", () => {
    // As the host writes a `resource.lazyBundle` entry.
    const record = entryRecord([
      ["entryType", "sresource"],
      ["name", "slazyBundle"],
      ["componentUrl", "sapp:///lazy.bundle"],
      ["size", "n2048"],
      ["loadSuccess", "b1"],
      ["mode", "snormal"],
      ["decodeStart", "n1739594612310.000"],
      ["decodeEnd", "n1739594612311.500"],
      ["requireStart", "n1739594612300.000"],
      ["requireEnd", "n1739594612309.250"],
    ]) + entryRecord([
      ["entryType", "sresource"],
      ["name", "slazyBundle"],
      ["componentUrl", "sapp:///missing.bundle"],
      ["loadSuccess", "b0"],
      ["mode", "snormal"],
      ["requireStart", "n1"],
      ["requireEnd", "n2"],
    ]);
    expect(decodeEntries(record, splitRecord)).toEqual([
      {
        entryType: "resource",
        name: "lazyBundle",
        componentUrl: "app:///lazy.bundle",
        size: 2048,
        loadSuccess: true,
        mode: "normal",
        decodeStart: 1739594612310,
        decodeEnd: 1739594612311.5,
        requireStart: 1739594612300,
        requireEnd: 1739594612309.25,
      },
      {
        entryType: "resource",
        name: "lazyBundle",
        componentUrl: "app:///missing.bundle",
        loadSuccess: false,
        mode: "normal",
        requireStart: 1,
        requireEnd: 2,
      },
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
    expect(host.observePerformance.mock.calls).toEqual([[1, 0, 0, 0, 0]]);
    first.disconnect();
    expect(host.observePerformance.mock.calls).toEqual([[1, 0, 0, 0, 0]]);
    second.observe([]);
    expect(host.observePerformance.mock.calls).toEqual([[1, 0, 0, 0, 0], [0, 0, 0, 0, 0]]);
  });

  it("tell the host when an observer asks for paint entries and when none does", () => {
    const { host, observers } = setup("");
    const pipelines = observers.createObserver(() => {});
    const paints = observers.createObserver(() => {});
    pipelines.observe(["pipeline"]);
    paints.observe(["paint"]);
    expect(host.observePerformance.mock.calls).toEqual([[1, 0, 0, 0, 0], [1, 1, 0, 0, 0]]);
    paints.observe(["paint.first-contentful-paint", "pipeline"]);
    expect(host.observePerformance.mock.calls).toEqual([[1, 0, 0, 0, 0], [1, 1, 0, 0, 0]]);
    paints.observe(["pipeline"]);
    expect(host.observePerformance.mock.calls).toEqual([[1, 0, 0, 0, 0], [1, 1, 0, 0, 0], [1, 0, 0, 0, 0]]);
    paints.observe(["paint"]);
    pipelines.disconnect();
    paints.disconnect();
    expect(host.observePerformance.mock.calls).toEqual([[1, 0, 0, 0, 0], [1, 1, 0, 0, 0], [1, 0, 0, 0, 0], [1, 1, 0, 0, 0], [0, 0, 0, 0, 0]]);
  });

  it("tell the host when an observer asks for memory entries and when none does", () => {
    const { host, observers } = setup("");
    const pipelines = observers.createObserver(() => {});
    const memory = observers.createObserver(() => {});
    pipelines.observe(["pipeline", "paint"]);
    memory.observe(["memory"]);
    expect(host.observePerformance.mock.calls).toEqual([[1, 1, 0, 0, 0], [1, 1, 1, 0, 0]]);
    memory.observe(["memory.memory", "pipeline"]);
    expect(host.observePerformance.mock.calls).toEqual([[1, 1, 0, 0, 0], [1, 1, 1, 0, 0]]);
    memory.disconnect();
    expect(host.observePerformance.mock.calls).toEqual([[1, 1, 0, 0, 0], [1, 1, 1, 0, 0], [1, 1, 0, 0, 0]]);
    pipelines.disconnect();
    expect(host.observePerformance.mock.calls).toEqual([[1, 1, 0, 0, 0], [1, 1, 1, 0, 0], [1, 1, 0, 0, 0], [0, 0, 0, 0, 0]]);
  });

  it("tell the host when an observer asks for resource entries and when none does", () => {
    const { host, observers } = setup("");
    const pipelines = observers.createObserver(() => {});
    const resources = observers.createObserver(() => {});
    pipelines.observe(["pipeline"]);
    resources.observe(["resource.lazyBundle"]);
    expect(host.observePerformance.mock.calls).toEqual([[1, 0, 0, 0, 0], [1, 0, 0, 1, 0]]);
    resources.observe(["resource", "pipeline"]);
    expect(host.observePerformance.mock.calls).toEqual([[1, 0, 0, 0, 0], [1, 0, 0, 1, 0]]);
    resources.disconnect();
    expect(host.observePerformance.mock.calls).toEqual([[1, 0, 0, 0, 0], [1, 0, 0, 1, 0], [1, 0, 0, 0, 0]]);
    pipelines.disconnect();
    expect(host.observePerformance.mock.calls).toEqual([
      [1, 0, 0, 0, 0],
      [1, 0, 0, 1, 0],
      [1, 0, 0, 0, 0],
      [0, 0, 0, 0, 0],
    ]);
  });

  it("tell the host when an observer asks for container entries and when none does", () => {
    const { host, observers } = setup("");
    const pipelines = observers.createObserver(() => {});
    const containers = observers.createObserver(() => {});
    pipelines.observe(["pipeline"]);
    containers.observe(["container"]);
    expect(host.observePerformance.mock.calls).toEqual([[1, 0, 0, 0, 0], [1, 0, 0, 0, 1]]);
    containers.observe(["container", "pipeline"]);
    expect(host.observePerformance.mock.calls).toEqual([[1, 0, 0, 0, 0], [1, 0, 0, 0, 1]]);
    containers.disconnect();
    pipelines.disconnect();
    expect(host.observePerformance.mock.calls).toEqual([
      [1, 0, 0, 0, 0],
      [1, 0, 0, 0, 1],
      [1, 0, 0, 0, 0],
      [0, 0, 0, 0, 0],
    ]);
  });

  it("receive container entries with intersectionRect as an object", () => {
    const record = entryRecord([
      ["entryType", "scontainer"],
      ["name", "s"],
      ["startTime", "n1739594612310.000"],
      ["duration", "n0"],
      ["identifier", "shero"],
      ["intersectionRect.x", "n10"],
      ["intersectionRect.y", "n20"],
      ["intersectionRect.width", "n40.5"],
      ["intersectionRect.height", "n20"],
      ["size", "n810"],
      ["firstRenderTime", "n1739594612307.429"],
    ]);
    const { observers } = setup(record);
    const heard: unknown[] = [];
    observers.createObserver(entry => heard.push(entry)).observe(["container"]);
    observers.deliver();
    expect(heard).toEqual([
      {
        entryType: "container",
        name: "",
        startTime: 1739594612310,
        duration: 0,
        identifier: "hero",
        intersectionRect: { x: 10, y: 20, width: 40.5, height: 20 },
        size: 810,
        firstRenderTime: 1739594612307.429,
      },
    ]);
  });

  it("receive memory entries by type and by type and name", () => {
    const record = entryRecord([
      ["entryType", "smemory"],
      ["name", "smemory"],
      ["detail.committedFrame.category", "scommittedFrame"],
      ["detail.committedFrame.sizeBytes", "n100"],
      ["detail.committedFrame.instanceCount", "n1"],
      ["detail.committedFrame.detail.fragments", "s1"],
      ["sizeBytes", "n100"],
    ]) + entryRecord([["entryType", "spipeline"], ["name", "sloadBundle"]]);
    const { observers } = setup(record);
    const memory: unknown[] = [];
    const named: unknown[] = [];
    observers.createObserver(entry => memory.push(entry)).observe(["memory"]);
    observers.createObserver(entry => named.push(entry.entryType)).observe(["memory.memory"]);
    observers.deliver();
    expect(memory).toEqual([
      {
        entryType: "memory",
        name: "memory",
        detail: {
          committedFrame: {
            category: "committedFrame",
            sizeBytes: 100,
            instanceCount: 1,
            detail: { fragments: "1" },
          },
        },
        sizeBytes: 100,
      },
    ]);
    expect(named).toEqual(["memory"]);
  });

  it("receive paint entries by type and by type and name", () => {
    const record = entryRecord([["entryType", "spaint"], ["name", "sfirst-paint"], ["startTime", "n1739594612307.429"]])
      + entryRecord([
        ["entryType", "spaint"],
        ["name", "sfirst-contentful-paint"],
        ["startTime", "n1739594612310.000"],
      ]);
    const { observers } = setup(record);
    const all: unknown[] = [];
    const contentful: unknown[] = [];
    observers.createObserver(entry => all.push(entry)).observe(["paint"]);
    observers.createObserver(entry => contentful.push(entry.name)).observe(["paint.first-contentful-paint"]);
    observers.deliver();
    expect(all).toEqual([
      { entryType: "paint", name: "first-paint", startTime: 1739594612307.429 },
      { entryType: "paint", name: "first-contentful-paint", startTime: 1739594612310 },
    ]);
    expect(contentful).toEqual(["first-contentful-paint"]);
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
