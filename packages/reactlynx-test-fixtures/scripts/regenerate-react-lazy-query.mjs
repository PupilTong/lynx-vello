// Compile the checked-in source with the workspace's actual ReactLynx toolchain.
// Run from any directory after installing workspace dependencies.
import {createHash} from 'node:crypto';
import {createRequire} from 'node:module';
import {mkdtemp, mkdir, copyFile, readFile, readdir, writeFile, rm} from 'node:fs/promises';
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
const output = await mkdtemp(join(tmpdir(), 'bobcat-react-lazy-query-'));
try {
  const instance = await createRspeedy({cwd: packageRoot, loadEnv: false, rspeedyConfig: {
    plugins: [pluginReactLynx()], environments: {web: {}},
    source: {entry: {'react-lazy-query': join(here, 'react-lazy.jsx')}},
    output: {distPath: {root: output}, cleanDistPath: false},
  }});
  await instance.build();
  const destination = join(outputRoot, 'react-lazy-query');
  await rm(destination, {recursive: true, force: true});
  await mkdir(destination, {recursive: true});
  const bundles = {};
  for (const file of (await readdir(output, {recursive: true})).sort()) {
    if (!file.endsWith('.bundle')) continue;
    await mkdir(dirname(join(destination, file)), {recursive: true});
    await copyFile(join(output, file), join(destination, file));
    bundles[file] = createHash('sha256').update(await readFile(join(output, file))).digest('hex');
  }
  await writeFile(join(outputRoot, 'react-lazy-query.provenance.json'), JSON.stringify({
    sources: ['react-lazy.jsx', 'react-lazy-child.jsx', 'react-lazy-child.css'],
    target: 'web', lazyBundleFetcher: 'QueryComponent',
    description: 'Unmodified web page and lazy bundle from the default ReactLynx compiler configuration.',
    react: require('@lynx-js/react/package.json').version,
    rspeedy: require('@lynx-js/rspeedy/package.json').version,
    reactPlugin: require('@lynx-js/react-rsbuild-plugin/package.json').version,
    bundles,
  }, null, 2) + '\n');
} finally { await rm(output, {recursive: true, force: true}); }
