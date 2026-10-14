// The browser host's text input (`docs/editable-text-design.md`, E4).
//
// The engine lays out, paints and edits its `contenteditable` hosts itself;
// what a page cannot give it is the browser's input method and its
// clipboard, which only reach a focused text control. So
// the facade keeps one hidden `<textarea>` — Monaco's, CodeMirror 5's and
// Flutter web's technique — over the canvas at the engine's caret, focused
// while the engine has an editing host focused, holding the host's value and
// selection, and turns its events into the engine's editing commands:
//
// - every `beforeinput` outside a composition is prevented, so the textarea never holds text the
//   engine does not, and the insertion or deletion it describes is sent as a command;
// - a composition is the browser's to draw into the textarea while it lasts (its `beforeinput` is
//   not cancelable); each update is sent as `setComposition` and its end as `commitText`;
// - each `TextInputState` the engine publishes is mirrored back: the value, the selection, where
//   the box sits, the host's `inputmode` and `enterkeyhint`.
//
// The pure mapping is exported for `scripts/text-input.test.ts`; the class
// at the end owns the element.

import type {
  EditCommand,
  KeyFields,
  TextInputFields,
} from './protocol.d.ts'

export const MODIFIER_ALT = 1
export const MODIFIER_CTRL = 2
export const MODIFIER_META = 4
export const MODIFIER_SHIFT = 8

// What Chromium's `keyCode` is for a key the input method processed.
const KEY_CODE_PROCESS = 229

/** The members of a `KeyboardEvent` a key message is made of. */
export interface KeyboardEventFields {
  altKey: boolean
  code: string
  ctrlKey: boolean
  isComposing?: boolean
  key: string
  keyCode?: number
  metaKey: boolean
  repeat: boolean
  shiftKey: boolean
  type: string
}

/**
 * One `keydown` or `keyup` as the engine is told about it: the browser's
 * own W3C `key` and `code`, except that a key the input method is composing
 * with is `'Process'`, which is what Chromium reports and what the engine's
 * keyboard default actions ignore. Safari and Firefox report the key itself
 * with `isComposing` (or `keyCode` 229) instead, and an arrow that moves
 * inside a composition must not also move the engine's caret.
 */
export function keyFields(event: KeyboardEventFields): KeyFields {
  const composing =
    event.isComposing === true || event.keyCode === KEY_CODE_PROCESS
  return {
    code: event.code,
    down: event.type === 'keydown',
    key: composing ? 'Process' : event.key,
    modifiers:
      (event.altKey ? MODIFIER_ALT : 0) |
      (event.ctrlKey ? MODIFIER_CTRL : 0) |
      (event.metaKey ? MODIFIER_META : 0) |
      (event.shiftKey ? MODIFIER_SHIFT : 0),
    repeat: event.repeat,
  }
}

/**
 * Whether a `keydown` is one the engine's own default action handles for a
 * view whose text comes from commands: the arrows, Home and End (movement,
 * extended with shift) and ctrl- or meta-A (select all). The textarea's own
 * handling of such a key is prevented, so its selection does not drift from
 * the engine's, which the next state mirrors back. No page listener can
 * cancel the key in the engine: its default action always runs.
 */
export function engineHandlesKey(fields: KeyFields): boolean {
  switch (fields.key) {
    case 'ArrowLeft':
    case 'ArrowRight':
    case 'ArrowUp':
    case 'ArrowDown':
    case 'Home':
    case 'End':
      return true
    case 'a':
    case 'A':
      return (fields.modifiers & (MODIFIER_CTRL | MODIFIER_META)) !== 0
    default:
      return false
  }
}

/**
 * The command a `beforeinput` outside a composition stands for, or nothing
 * for an input type the engine has no command for (`historyUndo`,
 * `insertReplacementText`, `insertFromDrop`, the line deletions, …), which
 * the proxy prevents and drops.
 *
 * `data` is the event's `data`, or for a paste the plain text of its
 * `dataTransfer`; `hasRangeSelection` is whether the textarea — the engine's
 * selection, mirrored — selects anything, which a cut needs: the browser has
 * already copied that text when `deleteByCut` arrives.
 */
export function beforeInputCommand(
  inputType: string,
  data: string | null,
  hasRangeSelection: boolean,
): EditCommand | undefined {
  switch (inputType) {
    case 'insertText':
    case 'insertFromPaste':
      return data === null || data === '' ? undefined : { kind: 'insert', text: data }
    case 'insertLineBreak':
    case 'insertParagraph':
      return { kind: 'insertLineBreak' }
    case 'deleteContentBackward':
      return { kind: 'deleteBackward' }
    case 'deleteContentForward':
      return { kind: 'deleteForward' }
    case 'deleteWordBackward':
      return { kind: 'deleteWordBackward' }
    case 'deleteWordForward':
      return { kind: 'deleteWordForward' }
    case 'deleteByCut':
      return hasRangeSelection ? { kind: 'deleteBackward' } : undefined
    default:
      return undefined
  }
}

/**
 * One composition's commands, from the textarea's composition and `input`
 * events.
 *
 * A `compositionupdate` fires before the browser changes the textarea, so
 * the selection the input method put inside the composition is only there
 * at the `input` that follows; the update's text is held until then and
 * sent with that selection, relative to where the composition began. An
 * update equal to the last one sent is not sent again.
 *
 * The end keeps what the engine already holds when it can: a
 * `compositionend` whose text is the composition last sent (Chromium sends
 * the final update, its `input`, then the end) is `finishComposition`,
 * which dispatches `compositionend` alone in the engine, as the browser
 * did; any other text — a commit that differs from the last update, or an
 * update whose `input` never came — is one `commitText` of it (empty text
 * cancels).
 */
export class CompositionTracker {
  #composing = false
  #pending: string | undefined
  #sent: string | undefined
  #sentText: string | undefined
  #start = 0

  get composing(): boolean {
    return this.#composing
  }

  /** `compositionstart`: the composition replaces the selection there. */
  start(selectionStart: number): void {
    this.#composing = true
    this.#pending = undefined
    this.#sent = undefined
    this.#sentText = undefined
    this.#start = selectionStart
  }

  /** `compositionupdate`: its text, held for the `input` after it. */
  update(data: string): void {
    if (this.#composing) {
      this.#pending = data
    }
  }

  /** `input` while composing: the held update, with the textarea's selection. */
  input(selectionStart: number, selectionEnd: number): EditCommand | undefined {
    const text = this.#pending
    if (!this.#composing || text === undefined) {
      return undefined
    }
    this.#pending = undefined
    const clamp = (offset: number): number =>
      Math.min(Math.max(offset - this.#start, 0), text.length)
    const start = clamp(Math.min(selectionStart, selectionEnd))
    const end = clamp(Math.max(selectionStart, selectionEnd))
    const sent = `${String(start)}:${String(end)}:${text}`
    if (sent === this.#sent) {
      return undefined
    }
    this.#sent = sent
    this.#sentText = text
    return { kind: 'setComposition', selectionEnd: end, selectionStart: start, text }
  }

  /** `compositionend`: its text is what is committed. */
  end(data: string): EditCommand | undefined {
    if (!this.#composing) {
      return undefined
    }
    const held = this.#pending === undefined && this.#sentText === data
    this.forget()
    return held
      ? { keepSelection: false, kind: 'finishComposition' }
      : { kind: 'commitText', relativeCursor: 0, text: data }
  }

  /**
   * The textarea lost focus while composing and the browser did not end the
   * composition first: the engine keeps what was composed.
   */
  blur(): EditCommand | undefined {
    if (!this.#composing) {
      return undefined
    }
    this.forget()
    return { keepSelection: false, kind: 'finishComposition' }
  }

  /** Drops the composition without a command: its host is gone. */
  forget(): void {
    this.#composing = false
    this.#pending = undefined
    this.#sent = undefined
    this.#sentText = undefined
  }
}

/** A box in the page's client coordinates, CSS px. */
export interface ClientBox {
  height: number
  left: number
  top: number
  width: number
}

/**
 * Where the textarea sits: over the engine's caret, mapped from viewport
 * CSS px through the canvas's client box, so the input method's candidate
 * window opens beside the text being composed. Nothing when the canvas has
 * no finite, non-empty box.
 */
export function proxyBox(
  state: TextInputFields,
  canvas: ClientBox,
  viewportWidth: number,
  viewportHeight: number,
): ClientBox | undefined {
  const scaleX = canvas.width / viewportWidth
  const scaleY = canvas.height / viewportHeight
  const box = {
    height: Math.max(state.caret.height * scaleY, 1),
    left: canvas.left + state.caret.x * scaleX,
    top: canvas.top + state.caret.y * scaleY,
    width: Math.max(state.caret.width * scaleX, 1),
  }
  return Object.values(box).every(Number.isFinite) && canvas.width > 0 && canvas.height > 0
    ? box
    : undefined
}

/**
 * The `inputmode` the textarea carries: the host's own, or the mode
 * its input type implies, or none. A password host is not told apart: a
 * `<textarea>` has no password mode.
 */
export function inputModeFor(state: TextInputFields): string {
  if (state.inputMode !== '') {
    return state.inputMode
  }
  switch (state.inputType) {
    case 'email':
    case 'search':
    case 'tel':
    case 'url':
      return state.inputType
    case 'number':
      return 'decimal'
    default:
      return ''
  }
}

/** What the proxy needs from the page around it. */
export interface TextInputProxyHost {
  /** The canvas's client box, or nothing while it has none. */
  canvasBounds(): ClientBox | undefined
  /** Makes the textarea and puts it in the document. */
  createTextarea(): HTMLTextAreaElement
  /**
   * Gives keyboard focus back to the canvas when the textarea had it and
   * the engine's host was blurred, where the canvas can take it.
   */
  focusCanvas(): void
  sendEdit(command: EditCommand): void
  sendKey(fields: KeyFields): void
  /** The view's viewport, CSS px. */
  viewport(): { height: number; width: number }
}

// Not shown, not hit-tested, not reached by Tab, and never auto-corrected,
// capitalized or spell-checked: the engine's value is the value.
const TEXTAREA_STYLE = [
  'position: fixed',
  'display: none',
  'opacity: 0',
  'pointer-events: none',
  'margin: 0',
  'padding: 0',
  'border: 0',
  'outline: none',
  'resize: none',
  'overflow: hidden',
  'white-space: pre',
  'color: transparent',
  'background: transparent',
  'caret-color: transparent',
].join('; ')

const TEXTAREA_ATTRIBUTES: readonly (readonly [string, string])[] = [
  ['autocapitalize', 'off'],
  ['autocomplete', 'off'],
  ['autocorrect', 'off'],
  ['spellcheck', 'false'],
  ['tabindex', '-1'],
  ['wrap', 'off'],
]

/**
 * The hidden `<textarea>` itself, made the first time the engine focuses a
 * host and kept until the canvas is disposed.
 *
 * Focus: a state that focuses a host focuses the textarea at once. When
 * the user moves focus elsewhere in the page while the engine keeps its host
 * focused, the next press released on the canvas (`press`) focuses the
 * textarea again, so typing reaches the host the user returned to. Only the
 * engine's focusing a host, or a press on the canvas, focuses the textarea:
 * a later state does not take focus back from wherever the user moved it.
 */
export class TextInputProxy {
  #composition = new CompositionTracker()
  #disposed = false
  #host: TextInputProxyHost
  // Set while the proxy itself blurs the textarea, whose `blur` is then not
  // the user's leaving.
  #ownBlur = false
  #state: TextInputFields | undefined
  #textarea: HTMLTextAreaElement | undefined

  constructor(host: TextInputProxyHost) {
    this.#host = host
  }

  /** Whether the engine has an editing host focused. */
  get active(): boolean {
    return this.#state?.host === true && !this.#disposed
  }

  /**
   * Whether the textarea has focus while the engine has a host focused: a
   * press on the canvas then keeps it there (the canvas's `mousedown`
   * default would move focus away and end an open composition), and the
   * engine decides whether the press blurs its host.
   */
  get holdsFocus(): boolean {
    return this.active && this.#focused()
  }

  /** The textarea, once made; for tests and the facade's own checks. */
  get textarea(): HTMLTextAreaElement | undefined {
    return this.#textarea
  }

  /** One published `TextInputState`. */
  update(state: TextInputFields): void {
    if (this.#disposed) {
      return
    }
    const focusing = !this.active
    this.#state = state
    if (!state.host) {
      this.#hide()
      return
    }
    const textarea = this.#ensure()
    textarea.style.display = 'block'
    this.#place()
    const mode = inputModeFor(state)
    if (mode === '') {
      textarea.removeAttribute('inputmode')
    } else {
      textarea.setAttribute('inputmode', mode)
    }
    if (state.enterKeyHint === '') {
      textarea.removeAttribute('enterkeyhint')
    } else {
      textarea.setAttribute('enterkeyhint', state.enterKeyHint)
    }
    // A composition is the browser's to draw into the textarea while it
    // lasts: writing the value or the selection now would end it.
    if (!this.#composition.composing) {
      if (textarea.value !== state.value) {
        textarea.value = state.value
      }
      if (
        textarea.selectionStart !== state.selectionStart ||
        textarea.selectionEnd !== state.selectionEnd
      ) {
        textarea.setSelectionRange(state.selectionStart, state.selectionEnd)
      }
    }
    // Only as the engine focuses a host: a later state (a value, a caret, a
    // relayout) does not take focus back from wherever the user moved it.
    if (focusing && !this.#focused()) {
      textarea.focus({ preventScroll: true })
    }
  }

  /**
   * A pointer released on the canvas. Focuses the textarea when the engine
   * has a host focused and the textarea lost focus (the user tabbed away or
   * clicked another control, and came back).
   */
  press(): void {
    const textarea = this.#textarea
    if (this.active && textarea !== undefined && !this.#focused()) {
      textarea.focus({ preventScroll: true })
    }
  }

  /** Follows the canvas after a scroll or a resize moved it. */
  reposition(): void {
    if (this.active) {
      this.#place()
    }
  }

  /** A load replaced the view: its focused host and composition are gone. */
  reset(): void {
    this.#composition.forget()
    this.#state = undefined
    this.#hide()
  }

  dispose(): void {
    if (this.#disposed) {
      return
    }
    this.reset()
    this.#disposed = true
    const textarea = this.#textarea
    if (textarea === undefined) {
      return
    }
    this.#textarea = undefined
    textarea.removeEventListener('keydown', this.#onKey)
    textarea.removeEventListener('keyup', this.#onKey)
    textarea.removeEventListener('beforeinput', this.#onBeforeInput)
    textarea.removeEventListener('input', this.#onInput)
    textarea.removeEventListener('compositionstart', this.#onCompositionStart)
    textarea.removeEventListener('compositionupdate', this.#onCompositionUpdate)
    textarea.removeEventListener('compositionend', this.#onCompositionEnd)
    textarea.removeEventListener('blur', this.#onBlur)
    textarea.ownerDocument.removeEventListener('scroll', this.#onScroll, {
      capture: true,
    })
    textarea.remove()
  }

  #ensure(): HTMLTextAreaElement {
    if (this.#textarea !== undefined) {
      return this.#textarea
    }
    const textarea = this.#host.createTextarea()
    textarea.style.cssText = TEXTAREA_STYLE
    for (const [name, value] of TEXTAREA_ATTRIBUTES) {
      textarea.setAttribute(name, value)
    }
    textarea.addEventListener('keydown', this.#onKey)
    textarea.addEventListener('keyup', this.#onKey)
    textarea.addEventListener('beforeinput', this.#onBeforeInput)
    textarea.addEventListener('input', this.#onInput)
    textarea.addEventListener('compositionstart', this.#onCompositionStart)
    textarea.addEventListener('compositionupdate', this.#onCompositionUpdate)
    textarea.addEventListener('compositionend', this.#onCompositionEnd)
    textarea.addEventListener('blur', this.#onBlur)
    // The textarea is `position: fixed`, so a scroll that moves the canvas
    // (the page's, or any scroller around it) moves it out from under the
    // caret.
    textarea.ownerDocument.addEventListener('scroll', this.#onScroll, {
      capture: true,
      passive: true,
    })
    this.#textarea = textarea
    return textarea
  }

  #focused(): boolean {
    const textarea = this.#textarea
    return textarea !== undefined && textarea.ownerDocument.activeElement === textarea
  }

  #hide(): void {
    const textarea = this.#textarea
    if (textarea === undefined) {
      return
    }
    // The engine ended the focus (or the view): the composition the browser
    // may still hold is its host's, so its end must not be committed into
    // whatever the engine focuses next.
    this.#composition.forget()
    if (this.#focused()) {
      this.#ownBlur = true
      try {
        textarea.blur()
      } finally {
        this.#ownBlur = false
      }
      this.#host.focusCanvas()
    }
    textarea.style.display = 'none'
  }

  #place(): void {
    const textarea = this.#textarea
    const state = this.#state
    const canvas = this.#host.canvasBounds()
    if (textarea === undefined || state === undefined || canvas === undefined) {
      return
    }
    const viewport = this.#host.viewport()
    const box = proxyBox(state, canvas, viewport.width, viewport.height)
    if (box === undefined) {
      return
    }
    textarea.style.left = `${String(box.left)}px`
    textarea.style.top = `${String(box.top)}px`
    textarea.style.width = `${String(box.width)}px`
    textarea.style.height = `${String(box.height)}px`
    textarea.style.lineHeight = `${String(box.height)}px`
  }

  #send(command: EditCommand | undefined): void {
    if (command !== undefined && !this.#disposed) {
      this.#host.sendEdit(command)
    }
  }

  #onKey = (event: KeyboardEvent): void => {
    const fields = keyFields(event)
    if (fields.down && engineHandlesKey(fields)) {
      event.preventDefault()
    }
    this.#host.sendKey(fields)
  }

  #onBeforeInput = (event: InputEvent): void => {
    const textarea = this.#textarea
    // A composition's own input is not cancelable, and is the browser's to
    // apply until the composition ends.
    if (textarea === undefined || event.isComposing || this.#composition.composing) {
      return
    }
    const data = event.data ?? event.dataTransfer?.getData('text/plain') ?? null
    const command = beforeInputCommand(
      event.inputType,
      data,
      textarea.selectionStart !== textarea.selectionEnd,
    )
    // The textarea never changes on its own: the engine's next state is
    // what it holds.
    event.preventDefault()
    this.#send(command)
  }

  #onCompositionStart = (): void => {
    const textarea = this.#textarea
    if (textarea === undefined) {
      return
    }
    this.#composition.start(
      Math.min(textarea.selectionStart, textarea.selectionEnd),
    )
  }

  #onCompositionUpdate = (event: CompositionEvent): void => {
    this.#composition.update(event.data)
  }

  #onInput = (): void => {
    const textarea = this.#textarea
    if (textarea === undefined) {
      return
    }
    this.#send(
      this.#composition.input(textarea.selectionStart, textarea.selectionEnd),
    )
  }

  #onCompositionEnd = (event: CompositionEvent): void => {
    this.#send(this.#composition.end(event.data))
  }

  // Focus left the textarea for something outside the engine (the user
  // tabbed away, another control took it). The engine's own focus stays
  // where it is — as a browser keeps a document's focused element while the
  // window is in the background — and only a composition the browser left
  // open is finished.
  #onBlur = (): void => {
    if (this.#ownBlur) {
      return
    }
    this.#send(this.#composition.blur())
  }

  #onScroll = (): void => {
    this.reposition()
  }
}
