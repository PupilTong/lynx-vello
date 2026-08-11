import { defineConfig } from '@rsbuild/core';

export default defineConfig({
  source: {
    entry: {
      index: './web/index.ts',
    },
  },
  server: {
    publicDir: [
      {
        name: 'dist',
        copyOnBuild: false,
      },
    ],
  },
});
