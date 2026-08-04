//! The DOM thread: script, styles, layout, paint order, hit testing.
//!
//! Everything that reads or mutates the document lives here, on one thread
//! that owns the `QuickJS` realm, the [`lynx_element::ElementTree`], and the
//! `dom::Document` underneath it. It publishes
//! [`Frame`](bobcat_core::lynx_element::dom::Frame)s — self-contained paint snapshots — and the
//! paint side consumes them without ever borrowing a document.
//!
//! The thread is spawned rather than moved into: `MainThreadRuntime` and
//! `Document` are built *inside* it from a [`Program`], which is just the
//! decoded source and page config. Nothing that holds a `QuickJS` realm or a
//! Stylo `Stylist` ever crosses a thread boundary.
//!
//! **Publishing is latest-wins, not a queue.** [`FrameSlot`] holds at most
//! one unconsumed frame; a newer one overwrites it. A frame the DOM thread
//! has already superseded is wasted work if painted, not work owed — and a
//! queue would let a slow paint thread fall arbitrarily far behind the
//! document it is drawing.
//!
//! Two request shapes reflect the two things the paint side needs:
//!
//! - [`Request::Publish`] is advisory. The DOM thread answers it only if the document actually
//!   changed, and the paint side does not wait.
//! - [`Request::Sync`] is a barrier: publish unconditionally, then acknowledge. The `frame` and
//!   `screenshot` console commands need "the current state, now" to stay deterministic, and so does
//!   boot.
//!
//! Requests are drained before a frame is built, so a burst of pointer moves
//! costs one style/layout/paint-order pass rather than one per event.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use bobcat_core::lynx_element::Viewport;
use bobcat_core::lynx_element::dom::input::InputEvent;
use bobcat_core::lynx_element::dom::{Frame, ScrollUpdate};
use bobcat_core::quickjs::MainThreadRuntime;

use crate::CliError;
use crate::page::Program;

/// What the paint side asks the DOM thread to do.
enum Request {
    Resize {
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    },
    Input(InputEvent),
    /// A scroll position the paint thread already painted. Absolute, so a
    /// late or duplicated message lands on the same place.
    Scroll(ScrollUpdate),
    /// Publish a frame if the document changed since the last one.
    Publish,
    /// Publish a frame unconditionally, then acknowledge on the channel.
    Sync(Sender<()>),
    Shutdown,
}

/// A published frame and the highest scroll update the document had applied
/// when it was built — what lets the paint thread tell "the document has
/// caught up with my scrolling" from "it has not yet".
#[derive(Debug)]
pub(crate) struct Published {
    pub(crate) frame: Frame,
    pub(crate) confirmed_scroll: u64,
}

/// The single-slot handoff between the two threads.
#[derive(Debug, Default)]
struct FrameSlot {
    frame: Mutex<Option<Published>>,
    /// Set once if the DOM thread dies; the paint side surfaces it and stops.
    fatal: Mutex<Option<CliError>>,
}

/// Locks a slot, treating a poisoned mutex as ordinary state.
///
/// Poisoning here only means the DOM thread panicked while holding it, which
/// is exactly the case the paint side has to keep working through: its job at
/// that point is to surface the error, not to panic in sympathy.
fn lock<T>(slot: &Mutex<T>) -> MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

fn fatal_of(slot: &FrameSlot) -> Option<CliError> {
    lock(&slot.fatal).take()
}

/// Runs the DOM thread's body, turning a panic into a reported error.
///
/// Single-threaded, a panic in script, style, or layout killed the process
/// and printed its message. Across a thread boundary it would instead unwind
/// into nothing: the paint side would keep presenting a frozen scene forever,
/// inputs would silently no-op, and only an explicit barrier would ever
/// notice — reporting the generic "it stopped" rather than what actually went
/// wrong. So the panic is caught, recorded with its own message, and `wake`
/// fires so an event-driven backend surfaces it at once rather than at the
/// next redraw it happens to schedule.
fn guard(slot: &FrameSlot, wake: &impl Fn(), body: impl FnOnce()) {
    if let Err(payload) = catch_unwind(AssertUnwindSafe(body)) {
        *lock(&slot.fatal) = Some(CliError::DomThread(panic_message(payload.as_ref())));
        wake();
    }
}

/// The message from a caught panic payload, which is a `&str` or a `String`
/// for every panic the standard macros produce.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload.downcast_ref::<&str>().map_or_else(
        || {
            payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "it panicked".to_owned())
        },
        |message| (*message).to_owned(),
    )
}

/// The paint side's handle on the DOM thread.
pub(crate) struct DomThread {
    requests: Sender<Request>,
    slot: Arc<FrameSlot>,
    join: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for DomThread {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DomThread")
            .field("running", &self.join.is_some())
            .finish_non_exhaustive()
    }
}

impl DomThread {
    /// Spawns the DOM thread and blocks until the bundle has booted.
    ///
    /// Boot stays synchronous on purpose: a bundle that fails to decode or
    /// throws in `renderPage` must still exit the process with that error,
    /// exactly as it did single-threaded. `wake` is called from the DOM
    /// thread whenever a frame is published — the headed backend uses it to
    /// nudge the window's event loop; a backend that polls at its own clock
    /// passes a no-op.
    pub(crate) fn spawn(
        program: Program,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        wake: impl Fn() + Send + 'static,
    ) -> Result<Self, CliError> {
        let (requests, inbox) = mpsc::channel();
        let (booted, boot_result) = mpsc::channel();
        let slot = Arc::new(FrameSlot::default());
        let thread_slot = Arc::clone(&slot);

        let join = std::thread::Builder::new()
            .name("bobcat-dom".to_owned())
            .spawn(move || {
                let viewport =
                    Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
                let runtime = match program.boot(viewport) {
                    Ok(runtime) => {
                        // Report success before the first frame so a boot
                        // error and a first-frame error stay distinguishable.
                        if booted.send(Ok(())).is_err() {
                            return;
                        }
                        runtime
                    }
                    Err(error) => {
                        let _ = booted.send(Err(error));
                        return;
                    }
                };
                guard(&thread_slot, &wake, || {
                    run(runtime, &inbox, &thread_slot, &wake);
                });
            })
            .map_err(|error| CliError::DomThread(format!("it could not be started: {error}")))?;

        match boot_result.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let _ = join.join();
                return Err(error);
            }
            Err(_) => {
                let _ = join.join();
                return Err(fatal_of(&slot).unwrap_or_else(|| {
                    CliError::DomThread("it stopped before reporting its boot result".to_owned())
                }));
            }
        }

        let dom = Self {
            requests,
            slot,
            join: Some(join),
        };
        // Boot leaves the paint side with a frame in hand, so the first
        // redraw draws the page rather than an empty scene.
        dom.sync()?;
        Ok(dom)
    }

    /// Takes the newest published frame, if the DOM thread has produced one
    /// since the last call.
    pub(crate) fn take_frame(&self) -> Option<Published> {
        lock(&self.slot.frame).take()
    }

    /// Whether a frame is waiting to be painted.
    pub(crate) fn has_frame(&self) -> bool {
        lock(&self.slot.frame).is_some()
    }

    /// The fatal error the DOM thread died of, if it did.
    pub(crate) fn fatal(&self) -> Option<CliError> {
        fatal_of(&self.slot)
    }

    pub(crate) fn resize(&self, width: f32, height: f32, device_pixel_ratio: f32) {
        self.send(Request::Resize {
            width,
            height,
            device_pixel_ratio,
        });
    }

    pub(crate) fn input(&self, event: InputEvent) {
        self.send(Request::Input(event));
        self.send(Request::Publish);
    }

    /// Reports a scroll position the paint thread has already painted.
    ///
    /// No publish request follows: the pixels are already right, and asking
    /// for a frame per wheel tick would put the round trip back that
    /// scrolling here exists to remove. The next frame produced for any other
    /// reason carries the offset.
    pub(crate) fn scrolled(&self, update: ScrollUpdate) {
        self.send(Request::Scroll(update));
    }

    /// Asks for a frame if anything changed, without waiting for one.
    pub(crate) fn request_frame(&self) {
        self.send(Request::Publish);
    }

    /// Publishes a frame for the current document state and waits for it.
    ///
    /// This is the barrier the deterministic paths need. A DOM thread that
    /// has already died reports its error here rather than hanging.
    pub(crate) fn sync(&self) -> Result<(), CliError> {
        let (ack, done) = mpsc::channel();
        if self.requests.send(Request::Sync(ack)).is_err() {
            return Err(self.dead());
        }
        match done.recv() {
            Ok(()) => Ok(()),
            Err(_) => Err(self.dead()),
        }
    }

    fn dead(&self) -> CliError {
        self.fatal()
            .unwrap_or_else(|| CliError::DomThread("it stopped unexpectedly".to_owned()))
    }

    /// Fire-and-forget. A closed channel means the DOM thread is gone, which
    /// the next [`Self::sync`] or [`Self::fatal`] reports properly; dropping
    /// the request here keeps input handling off the error path.
    fn send(&self, request: Request) {
        let _ = self.requests.send(request);
    }
}

impl Drop for DomThread {
    fn drop(&mut self) {
        let _ = self.requests.send(Request::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The DOM thread's loop.
///
/// One iteration blocks for a request, drains whatever else is queued behind
/// it, applies them all, and then publishes at most one frame. Coalescing is
/// the point: a trackpad delivers wheel events far faster than a document can
/// restyle, and answering each one with its own layout pass would make the
/// thread split slower than no split at all.
fn run(
    mut runtime: MainThreadRuntime,
    inbox: &Receiver<Request>,
    slot: &FrameSlot,
    wake: &(impl Fn() + Send),
) {
    let mut published_epoch = None;
    let mut confirmed_scroll = 0_u64;
    // Last values actually pushed into the document, so a repeated resize
    // does not re-dirty the tree.
    let mut applied_size: Option<(u32, u32)> = None;
    let mut applied_scale: Option<u32> = None;
    loop {
        let Ok(first) = inbox.recv() else {
            return;
        };
        let mut publish = false;
        let mut acks = Vec::new();
        let mut request = Some(first);
        loop {
            match request {
                Some(Request::Shutdown) => return,
                Some(Request::Resize {
                    width,
                    height,
                    device_pixel_ratio,
                }) => {
                    // Guarded, because the device setters dirty the whole
                    // subtree unconditionally: a scale-only change must not
                    // also restyle for a viewport that did not move.
                    let mut elements = runtime.elements_mut();
                    if applied_size != Some((width.to_bits(), height.to_bits())) {
                        elements.set_viewport(width, height);
                        applied_size = Some((width.to_bits(), height.to_bits()));
                    }
                    if applied_scale != Some(device_pixel_ratio.to_bits()) {
                        elements.set_device_pixel_ratio(device_pixel_ratio);
                        applied_scale = Some(device_pixel_ratio.to_bits());
                    }
                    drop(elements);
                    publish = true;
                }
                Some(Request::Input(event)) => {
                    // The element layer keeps the visual frame private and
                    // builds it for hit testing; the event is measured
                    // against what is currently painted, which is what the
                    // user aimed at.
                    runtime.elements_mut().handle_input(event);
                }
                Some(Request::Scroll(update)) => {
                    let mut elements = runtime.elements_mut();
                    // `NodeId` carries no generation, so a removal lets a
                    // later creation reuse the slot. This id was resolved
                    // against a frame on another thread; if any node has been
                    // removed since, it may now name a stranger, and
                    // scrolling one would be silently wrong rather than loud.
                    if update.epoch == elements.document().node_removal_epoch() {
                        elements.scroll_to(update.node, update.offset);
                    }
                    // The sequence is confirmed either way: a dropped update
                    // is settled, not pending, and leaving it unconfirmed
                    // would pin the paint side's offset forever.
                    confirmed_scroll = confirmed_scroll.max(update.seq);
                }
                Some(Request::Publish) => publish = true,
                Some(Request::Sync(ack)) => {
                    publish = true;
                    acks.push(ack);
                }
                None => break,
            }
            request = match inbox.try_recv() {
                Ok(request) => Some(request),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            };
        }

        // An advisory publish with nothing new to show costs nothing; a
        // barrier publishes regardless, because its caller asked for *this*
        // state rather than for a change.
        let changed = published_epoch != Some(runtime.elements().document().visual_epoch());
        if publish && (changed || !acks.is_empty()) {
            let frame = runtime.elements_mut().frame();
            published_epoch = Some(runtime.elements().document().visual_epoch());
            *lock(&slot.frame) = Some(Published {
                frame,
                confirmed_scroll,
            });
            wake();
        }
        for ack in acks {
            let _ = ack.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{FrameSlot, fatal_of, guard, panic_message};
    use crate::CliError;

    #[test]
    fn a_panicking_dom_thread_reports_its_message_and_wakes_the_paint_side() {
        // The seam that was dead code: without this, a panic after boot
        // turned into a silent freeze — the paint side re-presenting a stale
        // scene forever while every input no-opped.
        let slot = FrameSlot::default();
        let wakes = AtomicUsize::new(0);

        guard(
            &slot,
            &|| {
                wakes.fetch_add(1, Ordering::Relaxed);
            },
            || {
                panic!("layout fell over");
            },
        );

        assert_eq!(wakes.load(Ordering::Relaxed), 1, "the paint side is woken");
        let error = fatal_of(&slot).expect("the panic was recorded");
        assert!(matches!(error, CliError::DomThread(_)));
        assert!(
            error.to_string().contains("layout fell over"),
            "the panic's own message survives, got {error}",
        );
    }

    #[test]
    fn a_body_that_returns_normally_records_nothing() {
        let slot = FrameSlot::default();
        let wakes = AtomicUsize::new(0);
        guard(
            &slot,
            &|| {
                wakes.fetch_add(1, Ordering::Relaxed);
            },
            || {},
        );
        assert_eq!(wakes.load(Ordering::Relaxed), 0);
        assert!(fatal_of(&slot).is_none());
    }

    #[test]
    fn panic_payloads_of_both_shapes_are_readable() {
        // `panic!("literal")` yields a `&str`; a formatted one yields a
        // `String`. Losing either would leave the operator with a bare "it
        // panicked" exactly when the message matters.
        assert_eq!(panic_message(&"borrowed"), "borrowed");
        assert_eq!(panic_message(&String::from("owned")), "owned");
        assert_eq!(panic_message(&7_u32), "it panicked");
    }
}
