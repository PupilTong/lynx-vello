//! Native Lynx runtime core for lynx-vello.
//!
//! The protocol modules let hosts inject a [`script::ScriptEngine`] and
//! [`resource::ResourceFetcher`] through static generic contracts.
//! [`lynx_element::ElementPapi`] and its element tree live in `lynx-element`;
//! the concrete DOM composition and its private paint pipeline remain owned by
//! `lynx-element` and `dom` respectively.
//!
//! The default `quickjs` feature adds Bobcat's internal `QuickJS` adapter and
//! main-thread runtime. Disable default features to use only the external
//! engine contracts without compiling `QuickJS`.

#[cfg(feature = "quickjs")]
pub mod quickjs;
pub mod resource;
pub mod script;
pub mod view;

pub use lynx_element::dom::{self, pulsar};
pub use lynx_element::{
    ElementId, ElementPapi, ElementTree, LynxElement, PageConfig, PapiError, Viewport,
};
#[cfg(feature = "quickjs")]
pub use quickjs::QuickJsInitializationError;
#[cfg(feature = "quickjs")]
pub use quickjs::mainthread::{MainThreadError, MainThreadRuntime};
