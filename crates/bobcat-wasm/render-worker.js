import initWasm, { BobcatRenderer } from './pkg/bobcat_wasm.js'

let renderer
let running = false
let initialized = false
let scriptCompletion
let engineEventGeneration = 0
let requestQueue = Promise.resolve()

const MAX_SCRIPT_BYTES = 16 * 1024 * 1024
const MAX_STYLE_SHEET_BYTES = 16 * 1024 * 1024
const MAX_LYNX_XML_BYTES = 16 * 1024 * 1024

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error)
}

function postResponse(request, ok, error) {
  if (ok) {
    self.postMessage({ type: 'bobcat-response', ok, request })
  } else {
    self.postMessage({
      type: 'bobcat-response',
      error: errorMessage(error),
      ok,
      request,
    })
  }
}

function reportFatal(error) {
  running = false
  self.postMessage({ type: 'bobcat-error', message: errorMessage(error) })
}

// The Worker's frame clock, and the only one: while the engine owes the
// timeline another frame it draws at the display's rate, because drawing
// faster than the compositor shows is waste. `requestAnimationFrame` is that
// rate where a Worker is given one; a frame-interval timer stands in where it
// is not.
const FRAME_INTERVAL_MS = 16

function nextDisplayFrame() {
  return new Promise((resolve) => {
    if (typeof self.requestAnimationFrame === 'function') {
      self.requestAnimationFrame(() => resolve())
    } else {
      setTimeout(resolve, FRAME_INTERVAL_MS)
    }
  })
}

// One durable wakeup carries everything the engine has to say: a lifecycle
// event to drain and a frame to draw. A commit resolves it and the same
// Worker turn presents — no frame clock stands between the two. The clock is
// only ever the continuation's: an animation is the engine's own business,
// paced here, crossing nothing.
async function nextEngineWakeup() {
  if (renderer.isAnimating()) {
    await nextDisplayFrame()
  } else {
    await renderer.waitForEngineEvent()
  }
}

async function initialize(message) {
  if (initialized) {
    throw new Error('Bobcat Render Worker was initialized more than once')
  }
  initialized = true
  await initWasm()
  renderer = await BobcatRenderer.create(
    message.canvas,
    message.width,
    message.height,
    message.devicePixelRatio,
    message.workerUrl,
    message.threadCount,
    message.config.defaultDisplayLinear,
    message.config.defaultOverflowVisible,
    message.config.enableCSSSelector,
  )
  running = true
  self.postMessage({ type: 'bobcat-ready' })
}

function absoluteUrl(input) {
  // The UI facade resolves relative URLs against document.baseURI. A Worker
  // cannot reconstruct that base and must reject accidental relative input.
  return new URL(String(input)).href
}

// The Render Worker applies the browser's URL, fetch, CORS, cache and
// credentials policy; registration is the caller's, so a page's bytes reach
// the engine's registry only once every one of its sources has arrived.
async function fetchSource(kind, url, limit) {
  const response = await fetch(absoluteUrl(url))
  if (!response.ok) {
    throw new Error(
      `Could not fetch ${kind} ${response.url || url}: ${String(response.status)} ${response.statusText}`,
    )
  }
  return {
    bytes: await readBoundedBytes(response, limit),
    url: response.url || absoluteUrl(url),
  }
}

// Fetch the complete source once, then use the platform default decoder:
// UTF-8 with replacement for malformed byte sequences, matching web-core's
// raw Lynx XML loader. Parsing and section extraction remain in Rust.
async function fetchLynxXml(url) {
  const requestedUrl = absoluteUrl(url)
  const response = await fetch(requestedUrl)
  if (response.status !== 200) {
    throw new Error(
      `Could not fetch Lynx XML ${response.url || url}: expected HTTP status 200, received ${String(response.status)} ${response.statusText}`,
    )
  }
  const bytes = await readBoundedBytes(response, MAX_LYNX_XML_BYTES)
  return {
    source: new TextDecoder().decode(bytes),
    url: response.url || requestedUrl,
  }
}

async function readBoundedBytes(response, limit) {
  const declaredLength = Number(response.headers.get('content-length'))
  if (Number.isFinite(declaredLength) && declaredLength > limit) {
    throw new Error(`Response exceeds the ${String(limit)} byte limit`)
  }

  if (response.body === null) {
    const bytes = new Uint8Array(await response.arrayBuffer())
    if (bytes.byteLength > limit) {
      throw new Error(`Response exceeds the ${String(limit)} byte limit`)
    }
    return bytes
  }

  const reader = response.body.getReader()
  const chunks = []
  let length = 0
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) {
        break
      }
      length += value.byteLength
      if (length > limit) {
        await reader.cancel('Bobcat response exceeded its byte limit')
        throw new Error(`Response exceeds the ${String(limit)} byte limit`)
      }
      chunks.push(value)
    }
  } finally {
    reader.releaseLock()
  }

  const bytes = new Uint8Array(length)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.byteLength
  }
  return bytes
}

async function waitForScriptCompletion() {
  while (running && renderer !== undefined && !renderer.pump()) {
    await nextEngineWakeup()
  }
  if (!running || renderer === undefined) {
    throw new Error('Bobcat was disposed before the script completed')
  }
}

// The page's whole life after boot: every wakeup drains the engine's events
// and draws the frame it asked for.
async function servePage(generation) {
  const isCurrent = () =>
    running && renderer !== undefined && generation === engineEventGeneration
  while (isCurrent()) {
    await nextEngineWakeup()
    if (isCurrent()) {
      renderer.pump()
    }
  }
}

function trackScriptCompletion(request) {
  const generation = engineEventGeneration
  const completion = waitForScriptCompletion()
  scriptCompletion = completion
  void completion.then(
    () => postResponse(request, true),
    (error) => postResponse(request, false, error),
  )
  // Native construction has already rejected any boot failure. This waiter
  // consumes the successful ScriptFinished edge preserved for the host loop;
  // only a replacement page or disposal ends the live page it starts.
  void completion
    .catch(() => undefined)
    .then(() => servePage(generation))
    .catch(reportFatal)
}

// A view is its page: its sources are construction inputs, so loading a page
// builds a fresh native view and drops the previous one. Sources are fetched
// and registered before that happens, so a load that cannot fetch leaves the
// running page untouched.
async function replaceNativeView(request, entryUrl, styleSheetUrls) {
  if (scriptCompletion !== undefined) {
    try {
      await scriptCompletion
    } catch {
      // A failed page is still replaceable by the next submission.
    }
  }
  scriptCompletion = undefined
  // Advancing the generation first is what keeps the outgoing page's wakeup
  // loop off the renderer while the load owns its mutable borrow.
  engineEventGeneration += 1
  await renderer.load(entryUrl, styleSheetUrls)
  trackScriptCompletion(request)
}

async function dispatchRequest(message) {
  if (renderer === undefined) {
    throw new Error('Bobcat Render Worker is not initialized')
  }

  const { operation, request } = message
  switch (operation) {
    case 'load': {
      const sheets = []
      for (const url of message.styleSheetUrls) {
        sheets.push(await fetchSource('stylesheet', url, MAX_STYLE_SHEET_BYTES))
      }
      const entry = await fetchSource('script', message.url, MAX_SCRIPT_BYTES)
      // A browser host never decodes a `.web.bundle`, so the bytes it
      // registers are CSS text; core takes the text arm of the contract.
      const styleSheetUrls = sheets.map((sheet) =>
        renderer.registerStyleSheet(sheet.url, sheet.bytes),
      )
      const entryUrl = renderer.registerScript(entry.url, entry.bytes)
      await replaceNativeView(request, entryUrl, styleSheetUrls)
      break
    }
    case 'loadLynxXml': {
      const { source, url } = await fetchLynxXml(message.url)
      const [mainThreadScriptUrl, styleSheetUrl, backgroundThreadScriptUrl] =
        renderer.registerLynxXml(url, source)
      if (backgroundThreadScriptUrl !== null) {
        console.warn(
          `Bobcat preserved the Lynx XML background-thread script at ${backgroundThreadScriptUrl}, but background-thread execution is not implemented`,
        )
      }
      await replaceNativeView(
        request,
        mainThreadScriptUrl,
        styleSheetUrl === null ? [] : [styleSheetUrl],
      )
      break
    }
    case 'registerFonts':
      renderer.registerFonts(message.bytes)
      postResponse(request, true)
      break
    case 'setDefaultFontFamily':
      renderer.setDefaultFontFamily(message.family)
      postResponse(request, true)
      break
    case 'resize':
      renderer.resize(
        message.width,
        message.height,
        message.devicePixelRatio,
      )
      postResponse(request, true)
      break
    case 'dispose':
      running = false
      await renderer.dispose()
      renderer.free()
      renderer = undefined
      postResponse(request, true)
      self.close()
      break
    default:
      throw new Error(`Unknown Bobcat Render Worker operation: ${operation}`)
  }
}

self.addEventListener('message', (event) => {
  const message = event.data
  if (message?.type === 'bobcat-init') {
    void (async () => {
      try {
        await initialize(message)
      } catch (error) {
        reportFatal(error)
      }
    })()
  } else if (message?.type === 'bobcat-pointer') {
    const dispatch = () => {
      if (!running || renderer === undefined) {
        return
      }
      try {
        // No timestamp: core stamps the event's arrival from the same clock
        // its frames read, so nothing has to agree on a time origin.
        renderer.dispatchPointer(
          message.x,
          message.y,
          message.pointerId,
          message.device,
          message.phase,
          message.defaultPrevented,
        )
      } catch (error) {
        reportFatal(error)
      }
    }
    // Input shares the facade-operation queue so it cannot re-enter the Wasm
    // wrapper while an async native-view load owns its mutable borrow.
    requestQueue = requestQueue.then(dispatch)
  } else if (message?.type === 'bobcat-request') {
    const dispatch = async () => {
      try {
        await dispatchRequest(message)
      } catch (error) {
        postResponse(message.request, false, error)
      }
    }
    // Every facade operation shares one queue. In particular, resize and font
    // requests must not re-enter an async native load while it temporarily
    // owns `&mut BobcatRenderer` across WebGPU surface attachment.
    requestQueue = requestQueue.then(dispatch)
  }
})
