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

/// The attribute a Lynx timing flag rides on: one written to any element is
/// bound to the next flush's commit.
pub(super) const TIMING_FLAG_ATTRIBUTE: &str = "__lynx_timing_flag";
/// W3C Container Timing's attributes (WICG draft): `containertiming`
/// marks its element a root whose `container` entries carry the value as
/// their `identifier`, and `containertimingignore` cuts its element's
/// subtree out of the roots above it. Like the exposure attributes, they
/// belong to every tag.
pub(super) const CONTAINER_TIMING_ATTRIBUTE: &str = "containertiming";
pub(super) const CONTAINER_TIMING_IGNORE_ATTRIBUTE: &str = "containertimingignore";

/// One main-thread realm's own timing state, shared by that realm's members:
/// which realm it is to the registry, whether its load has rendered and
/// waits for the flush that ends it, and — for a `<frame>` card's realm —
/// when the realm was constructed, which its load reports as `createLynx`.
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

    /// A card realm's construction span.
    pub(super) fn set_created(&self, start: ClockInstant, end: ClockInstant) {
        self.created.set(Some((start, end)));
    }
}

/// What one `flushElementTree` call carries beyond the commit: the realm
/// that called it and its two timing arguments.
pub(super) struct FlushRequest<'a> {
    pub(super) realm: &'a RealmTiming,
    /// `pipelineOptions.pipelineID`, 0 for none.
    pub(super) pipeline: u64,
    /// The `__lynx_timing_flag` option, `""` for none.
    pub(super) flag: &'a str,
}

impl DocumentSlot {
    /// `__FlushElementTree`: the commit, and — when anything times this
    /// flush — the binding of its pipelines to the frame it committed.
    ///
    /// Timed are a flush that names a pipeline carrying timestamps, one with
    /// a flag option, one after a flag attribute or a native pipeline, and
    /// the realm's first flush after its load rendered. Commit timing is on
    /// only for the commit itself, and the record is taken only when it is
    /// this flush's: a flush that built no frame binds to the frame already
    /// committed, with no stages of its own.
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
        // The document exists: the boot module creates it before any card
        // can flush.
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
            // Bound to a frame already presented: complete now, reported at
            // the embedder's next turn.
            self.outbox.wake();
        }
        Ok(())
    }
}

impl DocumentSlot {
    /// Turns the document's Paint Timing contentful test on while the
    /// registry wants it ([`crate::performance::PipelineRegistry::wants_content_detection`]:
    /// someone observes `paint` and `first-contentful-paint` has not been
    /// recorded) and off otherwise, before a commit. One atomic load and one
    /// `bool` compare per commit; the document is written only on a change.
    pub(super) fn sync_content_detection(&mut self) {
        let wanted = self.outbox.registry().wants_content_detection();
        if wanted != self.content_detection {
            self.document_mut().set_content_detection(wanted);
            self.content_detection = wanted;
        }
    }

    /// Ends Container Timing in the document when the painter facade saw the
    /// view's first pointer down, wheel or key down since the last commit.
    /// One relaxed atomic load per commit otherwise.
    pub(super) fn sync_container_timing(&mut self) {
        if self.outbox.registry().take_container_stop() {
            self.document_mut().stop_container_timing();
        }
    }

    /// `node`'s `containertiming` attribute was set to `identifier`, or
    /// removed (`None`): the document marks or unmarks the root, and the
    /// registry keeps the identifier its entries carry.
    pub(super) fn container_timing_changed(&mut self, node: NodeId, identifier: Option<&str>) {
        self.outbox
            .registry()
            .set_container_identifier(node, identifier);
        self.document_mut()
            .set_container_timing(node, identifier.is_some());
    }
}

/// Installs a main-thread realm's performance members: the four pipeline
/// hooks every realm kind has, opening records as this realm, and
///
/// - `markLoadTiming(key, url)`: the realm's load milestones — `vmExecuteStart` (the load of the
///   bundle at `url` begins), then `vmExecuteEnd` and `mtsRenderStart`, after which the realm's
///   next flush ends the load; any other key does nothing;
/// - `openNativePipeline(origin, flag)`: a host update arrived with a flag, or a reload arrived,
///   and the next flush binds it;
/// - `connectBackground(key)`: the worker under `key` is this realm's BTS.
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
