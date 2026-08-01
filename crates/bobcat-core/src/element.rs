//! Static host contract for the Element PAPI exposed to a script engine.

use std::fmt;

/// The stable Element-PAPI identity crossing the JavaScript boundary.
pub type ElementId = u32;

/// A host-owned element tree that a script runtime can mutate.
///
/// The contract lives below any concrete JavaScript engine. Implementations
/// stay statically dispatched, so enabling an external engine does not require
/// a `dyn` element adapter.
pub trait ElementPapi: 'static {
    type Error: fmt::Display;

    fn create_page(&mut self, component_id: &str, component_css_id: i32) -> ElementId;

    fn create_view(&mut self, parent_component: ElementId) -> Result<ElementId, Self::Error>;

    fn append_element(
        &mut self,
        parent: ElementId,
        child: ElementId,
    ) -> Result<ElementId, Self::Error>;

    fn drop_element(&mut self, element: ElementId) -> bool;

    fn flush_element_tree(&mut self) -> bool;
}
