// Copyright 2024 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.

import { addRecentSession } from './recentHistory';
import type { LaunchSessionSource } from './recentHistory';

function openWithExplorerModule(url: string): Promise<void> {
  'background only';
  try {
    NativeModules.ExplorerModule.openSchema(url);
    return Promise.resolve();
  } catch (cause) {
    const error =
      cause instanceof Error
        ? cause
        : new Error(`[navigation] native open failed: ${String(cause)}`);
    console.error(error.message);
    return Promise.reject(error);
  }
}

/**
 * Opens a URL through `ExplorerModule.openSchema`. History is written only
 * after the host accepts the URL.
 */
export function openSchema(
  url: string,
  source: LaunchSessionSource = 'input'
): Promise<void> {
  'background only';
  return openWithExplorerModule(url).then(() => {
    addRecentSession(url, source);
  });
}

/** Navigate to a local bundle path with optional params. */
export function navigateTo(
  path: string,
  params?: Record<string, string | number>
): Promise<void> {
  'background only';
  let url = `file://lynx?local://${path}`;
  if (params) {
    const qs = Object.entries(params)
      .map(
        ([key, value]) =>
          `${encodeURIComponent(key)}=${encodeURIComponent(String(value))}`
      )
      .join('&');
    url += `?${qs}`;
  }
  return openWithExplorerModule(url);
}
