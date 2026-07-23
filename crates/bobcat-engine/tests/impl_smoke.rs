use std::sync::Arc;

use bobcat_engine::resource::{
    BufferedResourceRequest, HttpRequest, HttpResponse, PrefetchReceipt, PrefetchRequest,
    RequestId, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceErrorKind, ResourceErrorPhase, ResourceFetcher, ResourceFuture, ResourcePath,
    ResourceRequest, ResourceResponse, ResourceStream, RetryAdvice,
};
use bobcat_engine::script::{ScriptEngine, ScriptError, ScriptFuture, ScriptValue};
use bobcat_engine::view::LynxView;

#[derive(Debug)]
struct NullResourceFetcher;

impl NullResourceFetcher {
    fn failure<T>(
        kind: ResourceErrorKind,
        phase: ResourceErrorPhase,
    ) -> ResourceFuture<'static, T> {
        Box::pin(async move {
            Err(ResourceError {
                request_id: None,
                kind,
                phase,
                locator: None,
                status: None,
                message: Arc::from("smoke-test fetcher failure"),
                retry: RetryAdvice::Never,
            })
        })
    }
}

impl ResourceFetcher for NullResourceFetcher {
    fn supports_capability(&self, _capability: ResourceCapability) -> bool {
        false
    }

    fn resolve_locator(&self, _request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator> {
        Self::failure(
            ResourceErrorKind::InvalidRequest,
            ResourceErrorPhase::Resolve,
        )
    }

    fn fetch_resource(
        &self,
        _request: BufferedResourceRequest,
    ) -> ResourceFuture<'_, ResourceResponse> {
        Self::failure(
            ResourceErrorKind::UnsupportedOperation,
            ResourceErrorPhase::Open,
        )
    }

    fn open_resource(&self, _request: ResourceRequest) -> ResourceFuture<'_, ResourceStream> {
        Self::failure(
            ResourceErrorKind::UnsupportedOperation,
            ResourceErrorPhase::Open,
        )
    }

    fn fetch_resource_path(&self, _request: ResourceRequest) -> ResourceFuture<'_, ResourcePath> {
        Self::failure(
            ResourceErrorKind::UnsupportedOperation,
            ResourceErrorPhase::MaterializePath,
        )
    }

    fn fetch_http(&self, _request: HttpRequest) -> ResourceFuture<'_, HttpResponse> {
        Self::failure(
            ResourceErrorKind::UnsupportedOperation,
            ResourceErrorPhase::Connect,
        )
    }

    fn prefetch(&self, _request: PrefetchRequest) -> ResourceFuture<'_, PrefetchReceipt> {
        Self::failure(
            ResourceErrorKind::UnsupportedOperation,
            ResourceErrorPhase::Prefetch,
        )
    }

    fn cancel_request(&self, _request_id: RequestId) -> ResourceFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Debug)]
struct EchoCallable;

#[derive(Debug)]
struct EchoSymbol;

#[derive(Debug, Default)]
struct EchoScriptEngine;

impl ScriptEngine for EchoScriptEngine {
    type Callable = EchoCallable;
    type Symbol = EchoSymbol;

    fn evaluate(
        &mut self,
        source_text: &str,
    ) -> Result<ScriptValue<Self::Callable, Self::Symbol>, ScriptError> {
        Ok(ScriptValue::String(Arc::from(source_text)))
    }

    fn import_value<'a>(
        &'a mut self,
        specifier: &'a str,
        export_name: &'a str,
    ) -> ScriptFuture<'a, Self::Callable, Self::Symbol> {
        Box::pin(async move {
            Ok(ScriptValue::String(Arc::from(format!(
                "{specifier}#{export_name}"
            ))))
        })
    }

    fn call(
        &mut self,
        _callable: &Self::Callable,
        _this_value: &ScriptValue<Self::Callable, Self::Symbol>,
        arguments: &[ScriptValue<Self::Callable, Self::Symbol>],
    ) -> Result<ScriptValue<Self::Callable, Self::Symbol>, ScriptError> {
        Ok(ScriptValue::Boolean(arguments.len() == 2))
    }
}

#[test]
fn traits_compose_into_owned_and_shared_views() {
    let mut owned = LynxView::new(NullResourceFetcher, EchoScriptEngine);
    let evaluated = owned.script_engine_mut().evaluate("globalThis");
    assert!(matches!(
        evaluated,
        Ok(ScriptValue::String(value)) if value.as_ref() == "globalThis"
    ));

    let owned_parts = owned.into_parts();
    assert!(
        !owned_parts
            .resource_fetcher
            .supports_capability(ResourceCapability::Http)
    );
    let mut owned_engine = owned_parts.script_engine;
    let import = owned_engine.import_value("app.js", "default");
    drop(import);

    let shared_fetcher: Arc<dyn ResourceFetcher> = Arc::new(NullResourceFetcher);
    let shared_view =
        LynxView::from_shared_resource_fetcher(Arc::clone(&shared_fetcher), EchoScriptEngine);
    let returned = shared_view.into_parts();

    assert!(Arc::ptr_eq(&shared_fetcher, &returned.resource_fetcher));
    assert_eq!(Arc::strong_count(&shared_fetcher), 2);

    let mut returned_engine = returned.script_engine;
    let called = returned_engine.call(
        &EchoCallable,
        &ScriptValue::Undefined,
        &[ScriptValue::Boolean(true), ScriptValue::Null],
    );
    assert!(matches!(called, Ok(ScriptValue::Boolean(true))));
}
