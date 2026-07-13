//! Native Lynx runtime engine integration for lynx-vello.
//!
//! Engine subsystems are exposed as explicit modules so their ownership and
//! dependency boundaries remain visible as the runtime grows.

pub mod resource;
pub mod script;
pub mod view;
