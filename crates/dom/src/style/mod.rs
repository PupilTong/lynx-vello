//! The style subsystem: the per-document Stylo engine, the DOM traits Stylo
//! matches against, the traversal-driven flush, mutation-time invalidation,
//! post-flush damage classes, and the css-contain projection.

pub(crate) mod damage;
pub(crate) mod device;
pub(crate) mod engine;
pub(crate) mod flush;
mod invalidation;
mod traits;
