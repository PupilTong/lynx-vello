import initWasm, { BobcatRenderer } from './pkg/bobcat_wasm.js'

let renderer
let running = false
let resettingNativeView = false
let initialized = false
let entryScriptStarted = false
let scriptCompletion
let requestQueue = Promise.resolve()

const MAX_SCRIPT_BYTES = 16 * 1024 * 1024
const MAX_STYLE_SHEET_BYTES = 16 * 1024 * 1024
const MAX_LYNX_XML_BYTES = 16 * 1024 * 1024

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error)
}

function postResponse(request, ok, value) {
  if (ok) {
    self.postMessage({ type: 'bobcat-response', ok, request, value })
  } else {
    self.postMessage({
      type: 'bobcat-response',
      error: errorMessage(value),
      ok,
      request,
    })
  }
}

function reportFatal(error) {
  running = false
  self.postMessage({ type: 'bobcat-error', message: errorMessage(error) })
}

function scheduleFrame(callback) {
  if (typeof self.requestAnimationFrame === 'function') {
    self.requestAnimationFrame(callback)
  } else {
    setTimeout(callback, 16)
  }
}

function renderFrame() {
  if (!running) {
    return
  }
  if (!resettingNativeView) {
    try {
      // No timestamp crosses here. The animation timeline is core's own
      // clock, read on this Worker once per frame after the canvas surface
      // hands over an image; `requestAnimationFrame`'s timestamp is taken on
      // the page's main thread, on a different time origin, before this
      // Worker is even woken.
      renderer.renderIfRequested()
    } catch (error) {
      reportFatal(error)
      return
    }
  }
  scheduleFrame(renderFrame)
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
  scheduleFrame(renderFrame)
}

function absoluteUrl(input) {
  // The UI facade resolves relative URLs against document.baseURI. A Worker
  // cannot reconstruct that base and must reject accidental relative input.
  return new URL(String(input)).href
}

// The Render Worker applies the browser's URL, fetch, CORS, cache and
// credentials policy, then hands raw bytes to the engine's resource registry.
async function fetchAndRegister(kind, url, limit, register) {
  const response = await fetch(absoluteUrl(url))
  if (!response.ok) {
    throw new Error(
      `Could not fetch ${kind} ${response.url || url}: ${String(response.status)} ${response.statusText}`,
    )
  }
  const bytes = await readBoundedBytes(response, limit)
  return register(response.url || absoluteUrl(url), bytes)
}

function fetchScript(url) {
  return fetchAndRegister('script', url, MAX_SCRIPT_BYTES, (registered, bytes) =>
    renderer.registerScript(registered, bytes),
  )
}

// A browser host never decodes a `.web.bundle`, so the bytes it registers are
// CSS text; core takes the text arm of the stylesheet contract.
function fetchStyleSheet(url) {
  return fetchAndRegister(
    'stylesheet',
    url,
    MAX_STYLE_SHEET_BYTES,
    (registered, bytes) => renderer.registerStyleSheet(registered, bytes),
  )
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
  while (running && renderer !== undefined && !renderer.pollScript()) {
    await renderer.waitForEngineEvent()
  }
  if (!running || renderer === undefined) {
    throw new Error('Bobcat was disposed before the script completed')
  }
}

function trackScriptCompletion(request) {
  const completion = waitForScriptCompletion()
  scriptCompletion = completion
  void completion.then(
    () => postResponse(request, true),
    (error) => postResponse(request, false, error),
  )
}

function ensureEntryScriptNotStarted() {
  if (entryScriptStarted) {
    throw new Error('This Bobcat Canvas has already started its entry script')
  }
}

async function dispatchRequest(message) {
  if (renderer === undefined) {
    throw new Error('Bobcat Render Worker is not initialized')
  }

  const { operation, request } = message
  switch (operation) {
    case 'executeScript': {
      ensureEntryScriptNotStarted()
      const registeredUrl = await fetchScript(message.url)
      await renderer.executeScript(registeredUrl)
      entryScriptStarted = true
      trackScriptCompletion(request)
      break
    }
    case 'loadStyleSheet': {
      const registeredUrl = await fetchStyleSheet(message.url)
      await renderer.loadStyleSheet(registeredUrl)
      postResponse(request, true)
      break
    }
    case 'loadLynxXml': {
      // Core accepts exactly one entry script. Reject before fetching or
      // mounting the XML stylesheet so a failed repeated load is side-effect
      // free for the page that is already running.
      ensureEntryScriptNotStarted()
      const { source, url } = await fetchLynxXml(message.url)
      const [mainThreadScriptUrl, styleSheetUrl, backgroundThreadScriptUrl] =
        renderer.registerLynxXml(url, source)
      if (styleSheetUrl !== null) {
        await renderer.loadStyleSheet(styleSheetUrl)
      }
      if (backgroundThreadScriptUrl !== null) {
        console.warn(
          `Bobcat preserved the Lynx XML background-thread script at ${backgroundThreadScriptUrl}, but background-thread execution is not implemented`,
        )
      }
      await renderer.executeScript(mainThreadScriptUrl)
      entryScriptStarted = true
      trackScriptCompletion(request)
      break
    }
    case 'reset': {
      // A reset replaces the native view and its Lynx-main Worker, not this
      // Render Worker. Let a started entry evaluation publish its own response
      // first, then rebuild LynxView inside the existing Wasm instance/canvas.
      if (scriptCompletion !== undefined) {
        try {
          await scriptCompletion
        } catch {
          // A failed page is still replaceable by the next submission.
        }
      }
      entryScriptStarted = false
      scriptCompletion = undefined
      resettingNativeView = true
      try {
        await renderer.reset()
      } finally {
        resettingNativeView = false
      }
      postResponse(request, true)
      break
    }
    case 'registerFonts':
      postResponse(request, true, renderer.registerFonts(message.bytes))
      break
    case 'setDefaultFontFamily':
      postResponse(
        request,
        true,
        renderer.setDefaultFontFamily(message.family),
      )
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
    // wrapper while an async native-view reset owns its mutable borrow.
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
    // requests must not re-enter an async native reset while it temporarily
    // owns `&mut BobcatRenderer` across WebGPU surface attachment.
    requestQueue = requestQueue.then(dispatch)
  }
})
