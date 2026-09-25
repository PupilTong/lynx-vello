// `bobcat:system-info`: the runtime constants every realm's `SystemInfo`
// starts from, and what a screen adds to them. Which screen a realm passes is
// the host's; the crate's own tests read it back from live realms.

import { describe, expect, it } from "@rstest/core";
import { createSystemInfo } from "../src/system-info.ts";

describe("createSystemInfo", () => {
  it("answers the three runtime constants without a screen", () => {
    expect(createSystemInfo()).toEqual({
      platform: "headless",
      runtimeType: "quickjs",
      lynxSdkVersion: "4.1.0",
    });
  });

  it("answers a frozen object, a new one per call", () => {
    const first = createSystemInfo();
    expect(Object.isFrozen(first)).toBe(true);
    expect(createSystemInfo()).not.toBe(first);
  });

  it("adds the screen's members to the constants", () => {
    const screen = { pixelRatio: 2, pixelWidth: 750, pixelHeight: 1334 };
    const info = createSystemInfo(screen);
    expect(info).toEqual({
      platform: "headless",
      runtimeType: "quickjs",
      lynxSdkVersion: "4.1.0",
      pixelRatio: 2,
      pixelWidth: 750,
      pixelHeight: 1334,
    });
    expect(Object.isFrozen(info)).toBe(true);
    // The screen object is copied, not frozen or kept.
    expect(Object.isFrozen(screen)).toBe(false);
  });
});
