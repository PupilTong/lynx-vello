// Copyright 2024 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.

// The host globals Explorer pages read. `index.ts` references this file, so
// every program that imports `@explorer/lib` sees the same declarations.

declare module '@lynx-js/types' {
  interface GlobalProps {
    preferredTheme?: string;
    frontendTheme?: string;
    theme: string;
    isNotchScreen: boolean;
    screenHeight?: number;
    screenWidth?: number;
    safeAreaTop?: number;
    safeAreaBottom?: number;
    safeAreaLeft?: number;
    safeAreaRight?: number;
  }

  interface NativeModules {
    ExplorerModule: {
      openScan(): void;
      openSchema(url: string): void;
      getSettingInfo(): Record<string, unknown>;
      setThreadMode(index: number): void;
      saveThemePreferences(key: string, value: string): void;
      saveToLocalStorage(key: string, value: string): void;
      readFromLocalStorage(key: string): string | undefined;
    };
  }
}

// A module, so the `declare module` above augments `@lynx-js/types` instead of
// replacing it.
export {};
