// Keep separate compiler processes: NODE_ENV and the development HMR runtime
// are selected when the toolchain is imported.
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';

const generator = fileURLToPath(new URL('./regenerate-react-lazy.mjs', import.meta.url));
const builds = {
  production: ['react-lazy', 'react-lazy-sync', 'react-lazy-nested', 'react-reload', 'react-data-processor', 'react-global-props'],
  development: ['react-reload', 'react-global-props', 'react-lazy-nested'],
};
for (const [mode, fixtures] of Object.entries(builds)) {
  for (const fixture of fixtures) {
    const result = spawnSync(process.execPath, [generator, fixture, mode], {stdio: 'inherit'});
    if (result.error) throw result.error;
    if (result.status !== 0) throw Error(`${fixture} (${mode}) failed: ${result.signal ?? result.status}`);
  }
}
