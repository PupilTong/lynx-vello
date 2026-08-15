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
    vm.execute_script("globalThis.answer = 42", "app:///main.js")
        .expect("named script");

    let debug = format!("{factory:?}");
    assert!(!debug.contains("Realm"));
}
