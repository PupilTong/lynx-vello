// `lynx.fetchBundle`: the handle native answers with, over one plain host
// fetch and the `bobcat:future` `Future` it answers with.
//
// # What core does, and what it does not
//
// `fetchResource(url)` is **a fetch and nothing else** — the same thing that
// happens to an image source. `bobcat-core` learns that a URL was fetched and
// never what came back. Whether those bytes were a Lynx container whose
// sections are now loadable is the fetcher's business
// (`bobcat_resources::ContainerInstaller`, implemented by
// `bobcat_source::LazyBundleInstaller`), settled before the fetch completes.
// Everything Lynx-shaped about a `fetchBundle` — the `{wait, then}` object,
// the `{url, code, error_msg}` record, the `-2` timeout — lives here.
//
// It is **not a Promise**. Native's `ResponsePromise` is a host object with
// exactly `wait(seconds)` and `then(callback)`, `.then` answers `undefined`,
// and there is no chaining — so this builds that object rather than wrapping
// one. `options` is accepted and ignored, as native ignores it.
//
// # The two ways out are the Future's
//
// `wait(seconds)` is `Future.wait` in milliseconds — the job parks, the
// engine thread's tasks go on running, and a deadline that passes leaves the
// fetch running — and `.then` is the one conversion of that Future into its
// Promise, which the owner's epilogue settles. A `wait` **after** a `then`
// therefore throws a `TypeError`, `bobcat:future`'s structural refusal; see
// `docs/tracking/deviations.md`.
//
// # The repeat fetch, and why it settles at once
//
// Nothing here remembers a URL. What does is the **fetcher**, which answers
// `fetchResource` with `true` — no request, no Future — for a URL this view
// has already fetched. That handle is settled from the start: `wait` copies
// its record and `.then` runs through `later`. It is the fetcher's cache made
// visible *in the same job*, which is what native's `FindTemplateBundle` and
// web-core's promise cache both are, and what `rLynxPrepareLazyBundleMTS`
// depends on — its `lynx.loadScript('main-thread')` and
// `__LoadStyleSheet('CSS')` have to have run before the `callLepusMethod`
// reply reaches BTS.
//
// That is also the one place the two threads differ, which is why `later` is
// a parameter: a callback on a handle this realm already holds the record for
// runs inline on MTS (native's `LynxActor::Act`) and posted on BTS (native's
// `bts_runtime_mediator`).

import { fetchResource } from "bobcat-internal:host";
import { Future, TimeoutError } from "bobcat:future";

/** One settled fetch, as native spells it. */
export interface BundleInfo {
  /**
   * The string the caller passed, echoed. ReactLynx uses it as the
   * `bundleName` of every later `loadScript` and `__LoadStyleSheet`, and keys
   * its own cache by the same string, so the two have to stay equal.
   */
  url: string;
  /** `0` for a fetch that succeeded, `-1` for a failure, `-2` for a `wait` timeout. */
  code: number;
  error_msg: string;
}

/** The handle `lynx.fetchBundle` answers with: native's two members, and nothing else. */
export interface BundleHandle {
  wait(seconds: number): BundleInfo;
  then(callback: (info: BundleInfo) => void): void;
}

/**
 * One fetch this realm started, as the two members read it.
 *
 * `info` being set is the whole of "settled": whichever of a `wait` or the
 * Future's Promise got the outcome first writes it, and every later reader —
 * `wait` and `then` alike — answers out of it.
 */
interface Fetch {
  url: string;
  /** The host operation this fetch is; absent for one already fetched. */
  future?: Future<undefined>;
  info?: BundleInfo;
  /** Callbacks registered while this fetch was still outstanding. */
  callbacks: ((info: BundleInfo) => void)[];
  /** Whether that Future has already been converted into its one Promise. */
  converted: boolean;
}

export interface BundleFetchOptions {
  /** How this realm reports a callback that threw: `_ReportError` on MTS, `lynx.reportError` on BTS. */
  report: (error: unknown) => void;
  /**
   * How this realm runs a callback registered on a handle that has already
   * settled: inline on MTS, a posted task on BTS.
   */
  later: (run: () => void) => void;
}

/** A copy, so a caller that mutates what it was handed cannot reach the record. */
function copyOf(info: BundleInfo): BundleInfo {
  return { url: info.url, code: info.code, error_msg: info.error_msg };
}

/** What a `wait` that ran out its deadline answers: native's record, word for word. */
function timedOut(url: string, seconds: number): BundleInfo {
  return {
    url,
    code: -2,
    error_msg: `ResponsePromise wait timeout after ${seconds} seconds for url: ${url}`,
  };
}

/** The host's reason for a rejected fetch, as text. */
function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function createBundleFetches(options: BundleFetchOptions) {
  function run(callback: (info: BundleInfo) => void, info: BundleInfo) {
    // In its own try/catch: one callback that throws neither stops the ones
    // behind it nor ends the realm, exactly as a listener does.
    try { callback(copyOf(info)); }
    catch (error) { options.report(error); }
  }

  function handleOf(fetch: Fetch): BundleHandle {
    /** What the fetch settled to, written once and read by both members. */
    function settle(code: number, error_msg = ""): BundleInfo {
      fetch.info ??= { url: fetch.url, code, error_msg };
      return fetch.info;
    }

    /** Runs everything registered while this fetch was outstanding, once. */
    function deliver(info: BundleInfo): void {
      const callbacks = fetch.callbacks;
      fetch.callbacks = [];
      for (const registered of callbacks) run(registered, info);
    }

    return {
      wait(seconds: number): BundleInfo {
        // Native's JSI check: `wait` takes a number of *seconds*.
        if (typeof seconds !== "number") {
          throw new TypeError("fetchBundle(...).wait requires a number of seconds");
        }
        if (fetch.info) return copyOf(fetch.info);
        const future = fetch.future;
        // A handle with no Future is one the fetcher settled at the
        // `fetchBundle`, so `info` above already answered; this is the
        // unreachable arm.
        if (future === undefined) return copyOf(settle(0));
        try {
          // Seconds here, milliseconds there; `Infinity` stays `Infinity`,
          // which is the Future's own "no deadline at all". Every other
          // number is the Future's to read.
          future.wait(seconds * 1000);
        } catch (error) {
          // The timeout cancelled nothing: the operation went back into the
          // host's table, so a later `wait` or `then` still sees the result.
          if (error instanceof TimeoutError) return timedOut(fetch.url, seconds);
          // The realm refusing the call — the `TypeError` a `wait` after a
          // `then` gets above all — is not this fetch's outcome.
          if (error instanceof TypeError) throw error;
          // A rejected fetch: native's `-1`, carrying the host's reason.
          return copyOf(settle(-1, reasonOf(error)));
        }
        return copyOf(settle(0));
      },
      then(callback: (info: BundleInfo) => void): void {
        if (typeof callback !== "function") {
          throw new TypeError("fetchBundle(...).then requires a function");
        }
        const info = fetch.info;
        if (info) {
          options.later(() => run(callback, info));
          return;
        }
        fetch.callbacks.push(callback);
        const future = fetch.future;
        if (future === undefined || fetch.converted) return;
        // The first callback on an outstanding fetch is what converts the
        // Future, once: the host moves the fetch onto the owner's epilogue,
        // which awaits it on a task and enters this realm to resolve the
        // Promise. Every callback then runs as a reaction of that Promise,
        // each in its own try/catch, so none of them can become an unhandled
        // rejection.
        fetch.converted = true;
        void future.then(
          () => { deliver(settle(0)); },
          reason => { deliver(settle(-1, reasonOf(reason))); },
        );
      },
    };
  }

  /**
   * `lynx.fetchBundle(url, options?)`: one fetch of that URL, or the record
   * of one this view already fetched.
   *
   * Nothing is remembered here: `true` is the *fetcher* saying it already
   * holds that URL, which makes the handle settled from the start — so a
   * `.then` on it runs through `later`, inline on MTS. A number is the id of
   * the host future a fetch that had to be made settles through.
   */
  function fetchBundle(url: string, _options?: unknown): BundleHandle {
    if (typeof url !== "string") {
      throw new TypeError("fetchBundle requires a URL string");
    }
    const started = fetchResource(url);
    if (started === true) {
      return handleOf({
        url,
        info: { url, code: 0, error_msg: "" },
        callbacks: [],
        converted: false,
      });
    }
    return handleOf({
      url,
      future: new Future<undefined>(started),
      callbacks: [],
      converted: false,
    });
  }

  return { fetchBundle };
}
