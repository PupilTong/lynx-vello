// Copyright 2024 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.

/// <reference types="@rsbuild/core/types" />

/**
 * Type declarations specific to this project. The globals shared with the
 * showcase are declared by `@explorer/lib`.
 */

declare module '@lynx-js/types' {
  interface GlobalProps {
    initialPage?: string;
    platform?: string;
  }

  // The bundle-URL field is controlled through `value`, which the Explorer
  // hosts accept but `@lynx-js/types` only declares as `default-value`.
  interface InputProps {
    value?: string;
  }

  interface NativeModules {
    LynxNodeAPI?: {
      requireNodeAddon(addonName: string): void;
    };
  }
}

declare global {
  var __lynx_node_addon_exports__:
    | Record<string, Record<string, (...args: unknown[]) => unknown>>
    | undefined;
}

// A module, so the `declare module` above augments `@lynx-js/types` instead of
// replacing it.
export {};
