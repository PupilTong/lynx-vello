// The image decode worker `bobcat-resources` drives in the browser.
//
// Decoding is the platform's: `createImageBitmap` for the codec and its
// EXIF orientation, `createImageBitmap` again for a high-quality resize to
// the size the engine asked for, and an `OffscreenCanvas` to read the
// pixels out. Nothing here parses an image container.
//
// The Render Worker and this Worker share the Wasm memory. Every job has a
// mailbox of eight Int32 words at an address in that memory:
//   [0] state: 0 decoding, 1 size ready, 2 buffer provided, 3 pixels copied, -1 failed
//   [1] width, [2] height           of the decoded bitmap
//   [3] sourceWidth, [4] sourceHeight   of the image itself
//   [5] pointer, [6] length         of the pixel buffer the Render Worker allocated
// This Worker never waits on the mailbox: it stores, notifies, and echoes a
// message, so a Render Worker blocked in Atomics.wait and one parked on its
// event loop are both woken.

let memory
const jobs = new Map()

const STATE_SIZE_READY = 1
const STATE_DONE = 3
const STATE_FAILED = -1

function words(mailbox) {
  // Shared memory may have grown since the last job, so the view is taken
  // from the current buffer every time.
  return new Int32Array(memory.buffer, mailbox, 8)
}

function fit(width, height, maxWidth, maxHeight) {
  if (width <= maxWidth && height <= maxHeight) {
    return [Math.max(1, width), Math.max(1, height)]
  }
  const scale = Math.min(maxWidth / Math.max(1, width), maxHeight / Math.max(1, height))
  return [
    Math.max(1, Math.floor(width * scale)),
    Math.max(1, Math.floor(height * scale)),
  ]
}

function fail(id, mailbox, error) {
  jobs.delete(id)
  try {
    const mail = words(mailbox)
    Atomics.store(mail, 0, STATE_FAILED)
    Atomics.notify(mail, 0)
  } catch {
    // The mailbox is unreachable only if the memory itself is gone.
  }
  self.postMessage({
    type: 'error',
    id,
    message: String(error && error.message ? error.message : error),
  })
}

async function decode(message) {
  const { id, mailbox, bytes, maxWidth, maxHeight } = message
  try {
    const blob = new Blob([bytes])
    // Decoded at its own size first: the header the Render Worker probed
    // cannot see an EXIF orientation, and the bitmap can, so the size to
    // resize to is decided from the bitmap rather than from the header.
    let bitmap = await createImageBitmap(blob, { premultiplyAlpha: 'none' })
    const sourceWidth = bitmap.width
    const sourceHeight = bitmap.height
    const [width, height] = fit(sourceWidth, sourceHeight, maxWidth, maxHeight)
    if (width !== sourceWidth || height !== sourceHeight) {
      const resized = await createImageBitmap(bitmap, {
        premultiplyAlpha: 'none',
        resizeWidth: width,
        resizeHeight: height,
        resizeQuality: 'high',
      })
      bitmap.close()
      bitmap = resized
    }
    const canvas = new OffscreenCanvas(width, height)
    const context = canvas.getContext('2d', { willReadFrequently: true })
    context.drawImage(bitmap, 0, 0)
    bitmap.close()
    const data = context.getImageData(0, 0, width, height).data
    jobs.set(id, { data, mailbox })
    const mail = words(mailbox)
    mail[1] = width
    mail[2] = height
    mail[3] = sourceWidth
    mail[4] = sourceHeight
    Atomics.store(mail, 0, STATE_SIZE_READY)
    Atomics.notify(mail, 0)
    self.postMessage({ type: 'dims', id })
  } catch (error) {
    fail(id, mailbox, error)
  }
}

function copyOut(id) {
  const job = jobs.get(id)
  if (job === undefined) {
    return
  }
  jobs.delete(id)
  const mail = words(job.mailbox)
  const pointer = Atomics.load(mail, 5) >>> 0
  const length = Atomics.load(mail, 6) >>> 0
  if (length !== job.data.byteLength) {
    fail(id, job.mailbox, new Error('the pixel buffer does not match the decoded size'))
    return
  }
  new Uint8Array(memory.buffer, pointer, length).set(job.data)
  Atomics.store(mail, 0, STATE_DONE)
  Atomics.notify(mail, 0)
  self.postMessage({ type: 'done', id })
}

self.addEventListener('message', (event) => {
  const message = event.data
  if (message?.type === 'init') {
    memory = message.memory
  } else if (message?.type === 'decode') {
    if (memory === undefined) {
      fail(message.id, message.mailbox, new Error('the image worker was not initialized'))
      return
    }
    void decode(message)
  } else if (message?.type === 'buffer') {
    copyOut(message.id)
  }
})
