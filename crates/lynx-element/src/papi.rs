//! The statically dispatched Element-PAPI host contract.

use std::fmt;

use crate::ElementId;

/// A host-owned Lynx element tree that a script runtime can mutate.
///
/// Implementations own handle validation and DOM policy; JavaScript adapters
/// only translate their native value representation into these operations.
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
