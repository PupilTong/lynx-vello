import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { disableLynxBytecode } from '../../scripts/lynx-bytecode.ts';

import { pluginQRCode } from '@lynx-js/qrcode-rsbuild-plugin';
import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { pluginLynx } from '@lynx-js/rsbuild-plugin';
import { defineConfig } from '@rsbuild/core';

import { detectLanHost, producerDevPort } from './demo-ports.ts';

const projectRoot = path.dirname(fileURLToPath(import.meta.url));
const enableBundleAnalysis = !!process.env['RSPEEDY_BUNDLE_ANALYSIS'];
const enableFetchBundle = !!process.env['LAZY_BUNDLE_FETCHBUNDLE'];
const producerHost = detectLanHost();

export default defineConfig({
  tools: { bundlerChain: disableLynxBytecode },
  source: {
    entry: { main: './src/index.tsx' },
    define: {
      'process.env.LYNX_STANDALONE_PRODUCER_PORT': producerDevPort.toString(),
      'process.env.LYNX_STANDALONE_PRODUCER_HOST': JSON.stringify(producerHost),
    },
  },
  output: {
    distPath: {
      root: path.join(
        projectRoot,
        enableFetchBundle ? 'dist-consumer-fetchbundle' : 'dist-consumer',
      ),
    },
  },
  server: {
    proxy: {
      '/producer': {
        target: `http://127.0.0.1:${producerDevPort}`,
        pathRewrite: {
          '^/producer': '',
        },
      },
    },
  },
  plugins: [
    pluginLynx({ performance: { profile: enableBundleAnalysis } }),
    pluginReactLynx({
      ...(enableFetchBundle ? { engineVersion: '3.9' } : {}),
    }),
    pluginQRCode({
      schema(url) {
        return `${url}?fullscreen=true`;
      },
    }),
  ],
  environments: {
    lynx: {},
  },
});
