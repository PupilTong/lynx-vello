//! Native Bobcat embedders, sharing the complete source and resource systems.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "server")]
pub mod server;
