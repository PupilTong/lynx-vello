/** Declares crash cases in display order for remapping regression tests. */
import type { ErrorKind } from './infer.js';

export type Case =
  | { name: string; kind: 'bg'; err: ErrorKind; find: string; token: string }
  | { name: string; kind: 'main-thread'; marker: string };

export interface Section {
  name: string;
  cases: Case[];
}

export const sections: Section[] = [
  {
    name: '1 · LazyComponent (dynamic)',
    cases: [
      {
        name: 'L1. nested deep stack (dynamic, background)',
        kind: 'bg',
        err: 'call',
        find: 'Error("boom from deep nested call (LazyComponent, background)',
        token: 'Error',
      },
      {
        name: 'L2. TypeError (dynamic, background)',
        kind: 'bg',
        err: 'call',
        find: '.gone(',
        token: 'gone',
      },
      {
        name: 'L3. main-thread error (dynamic)',
        kind: 'main-thread',
        marker: 'boom from LazyComponent main-thread',
      },
      {
        name: 'L4. read .x of undefined (dynamic, background)',
        kind: 'bg',
        err: 'read',
        find: '(void 0).x',
        token: 'x',
      },
    ],
  },
  {
    name: '2 · Host (App.tsx)',
    cases: [
      {
        name: 'H1. throw new Error (host, background)',
        kind: 'bg',
        err: 'call',
        find: 'Error("explicit throw new Error (App.tsx host, background)',
        token: 'Error',
      },
      {
        name: 'H2. TypeError (host, background)',
        kind: 'bg',
        err: 'call',
        find: '.missing(',
        token: 'missing',
      },
      {
        name: 'H3. nested deep stack (host, background)',
        kind: 'bg',
        err: 'call',
        find: 'Error("boom from App.tsx deep nested call (host, background)',
        token: 'Error',
      },
      {
        name: 'H4. main-thread error (host)',
        kind: 'main-thread',
        marker: 'boom from App.tsx main-thread (host)',
      },
    ],
  },
  {
    name: '3 · CrashDemo (host)',
    cases: [
      {
        name: '1. TypeError (call undefined, background)',
        kind: 'bg',
        err: 'call',
        find: '.notAFunction(',
        token: 'notAFunction',
      },
      {
        name: '2. ReferenceError (background)',
        kind: 'bg',
        err: 'global',
        find: 'notDefinedVariable+1',
        token: 'notDefinedVariable',
      },
      {
        name: '3. throw new Error (background)',
        kind: 'bg',
        err: 'call',
        find: 'Error("explicit throw new Error (background)',
        token: 'Error',
      },
      {
        name: '4. nested deep stack (background)',
        kind: 'bg',
        err: 'call',
        find: 'Error("boom from deep nested call (background)',
        token: 'Error',
      },
      {
        name: '7. main-thread error',
        kind: 'main-thread',
        marker: 'boom from main-thread',
      },
    ],
  },
];
