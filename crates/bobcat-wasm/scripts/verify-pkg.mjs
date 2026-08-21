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
  'export function wasm_thread_entry_point',
]) {
  if (!glue.includes(requiredExport)) {
    throw new Error(`generated glue is missing ${requiredExport}`)
  }
}
for (const requiredMethod of [
  'registerScript(',
  'registerStyleSheet(',
  'registerLynxXml(',
  'executeScript(',
  'loadStyleSheet(',
  'pollScript(',
  'registerFonts(',
  'reset(',
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
  'finishBrowserScriptCheckpoint',
  'initThreadPool',
  'parallelChecksum',
  'pollDomCommand',
  'pollResponse',
  'scriptStarted',
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
  'LYNX_XML_PAGE_CONFIG: Readonly<PageConfig>',
  'executeScript(url: string | URL)',
  'loadStyleSheet(url: string | URL)',
  'loadLynxXml(url: string | URL)',
  'registerFonts(data: ArrayBuffer | Uint8Array)',
  'reset(): Promise<void>',
]) {
  if (!declarations.includes(requiredDeclaration)) {
    throw new Error(`browser declarations are missing ${requiredDeclaration}`)
  }
}
const lynxXmlConfigStart = facade.indexOf('export const LYNX_XML_PAGE_CONFIG')
const lynxXmlConfigEnd = facade.indexOf('function preferredThreadCount')
if (lynxXmlConfigStart === -1 || lynxXmlConfigEnd === -1) {
  throw new Error('browser facade is missing LYNX_XML_PAGE_CONFIG')
}
const lynxXmlConfig = facade.slice(lynxXmlConfigStart, lynxXmlConfigEnd)
for (const requiredConfigValue of [
  'defaultDisplayLinear: false',
  'defaultOverflowVisible: false',
  'enableCSSSelector: true',
]) {
  if (!lynxXmlConfig.includes(requiredConfigValue)) {
    throw new Error(`LYNX_XML_PAGE_CONFIG is missing ${requiredConfigValue}`)
  }
}
if (
  !facade.includes('async loadLynxXml(url)') ||
  !facade.includes("this.#request('loadLynxXml'")
) {
  throw new Error('browser facade does not dispatch loadLynxXml')
}
if (
  !facade.includes('async reset()') ||
  !facade.includes("this.#request('reset')")
) {
  throw new Error('browser facade does not dispatch native-view reset')
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
if (
  !renderWorker.includes('function ensureEntryScriptNotStarted()') ||
  !renderWorker.includes('if (entryScriptStarted)')
) {
  throw new Error('Render Worker is missing its one-shot entry-script guard')
}
const executeScriptDispatch = renderWorker.slice(
  renderWorker.indexOf("case 'executeScript':"),
  renderWorker.indexOf("case 'loadStyleSheet':"),
)
for (const requiredEntryGuardStep of [
  'ensureEntryScriptNotStarted()',
  'entryScriptStarted = true',
]) {
  if (!executeScriptDispatch.includes(requiredEntryGuardStep)) {
    throw new Error(
      `Render Worker script dispatch is missing ${requiredEntryGuardStep}`,
    )
  }
}
if (!renderWorker.includes('await renderer.loadStyleSheet(registeredUrl)')) {
  throw new Error(
    'Render Worker must register fetched stylesheet bytes before loading them',
  )
}
const lynxXmlDispatchStart = renderWorker.indexOf("case 'loadLynxXml':")
const lynxXmlDispatchEnd = renderWorker.indexOf("case 'registerFonts':")
if (lynxXmlDispatchStart === -1 || lynxXmlDispatchEnd === -1) {
  throw new Error('Render Worker is missing the Lynx XML dispatch case')
}
const lynxXmlDispatch = renderWorker.slice(
  lynxXmlDispatchStart,
  lynxXmlDispatchEnd,
)
for (const requiredLynxXmlLoaderStep of [
  'response.status !== 200',
  'await readBoundedBytes(response, MAX_LYNX_XML_BYTES)',
  'new TextDecoder().decode(bytes)',
  'url: response.url || requestedUrl',
]) {
  if (!renderWorker.includes(requiredLynxXmlLoaderStep)) {
    throw new Error(
      `Render Worker Lynx XML loader is missing ${requiredLynxXmlLoaderStep}`,
    )
  }
}
for (const requiredLynxXmlDispatchStep of [
  'ensureEntryScriptNotStarted()',
  'renderer.registerLynxXml(url, source)',
  'await renderer.loadStyleSheet(styleSheetUrl)',
  'console.warn(',
  'await renderer.executeScript(mainThreadScriptUrl)',
  'entryScriptStarted = true',
  'trackScriptCompletion(request)',
]) {
  if (!lynxXmlDispatch.includes(requiredLynxXmlDispatchStep)) {
    throw new Error(
      `Render Worker Lynx XML dispatch is missing ${requiredLynxXmlDispatchStep}`,
    )
  }
}
if (
  lynxXmlDispatch.indexOf('ensureEntryScriptNotStarted()') >
  lynxXmlDispatch.indexOf('await fetchLynxXml(message.url)')
) {
  throw new Error('Render Worker must reject repeated Lynx XML loads before fetch')
}
if (
  lynxXmlDispatch.indexOf('await renderer.loadStyleSheet(styleSheetUrl)') >
  lynxXmlDispatch.indexOf('await renderer.executeScript(mainThreadScriptUrl)')
) {
  throw new Error('Render Worker must load Lynx XML styles before its main script')
}
if (renderWorker.includes('DOMParser')) {
  throw new Error('Render Worker must leave Lynx XML parsing to Rust')
}
const resetDispatchStart = renderWorker.indexOf("case 'reset':")
const resetDispatchEnd = renderWorker.indexOf("case 'registerFonts':")
if (resetDispatchStart === -1 || resetDispatchEnd === -1) {
  throw new Error('Render Worker is missing the native-view reset case')
}
const resetDispatch = renderWorker.slice(resetDispatchStart, resetDispatchEnd)
for (const requiredResetStep of [
  'await scriptCompletion',
  'entryScriptStarted = false',
  'resettingNativeView = true',
  'await renderer.reset()',
  'resettingNativeView = false',
]) {
  if (!resetDispatch.includes(requiredResetStep)) {
    throw new Error(`Render Worker reset is missing ${requiredResetStep}`)
  }
}
if (!renderWorker.includes('requestQueue = requestQueue.then(dispatch)')) {
  throw new Error('Render Worker must serialize every facade operation')
}
if (renderWorker.includes('setTimeout(resolve, 1)')) {
  throw new Error('Render Worker still polls script completion on a timer')
}
if (!renderWorker.includes('await renderer.waitForEngineEvent()')) {
  throw new Error('Render Worker must await core engine events')
}
if (
  renderWorker.includes('SCRIPT_START_TIMEOUT_MS') ||
  renderWorker.includes('renderer.scriptStarted()')
) {
  throw new Error('Render Worker still applies a VM startup deadline')
}
if (
  !domWorker.includes('wasm_thread_entry_point(work)') ||
  !domWorker.includes('self.close()')
) {
  throw new Error('DOM Worker must run the wasm_thread entry point and close')
}
for (const removedCheckpointProtocol of [
  'finishBrowserScriptCheckpoint',
  'nativePostMessage',
  'setTimeout(',
]) {
  if (domWorker.includes(removedCheckpointProtocol)) {
    throw new Error(
      `DOM Worker still contains browser checkpoint protocol ${removedCheckpointProtocol}`,
    )
  }
}
if (!facade.includes('document.baseURI')) {
  throw new Error('browser facade must resolve relative URLs against document.baseURI')
}
if (facade.includes('REQUEST_TIMEOUT_MS')) {
  throw new Error('browser facade still applies a Render Worker readiness deadline')
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
  !wasmBytes.includes(
    Buffer.from('QuickJS could not allocate a runtime', 'utf8'),
  )
) {
  throw new Error('release Wasm does not contain the built-in QuickJS runtime')
}
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
if (WebAssembly.Module.customSections(module, 'name').length !== 0) {
  throw new Error('release Wasm still contains a debugging name section')
}
if (WebAssembly.Module.customSections(module, 'target_features').length === 0) {
  throw new Error('release Wasm lost its target_features custom section')
}
const imports = WebAssembly.Module.imports(module)
if (!imports.some(({ kind }) => kind === 'memory')) {
  throw new Error('WebAssembly memory is not imported')
}
if (!WebAssembly.Module.exports(module).some(({ kind }) => kind === 'memory')) {
  throw new Error('WebAssembly memory is not exported')
}
const forbiddenCImports = new Set([
  'atanh',
  'calloc',
  'fprintf',
  'free',
  'lrint',
  'malloc',
  'memchr',
  'printf',
  'realloc',
  'snprintf',
  'strchr',
  'strcmp',
  'strrchr',
  'vsnprintf',
])
const forbiddenImports = imports.filter(
  ({ module: importModule, name }) =>
    importModule.startsWith('wasi_') ||
    (importModule === 'env' && forbiddenCImports.has(name)),
)
if (forbiddenImports.length !== 0) {
  throw new Error(
    `QuickJS Wasm has forbidden platform imports: ${JSON.stringify(forbiddenImports)}`,
  )
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
