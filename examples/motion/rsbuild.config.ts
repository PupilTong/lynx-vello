import { pluginQRCode } from '@lynx-js/qrcode-rsbuild-plugin';
import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { pluginLynx } from '@lynx-js/rsbuild-plugin';
import { defineConfig } from '@rsbuild/core';

import { disableLynxBytecode } from '../../scripts/lynx-bytecode.ts';

const enableBundleAnalysis = !!process.env['RSPEEDY_BUNDLE_ANALYSIS'];

export default defineConfig({
  tools: { bundlerChain: disableLynxBytecode },
  source: {
    entry: {
      main: './src/index.tsx',
      mini: './src/Mini/index.tsx',
    },
  },
  environments: {
    lynx: {},
  },
  plugins: [
    pluginLynx({ performance: { profile: enableBundleAnalysis } }),
    pluginReactLynx(),
    pluginQRCode({
      schema(url) {
        return `${url}?fullscreen=true`;
      },
    }),
  ],
});
