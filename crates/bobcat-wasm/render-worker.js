import initWasm, { BobcatRenderer } from './pkg/bobcat_wasm.js'

let renderer
let running = false
let initialized = false
let executeQueue = Promise.resolve()

const MAX_SCRIPT_BYTES = 16 * 1024 * 1024
const MAX_STYLE_SHEET_BYTES = 16 * 1024 * 1024
const SCRIPT_START_TIMEOUT_MS = 10_000

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
  try {
    renderer.renderIfRequested()
  } catch (error) {
    reportFatal(error)
    return
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
  let startupTimeout
  let startupDeadline
  try {
    while (running && renderer !== undefined && !renderer.pollScript()) {
      if (renderer.scriptStarted()) {
        clearTimeout(startupTimeout)
        // A running browser script has deliberately no execution deadline:
        // browsers expose no synchronous interrupt for this injected VM.
        await renderer.waitForEngineEvent()
      } else {
        startupDeadline ??= new Promise((_, reject) => {
          startupTimeout = setTimeout(() => {
            reject(
              new Error(
                `Bobcat script Worker did not start within ${String(SCRIPT_START_TIMEOUT_MS)} ms`,
              ),
            )
          }, SCRIPT_START_TIMEOUT_MS)
        })
        await Promise.race([renderer.waitForEngineEvent(), startupDeadline])
      }
    }
  } finally {
    clearTimeout(startupTimeout)
  }
  if (!running || renderer === undefined) {
    throw new Error('Bobcat was disposed before the script completed')
  }
}

async function dispatchRequest(message) {
  if (renderer === undefined) {
    throw new Error('Bobcat Render Worker is not initialized')
  }

  const { operation, request } = message
  switch (operation) {
    case 'executeScript': {
      const registeredUrl = await fetchScript(message.url)
      await renderer.executeScript(registeredUrl)
      void waitForScriptCompletion().then(
        () => postResponse(request, true),
        (error) => postResponse(request, false, error),
      )
      break
    }
    case 'loadStyleSheet': {
      const registeredUrl = await fetchStyleSheet(message.url)
      await renderer.loadStyleSheet(registeredUrl)
      postResponse(request, true)
      break
    }
    case 'registerFonts':
      postResponse(request, true, renderer.registerFonts(message.bytes))
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
      renderer.dispose()
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
  } else if (message?.type === 'bobcat-request') {
    const dispatch = async () => {
      try {
        await dispatchRequest(message)
      } catch (error) {
        postResponse(message.request, false, error)
      }
    }
    // `executeScript` and `loadStyleSheet` both reach engine state the script
    // thread can be holding, and stylesheets cascade in load order, so they
    // share one queue rather than dispatching as their fetches complete.
    if (
      message.operation === 'executeScript' ||
      message.operation === 'loadStyleSheet'
    ) {
      executeQueue = executeQueue.then(dispatch)
    } else {
      void dispatch()
    }
  }
})
