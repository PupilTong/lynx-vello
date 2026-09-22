/** Runs full-flow remapping snapshots against current debug metadata. */
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { beforeAll, describe, expect, it } from '@rstest/core';

import { sections } from './cases.js';
import { computeFrame } from './frames.js';
import type { ComputedFrame } from './frames.js';
import type { Engine } from './infer.js';
import { buildMainThreadIndex } from './main-thread.js';
import type { MainThreadEntry } from './main-thread.js';
import { buildMapIndex } from './remap-lib.js';
import type { MapEntry, Step } from './remap-lib.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const example = path.resolve(here, '..');
const distDirs = [
  path.join(example, 'dist-producer'),
  path.join(example, 'dist-consumer'),
];

function normalizeEnvironmentTokens(value: string): string {
  return value
    .replace(/[0-9a-f]{40}/g, '<release>')
    .replace(/\.[0-9a-f]{8}\.js/g, '.<hash>.js')
    .replace(/http:\/\/[\d.]+:\d+/g, 'http://<host>');
}

function normalizeStepForSnapshot(step: Step): Step {
  if (step.kind === 'bytecode-debug-info') {
    return {
      ...step,
      lineno: -1,
      colno: -1,
      context_line: '<generated>',
      pre_context: [],
      post_context: [],
    };
  }
  return {
    ...step,
    context_line: step.context_line === undefined
      ? undefined
      : normalizeEnvironmentTokens(step.context_line),
    pre_context: step.pre_context.map(normalizeEnvironmentTokens),
    post_context: step.post_context.map(normalizeEnvironmentTokens),
  };
}

function normalizeFrameForSnapshot(frame: ComputedFrame): ComputedFrame {
  const normalizedRaw = normalizeEnvironmentTokens(frame.raw);
  return {
    code: frame.code,
    release: normalizeEnvironmentTokens(frame.release),
    raw: normalizedRaw.replace(/:\d+:\d+(\)?)$/, ':<loc>$1'),
    steps: frame.steps.map(normalizeStepForSnapshot),
  };
}

export function runEngine(engine: Engine): void {
  let bg: Map<string, MapEntry>;
  let mainThread: Map<string, MainThreadEntry>;
  beforeAll(() => {
    bg = buildMapIndex(distDirs);
    mainThread = buildMainThreadIndex(distDirs);
  });

  it('build products are present (run `pnpm test:build` first)', () => {
    expect([...bg.values()].some((e) => e.kind === 'background')).toBe(true);
  });

  for (const section of sections) {
    describe(section.name, () => {
      for (const testCase of section.cases) {
        it(testCase.name, async () => {
          expect(
            normalizeFrameForSnapshot(
              await computeFrame(testCase, engine, bg, mainThread),
            ),
          )
            .toMatchSnapshot();
        });
      }
    });
  }
}
