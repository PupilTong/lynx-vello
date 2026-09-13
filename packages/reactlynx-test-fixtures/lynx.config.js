import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { defineConfig } from '@lynx-js/rspeedy';

import { pluginSourceBundles } from './scripts/source-bundles.ts';

const engineVersion = '4.1.0';

export default defineConfig(({ env }) => {
  const mode = env === 'development' ? 'development' : 'production';
  const fixtures = mode === 'development'
    ? ['react-reload', 'react-global-props', 'react-lazy-nested']
    : ['react-lazy', 'react-lazy-sync', 'react-lazy-nested', 'react-reload', 'react-data-processor', 'react-global-props'];

  return {
    plugins: [pluginReactLynx({ engineVersion }), pluginSourceBundles(mode, engineVersion)],
    source: { entry: {} },
    // Each fixture owns a compilation and output directory, so lazy chunks
    // cannot be shared accidentally between otherwise independent test pages.
    environments: {
      ...Object.fromEntries(fixtures.map(fixture => [
        `lynx-${fixture}`,
        {
          source: { entry: { [fixture]: `./src/${fixture}.jsx` } },
          output: {
            distPath: { root: `dist/${fixture}${mode === 'development' ? '-development' : ''}` },
          },
        },
      ])),
      ...(mode === 'production' ? {
        web: {
          source: { entry: { 'react-bts-query': './src/react-bts-query.jsx' } },
          output: { distPath: { root: 'dist/react-bts-query' } },
        },
      } : {}),
    },
  };
});
