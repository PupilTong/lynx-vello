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
// `dispose` is the other half: `bobcat:frame` calls it when the frame lets
// go of this card. It lets go of every card a frame of this card's loaded,
// disposes of the card's BTS — its app cleanup runs and acknowledges — and
// only then closes this realm.
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
import { __BobcatUsePageRoot, __FlushElementTree } from "bobcat:element";
import { __BobcatCloseFrames } from "bobcat:frame";
import { Worker } from "bobcat-internal";
import {
  adoptStyleSheet,
  closeRealm,
  framePage,
  hasStyleSheet,
  loadModuleSync,
  parentNode,
} from "bobcat-internal:host";
import { backgroundThreadURL, bundleStyleSheetURL, sectionURL } from "bobcat:section-url";

/**
 * Boots the card at `url` into the frame `frameId`, and answers whether boot
 * ran through: `false` for a failure, which is reported here already.
 *
 * Every argument is a primitive, because only primitives cross into a
 * `ShadowRealm`'s wrapped function. `url` is the container URL — the frame's
 * `src` resolved against the view's base — and becomes this realm's
 * `__Card__`, the `bundleName` every `chunkURL` and `styleSheetURL` of the
 * card is written from and the base its workers resolve against — so they
 * name the URLs the container's installer registered under the same string.
 * `nativeModuleTable` is the view's, as the parent realm holds it.
 *
 * The card's `root` is app code: what it throws is reported and boot goes
 * on to connect the BTS and render what it built, as the page's boot renders
 * a page whose entry threw.
 */
export function boot(
  frameId: number,
  url: string,
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
    try {
      loadModuleSync(sectionURL("root", url), "module, exports");
    } catch (error) {
      _ReportError(error);
    }
    data = __BobcatProcessInitData(data);
    // The card's BTS, after its `root` ran, so what the root sent before it
    // existed is flushed to it in order behind `initialize`, as on the page.
    __BobcatConnectBackground(
      new Worker("bobcat:bts", { name: "lynx-bg" }),
      data,
      backgroundThreadURL(url),
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

/** The shadow root the nested page `page` sits in: its parent. */
function shadowRootOf(page: number): number {
  const shadow = parentNode(page);
  if (shadow === null) throw new Error("the nested page is in no frame");
  return shadow;
}

/**
 * Lets go of this card: every card a frame of this realm loaded first, then
 * the card's BTS — `__DestroyLifetime`, then `dispose`, whose app cleanup
 * (ReactLynx's `destroyCard`, its unmount effects) runs before the BTS
 * acknowledges and this realm terminates the Worker — and then this realm
 * itself, which the host frees outside every JavaScript call.
 *
 * Returns at once. The host keeps this realm while its BTS runs, so the
 * acknowledgement is delivered here; a BTS that never answers keeps it until
 * the view ends. What the card built under the nested page is the parent's
 * to free, with `resetFrame`, right after this returns, and this realm
 * ignores what its BTS says meanwhile.
 */
export function dispose(): void {
  __BobcatCloseFrames();
  void __BobcatDispose()
    .catch(error => {
      _ReportError(error);
    })
    .finally(closeRealm);
}
