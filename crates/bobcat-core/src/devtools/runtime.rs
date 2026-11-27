//! The `Runtime` and `Console` domains of one realm, and the realm's console
//! tap. One implementation for the MTS realm and every worker realm.

mod remote;
#[cfg(test)]
mod tests;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use quickjs_rust_bridge::{
    self as quickjs, Context, EvalOptions, EvalSource, HostValue, OwnKeys, OwnProperty,
    PropertyKey, PropertySlot, Reflected, Value, ValueKind,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

use self::remote::{Mirror, ObjectTable, RemoteObject, by_value, reflection};
use super::Frontend;
use super::protocol::{Error, Request};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::realm::RealmCore;
use crate::script::ScriptError;

const CONTEXT_ID: i64 = 1;
const CONSOLE_GROUP: &str = "console";
const DIAGNOSTICS: &str = "bobcat:diagnostics";
const LEVELS: [&str; 5] = ["log", "info", "debug", "warn", "error"];
const REPLACEMENT: &str = "\"use strict\"; (hook, original, level) => \
    ({ [level](...args) { return hook(level, original, this, args); } })[level]";
const SIDE_EFFECT: &str = "EvalError: Possible side-effect in debug-evaluate";

#[derive(Default)]
pub(crate) struct RuntimeState(Option<Rc<Shared>>);

struct Shared {
    frontend: Frontend,
    objects: RefCell<ObjectTable>,
    exceptions: Cell<i64>,
}

struct Script<'a> {
    engine: &'a ScriptEngine,
    js: &'a mut ScriptRuntime,
    left: Result<(), ScriptError>,
}

impl Script<'_> {
    fn run<T>(&mut self, run: impl FnOnce(&Context) -> T) -> T {
        let (value, left) = self.engine.devtools_run(self.js, run);
        if self.left.is_ok() {
            self.left = left;
        }
        value
    }
}

pub(crate) fn dispatch(
    state: &mut RuntimeState,
    core: &RealmCore,
    js: &mut ScriptRuntime,
    frontend: &Frontend,
    request: &Request,
    console_ready: bool,
) -> Result<(), ScriptError> {
    let shared = state.0.get_or_insert_with(|| {
        Rc::new(Shared {
            frontend: frontend.clone(),
            objects: RefCell::default(),
            exceptions: Cell::new(0),
        })
    });
    let mut script = Script {
        engine: &core.engine,
        js,
        left: Ok(()),
    };
    let tap = &core.console;
    if tap.on.get() && console_ready && !tap.installed.get() {
        tap.installed
            .set(install_console(core, &mut script).is_ok());
    }
    let id = request.id;
    let done = serde_json::Map::new();
    match request.method.as_str() {
        "Runtime.enable" => {
            if !tap.on.get() {
                if console_ready && !tap.installed.get() {
                    tap.installed
                        .set(install_console(core, &mut script).is_ok());
                }
                shared.enable(tap);
            }
            frontend.result(id, &done);
        }
        "Runtime.disable" => {
            tap.on.set(false);
            shared.objects.borrow_mut().clear();
            frontend.result(id, &done);
        }
        "Runtime.runIfWaitingForDebugger" => frontend.result(id, &done),
        "Runtime.evaluate" => frontend.reply(
            id,
            request
                .params()
                .and_then(|params| shared.evaluate(core, &mut script, &params)),
        ),
        "Runtime.callFunctionOn" => frontend.reply(
            id,
            request
                .params()
                .and_then(|params| shared.call_function_on(&mut script, &params)),
        ),
        "Runtime.getProperties" => frontend.reply(
            id,
            request
                .params()
                .and_then(|params| shared.properties(&core.engine, &params)),
        ),
        "Runtime.releaseObject" => frontend.reply(
            id,
            request.params().and_then(|params: Release| {
                shared.objects.borrow_mut().release(&params.object_id)?;
                Ok(done)
            }),
        ),
        "Runtime.releaseObjectGroup" => frontend.reply(
            id,
            request.params().map(|params: ReleaseGroup| {
                shared
                    .objects
                    .borrow_mut()
                    .release_group(&params.object_group);
                done
            }),
        ),
        _ => frontend.not_found(request),
    }
    script.left
}

pub(crate) fn detach(_state: &mut RuntimeState, core: &RealmCore) {
    core.console.on.set(false);
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the CDP method's own boolean parameters"
)]
struct Evaluate {
    expression: String,
    object_group: Option<String>,
    #[serde(default)]
    return_by_value: bool,
    #[serde(default)]
    generate_preview: bool,
    #[serde(default)]
    throw_on_side_effect: bool,
    #[serde(default)]
    await_promise: bool,
    context_id: Option<i64>,
    unique_context_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the CDP method's own boolean parameters"
)]
struct CallFunctionOn {
    function_declaration: String,
    object_id: Option<String>,
    #[serde(default)]
    arguments: Vec<CallArgument>,
    object_group: Option<String>,
    #[serde(default)]
    return_by_value: bool,
    #[serde(default)]
    generate_preview: bool,
    #[serde(default)]
    throw_on_side_effect: bool,
    #[serde(default)]
    await_promise: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallArgument {
    #[serde(default, deserialize_with = "present")]
    value: Option<Box<RawValue>>,
    unserializable_value: Option<String>,
    object_id: Option<String>,
}

fn present<'de, D: Deserializer<'de>>(value: D) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(value).map(Some)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the CDP method's own boolean parameters"
)]
struct GetProperties {
    object_id: String,
    #[serde(default)]
    own_properties: bool,
    #[serde(default)]
    accessor_properties_only: bool,
    #[serde(default)]
    non_indexed_properties_only: bool,
    #[serde(default)]
    generate_preview: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Release {
    object_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseGroup {
    object_group: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Evaluated {
    result: RemoteObject,
    #[serde(skip_serializing_if = "Option::is_none")]
    exception_details: Option<ExceptionDetails>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExceptionDetails {
    exception_id: i64,
    text: &'static str,
    line_number: u32,
    column_number: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    exception: RemoteObject,
    execution_context_id: i64,
}

#[derive(Serialize)]
struct ContextCreated<'a> {
    context: ContextDescription<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextDescription<'a> {
    id: i64,
    origin: &'a str,
    name: &'a str,
    unique_id: &'a str,
    aux_data: AuxData,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuxData {
    is_default: bool,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConsoleApiCalled {
    #[serde(rename = "type")]
    kind: &'static str,
    args: Vec<RemoteObject>,
    execution_context_id: i64,
    timestamp: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExceptionThrown {
    timestamp: f64,
    exception_details: ExceptionDetails,
}

struct Answer<'a> {
    group: Option<&'a str>,
    by_value: bool,
    preview: bool,
}

#[derive(Default)]
struct Location {
    url: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
}

fn install_console(core: &RealmCore, script: &mut Script<'_>) -> Result<(), Error> {
    let engine = &core.engine;
    let realm = engine.devtools_realm();
    let failed = |error: quickjs::Error| reflection(&error);
    let namespace = realm.module_namespace(DIAGNOSTICS).map_err(failed)?;
    let Some(console) = data_property(realm, &namespace, "console")
        .filter(|console| console.kind() == ValueKind::Object)
    else {
        return Err(Error::server("the realm has no console"));
    };
    let hook = engine
        .devtools_function("", 4, console_hook(Rc::clone(&core.console)))
        .map_err(failed)?;
    script.run(|realm| {
        let factory = realm
            .evaluate(EvalSource::new(REPLACEMENT), EvalOptions::default())
            .map_err(failed)?;
        for level in LEVELS {
            let Some(original) = data_property(realm, &console, level)
                .filter(|method| method.kind() == ValueKind::Function)
            else {
                continue;
            };
            let name = realm.string(level).map_err(failed)?;
            let replacement = realm
                .call(&factory, None, &[hook.clone(), original, name])
                .map_err(failed)?;
            realm
                .set_property(&console, level, &replacement)
                .map_err(failed)?;
        }
        Ok(())
    })
}

fn check_context(core: &RealmCore, params: &Evaluate) -> Result<(), Error> {
    if params.context_id.is_some_and(|id| id != CONTEXT_ID)
        || params
            .unique_context_id
            .as_deref()
            .is_some_and(|id| id != core.console.unique_id())
    {
        return Err(Error::server("Cannot find context with specified id"));
    }
    Ok(())
}

impl Shared {
    fn enable(self: &Rc<Self>, tap: &ConsoleTap) {
        *tap.shared.borrow_mut() = Rc::downgrade(self);
        tap.on.set(true);
        self.frontend.event(
            "Runtime.executionContextCreated",
            &ContextCreated {
                context: ContextDescription {
                    id: CONTEXT_ID,
                    origin: "",
                    name: "",
                    unique_id: tap.unique_id(),
                    aux_data: AuxData {
                        is_default: true,
                        kind: "default",
                    },
                },
            },
        );
    }

    fn evaluate(
        &self,
        core: &RealmCore,
        script: &mut Script<'_>,
        params: &Evaluate,
    ) -> Result<Evaluated, Error> {
        check_context(core, params)?;
        if params.await_promise {
            return Err(awaiting());
        }
        if params.throw_on_side_effect {
            return Ok(self.side_effect());
        }
        let answer = Answer {
            group: params.object_group.as_deref(),
            by_value: params.return_by_value,
            preview: params.generate_preview,
        };
        script.run(|realm| {
            let outcome =
                realm.evaluate(EvalSource::new(&params.expression), EvalOptions::default());
            self.answer(realm, outcome, &answer)
        })
    }

    fn call_function_on(
        &self,
        script: &mut Script<'_>,
        params: &CallFunctionOn,
    ) -> Result<Evaluated, Error> {
        if params.await_promise {
            return Err(awaiting());
        }
        if params.throw_on_side_effect {
            return Ok(self.side_effect());
        }
        let (this, held_group) = match &params.object_id {
            Some(id) => {
                let objects = self.objects.borrow();
                let held = objects.get(id)?;
                (Some(held.value.clone()), held.group.clone())
            }
            None => (None, None),
        };
        let answer = Answer {
            group: params.object_group.as_deref().or(held_group.as_deref()),
            by_value: params.return_by_value,
            preview: params.generate_preview,
        };
        script.run(|realm| {
            let arguments = params
                .arguments
                .iter()
                .map(|argument| self.argument(realm, argument))
                .collect::<Result<Vec<_>, _>>()?;
            let source = format!("({}\n)", params.function_declaration);
            let outcome = realm
                .evaluate(EvalSource::new(&source), EvalOptions::default())
                .and_then(|function| realm.call(&function, this.as_ref(), &arguments));
            self.answer(realm, outcome, &answer)
        })
    }

    fn argument(&self, realm: &Context, argument: &CallArgument) -> Result<Value, Error> {
        let invalid = |error: quickjs::Error| Error::invalid_params(error.message);
        if let Some(id) = &argument.object_id {
            return Ok(self.objects.borrow().get(id)?.value.clone());
        }
        if let Some(text) = &argument.unserializable_value {
            let number = match text.as_str() {
                "NaN" => f64::NAN,
                "Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                "-0" => -0.0,
                other => {
                    let digits = other.strip_suffix('n').ok_or_else(|| {
                        Error::invalid_params(format!("unknown unserializable value {other}"))
                    })?;
                    return realm.big_int_decimal(digits).map_err(invalid);
                }
            };
            return realm.number(number).map_err(invalid);
        }
        match &argument.value {
            Some(json) => realm.json_parse(json.get()),
            None => realm.undefined(),
        }
        .map_err(invalid)
    }

    fn answer(
        &self,
        realm: &Context,
        outcome: Result<Value, quickjs::Error>,
        answer: &Answer<'_>,
    ) -> Result<Evaluated, Error> {
        match outcome {
            Ok(value) if answer.by_value => Ok(Evaluated {
                result: by_value(realm, &value)?,
                exception_details: None,
            }),
            Ok(value) => {
                let result = Mirror {
                    realm,
                    objects: &mut self.objects.borrow_mut(),
                    group: answer.group,
                }
                .remote(&value, answer.preview)
                .map_err(|error| reflection(&error))?;
                Ok(Evaluated {
                    result,
                    exception_details: None,
                })
            }
            Err(error) => {
                let exception = thrown(&error);
                let location = error
                    .location
                    .map_or_else(Location::default, |location| Location {
                        url: location.source,
                        line: location.line,
                        column: location.column,
                    });
                Ok(Evaluated {
                    result: exception.clone(),
                    exception_details: Some(self.details(exception, location)),
                })
            }
        }
    }

    fn side_effect(&self) -> Evaluated {
        let exception = RemoteObject::error("EvalError", SIDE_EFFECT.to_owned());
        Evaluated {
            result: exception.clone(),
            exception_details: Some(self.details(exception, Location::default())),
        }
    }

    fn details(&self, exception: RemoteObject, location: Location) -> ExceptionDetails {
        self.exceptions.set(self.exceptions.get() + 1);
        let zero_based = |number: Option<u32>| number.unwrap_or(1).saturating_sub(1);
        ExceptionDetails {
            exception_id: self.exceptions.get(),
            text: "Uncaught",
            line_number: zero_based(location.line),
            column_number: zero_based(location.column),
            url: location.url,
            exception,
            execution_context_id: CONTEXT_ID,
        }
    }

    fn properties(
        &self,
        engine: &ScriptEngine,
        params: &GetProperties,
    ) -> Result<remote::Properties, Error> {
        let mut objects = self.objects.borrow_mut();
        let held = objects.get(&params.object_id)?;
        let (object, group) = (held.value.clone(), held.group.clone());
        Mirror {
            realm: engine.devtools_realm(),
            objects: &mut objects,
            group: group.as_deref(),
        }
        .properties(
            &object,
            params.own_properties,
            params.accessor_properties_only,
            if params.non_indexed_properties_only {
                OwnKeys::NonIndexed
            } else {
                OwnKeys::All
            },
            params.generate_preview,
        )
        .map_err(|error| reflection(&error))
    }

    fn console_api_called(&self, realm: &Context, kind: &'static str, values: &[Value]) {
        let args = {
            let mut mirror = Mirror {
                realm,
                objects: &mut self.objects.borrow_mut(),
                group: Some(CONSOLE_GROUP),
            };
            values
                .iter()
                .map(|value| mirror.remote(value, true))
                .collect::<Result<Vec<_>, _>>()
        };
        if let Ok(args) = args {
            self.emit(kind, args);
        }
    }

    fn emit(&self, kind: &'static str, args: Vec<RemoteObject>) {
        self.frontend.event(
            "Runtime.consoleAPICalled",
            &ConsoleApiCalled {
                kind,
                args,
                execution_context_id: CONTEXT_ID,
                timestamp: now_millis(),
            },
        );
    }

    fn exception_thrown(&self, error: &ScriptError) {
        let message = error.message.as_ref();
        let class_name = message
            .split_once(": ")
            .map(|(name, _)| name)
            .filter(|name| !name.is_empty() && !name.contains(char::is_whitespace))
            .unwrap_or("Error");
        let location = error
            .location
            .as_ref()
            .map_or_else(Location::default, |location| Location {
                url: location.source.as_deref().map(str::to_owned),
                line: location.line,
                column: location.column,
            });
        self.frontend.event(
            "Runtime.exceptionThrown",
            &ExceptionThrown {
                timestamp: now_millis(),
                exception_details: self.details(
                    RemoteObject::error(class_name, message.to_owned()),
                    location,
                ),
            },
        );
    }
}

fn awaiting() -> Error {
    Error::server("awaitPromise is not supported")
}

fn thrown(error: &quickjs::Error) -> RemoteObject {
    let Some(name) = error.name.as_deref() else {
        return RemoteObject::string(&error.message);
    };
    let mut description = if error.message.is_empty() {
        name.to_owned()
    } else {
        format!("{name}: {}", error.message)
    };
    if let Some(stack) = error.stack.as_deref().map(str::trim_end)
        && !stack.is_empty()
    {
        description.push('\n');
        description.push_str(stack);
    }
    RemoteObject::error(name, description)
}

fn data_property(realm: &Context, object: &Value, name: &str) -> Option<Value> {
    match realm.own_property(object, name) {
        Ok(Reflected::Read(Some(OwnProperty {
            slot: PropertySlot::Data { value, .. },
            ..
        }))) => Some(value),
        _ => None,
    }
}

fn console_hook(tap: Rc<ConsoleTap>) -> impl Fn(&Context, &[Value]) -> Result<HostValue, String> {
    move |realm, arguments| {
        let [level, original, this, list] = arguments else {
            return Ok(HostValue::Undefined);
        };
        let values = elements(realm, list);
        let shared = tap.shared.borrow().upgrade();
        if tap.on.get()
            && let Some(shared) = shared
        {
            let level = level
                .to_utf16()
                .map(|units| String::from_utf16_lossy(&units))
                .unwrap_or_default();
            shared.console_api_called(realm, console_type(&level), &values);
        }
        tap.replaced.set(tap.replaced.get() + 1);
        let called = realm.call(original, Some(this), &values);
        tap.replaced.set(tap.replaced.get() - 1);
        called
            .map(|_| HostValue::Undefined)
            .map_err(|error| error.message)
    }
}

fn elements(realm: &Context, list: &Value) -> Vec<Value> {
    let Ok(Reflected::Read(own)) = realm.own_properties(list, OwnKeys::All) else {
        return Vec::new();
    };
    own.into_iter()
        .map_while(|property| match property {
            OwnProperty {
                key: PropertyKey::String(name),
                slot: PropertySlot::Data { value, .. },
                ..
            } if name.parse::<u32>().is_ok() => Some(value),
            _ => None,
        })
        .collect()
}

fn console_type(level: &str) -> &'static str {
    match level {
        "info" => "info",
        "debug" => "debug",
        "warn" => "warning",
        "error" => "error",
        _ => "log",
    }
}

fn now_millis() -> f64 {
    use crate::clock::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConsoleCall {
    Log,
    Report,
}

#[derive(Default)]
pub(crate) struct ConsoleTap {
    on: Cell<bool>,
    installed: Cell<bool>,
    replaced: Cell<u32>,
    shared: RefCell<Weak<Shared>>,
    unique_id: OnceCell<String>,
    #[cfg(test)]
    live: std::sync::Arc<()>,
}

impl ConsoleTap {
    #[inline]
    pub(crate) fn is_on(&self) -> bool {
        self.on.get()
    }

    pub(crate) fn message(&self, call: ConsoleCall, level: &str, text: &str) {
        let kind = match call {
            ConsoleCall::Log if self.replaced.get() > 0 => return,
            ConsoleCall::Log => console_type(level),
            ConsoleCall::Report if level == "warn" => "warning",
            ConsoleCall::Report => "error",
        };
        let shared = self.shared.borrow().upgrade();
        if let Some(shared) = shared {
            shared.emit(kind, vec![RemoteObject::string(text)]);
        }
    }

    pub(crate) fn exception(&self, error: &ScriptError) {
        let shared = self.shared.borrow().upgrade();
        if let Some(shared) = shared {
            shared.exception_thrown(error);
        }
    }

    fn unique_id(&self) -> &str {
        self.unique_id.get_or_init(|| {
            static LAST: AtomicU64 = AtomicU64::new(0);
            let id = (LAST.fetch_add(1, Ordering::Relaxed) + 1).to_string();
            #[cfg(test)]
            tests::register_tap(&id, &self.live);
            id
        })
    }
}
