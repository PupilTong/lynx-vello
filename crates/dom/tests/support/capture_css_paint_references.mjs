#!/usr/bin/env node
/** Captures browser-owned CSS paint atlases through Playwright. */

import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";

import { chromium } from "playwright";

const [, , baseUrl, outputArgument, ...requestedShards] = process.argv;
if (!baseUrl || !outputArgument) {
  throw new Error("expected <base-url> <output-directory> [shard ...]");
}

const shards =
  requestedShards.length === 0
    ? Array.from({ length: 40 }, (_, index) => index)
    : requestedShards.map((value) => {
        const shard = Number.parseInt(value, 10);
        if (!Number.isInteger(shard) || shard < 0 || shard >= 40) {
          throw new Error(`invalid shard ${JSON.stringify(value)}`);
        }
        return shard;
      });

const browser = await chromium.launch({
  channel: "chrome",
  headless: true,
  args: [
    "--force-color-profile=srgb",
    "--font-render-hinting=none",
    "--disable-skia-runtime-opts",
    "--disable-font-subpixel-positioning",
    "--disable-lcd-text",
    "--disable-composited-antialiasing",
    "--disable-system-font-check",
    "--force-device-scale-factor=1",
    "--disable-low-res-tiling",
    "--disable-smooth-scrolling",
    "--disable-gpu",
  ],
});

try {
  console.log(`Chrome ${browser.version()}`);
  const context = await browser.newContext({
    viewport: { width: 640, height: 640 },
    screen: { width: 640, height: 640 },
    deviceScaleFactor: 1,
    locale: "en-US",
    timezoneId: "UTC",
    colorScheme: "light",
    reducedMotion: "no-preference",
  });
  const page = await context.newPage();
  const output = path.resolve(outputArgument);
  await fs.mkdir(output, { recursive: true });

  for (const shard of shards) {
    const suffix = shard.toString().padStart(2, "0");
    await page.goto(
      new URL(
        `output/playwright/css-paint/shard-${suffix}.html`,
        `${baseUrl.replace(/\/?$/, "/")}`,
      ).href,
    );
    await page.waitForFunction(
      () => window.__ATLAS_READY__ === true || window.__ATLAS_ERROR__,
      null,
      { timeout: 10_000 },
    );
    const state = await page.evaluate(() => ({
      error: window.__ATLAS_ERROR__ ?? null,
      dpr: window.devicePixelRatio,
      width: window.innerWidth,
      height: window.innerHeight,
    }));
    if (state.error) {
      throw new Error(`shard ${suffix}: ${state.error}`);
    }
    if (state.dpr !== 1 || state.width !== 640 || state.height !== 640) {
      throw new Error(
        `shard ${suffix}: unexpected geometry DPR ${state.dpr}, ` +
          `${state.width}x${state.height}`,
      );
    }
    const destination = path.join(output, `shard-${suffix}.png`);
    await page.screenshot({
      path: destination,
      type: "png",
      scale: "css",
      animations: "disabled",
    });
    console.log(destination);
  }

  await context.close();
} finally {
  await browser.close();
}
