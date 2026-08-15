import initWasm, { BobcatRenderer } from './pkg/bobcat_wasm.js'

let renderer
let running = false
let initialized = false
let responsePumpRunning = false
const pendingDomRequests = new Set()

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

function drainResponses() {
  for (;;) {
    const response = renderer.pollResponse()
    if (response === undefined) {
      return
    }
    const [request, ok, value] = response
    pendingDomRequests.delete(request)
    postResponse(request, ok, value)
  }
}

function enqueueDomRequest(request, enqueue) {
  pendingDomRequests.add(request)
  try {
    enqueue()
  } catch (error) {
    pendingDomRequests.delete(request)
    throw error
  }
  ensureResponsePump()
}

function ensureResponsePump() {
  if (responsePumpRunning || !running) {
    return
  }
  responsePumpRunning = true
  void pumpResponses()
}

async function pumpResponses() {
  try {
    while (running && pendingDomRequests.size !== 0) {
      drainResponses()
      if (pendingDomRequests.size !== 0) {
        await renderer.waitForResponse()
      }
    }
  } catch (error) {
    reportFatal(error)
  } finally {
    responsePumpRunning = false
    if (running && pendingDomRequests.size !== 0) {
      ensureResponsePump()
    }
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
    message.domWorkerUrl,
    message.threadCount,
  )
  running = true
  self.postMessage({ type: 'bobcat-ready' })
  scheduleFrame(renderFrame)
}

async function dispatchRequest(message) {
  if (renderer === undefined) {
    throw new Error('Bobcat Render Worker is not initialized')
  }

  const { operation, request } = message
  switch (operation) {
    case 'addAuthorStylesheet':
      enqueueDomRequest(request, () => {
        renderer.addAuthorStylesheet(request, message.css)
      })
      break
    case 'appendElement':
      enqueueDomRequest(request, () => {
        renderer.appendElement(request, message.parent, message.child)
      })
      break
    case 'createPage':
      enqueueDomRequest(request, () => {
        renderer.createPage(request)
      })
      break
    case 'createView':
      enqueueDomRequest(request, () => {
        renderer.createView(request)
      })
      break
    case 'dropElement':
      enqueueDomRequest(request, () => {
        renderer.dropElement(request, message.element)
      })
      break
    case 'flushElementTree':
      enqueueDomRequest(request, () => {
        renderer.flushElementTree(request)
      })
      break
    case 'registerFonts':
      enqueueDomRequest(request, () => {
        renderer.registerFonts(request, message.bytes)
      })
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
      while (pendingDomRequests.size !== 0) {
        const response = renderer.pollResponse()
        const [completedRequest, ok, value] = response
        pendingDomRequests.delete(completedRequest)
        postResponse(completedRequest, ok, value)
      }
      renderer.free()
      renderer = undefined
      postResponse(request, true)
      self.close()
      break
    default:
      throw new Error(`Unknown Bobcat Render Worker operation: ${operation}`)
  }
}

self.addEventListener('message', async (event) => {
  const message = event.data
  try {
    if (message?.type === 'bobcat-init') {
      await initialize(message)
    } else if (message?.type === 'bobcat-request') {
      try {
        await dispatchRequest(message)
      } catch (error) {
        postResponse(message.request, false, error)
      }
    }
  } catch (error) {
    reportFatal(error)
  }
})
