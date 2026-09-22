import { pluginTailwindCSS } from 'rsbuild-plugin-tailwindcss';

import { disableLynxBytecode } from '../../scripts/lynx-bytecode.ts';

import { pluginLynxConfig } from '@lynx-js/config-rsbuild-plugin';
import { pluginQRCode } from '@lynx-js/qrcode-rsbuild-plugin';
import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { defineConfig } from '@rsbuild/core';

export default defineConfig({
  tools: { bundlerChain: disableLynxBytecode },
  source: {
    entry: { main: './src/index.tsx' },
  },
  environments: {
    lynx: {},
  },
  plugins: [
    pluginReactLynx(),
    pluginQRCode({
      schema(url) {
        return `${url}?fullscreen=true`;
      },
    }),
    pluginTailwindCSS({
      config: 'tailwind.config.ts',
      exclude: [/[\\/]node_modules[\\/]/],
    }),
    pluginLynxConfig({
      enableCSSInlineVariables: true,
      enableCSSInheritance: true,
    }),
  ],
});
