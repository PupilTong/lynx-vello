use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use std::{panic, thread};

use bobcat_engine::resource::{
    BufferedResourceRequest, HttpRequest, HttpResponse, PrefetchReceipt, PrefetchRequest,
    RequestId, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceErrorKind, ResourceErrorPhase, ResourceFetcher, ResourceFuture, ResourcePath,
    ResourceRequest, ResourceResponse, ResourceStream, RetryAdvice,
};
use bobcat_engine::script::{ScriptEngine, ScriptErrorKind, ScriptErrorPhase, ScriptValue};
use bobcat_engine::view::ViewMetrics;
use quickjs_rust_bridge::EvalSource;

use super::{
    DEFAULT_EXECUTION_TIMEOUT, QuickJsCallable, QuickJsConfig, QuickJsScriptEngine, QuickJsSymbol,
    new_quickjs_view,
};

type Value = ScriptValue<QuickJsCallable, QuickJsSymbol>;

fn engine() -> QuickJsScriptEngine {
    QuickJsScriptEngine::new().expect("QuickJS realm should initialize")
}

fn evaluate(engine: &mut QuickJsScriptEngine, source: &str) -> Value {
    engine.evaluate(source).expect("script should evaluate")
}

fn run_with_watchdog<T: Send + 'static>(operation: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let outcome = panic::catch_unwind(panic::AssertUnwindSafe(operation));
        let _ = sender.send(outcome);
    });
    let outcome = receiver
        .recv_timeout(Duration::from_secs(3))
        .unwrap_or_else(|error| panic!("QuickJS interrupt watchdog expired: {error}"));
    worker
        .join()
        .expect("watchdog worker should capture its own panic");
    match outcome {
        Ok(value) => value,
        Err(payload) => panic::resume_unwind(payload),
    }
}

#[test]
fn execution_timeout_policy_is_configurable() {
    let default = QuickJsConfig::default();
    assert_eq!(default.execution_timeout(), Some(DEFAULT_EXECUTION_TIMEOUT));
    assert_eq!(
        default.with_execution_timeout(None).execution_timeout(),
        None
    );
}

#[test]
fn execution_timeout_maps_to_reusable_bobcat_error() {
    let (kind, phase, message, reused) = run_with_watchdog(|| {
        let config =
            QuickJsConfig::default().with_execution_timeout(Some(Duration::from_millis(20)));
        let mut engine =
            QuickJsScriptEngine::with_config(config).expect("QuickJS realm should initialize");
        let error = engine
            .evaluate("for (;;) {}")
            .expect_err("infinite evaluation must time out");
        let reused = matches!(engine.evaluate("6 * 7"), Ok(Value::Number(42.0)));
        (error.kind, error.phase, error.message, reused)
    });

    assert_eq!(kind, ScriptErrorKind::Other);
    assert_eq!(phase, ScriptErrorPhase::Evaluate);
    assert!(message.contains("configured timeout"));
    assert!(reused);
}

#[test]
fn realms_are_isolated() {
    let mut first = engine();
    let mut second = engine();

    evaluate(&mut first, "globalThis.answer = 42");

    assert!(matches!(
        evaluate(&mut first, "answer"),
        Value::Number(42.0)
    ));
    assert!(matches!(
        evaluate(&mut second, "typeof answer"),
        Value::String(value) if value.as_ref() == "undefined"
    ));
}

#[test]
fn primitives_cross_the_boundary_without_loss() {
    let mut engine = engine();
    let large_big_int = "1234567890123456789012345678901234567890";

    assert!(matches!(
        evaluate(&mut engine, "undefined"),
        Value::Undefined
    ));
    assert!(matches!(evaluate(&mut engine, "null"), Value::Null));
    assert!(matches!(
        evaluate(&mut engine, "true"),
        Value::Boolean(true)
    ));
    assert!(
        matches!(evaluate(&mut engine, "-0"), Value::Number(value) if value.is_sign_negative())
    );
    assert!(matches!(evaluate(&mut engine, "NaN"), Value::Number(value) if value.is_nan()));
    assert!(matches!(
        evaluate(&mut engine, &format!("{large_big_int}n")),
        Value::BigInt(value) if value.as_ref() == large_big_int
    ));
    assert!(matches!(
        evaluate(&mut engine, "Symbol('opaque')"),
        Value::Symbol(_)
    ));
}

#[test]
fn unicode_strings_round_trip_through_calls() {
    let mut engine = engine();
    let callable = match evaluate(&mut engine, "value => value") {
        Value::Callable(callable) => callable,
        other => panic!("expected callable, got {other:?}"),
    };
    let exact: Arc<str> = Arc::from("a🦀b");
    let result = engine
        .call(
            &callable,
            &Value::Undefined,
            &[Value::String(exact.clone())],
        )
        .expect("call should succeed");

    assert!(matches!(result, Value::String(value) if value == exact));
    assert!(matches!(
        evaluate(&mut engine, "'a🦀b'"),
        Value::String(value) if value == exact
    ));
}

#[test]
fn ill_formed_ecmascript_strings_do_not_cross_bobcats_utf8_boundary() {
    let mut engine = engine();
    let error = engine
        .evaluate("'\\uD800'")
        .expect_err("an unpaired surrogate cannot be represented by Arc<str>");

    assert_eq!(error.kind, ScriptErrorKind::NonTransferableValue);
    assert_eq!(error.phase, ScriptErrorPhase::Evaluate);
}

#[test]
fn callable_handles_are_cloneable_and_callable() {
    let mut engine = engine();
    let callable = match evaluate(&mut engine, "(left, right) => left + right") {
        Value::Callable(callable) => callable,
        other => panic!("expected callable, got {other:?}"),
    };
    let cloned = callable.clone();
    let result = engine
        .call(
            &cloned,
            &Value::Undefined,
            &[Value::Number(20.0), Value::Number(22.0)],
        )
        .expect("cloned callable should stay rooted");

    assert!(matches!(result, Value::Number(42.0)));
}

#[test]
fn all_boundary_values_round_trip_through_calls() {
    let mut engine = engine();
    let identity = match evaluate(&mut engine, "value => value") {
        Value::Callable(callable) => callable,
        other => panic!("expected callable, got {other:?}"),
    };
    let symbol = evaluate(&mut engine, "Symbol('round trip')");
    let callable = evaluate(&mut engine, "() => 42");
    let values = [
        Value::Undefined,
        Value::Null,
        Value::Boolean(false),
        Value::Number(f64::INFINITY),
        Value::BigInt(Arc::from("-123456789012345678901234567890")),
        Value::String(Arc::from("round trip")),
        symbol,
        callable,
    ];

    for (index, value) in values.into_iter().enumerate() {
        let result = engine
            .call(&identity, &Value::Undefined, &[value])
            .expect("boundary value should round-trip");
        match (index, result) {
            (0, Value::Undefined)
            | (1, Value::Null)
            | (2, Value::Boolean(false))
            | (6, Value::Symbol(_))
            | (7, Value::Callable(_)) => {}
            (3, Value::Number(value)) if value.is_infinite() => {}
            (4, Value::BigInt(value)) if value.as_ref() == "-123456789012345678901234567890" => {}
            (5, Value::String(value)) if value.as_ref() == "round trip" => {}
            (_, other) => panic!("boundary value {index} changed to {other:?}"),
        }
    }
}

#[test]
fn handles_from_another_engine_are_rejected() {
    let mut first = engine();
    let mut second = engine();
    let callable = match evaluate(&mut first, "() => 42") {
        Value::Callable(callable) => callable,
        other => panic!("expected callable, got {other:?}"),
    };
    let error = second
        .call(&callable, &Value::Undefined, &[])
        .expect_err("foreign callable must be rejected");

    assert_eq!(error.kind, ScriptErrorKind::WrongEngine);
    assert_eq!(error.phase, ScriptErrorPhase::Call);
}

#[test]
fn ordinary_objects_cannot_cross_the_boundary() {
    let mut engine = engine();
    let error = engine
        .evaluate("({ answer: 42 })")
        .expect_err("objects must not escape the realm");

    assert_eq!(error.kind, ScriptErrorKind::NonTransferableValue);
    assert_eq!(error.phase, ScriptErrorPhase::Evaluate);

    let callable = match evaluate(&mut engine, "() => ({ answer: 42 })") {
        Value::Callable(callable) => callable,
        other => panic!("expected callable, got {other:?}"),
    };
    let error = engine
        .call(&callable, &Value::Undefined, &[])
        .expect_err("objects returned by calls must not escape the realm");
    assert_eq!(error.kind, ScriptErrorKind::NonTransferableValue);
    assert_eq!(error.phase, ScriptErrorPhase::Call);
}

#[test]
fn checkpoint_runs_before_rejecting_a_nontransferable_result() {
    let mut engine = engine();
    let error = engine
        .evaluate(
            "globalThis.ranBeforeRejection = 0; Promise.resolve().then(() => ranBeforeRejection = 1); ({})",
        )
        .expect_err("object result must be rejected");

    assert_eq!(error.kind, ScriptErrorKind::NonTransferableValue);
    assert!(matches!(
        evaluate(&mut engine, "ranBeforeRejection"),
        Value::Number(1.0)
    ));
}

#[test]
fn checkpoints_run_after_script_and_callable_exceptions() {
    let mut engine = engine();
    let script_error = engine
        .evaluate(
            "globalThis.order = []; Promise.resolve().then(() => order.push('script-job')); throw new Error('script failure')",
        )
        .expect_err("the script should throw");
    assert_eq!(script_error.kind, ScriptErrorKind::Exception);
    assert!(matches!(
        evaluate(&mut engine, "order.join(',')"),
        Value::String(value) if value.as_ref() == "script-job"
    ));

    let callable = match evaluate(
        &mut engine,
        "() => { Promise.resolve().then(() => order.push('call-job')); throw new Error('call failure') }",
    ) {
        Value::Callable(callable) => callable,
        other => panic!("expected callable, got {other:?}"),
    };
    let call_error = engine
        .call(&callable, &Value::Undefined, &[])
        .expect_err("the callable should throw");
    assert_eq!(call_error.kind, ScriptErrorKind::Exception);
    assert!(matches!(
        evaluate(&mut engine, "order.join(',')"),
        Value::String(value) if value.as_ref() == "script-job,call-job"
    ));
}

#[test]
fn checkpoint_errors_after_primary_exceptions_precede_javascript_reentry() {
    let mut engine = engine();
    let primary = engine
        .evaluate(
            "Promise.reject(new Error('deferred rejection')); throw new Error('primary failure')",
        )
        .expect_err("the primary exception should win");
    assert_eq!(primary.kind, ScriptErrorKind::Exception);
    assert_eq!(primary.message.as_ref(), "Error: primary failure");

    let deferred = engine
        .evaluate("globalThis.enteredBeforeDeferredRejection = true")
        .expect_err("the deferred checkpoint error should be reported first");
    assert_eq!(deferred.kind, ScriptErrorKind::Exception);
    assert_eq!(deferred.message.as_ref(), "Error: deferred rejection");
    assert!(matches!(
        evaluate(&mut engine, "typeof enteredBeforeDeferredRejection"),
        Value::String(value) if value.as_ref() == "undefined"
    ));
}

#[test]
fn malformed_host_big_int_is_rejected_before_calling_javascript() {
    let mut engine = engine();
    let callable = match evaluate(&mut engine, "value => value") {
        Value::Callable(callable) => callable,
        other => panic!("expected callable, got {other:?}"),
    };
    let error = engine
        .call(
            &callable,
            &Value::Undefined,
            &[Value::BigInt(Arc::from("01"))],
        )
        .expect_err("non-canonical BigInt must be rejected");

    assert_eq!(error.kind, ScriptErrorKind::InvalidBoundaryValue);
    assert_eq!(error.phase, ScriptErrorPhase::Call);
}

#[test]
fn source_metadata_and_error_categories_are_preserved() {
    let mut engine = engine();
    let error = engine
        .evaluate_source(EvalSource {
            text: "const = 1",
            name: Some("app:///main.js"),
            line_offset: 6,
        })
        .expect_err("invalid source must fail");

    assert_eq!(error.kind, ScriptErrorKind::Syntax);
    assert_eq!(error.phase, ScriptErrorPhase::Evaluate);
    let location = error.location.expect("syntax error should have a location");
    assert_eq!(location.source.as_deref(), Some("app:///main.js"));
    assert_eq!(location.line, Some(7));

    let exception = engine
        .evaluate("throw new Error('private realm error')")
        .expect_err("throw must fail");
    assert_eq!(exception.kind, ScriptErrorKind::Exception);
    assert_eq!(exception.phase, ScriptErrorPhase::Evaluate);
    assert_eq!(exception.message.as_ref(), "Error: private realm error");

    let type_error = engine
        .evaluate("throw new TypeError('invalid receiver')")
        .expect_err("a TypeError must fail");
    assert_eq!(type_error.kind, ScriptErrorKind::Exception);
    assert_eq!(type_error.message.as_ref(), "TypeError: invalid receiver");

    let thrown_syntax = engine
        .evaluate("throw new SyntaxError('runtime syntax object')")
        .expect_err("a user-thrown SyntaxError must fail");
    assert_eq!(thrown_syntax.kind, ScriptErrorKind::Exception);

    let rejected_syntax = engine
        .evaluate("Promise.reject(new SyntaxError('rejected syntax object')); undefined")
        .expect_err("a rejected SyntaxError must fail the checkpoint");
    assert_eq!(rejected_syntax.kind, ScriptErrorKind::Exception);
}

#[test]
fn successful_operations_run_a_microtask_checkpoint() {
    let mut engine = engine();

    evaluate(
        &mut engine,
        "globalThis.checkpointValue = 0; Promise.resolve().then(() => checkpointValue = 42); undefined",
    );

    assert!(matches!(
        evaluate(&mut engine, "checkpointValue"),
        Value::Number(42.0)
    ));
    let schedule = match evaluate(
        &mut engine,
        "() => { checkpointValue = 0; Promise.resolve().then(() => checkpointValue = 84) }",
    ) {
        Value::Callable(callable) => callable,
        other => panic!("expected callable, got {other:?}"),
    };
    engine
        .call(&schedule, &Value::Undefined, &[])
        .expect("call and its checkpoint should succeed");
    assert!(matches!(
        evaluate(&mut engine, "checkpointValue"),
        Value::Number(84.0)
    ));
}

#[test]
fn rejected_microtasks_are_sanitized_as_checkpoint_exceptions() {
    let mut engine = engine();
    let error = engine
        .evaluate("Promise.reject(new Error('microtask rejection')); undefined")
        .expect_err("unhandled rejection must fail the checkpoint");

    assert_eq!(error.kind, ScriptErrorKind::Exception);
    assert_eq!(error.phase, ScriptErrorPhase::Evaluate);
    assert_eq!(error.message.as_ref(), "Error: microtask rejection");
}

#[test]
fn multiple_rejections_are_reported_before_javascript_reentry() {
    let mut engine = engine();
    let first = engine
        .evaluate(
            "Promise.reject(new Error('first')); Promise.reject(new Error('second')); undefined",
        )
        .expect_err("the first unhandled rejection must fail the checkpoint");
    assert_eq!(first.kind, ScriptErrorKind::Exception);
    assert_eq!(first.message.as_ref(), "Error: first");

    let second = engine
        .evaluate("globalThis.enteredBeforeRejectionsDrained = true")
        .expect_err("the second rejection must precede new JavaScript");
    assert_eq!(second.kind, ScriptErrorKind::Exception);
    assert_eq!(second.phase, ScriptErrorPhase::Evaluate);
    assert_eq!(second.message.as_ref(), "Error: second");

    assert!(matches!(
        evaluate(&mut engine, "typeof enteredBeforeRejectionsDrained"),
        Value::String(value) if value.as_ref() == "undefined"
    ));
}

#[test]
fn exactly_the_checkpoint_job_limit_is_not_an_error() {
    let config = QuickJsConfig::default().with_max_jobs_per_checkpoint(NonZeroUsize::MIN);
    let mut engine =
        QuickJsScriptEngine::with_config(config).expect("QuickJS realm should initialize");

    assert!(matches!(
        engine.evaluate(
            "globalThis.oneJob = 0; Promise.resolve().then(() => oneJob = 1); undefined",
        ),
        Ok(Value::Undefined)
    ));
    assert!(matches!(
        evaluate(&mut engine, "oneJob"),
        Value::Number(1.0)
    ));
}

#[test]
fn checkpoint_job_limit_is_configurable_and_enforced() {
    let config = QuickJsConfig::default().with_max_jobs_per_checkpoint(NonZeroUsize::MIN);
    let mut engine =
        QuickJsScriptEngine::with_config(config).expect("QuickJS realm should initialize");
    let error = engine
        .evaluate("Promise.resolve().then(() => 1); Promise.resolve().then(() => 2); undefined")
        .expect_err("two queued jobs should exceed a one-job checkpoint");

    assert_eq!(error.kind, ScriptErrorKind::Other);
    assert_eq!(error.phase, ScriptErrorPhase::Evaluate);
}

#[test]
fn an_incomplete_checkpoint_finishes_before_javascript_reentry() {
    let config = QuickJsConfig::default().with_max_jobs_per_checkpoint(NonZeroUsize::MIN);
    let mut engine =
        QuickJsScriptEngine::with_config(config).expect("QuickJS realm should initialize");
    let error = engine
        .evaluate(
            "globalThis.order = []; Promise.resolve().then(() => order.push('old-1')); Promise.resolve().then(() => order.push('old-2')); undefined",
        )
        .expect_err("the first checkpoint should leave one job queued");
    assert_eq!(error.kind, ScriptErrorKind::Other);

    assert!(matches!(
        engine.evaluate("order.push('new'); order.join(',')"),
        Ok(Value::String(value))
            if value.as_ref() == "old-1,old-2,new"
    ));
}

#[test]
fn import_value_reports_stable_unsupported_module_load() {
    let mut engine = engine();
    let result = poll_ready(engine.import_value("./module.js", "default"));
    let error = result.expect_err("modules are not wired into this bridge yet");

    assert_eq!(error.kind, ScriptErrorKind::ModuleLoad);
    assert_eq!(error.phase, ScriptErrorPhase::ImportValue);
}

#[derive(Debug)]
struct NullResourceFetcher;

impl NullResourceFetcher {
    fn failure<T>(phase: ResourceErrorPhase) -> ResourceFuture<'static, T> {
        Box::pin(async move {
            Err(ResourceError {
                request_id: None,
                kind: ResourceErrorKind::UnsupportedOperation,
                phase,
                locator: None,
                status: None,
                message: Arc::from("test fetcher does not load resources"),
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
        Self::failure(ResourceErrorPhase::Resolve)
    }

    fn fetch_resource(
        &self,
        _request: BufferedResourceRequest,
    ) -> ResourceFuture<'_, ResourceResponse> {
        Self::failure(ResourceErrorPhase::Open)
    }

    fn open_resource(&self, _request: ResourceRequest) -> ResourceFuture<'_, ResourceStream> {
        Self::failure(ResourceErrorPhase::Open)
    }

    fn fetch_resource_path(&self, _request: ResourceRequest) -> ResourceFuture<'_, ResourcePath> {
        Self::failure(ResourceErrorPhase::MaterializePath)
    }

    fn fetch_http(&self, _request: HttpRequest) -> ResourceFuture<'_, HttpResponse> {
        Self::failure(ResourceErrorPhase::Connect)
    }

    fn prefetch(&self, _request: PrefetchRequest) -> ResourceFuture<'_, PrefetchReceipt> {
        Self::failure(ResourceErrorPhase::Prefetch)
    }

    fn cancel_request(&self, _request_id: RequestId) -> ResourceFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[test]
fn quickjs_engine_composes_into_a_lynx_view() {
    let mut view = new_quickjs_view(NullResourceFetcher, ViewMetrics::new(390.0, 844.0, 3.0))
        .expect("view should initialize");

    assert_eq!(
        view.inner.script_engine().config(),
        QuickJsConfig::default()
    );
    assert!(format!("{:?}", view.widget_api()).contains("LynxWidgetApi"));
    let view_debug = format!("{view:?}");
    assert!(!view_debug.contains("ScriptEngine"));
    assert!(!view_debug.contains("QuickJsConfig"));
    assert!(matches!(
        view.inner.script_engine_mut().evaluate("6 * 7"),
        Ok(Value::Number(42.0))
    ));
    assert!(
        !view
            .resource_fetcher()
            .supports_capability(ResourceCapability::Http)
    );
}

#[test]
fn configuration_is_preserved_by_the_engine() {
    let config = QuickJsConfig::default()
        .with_memory_limit(Some(32 * 1024 * 1024))
        .with_max_stack_size(Some(512 * 1024))
        .with_max_jobs_per_checkpoint(NonZeroUsize::new(17).unwrap());
    let engine = QuickJsScriptEngine::with_config(config).expect("engine should initialize");

    assert_eq!(engine.config(), config);
    assert_eq!(engine.config().memory_limit(), Some(32 * 1024 * 1024));
    assert_eq!(engine.config().max_stack_size(), Some(512 * 1024));
    assert_eq!(engine.config().max_jobs_per_checkpoint().get(), 17);
}

fn poll_ready<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = Pin::from(Box::new(future));
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("unsupported import future unexpectedly suspended"),
    }
}
