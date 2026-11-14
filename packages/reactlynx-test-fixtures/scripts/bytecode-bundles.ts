// Native bundles whose JavaScript is PrimJS bytecode, for tests of the
// PrimJS-to-QuickJS bytecode converter (docs/primjs-bytecode-conversion-design.md).
// Run after both Rsbuild builds: it reads the `tasm.json` encoder inputs that
// `DEBUG=lynx` leaves in each native fixture's output directory.
//
// Writes, under dist/bytecode/ (removed and rebuilt on every run):
// - corpus/<program>/<variant>.lynx.bundle for every program in bytecode-corpus/,
//   with the program as the main-thread script (ROOT_LEPUS) and as the
//   background script `/app-service.js` (JS_BYTECODE);
// - fixtures/<fixture>/<name>.lynx.bundle, the page Rsbuild emitted before
//   scripts/source-bundles.ts replaced it (main thread bytecode, background
//   thread source), and <name>.js-bytecode.lynx.bundle, the same page with the
//   background thread compiled to bytecode as well;
// - fixtures/<fixture>/main-thread-bytecode/<chunk> and
//   fixtures/<fixture>/js-bytecode/<chunk> for each lazy bundle of a fixture,
//   at the path the fixture's provenance gives it: the lazy bundle's retained
//   `tasm.json` with its `main-thread` custom section encoded as `JsBytecode`,
//   and the same with `experimental_encodeQuickjsBytecode: true` as well;
// - cards/<card>/ for every card in bytecode-cards/: bytecode pages, any lazy
//   bundle the card fetches, the source twin of each under source/, and the
//   encoder inputs under tasm/;
// - index.rs, the registry `bytecode.rs` includes.
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { encode_napi, supportNapi } from '@lynx-js/tasm';

const packageRoot = fileURLToPath(new URL('../', import.meta.url));
const corpusRoot = join(packageRoot, 'bytecode-corpus');
const cardsRoot = join(packageRoot, 'bytecode-cards');
const dist = join(packageRoot, 'dist');
const output = join(dist, 'bytecode');

// The encoder's JavaScript fallback is a WebAssembly build that the package
// documents as nondeterministic when an instance is reused; the native addon
// exists for macOS and x64 Linux only.
if (!supportNapi()) {
  throw new Error(`@lynx-js/tasm has no native encoder for ${process.platform}-${process.arch}; `
    + 'the PrimJS bytecode fixtures can only be built on macOS or x64 Linux.');
}

// BinarySection values from lynx core/template_bundle/template_codec/template_binary.h.
const JS = 5;
const ROOT_LEPUS = 11;
const JS_BYTECODE = 14;
const LEPUS_CHUNK = 15;
const CUSTOM_SECTIONS = 16;
// CustomSectionEncodingType from template_binary.h: the `encoding` value a
// custom section's header carries; a header without one is STRING.
const CUSTOM_STRING = 0;
const CUSTOM_JS_BYTECODE = 1;
// `0xD8C54E17`, the packed bytecode header magic
// (lynx core/runtime/js/bytecode/quickjs/bytecode/quickjs_bytecode.h).
const PACKED_MAGIC = 0xd8c54e17;
// PrimJS header flags in the u64 after version byte 9 (bits 32 and 33).
const NEW_DEBUGINFO_FLAG = 1n << 32n;
const STRIP_VARINFO_FLAG = 1n << 33n;

interface Header {
  version: number;
  newDebugInfo: boolean;
  stripVarInfo: boolean;
}

interface Variant {
  // Bundle file stem, and the `Variant` name in bytecode.rs.
  file: string;
  rust: string;
  targetSdkVersion: string;
  debugInfoOutside: boolean;
  // What the encoder writes at the start of each section; checked on every build.
  mainThread: Header;
  background: Header;
}

// Every header form tasm 0.0.53 can write (the README lists what is not reachable).
const variants: Variant[] = [
  {
    file: 'sdk-2.13', rust: 'Sdk2_13', targetSdkVersion: '2.13', debugInfoOutside: true,
    mainThread: { version: 1, newDebugInfo: false, stripVarInfo: false },
    background: { version: 1, newDebugInfo: false, stripVarInfo: false },
  },
  {
    file: 'sdk-2.14', rust: 'Sdk2_14', targetSdkVersion: '2.14', debugInfoOutside: true,
    mainThread: { version: 9, newDebugInfo: true, stripVarInfo: false },
    background: { version: 9, newDebugInfo: true, stripVarInfo: false },
  },
  {
    file: 'sdk-4.1', rust: 'Sdk4_1', targetSdkVersion: '4.1', debugInfoOutside: true,
    mainThread: { version: 9, newDebugInfo: true, stripVarInfo: true },
    background: { version: 9, newDebugInfo: true, stripVarInfo: false },
  },
  {
    file: 'inline-debug', rust: 'InlineDebug', targetSdkVersion: '4.1', debugInfoOutside: false,
    mainThread: { version: 1, newDebugInfo: false, stripVarInfo: false },
    background: { version: 9, newDebugInfo: true, stripVarInfo: false },
  },
];

const sha256 = (value: Uint8Array) => createHash('sha256').update(value).digest('hex');

function encode(options: unknown): Buffer {
  // encode_napi throws on a non-zero status; the encoder also prints the
  // compiler's message to stdout.
  return encode_napi(options).buffer;
}

// Locates the section route of a flexible template (docs/lynx-binary-template.md):
// u8 10, u32 count, then count entries of (u8 type, u32 start, u32 end) with
// offsets relative to the end of the table. The route follows the header,
// whose template_info value is skipped by accepting only a table that
// describes contiguous sections, each opening with its own type byte, that end
// at the end of the file.
function sections(bundle: Buffer): Map<number, Buffer> {
  let offset = 8;
  for (let i = 0; i < 4; i++) offset += 4 + bundle.readUInt32LE(offset);
  offset += bundle.readUInt32LE(offset);
  for (let route = offset; route + 5 <= bundle.length; route++) {
    if (bundle[route] !== 10) continue;
    const count = bundle.readUInt32LE(route + 1);
    const base = route + 5 + count * 9;
    if (count === 0 || base > bundle.length) continue;
    const found = new Map<number, Buffer>();
    let expected = 0;
    for (let i = 0; i < count; i++) {
      const entry = route + 5 + i * 9;
      const type = bundle.readUInt8(entry);
      const start = bundle.readUInt32LE(entry + 1);
      const end = bundle.readUInt32LE(entry + 5);
      if (start !== expected || end < start || base + end > bundle.length || bundle[base + start] !== type) break;
      found.set(type, bundle.subarray(base + start, base + end));
      expected = end;
    }
    if (found.size === count && base + expected === bundle.length) return found;
  }
  throw new Error('No section route found in an encoded bundle');
}

function readHeader(bytecode: Buffer): Header {
  const version = bytecode.readUInt8(0);
  const word = version === 9 ? bytecode.readBigUInt64LE(1) : 0n;
  return { version, newDebugInfo: (word & NEW_DEBUGINFO_FLAG) !== 0n, stripVarInfo: (word & STRIP_VARINFO_FLAG) !== 0n };
}

// ROOT_LEPUS: u8 type, u64 length, bytecode.
function mainThreadBytecode(bundle: Buffer): Buffer {
  const section = sections(bundle).get(ROOT_LEPUS);
  if (!section) throw new Error('Bundle has no ROOT_LEPUS section');
  const length = Number(section.readBigUInt64LE(1));
  if (9 + length !== section.length) throw new Error('ROOT_LEPUS length does not match its section');
  return section.subarray(9);
}

// JS_BYTECODE: u8 type, u32 engine, u32 count, then per file u32-prefixed
// path, u64 length and the packed bytecode (24-byte header, then bytecode).
function backgroundBytecode(bundle: Buffer): Map<string, Buffer> {
  const section = sections(bundle).get(JS_BYTECODE);
  if (!section) throw new Error('Bundle has no JS_BYTECODE section');
  const files = new Map<string, Buffer>();
  let offset = 5;
  const count = section.readUInt32LE(offset);
  offset += 4;
  for (let i = 0; i < count; i++) {
    const pathLength = section.readUInt32LE(offset);
    const path = section.toString('utf8', offset + 4, offset + 4 + pathLength);
    offset += 4 + pathLength;
    const length = Number(section.readBigUInt64LE(offset));
    offset += 8;
    const packed = section.subarray(offset, offset + length);
    offset += length;
    if (packed.readUInt32LE(0) !== PACKED_MAGIC || packed.readUInt32LE(8) + packed.readUInt32LE(12) !== packed.length) {
      throw new Error(`JS_BYTECODE entry ${path} is not a packed bytecode buffer`);
    }
    files.set(path, packed.subarray(packed.readUInt32LE(8)));
  }
  if (offset !== section.length) throw new Error('JS_BYTECODE section has trailing bytes');
  return files;
}

// One Lepus value in header context (strings inline), as far as a custom
// section header needs: nil, double, bool, string, table, array, the integer
// types and undefined (lynx core/template_bundle/template_codec/binary_decoder/
// base_binary_reader.cc).
function skipValue(bytes: Buffer, offset: number): [unknown, number] {
  const tag = bytes[offset++];
  switch (tag) {
    case 0: case 17: return [undefined, offset];
    case 1: return [bytes.readDoubleLE(offset), offset + 8];
    case 2: return [bytes[offset] !== 0, offset + 1];
    case 3: {
      const length = bytes.readUInt32LE(offset);
      return [bytes.toString('utf8', offset + 4, offset + 4 + length), offset + 4 + length];
    }
    case 4: {
      const table: Record<string, unknown> = {};
      const count = bytes.readUInt32LE(offset);
      offset += 4;
      for (let i = 0; i < count; i++) {
        const length = bytes.readUInt32LE(offset);
        const key = bytes.toString('utf8', offset + 4, offset + 4 + length);
        [table[key], offset] = skipValue(bytes, offset + 4 + length);
      }
      return [table, offset];
    }
    case 5: {
      const array: unknown[] = [];
      const count = bytes.readUInt32LE(offset);
      offset += 4;
      for (let i = 0; i < count; i++) {
        let value: unknown;
        [value, offset] = skipValue(bytes, offset);
        array.push(value);
      }
      return [array, offset];
    }
    case 9: return [bytes.readInt32LE(offset), offset + 4];
    case 11: return [bytes.readUInt32LE(offset), offset + 4];
    case 10: case 12: return [Number(bytes.readBigUInt64LE(offset)), offset + 8];
    default: throw new Error(`Lepus value type ${tag} in a custom section header`);
  }
}

// CUSTOM_SECTIONS: u8 type, u32 count, then per section a u32-prefixed name,
// a header value, u32 start and u32 end (relative to the end of the headers).
// Answers each section's encoding by name.
function customSections(bundle: Buffer): Map<string, number> {
  const section = sections(bundle).get(CUSTOM_SECTIONS);
  const found = new Map<string, number>();
  if (!section) return found;
  let offset = 1;
  const count = section.readUInt32LE(offset);
  offset += 4;
  for (let i = 0; i < count; i++) {
    const length = section.readUInt32LE(offset);
    const name = section.toString('utf8', offset + 4, offset + 4 + length);
    let header: unknown;
    [header, offset] = skipValue(section, offset + 4 + length);
    offset += 8;
    const encoding = (header as { encoding?: unknown } | undefined)?.encoding;
    found.set(name, typeof encoding === 'number' ? encoding : CUSTOM_STRING);
  }
  return found;
}

// LEPUS_CHUNK: u8 type, u32 count, then per chunk a u32-prefixed name and its
// range. Answers the chunk names.
function lepusChunkNames(bundle: Buffer): string[] {
  const section = sections(bundle).get(LEPUS_CHUNK);
  if (!section) return [];
  const names: string[] = [];
  let offset = 1;
  const count = section.readUInt32LE(offset);
  offset += 4;
  for (let i = 0; i < count; i++) {
    const length = section.readUInt32LE(offset);
    names.push(section.toString('utf8', offset + 4, offset + 4 + length));
    offset += 4 + length + 8;
  }
  return names;
}

// What a test relies on in an encoded bundle: which top-level sections exist,
// the custom sections and their encodings, the Lepus chunk names and the
// JS_BYTECODE paths. Checked against what each output is meant to carry.
interface Layout {
  sections: number[];
  custom: Record<string, number>;
  lepusChunks: string[];
  jsBytecode: string[];
}

function layout(bundle: Buffer): Layout {
  const all = sections(bundle);
  return {
    sections: [...all.keys()].sort((a, b) => a - b),
    custom: Object.fromEntries(customSections(bundle)),
    lepusChunks: lepusChunkNames(bundle).sort(),
    jsBytecode: all.has(JS_BYTECODE) ? [...backgroundBytecode(bundle).keys()].sort() : [],
  };
}

function expectLayout(bundle: Buffer, expected: Partial<Layout> & { has?: number[]; lacks?: number[] }, what: string) {
  const actual = layout(bundle);
  const fail = (detail: string) => {
    throw new Error(`${what}: ${detail}; the encoder wrote ${JSON.stringify(actual)}`);
  };
  for (const type of expected.has ?? []) if (!actual.sections.includes(type)) fail(`no section ${type}`);
  for (const type of expected.lacks ?? []) if (actual.sections.includes(type)) fail(`a section ${type}`);
  const entries = (record: Record<string, number>) => JSON.stringify(Object.entries(record).sort());
  if (expected.custom && entries(actual.custom) !== entries(expected.custom)) fail('custom sections differ');
  if (expected.lepusChunks && JSON.stringify(actual.lepusChunks) !== JSON.stringify(expected.lepusChunks)) {
    fail('Lepus chunks differ');
  }
  if (expected.jsBytecode && JSON.stringify(actual.jsBytecode) !== JSON.stringify(expected.jsBytecode)) {
    fail('JS_BYTECODE files differ');
  }
}

function expectHeader(actual: Header, expected: Header, what: string) {
  if (actual.version !== expected.version || actual.newDebugInfo !== expected.newDebugInfo
    || actual.stripVarInfo !== expected.stripVarInfo) {
    throw new Error(`${what}: expected ${JSON.stringify(expected)}, the encoder wrote ${JSON.stringify(actual)}`);
  }
}

function encodeProgram(source: string, variant: Variant): Buffer {
  return encode({
    // The options a ReactLynx 4.1 page is encoded with (fiber architecture,
    // LepusNG), with the target SDK and debug-info placement of the variant.
    compilerOptions: {
      enableFiberArch: true,
      useLepusNG: true,
      bundleModuleMode: 'ReturnByFunction',
      templateDebugUrl: '',
      debugInfoOutside: variant.debugInfoOutside,
      targetSdkVersion: variant.targetSdkVersion,
      experimental_encodeQuickjsBytecode: true,
    },
    sourceContent: { dsl: 'react_nodiff', appType: 'card', config: {} },
    css: { cssMap: {}, cssSource: {} },
    lepusCode: { root: source },
    manifest: { '/app-service.js': source },
    customSections: {},
  });
}

// A lazy bundle's retained encoder input: its scripts are custom sections
// (`main-thread`, `background`, `CSS`), its manifest is empty.
interface LazyInput {
  compilerOptions: Record<string, unknown>;
  manifest: Record<string, string>;
  customSections: Record<string, { encoding?: string; content: unknown }>;
}

// One card of bytecode-cards/<name>/. File names are relative to that directory.
interface CardSpec {
  name: string;
  // The main-thread root script (ROOT_LEPUS).
  root: string;
  // Lepus chunk name -> file (LEPUS_CHUNK entries, loaded by `__LoadLepusChunk`).
  lepusChunks?: Record<string, string>;
  // Custom section name -> file, encoded as `JsBytecode`.
  jsBytecodeSections?: Record<string, string>;
  // Manifest path -> file (the background thread: JS, or JS_BYTECODE with the flag).
  manifest?: Record<string, string>;
  // Lazy bundle path, relative to the page -> file of its `main-thread` section.
  lazy?: Record<string, string>;
  // The console lines the bytecode forms log, in order, once the engine runs them.
  expected: string[];
  // The console lines the source twin logs, when they differ.
  sourceExpected?: string[];
}

const cards: CardSpec[] = [
  {
    name: 'chunk-twice',
    root: 'root.js',
    lepusChunks: { counter: 'counter.js' },
    expected: ['chunk-twice true,true run1,run2'],
  },
  {
    name: 'custom-section-bytecode',
    root: 'root.js',
    jsBytecodeSections: { 'answer__main-thread': 'answer.js' },
    expected: ['custom-section-bytecode answer:42'],
  },
  {
    name: 'refused-lazy',
    root: 'root.js',
    lazy: { 'lazy-bundle/refused.lynx.bundle': 'main-thread.js' },
    // Followed by `refused-lazy error_msg <the host's reason>` first.
    expected: ['refused-lazy code:-1'],
    sourceExpected: ['refused-lazy loaded 1'],
  },
  {
    name: 'background-bytecode-min',
    root: 'root.js',
    manifest: { '/app-service.js': 'app-service.js' },
    expected: ['background-bytecode-min main', 'background-bytecode-min background 2,4,6'],
  },
];

// The options the cards are encoded with: a fiber-architecture LepusNG page
// targeting 4.1.0, the engine version the ReactLynx fixtures are built for.
const cardCompilerOptions = {
  enableFiberArch: true,
  useLepusNG: true,
  bundleModuleMode: 'ReturnByFunction',
  templateDebugUrl: '',
  debugInfoOutside: true,
  targetSdkVersion: '4.1.0',
  experimental_encodeQuickjsBytecode: false,
};

const rust = (value: string) => JSON.stringify(value);
const programs: string[] = [];
const fixtures: string[] = [];
const cardEntries: string[] = [];

await rm(output, { recursive: true, force: true });

for (const file of (await readdir(corpusRoot)).filter(name => name.endsWith('.js')).sort()) {
  const name = file.slice(0, -'.js'.length);
  const source = await readFile(join(corpusRoot, file), 'utf8');
  await mkdir(join(output, 'corpus', name), { recursive: true });
  const bundles: string[] = [];
  for (const variant of variants) {
    let bundle: Buffer;
    try {
      bundle = encodeProgram(source, variant);
    } catch (error) {
      throw new Error(`bytecode-corpus/${file} (${variant.file}): ${(error as Error).message}`);
    }
    expectHeader(readHeader(mainThreadBytecode(bundle)), variant.mainThread, `${file} ${variant.file} ROOT_LEPUS`);
    const background = backgroundBytecode(bundle).get('/app-service.js');
    if (!background) throw new Error(`${file} ${variant.file}: JS_BYTECODE lacks /app-service.js`);
    expectHeader(readHeader(background), variant.background, `${file} ${variant.file} JS_BYTECODE`);
    const path = `corpus/${name}/${variant.file}.lynx.bundle`;
    await writeFile(join(output, path), bundle);
    bundles.push(`(Variant::${variant.rust}, include_bytes!(${rust(path)}))`);
  }
  programs.push(`    Program {
        name: ${rust(name)},
        source: include_str!(${rust(`../../bytecode-corpus/${file}`)}),
        bundles: &[${bundles.join(', ')}],
    },`);
}

for (const file of (await readdir(dist)).filter(name => name.endsWith('.provenance.json')).sort()) {
  const fixture = file.slice(0, -'.provenance.json'.length);
  const provenance: { target: string; nativePageSha256?: string; bundles: Record<string, string> } =
    JSON.parse(await readFile(join(dist, file), 'utf8'));
  if (provenance.target !== 'lynx') continue;
  // The page is `<entry>.lynx.bundle` at the top of the output directory, and
  // its encoder input is `.lynx/<entry>/tasm.json` (lazy bundles have theirs
  // under `.lynx/lazy-bundle/`).
  const page = Object.keys(provenance.bundles).find(path => !path.includes('/'));
  const entry = page?.replace(/\.lynx\.bundle$/, '');
  if (!page || entry === page || !entry) throw new Error(`${fixture}: no native page bundle in its provenance`);
  const options: { compilerOptions: Record<string, unknown> } =
    JSON.parse(await readFile(join(dist, fixture, '.lynx', entry, 'tasm.json'), 'utf8'));
  // The retained input reproduces the page Rsbuild emitted, byte for byte.
  const emitted = encode(options);
  if (sha256(emitted) !== provenance.nativePageSha256) {
    throw new Error(`${fixture}: re-encoding .lynx/${entry}/tasm.json does not reproduce the emitted page`);
  }
  mainThreadBytecode(emitted);
  const both = encode({ ...options, compilerOptions: { ...options.compilerOptions, experimental_encodeQuickjsBytecode: true } });
  mainThreadBytecode(both);
  if (backgroundBytecode(both).size === 0) throw new Error(`${fixture}: JS_BYTECODE is empty`);
  await mkdir(join(output, 'fixtures', fixture), { recursive: true });
  const pagePath = `fixtures/${fixture}/${entry}.lynx.bundle`;
  const bothPath = `fixtures/${fixture}/${entry}.js-bytecode.lynx.bundle`;
  await writeFile(join(output, pagePath), emitted);
  await writeFile(join(output, bothPath), both);

  // Lazy bundles: each retained input must reproduce one lazy bundle the
  // fixture emitted, which gives the path the bytecode forms are written at.
  const chunkPaths = Object.keys(provenance.bundles).filter(path => path.includes('/'));
  const lazyRoot = join(dist, fixture, '.lynx', 'lazy-bundle');
  const lazyInputs = existsSync(lazyRoot) ? (await readdir(lazyRoot)).sort() : [];
  const mainThreadChunks: string[] = [];
  const bothChunks: string[] = [];
  for (const name of lazyInputs) {
    const text = await readFile(join(lazyRoot, name, 'tasm.json'), 'utf8');
    const input: LazyInput = JSON.parse(text);
    const emittedHash = sha256(encode(JSON.parse(text)));
    const path = chunkPaths.find(chunk => provenance.bundles[chunk] === emittedHash);
    if (!path) throw new Error(`${fixture}: re-encoding .lynx/lazy-bundle/${name}/tasm.json reproduces no lazy bundle`);
    // What a production build writes (`enableSectionBytecode`, on outside
    // development and DEBUG): the `main-thread` section as `JsBytecode`.
    // Every other custom section is kept as it is.
    const custom = Object.fromEntries(Object.entries(input.customSections).map(([section, value]) =>
      [section, section === 'main-thread' ? { ...value, encoding: 'JsBytecode' } : value]));
    const mainThreadInput = { ...input, customSections: custom };
    const bothInput = {
      ...mainThreadInput,
      compilerOptions: { ...input.compilerOptions, experimental_encodeQuickjsBytecode: true },
    };
    const mainThread = encode(structuredClone(mainThreadInput));
    const bothForms = encode(structuredClone(bothInput));
    const encodings = Object.fromEntries(Object.entries(custom).map(([section, value]) => [
      section,
      value.encoding === 'JsBytecode' ? CUSTOM_JS_BYTECODE : value.encoding === 'CSS' ? 2 : CUSTOM_STRING,
    ]));
    // The background code stays a string custom section in both forms: the
    // encoder compiles only manifest files to JS_BYTECODE, and a lazy
    // bundle's manifest is empty.
    expectLayout(mainThread, { custom: encodings, lepusChunks: [], lacks: [JS_BYTECODE] }, `${fixture} ${path}`);
    expectLayout(bothForms, { custom: encodings, lepusChunks: [], jsBytecode: Object.keys(input.manifest).sort(), lacks: [JS] },
      `${fixture} ${path} js-bytecode`);
    const mainThreadPath = `fixtures/${fixture}/main-thread-bytecode/${path}`;
    const bothPathLazy = `fixtures/${fixture}/js-bytecode/${path}`;
    await mkdir(dirname(join(output, mainThreadPath)), { recursive: true });
    await mkdir(dirname(join(output, bothPathLazy)), { recursive: true });
    await writeFile(join(output, mainThreadPath), mainThread);
    await writeFile(join(output, bothPathLazy), bothForms);
    mainThreadChunks.push(`(${rust(path)}, include_bytes!(${rust(mainThreadPath)}))`);
    bothChunks.push(`(${rust(path)}, include_bytes!(${rust(bothPathLazy)}))`);
  }
  if (mainThreadChunks.length !== chunkPaths.length) {
    throw new Error(`${fixture}: ${chunkPaths.length} lazy bundles, ${mainThreadChunks.length} retained inputs`);
  }
  fixtures.push(`    BytecodeFixture {
        name: ${rust(fixture)},
        page: include_bytes!(${rust(pagePath)}),
        js_bytecode_page: include_bytes!(${rust(bothPath)}),
        chunks: &[${mainThreadChunks.join(', ')}],
        js_bytecode_chunks: &[${bothChunks.join(', ')}],
    },`);
}

for (const card of cards) {
  const read = (file: string) => readFile(join(cardsRoot, card.name, file), 'utf8');
  const root = await read(card.root);
  const lepusChunk: Record<string, string> = {};
  for (const [name, file] of Object.entries(card.lepusChunks ?? {})) lepusChunk[name] = await read(file);
  const bytecodeSections: Record<string, string> = {};
  for (const [name, file] of Object.entries(card.jsBytecodeSections ?? {})) bytecodeSections[name] = await read(file);
  const manifest: Record<string, string> = {};
  for (const [path, file] of Object.entries(card.manifest ?? {})) manifest[path] = await read(file);

  // The bytecode page: the root as ROOT_LEPUS, each Lepus chunk as a
  // LEPUS_CHUNK entry, each listed custom section as `JsBytecode`, and the
  // manifest as JS (source) or, with the flag, JS_BYTECODE.
  const pageInput = (jsBytecode: boolean) => ({
    compilerOptions: { ...cardCompilerOptions, experimental_encodeQuickjsBytecode: jsBytecode },
    sourceContent: { dsl: 'react_nodiff', appType: 'card', config: {} },
    css: { cssMap: {}, cssSource: {} },
    lepusCode: { root, lepusChunk },
    manifest,
    customSections: Object.fromEntries(Object.entries(bytecodeSections)
      .map(([name, content]) => [name, { encoding: 'JsBytecode', content }])),
  });
  // The source twin, as scripts/source-bundles.ts repacks a page: every body
  // a string custom section, the root as `<card>__main-thread`, a Lepus chunk
  // `n` as `n__main-thread`, a manifest path without its leading `/`.
  const sourceEntry = `${card.name}__main-thread`;
  const sourceSections: Record<string, { content: string }> = { [sourceEntry]: { content: root } };
  for (const [name, content] of Object.entries(lepusChunk)) sourceSections[`${name}__main-thread`] = { content };
  for (const [name, content] of Object.entries(bytecodeSections)) sourceSections[name] = { content };
  for (const [path, content] of Object.entries(manifest)) sourceSections[path.replace(/^\//, '')] = { content };
  const sourceInput = {
    compilerOptions: { ...cardCompilerOptions },
    sourceContent: { dsl: 'react_nodiff', appType: 'DynamicComponent', config: {} },
    customSections: sourceSections,
  };

  const dir = `cards/${card.name}`;
  const pagePath = `${dir}/${card.name}.lynx.bundle`;
  const bothPath = `${dir}/${card.name}.js-bytecode.lynx.bundle`;
  const sourcePath = `${dir}/source/${card.name}.lynx.bundle`;
  const files: [string, unknown][] = [[pagePath, pageInput(false)], [bothPath, pageInput(true)], [sourcePath, sourceInput]];
  const lazy: [string, string][] = [];
  for (const [path, file] of Object.entries(card.lazy ?? {})) {
    const content = await read(file);
    const lazyInput = (bytecode: boolean) => ({
      compilerOptions: { ...cardCompilerOptions },
      sourceContent: { dsl: 'react_nodiff', appType: 'DynamicComponent', config: {} },
      css: { cssMap: {}, cssSource: {} },
      manifest: {},
      customSections: { 'main-thread': bytecode ? { encoding: 'JsBytecode', content } : { content } },
    });
    files.push([`${dir}/${path}`, lazyInput(true)], [`${dir}/source/${path}`, lazyInput(false)]);
    lazy.push([path, file]);
  }
  const written = new Map<string, Buffer>();
  for (const [path, input] of files) {
    const bundle = encode(structuredClone(input));
    const tasm = `${dir}/tasm/${path.slice(dir.length + 1).replace(/\.lynx\.bundle$/, '').replaceAll('/', '_')}.json`;
    await mkdir(dirname(join(output, path)), { recursive: true });
    await mkdir(dirname(join(output, tasm)), { recursive: true });
    await writeFile(join(output, path), bundle);
    await writeFile(join(output, tasm), JSON.stringify(input, null, 2) + '\n');
    written.set(path, bundle);
  }

  const bytecodeCustom = Object.fromEntries(Object.keys(bytecodeSections).map(name => [name, CUSTOM_JS_BYTECODE]));
  const lepusChunks = Object.keys(lepusChunk).sort();
  const manifestPaths = Object.keys(manifest).sort();
  expectLayout(written.get(pagePath)!, { has: [ROOT_LEPUS], lacks: [JS_BYTECODE], custom: bytecodeCustom, lepusChunks },
    `${card.name} page`);
  if (manifestPaths.length > 0) expectLayout(written.get(pagePath)!, { has: [JS] }, `${card.name} page`);
  expectLayout(written.get(bothPath)!, { has: [ROOT_LEPUS, JS_BYTECODE], lacks: [JS], custom: bytecodeCustom, lepusChunks,
    jsBytecode: manifestPaths }, `${card.name} js-bytecode page`);
  expectLayout(written.get(sourcePath)!, {
    lacks: [JS_BYTECODE, LEPUS_CHUNK],
    custom: Object.fromEntries(Object.keys(sourceSections).map(name => [name, CUSTOM_STRING])),
  }, `${card.name} source page`);
  for (const [path] of lazy) {
    expectLayout(written.get(`${dir}/${path}`)!, { custom: { 'main-thread': CUSTOM_JS_BYTECODE }, lacks: [JS_BYTECODE] },
      `${card.name} ${path}`);
    expectLayout(written.get(`${dir}/source/${path}`)!, { custom: { 'main-thread': CUSTOM_STRING } },
      `${card.name} source ${path}`);
  }

  const lines = (values: string[]) => `&[${values.map(rust).join(', ')}]`;
  const chunkList = (prefix: string) =>
    `&[${lazy.map(([path]) => `(${rust(path)}, include_bytes!(${rust(`${prefix}${path}`)}))`).join(', ')}]`;
  cardEntries.push(`    Card {
        name: ${rust(card.name)},
        page: include_bytes!(${rust(pagePath)}),
        js_bytecode_page: include_bytes!(${rust(bothPath)}),
        chunks: ${chunkList(`${dir}/`)},
        expected: ${lines(card.expected)},
        source: SourceCard {
            page: include_bytes!(${rust(sourcePath)}),
            entry: ${rust(sourceEntry)},
            lepus_chunks: &[${Object.keys(lepusChunk).map(name => `(${rust(`${name}__main-thread`)}, ${rust(name)})`).join(', ')}],
            chunks: ${chunkList(`${dir}/source/`)},
            expected: ${lines(card.sourceExpected ?? card.expected)},
        },
    },`);
}

await writeFile(join(output, 'index.rs'), `// Generated by scripts/bytecode-bundles.ts. Do not commit.
#[rustfmt::skip]
pub static PROGRAMS: &[Program] = &[
${programs.join('\n')}
];

#[rustfmt::skip]
pub static FIXTURES: &[BytecodeFixture] = &[
${fixtures.join('\n')}
];

#[rustfmt::skip]
pub static CARDS: &[Card] = &[
${cardEntries.join('\n')}
];
`);

console.log(`bytecode bundles: ${programs.length} corpus programs x ${variants.length} variants, `
  + `${fixtures.length} fixtures, ${cardEntries.length} cards`);
