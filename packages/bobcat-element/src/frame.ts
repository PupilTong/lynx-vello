// The `bobcat:frame` ESM: what a `frame` element does in the realm that
// holds its handle — the parent half of a nested card.
//
// The `frame` custom element (`bobcat-core`'s `main/tree/frame.rs`) owns the
// nested page, `page[part=page]` in its shadow root, for the element's whole
// life. Which card renders under it, and when that card is let go of, is
// this module's, over three inputs:
//
// - `frameSourceChanged`, which `bobcat:element` calls after it has written
//   `src` to a frame's node;
// - `__BobcatFrameConnected` and `__BobcatFrameDisconnected`, which the host
//   calls in the page realm, one call per connection and disconnection the
//   custom element queued, out of the mutation that caused it.
//
// The steps:
//
// 1. A `null` or empty `src` does nothing: the frame keeps the card it has,
//    as native's `FrameElement::OnSetSrc` ignores a `src` it cannot load
//    (`lynx/core/renderer/dom/fiber/frame_element.cc`) and web-core's
//    `LynxView` keeps its running card when `url` is removed
//    (`web-core/ts/client/mainthread/LynxView.ts`). So does the `src` the
//    frame already has, as native skips an unchanged `src` and web-core an
//    unchanged `url`. A new `src` lets go of the frame's current card first
//    (`teardown`) and loads the new one, connected or not: a frame given a
//    `src` while detached renders into its nested page all the same.
// 2. `fetchResource(src)` fetches the container, `src` as written: the
//    fetcher resolves it against the view's base URL, and its installer
//    (`BundleInstaller`) registers the card's `root` at `<U>/root.js` and its
//    StyleInfo at `<U>/style-info.css`, `U` the resolved `src`, before the
//    fetch settles. `true` is a
//    container this view already fetched, which still starts in a later
//    microtask, so a start is never inside the `__SetAttribute` that asked
//    for it.
// 3. Once it settles, a fresh `ShadowRealm` imports `bobcat:frame-boot`'s
//    `boot` and `dispose` and calls `boot` with primitives alone — the
//    frame's node id, `src`, and the page data and `SystemInfo` as JSON
//    text — which renders the card under the frame's nested page.
// 4. A disconnection lets go of the card, as web-core's `LynxView` disposes
//    of its card on `disconnectedCallback`; a connection loads the frame's
//    `src` again if nothing is loaded or loading, as `connectedCallback`
//    renders it. Moving a frame is both, so it re-renders.
// 5. A frame whose handle is collected while it still has a card — it was
//    given a `src` and never connected — lets go of it too, through this
//    module's `FinalizationRegistry`, so no orphan child is left running on
//    its timers.
//
// Letting go (`teardown`) is two generic host calls in order: the child's
// `dispose` (its `closeRealm`, after disposing of the frames its own card
// loaded), and `resetFrame(frameId)`, which frees everything under the nested
// page and the sheets on the frame's shadow root. The retired child is freed
// by the host outside every JavaScript call.
//
// Every step checks that the frame still wants this load: a later `src` or a
// teardown bumps the frame's generation, and a fetch or an import that
// settles for an earlier one starts nothing.
//
// What the nested card does is the child's to report: a failure in the child
// reports there, through the same diagnostics pair, and its `boot` answers
// `false` for it, so nothing here reports it a second time. What fails here —
// the fetch, the realm, the import — is one diagnostic naming `src`. Nothing
// ends, and the frame stays empty.
//
// The per-frame state lives on the handle, under a symbol of this module,
// for the reason every per-handle record in `bobcat:element` does: QuickJS's
// `WeakMap` holds its values strongly. The host's calls name the frame by
// node id, which `bobcat:element`'s handle table resolves; a frame whose
// handle this realm does not hold is nothing to this realm. The states with a
// load started are also in `loaded`, for `__BobcatCloseFrames`; a state holds
// no handle, so neither keeps one alive.

import { fetchResource, resetFrame } from "bobcat-internal:host";
// Read only when the host calls in, never as this module evaluates:
// `bobcat:element` imports this module.
import { __BobcatHandleOf } from "bobcat:element";
import { reportError } from "bobcat:diagnostics";
import { Future } from "bobcat:future";
// A live binding, read when a load starts, never as this module evaluates:
// `bobcat:runtime` imports `bobcat:element`, which imports this module.
import { SystemInfo } from "bobcat:runtime";

/** `bobcat:frame-boot`'s `boot`, as the child's wrapped function. */
type Boot = (
  frameId: number,
  src: string,
  initData: string,
  globalProps: string,
  systemInfo: string,
) => boolean;

/** `bobcat:frame-boot`'s `dispose`, as the child's wrapped function. */
type Dispose = () => void;

/** One frame's card. */
interface FrameState {
  /** Bumped by every load and every teardown; a load of an earlier one is stale. */
  generation: number;
  /** The card the frame shows or is to show, or `null` before its first `src`. */
  src: string | null;
  /** Whether a load was started and not torn down: fetching, importing or booted. */
  loaded: boolean;
  /** The realm the current load created. */
  realm: ShadowRealm | undefined;
  /** The current child's `dispose`, once its import settled. */
  dispose: Dispose | undefined;
}

const stateSymbol = Symbol("frameState");

interface FrameHandle {
  [stateSymbol]?: FrameState;
}

/** Every state with a load started and not torn down. */
const loaded: Set<FrameState> = new Set();

/** A collected frame handle's card goes with it. */
const collected = new FinalizationRegistry((state: FrameState) => {
  close(state);
});

/** The state of the frame `frameId`, if this realm holds its handle and it ever had a `src`. */
function stateOf(frameId: number): FrameState | undefined {
  return (__BobcatHandleOf(frameId) as FrameHandle | undefined)?.[stateSymbol];
}

/**
 * The frame `handle` (node `frameId`) was given `src`: `null` for a removed
 * attribute, otherwise the written value, stringified. Only a non-empty `src`
 * other than the frame's current one does anything.
 */
export function frameSourceChanged(handle: object, frameId: number, src: string | null): undefined {
  if (src === null || src === "") return undefined;
  const frame = handle as FrameHandle;
  let state = frame[stateSymbol];
  if (state === undefined) {
    state = { generation: 0, src: null, loaded: false, realm: undefined, dispose: undefined };
    frame[stateSymbol] = state;
    collected.register(handle, state);
  }
  if (src === state.src) return undefined;
  if (state.src !== null) teardown(frameId, state);
  state.src = src;
  load(frameId, state, src);
  return undefined;
}

/**
 * The host's: the frame `frameId` was connected. Loads its `src` again when
 * nothing is loaded or loading — after a disconnection let go of it.
 */
export function __BobcatFrameConnected(frameId: number): undefined {
  const state = stateOf(frameId);
  if (state !== undefined && state.src !== null && !state.loaded) load(frameId, state, state.src);
  return undefined;
}

/** The host's: the frame `frameId` was disconnected. Lets go of its card. */
export function __BobcatFrameDisconnected(frameId: number): undefined {
  const state = stateOf(frameId);
  if (state !== undefined && state.loaded) teardown(frameId, state);
  return undefined;
}

/**
 * Lets go of every card a frame of this realm holds, without touching the
 * document: `bobcat:frame-boot`'s `dispose` calls it before it closes its
 * own realm, whose nested page the parent's `resetFrame` frees whole.
 */
export function __BobcatCloseFrames(): undefined {
  for (const state of [...loaded]) close(state);
  return undefined;
}

/** Starts loading `src` into the frame. */
function load(frameId: number, state: FrameState, src: string): void {
  state.generation += 1;
  state.loaded = true;
  loaded.add(state);
  const generation = state.generation;
  const current = () => state.generation === generation;
  let fetched: number | true;
  try {
    fetched = fetchResource(src);
  } catch (error) {
    report(src, error);
    return;
  }
  const start = () => {
    if (current()) startFrame(state, frameId, src, current);
  };
  if (fetched === true) {
    void Promise.resolve().then(start);
  } else {
    new Future<undefined>(fetched).then(start, error => {
      if (current()) report(src, error);
    });
  }
}

/** Creates the frame's realm and boots `src` in it. */
function startFrame(state: FrameState, frameId: number, src: string, current: () => boolean): void {
  let realm: ShadowRealm;
  try {
    realm = new ShadowRealm();
  } catch (error) {
    report(src, error);
    return;
  }
  state.realm = realm;
  Promise.all([
    realm.importValue("bobcat:frame-boot", "boot"),
    realm.importValue("bobcat:frame-boot", "dispose"),
  ]).then(([boot, dispose]) => {
    if (!current()) return;
    state.dispose = dispose as Dispose;
    // A `false` was reported in the child; there is nothing to add here.
    (boot as Boot)(frameId, src, "{}", "{}", JSON.stringify(SystemInfo));
  }).catch(error => {
    if (current()) report(src, error);
  });
}

/**
 * Lets go of the frame's card: closes its realm, then frees what it built
 * under the nested page and the sheets it mounted on the frame's shadow
 * root. The frame keeps its `src`, which a connection loads again.
 */
function teardown(frameId: number, state: FrameState): void {
  close(state);
  try {
    resetFrame(frameId);
  } catch (error) {
    report(state.src ?? "", error);
  }
}

/** Makes every load of the frame stale and closes the realm it created. */
function close(state: FrameState): void {
  state.generation += 1;
  state.loaded = false;
  loaded.delete(state);
  const dispose = state.dispose;
  state.dispose = undefined;
  state.realm = undefined;
  if (dispose === undefined) return;
  try {
    dispose();
  } catch (error) {
    report(state.src ?? "", error);
  }
}

function report(src: string, error: unknown): void {
  const message = error instanceof Error ? error.message : String(error);
  reportError(new Error(`frame ${src}: ${message}`));
}
