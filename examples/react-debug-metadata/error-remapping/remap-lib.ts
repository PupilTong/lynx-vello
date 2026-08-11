/** Reimplements biz_sourcemap reversal for artifact-backed snapshot tests. */
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';

import { SourceMapConsumer } from 'source-map';
import type {
  BasicSourceMapConsumer,
  IndexedSourceMapConsumer,
  MappingItem,
  RawSourceMap,
} from 'source-map';

export interface MapEntry {
  kind: string;
  /** Bundle path containing the content hash. */
  path: string;
  /** Absolute emitted bundle path. */
  jsFile: string;
  map: RawSourceMap;
}

/** One reversal step, matching biz_sourcemap's RemapStep JSON. */
export interface Step {
  kind: string;
  filename: string;
  lineno: number;
  colno: number;
  /** Original function name when present in the mapping. */
  function_name?: string;
  context_line?: string;
  pre_context: string[];
  post_context: string[];
}

const CONTEXT_LINES = 5;
const MAX_CONTEXT_LEN = 200;

function clip(line: string): string {
  return line.length > MAX_CONTEXT_LEN
    ? `${line.slice(0, MAX_CONTEXT_LEN)} [+${
      line.length - MAX_CONTEXT_LEN
    } chars]`
    : line;
}

function normalizeSource(source: string): string {
  const sourceRootIndex = source.indexOf('/src/');
  if (sourceRootIndex >= 0) return source.slice(sourceRootIndex + 1);
  return source.replace('webpack:///./', 'webpack:///');
}

function sliceContext(
  lines: string[],
  line1: number,
): Pick<Step, 'context_line' | 'pre_context' | 'post_context'> {
  const cl = lines[line1 - 1];
  return {
    context_line: cl == null ? undefined : clip(cl),
    pre_context: lines.slice(Math.max(0, line1 - 1 - CONTEXT_LINES), line1 - 1)
      .map((l) => clip(l)),
    post_context: lines.slice(line1, line1 + CONTEXT_LINES).map((l) => clip(l)),
  };
}

function greatestLowerBound(
  consumer: BasicSourceMapConsumer | IndexedSourceMapConsumer,
  genLine: number,
  genCol0: number,
): MappingItem | null {
  let closestMapping: MappingItem | null = null;
  consumer.eachMapping(
    (m) => {
      const atOrBefore = m.generatedLine < genLine
        || (m.generatedLine === genLine && m.generatedColumn <= genCol0);
      if (!atOrBefore) return;
      if (
        closestMapping === null
        || m.generatedLine > closestMapping.generatedLine
        || (m.generatedLine === closestMapping.generatedLine
          && m.generatedColumn > closestMapping.generatedColumn)
      ) {
        closestMapping = m;
      }
    },
    null,
    SourceMapConsumer.GENERATED_ORDER,
  );
  return closestMapping;
}

/** Resolves a generated position with backend-compatible lower-bound ties. */
export async function resolveStep(
  map: RawSourceMap,
  genLine: number,
  genCol0: number,
): Promise<Step | null> {
  return SourceMapConsumer.with(map, null, (consumer) => {
    const m = greatestLowerBound(consumer, genLine, Math.max(0, genCol0));
    if (m === null || !m.source) return null;
    const content = consumer.sourceContentFor(m.source, true);
    const lines = content ? content.split('\n') : [];
    const step: Step = {
      kind: 'source-map',
      filename: normalizeSource(m.source),
      lineno: m.originalLine,
      colno: m.originalColumn + 1,
      ...sliceContext(lines, m.originalLine),
    };
    if (m.name) step.function_name = m.name;
    return step;
  });
}

/** Builds a remapping step from source lines and a one-based position. */
export function stepFromLines(
  kind: string,
  filename: string,
  line1: number,
  col1: number,
  lines: string[],
  functionName?: string,
): Step {
  const step: Step = {
    kind,
    filename,
    lineno: line1,
    colno: col1,
    ...sliceContext(lines, line1),
  };
  if (functionName) step.function_name = functionName;
  return step;
}

function walk(dir: string, cb: (file: string) => void): void {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(p, cb);
    else cb(p);
  }
}

/** Indexes artifact source maps by release key. */
export function buildMapIndex(distDirs: string[]): Map<string, MapEntry> {
  const index = new Map<string, MapEntry>();
  for (const dir of distDirs) {
    if (!existsSync(dir)) continue;
    walk(dir, (file) => {
      if (path.basename(file) !== 'debug-metadata.json') return;
      const meta = JSON.parse(readFileSync(file, 'utf8')) as {
        artifacts?: {
          kind: string;
          path?: string;
          debugSources?: {
            kind: string;
            key?: string;
            map?: string | RawSourceMap;
          }[];
        }[];
      };
      for (const artifact of meta.artifacts ?? []) {
        for (const ds of artifact.debugSources ?? []) {
          if (ds.kind === 'source-map' && ds.key && ds.map) {
            const map = typeof ds.map === 'string'
              ? JSON.parse(ds.map) as RawSourceMap
              : ds.map;
            index.set(ds.key, {
              kind: artifact.kind,
              path: artifact.path ?? '',
              jsFile: path.join(dir, artifact.path ?? ''),
              map,
            });
          }
        }
      }
    });
  }
  return index;
}
