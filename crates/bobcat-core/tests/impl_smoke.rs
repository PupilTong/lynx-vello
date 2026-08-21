mod support;

use std::sync::Arc;

use bobcat_core::image::{AlphaType, DecodedImage, ImageFormat};
use bobcat_core::resource::ResourceFetcher;
use bobcat_core::script::{
    HostCallback, HostValue, ScriptEngine, ScriptEngineFactory, ScriptError, ScriptErrorKind,
    ScriptErrorPhase,
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

    fn call_host_member(
        &mut self,
        _namespace: &str,
        _name: &str,
        _arguments: &[HostValue],
    ) -> Result<bool, ScriptError> {
        // A VM that publishes nothing back reports exactly that.
        Ok(false)
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
    let mut view = LynxView::<NoWindow>::new(
        PageConfig::default(),
        resources,
        scripts,
        Arc::new(|| {}),
        393.0,
        727.0,
        2.0,
    )
    .expect("opaque view");

    assert_eq!(view.frame_size().width, 786);
    assert_eq!(view.frame_size().height, 1454);

    assert_eq!(
        view.register_fonts(Vec::from(b"not a font"))
            .expect("available document"),
        0
    );
    assert!(
        !view
            .set_default_font_family("missing")
            .expect("available document")
    );
    let image = DecodedImage::from_rgba8(
        1,
        1,
        AlphaType::Straight,
        vec![0, 0, 0, 255],
        ImageFormat::Png,
    )
    .expect("decoded image");
    view.register_image_url("app:///pixel.png", &image)
        .expect("available document");
}

#[test]
fn factory_is_transferable_but_the_created_vm_need_not_be() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<dyn ScriptEngineFactory>>();

    let factory: Arc<dyn ScriptEngineFactory> = Arc::new(InjectedFactory);
    let vm = factory.create().expect("VM created on its owner thread");
    assert_eq!(format!("{vm:?}"), "InjectedVm");
}
