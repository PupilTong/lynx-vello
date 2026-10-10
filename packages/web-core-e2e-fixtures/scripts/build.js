// Builds every case group in turn.
//
// One Rsbuild run per group rather than one run with many environments: each
// group is a different compiler configuration, and `pluginReactLynx` takes its
// switches once per run. Groups write into the same `dist/`, so the order does
// not matter and a single group can be rebuilt on its own:
//
//     E2E_GROUP=default npx rsbuild build
import { spawn } from 'node:child_process';
import path from 'node:path';

import { groups } from '../groups.js';

const root = path.join(import.meta.dirname, '..');
const only = process.argv[2];
const selected = only ? groups.filter((group) => group.name === only) : groups;
if (selected.length === 0) {
  console.error(`no such group: ${only}`);
  console.error(`groups: ${groups.map((group) => group.name).join(', ')}`);
  process.exit(1);
}

for (const group of selected) {
  console.log(`\n=== ${group.name}`);
  const code = await new Promise((resolve, reject) => {
    const child = spawn('npx', ['rsbuild', 'build'], {
      stdio: 'inherit',
      cwd: root,
      shell: true,
      env: { ...process.env, E2E_GROUP: group.name, NODE_ENV: group.mode ?? 'production' },
    });
    child.on('error', reject);
    child.on('exit', resolve);
  });
  if (code !== 0) {
    console.error(`group ${group.name} failed with ${code}`);
    process.exit(code ?? 1);
  }
}
