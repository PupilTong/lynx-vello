import { fileURLToPath } from "node:url";

import { NormalModuleReplacementPlugin } from "@rspack/core";
import { defineConfig } from "@rstest/core";

const testHost = fileURLToPath(
  new URL("./test/native-host.ts", import.meta.url),
);
const eventTarget = fileURLToPath(
  new URL("./src/event-target.ts", import.meta.url),
);

export default defineConfig({
  name: "bobcat-element",
  tools: {
    rspack(config) {
      // Resolve without the tsconfig solution. Its `paths` map each `bobcat:*`
      // specifier to a source file for the type checker, and rstest bundles a
      // specifier it can resolve; one it cannot resolve becomes an external,
      // which is what each suite's `rstest.mockRequire` replaces.
      delete config.resolve?.tsConfig;
      config.plugins ??= [];
      config.plugins.push(
        new NormalModuleReplacementPlugin(/^bobcat-internal:host$/, testHost),
        new NormalModuleReplacementPlugin(/^bobcat:event-target$/, eventTarget),
        new NormalModuleReplacementPlugin(/^bobcat:cross-thread-context$/, fileURLToPath(
          new URL("./src/cross-thread-context.ts", import.meta.url),
        )),
        new NormalModuleReplacementPlugin(/^bobcat:runtime$/, fileURLToPath(
          new URL("./src/main-thread-runtime.ts", import.meta.url),
        )),
      );
    },
  },
});
