//! Two documents, two threads, two pools, one restyle each — at the same
//! time.
//!
//! Style traversals used to be serialized against each other by a
//! process-wide mutex, because they shared Stylo's one global thread pool and
//! a worker waiting on its own scope will run another traversal's chunk on a
//! thread whose bloom filter and style-sharing cache are already borrowed.
//! Per-document pools make the thread sets disjoint instead, and these tests
//! are the claim that follows from that: concurrent flushes overlap, and
//! neither one's workers ever appear in the other's.

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

use dom::{Device, Document, NodeId, StylePool, StylesheetOrigin, standards_device};
use euclid::{Scale, Size2D};
use stylo::device::servo::FontMetricsProvider;
use stylo::font_metrics::FontMetrics;
use stylo::media_queries::MediaType;
use stylo::properties::ComputedValues;
use stylo::properties::style_structs::Font;
use stylo::queries::values::PrefersColorScheme;
use stylo::servo::media_features::PointerCapabilities;
use stylo::values::computed::font::GenericFontFamily;
use stylo::values::computed::{CSSPixelLength, Length};
use stylo::values::specified::font::QueryFontMetricsFlags;
use stylo_traits::{CSSPixel, DevicePixel};

/// Long enough that a loaded machine never trips it, short enough that a
/// serialized pair reports instead of hanging the suite.
const RENDEZVOUS_TIMEOUT: Duration = Duration::from_secs(20);

/// Not Stylo's own `FONT_MEDIUM_PX`, so a `font-size: medium` that reached the
/// witness is distinguishable from one that never did.
const WITNESS_BASE_FONT_SIZE_PX: f32 = 20.0;

/// A meeting point for two traversals that must be running at once.
///
/// Both parties are inside `driver::traverse_dom` when they arrive, on their
/// own document's worker. Under a shared pool serialized by one lock, the
/// second party never arrives and the first times out — which is the whole
/// difference this reports on, without measuring a wall clock.
#[derive(Debug, Default)]
struct Rendezvous {
    arrived: Mutex<usize>,
    released: Condvar,
    timed_out: AtomicBool,
}

impl Rendezvous {
    /// Blocks until both parties are here, or gives up and records that.
    fn meet(&self) {
        let mut arrived = self.arrived.lock().expect("rendezvous count");
        *arrived += 1;
        if *arrived >= 2 {
            self.released.notify_all();
            return;
        }
        let (_guard, timeout) = self
            .released
            .wait_timeout_while(arrived, RENDEZVOUS_TIMEOUT, |arrived| *arrived < 2)
            .expect("rendezvous count");
        if timeout.timed_out() {
            self.timed_out.store(true, Ordering::Release);
        }
    }

    fn met(&self) -> bool {
        !self.timed_out.load(Ordering::Acquire)
    }
}

/// Records which threads resolve font-relative lengths, and meets the other
/// document there once.
///
/// Stylo calls this from inside the traversal, on whichever worker is styling
/// the element — so the recorded set is exactly the set of threads that
/// touched this document's style data.
#[derive(Debug)]
struct WitnessProvider {
    threads: Arc<Mutex<HashSet<ThreadId>>>,
    rendezvous: Arc<Rendezvous>,
    met: AtomicBool,
}

impl FontMetricsProvider for WitnessProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &Font,
        base_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        self.threads
            .lock()
            .expect("witnessed threads")
            .insert(std::thread::current().id());
        if !self.met.swap(true, Ordering::AcqRel) {
            self.rendezvous.meet();
        }
        FontMetrics {
            // A `ch` resolves against this, so it has to be non-zero for the
            // computed widths the test compares to differ from the default.
            ascent: Length::new(base_size.px()),
            ..FontMetrics::default()
        }
    }

    fn base_size_for_generic(&self, _generic: GenericFontFamily) -> Length {
        self.threads
            .lock()
            .expect("witnessed threads")
            .insert(std::thread::current().id());
        if !self.met.swap(true, Ordering::AcqRel) {
            self.rendezvous.meet();
        }
        Length::new(WITNESS_BASE_FONT_SIZE_PX)
    }
}

fn witness_device(provider: WitnessProvider) -> Device {
    standards_device(
        MediaType::screen(),
        Size2D::<f32, CSSPixel>::new(800.0, 600.0),
        Size2D::<f32, DevicePixel>::new(800.0, 600.0),
        Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
        Box::new(provider),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        PrefersColorScheme::Light,
        PointerCapabilities::empty(),
        PointerCapabilities::empty(),
    )
}

fn pool(threads: usize) -> StylePool {
    StylePool::with_threads(NonZeroUsize::new(threads).expect("a positive worker count"))
        .expect("style workers start")
}

/// A page whose every element resolves an absolute font-size keyword, which
/// is what puts the witness on the traversal's own thread: Stylo asks the
/// device for the generic family's base size while cascading each one. Wide
/// enough for the driver to hand work to more than one worker.
fn wide_page(document: &mut Document<()>, elements: usize) -> Vec<NodeId> {
    document.add_stylesheet("div { font-size: medium; }", StylesheetOrigin::Author);
    let root = document.document_element().id();
    (0..elements)
        .map(|_| {
            let child = document.create_element("div", ());
            document.append_child(root, child);
            child
        })
        .collect()
}

#[test]
fn two_documents_restyle_at_the_same_time() {
    let rendezvous = Arc::new(Rendezvous::default());
    let witnesses: [Arc<Mutex<HashSet<ThreadId>>>; 2] = Default::default();

    let flushers: Vec<_> = witnesses
        .iter()
        .map(|threads| {
            let threads = Arc::clone(threads);
            let rendezvous = Arc::clone(&rendezvous);
            std::thread::spawn(move || {
                let device = witness_device(WitnessProvider {
                    threads,
                    rendezvous,
                    met: AtomicBool::new(false),
                });
                let mut document = Document::new(device, "page", ());
                document.set_style_pool(pool(2));
                let children = wide_page(&mut document, 64);
                document.layout();
                let sizes: Vec<f32> = children
                    .iter()
                    .map(|id| {
                        document
                            .get(*id)
                            .expect("the child is live")
                            .computed_style()
                            .expect("the child is styled")
                            .clone_font_size()
                            .computed_size()
                            .px()
                    })
                    .collect();
                (sizes, std::thread::current().id())
            })
        })
        .collect();

    let outcomes: Vec<_> = flushers
        .into_iter()
        .map(|flusher| flusher.join().expect("the flushing thread finished"))
        .collect();

    assert!(
        rendezvous.met(),
        "both traversals were inside `traverse_dom` at once"
    );
    for (sizes, _) in &outcomes {
        assert_eq!(sizes.len(), 64);
        assert!(
            sizes
                .iter()
                .all(|size| (size - WITNESS_BASE_FONT_SIZE_PX).abs() < f32::EPSILON),
            "every element restyled: {sizes:?}"
        );
    }

    let [first, second] = witnesses;
    let first = first.lock().expect("witnessed threads").clone();
    let second = second.lock().expect("witnessed threads").clone();
    assert!(!first.is_empty() && !second.is_empty());
    assert!(
        first.is_disjoint(&second),
        "no worker served both documents: {first:?} vs {second:?}"
    );
    let [(_, first_flusher), (_, second_flusher)] = outcomes.as_slice() else {
        unreachable!("two documents were flushed")
    };
    assert!(
        first.contains(first_flusher) && second.contains(second_flusher),
        "each flushing thread is index zero of its own pool, so it does style \
         work itself: {first:?} has {first_flusher:?}, {second:?} has {second_flusher:?}"
    );
    assert!(
        !first.contains(second_flusher) && !second.contains(first_flusher),
        "membership does not cross: neither flusher serves the other's document"
    );
}

/// Rayon's takeover is permanent, so a thread gets one pool for its whole
/// life. Every pool here is built on a `bobcat-main` created for its view and
/// dropped with it, which is the only reason that is affordable.
#[test]
fn a_thread_cannot_take_over_a_second_pool() {
    std::thread::spawn(|| {
        let first = pool(2);
        drop(first);
        let error = StylePool::with_threads(NonZeroUsize::new(2).expect("a positive count"))
            .expect_err("the takeover is permanent, so dropping does not release the thread");
        assert!(
            matches!(error, dom::StylePoolError::ThreadAlreadyPooled),
            "the refusal is named rather than reported as a spawn failure: {error}"
        );
    })
    .join()
    .expect("the probing thread finished");
}

#[test]
fn a_document_without_a_pool_traverses_on_the_flushing_thread() {
    let rendezvous = Arc::new(Rendezvous::default());
    let threads: Arc<Mutex<HashSet<ThreadId>>> = Arc::default();
    let device = witness_device(WitnessProvider {
        threads: Arc::clone(&threads),
        rendezvous: Arc::clone(&rendezvous),
        // Pre-armed: this document is the only party, so it must never wait.
        met: AtomicBool::new(true),
    });
    let mut document = Document::new(device, "page", ());
    wide_page(&mut document, 32);
    document.layout();

    let witnessed = threads.lock().expect("witnessed threads").clone();
    assert_eq!(
        witnessed,
        HashSet::from([std::thread::current().id()]),
        "with no pool the traversal never leaves the thread that flushed it"
    );
}

#[test]
fn a_pool_wider_than_stylo_indexes_is_refused() {
    let threads = NonZeroUsize::new(dom::MAX_STYLE_THREADS + 1).expect("a positive worker count");
    let error = StylePool::with_threads(threads).expect_err("the ceiling is enforced");
    assert!(
        error
            .to_string()
            .contains(&dom::MAX_STYLE_THREADS.to_string()),
        "the ceiling is named: {error}"
    );
}

/// The pool size is Stylo's own, arrived at by Stylo's own arithmetic and
/// counted the way Stylo counts it — the flushing thread included — so a lone
/// document restyles on exactly the threads it did when every document shared
/// Stylo's global pool.
///
/// The cut-off at one is Stylo's too: a pool holding only the flushing thread
/// is that thread doing the same work for extra bookkeeping.
#[test]
fn the_pool_size_is_stylos_own_heuristic() {
    let sizes: Vec<_> = [1_usize, 2, 3, 4, 8, 10, 16, 64]
        .into_iter()
        .map(|available| {
            (
                available,
                StylePool::thread_count_for(available).map(NonZeroUsize::get),
            )
        })
        .collect();
    assert_eq!(
        sizes,
        vec![
            (1, None),
            (2, None),
            (3, Some(2)),
            (4, Some(3)),
            (8, Some(6)),
            (10, Some(6)),
            (16, Some(6)),
            (64, Some(6)),
        ],
        "three quarters of the machine, capped at MAX_STYLE_THREADS, nothing below two"
    );
    assert!(
        sizes
            .iter()
            .all(|(_, count)| count.is_none_or(|count| count <= dom::MAX_STYLE_THREADS)),
        "the cap is never exceeded, so ScopedTLS is never indexed past its end"
    );
}
