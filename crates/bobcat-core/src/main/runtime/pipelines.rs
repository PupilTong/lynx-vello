//! The main-thread realms' half of `lynx.performance`: what a flush binds,
//! and the members a page realm and each `<frame>` card's realm mark their
//! pipelines and their load through.
//!
//! The main thread measures and nothing more. A flush that something times
//! turns the document's commit timing on around its commit and hands the
//! registry the commit id and the copied record; every other flush reads one
//! `bool` of the realm, two lengths of the slot and, when it names a
//! pipeline, one atomic count of the registry. No entry is assembled here and
//! the page epilogue does nothing for this.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use quickjs_rust_bridge::{HostArgument, HostValue};
use terminus::link::NodeId;

use super::{DocumentSlot, MainThreadError, borrow_slot, install};
use crate::background::WorkerKey;
use crate::clock::ClockInstant;
use crate::main::quickjs::ScriptEngine;
use crate::performance::{
    FlushBinding, LoadMark, Opener, OpenerId, PipelineOrigin, install_pipeline_members,
};
use crate::realm::string_argument;

pub(super) const TIMING_FLAG_ATTRIBUTE: &str = "__lynx_timing_flag";
pub(super) const CONTAINER_TIMING_ATTRIBUTE: &str = "containertiming";
pub(super) const CONTAINER_TIMING_IGNORE_ATTRIBUTE: &str = "containertimingignore";

pub(super) struct RealmTiming {
    pub(super) opener: OpenerId,
    load_pending: Cell<bool>,
    created: Cell<Option<(ClockInstant, ClockInstant)>>,
}

impl RealmTiming {
    pub(super) fn new(opener: OpenerId) -> Rc<Self> {
        Rc::new(Self {
            opener,
            load_pending: Cell::new(false),
            created: Cell::new(None),
        })
    }

    pub(super) fn set_created(&self, start: ClockInstant, end: ClockInstant) {
        self.created.set(Some((start, end)));
    }
}

pub(super) struct FlushRequest<'a> {
    pub(super) realm: &'a RealmTiming,
    pub(super) pipeline: u64,
    pub(super) flag: &'a str,
}

impl DocumentSlot {
    pub(super) fn flush(
        &mut self,
        thread: &crate::jobs::JsThreadHandle,
        request: &FlushRequest<'_>,
    ) -> Result<(), String> {
        let load = request.realm.load_pending.get();
        let others = load
            || !request.flag.is_empty()
            || !self.pending_flags.is_empty()
            || !self.native_pipelines.is_empty();
        let registry = Arc::clone(self.outbox.registry());
        let named = registry.claim_for_flush(request.pipeline, others);
        if !(others || named) {
            return self.commit_flush(thread).map(drop);
        }
        let started = ClockInstant::now();
        self.document_mut().set_commit_timing(true);
        let committed = self.commit_flush(thread);
        let document = self.document_mut();
        document.set_commit_timing(false);
        let commit_id = committed?;
        let timing = document
            .last_commit_timing()
            .filter(|timing| timing.commit_id == commit_id && timing.style_start >= started);
        let mut flags = Vec::with_capacity(1 + self.pending_flags.len());
        if !request.flag.is_empty() {
            flags.push(Arc::from(request.flag));
        }
        flags.append(&mut self.pending_flags);
        request.realm.load_pending.set(false);
        let completed = registry.bind_commit(FlushBinding {
            opener: request.realm.opener,
            pipeline: request.pipeline,
            flags,
            native: std::mem::take(&mut self.native_pipelines),
            load,
            commit_id,
            timing,
            started,
        });
        if completed {
            self.outbox.wake();
        }
        Ok(())
    }
}

impl DocumentSlot {
    pub(super) fn sync_content_detection(&mut self) {
        let wanted = self.outbox.registry().wants_content_detection();
        if wanted != self.content_detection {
            self.document_mut().set_content_detection(wanted);
            self.content_detection = wanted;
        }
    }

    pub(super) fn sync_container_timing(&mut self) {
        if self.outbox.registry().take_container_stop() {
            self.document_mut().stop_container_timing();
        }
    }

    pub(super) fn container_timing_changed(&mut self, node: NodeId, identifier: Option<&str>) {
        self.outbox
            .registry()
            .set_container_identifier(node, identifier);
        self.document_mut()
            .set_container_timing(node, identifier.is_some());
    }
}

pub(super) fn install_realm_members(
    engine: &mut ScriptEngine,
    slot: &Rc<RefCell<DocumentSlot>>,
    timing: &Rc<RealmTiming>,
) -> Result<(), MainThreadError> {
    let registry = Arc::clone(slot.borrow().outbox.registry());
    let opener = timing.opener;
    install_pipeline_members(engine, &registry, Opener::Realm(opener))
        .map_err(|error| MainThreadError::from_engine("installing the pipeline members", error))?;

    let pipelines = Arc::clone(&registry);
    let realm = Rc::clone(timing);
    install(engine, "markLoadTiming", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.markLoadTiming";
        match string_argument(NAME, arguments, 0)? {
            "vmExecuteStart" => {
                pipelines.open_load(
                    realm.opener,
                    string_argument(NAME, arguments, 1)?,
                    realm.created.get(),
                );
            }
            "vmExecuteEnd" => {
                pipelines.mark_load(realm.opener, LoadMark::VmExecuteEnd);
            }
            "mtsRenderStart" if pipelines.mark_load(realm.opener, LoadMark::MtsRenderStart) => {
                realm.load_pending.set(true);
            }
            _ => {}
        }
        Ok(HostValue::Undefined)
    })?;

    let pipelines = Arc::clone(&registry);
    let tree = Rc::clone(slot);
    install(engine, "openNativePipeline", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.openNativePipeline";
        let origin = PipelineOrigin::from_lynx_name(string_argument(NAME, arguments, 0)?)
            .unwrap_or(PipelineOrigin::UpdateTriggeredByNative);
        let id = pipelines.open_native(origin, string_argument(NAME, arguments, 1)?, opener);
        borrow_slot(NAME, &tree)?.native_pipelines.push(id);
        Ok(HostValue::Undefined)
    })?;

    let pipelines = registry;
    install(engine, "connectBackground", 1, move |arguments| {
        let Some(&HostArgument::Number(key)) = arguments.first() else {
            return Err("bobcat-internal:host.connectBackground requires a worker key".to_owned());
        };
        if !(key >= 1.0 && key.fract() == 0.0 && key <= 9_007_199_254_740_992.0) {
            return Err("bobcat-internal:host.connectBackground requires a worker key".to_owned());
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "an integer in 1..=2^53, checked above"
        )]
        pipelines.bind_worker(WorkerKey::new(key as u64), opener);
        Ok(HostValue::Undefined)
    })?;
    Ok(())
}
