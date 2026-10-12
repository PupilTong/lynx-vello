import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import type { RsbuildPlugin, Rspack } from '@rsbuild/core';
import { CSS, LynxTemplatePlugin } from '@lynx-js/template-webpack-plugin';

/** The named sections, by section name, and the CSS file each one holds. */
const SECTIONS: Record<string, string> = {
  CSS: 'CSS.css',
  'A &/中': 'named.css',
};

// No ReactLynx build writes a web card with `encoding: 'CSS'` custom sections
// today: a lazy bundle's CSS is routed into its `StyleInfo` on the web target
// (`routeSections` in `WebEncodePlugin`). This hook gives one environment's
// card two such sections through the template plugin's public `beforeEncode`
// hook, so that `@lynx-js/web-core/encode` itself writes them into the
// `CustomSectionsStyleInfo` section (label 7) of the emitted bundle.
export function pluginCssCustomSections(environment: string): RsbuildPlugin {
  return {
    name: 'bobcat:css-custom-sections',
    setup(api) {
      api.modifyRspackConfig((config, { environment: { name } }) => {
        if (name !== environment) return;
        config.plugins ??= [];
        config.plugins.push({
          apply(compiler: Rspack.Compiler) {
            compiler.hooks.thisCompilation.tap('bobcat:css-custom-sections', (compilation) => {
              const hooks = LynxTemplatePlugin.getLynxTemplatePluginHooks(
                compilation as unknown as Parameters<typeof LynxTemplatePlugin.getLynxTemplatePluginHooks>[0],
              );
              hooks.beforeEncode.tap('bobcat:css-custom-sections', (options) => {
                for (const [section, file] of Object.entries(SECTIONS)) {
                  const path = fileURLToPath(new URL(`../src/css-custom-sections/${file}`, import.meta.url));
                  compilation.fileDependencies.add(path);
                  options.encodeData.customSections[section] = {
                    encoding: 'CSS',
                    content: { ruleList: CSS.parse(readFileSync(path, 'utf8')).root },
                  };
                }
                return options;
              });
            });
          },
        });
      });
    },
  };
}
