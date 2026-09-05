//! The style subsystem: the per-document Stylo engine and the worker threads
//! its traversals run on, the DOM traits Stylo matches against, the
//! traversal-driven flush, mutation-time invalidation, post-flush damage
//! classes, the css-contain projection, and the DOM
//! selector-query APIs those same traits answer, plus the animation
//! timeline that drives Stylo's animation state between flushes.

pub(crate) mod animation;
pub(crate) mod curve_export;
pub(crate) mod damage;
pub(crate) mod device;
pub(crate) mod engine;
pub(crate) mod flush;
mod invalidation;
pub(crate) mod pool;
pub(crate) mod query;
mod traits;
