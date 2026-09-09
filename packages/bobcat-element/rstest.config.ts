import { fileURLToPath } from "node:url";

import { NormalModuleReplacementPlugin } from "@rspack/core";
import { defineConfig } from "@rstest/core";

const testHost = fileURLToPath(
  new URL("./test/native-host.mjs", import.meta.url),
);
const eventTarget = fileURLToPath(
  new URL("./src/event-target.mjs", import.meta.url),
);

export default defineConfig({
  name: "bobcat-element",
  tools: {
    rspack(config) {
      config.plugins ??= [];
      config.plugins.push(
        new NormalModuleReplacementPlugin(/^bobcat-internal:host$/, testHost),
        new NormalModuleReplacementPlugin(/^bobcat:event-target$/, eventTarget),
        new NormalModuleReplacementPlugin(/^bobcat:cross-thread-context$/, fileURLToPath(
          new URL("./src/cross-thread-context.mjs", import.meta.url),
        )),
        new NormalModuleReplacementPlugin(/^bobcat:runtime$/, fileURLToPath(
          new URL("./src/main-thread-runtime.mjs", import.meta.url),
        )),
      );
    },
  },
});
