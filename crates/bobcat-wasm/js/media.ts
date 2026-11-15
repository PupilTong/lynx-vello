// The page's main-thread half of the browser's video player: one detached
// `<video>` per video the engine opened, whose frames go to the Render
// Worker as `VideoFrame`s. An `HTMLVideoElement` exists only where a
// `Document` does, so this half lives beside the facade; the Render Worker's
// half (`src/platform/media.rs`) holds each video's picture and sends the
// instructions this carries out.
//
// - A frame is taken whenever the element presents one, with
//   `requestVideoFrameCallback`, and the first one at `loadeddata`, so a
//   paused video shows its first frame. `new VideoFrame(video)` copies
//   nothing: it refers to the decoded frame, and the transfer hands it to the
//   Render Worker, which closes it.
// - The element's own clock paces the frames: the browser plays the audio and
//   presents each frame against it.
// - A `play()` the browser rejects — an unmuted play outside a user gesture,
//   under the autoplay rules — is reported as the video's `error` as it is.
//   Nothing works around it. An `AbortError`, a `play()` a `pause()` or a
//   `close` interrupted, is no failure and is not reported.

import type {
  FacadeMessage,
  MediaEventFields,
  MediaFrameMessage,
  MediaRequestMessage,
} from './protocol.d.ts'

/** `MediaError.MEDIA_ERR_NETWORK` and `MEDIA_ERR_SRC_NOT_SUPPORTED`. */
const MEDIA_ERR_NETWORK = 2
const MEDIA_ERR_SRC_NOT_SUPPORTED = 4

interface Player {
  video: HTMLVideoElement
  /** The `blob:` URL of the bytes the engine handed over, revoked on close. */
  objectUrl: string | undefined
  /** The pending `requestVideoFrameCallback`. */
  callback: number | undefined
  closed: boolean
}

type Post = (message: FacadeMessage, transfer?: Transferable[]) => void

/** Every video the Render Worker opened, by its number. */
export class MediaElements {
  #players = new Map<number, Player>()
  #post: Post

  constructor(post: Post) {
    this.#post = post
  }

  /** Carries out one `bobcat-media` instruction. */
  handle(message: MediaRequestMessage): void {
    if (message.op === 'open') {
      this.#open(message)
      return
    }
    const player = this.#players.get(message.player)
    if (player === undefined) {
      return
    }
    switch (message.op) {
      case 'play':
        player.video.play().catch((error: unknown) => {
          if (player.closed || (error instanceof DOMException && error.name === 'AbortError')) {
            return
          }
          this.#event(message.player, {
            event: 'error',
            kind: 'playback',
            message: error instanceof Error ? `${error.name}: ${error.message}` : String(error),
          })
        })
        break
      case 'pause':
        player.video.pause()
        break
      case 'loop':
        player.video.loop = message.value
        break
      case 'muted':
        player.video.muted = message.value
        break
      case 'close':
        this.#close(message.player, player)
        break
    }
  }

  /** Closes every video: the Render Worker is gone. */
  dispose(): void {
    for (const [number, player] of this.#players) {
      this.#close(number, player)
    }
  }

  #open(message: Extract<MediaRequestMessage, { op: 'open' }>): void {
    const number = message.player
    const video = document.createElement('video')
    video.muted = message.muted
    video.defaultMuted = message.muted
    video.loop = message.looping
    video.playsInline = true
    video.preload = 'auto'
    const player: Player = { video, objectUrl: undefined, callback: undefined, closed: false }
    this.#players.set(number, player)
    video.addEventListener(
      'loadeddata',
      () => {
        this.#event(number, {
          event: 'firstframe',
          width: video.videoWidth,
          height: video.videoHeight,
          duration: video.duration,
        })
        this.#frame(number, player)
      },
      { once: true },
    )
    video.addEventListener('ended', () => {
      this.#event(number, { event: 'ended' })
    })
    video.addEventListener('error', () => {
      const error = video.error
      const source = error?.code === MEDIA_ERR_NETWORK || error?.code === MEDIA_ERR_SRC_NOT_SUPPORTED
      this.#event(number, {
        event: 'error',
        kind: source ? 'source' : 'playback',
        message: error?.message || `the video failed (MediaError code ${String(error?.code)})`,
      })
    })
    const next = (): void => {
      if (player.closed) {
        return
      }
      player.callback = video.requestVideoFrameCallback(() => {
        this.#frame(number, player)
        next()
      })
    }
    next()
    if (message.bytes !== null) {
      const blob = new Blob([message.bytes as Uint8Array<ArrayBuffer>], {
        type: message.mediaType ?? '',
      })
      player.objectUrl = URL.createObjectURL(blob)
      video.src = player.objectUrl
    } else {
      // Frames of a cross-origin video are read only under CORS.
      video.crossOrigin = 'anonymous'
      video.src = message.url
    }
  }

  /** Transfers the frame `player`'s element presents now. */
  #frame(number: number, player: Player): void {
    if (player.closed || player.video.readyState < HTMLMediaElement.HAVE_CURRENT_DATA) {
      return
    }
    let frame: VideoFrame
    try {
      frame = new VideoFrame(player.video)
    } catch {
      return
    }
    const message: MediaFrameMessage = { type: 'bobcat-media-frame', player: number, frame }
    try {
      this.#post(message, [frame])
    } catch {
      frame.close()
    }
  }

  #event(number: number, fields: MediaEventFields): void {
    if (this.#players.get(number)?.closed !== false) {
      return
    }
    this.#post({ type: 'bobcat-media-event', player: number, ...fields })
  }

  #close(number: number, player: Player): void {
    player.closed = true
    this.#players.delete(number)
    if (player.callback !== undefined) {
      player.video.cancelVideoFrameCallback(player.callback)
    }
    player.video.pause()
    player.video.removeAttribute('src')
    player.video.load()
    if (player.objectUrl !== undefined) {
      URL.revokeObjectURL(player.objectUrl)
    }
  }
}
