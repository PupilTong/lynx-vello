//! The one monotonic instant type every timestamp this crate takes is read
//! from: the painter's frame clock, a commit's timing marks
//! ([`crate::visual::timing`]) and a presented frame's
//! ([`crate::painter::PresentedFrame`]).
//!
//! `std::time::Instant` panics on `wasm32-unknown-unknown`, so that target
//! reads `web_time::Instant`, the same type on every other target. The
//! readings of one process are comparable across its threads, which is what
//! lets a commit's marks on the document's thread and a present's on the
//! painter's be subtracted.

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use std::time::Instant as ClockInstant;

#[cfg(target_arch = "wasm32")]
pub(crate) use web_time::Instant as ClockInstant;
