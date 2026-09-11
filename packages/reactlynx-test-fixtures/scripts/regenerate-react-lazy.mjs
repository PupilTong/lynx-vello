// Build native source fixtures without rewriting framework sources.
// DEBUG retains source in lazy MTS sections. Only the page's MTS bytecode is
// replaced with its original compiler source in a named external section.
import {createRequire} from 'node:module';
import {createHash} from 'node:crypto';
import {mkdtemp, mkdir, copyFile, readFile, readdir, writeFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, dirname} from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';

const here = fileURLToPath(new URL('../src/', import.meta.url));
const outputRoot = fileURLToPath(new URL('../dist/', import.meta.url));
await mkdir(outputRoot, {recursive: true});
// Latest stable Lynx release, verified on 2026-09-11. Other plugin options
// retain their defaults; this version selects the FetchBundle lazy runtime.
const engineVersion = '4.1.0';
const fixture = process.argv[2] ?? 'react-lazy';
if (!['react-lazy', 'react-lazy-sync', 'react-lazy-nested', 'react-reload', 'react-data-processor', 'react-global-props'].includes(fixture)) throw Error('unknown native fixture');
const mode = process.argv[3] ?? 'production';
if (!['production', 'development'].includes(mode)) throw Error('unknown build mode');
const outputName = mode === 'development' ? `${fixture}-development` : fixture;
const packageRoot = fileURLToPath(new URL('../', import.meta.url));
const require = createRequire(new URL('../package.json', import.meta.url));
const {encode} = require('@lynx-js/tasm');
process.env.NODE_ENV = mode;
process.env.DEBUG = 'rspeedy';
const rspeedyRoot = dirname(require.resolve('@lynx-js/rspeedy/package.json'));
const reactPluginRoot = dirname(require.resolve('@lynx-js/react-rsbuild-plugin/package.json'));
const {createRspeedy} = await import(pathToFileURL(join(rspeedyRoot, 'dist/index.js')));
const {pluginReactLynx} = await import(pathToFileURL(join(reactPluginRoot, 'dist/index.js')));
const output = await mkdtemp(join(tmpdir(), 'bobcat-react-lazy-'));
try {
  const instance = await createRspeedy({cwd: packageRoot, loadEnv: false, rspeedyConfig: {
    mode,
    plugins: [pluginReactLynx({engineVersion})], environments: {lynx: {}},
    source: {entry: {[fixture]: join(here, `${fixture}.jsx`)}},
    output: {distPath: {root: output}, cleanDistPath: false},
  }});
  await instance.build();
  const destination = join(outputRoot, outputName);
  await rm(destination, {recursive: true, force: true});
  await mkdir(destination, {recursive: true});
  const bundles = {};
  for (const file of (await readdir(output, {recursive: true})).sort()) {
    if (!file.startsWith('async/') || !file.endsWith('.bundle')) continue;
    await mkdir(dirname(join(destination, file)), {recursive: true});
    await copyFile(join(output, file), join(destination, file));
    bundles[file] = createHash('sha256').update(await readFile(join(output, file))).digest('hex');
  }
  const options = JSON.parse(await readFile(join(output, `.rspeedy/${fixture}/tasm.json`), 'utf8'));
  const publicPathMatch = options.lepusCode.root.match(/__webpack_require__\.p\s*=\s*("(?:[^"\\]|\\.)*")/);
  if (mode === 'development' && !publicPathMatch) throw Error('compiled page public path was not found');
  const customSections = {[`${fixture}__main-thread`]: {content: options.lepusCode.root}};
  for (const [path, content] of Object.entries(options.manifest)) {
    customSections[path.replace(/^\//, '')] = {content};
  }
  const {buffer} = await encode({
    compilerOptions: options.compilerOptions,
    sourceContent: {...options.sourceContent, appType: 'DynamicComponent'},
    customSections,
  });
  await writeFile(join(destination, `${fixture}.lynx.bundle`), buffer);
  bundles[`${fixture}.lynx.bundle`] = createHash('sha256').update(buffer).digest('hex');
  await writeFile(join(outputRoot, `${outputName}.provenance.json`), JSON.stringify({
    sources: ['react-reload', 'react-data-processor', 'react-global-props'].includes(fixture) ? [`${fixture}.jsx`] : fixture === 'react-lazy-nested'
      ? [`${fixture}.jsx`, `${fixture}-outer.jsx`, `${fixture}-outer.css`, `${fixture}-inner.jsx`, `${fixture}-value.js`]
      : [`${fixture}.jsx`, 'react-lazy-child.jsx', 'react-lazy-child.css'],
    target: 'lynx', mode, engineVersion, lazyBundleFetcher: 'FetchBundle',
    publicPath: publicPathMatch ? JSON.parse(publicPathMatch[1]) : null,
    description: 'Page repacked from the native compiler sources into source-only external sections; no bytecode execution. Any emitted lazy bundles retain their unmodified source MTS (DEBUG=rspeedy).',
    encoder: `@lynx-js/tasm@${require('@lynx-js/tasm/package.json').version}`,
    react: require('@lynx-js/react/package.json').version,
    rspeedy: require('@lynx-js/rspeedy/package.json').version,
    reactPlugin: require('@lynx-js/react-rsbuild-plugin/package.json').version,
    nativePageSha256: createHash('sha256').update(await readFile(join(output, `${fixture}.lynx.bundle`))).digest('hex'),
    scripts: Object.fromEntries(Object.entries(customSections).map(([name, {content}]) => [name, createHash('sha256').update(content).digest('hex')])),
    bundles,
  }, null, 2) + '\n');
} finally { await rm(output, {recursive: true, force: true}); }
