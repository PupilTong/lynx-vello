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
  path.join(packageDirectory, 'image-worker.js'),
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
  'dispatchPointer(',
  'registerScript(',
  'registerStyleSheet(',
  'registerLynxXml(',
  'bobcatrenderer_load(',
  'pump(',
  'registerFonts(',
  'setDefaultFontFamily(',
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
  'bobcatrenderer_executeScript',
  'bobcatrenderer_loadStyleSheet',
  'bobcatrenderer_reset',
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
  'load(url: string | URL, styleSheetUrls?: readonly (string | URL)[])',
  'loadLynxXml(url: string | URL)',
  'registerFonts(data: ArrayBuffer | Uint8Array)',
  'setDefaultFontFamily(family: string)',
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
for (const requiredPointerStep of [
  "canvas.addEventListener('pointerdown'",
  "canvas.addEventListener('pointermove'",
  "canvas.addEventListener('pointerup'",
  "canvas.addEventListener('pointercancel'",
  'canvas.setPointerCapture(event.pointerId)',
  'getBoundingClientRect()',
  "type: 'bobcat-pointer'",
]) {
  if (!facade.includes(requiredPointerStep)) {
    throw new Error(
      `browser facade pointer bridge is missing ${requiredPointerStep}`,
    )
  }
}
if (declarations.includes('dispatchPointer')) {
  throw new Error('browser declarations expose the private pointer bridge')
}
for (const [operation, method, message] of [
  ['load', 'async load(url, styleSheetUrls = [])', 'a page load'],
  [
    'setDefaultFontFamily',
    'async setDefaultFontFamily(family)',
    'setDefaultFontFamily',
  ],
]) {
  if (
    !facade.includes(method) ||
    !facade.includes(`this.#request('${operation}'`)
  ) {
    throw new Error(`browser facade does not dispatch ${message}`)
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
// A view is its page, so every load registers its sources first and then
// builds one native view from them.
const loadDispatch = renderWorker.slice(
  renderWorker.indexOf("case 'load': {"),
  renderWorker.indexOf("case 'loadLynxXml':"),
)
if (loadDispatch === '') {
  throw new Error('Render Worker is missing the page-load dispatch case')
}
for (const requiredLoadStep of [
  "await fetchSource('stylesheet', url, MAX_STYLE_SHEET_BYTES)",
  "await fetchSource('script', message.url, MAX_SCRIPT_BYTES)",
  'renderer.registerStyleSheet(sheet.url, sheet.bytes)',
  'renderer.registerScript(entry.url, entry.bytes)',
  'await replaceNativeView(request, entryUrl, styleSheetUrls)',
]) {
  if (!loadDispatch.includes(requiredLoadStep)) {
    throw new Error(`Render Worker page load is missing ${requiredLoadStep}`)
  }
}
// Registration is what a failed load would leave behind, so every fetch must
// complete before the first one.
if (
  loadDispatch.indexOf("await fetchSource('script'") >
  loadDispatch.indexOf('renderer.registerStyleSheet(')
) {
  throw new Error('Render Worker must fetch every page source before registering any')
}
if (
  !renderWorker.includes("case 'setDefaultFontFamily':") ||
  !renderWorker.includes('renderer.setDefaultFontFamily(message.family)')
) {
  throw new Error('Render Worker is missing the default-font dispatch case')
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
  'renderer.registerLynxXml(url, source)',
  'console.warn(',
  'await replaceNativeView(',
  'styleSheetUrl === null ? [] : [styleSheetUrl]',
]) {
  if (!lynxXmlDispatch.includes(requiredLynxXmlDispatchStep)) {
    throw new Error(
      `Render Worker Lynx XML dispatch is missing ${requiredLynxXmlDispatchStep}`,
    )
  }
}
if (
  lynxXmlDispatch.indexOf('renderer.registerLynxXml(url, source)') >
  lynxXmlDispatch.indexOf('await replaceNativeView(')
) {
  throw new Error('Render Worker must register Lynx XML sections before it loads them')
}
if (renderWorker.includes('DOMParser')) {
  throw new Error('Render Worker must leave Lynx XML parsing to Rust')
}
const replaceStart = renderWorker.indexOf('async function replaceNativeView(')
if (replaceStart === -1) {
  throw new Error('Render Worker is missing its native-view replacement step')
}
const replaceView = renderWorker.slice(
  replaceStart,
  renderWorker.indexOf('async function dispatchRequest('),
)
for (const requiredReplaceStep of [
  'await scriptCompletion',
  'engineEventGeneration += 1',
  'await renderer.load(entryUrl, styleSheetUrls)',
  'trackScriptCompletion(request)',
]) {
  if (!replaceView.includes(requiredReplaceStep)) {
    throw new Error(
      `Render Worker native-view replacement is missing ${requiredReplaceStep}`,
    )
  }
}
if (!renderWorker.includes('requestQueue = requestQueue.then(dispatch)')) {
  throw new Error('Render Worker must serialize every facade operation')
}
for (const requiredPointerStep of [
  "message?.type === 'bobcat-pointer'",
  'renderer.dispatchPointer(',
  'message.defaultPrevented',
]) {
  if (!renderWorker.includes(requiredPointerStep)) {
    throw new Error(
      `Render Worker pointer dispatch is missing ${requiredPointerStep}`,
    )
  }
}
if (renderWorker.includes('setTimeout(resolve, 1)')) {
  throw new Error('Render Worker still polls script completion on a timer')
}
if (!renderWorker.includes('await renderer.waitForEngineEvent()')) {
  throw new Error('Render Worker must await core engine events')
}
for (const requiredServeStep of [
  '.catch(() => undefined)',
  '.then(() => servePage(generation))',
]) {
  if (!renderWorker.includes(requiredServeStep)) {
    throw new Error(
      `Render Worker must serve every page's wakeups regardless of boot outcome: missing ${requiredServeStep}`,
    )
  }
}
if (renderWorker.includes('renderIfRequested')) {
  throw new Error('Render Worker still drives frames through renderIfRequested')
}
// The frame clock is the continuation's alone: a commit must reach the canvas
// through the engine wakeup, never by waiting for a display frame.
if (!/if \(renderer\.owesFrame\(\)\) \{\s*await nextDisplayFrame\(\)/.test(renderWorker)) {
  throw new Error(
    'Render Worker must wait for a display frame only while the view owes one',
  )
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
