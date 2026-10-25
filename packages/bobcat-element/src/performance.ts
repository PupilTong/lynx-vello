// The `bobcat:performance` ESM: `lynx.performance`'s JavaScript half, for
// both realm kinds. It imports nothing: each runtime hands it the host
// members of its own realm (`bobcat-internal:host`'s pipeline hooks, and in
// a worker its observer members), so the one module serves a main-thread
// realm and a worker realm alike.
//
// The pipeline hooks are what ReactLynx calls (`_generatePipelineOptions`,
// `_onPipelineStart`, `_markTiming`, `_bindPipelineIdWithTimingFlag`); each
// unpacks the options object ReactLynx passes into primitive arguments, so
// no object crosses into the host. The entries themselves are built in
// Rust, on the worker's own thread, as one record of key and value fields
// (`takePerformanceEntries`), and folded into objects here.
//
// Lynx's legacy Timing API (`addTimingListener`, `removeTimingListener`,
// `removeAllTimingListener`, `nativeApp.markTiming`) is deliberately absent.

/** What `_generatePipelineOptions` answers and `_onPipelineStart` reads. */
export interface PipelineOptions {
  pipelineID: string;
  needTimestamps: boolean;
  pipelineOrigin?: string;
  dsl?: string;
  stage?: string;
}

/** The four host members the pipeline hooks are written over. */
export interface PipelineHost {
  generatePipelineId(): number;
  onPipelineStart(id: number, origin: string, dsl: string, stage: string, needTimestamps: 0 | 1): void;
  markPipelineTiming(id: number, key: string): void;
  bindPipelineTimingFlag(id: number, flag: string): void;
}

/**
 * A pipeline id as the host takes one: the integer the decimal string
 * `_generatePipelineOptions` answered names, or `0` — "none" — for
 * anything else.
 */
export function pipelineNumber(id: unknown): number {
  const number = Number(id);
  return Number.isSafeInteger(number) && number > 0 ? number : 0;
}

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

/** `lynx.performance`'s four pipeline hooks, over `host`. */
export function createPipelineHooks(host: PipelineHost) {
  return {
    /** A fresh pipeline, which carries no timestamps until a flag says so. */
    _generatePipelineOptions(): PipelineOptions {
      return { pipelineID: String(host.generatePipelineId()), needTimestamps: false };
    },
    /** Opens the pipeline's record: its start, origin, `dsl` and `stage`. */
    _onPipelineStart(pipelineID: unknown, options?: Partial<PipelineOptions> | null) {
      host.onPipelineStart(
        pipelineNumber(pipelineID),
        text(options?.pipelineOrigin),
        text(options?.dsl),
        text(options?.stage),
        options?.needTimestamps ? 1 : 0,
      );
    },
    /** One framework mark (`diffVdomStart`, `mtsRenderEnd`, …), taken now. */
    _markTiming(pipelineID: unknown, key: unknown) {
      host.markPipelineTiming(pipelineNumber(pipelineID), String(key));
    },
    /** The pipeline reports `flag`, and so carries timestamps. */
    _bindPipelineIdWithTimingFlag(pipelineID: unknown, flag: unknown) {
      host.bindPipelineTimingFlag(pipelineNumber(pipelineID), String(flag));
    },
  };
}

/** One entry as an observer receives it: Lynx's `PerformanceEntry`. */
export interface PerformanceEntry {
  entryType: string;
  name: string;
  [key: string]: unknown;
}

/**
 * One category of a `memory` entry: Lynx's `MemoryUsageItem`
 * (`MemoryUsageItem.yml`), with the `instanceCount` Lynx's
 * `MemoryMonitor::ReportMemory` writes beside it.
 */
export interface MemoryUsageItem {
  category: string;
  sizeBytes: number;
  instanceCount: number;
  detail?: Record<string, string>;
}

/**
 * Lynx's `memory` entry (`MemoryUsageEntry.yml`): every category's latest
 * reading under its name, and their sum. Categories are
 * `mainThreadScriptingEngine`, `backgroundThreadScriptingEngine`,
 * `lynxTasmElement` and this engine's own `committedFrame`.
 */
export interface MemoryUsageEntry extends PerformanceEntry {
  entryType: "memory";
  name: "memory";
  sizeBytes: number;
  detail?: Record<string, MemoryUsageItem>;
}

/**
 * Lynx's `resource.lazyBundle` entry (`LazyBundleEntry.yml`), as Lynx's
 * `GetLazyBundleEntry` writes it: one `lynx.fetchBundle` that made a
 * request. `size` is absent when the fetcher did not report one and the
 * decode pair when nothing was installed; `mode` is always `"normal"`, and
 * `sync`, which the YAML lists, is never written, as on native.
 */
export interface LazyBundleEntry extends PerformanceEntry {
  entryType: "resource";
  name: "lazyBundle";
  componentUrl: string;
  size?: number;
  loadSuccess: boolean;
  mode: string;
  decodeStart?: number;
  decodeEnd?: number;
  requireStart: number;
  requireEnd: number;
}

/**
 * W3C Container Timing's `PerformanceContainerTiming` (WICG draft,
 * https://w3c.github.io/container-timing/), in its own field names: a
 * presented frame grew the painted region of an element carrying a
 * `containertiming` attribute. `name` is `""`, as the draft's is, and
 * `identifier` the attribute's value; `startTime` and `firstRenderTime` are
 * Unix milliseconds, as every timestamp of this API is; `intersectionRect`
 * (the region's bounding box) is in viewport CSS pixels and `size` (the
 * region's area) in square CSS pixels.
 *
 * `rootElement` and `lastPaintedElement` are elements in the standard, and
 * a background thread has no element objects, so neither key is present.
 */
export interface ContainerTimingEntry extends PerformanceEntry {
  entryType: "container";
  name: "";
  identifier: string;
  startTime: number;
  duration: 0;
  firstRenderTime: number;
  size: number;
  intersectionRect: { x: number; y: number; width: number; height: number };
}

/**
 * One value field of a record: `n` and a number, `b1` or `b0` for a
 * boolean, or `s` and a string.
 */
function fieldValue(raw: string): unknown {
  switch (raw[0]) {
    case "n":
      return Number(raw.slice(1));
    case "b":
      return raw === "b1";
    default:
      return raw.slice(1);
  }
}

/**
 * A record key's path: split at every `.` no `\` escapes, each segment
 * unescaped (`frameworkRenderingTiming.a\.b` is `["frameworkRenderingTiming",
 * "a.b"]`).
 */
function keyPath(key: string): string[] {
  if (!key.includes("\\")) return key.split(".");
  const path: string[] = [];
  let segment = "";
  for (let index = 0; index < key.length; index += 1) {
    const character = key[index]!;
    if (character === "\\") {
      index += 1;
      segment += key[index] ?? "";
    } else if (character === ".") {
      path.push(segment);
      segment = "";
    } else {
      segment += character;
    }
  }
  path.push(segment);
  return path;
}

/**
 * The entries one `takePerformanceEntries` record holds: per entry, its
 * pair count, then a key field and a value field per pair, the value `n`
 * and a number, `b1` or `b0` for a boolean, or `s` and a string. A key is a path: each `.` opens one
 * nested object (`frameworkRenderingTiming.diffVdomStart`,
 * `detail.lynxTasmElement.detail.elementCount`), and a `.` or `\` inside a
 * name is escaped with `\`.
 */
export function decodeEntries(record: string, split: (record: string) => string[]): PerformanceEntry[] {
  const fields = split(record);
  const entries: PerformanceEntry[] = [];
  let index = 0;
  while (index < fields.length) {
    const pairs = Number(fields[index++]);
    const entry: Record<string, unknown> = {};
    for (let pair = 0; pair < pairs; pair += 1) {
      const key = fields[index++]!;
      const raw = fields[index++]!;
      const value = fieldValue(raw);
      const path = keyPath(key);
      const leaf = path.pop()!;
      let object = entry;
      for (const segment of path) {
        object = (object[segment] ??= {}) as Record<string, unknown>;
      }
      object[leaf] = value;
    }
    entries.push(entry as PerformanceEntry);
  }
  return entries;
}

/** The global event every entry is emitted as, as on native. */
export const PERFORMANCE_EVENT = "lynx.performance.onPerformanceEvent";

/** What `createObserver` answers. */
export interface PerformanceObserver {
  /**
   * Receives every entry whose `entryType`, or `entryType.name`, is in
   * `names`, replacing the list given before; an empty list receives
   * nothing.
   */
  observe(names: string[]): void;
  /** Receives nothing more. */
  disconnect(): void;
}

/** The worker realm's two observer members. */
export interface ObserverHost {
  /**
   * The realm has an observer (`on`), one of its observers asked for
   * `paint` entries (`paint`), which is what turns the paint walk's
   * contentful test on, one asked for `memory` entries (`memory`), one
   * asked for `resource` entries (`resource`), which is what times the
   * view's `fetchBundle`s, and one asked for `container` entries
   * (`container`). Asking for `memory` turns no monitor on: the memory
   * monitor is the embedder's (`LynxView::set_memory_monitor`), and an
   * observer of `memory` receives what it reports while it is on, and
   * nothing while it is off. Asking for `container` turns nothing on
   * either: a `containertiming` attribute is what makes the paint walk
   * record, and an observer of `container` receives every root's entries,
   * the page's and every `<frame>` card's.
   */
  observePerformance(on: 0 | 1, paint: 0 | 1, memory: 0 | 1, resource: 0 | 1, container: 0 | 1): void;
  takePerformanceEntries(): string;
}

/** Whether `names` asks for entries of `entryType`. */
function asksFor(names: readonly string[], entryType: string): boolean {
  return names.some(name => name === entryType || name.startsWith(`${entryType}.`));
}

/** The part of `GlobalEventEmitter` the observers use. */
export interface PerformanceEmitter {
  addListener(name: string, listener: (...args: unknown[]) => void): void;
  removeListener(name: string, listener: (...args: unknown[]) => void): void;
  emit(name: string, args: unknown[]): void;
}

/**
 * The BTS's `createObserver` and the delivery its worker's
 * `PerformanceReady` runs. Each observer is one listener on `emitter`'s
 * `lynx.performance.onPerformanceEvent`, so a
 * `GlobalEventEmitter` listener hears the same entries, as on native. The
 * host is told only when the realm goes from no observer to one and back,
 * from no observer of `paint` (`observe(['paint'])`, W3C Paint Timing's
 * `first-paint` and `first-contentful-paint`) to one and back, from no
 * observer of `memory` (`observe(['memory'])`, Lynx's `memory` entry) to
 * one and back, from no observer of `resource` (`observe(['resource'])`,
 * Lynx's `resource.lazyBundle`) to one and back, or from no observer of
 * `container` (`observe(['container'])`, W3C Container Timing's
 * `container`) to one and back: until then nothing is sent to this worker
 * or assembled for it.
 */
export function createPerformanceObservers(
  host: ObserverHost,
  emitter: PerformanceEmitter,
  split: (record: string) => string[],
) {
  const active = new Map<PerformanceObserver, readonly string[]>();
  let observing = false;
  let observingPaint = false;
  let observingMemory = false;
  let observingResource = false;
  let observingContainer = false;
  function sync() {
    const next = active.size > 0;
    let paint = false;
    let memory = false;
    let resource = false;
    let container = false;
    for (const names of active.values()) {
      paint ||= asksFor(names, "paint");
      memory ||= asksFor(names, "memory");
      resource ||= asksFor(names, "resource");
      container ||= asksFor(names, "container");
    }
    if (
      next === observing
      && paint === observingPaint
      && memory === observingMemory
      && resource === observingResource
      && container === observingContainer
    ) return;
    observing = next;
    observingPaint = paint;
    observingMemory = memory;
    observingResource = resource;
    observingContainer = container;
    host.observePerformance(
      next ? 1 : 0,
      paint ? 1 : 0,
      memory ? 1 : 0,
      resource ? 1 : 0,
      container ? 1 : 0,
    );
  }
  function createObserver(callback: (entry: PerformanceEntry) => void): PerformanceObserver {
    if (typeof callback !== "function") {
      throw new TypeError("createObserver requires a callback");
    }
    let names: string[] = [];
    const listener = (entry: unknown) => {
      const { entryType, name } = entry as PerformanceEntry;
      if (names.some(wanted => wanted === entryType || wanted === `${entryType}.${name}`)) {
        callback(entry as PerformanceEntry);
      }
    };
    const observer: PerformanceObserver = {
      observe(list: unknown) {
        names = Array.isArray(list) ? list.filter((name): name is string => typeof name === "string") : [];
        if (names.length === 0) {
          observer.disconnect();
          return;
        }
        if (!active.has(observer)) {
          emitter.addListener(PERFORMANCE_EVENT, listener);
        }
        active.set(observer, names);
        sync();
      },
      disconnect() {
        if (!active.delete(observer)) return;
        emitter.removeListener(PERFORMANCE_EVENT, listener);
        sync();
      },
    };
    return observer;
  }
  /** Takes every entry this worker is owed and emits each. */
  function deliver() {
    const record = host.takePerformanceEntries();
    if (record === "") return;
    for (const entry of decodeEntries(record, split)) {
      emitter.emit(PERFORMANCE_EVENT, [entry]);
    }
  }
  return { createObserver, deliver };
}
