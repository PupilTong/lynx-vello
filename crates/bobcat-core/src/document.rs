//! Bobcat's rendered document specialization.

/// A DOM document with the retained Pulsar renderer injected at construction.
pub type Document<T> = dom::Document<T, pulsar::Pulsar>;

/// Constructs a rendered Bobcat document.
#[must_use]
pub fn new<T>(device: dom::Device) -> Document<T> {
    dom::Document::with_renderer(device, pulsar::Pulsar::new())
}
