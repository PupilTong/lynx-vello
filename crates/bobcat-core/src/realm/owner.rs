//! Driving a realm once it is open: the one driver a view's page on
//! `bobcat-main` and a worker on `bobcat-workers` both run.
//!
//! Each of the two owns one realm and drives it the same way. Its tasks are
//! spawned under a guard that ends it if one of them unwinds; each task
//! reaches the realm through one job at a time, which runs one synchronous
//! operation and then the epilogue; one latch ends it, and one report says
//! why; and the owner's own task waits for that end, reclaims the tasks and
//! releases the realm. [`RealmOwner`] is what each of the two supplies —
//! where its runtime, its realm, its host and its [`Lifetime`] are, where
//! what it reports goes, which of the tables in [`policy`] its failures are
//! read from, and what its role adds to an entry, to the epilogue, to the end
//! and to the release — and the functions here are the rest, written once.
//!
//! # Entries
//!
//! [`enter`] is the one way into an owner's realm: it queues one job and
//! answers with what the operation returned. [`enter_now`] is that job's
//! body: [`RealmOwner::before_operation`], which is what the owner's role
//! does at the start of every entry, then the operation, then the epilogue.
//! The job that opens a realm cannot go through [`enter`], because the realm
//! does not exist until that job stores it, and runs [`enter_now`] inline once
//! it has, for the realm's first epilogue. [`after_end`] is the job for what
//! runs against a realm after the end, past the latch and the epilogue.
//!
//! # The epilogue
//!
//! Every entry ends in [`epilogue`], one function for both kinds of owner,
//! which settles what the entry left owing. The steps both kinds have are
//! written there; what only one role has is a hook of [`RealmOwner`]'s, run at
//! a fixed place among them, and a hook a role does not need is empty. The
//! order of the steps is the contract, and [`epilogue`] lists it.
//!
//! The module requests an entry left waiting — its imports, and a worker's
//! load of its root module — and the futures it asked to settle are each a
//! task the epilogue spawns — [`load_module`] and [`settle_future`] — which
//! waits for its answer outside any job and then enters the realm to hand it
//! over. A realm's `ShadowRealm` children ([`super::child`]) have no entries
//! of their own: their code runs inside the owner's, and the epilogue
//! services them beside the realm — their timers, their module requests and
//! future settles ([`load_child_module`], [`settle_child_future`]), what they
//! left unhandled, and their release. [`module_answer`] is the one reading of
//! a module request's answer, for those loads and for the entry a role
//! completes itself. A request for a native module's ES module, `lynx:<name>`,
//! never reaches the host: [`load_native_esm`] completes it from the bytecode
//! the view's outbox carries, or fails it naming the specifier, and a
//! `ShadowRealm` child's is failed the same way.
//!
//! # The end
//!
//! [`end`] sets the latch, once, and runs what the owner's role owes an end.
//! [`terminal`] reports why an owner is over and ends it: a view's
//! `StartupFailed`, a worker's `Failed`, or the `Closed` of a worker that
//! closed itself. [`trapped`] reports a panic and ends the owner. Each report
//! goes through a latch of its own on the lifetime, so one end is one report
//! and a panic after it is still reported.
//!
//! A panic that unwinds an owner's own task — `serve_view` or `serve_worker` —
//! or a whole engine thread is not reported here: the `Rc` of the owner that
//! task held is dropped with it, so the report is the thread's, read from the
//! thread's own table of who to tell. The event it sends is built by the
//! same Panic row [`trapped`] reports through.
//!
//! Every failure the driver itself meets — a timer callback, a module load, a
//! future's settle, and the root module's rejection where the owner's
//! [`RealmOwner::BOOT_REJECTION`] names a scene — is reported through
//! [`policy::report`], under the [`Scene`] it happened in.

use std::any::Any;
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use bytes::Bytes;

use super::RealmCore;
use super::child::{ChildKey, ChildRealms};
use super::policy::{self, Row, Scene};
use crate::clock::ClockInstant;
use crate::esm::NATIVE_MODULE_ESM_PREFIX;
use crate::future::HostFuture;
use crate::jobs::{JobKind, JsThreadName};
use crate::lifetime::{EndOnUnwind, Lifetime, Settles, run_job};
use crate::link::{HostOutbox, SourceAnswer};
use crate::main::quickjs::{ScriptRuntime, SharedRuntime};
use crate::resource::{LoadedSource, SourceRequest, unanswered_source};
use crate::script::ScriptError;
use crate::view::LynxViewError;

pub(crate) trait RealmOwner: Sized + 'static {
    type Realm: 'static;
    type Event;

    const THREAD: JsThreadName;

    fn lifetime(&self) -> &Lifetime;

    fn runtime(&self) -> &SharedRuntime;

    fn realm(&self) -> &RefCell<Option<Box<Self::Realm>>>;

    fn core(realm: &mut Self::Realm) -> &RealmCore;

    fn host(&self) -> &HostOutbox;

    fn send(&self, event: Self::Event);

    fn row(scene: Scene) -> Row<Self::Event>;

    fn panic_row() -> Row<Self::Event>;

    fn before_operation(&self, _realm: &mut Self::Realm) {}

    fn run_due_timers(realm: &mut Self::Realm, js: &mut ScriptRuntime) -> Vec<ScriptError>;

    fn after_timers(_owner: &Rc<Self>, _realm: &mut Self::Realm) {}

    fn booted(&self) -> bool;

    fn mark_booted(&self);

    fn on_booted(&self) {}

    const BOOT_REJECTION: Option<(Scene, Option<&'static str>)>;

    fn after_boot(&self) {}

    fn entry_name(&self, _realm: &Self::Realm) -> Option<String> {
        None
    }

    fn after_settles(_owner: &Rc<Self>, _realm: &mut Self::Realm) {}

    fn next_deadline(realm: &mut Self::Realm) -> Option<ClockInstant> {
        Self::core(realm).timers.next_deadline()
    }

    fn on_end(&self) {}

    fn before_release(_owner: &Rc<Self>) -> impl Future<Output = ()> {
        std::future::ready(())
    }
}

pub(crate) fn spawn<O: RealmOwner>(owner: &Rc<O>, future: impl Future<Output = ()> + 'static) {
    let guard = EndOnUnwind::new(owner);
    owner.lifetime().spawn(async move {
        let _guard = guard;
        future.await;
    });
}

pub(crate) fn end<O: RealmOwner>(owner: &Rc<O>) {
    if owner.lifetime().end() {
        owner.on_end();
    }
}

pub(crate) fn terminal<O: RealmOwner>(owner: &Rc<O>, event: O::Event) {
    if owner.lifetime().report_terminal() {
        owner.send(event);
    }
    end(owner);
}

pub(crate) fn trapped<O: RealmOwner>(owner: &Rc<O>, payload: &(dyn Any + Send)) {
    if owner.lifetime().report_panic() {
        owner.send(O::panic_row().event_for_panic(payload));
    }
    end(owner);
}

pub(crate) fn enter<O, T, F>(
    owner: &Rc<O>,
    kind: JobKind,
    operation: F,
) -> impl Future<Output = Option<T>> + use<O, T, F>
where
    O: RealmOwner,
    T: 'static,
    F: FnOnce(&mut O::Realm, &mut ScriptRuntime) -> T + 'static,
{
    run_job(owner, kind, move |owner| enter_now(owner, operation))
}

pub(crate) fn queue<O, F>(owner: &Rc<O>, kind: JobKind, operation: F)
where
    O: RealmOwner,
    F: FnOnce(&mut O::Realm, &mut ScriptRuntime) + 'static,
{
    drop(enter(owner, kind, operation));
}

pub(crate) fn enter_now<O: RealmOwner, T>(
    owner: &Rc<O>,
    operation: impl FnOnce(&mut O::Realm, &mut ScriptRuntime) -> T,
) -> Option<T> {
    if owner.lifetime().ended() {
        return None;
    }
    let mut js = owner.runtime().borrow_mut();
    let Ok(js) = js.as_mut() else {
        return None;
    };
    let mut realm = owner.realm().borrow_mut();
    let realm = realm.as_deref_mut()?;
    owner.before_operation(realm);
    let value = operation(realm, js);
    epilogue(owner, realm, js);
    Some(value)
}

fn epilogue<O: RealmOwner>(owner: &Rc<O>, realm: &mut O::Realm, js: &mut ScriptRuntime) {
    let lifetime = owner.lifetime();
    if lifetime.ended() {
        return;
    }
    #[cfg(test)]
    lifetime.count_epilogue();
    for error in O::run_due_timers(realm, js) {
        policy::report(owner, Scene::Timer, error);
    }
    if lifetime.ended() {
        return;
    }
    O::core(realm).settle_drawn_canvases();
    O::after_timers(owner, realm);
    if lifetime.ended() {
        return;
    }
    if !owner.booted() {
        match O::core(realm).engine.module_finished() {
            Ok(false) => {}
            Ok(true) => {
                owner.mark_booted();
                owner.on_booted();
            }
            Err(error) => {
                owner.mark_booted();
                if let Some((scene, context)) = O::BOOT_REJECTION {
                    let error = match context {
                        Some(context) => policy::context_of(context, error),
                        None => error,
                    };
                    policy::report(owner, scene, error);
                }
            }
        }
    }
    if lifetime.ended() {
        return;
    }
    owner.after_boot();
    while let Some(url) = O::core(realm).engine.take_module_request() {
        if owner.entry_name(realm).is_some_and(|entry| entry == url) {
            continue;
        }
        if url.starts_with(NATIVE_MODULE_ESM_PREFIX) {
            let bytecode = owner.host().native_esms().get(&url).cloned();
            spawn(owner, load_native_esm(Rc::clone(owner), url, bytecode));
            continue;
        }
        let answer = owner.host().request(SourceRequest::Module(url.clone()));
        spawn(owner, load_module(Rc::clone(owner), url, answer));
    }
    let children = Rc::clone(&O::core(realm).children);
    for (child, url) in children.take_module_requests() {
        if url.starts_with(NATIVE_MODULE_ESM_PREFIX) {
            let refused = Err(format!(
                "module '{url}': a native module's ES module loads only in a worker realm, \
                 not in a ShadowRealm"
            ));
            spawn(
                owner,
                complete_child_module(Rc::clone(owner), child, url, refused),
            );
            continue;
        }
        let answer = owner.host().request(SourceRequest::Module(url.clone()));
        spawn(
            owner,
            load_child_module(Rc::clone(owner), child, url, answer),
        );
    }
    for (id, future) in O::core(realm).futures.take_settle_requests() {
        spawn(owner, settle_future(Rc::clone(owner), id, future));
    }
    for (child, id, future) in children.take_settle_requests() {
        spawn(
            owner,
            settle_child_future(Rc::clone(owner), child, id, future),
        );
    }
    report_shadow_realm_rejections(owner, realm, js, &children);
    if lifetime.ended() {
        return;
    }
    O::after_settles(owner, realm);
    children.release_unneeded();
    let deadline = match (O::next_deadline(realm), children.next_deadline()) {
        (Some(own), Some(child)) => Some(own.min(child)),
        (own, child) => own.or(child),
    };
    lifetime.arm_deadline(deadline);
    lifetime.record_checkpoint(js.checkpoint_generation());
}

fn report_shadow_realm_rejections<O: RealmOwner>(
    owner: &Rc<O>,
    realm: &mut O::Realm,
    js: &mut ScriptRuntime,
    children: &ChildRealms,
) {
    if children.is_empty() {
        return;
    }
    for error in children.take_unhandled_rejections(js) {
        policy::report(owner, Scene::ShadowRealm, error);
    }
    match O::core(realm).engine.take_unhandled_rejection(js) {
        Ok(executed) if executed > 0 => js.mark_checkpoint(),
        Ok(_) => {}
        Err(error) => policy::report(owner, Scene::ShadowRealm, error),
    }
}

async fn load_module<O: RealmOwner>(owner: Rc<O>, url: String, answer: SourceAnswer) {
    let loaded = await_source(answer)
        .await
        .map_err(|error| error.to_string())
        .and_then(|answer| module_answer(&url, answer))
        .map_err(|reason| format!("module '{url}': {reason}"));
    let completing = Rc::clone(&owner);
    enter(&owner, JobKind::Module, move |realm, js| {
        let loaded = loaded
            .as_ref()
            .map(|(response, source)| (response.as_str(), source.as_str()))
            .map_err(String::as_str);
        if let Err(error) = O::core(realm).engine.complete_module(js, &url, loaded) {
            policy::report(&completing, Scene::Module, error);
        }
    })
    .await;
}

async fn load_native_esm<O: RealmOwner>(owner: Rc<O>, url: String, bytecode: Option<Bytes>) {
    let completing = Rc::clone(&owner);
    enter(&owner, JobKind::Module, move |realm, js| {
        let engine = &O::core(realm).engine;
        let completed = if let Some(bytecode) = &bytecode {
            engine.complete_module_bytecode(js, &url, bytecode)
        } else {
            let name = &url[NATIVE_MODULE_ESM_PREFIX.len()..];
            engine.complete_module(
                js,
                &url,
                Err(&format!(
                    "module '{url}': this view has no native module `{name}` that supplies an \
                     ES module"
                )),
            )
        };
        if let Err(error) = completed {
            policy::report(&completing, Scene::Module, error);
        }
    })
    .await;
}

async fn load_child_module<O: RealmOwner>(
    owner: Rc<O>,
    child: ChildKey,
    url: String,
    answer: SourceAnswer,
) {
    let loaded = await_source(answer)
        .await
        .map_err(|error| error.to_string())
        .and_then(|answer| module_answer(&url, answer))
        .map_err(|reason| format!("module '{url}': {reason}"));
    complete_child_module(owner, child, url, loaded).await;
}

async fn complete_child_module<O: RealmOwner>(
    owner: Rc<O>,
    child: ChildKey,
    url: String,
    loaded: Result<(String, String), String>,
) {
    let completing = Rc::clone(&owner);
    enter(&owner, JobKind::Module, move |realm, js| {
        let loaded = loaded
            .as_ref()
            .map(|(response, source)| (response.as_str(), source.as_str()))
            .map_err(String::as_str);
        let children = Rc::clone(&O::core(realm).children);
        if let Some(Err(error)) = children.complete_module(js, child, &url, loaded) {
            policy::report(&completing, Scene::Module, error);
        }
    })
    .await;
}

async fn settle_child_future<O: RealmOwner>(
    owner: Rc<O>,
    child: ChildKey,
    id: u32,
    future: HostFuture,
) {
    let outcome = future.await;
    let settling = Rc::clone(&owner);
    enter(&owner, JobKind::Future, move |realm, js| {
        let children = Rc::clone(&O::core(realm).children);
        if let Some(Err(error)) = children.deliver_future(js, child, id, outcome) {
            policy::report(&settling, Scene::Future, error);
        }
    })
    .await;
}

async fn settle_future<O: RealmOwner>(owner: Rc<O>, id: u32, future: HostFuture) {
    let outcome = future.await;
    let settling = Rc::clone(&owner);
    enter(&owner, JobKind::Future, move |realm, js| {
        let delivered = crate::future::deliver(&O::core(realm).engine, js, id, outcome);
        if let Err(error) = delivered {
            policy::report(&settling, Scene::Future, error);
        }
    })
    .await;
}

pub(crate) async fn await_source(answer: SourceAnswer) -> Result<LoadedSource, LynxViewError> {
    answer
        .await
        .unwrap_or_else(|_| Err(unanswered_source().into()))
}

pub(crate) fn module_answer(
    requested: &str,
    answer: LoadedSource,
) -> Result<(String, String), String> {
    let kind = match answer {
        LoadedSource::Module { source, url } => return Ok((url, source)),
        LoadedSource::StyleSheet(_) => "stylesheet",
        LoadedSource::Font(_) => "font",
        LoadedSource::Fetched(_) => "plain fetch",
    };
    Err(format!("the fetcher returned a {kind} for {requested}"))
}

pub(crate) fn after_end<O, T, F>(
    owner: &Rc<O>,
    operation: F,
) -> impl Future<Output = Option<T>> + use<O, T, F>
where
    O: RealmOwner,
    T: 'static,
    F: FnOnce(&mut Option<Box<O::Realm>>, &mut ScriptRuntime) -> T + 'static,
{
    run_job(owner, JobKind::Release, move |owner| {
        let mut js = owner.runtime().borrow_mut();
        let Ok(js) = js.as_mut() else {
            return None;
        };
        Some(operation(&mut owner.realm().borrow_mut(), js))
    })
}

pub(crate) async fn run_owner<O: RealmOwner>(owner: &Rc<O>) {
    owner
        .lifetime()
        .serve(&mut |payload| trapped(owner, payload.as_ref()))
        .await;
    end(owner);
    owner
        .lifetime()
        .reap(&mut |payload| trapped(owner, payload.as_ref()))
        .await;
    O::before_release(owner).await;
    after_end(owner, |realm, _| *realm = None).await;
}

impl<O: RealmOwner> Settles for O {
    fn lifetime(&self) -> &Lifetime {
        RealmOwner::lifetime(self)
    }

    const THREAD: JsThreadName = <Self as RealmOwner>::THREAD;

    fn outbox(&self) -> &HostOutbox {
        RealmOwner::host(self)
    }

    fn settle(owner: &Rc<Self>, kind: JobKind) -> impl Future<Output = Option<()>> {
        enter(owner, kind, |_, _| ())
    }

    fn end(owner: &Rc<Self>) {
        end(owner);
    }

    fn trapped(owner: &Rc<Self>, payload: &(dyn Any + Send)) {
        trapped(owner, payload);
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use super::*;
    use crate::resource::{FetchOutcome, ResourceErrorKind, StyleSheetSource};
    use crate::test_support::block_on;

    const REQUESTED: &str = "app:///dep.js";

    #[test]
    fn a_script_answer_is_its_response_url_and_its_source() {
        let answered = module_answer(
            REQUESTED,
            LoadedSource::Module {
                source: "export {};".to_owned(),
                url: "app:///redirected.js".to_owned(),
            },
        );
        let Ok((url, source)) = answered else {
            panic!("a script answer is read as one: {answered:?}");
        };
        assert_eq!(url, "app:///redirected.js");
        assert_eq!(source, "export {};");
    }

    #[test]
    fn an_answer_of_another_kind_is_refused_naming_the_request_and_the_kind() {
        for (answer, kind) in [
            (
                LoadedSource::StyleSheet(StyleSheetSource::Text(String::new())),
                "stylesheet",
            ),
            (
                LoadedSource::Font(terminus::link::FontBlob::from_static(b"not a font")),
                "font",
            ),
            (
                LoadedSource::Fetched(FetchOutcome::default()),
                "plain fetch",
            ),
        ] {
            let answered = module_answer(REQUESTED, answer);
            assert_eq!(
                answered,
                Err(format!("the fetcher returned a {kind} for {REQUESTED}"))
            );
        }
    }

    #[test]
    fn an_answer_that_never_comes_is_an_unanswered_source() {
        let (completion, answer) = oneshot::channel();
        drop(completion);
        let answered = block_on(await_source(answer));
        let Err(LynxViewError::Resource(error)) = answered else {
            panic!("a dropped completion is a failed load: {answered:?}");
        };
        assert!(matches!(error.kind, ResourceErrorKind::Unavailable));
        assert_eq!(error.message, unanswered_source().message);
    }
}
