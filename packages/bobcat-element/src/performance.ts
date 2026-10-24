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
 * The entries one `takePerformanceEntries` record holds: per entry, its
 * pair count, then a key field and a value field per pair, the value `n`
 * and a number or `s` and a string. A dotted key is a member of the object
 * its first segment names (`frameworkRenderingTiming.diffVdomStart`).
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
      const value = raw.startsWith("n") ? Number(raw.slice(1)) : raw.slice(1);
      const dot = key.indexOf(".");
      if (dot === -1) {
        entry[key] = value;
      } else {
        const outer = key.slice(0, dot);
        const object = (entry[outer] ??= {}) as Record<string, unknown>;
        object[key.slice(dot + 1)] = value;
      }
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
  observePerformance(on: 0 | 1): void;
  takePerformanceEntries(): string;
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
 * host is told only when the realm goes from no observer to one and back:
 * until then nothing is sent to this worker or assembled for it.
 */
export function createPerformanceObservers(
  host: ObserverHost,
  emitter: PerformanceEmitter,
  split: (record: string) => string[],
) {
  const active = new Set<PerformanceObserver>();
  let observing = false;
  function sync() {
    const next = active.size > 0;
    if (next === observing) return;
    observing = next;
    host.observePerformance(next ? 1 : 0);
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
          active.add(observer);
          emitter.addListener(PERFORMANCE_EVENT, listener);
        }
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
