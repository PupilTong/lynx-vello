// Copyright 2024 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.

/// <reference path="./typing.d.ts" />

export { openSchema, navigateTo } from './navigation';

export { AppContextProvider, useTheme, useSafeArea } from './context';

export {
  getRecentSessions,
  addRecentSession,
  clearRecentSessions,
} from './recentHistory';
export type { LaunchSession, LaunchSessionSource } from './recentHistory';
export {
  parseLaunchCommand,
  setCommandBoolean,
  setCommandTheme,
} from './launchCommand';
export type { LaunchCommand, CommandTheme } from './launchCommand';

export type {
  ThemePreference,
  ResolvedTheme,
  ThemeContext,
  SafeAreaContext,
} from './context';
