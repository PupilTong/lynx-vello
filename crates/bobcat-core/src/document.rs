//! Bobcat's Pulsar-rendered document and Lynx element-tree specializations.

use std::cell::Ref;
use std::ops::{Deref, DerefMut};

use lynx_element::{ElementId, ElementPapi, PageConfig, PapiError, Viewport};
use pulsar::ImageStore;
use pulsar::vello::Scene;

/// A DOM document with the retained Pulsar renderer injected at construction.
pub type Document<T> = dom::Document<T, pulsar::Pulsar>;

/// Constructs a rendered Bobcat document.
#[must_use]
pub fn new<T>(device: dom::Device) -> Document<T> {
    dom::Document::with_renderer(device, pulsar::Pulsar::new())
}

/// A Lynx element tree with Bobcat's retained Pulsar renderer injected.
///
/// The inner generic tree is owned by `lynx-element`; this facade makes the
/// concrete runtime composition explicit and keeps Pulsar-specific resource
/// access out of the Lynx policy crate.
#[derive(Debug)]
pub struct ElementTree(lynx_element::ElementTree<pulsar::Pulsar>);

impl ElementTree {
    /// Creates an empty Lynx tree and injects Pulsar into its DOM document.
    #[must_use]
    pub fn new(viewport: Viewport, config: PageConfig) -> Self {
        Self(lynx_element::ElementTree::with_renderer(
            viewport,
            config,
            pulsar::Pulsar::new(),
        ))
    }

    /// The retained scene produced by the last render.
    #[must_use]
    pub fn scene(&self) -> Ref<'_, Scene> {
        self.0.render_output()
    }

    /// Decoded images registered with the injected renderer.
    #[must_use]
    pub fn images(&self) -> Ref<'_, ImageStore> {
        Ref::map(self.0.renderer(), pulsar::Pulsar::images)
    }

    /// Registers or updates decoded images without exposing DOM mutation.
    pub fn images_mut(&mut self) -> &mut ImageStore {
        self.0.renderer_mut().images_mut()
    }
}

impl Deref for ElementTree {
    type Target = lynx_element::ElementTree<pulsar::Pulsar>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ElementTree {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl ElementPapi for ElementTree {
    type Error = PapiError;

    fn create_page(&mut self, component_id: &str, component_css_id: i32) -> ElementId {
        self.0.create_page(component_id, component_css_id)
    }

    fn create_view(&mut self, parent_component: ElementId) -> Result<ElementId, Self::Error> {
        self.0.create_view(parent_component)
    }

    fn append_element(
        &mut self,
        parent: ElementId,
        child: ElementId,
    ) -> Result<ElementId, Self::Error> {
        self.0.append_element(parent, child)
    }

    fn drop_element(&mut self, element: ElementId) -> bool {
        self.0.drop_element(element)
    }

    fn flush_element_tree(&mut self) -> bool {
        self.0.flush_element_tree()
    }
}

#[cfg(test)]
mod tests {
    use super::{ElementTree, PageConfig, Viewport};

    #[test]
    fn renderer_resource_access_schedules_a_new_scene() {
        let mut tree = ElementTree::new(Viewport::new(393.0, 727.0), PageConfig::default());
        let epoch = tree.document().visual_epoch();

        let _ = tree.images_mut().remove_url("missing");

        assert_eq!(tree.document().visual_epoch(), epoch + 1);
    }
}
