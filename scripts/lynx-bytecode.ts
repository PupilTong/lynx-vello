import { LynxTemplatePlugin } from '@lynx-js/template-webpack-plugin';
import type { LynxTemplatePluginOptions } from '@lynx-js/template-webpack-plugin';
import type { RsbuildConfig, Rspack } from '@rsbuild/core';

// WebEncodePlugin uses the JsBytecode tag to identify MTS source, so only
// change native environments. Native pages use the source external layout.
export const disableLynxBytecode: NonNullable<NonNullable<RsbuildConfig['tools']>['bundlerChain']> =
  (chain, { environment }) => {
    if (environment.name !== 'lynx' && !environment.name.startsWith('lynx-')) return;
    for (const name of Object.keys(chain.plugins.entries())) {
      if (!name.startsWith('lynx:template-')) continue;
      const entry = name.slice('lynx:template-'.length);
      chain.plugin(name).tap(args => {
        const options = args[0] as LynxTemplatePluginOptions;
        const isLazy = (asset: string) => options.experimental_isLazyBundle
          || !asset.replace(/^\//, '').startsWith(`${options.intermediate}/`);
        return [{
          ...options,
          enableSectionBytecode: false,
          customSectionNaming: () => ({
            mainThread: (asset: string, index: number) => isLazy(asset)
              ? (index === 0 ? 'main-thread' : undefined)
              : `${entry}${index === 0 ? '' : `:${index}`}__main-thread`,
            background: (asset: string, index: number) => isLazy(asset)
              ? (index === 0 ? 'background' : asset.replace(/^\//, ''))
              : asset.replace(/^\//, ''),
            css: (asset: string, index: number) => isLazy(asset)
              ? (index === 0 ? 'CSS' : `CSS:${index}`)
              : `${entry}:CSS${index === 0 ? '' : `:${index}`}`,
          }),
        }];
      });
    }
    chain.plugin('bobcat:source-bundles').use({
      apply(compiler: Rspack.Compiler) {
        compiler.hooks.thisCompilation.tap('bobcat:source-bundles', compilation => {
          const hooks = LynxTemplatePlugin.getLynxTemplatePluginHooks(compilation);
          hooks.beforeEncode.tap({ name: 'bobcat:source-bundles', stage: 1000 }, args => {
            const { encodeData } = args;
            // Run after LynxEncodePlugin has generated the page's BTS bootstrap.
            // buildCustomSections omits it for lazy libraries, but pages need it.
            if (encodeData.sourceContent.appType === 'card') {
              const bootstrap = encodeData.manifest['/app-service.js'];
              if (bootstrap !== undefined) {
                encodeData.customSections['app-service.js'] = { content: bootstrap };
              }
            }
            encodeData.sourceContent.appType = 'DynamicComponent';
            return args;
          });
        });
      },
    });
  };
