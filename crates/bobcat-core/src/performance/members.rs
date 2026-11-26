//! The host members a realm's `lynx.performance` is written over, under
//! `bobcat-internal:host`: the four pipeline hooks every realm kind has, and
//! the members only a worker realm has — its observer switch, the drain its
//! `PerformanceReady` job calls, and its load marks.
//!
//! Every member takes and answers primitives. What `ReactLynx` passes as an
//! object (`pipelineOptions`) is unpacked by `bobcat:performance` before it
//! gets here, and what a worker is handed back is one record string, folded
//! into objects in JavaScript.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use quickjs_rust_bridge::{HostArgument, HostValue};

use super::{EntryTypes, Opener, PipelineOrigin, PipelineRegistry, WorkerLoadMark, encode_entries};
use crate::background::WorkerKey;
use crate::esm::HOST_MODULE_SPECIFIER;
use crate::link::HostOutbox;
use crate::main::quickjs::ScriptEngine;
use crate::realm::string_argument;
use crate::script::ScriptError;

const MAX_PIPELINE_ID: u64 = 1 << 53;

pub(crate) fn pipeline_argument(arguments: &[HostArgument<'_>], index: usize) -> u64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "2^53, which an f64 holds exactly"
    )]
    let max = MAX_PIPELINE_ID as f64;
    match arguments.get(index) {
        Some(&HostArgument::Number(value))
            if value >= 1.0 && value.fract() == 0.0 && value <= max =>
        {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "an integer in 1..=2^53, checked above"
            )]
            let id = value as u64;
            id
        }
        _ => 0,
    }
}

fn flag_argument(arguments: &[HostArgument<'_>], index: usize) -> bool {
    match arguments.get(index) {
        Some(&HostArgument::Number(value)) => value != 0.0,
        Some(&HostArgument::Boolean(value)) => value,
        _ => false,
    }
}

fn register(
    engine: &mut ScriptEngine,
    name: &str,
    arity: u8,
    callback: impl FnMut(&[HostArgument<'_>]) -> Result<HostValue, String> + 'static,
) -> Result<(), ScriptError> {
    engine.register_host_module_function(HOST_MODULE_SPECIFIER, name, arity, callback)
}

pub(crate) fn install_pipeline_members(
    engine: &mut ScriptEngine,
    registry: &Arc<PipelineRegistry>,
    opener: Opener,
) -> Result<(), ScriptError> {
    let pipelines = Arc::clone(registry);
    register(engine, "generatePipelineId", 0, move |_arguments| {
        #[expect(
            clippy::cast_precision_loss,
            reason = "ids stay far below 2^53, which an f64 holds exactly"
        )]
        Ok(HostValue::Number(pipelines.generate_id() as f64))
    })?;
    let pipelines = Arc::clone(registry);
    register(engine, "onPipelineStart", 5, move |arguments| {
        const NAME: &str = "bobcat-internal:host.onPipelineStart";
        let id = pipeline_argument(arguments, 0);
        let origin = PipelineOrigin::from_lynx_name(string_argument(NAME, arguments, 1)?);
        let dsl = string_argument(NAME, arguments, 2)?;
        let stage = string_argument(NAME, arguments, 3)?;
        let need_timestamps = flag_argument(arguments, 4);
        pipelines.open(id, origin, dsl, stage, need_timestamps, opener);
        Ok(HostValue::Undefined)
    })?;
    let pipelines = Arc::clone(registry);
    register(engine, "markPipelineTiming", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.markPipelineTiming";
        let id = pipeline_argument(arguments, 0);
        pipelines.mark(id, string_argument(NAME, arguments, 1)?);
        Ok(HostValue::Undefined)
    })?;
    let pipelines = Arc::clone(registry);
    register(engine, "bindPipelineTimingFlag", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.bindPipelineTimingFlag";
        let id = pipeline_argument(arguments, 0);
        pipelines.bind_flag(id, string_argument(NAME, arguments, 1)?, opener);
        Ok(HostValue::Undefined)
    })?;
    Ok(())
}

pub(crate) fn install_worker_members(
    engine: &mut ScriptEngine,
    host: &HostOutbox,
    key: WorkerKey,
) -> Result<Rc<Cell<bool>>, ScriptError> {
    let registry = host.registry();
    install_pipeline_members(engine, registry, Opener::Worker(key))?;
    let observed = Rc::new(Cell::new(false));
    let pipelines = Arc::clone(registry);
    let setting = Rc::clone(&observed);
    register(engine, "observePerformance", 3, move |arguments| {
        let on = flag_argument(arguments, 0);
        let asked = [EntryTypes::PAINT, EntryTypes::RESOURCE]
            .into_iter()
            .enumerate()
            .filter(|&(index, _)| flag_argument(arguments, index + 1))
            .fold(EntryTypes::NONE, |asked, (_, kind)| asked | kind);
        setting.set(on);
        pipelines.observe_worker(key, on.then_some(asked));
        Ok(HostValue::Undefined)
    })?;
    let pipelines = Arc::clone(registry);
    register(engine, "takePerformanceEntries", 0, move |_arguments| {
        let entries = pipelines.take_completed_for_worker(key);
        Ok(HostValue::String(encode_entries(
            &entries,
            &pipelines.time_origin(),
        )))
    })?;
    let pipelines = Arc::clone(registry);
    let waking = host.clone();
    register(engine, "markLoadTiming", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.markLoadTiming";
        let Some(mark) = WorkerLoadMark::from_realm_key(string_argument(NAME, arguments, 0)?)
        else {
            return Ok(HostValue::Boolean(false));
        };
        let completion = pipelines.mark_worker_load(key, mark);
        if completion.completed {
            waking.wake();
        }
        Ok(HostValue::Boolean(completion.owed_to_caller))
    })?;
    Ok(observed)
}
