mod support;

use std::sync::Arc;

use bobcat_core::resource::ResourceFetcher;
use bobcat_core::script::{
    HostCallback, ScriptEngine, ScriptEngineFactory, ScriptError, ScriptErrorKind, ScriptErrorPhase,
};
use bobcat_core::{LynxView, NoWindow, PageConfig};
use support::FetcherDouble;

#[derive(Debug)]
struct InjectedFactory;

impl ScriptEngineFactory for InjectedFactory {
    fn create(&self) -> Result<Box<dyn ScriptEngine>, ScriptError> {
        Ok(Box::new(InjectedVm))
    }
}

#[derive(Debug)]
struct InjectedVm;

impl ScriptEngine for InjectedVm {
    fn register_host_function(
        &mut self,
        _namespace: &str,
        _name: &str,
        _arity: u8,
        _callback: HostCallback,
    ) -> Result<(), ScriptError> {
        Ok(())
    }

    fn execute_script(&mut self, _source: &str, source_name: &str) -> Result<(), ScriptError> {
        if source_name.is_empty() {
            return Err(ScriptError {
                kind: ScriptErrorKind::Other,
                phase: ScriptErrorPhase::Execute,
                message: "source name must not be empty".into(),
                location: None,
            });
        }
        Ok(())
    }

    fn collect_garbage(&mut self) -> Result<(), ScriptError> {
        Ok(())
    }
}

fn assert_factory_contract<T: ScriptEngineFactory>() {}

#[test]
fn external_vm_factory_composes_into_the_opaque_view() {
    assert_factory_contract::<InjectedFactory>();
    let resources: Arc<dyn ResourceFetcher> = Arc::new(FetcherDouble::new(Vec::new()));
    let scripts: Arc<dyn ScriptEngineFactory> = Arc::new(InjectedFactory);
    let view =
        LynxView::<NoWindow>::new(PageConfig::default(), resources, scripts, 393.0, 727.0, 2.0)
            .expect("opaque view");

    assert_eq!(view.frame_size().width, 786);
    assert_eq!(view.frame_size().height, 1454);
}

#[test]
fn factory_is_transferable_but_the_created_vm_need_not_be() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<dyn ScriptEngineFactory>>();

    let factory: Arc<dyn ScriptEngineFactory> = Arc::new(InjectedFactory);
    let vm = factory.create().expect("VM created on its owner thread");
    assert_eq!(format!("{vm:?}"), "InjectedVm");
}
