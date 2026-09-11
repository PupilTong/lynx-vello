//! Where the fetcher's own work runs, and how it gets back.
//!
//! The protocol initiates loads on the painter's thread and promises no
//! ambient runtime, so natively this crate brings one of its own: a
//! `current_thread` tokio runtime that [`Executor::new`] builds on the thread
//! constructing the [`Resources`](crate::Resources) and hands to a thread
//! named `bobcat-resources-driver`, which drives it and shuts it down. Its
//! blocking pool runs every transport read, preprocessing pass and platform
//! decode. Browser IO has no runtime here at all — it runs as local futures
//! on the Render Worker's event loop.
//!
//! Images wake the host to service reports; sources complete directly into
//! main's FIFO, whose lifecycle notifications subsequently wake the host.

use std::sync::Arc;

/// The wakeup the embedder handed the view, shared with the jobs so a
/// completion made between turns is answered by one.
pub type Wakeup = Arc<dyn Fn() + Send + Sync>;

/// The native job runner: one tokio runtime, owned by a thread of its own.
///
/// [`Resources`](crate::Resources) is what holds it, through an `Arc` that
/// every clone and every scope shares. [`Shared`](crate::Shared) — the part
/// of the system a job holds — holds no piece of the executor, so no job can
/// keep the runtime alive and the last drop always happens on the painter's
/// thread.
///
/// # What runs where
///
/// The painter's thread submits the three jobs this executor starts — a
/// source load, an image load, an image refinement — and submitting one of
/// those only enqueues a task ([`Self::spawn`]). Each job is one async task,
/// so the driver thread runs the job's awaits and sends its completion, while
/// every blocking step — the transport, preprocessing and the platform
/// decoder — runs on the runtime's blocking pool. That pool is capped at
/// `worker_threads` threads and its threads are created lazily, by the
/// `spawn_blocking` call that finds none idle, on the thread making that
/// call — the driver's, for those three jobs.
///
/// Not every thread of this executor is created on the driver: [`Self::new`]
/// starts the driver thread itself on whichever thread builds the
/// `Resources`.
///
/// The one piece of the fetcher's work that does not come here is the
/// painter's own synchronous restore in `images::read`, which fetches and
/// decodes inline on the painter's thread because `FrameImages::read` has to
/// answer inside the call.
///
/// # The decode permit
///
/// A job acquires a [`Semaphore`](tokio::sync::Semaphore) permit *before* it submits its decode
/// closure and holds it until that closure returns, so a decode waiting for
/// a permit occupies no pool thread and no blocking closure ever waits on a
/// permit. `max_blocking_threads` is `worker_threads.max(1)` and caps IO,
/// preprocessing and decoding together;
/// `decode_parallelism.unwrap_or(worker_threads).max(1)` permits cap how many
/// decode closures are submitted at once. At the default they never lower the
/// throughput a pool of that size could reach, though a decode can still wait
/// for a permit behind another job's queued transport read; they bind harder
/// only when a host sets a lower number, and then a waiting decode holds no
/// thread. The painter's restore decode takes no permit by construction.
///
/// # Panics
///
/// A panic inside a blocking closure is delivered to the job's task as a
/// [`JoinError`](tokio::task::JoinError) and becomes that job's reported
/// failure: `Completion::Failed` for an image load, `Completion::RefineFailed`
/// for a refinement, the protocol's `Failure` for a source. The pool's thread
/// survives it, and so does the next job.
///
/// A task body holds only awaits and the completion, so the only panic it can
/// raise itself comes from the embedder's `wakeup`, which `Shared::complete`
/// calls *after* the completion is queued: the report is there, waiting for a
/// turn nothing asked for.
///
/// What is never reported is an image or source whose task never ran to its
/// completion. An OS that refuses a new pool thread is how that happens:
/// `spawn_blocking` panics on the thread that called it, unless the refusal
/// is transient and a pool thread is already running to pick the closure up.
/// For the three jobs above the panic is the task's, on the driver, and the
/// job goes unreported. The hand-written pool this replaced asked for its
/// threads once, at construction, printed a refusal to standard error and
/// carried on with the threads it had; a fetch it could queue nowhere
/// answered with an `Unavailable` failure.
///
/// # Shutdown
///
/// Dropping the last [`Resources`](crate::Resources) cancels the driver and
/// joins it, so the drop blocks the dropping thread until the driver leaves
/// the poll it is in. What the join does not wait for is the work: a task is
/// dropped at its next suspension point (a dropped `SourceCompletion` reports
/// the protocol's unanswered-source failure unless its view is already
/// cancelled), a blocking closure a thread has already picked up runs to
/// completion and its result is discarded, and a queued closure may still run
/// or may be dropped, whichever tokio's pool reaches first.
///
/// Only the driver's tasks call the embedder's `wakeup`, so once the join
/// returns no job of this executor can wake the embedder again — and a wakeup
/// already running is one of the things that join waits for. A `wakeup` must
/// therefore not block, and must not take a lock the thread dropping the
/// `Resources` can be holding.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Executor {
    /// A handle owns nothing: the runtime itself is the driver thread's.
    handle: tokio::runtime::Handle,
    /// What ends the driver's `block_on`, and with it the runtime.
    stop: tokio_util::sync::CancellationToken,
    driver: Option<std::thread::JoinHandle<()>>,
    decode_permits: Arc<tokio::sync::Semaphore>,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for Executor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Executor")
            .field("decode_permits", &self.decode_permits.available_permits())
            .finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Executor {
    /// Builds the runtime and hands it to the driver thread that owns it.
    ///
    /// # Panics
    ///
    /// If the platform refuses the runtime or the driver thread. A
    /// `current_thread` runtime asks the platform for nothing, and a host
    /// that cannot start one more thread has nothing to fall back to, so
    /// this is the abort `bobcat-core` takes for its own runtimes rather
    /// than a failure an embedder could act on.
    pub(crate) fn new(worker_threads: usize, decode_parallelism: Option<usize>) -> Self {
        let threads = worker_threads.max(1);
        let permits = decode_parallelism.unwrap_or(threads).max(1);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .thread_name("bobcat-resources")
            .max_blocking_threads(threads)
            .build()
            .expect("the bobcat-resources runtime must initialize");
        let handle = runtime.handle().clone();
        let stop = tokio_util::sync::CancellationToken::new();
        let driver = std::thread::Builder::new()
            .name("bobcat-resources-driver".to_owned())
            .spawn({
                let stop = stop.clone();
                move || {
                    runtime.block_on(stop.cancelled());
                    // Driven and shut down on the one thread it was moved to,
                    // so it is never dropped from inside itself.
                    runtime.shutdown_background();
                }
            })
            .expect("the bobcat-resources driver thread must start");
        Self {
            handle,
            stop,
            driver: Some(driver),
            decode_permits: Arc::new(tokio::sync::Semaphore::new(permits)),
        }
    }

    /// Runs `job` as one task on the driver thread. Fire and forget: the job
    /// reports through its own completion, not through a handle.
    pub(crate) fn spawn<F>(&self, job: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        drop(self.handle.spawn(job));
    }

    /// What a task needs to submit more work: a handle owns no runtime, so a
    /// job holding one cannot keep this executor alive.
    pub(crate) fn handle(&self) -> tokio::runtime::Handle {
        self.handle.clone()
    }

    /// The permits that gate decode submissions.
    pub(crate) fn decode_permits(&self) -> Arc<tokio::sync::Semaphore> {
        Arc::clone(&self.decode_permits)
    }
}

/// Runs `job` on `handle`'s blocking pool and hands back what it returned, or
/// the message its panic left. `what` names the step in that message.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn blocking<T: Send + 'static>(
    handle: &tokio::runtime::Handle,
    what: &'static str,
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    match handle.spawn_blocking(job).await {
        Ok(value) => Ok(value),
        Err(error) if error.is_panic() => Err(format!(
            "the {what} panicked: {}",
            panic_message(&*error.into_panic())
        )),
        Err(_) => Err(format!(
            "the {what} was dropped when the resource system shut down"
        )),
    }
}

/// What a panic said, out of the payload the task harness kept.
#[cfg(not(target_arch = "wasm32"))]
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&'static str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Executor {
    /// Ends the driver and waits for it, on the painter's thread. Joining a
    /// thread is permitted inside another runtime's `block_on` — the capture
    /// server builds and drops its `Resources` inside one — where dropping a
    /// runtime would panic, and the painter's thread is never the driver.
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(driver) = self.driver.take() {
            let _ = driver.join();
        }
    }
}
