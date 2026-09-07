//! Where the fetcher's own work runs, and how it gets back.
//!
//! The protocol initiates loads on the painter's thread and promises no
//! ambient runtime. Natively IO and decoding run on a pool of plain threads:
//! source jobs have a concrete queue variant, while image and byte jobs are
//! closures. Browser IO runs as local futures on the Render Worker's event loop.
//!
//! Images wake the host to service reports; sources complete directly into
//! main's FIFO, whose lifecycle notifications subsequently wake the host.

use std::sync::Arc;

/// The wakeup the embedder handed the view, shared with the workers so a
/// completion made between turns is answered by one.
pub type Wakeup = Arc<dyn Fn() + Send + Sync>;

#[cfg(not(target_arch = "wasm32"))]
enum Job {
    Task(Box<dyn FnOnce() + Send + 'static>),
    // Keep image queue entries small; Box<SourceJob> is a concrete thin pointer.
    Source(Box<crate::sources::SourceJob>),
}

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
                        let _ =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
                                Job::Task(task) => task(),
                                Job::Source(source) => source.run(),
                            }));
                    }
                });
            if let Err(error) = spawned {
                eprintln!("bobcat-resources: could not start worker {index}: {error}");
            }
        }
        Self { jobs }
    }

    /// Source loading has a concrete queue variant and no callback vtable.
    pub(crate) fn source(&self, job: crate::sources::SourceJob) {
        let _ = self.jobs.send(Job::Source(Box::new(job)));
    }

    /// Queues `job` for a worker. A job queued after every worker has gone
    /// is dropped, which can only happen during teardown.
    pub(crate) fn run(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.jobs.send(Job::Task(Box::new(job)));
    }
}
