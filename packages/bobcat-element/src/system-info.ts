// The one place a realm's `SystemInfo` is built, preloaded as the
// `bobcat:system-info` ESM on both runtimes, so the MTS and the BTS cannot
// report different runtime constants.
//
// `platform`, `runtimeType` and `lynxSdkVersion` name the runtime this engine
// is: the selected runtime target, independent of the minimum SDK a card was
// compiled for. The screen members come from the embedder: the MTS realm
// passes the numbers its boot module was written with, and the BTS passes the
// MTS realm's whole `SystemInfo`, which the `initialize` message carries.

/**
 * A frozen `SystemInfo`: the three runtime constants, then the members of
 * `screen` — `pixelRatio`, `pixelWidth` and `pixelHeight` as a view's realm
 * passes them, or another realm's whole `SystemInfo`, whose constants are
 * these — over them. Without a screen it is the constants alone.
 */
export function createSystemInfo(
  screen?: Record<string, unknown>,
): Readonly<Record<string, unknown>> {
  return Object.freeze({
    platform: "headless",
    runtimeType: "quickjs",
    lynxSdkVersion: "4.1.0",
    ...screen,
  });
}
