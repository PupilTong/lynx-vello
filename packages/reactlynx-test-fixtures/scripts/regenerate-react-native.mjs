// Repack real compiler output into the source-only native external format.
// Historical fixture: build the example first with `pnpm build:legacy-native`.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { createHash } from 'node:crypto';
const root = new URL('../../../', import.meta.url);
const require = createRequire(new URL('../package.json', import.meta.url));
const { decode_napi, encode } = require('@lynx-js/tasm');
const output = new URL('../dist/', import.meta.url);
mkdirSync(output, {recursive: true});
const native = readFileSync(new URL('examples/react/dist/main.lynx.bundle', root));
const web = readFileSync(new URL('examples/react/dist/main.web.bundle', root));
const decoded = decode_napi(native);
let main;
for (let p = 12; p < web.length;) {
  const type = web.readUInt32LE(p), length = web.readUInt32LE(p + 4);
  p += 8;
  const section = web.subarray(p, p + length); p += length;
  if (type !== 3) continue;
  let i = 4;
  for (let count = section.readUInt32LE(0); count--;) {
    let length = section.readUInt32LE(i); i += 4;
    const name = section.subarray(i, i + length).toString(); i += length;
    length = section.readUInt32LE(i); i += 4;
    const content = section.subarray(i, i + length).toString(); i += length;
    if (name === 'root') main = content;
  }
}
if (!main) throw Error('web bundle has no source MTS root');
const customSections = {'react__main-thread': {content: main}};
for (const {path, type, content} of decoded['background-thread-script']) {
  if (type !== 'source') throw Error('BTS bytecode is outside this fixture');
  customSections[path.replace(/^\//, '')] = {content};
}
const options = {
  compilerOptions: {enableFiberArch:true, useLepusNG:true, targetSdkVersion:'3.5', enableCSSInvalidation:true, enableCSSSelector:true, debugInfoOutside:true},
  sourceContent: {appType:'DynamicComponent'}, customSections,
};
const {buffer} = await encode(options);
writeFileSync(new URL('react-native.lynx.bundle', output), buffer);
const sha256 = value => createHash('sha256').update(value).digest('hex');
writeFileSync(new URL('react-native.provenance.json', output), JSON.stringify({
  description: 'Real native BTS source and the corresponding web MTS source, repacked without styles/images or MTS bytecode; this is not execution of the original native bytecode card.',
  encoder: `@lynx-js/tasm@${require('@lynx-js/tasm/package.json').version}`, reactLynx: '@lynx-js/react@0.123.0', wrapper: '@lynx-js/runtime-wrapper-webpack-plugin@0.2.2',
  inputNativeSha256:sha256(native), inputWebSha256:sha256(web),
  scripts:Object.fromEntries(Object.entries(customSections).map(([name,{content}])=>[name,{bytes:Buffer.byteLength(content),sha256:sha256(content)}])),
}, null, 2)+'\n');
