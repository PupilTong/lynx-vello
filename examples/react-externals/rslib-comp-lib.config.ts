import { defineExternalBundleRslibConfig } from '@lynx-js/lynx-bundle-rslib-config';
import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';

const isAsync = process.env['REACTLYNX_ASYNC'] === 'true';

export default defineExternalBundleRslibConfig({
  id: 'comp-lib',
  source: {
    entry: {
      './App.js': './external-bundle/CompLib.tsx',
    },
  },
  plugins: [
    pluginReactLynx(),
  ],
  performance: {
    buildCache: {
      cacheDigest: [isAsync ? 'react-async' : 'react-sync'],
    },
  },
  output: {
    externalsPresets: isAsync
      ? { reactlynx: { async: true } }
      : { reactlynx: true },
    ...(isAsync && {
      distPath: { root: 'dist-external-bundle-react-async' },
    }),
    globalObject: 'globalThis',
  },
});
