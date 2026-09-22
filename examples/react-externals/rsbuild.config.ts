import os from 'node:os';
import path from 'node:path';

import { disableLynxBytecode } from '../../scripts/lynx-bytecode.ts';

import { pluginExternalBundle } from '@lynx-js/external-bundle-rsbuild-plugin';
import { pluginQRCode } from '@lynx-js/qrcode-rsbuild-plugin';
import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { defineConfig } from '@rsbuild/core';

import { pluginLynxBundleAnalysisStats } from '../bundle-analysis-stats.plugin.ts';

const isAsync = process.env['REACTLYNX_ASYNC'] === 'true';

function detectLanHost() {
  if (process.env['LYNX_HOST']) return process.env['LYNX_HOST'];
  for (const ifaces of Object.values(os.networkInterfaces())) {
    for (const iface of ifaces ?? []) {
      if (iface.family === 'IPv4' && !iface.internal) return iface.address;
    }
  }
  return 'localhost';
}
const port = Number(process.env['PORT'] ?? 3000);
const assetPrefix = `http://${detectLanHost()}:${port}/`;

export default defineConfig({
  tools: { bundlerChain: disableLynxBytecode },
  source: {
    entry: { main: './src/index.tsx' },
  },
  plugins: [
    pluginReactLynx(),
    pluginQRCode({
      schema(url) {
        return `${url}?fullscreen=true`;
      },
    }),
    pluginExternalBundle({
      ...(isAsync && {
        externalBundleRoot: 'dist-external-bundle-react-async',
      }),
      externalsPresets: isAsync
        ? { reactlynx: { async: true } }
        : { reactlynx: true },
      externalsPresetDefinitions: {
        reactlynx: {
          resolveManagedAssets(_value, { rootPath, environmentName }) {
            if (environmentName === 'web') return {};
            const variant = process.env['NODE_ENV'] === 'development' ? 'dev' : 'prod';
            return { 'react.lynx.bundle': path.join(rootPath, `dist-react-umd-${variant}`, 'react.lynx.bundle') };
          },
        },
      },
      externals: {
        './App.js': 'comp-lib.lynx.bundle',
      },
      globalObject: 'globalThis',
    }),
    pluginLynxBundleAnalysisStats(),
  ],
  environments: {
    ...(isAsync ? {} : { web: {} }),
    lynx: {},
  },
  output: {
    filenameHash: 'contenthash:8',
    assetPrefix,
    ...(isAsync && { distPath: { root: 'dist-react-async' } }),
  },
  dev: {
    assetPrefix,
  },
  server: {
    port,
    strictPort: true,
  },
});
