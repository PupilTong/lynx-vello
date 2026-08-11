/** Infers and reverses PrimJS main-thread frames for remapping tests. */
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';

import { SourceMapConsumer } from 'source-map';
import type { RawSourceMap } from 'source-map';

import { stepFromLines } from './remap-lib.js';
import type { Step } from './remap-lib.js';

interface LineCol {
  line: number;
  column: number;
}
interface FunctionInfo {
  function_id: number;
  function_name: string;
  line_col: LineCol[];
}
export interface MainThreadEntry {
  release: string;
  /** main-thread bundle path, e.g. `.rspeedy/LazyComponent/main-thread.js`. */
  path: string;
  functions: FunctionInfo[];
  /** generated main-thread.js source, for the bytecode step's context. */
  functionSource: string;
  map: RawSourceMap;
}

export interface MainThreadResult {
  release: string;
  path: string;
  functionId: number;
  /** pc the engine reports — the last bytecode on the throw line (device: 21:38). */
  pc: number;
  /** ordered reversal chain: bytecode-debug-info -> source-map (matches biz_sourcemap). */
  steps: Step[];
}

function walk(dir: string, cb: (file: string) => void): void {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p, cb);
    else cb(p);
  }
}

/** Indexes main-thread bytecode functions and source maps by release. */
export function buildMainThreadIndex(
  distDirs: string[],
): Map<string, MainThreadEntry> {
  const index = new Map<string, MainThreadEntry>();
  for (const dir of distDirs) {
    if (!existsSync(dir)) continue;
    walk(dir, (file) => {
      if (path.basename(file) !== 'debug-metadata.json') return;
      const meta = JSON.parse(readFileSync(file, 'utf8')) as {
        artifacts?: {
          kind: string;
          debugSources?: Record<string, unknown>[];
        }[];
      };
      for (
        const artifact of meta.artifacts as {
          kind: string;
          path?: string;
          debugSources?: Record<string, unknown>[];
        }[] ?? []
      ) {
        if (artifact.kind !== 'main-thread') continue;
        const smds = artifact.debugSources?.find((d) =>
          d.kind === 'source-map'
        );
        const bcds = artifact.debugSources?.find((d) =>
          d.kind === 'bytecode-debug-info'
        );
        if (!smds?.key || !smds.map || !bcds?.debugInfo) continue;
        const map = (typeof smds.map === 'string'
          ? JSON.parse(smds.map)
          : smds.map) as RawSourceMap;
        const dbg = (typeof bcds.debugInfo === 'string'
          ? JSON.parse(bcds.debugInfo) as unknown
          : bcds.debugInfo) as {
            lepusNG_debug_info?: {
              function_info?: FunctionInfo[];
              function_source?: string;
            };
          };
        const lng = dbg.lepusNG_debug_info;
        index.set(smds.key as string, {
          release: smds.key as string,
          path: artifact.path ?? '',
          functions: lng?.function_info ?? [],
          functionSource: lng?.function_source ?? '',
          map,
        });
      }
    });
  }
  return index;
}

/** Infers a main-thread frame from a unique source-message marker. */
export async function inferMainThread(
  marker: string,
  index: Map<string, MainThreadEntry>,
): Promise<MainThreadResult> {
  for (const [release, entry] of index) {
    const srcIdx = (entry.map.sourcesContent ?? []).findIndex((c) =>
      c?.includes(marker)
    );
    if (srcIdx < 0) continue;
    const content = entry.map.sourcesContent![srcIdx];
    const file = path.basename(entry.map.sources[srcIdx]);
    const throwLine =
      content.slice(0, content.indexOf(marker)).split('\n').length;
    const genLines = entry.functionSource.split('\n');
    return SourceMapConsumer.with(entry.map, null, (consumer) => {
      let selectedFunctionId = -1;
      let selectedPc = -1;
      let selectedPosition: LineCol | null = null;
      for (const functionInfo of entry.functions) {
        for (
          let entryIndex = 0;
          entryIndex < functionInfo.line_col.length;
          entryIndex++
        ) {
          const candidate = functionInfo.line_col[entryIndex];
          const pos = consumer.originalPositionFor({
            line: candidate.line,
            column: candidate.column,
          });
          if (
            pos.source && path.basename(pos.source) === file
            && pos.line === throwLine
            && entryIndex + 1 > selectedPc
          ) {
            selectedFunctionId = functionInfo.function_id;
            selectedPc = entryIndex + 1;
            selectedPosition = candidate;
          }
        }
      }
      if (selectedFunctionId < 0 || !selectedPosition) {
        throw new Error(
          `no mainThread pc maps to ${file}:${throwLine} for ${marker}`,
        );
      }
      const bytecodeStep = stepFromLines(
        'bytecode-debug-info',
        'main-thread.js',
        selectedPosition.line,
        selectedPosition.column + 1,
        genLines,
      );
      const pos = consumer.originalPositionFor({
        line: selectedPosition.line,
        column: selectedPosition.column,
      });
      const srcContent = pos.source
        ? consumer.sourceContentFor(pos.source, true)
        : null;
      const srcLines = srcContent ? srcContent.split('\n') : [];
      const sourceMapStep = stepFromLines(
        'source-map',
        pos.source
          ? (pos.source.includes('/src/')
            ? pos.source.slice(pos.source.indexOf('/src/') + 1)
            : pos.source)
          : file,
        pos.line ?? 0,
        (pos.column ?? 0) + 1,
        srcLines,
        pos.name ?? undefined,
      );
      return {
        release,
        path: entry.path,
        functionId: selectedFunctionId,
        pc: selectedPc,
        steps: [bytecodeStep, sourceMapStep],
      };
    });
  }
  throw new Error(`marker not found in any main-thread bundle: ${marker}`);
}
