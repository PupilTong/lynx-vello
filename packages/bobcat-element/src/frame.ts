// The `bobcat:frame` ESM: what a `frame` element's `src` does in the realm
// that wrote it — the parent half of a nested card.
//
// `bobcat:element` calls `frameSourceChanged` after it has written `src` to a
// frame's node. From there the load is this module's:
//
// 1. A `null` or empty `src` does nothing: the frame keeps the card it has,
//    as native's `FrameElement::OnSetSrc` ignores a `src` it cannot load
//    (`lynx/core/renderer/dom/fiber/frame_element.cc`) and web-core's
//    `LynxView` keeps its running card when `url` is removed
//    (`web-core/ts/client/mainthread/LynxView.ts`). A new `src` lets go of
//    the frame's current card first — `releaseFrame` frees its nested page
//    and retires the `ShadowRealm` that built it. The other release is the
//    frame's own: `dropElement` releases a frame whose handle was collected.
// 2. `fetchResource(src)` fetches the container, `src` as written: the
//    fetcher resolves it against the view's base URL, and its installer
//    (`BundleInstaller`) registers the card's `root` at `<U>/root.js` and its
//    StyleInfo at `<U>/style-info.css`, `U` the resolved `src`, before the
//    fetch settles. `true` is a
//    container this view already fetched, which still starts in a later
//    microtask, so a start is never inside the `__SetAttribute` that asked
//    for it.
// 3. Once it settles, a fresh `ShadowRealm` imports `bobcat:frame-boot` and
//    calls its `boot` with primitives alone — the frame's node id, `src`,
//    and the page data and `SystemInfo` as JSON text — which binds the child
//    to the frame and runs the card there.
//
// Every step checks that the frame still wants this load: a later, different
// `src` bumps the frame's generation, and a fetch or an import that settles
// for an earlier one starts nothing. A write of the `src` the frame already
// has does nothing, as native's `FrameElement::OnSetSrc` skips an unchanged
// `src` and web-core's `LynxView` an unchanged `url`; nor does a removal,
// which leaves a load in flight to finish.
//
// What the nested card does is the child's to report: a failure in the child
// reports there, through the same diagnostics pair, and its `boot` answers
// `false` for it, so nothing here reports it a second time. What fails here —
// the fetch, the realm, the import — is one diagnostic naming `src`. Nothing
// ends, and the frame stays empty.
//
// The per-frame state lives on the handle, under a symbol of this module,
// for the reason every per-handle record in `bobcat:element` does: QuickJS's
// `WeakMap` holds its values strongly. It is also what holds the frame's
// `ShadowRealm`, which the host frees once nothing refers to it.

import { fetchResource, releaseFrame } from "bobcat-internal:host";
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

/** One frame's load, filed on its handle. */
interface FrameLoad {
  /** Bumped by every new `src`; a load of an earlier one is stale. */
  generation: number;
  /** The card the frame shows or is loading, or `null` before its first. */
  src: string | null;
  /** The realm the current load created, held for the frame's life. */
  realm: ShadowRealm | undefined;
}

const loadSymbol = Symbol("frameLoad");

interface FrameHandle {
  [loadSymbol]?: FrameLoad;
}

/**
 * The frame `handle` (node `frameId`) was given `src`: `null` for a removed
 * attribute, otherwise the written value, stringified. Only a non-empty `src`
 * other than the frame's current one does anything.
 */
export function frameSourceChanged(handle: object, frameId: number, src: string | null): undefined {
  if (src === null || src === "") return undefined;
  const frame = handle as FrameHandle;
  const load = frame[loadSymbol] ??= { generation: 0, src: null, realm: undefined };
  if (src === load.src) return undefined;
  const replaced = load.src !== null;
  load.generation += 1;
  load.src = src;
  load.realm = undefined;
  if (replaced) releaseFrame(frameId);
  const wanted = src;
  const generation = load.generation;
  const current = () => load.generation === generation;
  let fetched: number | true;
  try {
    fetched = fetchResource(wanted);
  } catch (error) {
    report(wanted, error);
    return undefined;
  }
  const start = () => {
    if (current()) startFrame(load, frameId, wanted, current);
  };
  if (fetched === true) {
    void Promise.resolve().then(start);
  } else {
    new Future<undefined>(fetched).then(start, error => {
      if (current()) report(wanted, error);
    });
  }
  return undefined;
}

/** Creates the frame's realm and boots `src` in it. */
function startFrame(load: FrameLoad, frameId: number, src: string, current: () => boolean): void {
  let realm: ShadowRealm;
  try {
    realm = new ShadowRealm();
  } catch (error) {
    report(src, error);
    return;
  }
  load.realm = realm;
  realm.importValue("bobcat:frame-boot", "boot").then(boot => {
    // A `false` was reported in the child; there is nothing to add here.
    if (current()) (boot as Boot)(frameId, src, "{}", "{}", JSON.stringify(SystemInfo));
  }).catch(error => {
    if (current()) report(src, error);
  });
}

function report(src: string, error: unknown): void {
  const message = error instanceof Error ? error.message : String(error);
  reportError(new Error(`frame ${src}: ${message}`));
}
