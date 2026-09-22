import { pluginQRCode } from '@lynx-js/qrcode-rsbuild-plugin';
import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { pluginLynx } from '@lynx-js/rsbuild-plugin';
import { defineConfig } from '@rsbuild/core';

import { disableLynxBackgroundBytecode } from '../../scripts/lynx-bytecode.ts';

const enableBundleAnalysis = !!process.env['RSPEEDY_BUNDLE_ANALYSIS'];

export default defineConfig({
  tools: { bundlerChain: disableLynxBackgroundBytecode },
  source: {
    entry: { main: './src/index.tsx' },
    define: {
      __ALOG__: 'true',
      __ALOG_ELEMENT_API__: 'true',
    },
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
  environments: {
    web: {},
    lynx: {},
  },
});
