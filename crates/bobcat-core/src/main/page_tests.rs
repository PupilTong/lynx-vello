//! What a view's tasks owe each other, driven with the test standing in for
//! both ends of the link.
//!
//! Real `QuickJS`, a real `LocalSet` and the real [`serve_view`], with no
//! group thread, no painter and no GPU: the test answers every source request
//! by hand and reads what the view published. That is the whole seam these
//! pins need, because what they are about is which task ran, in what order,
//! and how many entries into the realm it took.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::LocalSet;

use super::*;
use crate::background::{WorkerCommand, WorkerMessage, WorkerStart};
use crate::link::{DetachedView, ViewNotice, detached_outbox};
use crate::main::WorkerFactory;
use crate::main::runtime::install_shared_modules;
use crate::main::tree::{PageConfig, Viewport, new_document};
use crate::resource::{SourceCompletion, SourceRequest};
use crate::view::NoWakeup;

/// How many times the harness lets every ready task run before it gives up on
/// something happening. A hang detector rather than a schedule: everything
/// here is on one thread and cooperative, so a step that has not happened in
/// this many turns is a step that never will.
const TURNS: usize = 512;

/// Runs one test body on a `LocalSet` over a current-thread runtime, which is
/// the shape `bobcat-main` itself runs.
fn on_a_local_set<F: std::future::Future<Output = ()>>(body: F) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime asks the platform for nothing");
    LocalSet::new().block_on(&runtime, body);
}

/// One group's shared runtime, with the test holding the worker thread's end
/// of the factory so a `Start` is observable and no worker ever boots.
fn group() -> (Rc<GroupContext>, mpsc::UnboundedReceiver<WorkerCommand>) {
    let mut js = ScriptRuntime::new().expect("a QuickJS runtime");
    install_shared_modules(&mut js).expect("the shared modules register");
    let (workers, commands) = mpsc::unbounded_channel();
    let context = GroupContext {
        js: Rc::new(RefCell::new(js)),
        style_pool: None,
        requester: Arc::new(NoWakeup),
        workers: WorkerFactory::new(workers),
    };
    (Rc::new(context), commands)
}

/// The document a page is built over, at the viewport the harness uses.
fn document() -> LynxDocument {
    new_document(Viewport::new(320.0, 240.0), PageConfig::default())
}

/// One view served by the real owner, with the test on the host's end of its
/// link.
struct Harness {
    workers: mpsc::UnboundedReceiver<WorkerCommand>,
    commands: mpsc::UnboundedSender<ToMain>,
    view: DetachedView,
    events: Vec<EngineEvent>,
    sources: Vec<(SourceRequest, SourceCompletion)>,
    /// The owner's handle, so a step that never happened because the owner
    /// trapped is reported as that panic rather than as a deadline.
    owner: task::JoinHandle<()>,
}

impl Harness {
    fn new(context: Rc<GroupContext>, workers: mpsc::UnboundedReceiver<WorkerCommand>) -> Self {
        let (outbox, view) = detached_outbox(Arc::new(NoWakeup));
        let (commands, incoming) = mpsc::unbounded_channel();
        let attached = AttachedView {
            viewport: Viewport::new(320.0, 240.0),
            sources: MainSources {
                config: PageConfig::default(),
                fonts: Vec::new(),
                default_font_family: None,
                style_sheets: Vec::new(),
                entry: "app:///main.js".to_owned(),
                background_entry: None,
                init_data: None,
                global_props: None,
            },
            commands: incoming,
            cancel: view.cancel.clone(),
        };
        let owner = task::spawn_local(serve_view(context, attached, outbox));
        Self {
            workers,
            commands,
            view,
            events: Vec::new(),
            sources: Vec::new(),
            owner,
        }
    }

    /// Lets every ready task of the view run, collecting whatever it said.
    async fn turn(&mut self) {
        task::yield_now().await;
        while let Ok(notice) = self.view.notices.try_recv() {
            match notice {
                ViewNotice::Engine(event) => self.events.push(event),
                ViewNotice::RequestSource {
                    request,
                    completion,
                } => self.sources.push((request, completion)),
                ViewNotice::RequestImages(_) => {}
            }
        }
    }

    /// Turns until `ready` answers, or fails naming what never happened.
    async fn until(&mut self, what: &str, mut ready: impl FnMut(&mut Self) -> bool) {
        for _ in 0..TURNS {
            if ready(self) {
                return;
            }
            self.turn().await;
        }
        // An owner that trapped is why nothing happened, and its payload says
        // more than the deadline this loop ran out of. Awaiting a handle that
        // has already finished answers at once.
        if self.owner.is_finished() {
            match (&mut self.owner).await {
                Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                _ => {}
            }
        }
        panic!("{what}");
    }

    /// Answers one outstanding source request, whichever it is.
    fn answer(&mut self, url: &str, source: &str) {
        let (_, completion) = self.sources.pop().expect("a source request is outstanding");
        completion.complete(Ok(LoadedSource::Entry {
            source: source.to_owned(),
            url: url.to_owned(),
        }));
    }

    /// Answers one outstanding source request with a failure.
    fn refuse(&mut self) {
        let (_, completion) = self.sources.pop().expect("a source request is outstanding");
        completion.complete(Err(unanswered_source().into()));
    }

    /// Boots the view over `entry` and returns the commit its boot published.
    async fn boot(&mut self, entry: &str) -> u64 {
        self.until("the view never asked for its entry", |harness| {
            !harness.sources.is_empty()
        })
        .await;
        self.answer("app:///main.js", entry);
        self.until("the entry never finished", |harness| {
            harness
                .events
                .iter()
                .any(|event| matches!(event, EngineEvent::ScriptFinished))
        })
        .await;
        self.view
            .published
            .commit()
            .expect("boot published a frame")
    }

    /// The `Start` the boot module's BTS `Worker` sent, which nothing on this
    /// test's side ever boots.
    fn background_worker(&mut self) -> WorkerStart {
        let Some(WorkerCommand::Start(start)) = self.workers.try_recv().ok() else {
            panic!("boot creates the BTS worker")
        };
        start
    }
}

/// A page that renders one box, which is enough for a resize to have
/// something to lay out again.
const ONE_BOX: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __AppendElement(page, box);
  globalThis.box = box;
};
";

/// The same page, with a listener on its box so a dispatch into it genuinely
/// enters JavaScript.
const LISTENING_BOX: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __AppendElement(page, box);
  __AddEventListener(box, 'tap', () => {});
};
";

/// A host's whole round of input is one entry into the realm, so it is one
/// commit and one acknowledgement — not one of each per command.
#[test]
fn a_burst_of_commands_is_one_commit_and_one_acknowledgement() {
    on_a_local_set(async {
        let (context, workers) = group();
        let mut harness = Harness::new(context, workers);
        let booted = harness.boot(ONE_BOX).await;

        // Every one of these is queued before the consumer wakes, and every
        // one of them genuinely changes the viewport, so an entry apiece
        // would be a commit apiece.
        for step in 0..5u8 {
            harness
                .commands
                .send(ToMain::Resize {
                    width: 320.0 - f32::from(step),
                    height: 240.0 + f32::from(step),
                    device_pixel_ratio: 1.0,
                })
                .expect("the view is still serving");
        }
        harness
            .commands
            .send(ToMain::BeginFrame { now: 0.0, seq: 7 })
            .expect("the view is still serving");

        harness
            .until("the burst was never acknowledged", |harness| {
                harness.view.published.begin_frame_serviced() == 7
            })
            .await;
        assert_eq!(
            harness.view.published.commit(),
            Some(booted + 1),
            "the whole burst committed once"
        );
    });
}

/// A module completion is a task of its own, so what its continuation changed
/// is committed without a command to carry it.
#[test]
fn a_module_completion_commits_with_no_command_behind_it() {
    on_a_local_set(async {
        let (context, workers) = group();
        let mut harness = Harness::new(context, workers);
        let booted = harness
            .boot(&format!(
                "{ONE_BOX}\nimport('app:///dep.js').then(() => __SetAttribute(globalThis.box, 'loaded', 'yes'));"
            ))
            .await;
        harness
            .until("the entry's import was never requested", |harness| {
                harness
                    .sources
                    .iter()
                    .any(|(request, _)| matches!(request, SourceRequest::Module(url) if url == "app:///dep.js"))
            })
            .await;
        assert_eq!(
            harness.view.published.commit(),
            Some(booted),
            "nothing has committed since boot"
        );

        harness.answer("app:///dep.js", "export const value = 1;");
        harness
            .until("the import's continuation never committed", |harness| {
                harness.view.published.commit() == Some(booted + 1)
            })
            .await;
        assert_eq!(
            harness
                .events
                .iter()
                .filter(|event| matches!(event, EngineEvent::ScriptFinished))
                .count(),
            1,
            "and it is not a second boot"
        );
    });
}

/// An import that cannot be completed is fatal to whatever awaited it: it is
/// reported once, and the end reaches every task of the view — the owner
/// returns, which closes the command channel, and the workers the view
/// created are told to stop. A command that arrives behind the end is dropped
/// whole, `BeginFrame` included.
#[test]
fn a_fatal_module_failure_ends_every_task_of_the_view() {
    on_a_local_set(async {
        let (context, workers) = group();
        let mut harness = Harness::new(context, workers);
        let booted = harness
            .boot(&format!("{ONE_BOX}\nimport('app:///dep.js');"))
            .await;
        let mut background = harness.background_worker();
        harness
            .until("the entry's import was never requested", |harness| {
                !harness.sources.is_empty()
            })
            .await;

        harness.refuse();
        harness
            .until("the failed import was never reported", |harness| {
                harness
                    .events
                    .iter()
                    .any(|event| matches!(event, EngineEvent::ScriptRunError(_)))
            })
            .await;
        // Exactly one command behind the end, which has nowhere to go: the end
        // acknowledged whatever was pending, and what arrives after it is
        // dropped rather than served. A send the closing has already overtaken
        // says the same thing.
        let _ = harness
            .commands
            .send(ToMain::BeginFrame { now: 0.0, seq: 9 });
        harness
            .until("the view's owner never returned", |harness| {
                harness.commands.is_closed()
            })
            .await;

        assert_ne!(
            harness.view.published.begin_frame_serviced(),
            9,
            "the command behind the end was never served"
        );
        assert_eq!(
            harness.view.published.commit(),
            Some(booted),
            "and nothing committed after the failure"
        );
        assert_eq!(
            harness
                .events
                .iter()
                .filter(|event| matches!(event, EngineEvent::ScriptRunError(_)))
                .count(),
            1,
            "one failure is reported once"
        );
        assert!(
            !harness
                .events
                .iter()
                .skip_while(|event| !matches!(event, EngineEvent::ScriptRunError(_)))
                .any(|event| matches!(event, EngineEvent::ScriptFinished)),
            "and nothing claims the entry finished after it"
        );
        assert!(
            matches!(background.messages.try_recv(), Ok(WorkerMessage::Terminate)),
            "the realm going takes the workers it created with it"
        );
    });
}

/// A sibling's task ending announces a checkpoint nobody ran: the job queue
/// it left is this page's too, so the page settles what its own realm owes
/// rather than staying parked.
///
/// Built over the page directly rather than over [`serve_view`], because what
/// it counts — that the epilogue ran at all — is not something a page with
/// nothing to publish says out loud.
#[test]
fn a_siblings_checkpoint_makes_a_parked_page_settle() {
    on_a_local_set(async {
        let (context, _workers) = group();
        let (outbox, mut view) = detached_outbox(Arc::new(NoWakeup));
        let (page, _ended) = Page::new(Rc::clone(&context), outbox, document());
        page.open_realm(ONE_BOX, "app:///main.js", None, None, None);
        for _ in 0..TURNS {
            if view.published.commit().is_some() {
                break;
            }
            task::yield_now().await;
        }
        assert!(view.published.commit().is_some(), "the page booted");
        assert_eq!(
            page.task_count(),
            3,
            "a live realm waits on its workers, its clock and the runtime's checkpoints"
        );

        // Parked: nothing of this page's own is running, and its own entries
        // are not what the follower is watching for.
        for _ in 0..8 {
            task::yield_now().await;
        }
        let settled = page.epilogue_count();

        // Exactly what `finish_view` does when a sibling's task ends.
        context.js.borrow().mark_checkpoint();
        for _ in 0..TURNS {
            if page.epilogue_count() > settled {
                break;
            }
            task::yield_now().await;
        }
        assert_eq!(
            page.epilogue_count(),
            settled + 1,
            "the bump nobody's entry produced is the one that wakes this page"
        );
    });
}

/// A page's own entries do not wake its checkpoint follower: the generation
/// it records at the end of every entry is what tells its own bumps from a
/// sibling's.
#[test]
fn a_pages_own_entries_never_wake_its_checkpoint_follower() {
    on_a_local_set(async {
        let (context, _workers) = group();
        let (outbox, mut view) = detached_outbox(Arc::new(NoWakeup));
        let (page, _ended) = Page::new(Rc::clone(&context), outbox, document());
        // The listener is what makes the dispatch below a real entry into
        // JavaScript rather than a walk that meets nobody.
        page.open_realm(LISTENING_BOX, "app:///main.js", None, None, None);
        for _ in 0..TURNS {
            if view.published.commit().is_some() {
                break;
            }
            task::yield_now().await;
        }
        // A boot that failed says why, which is a better report than the
        // assertion below and than every step after it.
        while let Ok(notice) = view.notices.try_recv() {
            if let ViewNotice::Engine(EngineEvent::StartupFailed(error)) = notice {
                panic!("the page never booted: {error}");
            }
        }
        assert!(view.published.commit().is_some(), "the page booted");

        // The probe is how the test learns which node to aim the dispatch at.
        let (target, listening) = std::sync::mpsc::channel();
        page.apply(std::iter::once(ToMain::Probe(Box::new(move |document| {
            let page = document.document_element().id();
            let box_id = document.get(page).expect("the page is live").child_ids()[0];
            let _ = target.send(box_id);
        }))));
        let target = listening.try_recv().expect("the probe ran");
        for _ in 0..64 {
            task::yield_now().await;
        }
        let settled = page.epilogue_count();

        // One entry of this page's own, which enters JavaScript and so bumps
        // the generation every realm on the runtime shares.
        page.apply(std::iter::once(ToMain::DispatchEvent {
            target,
            name: "tap",
            detail: "{}".to_owned(),
        }));
        for _ in 0..64 {
            task::yield_now().await;
        }
        assert_eq!(
            page.epilogue_count(),
            settled + 1,
            "the entry ran one epilogue, and its own checkpoint woke nothing"
        );
    });
}
