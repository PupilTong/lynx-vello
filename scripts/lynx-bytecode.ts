import { LynxTemplatePlugin } from '@lynx-js/template-webpack-plugin';
import type { RsbuildConfig, Rspack } from '@rsbuild/core';

// Keep native BTS manifests as source. MTS encoding and section layout remain
// under the upstream plugins' control.
export const disableLynxBackgroundBytecode: NonNullable<NonNullable<RsbuildConfig['tools']>['bundlerChain']> =
  (chain, { environment }) => {
    if (environment.name !== 'lynx' && !environment.name.startsWith('lynx-')) return;
    chain.plugin('bobcat:background-source').use({
      apply(compiler: Rspack.Compiler) {
        compiler.hooks.thisCompilation.tap('bobcat:background-source', compilation => {
          const hooks = LynxTemplatePlugin.getLynxTemplatePluginHooks(compilation);
          hooks.beforeEncode.tap({ name: 'bobcat:background-source', stage: 1000 }, args => {
            args.encodeData.compilerOptions['experimental_encodeQuickjsBytecode'] = false;
            return args;
          });
        });
      },
    });
  };
