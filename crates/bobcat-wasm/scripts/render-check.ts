// A rendering check for the browser target: a Lynx XML card with a still
// image and three animated ones, drawn by the threaded Wasm build in a real
// Chrome with WebGPU, screenshotted, and asserted on pixel by pixel.
//
//   pnpm --filter bobcat-wasm build
//   node crates/bobcat-wasm/scripts/render-check.ts [--package <dir>] [--chrome <path>] [--out <dir>] [--repeat <n>] [--console]
//
// `--package` serves another build's `pkg/` and `dist/` (the default is this
// package's own); `--out` keeps the screenshots; `--repeat` runs the check
// that many times, each in a fresh browser, and fails if any run fails;
// `--console` prints the console of the page and its Workers, which a failed
// run prints anyway; `CHROME` or `--chrome`
// names the browser, by default Google Chrome where macOS installs it. The
// check is local: it needs a WebGPU adapter in headless Chrome, and CI's
// Ubuntu runner has no GPU.
//
// It uses nothing but Node and Chrome: a static server that sends the
// cross-origin isolation headers a threaded build needs, Chrome's own
// `--headless` and `--remote-debugging-port`, the DevTools protocol over
// Node's `WebSocket`, and a PNG reader over `node:zlib`.
//
// What it asserts:
// - `still.png`, 16 × 16, left half opaque orange, right half blue at 50 %
//   alpha, is drawn with the browser's colour and alpha handling: orange,
//   and the blue composited over the white page.
// - `rgb-twice.gif` (red, green, then blue over the left half; its file says
//   two plays) as a CSS `background-image`, a URL picture, ends on its last
//   frame, blue left and green right: frames decoded by index with earlier
//   frames composited under them, played to the count the file states.
// - The same file as an `<image loop-count="1">`, a picture of the element's
//   own, ends on its last frame too, after one play. An `<image>` without
//   `loop-count` loops forever whatever the file states (Lynx's rule,
//   `docs/picture-source-design.md` "Animated images"), so it never rests.
// - `fast-forever.gif` (white, then red; forever) in an `<image>` is seen in
//   both colours.
// - `rgb.mp4` (`bobcat-resources/tests/fixtures/video`: red, green, then
//   blue, 0.4 s each, H.264 in MP4) in a muted, looping `<video>` that the
//   card plays as it renders: a muted play needs no user gesture. The
//   browser's `<video>` on the page's thread plays it, and its frames reach
//   the Render Worker as `VideoFrame`s. Once its first frame is drawn, every
//   screenshot shows one of the clip's colours, and all three are seen.

import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { crc32, deflateSync, inflateSync } from 'node:zlib'

const here = path.dirname(fileURLToPath(import.meta.url))
const packageDirectory = path.resolve(here, '..')
const animatedFixtures = path.resolve(
  here,
  '../../bobcat-resources/tests/fixtures/animated',
)
const videoFixtures = path.resolve(here, '../../bobcat-resources/tests/fixtures/video')

const WIDTH = 560
const HEIGHT = 120

function argument(name: string): string | undefined {
  const index = process.argv.indexOf(name)
  return index === -1 ? undefined : process.argv[index + 1]
}

const served = path.resolve(argument('--package') ?? packageDirectory)
const chrome =
  argument('--chrome') ??
  process.env['CHROME'] ??
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
const out = argument('--out')

// ---- Fixtures ------------------------------------------------------------

function chunk(type: string, data: Buffer): Buffer {
  const length = Buffer.alloc(4)
  length.writeUInt32BE(data.length)
  const body = Buffer.concat([Buffer.from(type, 'latin1'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(body))
  return Buffer.concat([length, body, crc])
}

/** A 16 × 16 RGBA PNG: opaque orange on the left, half-transparent blue on the right. */
function stillPng(): Buffer {
  const size = 16
  const rows: Buffer[] = []
  for (let y = 0; y < size; y += 1) {
    const row = Buffer.alloc(1 + size * 4)
    for (let x = 0; x < size; x += 1) {
      const texel = x < size / 2 ? [255, 128, 0, 255] : [0, 0, 255, 128]
      row.set(texel, 1 + x * 4)
    }
    rows.push(row)
  }
  const header = Buffer.alloc(13)
  header.writeUInt32BE(size, 0)
  header.writeUInt32BE(size, 4)
  header.set([8, 6, 0, 0, 0], 8)
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(Buffer.concat(rows))),
    chunk('IEND', Buffer.alloc(0)),
  ])
}

const CARD = `<!doctype lynx>
<lynx engine-version="4.2">
<style>
.page {
  width: ${WIDTH}px;
  height: ${HEIGHT}px;
  background-color: #ffffff;
}
.cell {
  position: absolute;
  top: 10px;
  width: 100px;
  height: 100px;
}
.at-0 { left: 10px; }
.at-1 { left: 120px; }
.at-2 { left: 230px; }
.at-3 { left: 340px; }
.at-4 { left: 450px; }
.css-twice {
  background-image: url(rgb-twice.gif);
  background-size: 100% 100%;
}
</style>
<script thread="main">
const engine = lynx.getEngine();

Object.assign(globalThis, {
  processData(data) {
    return data;
  },
});

function renderPage() {
  const page = __CreatePage("0", 0);
  const pageId = __GetElementUniqueID(page);
  __SetClasses(page, "page");
  ["still.png", "fast-forever.gif"].forEach((src, index) => {
    const image = __CreateImage(pageId);
    __SetClasses(image, "cell at-" + index);
    __SetAttribute(image, "src", src);
    __AppendElement(page, image);
  });
  const css = __CreateView(pageId);
  __SetClasses(css, "cell at-2 css-twice");
  __AppendElement(page, css);
  const once = __CreateImage(pageId);
  __SetClasses(once, "cell at-3");
  __SetAttribute(once, "loop-count", "1");
  __SetAttribute(once, "src", "rgb-twice.gif");
  __AppendElement(page, once);
  const video = __CreateElement("video", pageId);
  __SetClasses(video, "cell at-4");
  __SetAttribute(video, "muted", "true");
  __SetAttribute(video, "loop", "true");
  __SetAttribute(video, "src", "rgb.mp4");
  __AppendElement(page, video);
  __InvokeUIMethod(video, "play", {}, () => {});
}

engine.addEventListener("__RenderPage", renderPage);
</script>
</lynx>
`

const PAGE = `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<style>
html, body { margin: 0; background: #ffffff; }
canvas { display: block; width: ${WIDTH}px; height: ${HEIGHT}px; }
</style>
</head>
<body>
<canvas id="canvas" width="${WIDTH}" height="${HEIGHT}"></canvas>
<script type="module">
import { BobcatCanvas, LYNX_XML_PAGE_CONFIG } from '/bobcat-wasm/dist/facade.js'
try {
  const canvas = document.getElementById('canvas')
  const view = await BobcatCanvas.create(canvas, ${WIDTH}, ${HEIGHT}, 1, LYNX_XML_PAGE_CONFIG)
  view.onerror = (error) => { window.renderCheck = { error: String(error) } }
  await view.loadLynxXml('/card.lynx.xml')
  window.renderCheck = { loaded: performance.now() }
} catch (error) {
  window.renderCheck = { error: String(error && error.stack || error) }
}
</script>
</body>
</html>
`

const TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.wasm': 'application/wasm',
  '.gif': 'image/gif',
  '.mp4': 'video/mp4',
  '.png': 'image/png',
  '.xml': 'application/xml; charset=utf-8',
}

function file(pathname: string): { body: Buffer; type: string } | undefined {
  const type = (name: string): string => TYPES[path.extname(name)] ?? 'application/octet-stream'
  if (pathname === '/' || pathname === '/index.html') {
    return { body: Buffer.from(PAGE), type: TYPES['.html']! }
  }
  if (pathname === '/card.lynx.xml') {
    return { body: Buffer.from(CARD), type: TYPES['.xml']! }
  }
  if (pathname === '/still.png') {
    return { body: stillPng(), type: TYPES['.png']! }
  }
  if (pathname === '/fast-forever.gif' || pathname === '/rgb-twice.gif') {
    return {
      body: readFileSync(path.join(animatedFixtures, pathname.slice(1))),
      type: TYPES['.gif']!,
    }
  }
  if (pathname === '/rgb.mp4') {
    return { body: readFileSync(path.join(videoFixtures, 'rgb.mp4')), type: TYPES['.mp4']! }
  }
  const match = /^\/bobcat-wasm\/(pkg|dist)\/([\w.-]+)$/.exec(pathname)
  if (match !== null) {
    try {
      return {
        body: readFileSync(path.join(served, match[1]!, match[2]!)),
        type: type(match[2]!),
      }
    } catch {
      return undefined
    }
  }
  return undefined
}

function serve(request: IncomingMessage, response: ServerResponse): void {
  const pathname = new URL(request.url ?? '/', 'http://localhost').pathname
  const found = file(pathname)
  // A threaded build needs a cross-origin-isolated page.
  response.setHeader('Cross-Origin-Opener-Policy', 'same-origin')
  response.setHeader('Cross-Origin-Embedder-Policy', 'require-corp')
  response.setHeader('Cross-Origin-Resource-Policy', 'same-origin')
  response.setHeader('Cache-Control', 'no-store')
  if (found === undefined) {
    response.writeHead(404).end()
    return
  }
  response.writeHead(200, { 'Content-Type': found.type }).end(found.body)
}

// ---- PNG -------------------------------------------------------------------

interface Image {
  width: number
  height: number
  rgba: Uint8Array
}

/** Reads an 8-bit RGB or RGBA, non-interlaced PNG: what Chrome captures. */
function readPng(png: Buffer): Image {
  let offset = 8
  let width = 0
  let height = 0
  let channels = 4
  const data: Buffer[] = []
  while (offset < png.length) {
    const length = png.readUInt32BE(offset)
    const type = png.toString('latin1', offset + 4, offset + 8)
    const body = png.subarray(offset + 8, offset + 8 + length)
    if (type === 'IHDR') {
      width = body.readUInt32BE(0)
      height = body.readUInt32BE(4)
      if (body[8] !== 8 || (body[9] !== 2 && body[9] !== 6) || body[12] !== 0) {
        throw new Error('only 8-bit RGB or RGBA, non-interlaced PNGs are read')
      }
      channels = body[9] === 6 ? 4 : 3
    } else if (type === 'IDAT') {
      data.push(body)
    }
    offset += 12 + length
  }
  const raw = inflateSync(Buffer.concat(data))
  const stride = width * channels
  const pixels = new Uint8Array(stride * height)
  for (let y = 0; y < height; y += 1) {
    const filter = raw[y * (stride + 1)]!
    const row = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1))
    for (let x = 0; x < stride; x += 1) {
      const left = x >= channels ? pixels[y * stride + x - channels]! : 0
      const up = y > 0 ? pixels[(y - 1) * stride + x]! : 0
      const corner = x >= channels && y > 0 ? pixels[(y - 1) * stride + x - channels]! : 0
      let predicted = 0
      if (filter === 1) predicted = left
      else if (filter === 2) predicted = up
      else if (filter === 3) predicted = (left + up) >> 1
      else if (filter === 4) {
        const estimate = left + up - corner
        const [a, b, c] = [estimate - left, estimate - up, estimate - corner].map(Math.abs) as [
          number,
          number,
          number,
        ]
        predicted = a <= b && a <= c ? left : b <= c ? up : corner
      }
      pixels[y * stride + x] = (row[x]! + predicted) & 0xff
    }
  }
  const rgba = new Uint8Array(width * height * 4)
  for (let index = 0; index < width * height; index += 1) {
    rgba[index * 4] = pixels[index * channels]!
    rgba[index * 4 + 1] = pixels[index * channels + 1]!
    rgba[index * 4 + 2] = pixels[index * channels + 2]!
    rgba[index * 4 + 3] = channels === 4 ? pixels[index * channels + 3]! : 255
  }
  return { width, height, rgba }
}

function at(image: Image, x: number, y: number): [number, number, number] {
  const index = (y * image.width + x) * 4
  return [image.rgba[index]!, image.rgba[index + 1]!, image.rgba[index + 2]!]
}

function near(actual: [number, number, number], expected: [number, number, number], tolerance = 4): boolean {
  return actual.every((channel, index) => Math.abs(channel - expected[index]!) <= tolerance)
}

// ---- Chrome ----------------------------------------------------------------

interface Event {
  method: string
  params: Record<string, unknown>
  sessionId?: string
}

interface Session {
  send(
    method: string,
    params?: Record<string, unknown>,
    sessionId?: string,
  ): Promise<Record<string, unknown>>
  /** Calls `listener` with every event of the page and its attached Workers. */
  listen(listener: (event: Event) => void): void
  close(): void
}

function connect(url: string): Promise<Session> {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url)
    let next = 1
    const pending = new Map<number, { resolve: (value: Record<string, unknown>) => void; reject: (error: Error) => void }>()
    const listeners: ((event: Event) => void)[] = []
    socket.addEventListener('message', (event) => {
      const message = JSON.parse(String(event.data)) as {
        id?: number
        result?: Record<string, unknown>
        error?: { message: string }
        method?: string
        params?: Record<string, unknown>
        sessionId?: string
      }
      if (message.id === undefined) {
        if (message.method !== undefined) {
          const delivered: Event = { method: message.method, params: message.params ?? {} }
          if (message.sessionId !== undefined) delivered.sessionId = message.sessionId
          for (const listener of listeners) listener(delivered)
        }
        return
      }
      const waiter = pending.get(message.id)
      pending.delete(message.id)
      if (message.error !== undefined) waiter?.reject(new Error(message.error.message))
      else waiter?.resolve(message.result ?? {})
    })
    socket.addEventListener('error', () => reject(new Error(`cannot connect to ${url}`)))
    socket.addEventListener('open', () => {
      resolve({
        send(method, params = {}, sessionId) {
          const id = next
          next += 1
          socket.send(JSON.stringify({ id, method, params, sessionId }))
          return new Promise((done, fail) => pending.set(id, { resolve: done, reject: fail }))
        },
        listen(listener) {
          listeners.push(listener)
        },
        close() {
          socket.close()
        },
      })
    })
  })
}

function launch(profile: string): Promise<{ port: number; stop: () => Promise<void> }> {
  let stopped: () => Promise<void> = () => Promise.resolve()
  return new Promise((resolve, reject) => {
    const browser = spawn(
      chrome,
      [
        '--headless=new',
        '--remote-debugging-port=0',
        `--user-data-dir=${profile}`,
        '--no-first-run',
        '--no-default-browser-check',
        '--enable-unsafe-webgpu',
        '--force-device-scale-factor=1',
        // Large enough that the canvas lies inside the viewport whatever
        // the headless window keeps for itself.
        '--window-size=800,600',
        'about:blank',
      ],
      { stdio: ['ignore', 'ignore', 'pipe'] },
    )
    let errors = ''
    browser.stderr.setEncoding('utf8')
    browser.stderr.on('data', (text: string) => {
      errors += text
      const match = /DevTools listening on ws:\/\/[^:]+:(\d+)\//.exec(errors)
      if (match !== null) {
        resolve({ port: Number(match[1]), stop: () => stopped() })
      }
    })
    browser.on('error', reject)
    const exited = new Promise<void>((done) => browser.on('exit', () => done()))
    browser.on('exit', (code) => reject(new Error(`Chrome exited (${String(code)}): ${errors}`)))
    stopped = async () => {
      browser.kill()
      await exited
    }
  })
}

async function evaluate(session: Session, expression: string): Promise<unknown> {
  const answer = await session.send('Runtime.evaluate', { expression, returnByValue: true })
  return (answer['result'] as { value?: unknown } | undefined)?.value
}

async function screenshot(session: Session): Promise<Buffer> {
  const answer = await session.send('Page.captureScreenshot', {
    format: 'png',
    clip: { x: 0, y: 0, width: WIDTH, height: HEIGHT, scale: 1 },
  })
  return Buffer.from(String(answer['data']), 'base64')
}

const delay = (ms: number): Promise<void> => new Promise((done) => setTimeout(done, ms))

/** What the page reports once the card has loaded or failed, within a minute. */
async function loaded(session: Session): Promise<{ loaded?: number; error?: string }> {
  const deadline = Date.now() + 60_000
  while (Date.now() < deadline) {
    const state = (await evaluate(session, 'window.renderCheck ?? null')) as {
      loaded?: number
      error?: string
    } | null
    if (state !== null) return state
    await delay(100)
  }
  throw new Error('the card did not load within a minute')
}

// ---- The check ---------------------------------------------------------------

/**
 * Collects the console output and uncaught exceptions of the page and of
 * every Worker it starts, the Render Worker and its own Workers included:
 * where the resource system's notes (`console.warn`) go.
 */
async function captureConsole(session: Session, lines: string[]): Promise<void> {
  const attach = (sessionId?: string): Promise<unknown> =>
    Promise.all([
      session.send('Runtime.enable', {}, sessionId),
      session.send(
        'Target.setAutoAttach',
        { autoAttach: true, waitForDebuggerOnStart: true, flatten: true },
        sessionId,
      ),
    ]).then(() =>
      sessionId === undefined
        ? undefined
        : session.send('Runtime.runIfWaitingForDebugger', {}, sessionId),
    )
  session.listen((event) => {
    if (event.method === 'Target.attachedToTarget') {
      const attached = event.params['sessionId'] as string
      void attach(attached).catch(() => undefined)
    } else if (event.method === 'Runtime.consoleAPICalled') {
      const args = (event.params['args'] as { value?: unknown; description?: string }[]) ?? []
      lines.push(
        `[${String(event.params['type'])}] ${args.map((arg) => String(arg.value ?? arg.description)).join(' ')}`,
      )
    } else if (event.method === 'Runtime.exceptionThrown') {
      const details = event.params['exceptionDetails'] as { text?: string; exception?: { description?: string } }
      lines.push(`[exception] ${details.exception?.description ?? details.text ?? ''}`)
    }
  })
  await attach()
}

async function main(): Promise<boolean> {
  const server = createServer(serve)
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done))
  const origin = `http://127.0.0.1:${String((server.address() as AddressInfo).port)}`
  const profile = mkdtempSync(path.join(tmpdir(), 'bobcat-render-check-'))
  const browser = await launch(profile)
  const failures: string[] = []
  try {
    const targets = (await (await fetch(`http://127.0.0.1:${String(browser.port)}/json/list`)).json()) as {
      type: string
      webSocketDebuggerUrl: string
    }[]
    const target = targets.find((candidate) => candidate.type === 'page')
    if (target === undefined) throw new Error('Chrome opened no page')
    const session = await connect(target.webSocketDebuggerUrl)
    const consoleLines: string[] = []
    await captureConsole(session, consoleLines)
    await session.send('Page.enable')
    await session.send('Page.navigate', { url: `${origin}/` })
    const state = await loaded(session)
    if (state.error !== undefined) throw new Error(`the card failed: ${state.error}`)
    const webgpu = await evaluate(session, "typeof navigator.gpu === 'object'")
    const decoder = await evaluate(session, "typeof ImageDecoder === 'function'")
    console.log(`WebGPU: ${String(webgpu)}; WebCodecs ImageDecoder: ${String(decoder)}`)

    // Forty screenshots about 50 ms apart: the forever-looping GIF shows
    // each of its frames in some of them.
    const shots: Image[] = []
    const pngs: Buffer[] = []
    for (let index = 0; index < 40; index += 1) {
      const png = await screenshot(session)
      pngs.push(png)
      shots.push(readPng(png))
      await delay(45)
    }
    // `rgb-twice.gif` plays for 1.2 s from its first draw, the `<image>`'s
    // one play for 0.6 s; wait both out.
    await delay(2_000)
    // Four screenshots across 750 ms, longer than the 300 ms the last frame
    // is shown in a loop: a picture that rests on its last frame shows it in
    // all four, and one that still loops cannot.
    const finals: Image[] = []
    for (let index = 0; index < 4; index += 1) {
      if (index > 0) await delay(250)
      const png = await screenshot(session)
      pngs.push(png)
      finals.push(readPng(png))
    }
    const final = finals[0]!
    if (out !== undefined) {
      mkdirSync(out, { recursive: true })
      pngs.forEach((png, index) => writeFileSync(path.join(out, `shot-${String(index).padStart(2, '0')}.png`), png))
    }

    const check = (label: string, actual: [number, number, number], expected: [number, number, number]): void => {
      const ok = near(actual, expected)
      console.log(`${ok ? 'ok  ' : 'FAIL'} ${label}: ${JSON.stringify(actual)} (expected ${JSON.stringify(expected)})`)
      if (!ok) failures.push(label)
    }
    check('still.png, opaque half', at(final, 35, 60), [255, 128, 0])
    check('still.png, half-transparent half over white', at(final, 85, 60), [127, 127, 255])
    finals.forEach((shot, index) => {
      const when = `screenshot ${String(index + 1)} of 4`
      check(`rgb-twice.gif as a CSS image rests on its last frame, left (${when})`, at(shot, 255, 60), [0, 0, 255])
      check(`rgb-twice.gif as a CSS image rests on its last frame, right (${when})`, at(shot, 305, 60), [0, 255, 0])
      check(`<image loop-count="1"> rests on its last frame, left (${when})`, at(shot, 365, 60), [0, 0, 255])
      check(`<image loop-count="1"> rests on its last frame, right (${when})`, at(shot, 415, 60), [0, 255, 0])
    })
    const forever = shots.map((shot) => at(shot, 170, 60))
    const white = forever.some((colour) => near(colour, [255, 255, 255]))
    const red = forever.some((colour) => near(colour, [255, 0, 0]))
    console.log(`${white && red ? 'ok  ' : 'FAIL'} fast-forever.gif shows white and red: white ${String(white)}, red ${String(red)}`)
    if (!(white && red)) failures.push('fast-forever.gif animates')
    // The video cell's centre: white until the first frame is drawn (the
    // page), then the clip's colours only, all three of them as it loops.
    const clip: Record<string, [number, number, number]> = {
      red: [255, 0, 0],
      green: [0, 255, 0],
      blue: [0, 0, 255],
    }
    const video = [...shots, ...finals].map((shot) => at(shot, 500, 60))
    const drawn = video.slice(video.findIndex((colour) => !near(colour, [255, 255, 255])))
    const named = drawn.map(
      (colour) => Object.keys(clip).find((name) => near(colour, clip[name]!, 24)) ?? JSON.stringify(colour),
    )
    const seen = new Set(named)
    const onlyClip = named.every((name) => name in clip)
    const allThree = Object.keys(clip).every((name) => seen.has(name))
    const videoOk = drawn.length > 0 && onlyClip && allThree
    console.log(
      `${videoOk ? 'ok  ' : 'FAIL'} rgb.mp4 in a muted, looping <video> plays the clip's colours: ${named.join(' ')}`,
    )
    if (!videoOk) failures.push('rgb.mp4 plays')
    if (failures.length > 0 || process.argv.includes('--console')) {
      console.log(`console of the page and its Workers (${String(consoleLines.length)} lines):`)
      for (const line of consoleLines) console.log(`  ${line}`)
    }
    session.close()
  } finally {
    await browser.stop()
    server.close()
    rmSync(profile, { recursive: true, force: true })
  }
  if (failures.length > 0) {
    console.error(`render check failed: ${failures.join(', ')}`)
    return false
  }
  console.log('render check passed')
  return true
}

// `--repeat N` runs the whole check N times, each in a fresh browser, and
// fails if any run does: an intermittent failure is a failure.
const repeat = Math.max(1, Number(argument('--repeat') ?? '1'))
let passed = 0
for (let run = 1; run <= repeat; run += 1) {
  if (repeat > 1) console.log(`--- run ${String(run)} of ${String(repeat)}`)
  if (await main()) passed += 1
}
if (repeat > 1) console.log(`${String(passed)} of ${String(repeat)} runs passed`)
if (passed < repeat) process.exitCode = 1
