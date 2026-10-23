// The `bobcat:frame-boot` ESM: a nested card's main-thread boot, inside the
// `ShadowRealm` child `bobcat:frame` created for a `frame` element.
//
// It is the page's own boot (the `bobcat:boot` module `bobcat-core` writes in
// `main/runtime/lib.rs`), in the page's order, minus what a frame does not
// have: no `Document` is constructed — the child shares the page realm's
// document, and its `__CreatePage` answers the nested page the `frame`
// element built in its shadow root, which `framePage` reads and
// `__BobcatUsePageRoot` makes this realm's page root — and the entry is not
// imported but loaded synchronously, from the URL the fetched container
// registered its `root` at. The page data is whatever the parent hands over,
// as JSON text: `bobcat:frame` hands over the frame's `data` and
// `global-props` as they stand when this card boots, `{}` for one the frame
// was never given.
//
// Once the entry has loaded, a throwing one included, the boot dispatches
// the frame's `load` at the nested page, this realm's page root (user
// rulings 2026-10-21 and 2026-10-23): a `__CreateEvent` event, bubbling and
// composed, whose `detail` is `{statusCode: 0, statusMessage: "success",
// url}` with `url` the frame's `src` as written. Its path crosses the
// frame's shadow root into the page, so the listener table invites the realm
// that holds the frame's handle, whose `bindload` on the frame, listeners on
// the frame's ancestors and `global-bindEvent` for `load` hear it with
// `event.target` the frame; this realm's own listeners on its page hear it
// after them, in the routed dispatch's realm order (the page realm first,
// then the cards), all inside the host's `dispatchEvent` call. It is
// web-core's event — `dispatchLynxViewLoadEvent`
// (`web-core/ts/client/mainthread/elementAPIs/createElementAPI.ts`) dispatches
// the same `load` at the nested `lynx-view`, inside the card's first
// `__FlushElementTree`, and makes it cancelable, which nothing is here — at
// native's time: `FrameElement::SendLoadEvent`
// (`lynx/core/renderer/dom/fiber/frame_element.cc`) sends its `load` once the
// bundle is loaded, before the nested instance renders, as this boot sends
// it before `__BobcatRenderPage`. Native's does not bubble, is also sent for
// a failed fetch with the error code and message, and comes with a Lepus
// global `frameLoaded` event; the failure `load` and `frameLoaded` are not
// built, so a boot that fails before the entry loaded, and a fetch or an
// import `bobcat:frame` saw fail, dispatch nothing. What any realm's
// listener throws is that realm's failure, which the host reports, and boot
// goes on. The parent's listeners run inside the dispatch, so one of them can
// let go of this card while it boots — a new `src` written in `bindload` —
// and then boot stops there.
//
// The card has a background thread of its own, as a frame's card has in
// native (a nested `LynxTemplateRender`) and in web-core (a nested
// `lynx-view`'s `BackgroundThread`): this realm creates a `Worker` at
// `bobcat:bts` named `lynx-bg`, exactly as the page's boot does, and
// connects it with the card's BTS boot script — the container URL at
// `#background-thread` — as its entry. The worker is this realm's: what it
// says is delivered here, and the host keeps this realm while it runs. Its
// `initialize` carries the parent's `SystemInfo` and the view's native module
// table, as native's nested view inherits both; its `lynx.setSharedData` is
// the group's, which every BTS shares.
//
// `updateData` and `updateGlobalProps` take each later write of the frame's
// `data` and `global-props` while this card runs, as the page's own
// `__BobcatUpdateData` and `__BobcatUpdateGlobalProps` take the view's
// `update_data` and `update_global_props`, the BTS's `updateCardData` and
// `updateGlobalProps` included.
//
// The card's exposure is its own: its records are events at its elements,
// whose handles this realm holds, so its BTS hears its own elements alone;
// its `lynx.stopExposure` lists the elements this realm registered, so it
// stops this card's elements alone.
//
// `dispose` is the other half: `bobcat:frame` calls it when the frame lets
// go of this card, with a `done` of its own. It stops the card's exposure
// with `sendEvent` — the elements this realm registered — as native's card
// sends its exposed elements a last `disexposure` when it is torn down (user
// ruling 2026-10-16), and lets go of every card a frame of this card's
// loaded. When the stop queued records they are dispatched at the card's
// elements in the host's next intersection delivery entry, so `dispose`
// waits for this realm's next flush of records, which posts the last list
// to the card's BTS (user ruling 2026-10-20, option A), and for its own
// frames' cards to do the same; then it disposes of the card's BTS — its
// app cleanup runs and acknowledges — calls `done`, and closes this realm
// once the BTS answered. When nothing waits, all of it happens inside
// `dispose`, as before. The nested page is this card's alone: the parent's
// `resetFrame`, in `done`, frees it with everything under it, so this
// realm's page root then names a freed node, which nothing on the way to
// `closeRealm` reads: `__DestroyLifetime` runs before `done`, before the
// free, and what the BTS says meanwhile is ignored.
//
// `bobcat:runtime` is imported before `bobcat:element`, as the boot module
// imports them: the two import each other, and this order is what lets
// `bobcat:element` evaluate ahead of the module that reads its exports.

import {
  _ReportError,
  __BobcatConnectBackground,
  __BobcatDispose,
  __BobcatInitEntry,
  __BobcatInitializeMTS,
  __BobcatProcessInitData,
  __BobcatRenderPage,
  __BobcatUpdateData,
  __BobcatUpdateGlobalProps,
  lynx,
} from "bobcat:runtime";
import {
  __BobcatAfterExposureRecords,
  __BobcatSwitchExposure,
  __BobcatUsePageRoot,
  __CreateEvent,
  __DispatchEvent,
  __FlushElementTree,
  __GetPageElement,
} from "bobcat:element";
import { __BobcatCloseFrames } from "bobcat:frame";
import { Worker, __BobcatWorkerKey } from "bobcat-internal";
import {
  adoptStyleSheet,
  closeRealm,
  framePage,
  hasStyleSheet,
  loadModuleSync,
  markLoadTiming,
  parentNode,
} from "bobcat-internal:host";
import { backgroundThreadURL, bundleStyleSheetURL, sectionURL } from "bobcat:section-url";

/**
 * Whether `dispose` has run: the frame let go of this card, which a listener
 * of the parent's can do from inside `boot`'s `load` dispatch.
 */
let disposed = false;

/**
 * Boots the card at `url` into the frame `frameId`, and answers whether boot
 * ran through: `false` for a failure, which is reported here already, and
 * for a card the frame let go of during its own `load` dispatch.
 *
 * Every argument is a primitive, because only primitives cross into a
 * `ShadowRealm`'s wrapped function. `url` is the container URL — the frame's
 * `src` resolved against the view's base — and becomes this realm's
 * `__Card__`, the `bundleName` every `chunkURL` and `styleSheetURL` of the
 * card is written from and the base its workers resolve against — so they
 * name the URLs the container's installer registered under the same string.
 * `src` is the frame's `src` as written, which the frame's `load` names.
 * `nativeModuleTable` is the view's, as the parent realm holds it.
 *
 * The card's `root` is app code: what it throws is reported and boot goes
 * on to connect the BTS and render what it built, as the page's boot renders
 * a page whose entry threw. Between the two, the frame's `load` is
 * dispatched at the nested page ([`dispatchLoad`]); what that dispatch
 * throws is reported the same way — a listener's throw never reaches it,
 * being reported by the host — and boot goes on, unless a listener of the
 * parent's let go of this card meanwhile: then boot stops there and answers
 * `false`.
 */
export function boot(
  frameId: number,
  url: string,
  src: string,
  initDataJson: string,
  globalPropsJson: string,
  systemInfoJson: string,
  nativeModuleTable: string,
): boolean {
  try {
    // Refused past eight frames deep, so a card that embeds itself stops.
    const page = framePage(frameId);
    __BobcatUsePageRoot(page);
    __BobcatInitializeMTS({
      initData: JSON.parse(initDataJson),
      globalProps: JSON.parse(globalPropsJson),
      systemInfo: JSON.parse(systemInfoJson),
      enableJSDataProcessor: false,
      nativeModuleTable,
    });
    // The card's `root` may clear `lynx.__initData`; boot keeps its own copy.
    let data = lynx.__initData;
    __BobcatInitEntry(url);
    // The card's own page CSS, mounted on the frame's shadow root — the
    // nested page's parent — so it styles the nested tree alone. A card
    // without one has none registered.
    const sheet = bundleStyleSheetURL(url);
    if (hasStyleSheet(sheet)) adoptStyleSheet(sheet, shadowRootOf(page));
    // The card's load: it begins with its `root`'s evaluation, and ends at
    // this realm's first flush after the render below.
    markLoadTiming("vmExecuteStart", url);
    try {
      loadModuleSync(sectionURL("root", url), "module, exports");
    } catch (error) {
      _ReportError(error);
    }
    markLoadTiming("vmExecuteEnd");
    // The entry has loaded, whether or not it threw.
    try {
      dispatchLoad(src);
    } catch (error) {
      _ReportError(error);
    }
    // The parent's listeners run inside that dispatch, and one that gives
    // the frame a new `src` or lets go of it otherwise lets go of this card
    // there: `dispose` has run, and the parent may have freed the nested
    // page already. The card is gone, so boot stops: no BTS is connected and
    // nothing renders into the freed page.
    if (disposed) return false;
    data = __BobcatProcessInitData(data);
    // The card's BTS, after its `root` ran, so what the root sent before it
    // existed is flushed to it in order behind `initialize`, as on the page.
    const background = new Worker("bobcat:bts", { name: "lynx-bg" });
    __BobcatConnectBackground(
      background,
      data,
      backgroundThreadURL(url),
      __BobcatWorkerKey(background),
    );
    __BobcatRenderPage(data);
    // Queued after the jobs the lifecycle hooks already scheduled, as the
    // page's boot queues its flush.
    void Promise.resolve().then(() => __FlushElementTree()).catch(error => {
      _ReportError(error);
    });
    return true;
  } catch (error) {
    _ReportError(error);
    return false;
  }
}

/**
 * Takes the frame's new `data`, JSON text of a plain object, as the page's
 * `update_data` takes the view's with no processor name and no reset: the
 * card's `processData` with the default processor, then its `updatePage`,
 * which is the card's own code, and merges the update into the card's data
 * as that code does; then the processed data is posted to the card's BTS as
 * `updateCardData`, which is what re-renders a ReactLynx card once its first
 * screen is out. What fails is the card's and is reported here, so nothing
 * crosses back.
 */
export function updateData(json: string): void {
  try {
    __BobcatUpdateData(json, "", false);
  } catch (error) {
    _ReportError(error);
  }
}

/**
 * Takes the frame's new `global-props`, JSON text of a plain object, as the
 * page's `update_global_props` takes the view's: merged over the card's
 * current global props, its top-level keys replacing theirs, and the merged
 * object handed to the card's BTS (`updateGlobalProps`) and then to the
 * card's own `updateGlobalProps`. What it merges over is what `boot` handed
 * `__BobcatInitializeMTS`, which seeds the same host copy the page's boot
 * seeds.
 */
export function updateGlobalProps(json: string): void {
  try {
    __BobcatUpdateGlobalProps(json);
  } catch (error) {
    _ReportError(error);
  }
}

/**
 * Dispatches the frame's `load` at this realm's page root, the nested page
 * `__BobcatUsePageRoot` chose: bubbling and composed, so the realm that holds
 * the frame's handle hears it retargeted to the frame, with web-core's
 * success `detail` and `src`, the frame's `src` as written — web-core's
 * `host.url`, native's `data->src` — for its `url`.
 */
function dispatchLoad(src: string): void {
  const event = __CreateEvent(
    // Native's `Event::EventType::kCustomEvent`, which `__CreateEvent`
    // accepts and ignores.
    8,
    "load",
    { bubbles: true, composed: true },
    { statusCode: 0, statusMessage: "success", url: src },
  );
  __DispatchEvent(__GetPageElement(), event);
}

/** The shadow root the nested page `page` sits in: its parent. */
function shadowRootOf(page: number): number {
  const shadow = parentNode(page);
  if (shadow === null) throw new Error("the nested page is in no frame");
  return shadow;
}

/**
 * Lets go of this card: stops its exposure with `sendEvent` — every element
 * this realm registered, and those it registers from here on — which
 * answers how many `disexposure` records it queued, and lets go of every
 * card a frame of this realm loaded; once those records are posted to the
 * card's BTS ([`__BobcatAfterExposureRecords`]) and every such card has
 * posted its own, the card's BTS — `__DestroyLifetime`, then `dispose`,
 * whose app cleanup (ReactLynx's `destroyCard`, its unmount effects) runs
 * before the BTS acknowledges and this realm terminates the Worker — then
 * `done`, the parent's, which resets the frame, and then this realm itself,
 * which the host frees outside every JavaScript call.
 *
 * Returns at once. When the stop queued nothing and no frame's card waits,
 * everything up to `done` has happened by then; otherwise it happens in the
 * host's next intersection delivery entry, one main-thread entry later,
 * after the records were dispatched at the card's still-live elements. The
 * host keeps this realm while its BTS runs, so the acknowledgement is
 * delivered here; a BTS that never answers keeps it until the view ends.
 * The nested page, with what the card built under it, is the parent's to
 * free, with `resetFrame`, in `done`, and this realm ignores what its BTS
 * says from its disposal on.
 */
export function dispose(done?: () => void): void {
  disposed = true;
  let pending = 2;
  const arrive = () => {
    pending -= 1;
    if (pending > 0) return;
    void __BobcatDispose()
      .catch(error => {
        _ReportError(error);
      })
      .finally(closeRealm);
    try {
      done?.();
    } catch (error) {
      _ReportError(error);
    }
  };
  let queued = 0;
  try {
    queued = __BobcatSwitchExposure(false, true);
  } catch (error) {
    _ReportError(error);
  }
  __BobcatCloseFrames(arrive);
  if (queued > 0) __BobcatAfterExposureRecords(arrive);
  else arrive();
}
