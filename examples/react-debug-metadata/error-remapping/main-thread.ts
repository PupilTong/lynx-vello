/** Resolves source-section main-thread frames through their source maps. */
import { readFileSync } from 'node:fs';

import { buildMapIndex, resolveStep } from './remap-lib.js';
import type { MapEntry, Step } from './remap-lib.js';

export type MainThreadEntry = MapEntry;

export interface MainThreadResult {
  release: string;
  path: string;
  lineno: number;
  colno: number;
  steps: Step[];
}

export function buildMainThreadIndex(distDirs: string[]): Map<string, MainThreadEntry> {
  return new Map([...buildMapIndex(distDirs)].filter(([, entry]) => entry.kind === 'main-thread'));
}

/** Locates a source error marker without requiring bytecode function IDs/PCs. */
export async function inferMainThread(
  marker: string,
  index: Map<string, MainThreadEntry>,
): Promise<MainThreadResult> {
  for (const [release, entry] of index) {
    const source = readFileSync(entry.jsFile, 'utf8');
    const offset = source.indexOf(marker);
    if (offset < 0) continue;
    const prefix = source.slice(0, offset);
    const lineno = prefix.split('\n').length;
    const colno = offset - prefix.lastIndexOf('\n');
    const step = await resolveStep(entry.map, lineno, colno - 1);
    if (!step) throw new Error(`no source map position for ${marker}`);
    return { release, path: entry.path, lineno, colno, steps: [step] };
  }
  throw new Error(`marker not found in any main-thread bundle: ${marker}`);
}
