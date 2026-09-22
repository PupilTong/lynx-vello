import { pluginQRCode } from '@lynx-js/qrcode-rsbuild-plugin';
import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { defineConfig } from '@rsbuild/core';

import { disableLynxBackgroundBytecode } from '../../scripts/lynx-bytecode.ts';

import { pluginLynxBundleAnalysisStats } from '../bundle-analysis-stats.plugin.ts';

export default defineConfig({
  tools: { bundlerChain: disableLynxBackgroundBytecode },
  source: {
    entry: { main: './src/index.tsx' },
  },
  plugins: [
    pluginReactLynx(),
    pluginQRCode({
      fullscreen: true,
    }),
    pluginLynxBundleAnalysisStats(),
  ],
  environments: {
    web: {},
    lynx: {},
  },
});
