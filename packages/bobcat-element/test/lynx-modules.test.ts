import { describe, expect, it } from "@rstest/core";
import { createLynxModules } from "../src/lynx-modules.ts";

function environment() {
  const app = { _apiList: { native: true } };
  const lynx = { SystemInfo: { platform: "headless" } };
  const modules = createLynxModules(app, lynx, console);
  Object.assign(app, modules);
  return { app, modules };
}

describe("compiled BTS module ABI", () => {
  it("gives definitions an app scope and factories the compiler API arguments", () => {
    const { app, modules } = environment();
    modules.register({ '/late.js': `
      const owner=tt, helper=41;
      tt.define('late.js',function(require,module,exports,Card,setTimeout,setInterval,clearInterval,clearTimeout,NativeModules,tt){
        module.exports={owner,receiver:this,api:tt,value:helper+1,strict:(function(){return this})()===undefined};
      });
      ({init(){throw Error('tt.require must ignore Script completion');}})
    ` }, true);
    const result = modules.require('late.js');
    expect(result).toEqual({owner:app,receiver:app,api:app._apiList,value:42,strict:true});
    expect(modules.require('late.js')).toBe(result);
  });

  it("evaluates native init wrappers and converted web copies with the same app", () => {
    const source = `(function(){return {init:function({tt}){
      tt.define('/entry.js',function(require,module,exports,Card,setTimeout,setInterval,clearInterval,clearTimeout,NativeModules,tt,console,Component,ReactLynx,nativeAppId,Behavior,LynxJSBI,lynx){
        module.exports={argc:arguments.length,api:tt,platform:lynx.SystemInfo.platform,receiver:this};
      });
      return tt.require('/entry.js');
    }}})()`;
    for (const wrapped of [true, false]) {
      const {app, modules} = environment();
      modules.register({'/entry.js': source}, wrapped);
      const result = modules.requireModule('/entry.js');
      expect(result).toEqual({argc:39,api:app._apiList,platform:"headless",receiver:app});
      expect(modules.requireModule('/entry.js')).toBe(result);
    }
  });

  it("allows lexical minifier names in web Scripts without leaking bindings", () => {
    const {modules} = environment();
    modules.register({'/web.js': '"use strict"; let tt=42; module.exports={tt,platform:lynx.SystemInfo.platform,scope:lynxCoreInject.tt._apiList.native};'}, false);
    expect(modules.requireModule('/web.js')).toEqual({tt:42,platform:"headless",scope:true});
    expect(Object.hasOwn(globalThis, 'lynxCoreInject')).toBe(false);
  });

  it("publishes partial CommonJS exports for cycles and resolves relative names", () => {
    const {modules} = environment();
    modules.register({
      '/dir/a.js': `tt.define('dir/a.js',function(require,module){
        module.exports.a=1; module.exports.fromB=require('./b').fromA;
      });`,
      '/dir/b.js': `tt.define('dir/b.js',function(require,module){
        module.exports.fromA=require('../dir/a').a;
      });`,
    }, false);
    expect(modules.require('dir/a.js')).toEqual({a:1,fromB:1});
  });

  it("loads registered JSON and named sections and rejects absent sources", () => {
    const {app, modules} = environment();
    modules.register({'/data.json':'{"value":7}'}, false);
    expect(modules.requireModule('/data.json')).toEqual({value:7});
    modules.registerSections({background:'({init({tt}){return {app:tt}}})'});
    const result = modules.loadScript('background', {});
    expect(result).toEqual({app});
    expect(modules.loadScript('background', {})).toBe(result);
    expect(() => modules.requireModule('/absent.js')).toThrow('not registered');
    expect(() => modules.loadScript('absent', {})).toThrow('not registered');
  });
});
