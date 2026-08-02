//! Native Lynx runtime core for lynx-vello.
//!
//! The protocol modules let hosts inject a [`script::ScriptEngine`] and
//! [`resource::ResourceFetcher`] through static generic contracts.
//! The concrete [`lynx_element::ElementTree`] lives in the internal
//! `lynx-element` layer. Product embedders enable `renderer` and use
//! `bobcat_core::renderer`; they never receive the tree's retained scene or
//! GPU submission objects.
//!
//! The default `quickjs` feature adds Bobcat's internal `QuickJS` adapter and
//! main-thread runtime. Disable default features to use only the external
//! engine contracts without compiling `QuickJS`. The non-default `renderer`
//! feature adds the QuickJS-backed product façade and, on macOS/Linux, its
//! native window renderer.

#[cfg(feature = "quickjs")]
pub mod quickjs;
#[cfg(feature = "renderer")]
pub mod renderer;
pub mod resource;
pub mod script;
pub mod view;
