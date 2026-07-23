//! `lynx-widget` — the Widget Element-PAPI layer of **lynx-vello**.

pub mod handle;
pub mod kind;
pub mod papi;
pub mod state;
pub mod style;
pub mod ua;

mod ingest;

pub use handle::WidgetHandle;
pub use kind::WidgetKind;
pub use papi::{WidgetError, WidgetTree};
pub use state::{EventBinding, EventBindingKind, WidgetState};
pub use style::{StyleEngine, ViewMetrics};
pub use ua::PageConfig;
pub use w3c_dom::{
    ComputedStyle, ElementState, Parallelism, StylesheetOrigin, property_is_supported,
};

pub type Widget = w3c_dom::Node<WidgetState>;
