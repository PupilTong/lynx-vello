// The main thread's half of the image decode protocol `bobcat-resources`
// drives from the Render Worker; `decode/browser.rs` in that crate is the
// other half.
//
// Decoding is the platform's: an `Image` element fed a Blob URL, which is
// the browser's codecs, its EXIF orientation handling and its SVG rasterizer,
// and a 2D canvas to resize with and read the pixels out of. Nothing here
// parses an image container.
//
// The Render Worker and this thread share the Wasm memory. Every job has a
// mailbox of eight Int32 words at an address in that memory:
//   [0] state: 0 decoding, 1 size ready, 2 buffer provided, 3 pixels copied, -1 failed
//   [1] width, [2] height           of the decoded bitmap
//   [3] sourceWidth, [4] sourceHeight   of the image itself
//   [5] pointer, [6] length         of the pixel buffer the Render Worker allocated
// This thread never waits on the mailbox — a main thread may not — so it
// stores, notifies, and echoes a message over the port, and a Render Worker
// blocked in Atomics.wait and one parked on its event loop are both woken.

const STATE_SIZE_READY = 1
const STATE_DONE = 3
const STATE_FAILED = -1

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

/** The decoder for one Render Worker, reached over one end of a MessageChannel. */
export class ImageDecoder {
  #canvas
  #context
  #jobs = new Map()
  #memory
  #port

  constructor(port) {
    this.#port = port
    port.onmessage = (event) => this.#onMessage(event.data)
  }

  close() {
    this.#port.onmessage = null
    this.#port.close()
    this.#jobs.clear()
    this.#memory = undefined
  }

  #words(mailbox) {
    // Shared memory may have grown since the last job, so the view is taken
    // from the current buffer every time.
    return new Int32Array(this.#memory.buffer, mailbox, 8)
  }

  #fail(id, mailbox, error) {
    this.#jobs.delete(id)
    try {
      const mail = this.#words(mailbox)
      Atomics.store(mail, 0, STATE_FAILED)
      Atomics.notify(mail, 0)
    } catch {
      // The mailbox is unreachable only if the memory itself is gone.
    }
    this.#port.postMessage({
      type: 'error',
      id,
      message: String(error && error.message ? error.message : error),
    })
  }

  async #decode(message) {
    const { id, mailbox, bytes, maxWidth, maxHeight, mediaType } = message
    // The type matters for SVG, which an Image only renders when told what
    // it is; every raster container is sniffed from its bytes regardless.
    const url = URL.createObjectURL(new Blob([bytes], { type: mediaType }))
    try {
      const image = new Image()
      image.decoding = 'async'
      image.src = url
      await image.decode()
      // The size the browser decoded, after any EXIF orientation, which the
      // header the Render Worker probed cannot see.
      const sourceWidth = image.naturalWidth
      const sourceHeight = image.naturalHeight
      if (sourceWidth === 0 || sourceHeight === 0) {
        throw new Error('the image has no intrinsic size')
      }
      const [width, height] = fit(sourceWidth, sourceHeight, maxWidth, maxHeight)
      const context = this.#contextFor(width, height)
      context.drawImage(image, 0, 0, width, height)
      const data = context.getImageData(0, 0, width, height).data
      this.#jobs.set(id, { data, mailbox })
      const mail = this.#words(mailbox)
      mail[1] = width
      mail[2] = height
      mail[3] = sourceWidth
      mail[4] = sourceHeight
      Atomics.store(mail, 0, STATE_SIZE_READY)
      Atomics.notify(mail, 0)
      this.#port.postMessage({ type: 'dims', id })
    } catch (error) {
      this.#fail(id, mailbox, error)
    } finally {
      URL.revokeObjectURL(url)
    }
  }

  // One canvas serves every job. Resizing it clears it and resets its state,
  // so the smoothing quality is chosen again each time.
  #contextFor(width, height) {
    if (this.#canvas === undefined) {
      this.#canvas = document.createElement('canvas')
      this.#context = this.#canvas.getContext('2d', { willReadFrequently: true })
    }
    this.#canvas.width = width
    this.#canvas.height = height
    this.#context.imageSmoothingEnabled = true
    this.#context.imageSmoothingQuality = 'high'
    return this.#context
  }

  #copyOut(id) {
    const job = this.#jobs.get(id)
    if (job === undefined) {
      return
    }
    this.#jobs.delete(id)
    const mail = this.#words(job.mailbox)
    const pointer = Atomics.load(mail, 5) >>> 0
    const length = Atomics.load(mail, 6) >>> 0
    if (length !== job.data.byteLength) {
      this.#fail(id, job.mailbox, new Error('the pixel buffer does not match the decoded size'))
      return
    }
    new Uint8Array(this.#memory.buffer, pointer, length).set(job.data)
    Atomics.store(mail, 0, STATE_DONE)
    Atomics.notify(mail, 0)
    this.#port.postMessage({ type: 'done', id })
  }

  #onMessage(message) {
    if (message?.type === 'init') {
      this.#memory = message.memory
    } else if (message?.type === 'decode') {
      if (this.#memory === undefined) {
        this.#fail(message.id, message.mailbox, new Error('the image decoder was not initialized'))
        return
      }
      void this.#decode(message)
    } else if (message?.type === 'buffer') {
      this.#copyOut(message.id)
    }
  }
}
