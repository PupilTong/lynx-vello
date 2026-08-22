import { fileURLToPath } from "node:url";

import { NormalModuleReplacementPlugin } from "@rspack/core";
import { defineConfig } from "@rstest/core";

const testHost = fileURLToPath(
  new URL("./test/native-host.mjs", import.meta.url),
);

export default defineConfig({
  name: "bobcat-element",
  tools: {
    rspack(config) {
      config.plugins ??= [];
      config.plugins.push(
        new NormalModuleReplacementPlugin(/^bobcat-internal:host$/, testHost),
      );
    },
  },
});
