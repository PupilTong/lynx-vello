import { requestScriptFrame } from "bobcat-internal:host";
import { reportException } from "bobcat:event-target";

// One realm's animation-frame callbacks, preloaded as the
// `bobcat:animation-frame` ESM on both runtimes.
//
// Every realm kind can have them: `bobcat:runtime` gives an MTS entry
// `lynx.requestAnimationFrame` from here, `bobcat:bts-runtime` gives the BTS
// the same function as a `lynx` member and a bundle export, and a plain
// `Worker` imports this module itself. None of them installs a global. What
// a realm shares is this file: the MTS realm and each worker realm are on
// different runtimes, so each realm that imports it has its own table.
//
// # What lives here and what lives in the host
//
// The callbacks and the demand. The callbacks are realm values, so they stay
// here under the ids this module hands out. The demand is the one fact the
// host needs: `requestScriptFrame(true)` when the first callback is filed and
// `requestScriptFrame(false)` when the last one is cancelled, never once per
// callback. The host answers a pending demand by calling `__BobcatBeginFrame`
// on this module, in this realm, with the painter's vsync timestamp.
//
// A callback that throws is reported through `bobcat:event-target`'s
// `reportException` — `_ReportError` on MTS, the global scope's
// `reportError` on a worker, which makes it an uncaught exception of that
// worker — and the frame's other callbacks still run.

const callbacks = new Map<number, (milliseconds: number) => void>();
let nextId = 1;
/** Whether the host was last told this realm wants a frame. */
let frameRequested = false;

/** Tells the host about a change in whether any callback is waiting. */
function updateFrameRequest() {
  const pending = callbacks.size > 0;
  if (pending === frameRequested) return;
  frameRequested = pending;
  requestScriptFrame(pending);
}

export function requestAnimationFrame(callback: (milliseconds: number) => void): number {
  if (typeof callback !== "function") throw new TypeError("requestAnimationFrame requires a function");
  const id = nextId++;
  callbacks.set(id, callback);
  updateFrameRequest();
  return id;
}

export function cancelAnimationFrame(id: number): undefined {
  callbacks.delete(id);
  updateFrameRequest();
  return undefined;
}

/**
 * Drops every waiting callback and withdraws the demand: what a realm's
 * teardown does, since no frame it asked for should run after it.
 */
export function clearAnimationFrames(): undefined {
  callbacks.clear();
  updateFrameRequest();
  return undefined;
}

/**
 * Runs the callbacks that were waiting when this frame began, each with the
 * vsync timestamp in milliseconds. The host calls this while handling the
 * painter's vsync for this realm.
 *
 * Each callback is removed before it runs, and one a callback files waits for
 * the next frame: the ids are read once, up front. The host's demand bit was
 * consumed by the vsync that led here, so it is cleared first and raised again
 * by whatever is still waiting at the end.
 */
export function __BobcatBeginFrame(milliseconds: number): undefined {
  frameRequested = false;
  for (const id of Array.from(callbacks.keys())) {
    const callback = callbacks.get(id);
    callbacks.delete(id);
    if (callback) {
      try { callback(milliseconds); }
      catch (error) { reportException(error); }
    }
  }
  updateFrameRequest();
  return undefined;
}
