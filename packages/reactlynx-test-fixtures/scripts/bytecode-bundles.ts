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
// - index.rs, the registry `bytecode.rs` includes.
import { createHash } from 'node:crypto';
import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { encode_napi, supportNapi } from '@lynx-js/tasm';

const packageRoot = fileURLToPath(new URL('../', import.meta.url));
const corpusRoot = join(packageRoot, 'bytecode-corpus');
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
const ROOT_LEPUS = 11;
const JS_BYTECODE = 14;
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

const rust = (value: string) => JSON.stringify(value);
const programs: string[] = [];
const fixtures: string[] = [];

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
  fixtures.push(`    BytecodeFixture {
        name: ${rust(fixture)},
        page: include_bytes!(${rust(pagePath)}),
        js_bytecode_page: include_bytes!(${rust(bothPath)}),
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
`);

console.log(`bytecode bundles: ${programs.length} corpus programs x ${variants.length} variants, ${fixtures.length} fixtures`);
