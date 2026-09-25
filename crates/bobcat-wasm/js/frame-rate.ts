/** Samples the owning thread's rAF without driving the renderer's pump. */
export function monitorFrameRate(
  request: (callback: (timestamp: number) => void) => number,
  cancel: (id: number) => void,
  report: (fps: number) => void,
): () => void {
  let start: number | undefined
  let previous: number | undefined
  let frames = 0
  let stopped = false
  let id: number
  const sample = (timestamp: number): void => {
    if (stopped) return
    // A hidden tab can suspend rAF. Start a fresh window when it returns.
    if (start === undefined || (previous !== undefined && timestamp - previous > 2000)) {
      start = timestamp
      frames = 0
    } else {
      frames += 1
      const elapsed = timestamp - start
      if (elapsed >= 500) {
        report(frames * 1000 / elapsed)
        start = timestamp
        frames = 0
      }
    }
    previous = timestamp
    if (!stopped) id = request(sample)
  }
  id = request(sample)
  return () => {
    stopped = true
    cancel(id)
  }
}
