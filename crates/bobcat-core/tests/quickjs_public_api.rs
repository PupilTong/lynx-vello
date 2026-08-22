#![cfg(feature = "quickjs")]

use std::sync::Arc;

use bobcat_core::quickjs_engine_factory;
use bobcat_core::script::ScriptEngineFactory;

#[test]
fn quickjs_is_exposed_only_as_a_transferable_factory_capability() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<dyn ScriptEngineFactory>>();

    let factory = quickjs_engine_factory();
    let mut vm = factory.create().expect("QuickJS realm");
    vm.register_module_source("bobcat:answer", "export const answer = 42;")
        .expect("preloaded module");
    vm.execute_module(
        "import { answer } from 'bobcat:answer';\n\
         if (answer !== 42) throw new Error('wrong answer');",
        "app:///main.mjs",
    )
    .expect("named ESM entry");

    let debug = format!("{factory:?}");
    assert!(!debug.contains("Realm"));
}
