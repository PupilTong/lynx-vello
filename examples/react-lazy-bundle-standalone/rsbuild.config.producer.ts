import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { disableLynxBackgroundBytecode } from '../../scripts/lynx-bytecode.ts';

import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { pluginLynx } from '@lynx-js/rsbuild-plugin';
import { defineConfig } from '@rsbuild/core';

import { detectLanHost, producerDevPort } from './demo-ports.ts';

const projectRoot = path.dirname(fileURLToPath(import.meta.url));
const enableBundleAnalysis = !!process.env['RSPEEDY_BUNDLE_ANALYSIS'];
const enableFetchBundle = !!process.env['LAZY_BUNDLE_FETCHBUNDLE'];
const producerPublicPath = `http://${detectLanHost()}:${producerDevPort}/`;

export default defineConfig({
  tools: { bundlerChain: disableLynxBackgroundBytecode },
  source: {
    entry: {
      LazyComponent: './src/LazyComponent.tsx',
      LazyComponentSync: './src/LazyComponentSync.tsx',
      LazyComponentAsync: './src/LazyComponentAsync.tsx',
      add: './src/utils/add.ts',
      minus: './src/utils/minus.ts',
      dynamic: './src/utils/dynamic.ts',
    },
  },
  output: {
    assetPrefix: producerPublicPath,
    distPath: {
      root: path.join(
        projectRoot,
        enableFetchBundle ? 'dist-producer-fetchbundle' : 'dist-producer',
      ),
    },
  },
  dev: {
    assetPrefix: producerPublicPath,
  },
  server: {
    port: producerDevPort,
    strictPort: true,
  },
  plugins: [
    pluginLynx({ performance: { profile: enableBundleAnalysis } }),
    pluginReactLynx({
      experimental_isLazyBundle: true,
      ...(enableFetchBundle ? { engineVersion: '3.9' } : {}),
    }),
  ],
  environments: {
    lynx: {},
  },
});
