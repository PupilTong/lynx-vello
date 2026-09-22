import { createRequire } from 'node:module';
import path from 'node:path';

import { defineExternalBundleRslibConfig } from '@lynx-js/lynx-bundle-rslib-config';
import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';

const require = createRequire(import.meta.url);
const runtimePackage = path.dirname(require.resolve('@lynx-js/react-umd/prod'));
const development = process.env['NODE_ENV'] === 'development';

export default defineExternalBundleRslibConfig({
  id: 'react',
  source: {
    entry: {
      // The package publishes TS sources; its /entry export names an absent JS file.
      ReactLynx: path.join(runtimePackage, '../src', development ? 'index.dev.ts' : 'index.ts'),
    },
  },
  plugins: [pluginReactLynx()],
  output: {
    distPath: { root: `dist-react-umd-${development ? 'dev' : 'prod'}` },
  },
}, {
  enableJsBytecode: false,
});
