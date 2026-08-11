import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { defineConfig } from '@rstest/core';

export default defineConfig({
  name: 'error-remapping',
  root: dirname(fileURLToPath(import.meta.url)),
  include: ['**/*.test.ts'],
});
