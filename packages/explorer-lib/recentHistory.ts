// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.

import { parseLaunchCommand } from './launchCommand';

const STORAGE_KEY = 'explorer_launch_sessions_v1';
const LEGACY_STORAGE_KEY = 'explorer_recent_urls';
const NATIVE_STORAGE_KEY = 'explorerNativeLaunchSessionsV1';
const MAX_ITEMS = 20;

export type LaunchSessionSource = 'input' | 'scan' | 'showcase' | 'external';

export interface LaunchSession {
  readonly id: string;
  readonly url: string;
  readonly source: LaunchSessionSource;
  readonly openedAt: number;
  readonly fullscreen: boolean;
  readonly hiddenNav: boolean;
  readonly theme: 'dark' | 'light' | null;
}

let memoryCache: LaunchSession[] | null = null;

function getStorage(key: string): unknown {
  try {
    const value = (lynx as any).getStorageSync(key);
    return typeof value === 'string' ? JSON.parse(value) : value;
  } catch {
    return undefined;
  }
}

function isSession(value: unknown): value is LaunchSession {
  if (!value || typeof value !== 'object') return false;
  const session = value as Partial<LaunchSession>;
  return (
    typeof session.id === 'string' &&
    typeof session.url === 'string' &&
    (session.source === 'input' ||
      session.source === 'scan' ||
      session.source === 'showcase' ||
      session.source === 'external') &&
    typeof session.openedAt === 'number' &&
    typeof session.fullscreen === 'boolean' &&
    typeof session.hiddenNav === 'boolean' &&
    (session.theme === null ||
      session.theme === 'dark' ||
      session.theme === 'light')
  );
}

function newSession(
  id: string,
  url: string,
  source: LaunchSessionSource,
  openedAt: number
): LaunchSession {
  const command = parseLaunchCommand(url);
  return {
    id,
    url,
    source,
    openedAt,
    fullscreen: command.fullscreen,
    hiddenNav: command.hiddenNav,
    theme: command.theme,
  };
}

function migrateLegacyHistory(): LaunchSession[] {
  const value = getStorage(LEGACY_STORAGE_KEY);
  if (!Array.isArray(value)) return [];
  const now = Date.now();
  return value
    .filter((url): url is string => typeof url === 'string')
    .map((url, index) =>
      newSession(`migrated-${index}-${url}`, url, 'input', now - index)
    );
}

function readStorage(): LaunchSession[] {
  if (memoryCache !== null) return memoryCache;
  const stored = getStorage(STORAGE_KEY);
  memoryCache = Array.isArray(stored)
    ? stored.filter(isSession)
    : migrateLegacyHistory();
  if (!Array.isArray(stored) && memoryCache.length) writeStorage(memoryCache);
  return memoryCache;
}

function writeStorage(sessions: LaunchSession[]): void {
  memoryCache = sessions;
  try {
    (lynx as any).setStorageSync(STORAGE_KEY, JSON.stringify(sessions));
  } catch {
    // In-memory history still works on hosts without synchronous storage.
  }
}

function readNativeSessions(): LaunchSession[] {
  try {
    if (typeof NativeModules === 'undefined') return [];
    const value = NativeModules.ExplorerModule.readFromLocalStorage(
      NATIVE_STORAGE_KEY
    );
    if (typeof value !== 'string') return [];
    const parsed: unknown = JSON.parse(value);
    return Array.isArray(parsed) ? parsed.filter(isSession) : [];
  } catch {
    return [];
  }
}

export function getRecentSessions(): LaunchSession[] {
  const sessions = [...readStorage(), ...readNativeSessions()].sort(
    (left, right) => right.openedAt - left.openedAt
  );
  const seen = new Set<string>();
  return sessions
    .filter((session) => {
      if (seen.has(session.url)) return false;
      seen.add(session.url);
      return true;
    })
    .slice(0, MAX_ITEMS);
}

export function addRecentSession(
  url: string,
  source: LaunchSessionSource = 'input'
): LaunchSession[] {
  const now = Date.now();
  const next = [
    newSession(`${now}-${url}`, url, source, now),
    ...readStorage().filter((item) => item.url !== url),
  ].slice(0, MAX_ITEMS);
  writeStorage(next);
  return [...next];
}

export function clearRecentSessions(): void {
  writeStorage([]);
  try {
    NativeModules.ExplorerModule.saveToLocalStorage(NATIVE_STORAGE_KEY, '[]');
  } catch {
    // Non-iOS hosts may not expose native preference storage.
  }
}
