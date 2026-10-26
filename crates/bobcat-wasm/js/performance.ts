// Lynx performance entries as a page receives them: the JSON object Lynx
// native's `onPerformanceEvent` hands a host, which the engine writes on the
// Render Worker and the facade parses here. Everything in this module is
// pure — no DOM, no Worker — so a panel draws from it and a Node test checks
// it directly.

/** Lynx's `PerformanceMetric`: a named span between two named marks. */
export interface LynxPerformanceMetric {
  name: string
  /** Milliseconds. */
  duration: number
  startTimestampName: string
  /** Unix milliseconds. */
  startTimestamp: number
  endTimestampName: string
  /** Unix milliseconds. */
  endTimestamp: number
}

/**
 * Any entry. `entryType` and `name` are always present; every other key is
 * Lynx's, and a key the engine has no value for is absent rather than zero.
 */
export interface LynxPerformanceEntry {
  entryType: string
  name: string
  [key: string]: unknown
}

/**
 * A `pipeline` entry: a load (`loadBundle`), a reload
 * (`reloadBundleFromNative`, `reloadBundleFromBts`) or one pipeline that
 * carried a `__lynx_timing_flag` (`updateTriggeredByBts` and the rest).
 * Every timestamp is Unix milliseconds with three decimals.
 */
export interface LynxPipelineEntry extends LynxPerformanceEntry {
  entryType: 'pipeline'
  /** The `__lynx_timing_flag`; `''` on a load's or a reload's own entry. */
  identifier: string
  pipelineStart: number
  pipelineEnd: number
  mtsRenderStart?: number
  mtsRenderEnd?: number
  resolveStart: number
  resolveEnd: number
  layoutStart: number
  layoutEnd: number
  paintingUiOperationExecuteStart: number
  paintingUiOperationExecuteEnd: number
  layoutUiOperationExecuteStart: number
  layoutUiOperationExecuteEnd: number
  paintEnd: number
  // A load's marks.
  loadBundleStart?: number
  loadBundleEnd?: number
  parseStart?: number
  parseEnd?: number
  loadBackgroundStart?: number
  loadBackgroundEnd?: number
  createLynxStart?: number
  createLynxEnd?: number
  loadCoreStart?: number
  loadCoreEnd?: number
  // A reload's marks.
  reloadBundleStart?: number
  reloadBundleEnd?: number
  reloadBackgroundStart?: number
  reloadBackgroundEnd?: number
  // The host's `setExtraTiming` marks, on a load.
  openTime?: number
  containerInitStart?: number
  containerInitEnd?: number
  prepareTemplateStart?: number
  prepareTemplateEnd?: number
  // A load's or a reload's metrics; `fcp` and `totalFcp` once the host set
  // the marks they start at.
  lynxFcp?: LynxPerformanceMetric
  fcp?: LynxPerformanceMetric
  totalFcp?: LynxPerformanceMetric
  // On the entry flagged `__lynx_timing_actual_fmp`.
  actualFmp?: LynxPerformanceMetric
  lynxActualFmp?: LynxPerformanceMetric
  totalActualFmp?: LynxPerformanceMetric
  /** The framework's `dsl`, `stage` and `_markTiming` marks. */
  frameworkRenderingTiming?: Record<string, string | number>
  /** This engine's painter stages: `hostPlatformType` is `'vello'`. */
  hostPlatformTiming?: {
    hostPlatformType: string
    composeStart: number
    composeEnd: number
  }
}

/**
 * A W3C Paint Timing `paint` entry: `first-paint` or
 * `first-contentful-paint`. `startTime` is Unix milliseconds like every
 * pipeline mark, not the standard's milliseconds after `timeOrigin`.
 */
export interface LynxPaintEntry extends LynxPerformanceEntry {
  entryType: 'paint'
  startTime: number
}

/** One stage pair of a pipeline entry: `<name>Start` to `<name>End`. */
export interface LynxStage {
  /** The Lynx key without its `Start`/`End` suffix, such as `layout`. */
  name: string
  start: number
  end: number
}

/** The axis a waterfall is drawn on, in Unix milliseconds. */
export interface WaterfallAxis {
  start: number
  end: number
}

/** One stage bar, its left edge and width as percentages of the axis. */
export interface WaterfallBar extends LynxStage {
  duration: number
  left: number
  width: number
}

/** One `paint` entry's mark on the axis. */
export interface WaterfallMarker {
  name: string
  time: number
  left: number
}

export interface Waterfall {
  axis: WaterfallAxis
  bars: WaterfallBar[]
  markers: WaterfallMarker[]
}

/**
 * The thread a mark is taken on in the browser embedder: the page's own
 * (`openTime`), the Render Worker (the host's template fetch and the view's
 * construction, and the painter's compose and present), the Lynx main
 * thread, or the background thread (BTS).
 */
export type LynxThread = 'page' | 'render' | 'main' | 'background'

/**
 * One part of `totalFcp`, from one host or engine mark to the next: the
 * wait before the fetch (`open`), the template fetch (`fetchBundle`), its
 * decode and the view's group (`prepare`), `createLynx`, the view's boot up
 * to the entry's evaluation (`viewBoot`), and `lynxFcp`.
 */
export interface FcpSegment {
  name: 'open' | 'fetchBundle' | 'prepare' | 'createLynx' | 'viewBoot' | 'lynxFcp'
  thread: LynxThread
  start: number
  end: number
  duration: number
  left: number
  width: number
}

/** One of Lynx's three load metrics, drawn as a span over the segments. */
export interface FcpSpan {
  name: 'totalFcp' | 'fcp' | 'lynxFcp'
  duration: number
  left: number
  width: number
}

export interface FcpBreakdown {
  axis: WaterfallAxis
  segments: FcpSegment[]
  spans: FcpSpan[]
}

/**
 * The engine's stage pairs a waterfall draws, in Lynx's order: the load's
 * or reload's own marks, the background thread's, then the pipeline's. The
 * host's marks before `loadBundleStart` are not stages of this waterfall;
 * `fcpBreakdown` draws them.
 */
const STAGE_NAMES = [
  'loadBundle',
  'reloadBundle',
  'parse',
  'loadCore',
  'loadBackground',
  'reloadBackground',
  'mtsRender',
  'resolve',
  'layout',
  'paintingUiOperationExecute',
  'layoutUiOperationExecute',
] as const

/** Where each stage pair is marked. */
const STAGE_THREADS: Readonly<Record<string, LynxThread>> = {
  prepareTemplate: 'render',
  createLynx: 'render',
  loadBundle: 'main',
  reloadBundle: 'main',
  // The bundle's decode runs in the embedder's fetcher.
  parse: 'render',
  loadCore: 'background',
  loadBackground: 'background',
  reloadBackground: 'background',
  mtsRender: 'main',
  resolve: 'main',
  layout: 'main',
  paintingUiOperationExecute: 'main',
  // The painter's compose of the presented frame.
  layoutUiOperationExecute: 'render',
}

/** The pipeline names whose entry is a page's own load. */
const LOAD_NAMES: ReadonlySet<string> = new Set([
  'loadBundle',
  'reloadBundleFromNative',
])

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function finite(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value)
}

/**
 * One entry from the engine's JSON text, or `undefined` for text that is not
 * an object with a string `entryType` and `name`.
 */
export function parsePerformanceEntry(text: string): LynxPerformanceEntry | undefined {
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch {
    return undefined
  }
  if (
    !isRecord(value) ||
    typeof value['entryType'] !== 'string' ||
    typeof value['name'] !== 'string'
  ) {
    return undefined
  }
  return value as LynxPerformanceEntry
}

export function isPipelineEntry(entry: LynxPerformanceEntry): entry is LynxPipelineEntry {
  return (
    entry.entryType === 'pipeline' &&
    finite(entry['pipelineStart']) &&
    finite(entry['paintEnd'])
  )
}

export function isPaintEntry(entry: LynxPerformanceEntry): entry is LynxPaintEntry {
  return entry.entryType === 'paint' && finite(entry['startTime'])
}

/** Whether a pipeline entry is a page's own load, which begins a new group. */
export function isLoadEntry(entry: LynxPipelineEntry): boolean {
  return LOAD_NAMES.has(entry.name)
}

/** Where a load's axis begins: `loadBundleStart`, or the reload's start. */
function loadStart(entry: LynxPipelineEntry): number {
  return entry.loadBundleStart ?? entry.reloadBundleStart ?? entry.pipelineStart
}

/** The thread a stage pair is marked on; the Lynx main thread when unknown. */
export function stageThread(name: string): LynxThread {
  return STAGE_THREADS[name] ?? 'main'
}

/** `paintEnd − pipelineStart`, the pipeline's whole span. */
export function pipelineDuration(entry: LynxPipelineEntry): number {
  return entry.paintEnd - entry.pipelineStart
}

/**
 * Every stage pair present in `entry`, ordered by start. A pair with either
 * mark missing is left out, never reported as zero.
 */
export function pipelineStages(entry: LynxPipelineEntry): LynxStage[] {
  const stages: LynxStage[] = []
  for (const name of STAGE_NAMES) {
    const start = entry[`${name}Start`]
    const end = entry[`${name}End`]
    if (finite(start) && finite(end)) {
      stages.push({ name, start, end })
    }
  }
  // `sort` is stable, so stages that start together keep Lynx's order.
  return stages.sort((a, b) => a.start - b.start)
}

/**
 * The stage waterfall of `entry`, on one axis from `start` (by default the
 * load's `loadBundleStart`, or `pipelineStart` for any other pipeline) to its
 * `paintEnd`. The axis widens to take in any stage or `paints` mark that
 * falls outside it, so nothing is drawn off its edge.
 */
export function pipelineWaterfall(
  entry: LynxPipelineEntry,
  paints: readonly LynxPaintEntry[] = [],
  start: number = isLoadEntry(entry) ? loadStart(entry) : entry.pipelineStart,
): Waterfall {
  const stages = pipelineStages(entry)
  let axisStart = start
  let axisEnd = entry.paintEnd
  for (const stage of stages) {
    axisStart = Math.min(axisStart, stage.start)
    axisEnd = Math.max(axisEnd, stage.end)
  }
  for (const paint of paints) {
    axisStart = Math.min(axisStart, paint.startTime)
    axisEnd = Math.max(axisEnd, paint.startTime)
  }
  const span = axisEnd - axisStart
  const percent = (time: number): number =>
    span > 0 ? ((time - axisStart) / span) * 100 : 0
  return {
    axis: { start: axisStart, end: axisEnd },
    bars: stages.map((stage) => ({
      ...stage,
      duration: stage.end - stage.start,
      left: percent(stage.start),
      width: percent(stage.end) - percent(stage.start),
    })),
    markers: paints.map((paint) => ({
      name: paint.name,
      time: paint.startTime,
      left: percent(paint.startTime),
    })),
  }
}

/** The marks `fcpBreakdown` cuts at, each starting the segment it names. */
const FCP_MARKS: readonly (readonly [FcpSegment['name'], string, LynxThread])[] = [
  ['open', 'openTime', 'page'],
  ['fetchBundle', 'prepareTemplateStart', 'render'],
  ['prepare', 'prepareTemplateEnd', 'render'],
  ['createLynx', 'createLynxStart', 'render'],
  ['viewBoot', 'createLynxEnd', 'main'],
  ['lynxFcp', 'loadBundleStart', 'main'],
]

function metricOf(entry: LynxPipelineEntry, key: string): LynxPerformanceMetric | undefined {
  const value = entry[key]
  return isRecord(value) && finite(value['duration']) && finite(value['endTimestamp'])
    ? (value as unknown as LynxPerformanceMetric)
    : undefined
}

/**
 * A load's `totalFcp` cut at each host and engine mark it carries, from
 * `openTime` (or `prepareTemplateStart` when the host set no `openTime`) to
 * the first paint every metric ends at, on one axis, with the spans of
 * `totalFcp`, `fcp` and `lynxFcp` over it. A mark the entry lacks merges its
 * segment into the one before. `undefined` for an entry without
 * `prepareTemplateStart`, which has no `fcp` to break down.
 *
 * Segments are measured as reported. Marks taken on different threads can
 * disagree by a clock offset, so a segment can come out negative; it is then
 * drawn with no width.
 */
export function fcpBreakdown(entry: LynxPipelineEntry): FcpBreakdown | undefined {
  const lynxFcp = metricOf(entry, 'lynxFcp')
  if (!finite(entry.prepareTemplateStart) || !finite(entry.loadBundleStart)) {
    return undefined
  }
  const end = lynxFcp?.endTimestamp ?? entry.paintEnd
  const marks = FCP_MARKS.flatMap(([name, key, thread]) => {
    const at = entry[key]
    return finite(at) ? [{ name, at, thread }] : []
  })
  const axisStart = Math.min(...marks.map((mark) => mark.at))
  const axisEnd = Math.max(end, ...marks.map((mark) => mark.at))
  const span = axisEnd - axisStart
  const percent = (time: number): number =>
    span > 0 ? ((time - axisStart) / span) * 100 : 0
  const segments = marks.map((mark, index): FcpSegment => {
    const segmentEnd = marks[index + 1]?.at ?? end
    return {
      name: mark.name,
      thread: mark.thread,
      start: mark.at,
      end: segmentEnd,
      duration: segmentEnd - mark.at,
      left: percent(mark.at),
      width: Math.max(0, percent(segmentEnd) - percent(mark.at)),
    }
  })
  const spans: FcpSpan[] = []
  const spanOf = (name: FcpSpan['name'], start: unknown): void => {
    if (!finite(start)) return
    spans.push({
      name,
      duration: metricOf(entry, name)?.duration ?? end - start,
      left: percent(start),
      width: Math.max(0, percent(end) - percent(start)),
    })
  }
  spanOf('totalFcp', entry.openTime)
  spanOf('fcp', entry.prepareTemplateStart)
  spanOf('lynxFcp', entry.loadBundleStart)
  return { axis: { start: axisStart, end: axisEnd }, segments, spans }
}

/** The entries of one page load, from its load entry to the next load's. */
export interface PerformanceLoad {
  /** What the page called the load: its URL or a short name. */
  label: string
  /** The `loadBundle` or `reloadBundleFromNative` entry, once it arrived. */
  load: LynxPipelineEntry | undefined
  /** The newest `first-paint` and `first-contentful-paint`. */
  paints: LynxPaintEntry[]
  /** Every pipeline entry, the load's included, in arrival order. */
  pipelines: LynxPipelineEntry[]
  /** Pipeline entries this load received, the dropped ones included. */
  pipelineCount: number
  /** The oldest pipeline entries the row cap dropped. */
  dropped: number
}

/**
 * Groups entries by page load. A load begins when the page says so
 * (`begin`) or when a load entry arrives for a group that already has a
 * different one. A load entry that repeats the group's own — the engine
 * re-sends it once the host's marks fill `fcp` and `totalFcp` — replaces it
 * in place. Entries that are neither pipelines nor paints are ignored.
 */
export class PerformanceTimeline {
  readonly #rowCap: number
  readonly #loadCap: number
  readonly #loads: PerformanceLoad[] = []

  /**
   * `rowCap` pipeline entries are kept per load, the oldest dropped first;
   * `loadCap` loads are kept, the oldest dropped first.
   */
  constructor(rowCap = 200, loadCap = 20) {
    this.#rowCap = rowCap
    this.#loadCap = loadCap
  }

  get loads(): readonly PerformanceLoad[] {
    return this.#loads
  }

  get current(): PerformanceLoad | undefined {
    return this.#loads.at(-1)
  }

  /** Starts a new, empty load. */
  begin(label: string): PerformanceLoad {
    const load: PerformanceLoad = {
      label,
      load: undefined,
      paints: [],
      pipelines: [],
      pipelineCount: 0,
      dropped: 0,
    }
    this.#loads.push(load)
    if (this.#loads.length > this.#loadCap) {
      this.#loads.splice(0, this.#loads.length - this.#loadCap)
    }
    return load
  }

  /** Files `entry` under its load and answers that load. */
  add(entry: LynxPerformanceEntry): PerformanceLoad | undefined {
    if (isPaintEntry(entry)) {
      const load = this.current ?? this.begin(entry.name)
      const index = load.paints.findIndex((paint) => paint.name === entry.name)
      if (index === -1) {
        load.paints.push(entry)
      } else {
        load.paints[index] = entry
      }
      return load
    }
    if (!isPipelineEntry(entry)) {
      return undefined
    }
    let load = this.current ?? this.begin(entry.name)
    if (isLoadEntry(entry)) {
      const previous = load.load
      if (previous !== undefined && previous.pipelineStart === entry.pipelineStart) {
        load.load = entry
        const index = load.pipelines.indexOf(previous)
        if (index !== -1) {
          load.pipelines[index] = entry
        }
        return load
      }
      if (previous !== undefined) {
        load = this.begin(entry.name)
      }
      load.load = entry
    }
    load.pipelines.push(entry)
    load.pipelineCount += 1
    if (load.pipelines.length > this.#rowCap) {
      const excess = load.pipelines.length - this.#rowCap
      load.pipelines.splice(0, excess)
      load.dropped += excess
    }
    return load
  }
}
