//! The display's own clock, for the window this embedder owns.
//!
//! `CVDisplayLink` calls back once per refresh of one display, on a thread of
//! its own. All this does with that callback is post one wakeup into the run
//! loop: the view lives on the main thread and nothing here may touch it. The
//! turn that wakeup opens is where the frame is actually drawn, which is why
//! the callback carries no timestamp — the engine reads its own clock when it
//! composes.
//!
//! It runs only while [`bobcat_core::LynxView::owes_frame`] holds. An idle
//! window would otherwise be woken sixty times a second to decide it has
//! nothing to do.

use std::ffi::c_void;

/// The `CoreVideo` entry points this needs, and nothing else.
///
/// A C API, so the calls are `unsafe`; each one's contract is stated at its
/// call site. The link itself is an opaque retained object, and the timestamps
/// the callback is handed are ignored.
#[allow(
    unsafe_code,
    reason = "CoreVideo is a C API with no Rust binding in this workspace"
)]
mod core_video {
    use std::ffi::c_void;

    /// `CVDisplayLinkRef`, opaque.
    pub(super) type DisplayLinkRef = *mut c_void;
    /// `CGDirectDisplayID`, which winit hands out as the monitor's native id.
    pub(super) type DisplayId = u32;
    /// `CVReturn`; `kCVReturnSuccess` is zero.
    pub(super) type Return = i32;

    pub(super) type OutputCallback = extern "C" fn(
        link: DisplayLinkRef,
        now: *const c_void,
        output_time: *const c_void,
        flags_in: u64,
        flags_out: *mut u64,
        context: *mut c_void,
    ) -> Return;

    #[link(name = "CoreVideo", kind = "framework")]
    unsafe extern "C" {
        pub(super) fn CVDisplayLinkCreateWithCGDisplay(
            display: DisplayId,
            link: *mut DisplayLinkRef,
        ) -> Return;
        pub(super) fn CVDisplayLinkSetOutputCallback(
            link: DisplayLinkRef,
            callback: OutputCallback,
            context: *mut c_void,
        ) -> Return;
        pub(super) fn CVDisplayLinkStart(link: DisplayLinkRef) -> Return;
        pub(super) fn CVDisplayLinkStop(link: DisplayLinkRef) -> Return;
        pub(super) fn CVDisplayLinkRelease(link: DisplayLinkRef);
    }
}

use self::core_video::{DisplayId, DisplayLinkRef, Return};

const SUCCESS: Return = 0;

/// What the display link does on every refresh. Boxed twice so the callback
/// gets one thin pointer to reconstruct, and owned by [`DisplayLink`] for
/// exactly as long as the link that may call it.
type Wake = Box<dyn Fn() + Send>;

/// One display's vsync, delivered as a wakeup on the run loop.
pub(crate) struct DisplayLink {
    link: DisplayLinkRef,
    wake: *mut Wake,
    running: bool,
}

impl std::fmt::Debug for DisplayLink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DisplayLink")
            .field("running", &self.running)
            .finish_non_exhaustive()
    }
}

/// Posts the wakeup. Runs on `CoreVideo`'s own thread, so it touches nothing
/// but the closure it was handed — which does one non-blocking send.
#[allow(
    unsafe_code,
    reason = "the C callback reconstructs the context pointer DisplayLink gave it"
)]
extern "C" fn on_vsync(
    _link: DisplayLinkRef,
    _now: *const c_void,
    _output_time: *const c_void,
    _flags_in: u64,
    _flags_out: *mut u64,
    context: *mut c_void,
) -> Return {
    // SAFETY: `context` is the `*mut Wake` handed to
    // `CVDisplayLinkSetOutputCallback` in `DisplayLink::new`. It is owned by
    // the `DisplayLink`, which stops this callback before it frees it, so the
    // pointer is valid for the whole time the callback can run, and this is
    // the only thread that dereferences it while the link is started.
    let wake = unsafe { &*context.cast::<Wake>() };
    wake();
    SUCCESS
}

impl DisplayLink {
    /// Opens the link for one display, stopped.
    ///
    /// Answers `None` if `CoreVideo` will not give one — a display id that has
    /// gone away with its monitor, most likely. The caller falls back to
    /// waiting for something else to wake it, which costs the animation its
    /// pacing but not the window.
    #[allow(
        unsafe_code,
        reason = "creating and configuring a CVDisplayLink is three C calls"
    )]
    pub(crate) fn new(display: DisplayId, wake: impl Fn() + Send + 'static) -> Option<Self> {
        let mut link: DisplayLinkRef = std::ptr::null_mut();
        // SAFETY: `link` is a live out-parameter for the whole call, and
        // CoreVideo writes a retained link into it only on success.
        let created =
            unsafe { core_video::CVDisplayLinkCreateWithCGDisplay(display, &raw mut link) };
        if created != SUCCESS || link.is_null() {
            return None;
        }

        let wake: *mut Wake = Box::into_raw(Box::new(Box::new(wake) as Wake));
        // SAFETY: `link` was just created, and `wake` is a live allocation
        // this struct owns and outlives every call the link can make: `Drop`
        // stops the link before it frees it.
        let armed =
            unsafe { core_video::CVDisplayLinkSetOutputCallback(link, on_vsync, wake.cast()) };
        if armed != SUCCESS {
            // SAFETY: `link` is a live link this call has sole ownership of,
            // and it was never started, so no callback can be running.
            unsafe { core_video::CVDisplayLinkRelease(link) };
            // SAFETY: `wake` came from `Box::into_raw` above and the link
            // that could have read it is released.
            drop(unsafe { Box::from_raw(wake) });
            return None;
        }

        Some(Self {
            link,
            wake,
            running: false,
        })
    }

    /// Starts or stops the link, if it is not already there.
    #[allow(
        unsafe_code,
        reason = "starting and stopping a CVDisplayLink are one C call each"
    )]
    pub(crate) fn set_running(&mut self, running: bool) {
        if running == self.running {
            return;
        }
        // SAFETY: `self.link` is live for this struct's whole life, and both
        // calls are the documented way to drive it. `CVDisplayLinkStop`
        // returns only once the callback is no longer running.
        let changed = unsafe {
            if running {
                core_video::CVDisplayLinkStart(self.link)
            } else {
                core_video::CVDisplayLinkStop(self.link)
            }
        };
        // A link that refuses to start leaves the window on whatever else
        // wakes it, which is the same place a display with no link is.
        self.running = running && changed == SUCCESS;
    }
}

impl Drop for DisplayLink {
    #[allow(
        unsafe_code,
        reason = "releasing the link and its context, in that order"
    )]
    fn drop(&mut self) {
        self.set_running(false);
        // SAFETY: the link is stopped, so no callback is running or can
        // start, and this is the only owner of both allocations. The link
        // goes first: it is what could still reach the context.
        unsafe {
            core_video::CVDisplayLinkRelease(self.link);
            drop(Box::from_raw(self.wake));
        }
    }
}
