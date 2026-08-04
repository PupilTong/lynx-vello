//! The style subsystem: the per-document Stylo engine, the DOM traits Stylo
//! matches against, the traversal-driven flush, mutation-time invalidation,
//! post-flush damage classes, and the css-contain projection.

pub(crate) mod contain;
pub(crate) mod damage;
pub(crate) mod engine;
mod flush;
mod invalidation;
mod traits;
