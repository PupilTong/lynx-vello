//! Where the fetcher's own work runs, and how it gets back.
//!
//! The protocol polls every fetch on the painter's thread and promises no
//! runtime there, so the IO and decoding this crate does need a home of
//! their own. Natively that is a small pool of plain threads: a job is a
//! closure, a completion is a value sent back over a channel, and the
//! painter drains that channel in its turn. In the browser the Render Worker
//! is single-threaded and `fetch` and the main thread's decoder are
//! asynchronous anyway, so a job is a local future and the same channel
//! carries its result.
//!
//! Either way a completion is followed by the host's wakeup, which is what
//! turns "finished" into "the painter takes a turn".

use std::sync::Arc;

/// The wakeup the embedder handed the view, shared with the workers so a
/// completion made between turns is answered by one.
pub type Wakeup = Arc<dyn Fn() + Send + Sync>;

#[cfg(not(target_arch = "wasm32"))]
type Job = Box<dyn FnOnce() + Send + 'static>;

/// The native worker pool.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub(crate) struct Executor {
    jobs: flume::Sender<Job>,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for Executor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Executor").finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Executor {
    /// Starts `threads` workers. They exit once every handle is dropped and
    /// the queue drains, so a fetcher that goes away takes its pool with it.
    pub(crate) fn new(threads: usize) -> Self {
        let (jobs, receiver) = flume::unbounded::<Job>();
        for index in 0..threads.max(1) {
            let receiver = receiver.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("bobcat-resources-{index}"))
                .spawn(move || {
                    for job in receiver {
                        // A job that panics must not take the worker with
                        // it: the next image still has to load.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    }
                });
            if let Err(error) = spawned {
                eprintln!("bobcat-resources: could not start worker {index}: {error}");
            }
        }
        Self { jobs }
    }

    /// Queues `job` for a worker. A job queued after every worker has gone
    /// is dropped, which can only happen during teardown.
    pub(crate) fn run(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.jobs.send(Box::new(job));
    }
}
