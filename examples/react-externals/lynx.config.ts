import os from 'node:os';

import { pluginExternalBundle } from '@lynx-js/external-bundle-rsbuild-plugin';
import { pluginQRCode } from '@lynx-js/qrcode-rsbuild-plugin';
import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { defineConfig } from '@lynx-js/rspeedy';

import { pluginLynxBundleAnalysisStats } from '../bundle-analysis-stats.plugin.js';

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
