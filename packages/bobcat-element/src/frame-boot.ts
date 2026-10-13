// The `bobcat:frame-boot` ESM: a nested card's main-thread boot, inside the
// `ShadowRealm` child `bobcat:frame` created for a `frame` element.
//
// It is the page's own boot (the `bobcat:boot` module `bobcat-core` writes in
// `main/runtime/lib.rs`) minus what a frame does not have: no `Document` is
// constructed — the child shares the page realm's document, and its
// `createPage` answers the nested page `bindFrame` creates in the frame's
// shadow root — no BTS Worker is started, and the entry is not imported but
// loaded synchronously, from the URL the fetched container registered its
// `root` at. The page data is whatever the parent hands over, as JSON text;
// `bobcat:frame` hands over `{}` for both today.
//
// `bobcat:runtime` is imported before `bobcat:element`, as the boot module
// imports them: the two import each other, and this order is what lets
// `bobcat:element` evaluate ahead of the module that reads its exports.

import {
  _ReportError,
  __BobcatDetachBackground,
  __BobcatInitEntry,
  __BobcatInitializeMTS,
  __BobcatProcessInitData,
  __BobcatRenderPage,
  lynx,
} from "bobcat:runtime";
import { __FlushElementTree } from "bobcat:element";
import { adoptStyleSheet, bindFrame, hasStyleSheet, loadModuleSync } from "bobcat-internal:host";
import { bundleStyleSheetURL, sectionURL } from "bobcat:section-url";

/**
 * Boots the card at `src` into the frame `frameId`, and answers whether boot
 * ran through: `false` for a failure, which is reported here already.
 *
 * Every argument is a primitive, because only primitives cross into a
 * `ShadowRealm`'s wrapped function. `src` is the container URL as the frame
 * wrote it, and becomes this realm's `__Card__`, the `bundleName` every
 * `chunkURL` and `styleSheetURL` of the card is written from — so they name
 * the URLs the container's installer registered under the same string.
 *
 * The card's `root` is app code: what it throws is reported and boot goes
 * on to render what it built, as the page's boot renders a page whose entry
 * threw.
 */
export function boot(
  frameId: number,
  src: string,
  initDataJson: string,
  globalPropsJson: string,
  systemInfoJson: string,
): boolean {
  try {
    bindFrame(frameId);
    // A frame has no BTS: what this realm would send one is dropped.
    __BobcatDetachBackground();
    __BobcatInitializeMTS({
      initData: JSON.parse(initDataJson),
      globalProps: JSON.parse(globalPropsJson),
      systemInfo: JSON.parse(systemInfoJson),
      enableJSDataProcessor: false,
    });
    // The card's `root` may clear `lynx.__initData`; boot keeps its own copy.
    let data = lynx.__initData;
    __BobcatInitEntry(src);
    // The card's own page CSS, mounted on the frame's shadow root, so it
    // styles the nested tree alone. A card without one has none registered.
    const sheet = bundleStyleSheetURL(src);
    if (hasStyleSheet(sheet)) adoptStyleSheet(sheet);
    try {
      loadModuleSync(sectionURL("root", src), "module, exports");
    } catch (error) {
      _ReportError(error);
    }
    data = __BobcatProcessInitData(data);
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
