//! The style subsystem: the per-document Stylo engine, the DOM traits Stylo
//! matches against, the traversal-driven flush, mutation-time invalidation,
//! post-flush damage classes, the css-contain projection, and the DOM
//! selector-query APIs those same traits answer.

pub(crate) mod damage;
pub(crate) mod device;
pub(crate) mod engine;
pub(crate) mod flush;
mod invalidation;
pub(crate) mod query;
mod traits;
