//! JS owns rAF callbacks. The host member only reports whether that realm
//! needs another display frame through its existing host-notice channel.

use quickjs_rust_bridge::HostValue;

use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;

pub(crate) fn install(
    engine: &mut ScriptEngine,
    js: &mut ScriptRuntime,
    mut notify: impl FnMut(bool) + 'static,
) -> Result<(), ScriptError> {
    engine.register_host_module_function(
        js,
        crate::esm::HOST_MODULE_SPECIFIER,
        "requestScriptFrame",
        1,
        Box::new(move |arguments| {
            let Some(HostValue::Boolean(pending)) = arguments.first() else {
                return Err("requestScriptFrame expects a boolean".to_owned());
            };
            notify(*pending);
            Ok(HostValue::Undefined)
        }),
    )
}
