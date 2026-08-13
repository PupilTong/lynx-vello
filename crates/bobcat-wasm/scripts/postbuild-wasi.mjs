import { readFileSync, writeFileSync } from 'node:fs'

const loaderUrl = new URL(
  '../bobcat_wasm_threads.wasi-browser.js',
  import.meta.url,
)
const generated = readFileSync(loaderUrl, 'utf8')
const original = `const __workerPoolSize = Math.max(
  2,
  globalThis.navigator?.hardwareConcurrency ?? 4,
)`
const capped = `const __workerPoolSize = Math.max(
  2,
  Math.min(3, globalThis.navigator?.hardwareConcurrency ?? 4),
)`

if (
  !generated.includes(original) ||
  generated.indexOf(original) !== generated.lastIndexOf(original)
) {
  throw new Error(
    'The NAPI-RS browser loader worker-pool template changed; update the checked postbuild transform.',
  )
}

writeFileSync(loaderUrl, generated.replace(original, capped))
