/** Infers device-calibrated engine frames from current background bundles. */
import { existsSync, readFileSync } from 'node:fs';

import type { MapEntry } from './remap-lib.js';

export type Engine = 'v8' | 'jsc' | 'primjs';
export const ENGINES: Engine[] = ['v8', 'jsc', 'primjs'];

export type ErrorKind = 'call' | 'read' | 'global';

type Anchor = 'start' | 'end' | 'call-end' | 'module-top';

function anchor(engine: Engine, err: ErrorKind): Anchor {
  if (engine === 'v8') return err === 'call' ? 'start' : 'end';
  if (engine === 'primjs') {
    if (err === 'call') return 'call-end';
    if (err === 'global') return 'module-top';
    return 'start';
  }
  return 'end';
}

function callExprEnd(line: string, openingParenIndex0: number): number {
  let parenthesisDepth = 0;
  let activeQuote: string | null = null;
  for (let i = openingParenIndex0; i < line.length; i++) {
    const character = line[i];
    if (activeQuote) {
      if (character === '\\') i++;
      else if (character === activeQuote) activeQuote = null;
      continue;
    }
    if (character === '"' || character === '\'' || character === '`') {
      activeQuote = character;
    } else if (character === '(') {
      parenthesisDepth++;
    } else if (character === ')' && --parenthesisDepth === 0) {
      return i + 1;
    }
  }
  throw new Error(`unbalanced call parens at ${openingParenIndex0}`);
}

export interface BgInfer {
  lineno: number;
  /** 1-based generated column the engine reports. */
  colno: number;
  release: string;
}

/** Locates the engine-reported frame for a failing token in a background bundle. */
export function inferBgFrame(
  find: string,
  token: string,
  err: ErrorKind,
  engine: Engine,
  index: Map<string, MapEntry>,
): BgInfer {
  const tokenOffsetInFind = find.indexOf(token);
  if (tokenOffsetInFind < 0) {
    throw new Error(`token (${token}) not in find (${find})`);
  }
  for (const [release, entry] of index) {
    if (entry.kind !== 'background') continue;
    let text: string;
    try {
      text = readFileSync(entry.jsFile, 'utf8');
    } catch {
      continue;
    }
    const lines = text.split('\n');
    for (let lineIndex = 0; lineIndex < lines.length; lineIndex++) {
      const line = lines[lineIndex];
      const findIndex = line.indexOf(find);
      if (findIndex < 0) continue;
      const tokenStartColumn0 = findIndex + tokenOffsetInFind;
      const tokenEndColumn0 = tokenStartColumn0 + token.length;
      const selectedAnchor = anchor(engine, err);
      let reportedColumn0 = tokenEndColumn0;
      if (selectedAnchor === 'start') {
        reportedColumn0 = tokenStartColumn0;
      } else if (selectedAnchor === 'call-end') {
        reportedColumn0 = callExprEnd(line, tokenEndColumn0);
      } else if (selectedAnchor === 'module-top') {
        reportedColumn0 = line.lastIndexOf('(');
      }
      return {
        lineno: lineIndex + 1,
        colno: reportedColumn0 + 1,
        release,
      };
    }
  }
  const scanned = [...index.entries()].map(([release, entry]) =>
    `  ${entry.kind} ${release.slice(0, 12)}… ${entry.jsFile} (exists: ${
      existsSync(entry.jsFile)
    })`
  );
  throw new Error(
    `find not located in any background bundle: ${find}\nscanned index:\n${
      scanned.join('\n')
    }`,
  );
}
