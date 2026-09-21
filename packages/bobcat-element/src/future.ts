import { settleFuture, takeFuture, waitFuture } from "bobcat-internal:host";

// One host-backed operation, usable either way, preloaded as the
// `bobcat:future` ESM. Every realm this engine builds has it: the views'
// main-thread realms on one runtime, and each worker realm on the group's
// worker runtime. It is an explicit import, and nothing is installed as a
// global.
//
// # What lives here and what lives in the host
//
// The host owns the operation. It is a Rust future it registered under a
// number, and that number is the whole of what crosses the boundary: only
// primitives and structured clones cross it, so a future cannot be handed
// over as itself, and what it settles to is taken by the same number once it
// has.
//
// This side owns the two shapes a realm asks for it in, and the fact that
// they are exclusive:
//
// - `wait(timeout?)` is synchronous. It parks the *job* it runs in until the
//   operation settles, the realm ends, or the deadline passes: the engine
//   thread's tasks go on running, and no other job does — not this realm's
//   promise jobs, and not a realm sharing its thread.
// - `then` converts this Future into one Promise, once. The host hands the
//   operation to its owner's epilogue, which awaits it on a task and then
//   *enters the realm* to deliver what it settled to.
//
// A `wait` after that conversion is refused, and the refusal is structural
// rather than a policy: the delivery is a job, and a job cannot run inside
// another job's wait, so a `wait` on a converted Future could only ever time
// out or hang.
//
// # What a timeout is not
//
// A timed-out `wait` cancels nothing. The operation goes on, the host keeps
// it, and a later `wait` or a `then` on the same Future still answers from
// it. `wait()` with no argument names no deadline at all and so never times
// out.

/** What `Future.wait` throws when its deadline passed. */
export class TimeoutError extends Error {
  override name = "TimeoutError";
}

/** One Promise conversion, while the host's settle is still outstanding. */
interface Settling {
  resolve(value: unknown): void;
  reject(reason: unknown): void;
}

/**
 * Every Future converted to a Promise whose host settle has not arrived, by
 * the host's own id. It is also the authority on whether a delivery is still
 * wanted: a settle for an id no longer here is dropped.
 */
const settling: Map<number, Settling> = new Map();

/** What a completed `wait` answered, memoized for whatever asks next. */
type Answer<T> = { readonly value: T } | { readonly error: unknown };

export class Future<T = unknown> implements PromiseLike<T> {
  /** The host's number for the operation. */
  readonly #id: number;
  #answer: Answer<T> | undefined;
  /** The one Promise `then` converted this Future into, if it did. */
  #promise: Promise<T> | undefined;
  /** Whether that conversion asked the host for an asynchronous settle. */
  #converted = false;

  constructor(id: number) {
    if (typeof id !== "number") {
      throw new TypeError("Future expects a host future id");
    }
    this.#id = id;
  }

  /**
   * The operation's value, waiting for it here if it has not settled.
   *
   * `timeout` is a number of milliseconds; omitted, it is no deadline at all.
   * A deadline that passes throws a `TimeoutError` and leaves this Future
   * usable.
   */
  wait(timeout?: number): T {
    if (this.#converted) {
      throw new TypeError("this Future is already a Promise");
    }
    const answer = this.#answer;
    if (answer !== undefined) {
      return unwrap(answer);
    }
    const milliseconds = timeout === undefined ? Infinity : Number(timeout);
    if (Number.isNaN(milliseconds)) {
      throw new TypeError("Future.wait expects a number of milliseconds");
    }
    if (!waitFuture(this.#id, milliseconds)) {
      throw new TimeoutError(
        `the Future did not settle within ${milliseconds} ms`,
      );
    }
    try {
      this.#answer = { value: takeFuture(this.#id) as T };
    } catch (reason) {
      // A rejection reaches a host member's caller as an `InternalError`
      // carrying the host's own wording; the class is this realm's to choose.
      const message = reason instanceof Error ? reason.message : String(reason);
      this.#answer = { error: new Error(message) };
    }
    return unwrap(this.#answer);
  }

  /**
   * This Future as the one Promise it converts to.
   *
   * A Future that has already answered needs no host settle: the Promise is
   * built from what the `wait` left here.
   */
  then<R1 = T, R2 = never>(
    onFulfilled?: ((value: T) => R1 | PromiseLike<R1>) | null,
    onRejected?: ((reason: unknown) => R2 | PromiseLike<R2>) | null,
  ): Promise<R1 | R2> {
    this.#promise ??= this.#convert();
    return this.#promise.then(onFulfilled, onRejected);
  }

  #convert(): Promise<T> {
    const answer = this.#answer;
    if (answer !== undefined) {
      return "error" in answer
        ? Promise.reject(answer.error)
        : Promise.resolve(answer.value);
    }
    this.#converted = true;
    return new Promise<T>((resolve, reject) => {
      settling.set(this.#id, {
        resolve: resolve as (value: unknown) => void,
        reject,
      });
      settleFuture(this.#id);
    });
  }
}

/**
 * Settles the Promise one Future was converted into.
 *
 * `value` is the host's settled value for a fulfilment and its reason for a
 * rejection. A settle for a Future nothing is waiting on is dropped, which is
 * what a realm released between the request and the delivery leaves.
 */
export function __BobcatSettleFuture(
  id: number,
  rejected: boolean,
  value: unknown,
): undefined {
  const pending = settling.get(id);
  if (pending === undefined) {
    return undefined;
  }
  settling.delete(id);
  if (rejected) {
    pending.reject(new Error(String(value)));
  } else {
    pending.resolve(value);
  }
  return undefined;
}

function unwrap<T>(answer: Answer<T>): T {
  if ("error" in answer) {
    throw answer.error;
  }
  return answer.value;
}
