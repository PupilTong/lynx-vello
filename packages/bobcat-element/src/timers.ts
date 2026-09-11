import { clearTimer, setTimer } from "bobcat-internal:host";

// The realm's half of `setTimeout`, `setInterval`, `clearTimeout`, and
// `clearInterval`, preloaded as the `bobcat:timers` ESM. Every realm this
// engine builds has it: the views' main-thread realms on one runtime, and
// each worker realm on the group's worker runtime.
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
// wait the thread carrying this realm parks in, and it owns HTML's
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

interface ScheduledTimer {
  /** The realm value to call when the timer fires. */
  handler: unknown;
  /** The arguments the call forwards. */
  args: unknown[];
  /** Whether firing it leaves it armed. */
  repeats: boolean;
}

/**
 * Every timer the realm has started and not cleared, by host id.
 *
 * It is also the authority on whether a timer still exists: the host takes a
 * whole batch of due timers at once, and a callback early in that batch can
 * clear one later in it, which the standard says must then not run.
 */
const scheduled: Map<number, ScheduledTimer> = new Map();

function start(
  handler: unknown,
  delay: unknown,
  args: unknown[],
  repeats: boolean,
): number {
  // A delay that is not a number is not rejected here: the host puts every
  // one through the standard's `long` conversion, so there is one place that
  // decides what `undefined`, a negative, and a huge value mean.
  const id = setTimer(Number(delay), repeats);
  scheduled.set(id, { handler, args, repeats });
  return id;
}

function stop(id: unknown): undefined {
  const key = Number(id);
  if (scheduled.delete(key)) {
    clearTimer(key);
  }
  return undefined;
}

/**
 * Runs the timer the host has taken from its schedule.
 */
export function __BobcatRunTimer(id: number): undefined {
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
  Reflect.apply(timer.handler as Function, undefined, timer.args);
  return undefined;
}

Object.assign(globalThis, {
  setTimeout(handler: unknown, delay: unknown, ...args: unknown[]): number {
    return start(handler, delay, args, false);
  },
  setInterval(handler: unknown, delay: unknown, ...args: unknown[]): number {
    return start(handler, delay, args, true);
  },
  clearTimeout(id: unknown): undefined {
    return stop(id);
  },
  clearInterval(id: unknown): undefined {
    return stop(id);
  },
});
