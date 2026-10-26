import assert from 'node:assert/strict'
import test from 'node:test'

import {
  PerformanceTimeline,
  isLoadEntry,
  isPaintEntry,
  isPipelineEntry,
  parsePerformanceEntry,
  pipelineDuration,
  pipelineStages,
  pipelineWaterfall,
  type LynxPaintEntry,
  type LynxPerformanceEntry,
  type LynxPipelineEntry,
} from '../js/performance.ts'

// The shape `PerformanceEntry::write_lynx_json` writes for a load: Lynx's
// keys in its order, metrics and the two nested objects as objects.
const LOAD_JSON = JSON.stringify({
  entryType: 'pipeline',
  name: 'loadBundle',
  identifier: '',
  pipelineStart: 1000.0,
  pipelineEnd: 1100.0,
  mtsRenderStart: 1020.0,
  mtsRenderEnd: 1040.0,
  resolveStart: 1050.0,
  resolveEnd: 1060.0,
  layoutStart: 1060.0,
  layoutEnd: 1075.0,
  paintingUiOperationExecuteStart: 1075.0,
  paintingUiOperationExecuteEnd: 1090.0,
  layoutUiOperationExecuteStart: 1092.0,
  layoutUiOperationExecuteEnd: 1098.0,
  paintEnd: 1100.0,
  loadBundleStart: 1010.0,
  loadBundleEnd: 1080.0,
  loadBackgroundStart: 1030.0,
  loadBackgroundEnd: 1070.0,
  createLynxStart: 990.0,
  createLynxEnd: 1005.0,
  lynxFcp: {
    name: 'lynxFcp',
    duration: 90.0,
    startTimestampName: 'loadBundleStart',
    startTimestamp: 1010.0,
    endTimestampName: 'paintEnd',
    endTimestamp: 1100.0,
  },
  hostPlatformTiming: {
    hostPlatformType: 'vello',
    composeStart: 1092.0,
    composeEnd: 1098.0,
  },
})

function load(overrides: Partial<LynxPipelineEntry> = {}): LynxPipelineEntry {
  const entry = parsePerformanceEntry(LOAD_JSON)
  assert.ok(entry !== undefined && isPipelineEntry(entry))
  return { ...entry, ...overrides }
}

function pipeline(identifier: string, start: number): LynxPipelineEntry {
  return {
    entryType: 'pipeline',
    name: 'updateTriggeredByBts',
    identifier,
    pipelineStart: start,
    pipelineEnd: start + 10,
    resolveStart: start + 1,
    resolveEnd: start + 2,
    layoutStart: start + 2,
    layoutEnd: start + 4,
    paintingUiOperationExecuteStart: start + 4,
    paintingUiOperationExecuteEnd: start + 6,
    layoutUiOperationExecuteStart: start + 7,
    layoutUiOperationExecuteEnd: start + 9,
    paintEnd: start + 10,
  }
}

function paint(name: string, startTime: number): LynxPaintEntry {
  return { entryType: 'paint', name, startTime }
}

test('parses the engine JSON and refuses text that is not an entry', () => {
  const entry = parsePerformanceEntry(LOAD_JSON)
  assert.ok(entry !== undefined)
  assert.equal(entry.entryType, 'pipeline')
  assert.equal(entry.name, 'loadBundle')
  assert.ok(isPipelineEntry(entry))
  assert.ok(isLoadEntry(entry))
  assert.equal((entry.lynxFcp as { duration: number }).duration, 90)

  const first = parsePerformanceEntry('{"entryType":"paint","name":"first-paint","startTime":1100.000}')
  assert.ok(first !== undefined && isPaintEntry(first))

  assert.equal(parsePerformanceEntry('not json'), undefined)
  assert.equal(parsePerformanceEntry('[]'), undefined)
  assert.equal(parsePerformanceEntry('{"name":"x"}'), undefined)
  assert.equal(parsePerformanceEntry('{"entryType":"pipeline","name":1}'), undefined)
})

test('lists the stage pairs present, ordered by start, and skips missing ones', () => {
  const stages = pipelineStages(load())
  assert.deepEqual(
    stages.map((stage) => stage.name),
    [
      'loadBundle',
      'mtsRender',
      'loadBackground',
      'resolve',
      'layout',
      'paintingUiOperationExecute',
      'layoutUiOperationExecute',
    ],
  )
  // `parseStart`/`parseEnd` are absent: no bar, not a zero-width one.
  assert.ok(!stages.some((stage) => stage.name === 'parse'))
  // Half a pair is no stage either.
  const half = load({ parseStart: 1012 })
  assert.ok(!pipelineStages(half).some((stage) => stage.name === 'parse'))
  assert.equal(pipelineDuration(load()), 100)
})

test('lays a load out from loadBundleStart to paintEnd, with paint markers', () => {
  const { axis, bars, markers } = pipelineWaterfall(load(), [
    paint('first-paint', 1100),
    paint('first-contentful-paint', 1100),
  ])
  assert.deepEqual(axis, { start: 1010, end: 1100 })
  const loadBundle = bars.find((bar) => bar.name === 'loadBundle')!
  assert.equal(loadBundle.left, 0)
  assert.equal(loadBundle.duration, 70)
  assert.ok(Math.abs(loadBundle.width - (70 / 90) * 100) < 1e-9)
  const layout = bars.find((bar) => bar.name === 'layout')!
  assert.ok(Math.abs(layout.left - (50 / 90) * 100) < 1e-9)
  assert.deepEqual(
    markers.map((marker) => [marker.name, marker.left]),
    [
      ['first-paint', 100],
      ['first-contentful-paint', 100],
    ],
  )
})

test('widens the axis for a paint mark past paintEnd and aligns a pipeline to its start', () => {
  const late = pipelineWaterfall(load(), [paint('first-contentful-paint', 1190)])
  assert.deepEqual(late.axis, { start: 1010, end: 1190 })
  assert.equal(late.markers[0]!.left, 100)

  const update = pipelineWaterfall(pipeline('tap', 5000))
  assert.deepEqual(update.axis, { start: 5000, end: 5010 })
  assert.equal(update.bars[0]!.name, 'resolve')
  assert.ok(Math.abs(update.bars[0]!.left - 10) < 1e-9)
})

test('groups entries by load and replaces a re-sent load entry in place', () => {
  const timeline = new PerformanceTimeline()
  timeline.begin('homepage')
  timeline.add(paint('first-paint', 1100))
  timeline.add(load())
  timeline.add(pipeline('tap', 2000))
  assert.equal(timeline.loads.length, 1)
  const homepage = timeline.current!
  assert.equal(homepage.label, 'homepage')
  assert.equal(homepage.pipelineCount, 2)
  assert.equal(homepage.paints.length, 1)

  // The engine re-sends the load once the host's marks fill `fcp`.
  const filled = load({
    fcp: {
      name: 'fcp',
      duration: 95,
      startTimestampName: 'prepareTemplateStart',
      startTimestamp: 1005,
      endTimestampName: 'paintEnd',
      endTimestamp: 1100,
    },
  })
  timeline.add(filled)
  assert.equal(timeline.loads.length, 1)
  assert.equal(homepage.load, filled)
  assert.equal(homepage.pipelines[0], filled)
  assert.equal(homepage.pipelineCount, 2)

  // A different load starts a group of its own, even without `begin`.
  timeline.add(load({ pipelineStart: 9000 }))
  assert.equal(timeline.loads.length, 2)
  assert.equal(timeline.current!.label, 'loadBundle')
  assert.equal(timeline.loads[0], homepage)

  // A group the page began takes the load entry that follows it.
  timeline.begin('showcase')
  timeline.add(load({ pipelineStart: 12000 }))
  assert.equal(timeline.loads.length, 3)
  assert.equal(timeline.current!.label, 'showcase')
  assert.equal(timeline.current!.load!.pipelineStart, 12000)
})

test('caps the rows of a load and the number of loads kept', () => {
  const timeline = new PerformanceTimeline(3, 2)
  timeline.begin('one')
  for (let index = 0; index < 5; index += 1) {
    timeline.add(pipeline(`flag-${String(index)}`, index * 100))
  }
  const one = timeline.current!
  assert.equal(one.pipelineCount, 5)
  assert.equal(one.dropped, 2)
  assert.deepEqual(
    one.pipelines.map((entry) => entry.identifier),
    ['flag-2', 'flag-3', 'flag-4'],
  )
  timeline.begin('two')
  timeline.begin('three')
  assert.deepEqual(
    timeline.loads.map((entry) => entry.label),
    ['two', 'three'],
  )
  const other: LynxPerformanceEntry = { entryType: 'resource', name: 'x' }
  assert.equal(timeline.add(other), undefined)
})
