import { createHash } from 'node:crypto';
import { readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { basename, dirname, join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { RsbuildPlugin } from '@lynx-js/rspeedy';
import { encode } from '@lynx-js/tasm';

const require = createRequire(import.meta.url);
const sourceRoot = fileURLToPath(new URL('../src/', import.meta.url));
const sha256 = (value: string | Uint8Array) => createHash('sha256').update(value).digest('hex');
const version = (name: string): string => require(`${name}/package.json`).version;

interface CompilerSources {
  compilerOptions: Record<string, unknown>;
  sourceContent: Record<string, unknown>;
  lepusCode: { root: string };
  manifest: Record<string, string>;
}

// Rspeedy owns compilation. This hook only adapts emitted native pages for
// Bobcat's source evaluator and records what each selected environment built.
export function pluginSourceBundles(mode: string, engineVersion: string): RsbuildPlugin {
  return {
    name: 'bobcat:source-fixture-bundles',
    setup(api) {
      api.onAfterEnvironmentCompile(async ({ environment, stats }) => {
        if (!stats || stats.hasErrors()) return;
        const [fixture] = Object.keys(environment.entry);
        if (!fixture) throw new Error('A fixture environment needs an entry');
        const output = environment.distPath;
        const native = environment.name.startsWith('lynx-');
        let nativePageSha256: string | undefined;
        let publicPath: string | null = null;
        let scripts: Record<string, string> | undefined;
        if (native) {
          // DEBUG=rspeedy retains both this page's compiler input and the
          // original MTS source inside lazy bundles. Lazy bytes stay unchanged.
          const options: CompilerSources = JSON.parse(await readFile(join(output, `.rspeedy/${fixture}/tasm.json`), 'utf8'));
          const publicPathMatch = options.lepusCode.root.match(/__webpack_require__\.p\s*=\s*("(?:[^"\\]|\\.)*")/);
          if (mode === 'development' && !publicPathMatch) throw new Error('Compiled page public path was not found');
          publicPath = publicPathMatch?.[1] ? JSON.parse(publicPathMatch[1]) : null;
          const customSections = { [`${fixture}__main-thread`]: { content: options.lepusCode.root } };
          for (const [path, content] of Object.entries(options.manifest)) {
            customSections[path.replace(/^\//, '')] = { content };
          }
          const page = join(output, `${fixture}.${environment.name}.bundle`);
          nativePageSha256 = sha256(await readFile(page));
          const result = await encode({
            compilerOptions: options.compilerOptions,
            sourceContent: { ...options.sourceContent, appType: 'DynamicComponent' },
            customSections,
          });
          if (result.status !== 0) throw new Error(result.error_msg);
          await writeFile(join(output, `${fixture}.lynx.bundle`), result.buffer);
          await rm(page);
          scripts = Object.fromEntries(Object.entries(customSections).map(([name, { content }]) => [name, sha256(content)]));
        }
        const bundles: Record<string, string> = {};
        for (const file of (await readdir(output, { recursive: true })).sort()) {
          if (file.endsWith('.bundle') && !file.startsWith('.rspeedy/')) {
            bundles[file] = sha256(await readFile(join(output, file)));
          }
        }
        const sources = Array.from(stats.compilation.fileDependencies)
          .filter(file => file.startsWith(sourceRoot))
          .map(file => relative(sourceRoot, file).split(sep).join('/')).sort();
        await writeFile(join(dirname(output), `${basename(output)}.provenance.json`), JSON.stringify({
          sources, target: native ? 'lynx' : 'web', mode, engineVersion,
          description: native ? 'Page repacked from original compiler sources; lazy bundles unchanged.' : 'Unmodified web compiler output.',
          publicPath, nativePageSha256, scripts, bundles,
          encoder: `@lynx-js/tasm@${version('@lynx-js/tasm')}`,
          react: version('@lynx-js/react'), rspeedy: version('@lynx-js/rspeedy'),
          reactPlugin: version('@lynx-js/react-rsbuild-plugin'),
        }, null, 2) + '\n');
      });
    },
  };
}
