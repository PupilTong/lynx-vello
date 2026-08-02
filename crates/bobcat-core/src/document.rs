//! Bobcat's DOM document and Lynx element-tree composition.
//!
//! Painting is an internal `dom` responsibility. Bobcat selects no renderer
//! and exposes no painter; it only re-exports the concrete document and Lynx
//! policy layer used by the runtime.

pub use lynx_element::ElementTree;
use lynx_element::dom;

/// A DOM document with its paint pipeline owned internally by `dom`.
pub type Document<T> = dom::Document<T>;

/// Constructs a Bobcat document.
#[must_use]
pub fn new<T>(device: dom::Device) -> Document<T> {
    dom::Document::new(device)
}

#[cfg(test)]
mod tests {
    use lynx_element::{PageConfig, Viewport};

    use super::ElementTree;

    #[test]
    fn image_resource_access_schedules_a_new_scene() {
        let mut tree = ElementTree::new(Viewport::new(393.0, 727.0), PageConfig::default());
        let epoch = tree.document().visual_epoch();

        let _ = tree.images_mut().remove_url("missing");

        assert_eq!(tree.document().visual_epoch(), epoch + 1);
    }
}
