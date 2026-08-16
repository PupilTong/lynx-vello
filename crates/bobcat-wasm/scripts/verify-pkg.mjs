import { execFileSync } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const packageDirectory = fileURLToPath(new URL('..', import.meta.url))
const gluePath = path.join(packageDirectory, 'pkg/bobcat_wasm.js')
const wasmPath = path.join(packageDirectory, 'pkg/bobcat_wasm_bg.wasm')

for (const script of [
  path.join(packageDirectory, 'dom-worker.js'),
  path.join(packageDirectory, 'facade.js'),
  path.join(packageDirectory, 'render-worker.js'),
  gluePath,
]) {
  execFileSync(process.execPath, ['--check', script], { stdio: 'inherit' })
}

const glue = await readFile(gluePath, 'utf8')
if (!/new WebAssembly\.Memory\(\{[^}]*shared:\s*true/.test(glue)) {
  throw new Error('generated glue does not construct shared WebAssembly memory')
}
for (const requiredExport of [
  'export class BobcatRenderer',
  'export function finishBrowserScriptCheckpoint',
  'export function wasm_thread_entry_point',
]) {
  if (!glue.includes(requiredExport)) {
    throw new Error(`generated glue is missing ${requiredExport}`)
  }
}
for (const requiredMethod of [
  'registerScript(',
  'registerStyleSheet(',
  'executeScript(',
  'loadStyleSheet(',
  'pollScript(',
  'registerFonts(',
  'scriptStarted(',
  'waitForEngineEvent(',
]) {
  if (!glue.includes(requiredMethod)) {
    throw new Error(`generated renderer is missing ${requiredMethod}`)
  }
}
if (!glue.includes('passArray8ToWasm0(bytes')) {
  throw new Error('generated script registry must accept raw Uint8Array bytes')
}
for (const removedExport of [
  'createBrowserSession',
  'initThreadPool',
  'parallelChecksum',
  'pollDomCommand',
  'pollResponse',
  'waitForResponse',
  'wasmMemory',
]) {
  if (glue.includes(removedExport)) {
    throw new Error(`generated glue still exposes removed ${removedExport}`)
  }
}

const facade = await readFile(
  path.join(packageDirectory, 'facade.js'),
  'utf8',
)
const declarations = await readFile(
  path.join(packageDirectory, 'facade.d.ts'),
  'utf8',
)
if (facade.includes('./pkg/bobcat_wasm.js')) {
  throw new Error('browser UI facade must not instantiate the Wasm module')
}
for (const requiredDeclaration of [
  'pageConfig: PageConfig',
  'executeScript(url: string | URL)',
  'loadStyleSheet(url: string | URL)',
  'registerFonts(data: ArrayBuffer | Uint8Array)',
]) {
  if (!declarations.includes(requiredDeclaration)) {
    throw new Error(`browser declarations are missing ${requiredDeclaration}`)
  }
}

const renderWorker = await readFile(
  path.join(packageDirectory, 'render-worker.js'),
  'utf8',
)
const domWorker = await readFile(
  path.join(packageDirectory, 'dom-worker.js'),
  'utf8',
)
const engineConstruction = renderWorker.indexOf('await BobcatRenderer.create(')
if (engineConstruction === -1 || !renderWorker.includes('message.threadCount')) {
  throw new Error('Render Worker must pass the style thread count to view construction')
}
if (renderWorker.includes('initThreadPool')) {
  throw new Error('Render Worker still initializes wasm-bindgen-rayon')
}
if (!renderWorker.includes('await renderer.executeScript(registeredUrl)')) {
  throw new Error('Render Worker must route fetched URLs through executeScript')
}
if (!renderWorker.includes('await renderer.loadStyleSheet(registeredUrl)')) {
  throw new Error(
    'Render Worker must register fetched stylesheet bytes before loading them',
  )
}
if (renderWorker.includes('setTimeout(resolve, 1)')) {
  throw new Error('Render Worker still polls script completion on a timer')
}
if (!renderWorker.includes('await renderer.waitForEngineEvent()')) {
  throw new Error('Render Worker must await core engine events')
}
if (!domWorker.includes('finishBrowserScriptCheckpoint()')) {
  throw new Error('DOM Worker must release browser host callbacks before closing')
}
if (!facade.includes('document.baseURI')) {
  throw new Error('browser facade must resolve relative URLs against document.baseURI')
}
if (renderWorker.includes('self.location.href')) {
  throw new Error('Render Worker must not resolve resource URLs against its own package URL')
}
if (
  !renderWorker.includes('await response.arrayBuffer()') ||
  renderWorker.includes('await response.text()')
) {
  throw new Error('Render Worker must preserve raw script bytes for core UTF-8 validation')
}
for (const forbiddenDomApi of [
  'addAuthorStylesheet',
  'appendElement',
  'createPage',
  'createView',
  'dropElement',
  'flushElementTree',
]) {
  if (
    facade.includes(forbiddenDomApi) ||
    declarations.includes(forbiddenDomApi) ||
    renderWorker.includes(forbiddenDomApi)
  ) {
    throw new Error(`browser facade still exposes direct DOM API ${forbiddenDomApi}`)
  }
}

const wasmBytes = await readFile(wasmPath)
if (
  wasmBytes.includes(
    Buffer.from('Parking not supported on this platform', 'utf8'),
  )
) {
  throw new Error(
    'parking_lot_core selected its non-atomic Wasm parker; enable its nightly feature',
  )
}
const module = new WebAssembly.Module(wasmBytes)
if (!WebAssembly.Module.imports(module).some(({ kind }) => kind === 'memory')) {
  throw new Error('WebAssembly memory is not imported')
}
if (!WebAssembly.Module.exports(module).some(({ kind }) => kind === 'memory')) {
  throw new Error('WebAssembly memory is not exported')
}

const pack = JSON.parse(
  execFileSync('npm', ['pack', '--dry-run', '--json'], {
    cwd: packageDirectory,
    encoding: 'utf8',
    env: {
      ...process.env,
      npm_config_cache: path.join(os.tmpdir(), 'bobcat-wasm-npm-cache'),
    },
  }),
)[0]
const packed = new Set(pack.files.map(({ path: packedPath }) => packedPath))
const required = [
  'dom-worker.js',
  'facade.d.ts',
  'facade.js',
  'pkg/bobcat_wasm.js',
  'pkg/bobcat_wasm_bg.wasm',
  'render-worker.js',
]
for (const requiredPath of required) {
  if (!packed.has(requiredPath)) {
    throw new Error(`npm package is missing ${requiredPath}`)
  }
}
if ([...packed].some((packedPath) => packedPath.includes('wasm-bindgen-rayon'))) {
  throw new Error('npm package still contains wasm-bindgen-rayon artifacts')
}

console.log(
  `verified shared-memory Wasm and ${String(pack.entryCount)} npm package files`,
)
