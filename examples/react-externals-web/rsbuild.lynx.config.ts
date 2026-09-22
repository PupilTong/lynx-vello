import { pluginExternalBundle } from '@lynx-js/external-bundle-rsbuild-plugin';
import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { defineConfig } from '@rsbuild/core';

import { disableLynxBackgroundBytecode } from '../../scripts/lynx-bytecode.ts';

export default defineConfig({
  tools: { bundlerChain: disableLynxBackgroundBytecode },
  source: {
    entry: { main: './src/index.tsx' },
  },
  plugins: [
    pluginReactLynx(),
    pluginExternalBundle({
      externalsPresets: {
        reactlynx: {
          async: true,
        },
      },
      externals: {
        './App.js': 'comp-lib.web.bundle',
      },
      globalObject: 'globalThis',
    }),
  ],
  environments: {
    web: {},
  },
});
