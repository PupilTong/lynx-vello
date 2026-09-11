// Compile the checked-in source with the workspace's actual ReactLynx toolchain.
// Run from any directory after installing workspace dependencies.
import {createRequire} from 'node:module';
import {mkdtemp, mkdir, copyFile, writeFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, dirname} from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';

const here = fileURLToPath(new URL('../src/', import.meta.url));
const outputRoot = fileURLToPath(new URL('../dist/', import.meta.url));
await mkdir(outputRoot, {recursive: true});
const packageRoot = fileURLToPath(new URL('../', import.meta.url));
const require = createRequire(new URL('../package.json', import.meta.url));
process.env.NODE_ENV = 'production';
const rspeedyRoot = dirname(require.resolve('@lynx-js/rspeedy/package.json'));
const reactPluginRoot = dirname(require.resolve('@lynx-js/react-rsbuild-plugin/package.json'));
const {createRspeedy} = await import(pathToFileURL(join(rspeedyRoot, 'dist/index.js')));
const {pluginReactLynx} = await import(pathToFileURL(join(reactPluginRoot, 'dist/index.js')));
const output = await mkdtemp(join(tmpdir(), 'bobcat-react-bts-query-'));
try {
  const instance = await createRspeedy({cwd: packageRoot, loadEnv: false, rspeedyConfig: {
    plugins: [pluginReactLynx()], environments: {web: {}},
    source: {entry: {'react-bts-query': join(here, 'react-bts-query.jsx')}},
    output: {distPath: {root: output}, cleanDistPath: false},
  }});
  await instance.build();
  await copyFile(join(output, 'react-bts-query.web.bundle'), join(outputRoot, 'react-bts-query.web.bundle'));
  await writeFile(join(outputRoot, 'react-bts-query.provenance.json'), JSON.stringify({
    source: 'react-bts-query.jsx', target: 'web',
    react: require('@lynx-js/react/package.json').version,
    rspeedy: require('@lynx-js/rspeedy/package.json').version,
    reactPlugin: require('@lynx-js/react-rsbuild-plugin/package.json').version,
  }, null, 2) + '\n');
} finally { await rm(output, {recursive: true, force: true}); }
