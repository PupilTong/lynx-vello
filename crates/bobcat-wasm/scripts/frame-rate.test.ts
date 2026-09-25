import assert from 'node:assert/strict'
import test from 'node:test'

import { monitorFrameRate } from '../js/frame-rate.ts'

function clock() {
  let callback: ((timestamp: number) => void) | undefined
  const reports: number[] = []
  const stop = monitorFrameRate(
    (next) => { callback = next; return 1 },
    () => { callback = undefined },
    (fps) => reports.push(fps),
  )
  return {
    reports,
    stop,
    tick(timestamp: number) {
      const next = callback
      callback = undefined
      next?.(timestamp)
    },
  }
}

test('reports elapsed rAF cadence, including dropped frames, twice a second', () => {
  const timer = clock()
  timer.tick(0)
  for (let time = 20; time < 500; time += 20) timer.tick(time)
  assert.deepEqual(timer.reports, [])
  timer.tick(500)
  assert.deepEqual(timer.reports, [50])
  for (let time = 550; time <= 1000; time += 50) timer.tick(time)
  assert.deepEqual(timer.reports, [50, 20])
})

test('starts a fresh sample after rAF resumes and cancels on disposal', () => {
  const timer = clock()
  timer.tick(0)
  timer.tick(20)
  timer.tick(5000)
  for (let time = 5020; time <= 5500; time += 20) timer.tick(time)
  assert.deepEqual(timer.reports, [50])
  timer.stop()
  timer.tick(6000)
  assert.deepEqual(timer.reports, [50])
})
