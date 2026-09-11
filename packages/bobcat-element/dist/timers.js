// Generated from src/timers.ts by TypeScript 7: edit that file and run `pnpm --filter bobcat-element build`.
// source fnv1a64 44f86e263171d831
import { clearTimer, setTimer } from "bobcat-internal:host";
/**
 * Every timer the realm has started and not cleared, by host id.
 *
 * It is also the authority on whether a timer still exists: the host takes a
 * whole batch of due timers at once, and a callback early in that batch can
 * clear one later in it, which the standard says must then not run.
 */
const scheduled = new Map();
function start(handler, delay, args, repeats) {
    // A delay that is not a number is not rejected here: the host puts every
    // one through the standard's `long` conversion, so there is one place that
    // decides what `undefined`, a negative, and a huge value mean.
    const id = setTimer(Number(delay), repeats);
    scheduled.set(id, { handler, args, repeats });
    return id;
}
function stop(id) {
    const key = Number(id);
    if (scheduled.delete(key)) {
        clearTimer(key);
    }
    return undefined;
}
/**
 * Runs the timer the host has taken from its schedule.
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
    Reflect.apply(timer.handler, undefined, timer.args);
    return undefined;
}
Object.assign(globalThis, {
    setTimeout(handler, delay, ...args) {
        return start(handler, delay, args, false);
    },
    setInterval(handler, delay, ...args) {
        return start(handler, delay, args, true);
    },
    clearTimeout(id) {
        return stop(id);
    },
    clearInterval(id) {
        return stop(id);
    },
});
