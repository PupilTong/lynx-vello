// Copyright 2024 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.

import { pluginQRCode } from '@lynx-js/qrcode-rsbuild-plugin';
import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { defineConfig } from '@rsbuild/core';
import { pluginSass } from '@rsbuild/plugin-sass';

import { disableLynxBackgroundBytecode } from '../../scripts/lynx-bytecode.ts';

export default defineConfig({
  tools: { bundlerChain: disableLynxBackgroundBytecode },
  source: {
    entry: { main: './index.tsx' },
    alias: {
      '@components': './components',
      '@assets': './assets',
    },
  },
  plugins: [pluginReactLynx(), pluginSass({}), pluginQRCode()],
  // Upstream builds only native bundles; Bobcat consumes the web ones.
  environments: {
    web: {},
    lynx: {},
  },
});
