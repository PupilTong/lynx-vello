//! Native Lynx runtime core for lynx-vello.
//!
//! The protocol modules let hosts inject a [`script::ScriptEngine`] and
//! [`resource::ResourceFetcher`] through static generic contracts.
//! [`lynx_element::ElementPapi`] and its generic element tree live in
//! `lynx-element`; [`document`] injects Pulsar and exposes the rendered Bobcat
//! specialization.
//!
//! The default `quickjs` feature adds Bobcat's internal `QuickJS` adapter and
//! main-thread runtime. Disable default features to use only the external
//! engine contracts without compiling `QuickJS`.

pub mod document;
pub mod element;
#[cfg(feature = "quickjs")]
pub mod quickjs;
pub mod resource;
pub mod script;
pub mod view;

pub use document::ElementTree;
pub use dom;
pub use lynx_element::{ElementId, ElementPapi, LynxElement, PageConfig, PapiError, Viewport};
pub use pulsar;
#[cfg(feature = "quickjs")]
pub use quickjs::QuickJsInitializationError;
#[cfg(feature = "quickjs")]
pub use quickjs::mainthread::{MainThreadError, MainThreadRuntime};
