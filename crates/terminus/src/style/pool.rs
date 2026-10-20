//! The worker threads one thread's style traversals run on.
//!
//! A pool belongs to exactly one thread, and that is the whole point of it.
//! Stylo's bloom filter and style-sharing cache do not own their buffers: each
//! takes an `AtomicRefMut` on a leaked, per-OS-thread `AtomicRefCell`
//! (`StyleBloom::new` borrows `BLOOM_KEY`, `StyleSharingCache` borrows
//! `SHARING_CACHE_KEY`), so a second live instance on one thread panics on the
//! borrow. Both sit in a `ThreadLocalStyleContext`, and those contexts live in
//! the `ScopedTLS` that `driver::traverse_dom` owns for the whole call — so
//! every worker a traversal touched keeps its two buffers borrowed until that
//! traversal returns, not until it finishes a chunk. A rayon worker waiting on
//! its own scope runs any job it can find, including a chunk belonging to a
//! different traversal; on a shared pool that chunk builds a second
//! `ThreadLocalStyleContext` on a thread that already holds the borrows.
//!
//! Disjoint thread sets are what make that unrepresentable rather than
//! guarded. A pool never leaves the thread that built it — a document holds it
//! behind an `Rc`, and rayon's takeover below pins it there anyway — so the
//! documents sharing a pool are exactly the documents flushing on one thread,
//! and that thread cannot drive two traversals at once: it is inside the first
//! one. No two threads share a worker, which is what lets two views restyle at
//! the same time.
//!
//! The thread that flushes *is* a member, and index zero of its own pool.
//! [`StylePool::with_spawn_handler`] therefore has to be called on that
//! thread: rayon's `use_current_thread` takes it over in place, Stylo sees
//! `current_thread_index() == Some(0)` and runs the root closure inline, and
//! the workers spill out of it only when a level is wider than the traversal's
//! work unit. That is what Stylo's own global pool did, and what Gecko relies
//! on, so a lone document restyles on exactly the threads and with exactly the
//! parallelism it did before these pools became per-thread.
//!
//! It is not free. Rayon leaks the `WorkerThread` it hands the calling thread
//! and the `Registry` behind it — measured at about 25 KB for a six-thread
//! pool, once per pool ever built, with the managed workers still exiting
//! normally on drop — and a thread that has taken over one pool can never take
//! over another, for the whole of its life. Both are affordable only because a
//! pool is built once on a `bobcat-main` that is created fresh and dies with
//! the views it carries. A pool built twice on one thread is a bug this cannot
//! express: rayon refuses it, and [`StylePoolError::ThreadAlreadyPooled`] says
//! so — which is why the pool is the thread's rather than any one document's.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fmt, io};

use stylo::parallel::STYLE_THREAD_STACK_SIZE_KB;
use stylo::thread_state;

/// The most threads one pool may hold, the flushing thread included.
///
/// Stylo's `ScopedTLS` — the per-traversal home of every thread's
/// `ThreadLocalStyleContext` — is a fixed-length array indexed by rayon's
/// thread index, and its length is Stylo's own private `STYLO_MAX_THREADS`.
/// A wider pool indexes past the end of that array, so this is a hard
/// ceiling rather than a tuning knob, and it has to track that constant.
///
/// Stylo caps its own pool at the same six, counted the same way, and for a
/// reason of its own: the benefit of more threads levels off there.
pub const MAX_STYLE_THREADS: usize = 6;

/// Names each pool's threads apart, so a process hosting several views can
/// tell whose workers are whose in a profile or a backtrace.
static NEXT_STYLE_POOL: AtomicU64 = AtomicU64::new(0);

/// Why a pool could not be built.
#[derive(Debug)]
pub enum StylePoolError {
    /// More workers were asked for than [`MAX_STYLE_THREADS`].
    TooManyThreads(usize),
    /// A worker thread could not be started.
    Spawn(String),
    /// This thread already took over a pool. Rayon's takeover is permanent,
    /// so a thread gets one pool for its whole life.
    ThreadAlreadyPooled,
}

impl fmt::Display for StylePoolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyThreads(threads) => write!(
                formatter,
                "a style pool holds at most {MAX_STYLE_THREADS} threads, not {threads}"
            ),
            Self::Spawn(message) => write!(formatter, "a style worker failed to start: {message}"),
            Self::ThreadAlreadyPooled => formatter.write_str(
                "this thread is already part of a style pool, and rayon's takeover is permanent",
            ),
        }
    }
}

impl std::error::Error for StylePoolError {}

/// One managed pool thread, before it runs.
///
/// Index zero is never one of these: that member is the flushing thread,
/// taken over in place rather than started.
///
/// The seam an embedder whose threads are not `std::thread`'s needs: a Wasm
/// host spawns a Worker and calls [`StyleWorker::run`] inside it. Wrapping
/// rayon's own builder keeps that dependency out of this crate's public API.
pub struct StyleWorker(rayon::ThreadBuilder);

impl fmt::Debug for StyleWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StyleWorker")
            .field("name", &self.name())
            .field("stack_size", &self.stack_size())
            .finish()
    }
}

impl StyleWorker {
    /// The name the thread should carry.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.0.name()
    }

    /// The stack the thread needs; Stylo's recursive matching sizes it.
    #[must_use]
    pub fn stack_size(&self) -> Option<usize> {
        self.0.stack_size()
    }

    /// Runs the worker loop. Returns when the pool that owns it is dropped.
    pub fn run(self) {
        self.0.run();
    }
}

/// One thread's style threads, one of which is that thread itself.
///
/// Dropping it retires the managed members; they exit once the work they hold
/// finishes, and nothing waits for them. What rayon leaked to take over the
/// flushing thread is not reclaimed.
#[derive(Debug)]
pub struct StylePool {
    pool: rayon::ThreadPool,
}

impl StylePool {
    /// The pool size a machine with this much parallelism gets, the flushing
    /// thread included: Stylo's own heuristic — three quarters of it, under
    /// [`MAX_STYLE_THREADS`] — with Stylo's own arithmetic and Stylo's own
    /// meaning for the number.
    ///
    /// `None` at one, where the pool would hold the flushing thread and
    /// nothing else; that document traverses on the flushing thread with no
    /// pool at all, which is the same work on the same thread for none of the
    /// bookkeeping. Stylo cuts off at the same place for the same reason.
    ///
    /// Every target computes this from the same function, so a Wasm view and
    /// a native view on comparable hardware get the same pool. The input is
    /// what differs: `std::thread::available_parallelism` cannot answer on
    /// Wasm, so the embedder passes `navigator.hardwareConcurrency` here
    /// instead of doing arithmetic of its own.
    #[must_use]
    pub fn thread_count_for(available: usize) -> Option<NonZeroUsize> {
        NonZeroUsize::new((available * 3 / 4).min(MAX_STYLE_THREADS))
            .filter(|count| count.get() > 1)
    }

    /// [`StylePool::thread_count_for`] this machine's own parallelism.
    ///
    /// `None` on every target whose parallelism the standard library cannot
    /// answer for, Wasm among them.
    #[must_use]
    pub fn default_thread_count() -> Option<NonZeroUsize> {
        Self::thread_count_for(std::thread::available_parallelism().ok()?.get())
    }

    /// Builds a pool the calling thread joins, using this runtime's threads
    /// for the rest of it.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_threads(threads: NonZeroUsize) -> Result<Self, StylePoolError> {
        Self::with_spawn_handler(threads, |worker| {
            let mut builder = std::thread::Builder::new();
            if let Some(name) = worker.name() {
                builder = builder.name(name.to_owned());
            }
            if let Some(stack_size) = worker.stack_size() {
                builder = builder.stack_size(stack_size);
            }
            builder.spawn(move || worker.run()).map(|_| ())
        })
    }

    /// Builds a pool that the calling thread joins as index zero and that
    /// `spawn` starts the other `threads - 1` members of.
    ///
    /// **Call this on the thread that will flush the document.** That thread
    /// is taken over in place and permanently: it runs the root closure of
    /// every traversal inline, it can never join a second pool, and about
    /// 25 KB of it is never reclaimed. The module documentation says why that
    /// is the affordable side of the trade.
    ///
    /// Construction does not wait for the other members to come up: rayon
    /// primes the pool lazily, so a host that cannot block — or whose threads
    /// are Workers that boot asynchronously — is not asked to.
    pub fn with_spawn_handler<S>(
        threads: NonZeroUsize,
        mut spawn: S,
    ) -> Result<Self, StylePoolError>
    where
        S: FnMut(StyleWorker) -> io::Result<()>,
    {
        let threads = threads.get();
        if threads > MAX_STYLE_THREADS {
            return Err(StylePoolError::TooManyThreads(threads));
        }
        let id = NEXT_STYLE_POOL.fetch_add(1, Ordering::Relaxed);
        let pool = rayon::ThreadPoolBuilder::new()
            .use_current_thread()
            .num_threads(threads)
            .thread_name(move |index| format!("StyleThread#{id}.{index}"))
            .start_handler(|_| thread_state::initialize_layout_worker_thread())
            .stack_size(STYLE_THREAD_STACK_SIZE_KB * 1024)
            .spawn_handler(move |thread| spawn(StyleWorker(thread)))
            .build()
            .map_err(|error| {
                // Rayon reports the refusal to take over a thread twice the
                // same way it reports a spawn failure, so it is read back out
                // of the message rather than matched on a private kind.
                if error
                    .to_string()
                    .contains("already part of another thread pool")
                {
                    StylePoolError::ThreadAlreadyPooled
                } else {
                    StylePoolError::Spawn(error.to_string())
                }
            })?;
        Ok(Self { pool })
    }

    /// How many threads the pool holds, the flushing thread included.
    #[must_use]
    pub fn thread_count(&self) -> usize {
        self.pool.current_num_threads()
    }

    pub(crate) const fn rayon(&self) -> &rayon::ThreadPool {
        &self.pool
    }
}
