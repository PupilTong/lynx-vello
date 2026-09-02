// @ts-check
import { clearTimer, setTimer } from "bobcat-internal:host";

// The realm's half of `setTimeout`, `setInterval`, `clearTimeout`, and
// `clearInterval`, preloaded as the `bobcat:timers` ESM in the QuickJS
// main-thread realm.
//
// These four are bare globals rather than named exports, because that is how
// a card reaches them: a compiled main-thread chunk calls `setTimeout` as a
// free variable, the way it would in the browser web-core runs in. So this
// module is imported for its effect and exports nothing a card uses — only
// `__BobcatRunTimer`, which the host calls back.
//
// # What lives here and what lives in the host
//
// The callback. It is a realm value, and only primitives cross the host
// boundary, so the host never sees one — it is filed here under the id the
// host hands back, and the host asks for it by that id when the timer comes
// due. Everything about *when* is the host's: it owns the clock, it owns the
// wait the Lynx main thread parks in between commands, and it owns HTML's
// delay clamp. This side keeps no deadline at all, which is why nothing here
// can disagree with the schedule.
//
// # Deviations from HTML
//
// A handler that is not callable throws where it fires rather than being
// compiled as a script: this realm has no `eval`, and a card that passes a
// non-function has a bug worth seeing. A thrown handler is reported to the
// embedder and leaves the realm usable, which is what the standard's
// "report the exception" step amounts to here.

/**
 * @typedef {object} ScheduledTimer
 * @property {unknown} handler The realm value to call when the timer fires.
 * @property {unknown[]} args The arguments the call forwards.
 * @property {boolean} repeats Whether firing it leaves it armed.
 */

/**
 * Every timer the realm has started and not cleared, by host id.
 *
 * It is also the authority on whether a timer still exists: the host takes a
 * whole batch of due timers at once, and a callback early in that batch can
 * clear one later in it, which the standard says must then not run.
 *
 * @type {Map<number, ScheduledTimer>}
 */
const scheduled = new Map();

/**
 * @param {unknown} handler
 * @param {unknown} delay
 * @param {unknown[]} args
 * @param {boolean} repeats
 * @returns {number}
 */
function start(handler, delay, args, repeats) {
  // A delay that is not a number is not rejected here: the host puts every
  // one through the standard's `long` conversion, so there is one place that
  // decides what `undefined`, a negative, and a huge value mean.
  const id = setTimer(Number(delay), repeats);
  scheduled.set(id, { handler, args, repeats });
  return id;
}

/**
 * @param {unknown} id
 * @returns {undefined}
 */
function stop(id) {
  const key = Number(id);
  if (scheduled.delete(key)) {
    clearTimer(key);
  }
  return undefined;
}

/**
 * Runs the timer the host has taken from its schedule.
 *
 * @param {number} id
 * @returns {undefined}
 */
export function __BobcatRunTimer(id) {
  const timer = scheduled.get(id);
  if (timer === undefined) {
    return undefined;
  }
  // A repeat stays filed, because the host has already re-armed it; a
  // one-shot is spent, and clearing it now is what lets its own callback
  // clear it harmlessly.
  if (!timer.repeats) {
    scheduled.delete(id);
  }
  Reflect.apply(/** @type {Function} */ (timer.handler), undefined, timer.args);
  return undefined;
}

Object.assign(globalThis, {
  /**
   * @param {unknown} handler
   * @param {unknown} delay
   * @param {...unknown} args
   * @returns {number}
   */
  setTimeout(handler, delay, ...args) {
    return start(handler, delay, args, false);
  },
  /**
   * @param {unknown} handler
   * @param {unknown} delay
   * @param {...unknown} args
   * @returns {number}
   */
  setInterval(handler, delay, ...args) {
    return start(handler, delay, args, true);
  },
  /**
   * @param {unknown} id
   * @returns {undefined}
   */
  clearTimeout(id) {
    return stop(id);
  },
  /**
   * @param {unknown} id
   * @returns {undefined}
   */
  clearInterval(id) {
    return stop(id);
  },
});
