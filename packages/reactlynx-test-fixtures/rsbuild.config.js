import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { defineConfig } from '@rsbuild/core';

import { disableLynxBackgroundBytecode } from '../../scripts/lynx-bytecode.ts';

import { pluginSourceBundles } from './scripts/source-bundles.ts';

const engineVersion = '4.1.0';

export default defineConfig(({ env }) => {
  const mode = env === 'development' ? 'development' : 'production';
  const fixtures = mode === 'development'
    ? ['react-reload', 'react-global-props', 'react-lazy-nested']
    : ['react-native', 'react-lazy', 'react-lazy-sync', 'react-lazy-nested', 'react-reload', 'react-data-processor', 'react-global-props', 'react-list'];

  return {
    tools: { bundlerChain: disableLynxBackgroundBytecode },
    plugins: [pluginReactLynx({ engineVersion }), pluginSourceBundles(mode, engineVersion)],
    source: { entry: {} },
    // Each fixture owns a compilation and output directory, so lazy chunks
    // cannot be shared accidentally between otherwise independent test pages.
    environments: {
      ...Object.fromEntries(fixtures.map(fixture => [
        `lynx-${fixture}`,
        {
          source: { entry: { [fixture]: `./src/${fixture === 'react-native' ? 'react-bts-query' : fixture}.jsx` } },
          output: {
            distPath: { root: `dist/${fixture}${mode === 'development' ? '-development' : ''}` },
          },
        },
      ])),
      ...(mode === 'production' ? {
        ...Object.fromEntries(['react-bts-query', 'basic-bindtap', 'basic-class-selector',
          'basic-performance-large-css', 'basic-mts-run-on-main-thread', 'basic-mts-run-on-background']
          .map(fixture => [`web-${fixture}`, {
            source: { entry: { [fixture]: fixture === 'react-bts-query'
              ? `./src/${fixture}.jsx` : `./src/${fixture}/index.jsx` } },
            output: { distPath: { root: `dist/${fixture}` } },
          }])),
      } : {}),
    },
  };
});
