import bindings, {
  createBobcatCanvas,
  parallelChecksum as runParallelChecksum,
} from './bobcat_wasm_threads.wasi-browser.js'

const MAX_THREADS = 3

export function parallelChecksum(bytes, threads) {
  const hardwareThreads = Math.max(
    2,
    globalThis.navigator?.hardwareConcurrency ?? 2,
  )
  const requestedThreads = Number.isFinite(threads) ? Math.trunc(threads) : 1
  const workerCapacity = Math.max(1, hardwareThreads - 1)

  return runParallelChecksum(
    bytes,
    Math.max(1, Math.min(MAX_THREADS, workerCapacity, requestedThreads)),
  )
}

const NativeBobcatCanvas = bindings.BobcatCanvas

if (typeof NativeBobcatCanvas !== 'function') {
  throw new TypeError('The WASI addon did not export BobcatCanvas')
}

Object.defineProperty(NativeBobcatCanvas, 'create', {
  configurable: false,
  enumerable: false,
  value: createBobcatCanvas,
  writable: false,
})

export const BobcatCanvas = NativeBobcatCanvas

// Importing the NAPI-RS browser loader instantiates the module with its shared
// memory. Keep a no-op default for the same ergonomic API wasm-pack exposes.
export default async function init() {}
