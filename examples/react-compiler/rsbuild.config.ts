import { pluginBabel } from '@rsbuild/plugin-babel';

import { disableLynxBytecode } from '../../scripts/lynx-bytecode.ts';

import { pluginQRCode } from '@lynx-js/qrcode-rsbuild-plugin';
import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { pluginLynx } from '@lynx-js/rsbuild-plugin';
import { defineConfig } from '@rsbuild/core';

const enableBundleAnalysis = !!process.env['RSPEEDY_BUNDLE_ANALYSIS'];
const reactLynxCompilerTarget = '17';

export default defineConfig({
  tools: { bundlerChain: disableLynxBytecode },
  source: {
    entry: { main: './src/index.tsx' },
  },
  environments: {
    lynx: {},
  },
  plugins: [
    pluginLynx({ performance: { profile: enableBundleAnalysis } }),
    pluginReactLynx(),
    pluginBabel({
      include: /\.(?:jsx|tsx)$/,
      babelLoaderOptions(opts) {
        opts.plugins?.unshift([
          'babel-plugin-react-compiler',
          {
            target: reactLynxCompilerTarget,
          },
        ]);
      },
    }),
    pluginQRCode({
      schema(url) {
        return `${url}?fullscreen=true`;
      },
    }),
  ],
});
