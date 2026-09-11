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
use tokio::task;
use tokio::task::LocalSet;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::background::{WorkerCommand, WorkerMessage, WorkerStart};
use crate::link::{DetachedView, ViewNotice, detached_outbox};
use crate::main::WorkerFactory;
use crate::main::runtime::install_shared_modules;
use crate::main::tree::PageConfig;
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

fn ingredients() -> DocumentIngredients {
    DocumentIngredients::for_test(Viewport::new(320.0, 240.0), PageConfig::default())
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
        Self::serving(context, workers, ViewSources::new("app:///main.js"))
    }

    fn serving(
        context: Rc<GroupContext>,
        workers: mpsc::UnboundedReceiver<WorkerCommand>,
        sources: ViewSources,
    ) -> Self {
        let (outbox, view) = detached_outbox(Arc::new(NoWakeup));
        let (commands, incoming) = mpsc::unbounded_channel();
        let attached = AttachedView {
            viewport: Viewport::new(320.0, 240.0),
            sources,
            commands: incoming,
            cancel: view.token.clone(),
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

/// One page over the token that ends it, with the test holding the owner's
/// tail rather than a task running it.
///
/// The pins below that use this are about the owner's own steps — the end it
/// runs after its wait — and nothing is queued for another task in either of
/// them, so the cancel is the only wake there is and running that tail inline
/// is running it where it would have run anyway. A pin about which task the
/// scheduler picks belongs on [`Harness`] instead.
struct OwnedPage {
    page: Rc<Page>,
    view: DetachedView,
    /// The view's end signal, which here the test is the embedder of.
    token: CancellationToken,
    /// Held, not sent on: the command consumer this page spawned is parked on
    /// the other end, and dropping this would close the channel and end the
    /// view by a path neither pin here is about.
    _commands: mpsc::UnboundedSender<ToMain>,
}

impl OwnedPage {
    fn new(context: Rc<GroupContext>) -> Self {
        let (outbox, view) = detached_outbox(Arc::new(NoWakeup));
        let token = view.token.clone();
        let (commands, incoming) = mpsc::unbounded_channel();
        let page = Page::new(context, outbox, ingredients(), token.clone());
        page.spawn(consume_commands(Rc::clone(&page), incoming));
        Self {
            page,
            view,
            token,
            _commands: commands,
        }
    }

    /// Opens the realm over `entry` and turns until its first frame is
    /// published.
    async fn boot(&mut self, entry: &str) -> u64 {
        self.page
            .open_realm(entry, "app:///main.js", None, None, None);
        for _ in 0..TURNS {
            if self.view.published.commit().is_some() {
                break;
            }
            task::yield_now().await;
        }
        self.view.published.commit().expect("the page booted")
    }

    /// Every lifecycle event this page has published so far.
    fn events(&mut self) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        while let Ok(notice) = self.view.notices.try_recv() {
            if let ViewNotice::Engine(event) = notice {
                events.push(event);
            }
        }
        events
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

/// The same page, with a timer far enough out that nothing will ever fire it,
/// so the deadline a live realm publishes is there to be withdrawn.
const ONE_BOX_WITH_TIMER: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __AppendElement(page, box);
  setTimeout(() => {}, 3600000);
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

/// The host's page data rides from the view's sources to its realm as the
/// text it was given, and is parsed there before the entry loads: the entry
/// sees the global props as it evaluates, and `processData` gets the init
/// data. Each side checks its own, so a swap anywhere on the way fails boot.
#[test]
fn page_data_reaches_the_realm_it_was_given_to() {
    on_a_local_set(async {
        let (context, workers) = group();
        let mut harness = Harness::serving(
            context,
            workers,
            ViewSources {
                init_data: Some(r#"{"boxes": 2}"#.to_owned()),
                global_props: Some(r#"{"theme": "dark"}"#.to_owned()),
                ..ViewSources::new("app:///main.js")
            },
        );
        harness
            .boot(
                r"
                if (__globalProps.theme !== 'dark') throw new Error('global props');
                globalThis.processData = function (data) {
                  if (data.boxes !== 2) throw new Error('init data');
                  return data;
                };
                ",
            )
            .await;
    });
}

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
        let page = Page::new(
            Rc::clone(&context),
            outbox,
            ingredients(),
            view.token.clone(),
        );
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
            2,
            "a live realm waits on its workers, and on its clock — its deadline and the \
             runtime's checkpoints are one task"
        );

        // Parked: nothing of this page's own is running, and its own entries
        // are not what the clock task's checkpoint arm is watching for.
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

/// A page's own entries do not wake its clock task: the generation it records
/// at the end of every entry is what tells its own bumps from a sibling's.
#[test]
fn a_pages_own_entries_never_wake_its_clock_task() {
    on_a_local_set(async {
        let (context, _workers) = group();
        let (outbox, mut view) = detached_outbox(Arc::new(NoWakeup));
        let page = Page::new(
            Rc::clone(&context),
            outbox,
            ingredients(),
            view.token.clone(),
        );
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

/// A panic in one task of a view ends the view during the unwind, before any
/// other task of it runs: the guard the page wraps every task in is what does
/// it, and the owner — which is woken by the same cancellation and therefore
/// polled behind the sibling — is what reports the payload.
///
/// The seam spawns the panicking task first and the recorder behind it, so
/// what the recorder saw is what a sibling polled between the unwind and the
/// owner's turn would have seen.
#[test]
fn a_task_that_traps_ends_the_view_before_a_sibling_runs() {
    on_a_local_set(async {
        let (context, workers) = group();
        let mut harness = Harness::new(context, workers);
        harness.boot(ONE_BOX).await;

        let (latch, seen) = std::sync::mpsc::channel();
        harness
            .commands
            .send(ToMain::Trap(latch))
            .expect("the view is still serving");
        harness
            .until("the view's owner never returned", |harness| {
                harness.commands.is_closed()
            })
            .await;
        harness.turn().await;

        assert_eq!(
            seen.try_recv(),
            Ok(true),
            "the sibling polled during the unwind found a view that had already ended"
        );
        let reports: Vec<_> = harness
            .events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::ScriptRunError(error) => Some(error.message.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(reports.len(), 1, "one panic is reported once: {reports:?}");
        assert!(
            reports[0].contains("the Lynx main thread panicked")
                && reports[0].contains("a task of the view trapped"),
            "the report carries the payload: {}",
            reports[0]
        );
    });
}

/// The end acknowledges whatever `BeginFrame` was applied and not yet
/// answered, and withdraws the deadline the realm had armed — both of them on
/// the owner's own turn, because a release cancels the token from another
/// thread and nothing on this one has run since.
///
/// A painter blocked on that sequence number is waiting for a frame that will
/// never come, so releasing it is the last thing the view owes it.
#[test]
fn the_end_acknowledges_the_begin_frame_a_painter_is_blocked_on() {
    on_a_local_set(async {
        let (context, _workers) = group();
        let mut owned = OwnedPage::new(context);
        owned.boot(ONE_BOX_WITH_TIMER).await;
        assert!(
            owned.page.armed_deadline().is_some(),
            "the booted realm armed a timer"
        );
        owned.page.arm_begin_frame_for_test(11);

        // The embedder's release, with nothing else touched: the command
        // channel stays open, so the owner's wait is the token alone.
        owned.token.cancel();
        owned.page.run_owner().await;

        assert_eq!(
            owned.view.published.begin_frame_serviced(),
            11,
            "the end acknowledged the pending BeginFrame"
        );
        assert!(
            owned.page.armed_deadline().is_none(),
            "and withdrew the deadline the realm had armed"
        );
    });
}

/// A burst queued behind the embedder's release is discarded rather than
/// applied: the consumer reads the token once per burst, at the wake
/// boundary, and a token already cancelled there ends the view instead of
/// serving what is queued.
///
/// The order this pins is the order production has, which is why it runs over
/// the real [`serve_view`] rather than the owner's tail alone: the command is
/// sent first, so it wakes the consumer, and the cancel that follows wakes the
/// owner *behind* it. The consumer is what runs first, so nothing here can
/// rest on the owner being woken first.
///
/// The channel is left open, which is also the shape a fatal event's cancel in
/// `LynxView::pump` leaves.
#[test]
fn a_burst_queued_behind_a_release_is_never_applied() {
    on_a_local_set(async {
        let (context, workers) = group();
        let mut harness = Harness::new(context, workers);
        let booted = harness.boot(ONE_BOX).await;

        // Genuinely dirtying: the same command applied on its own is what the
        // burst pin above counts a commit for.
        harness
            .commands
            .send(ToMain::Resize {
                width: 200.0,
                height: 100.0,
                device_pixel_ratio: 1.0,
            })
            .expect("the view is still serving");
        harness.view.token.cancel();
        harness
            .until("the view never ended", |harness| {
                harness.owner.is_finished()
            })
            .await;

        assert_eq!(
            harness.view.published.commit(),
            Some(booted),
            "the queued command never reached the realm"
        );
    });
}

/// A panic is reported whatever the view was told before it. The report-once
/// latch a startup failure spends is not the one a panic goes through: the
/// lifetime holds a latch of its own for the payload-bearing report, so a view
/// that failed and then trapped says both.
#[test]
fn a_view_that_already_failed_still_reports_a_task_that_traps() {
    on_a_local_set(async {
        let (context, _workers) = group();
        let mut owned = OwnedPage::new(context);
        owned
            .page
            .fail(EngineEvent::StartupFailed(unanswered_source().into()));
        owned
            .page
            .spawn(async { panic!("a task of the view trapped") });
        // Let the panicking task run: the owner aborts what has not been
        // polled, so a task that never ran is a task that never trapped.
        for _ in 0..8 {
            task::yield_now().await;
        }

        owned.page.run_owner().await;

        let events = owned.events();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, EngineEvent::StartupFailed(_)))
                .count(),
            1,
            "the startup failure is reported once"
        );
        let reports: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::ScriptRunError(error) => Some(error.message.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(
            reports.len(),
            1,
            "and the panic behind it is reported once too: {reports:?}"
        );
        assert!(
            reports[0].contains("a task of the view trapped"),
            "carrying the payload: {}",
            reports[0]
        );
    });
}
