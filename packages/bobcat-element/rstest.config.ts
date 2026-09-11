import { defineConfig } from "@rstest/core";

export default defineConfig({
  name: "bobcat-element",
  tools: {
    rspack(config) {
      // Resolve without the tsconfig solution. Its `paths` map each `bobcat:*`
      // specifier to a source file for the type checker, and rstest bundles a
      // specifier it can resolve; one it cannot resolve becomes an external,
      // which is what each suite's `rstest.mockRequire` replaces.
      delete config.resolve?.tsConfig;
    },
  },
});
