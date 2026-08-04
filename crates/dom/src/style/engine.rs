//! Standards-oriented CSS parsing, selector matching, and cascade execution.

use std::sync::Arc as StdArc;

use stylo::context::QuirksMode;
use stylo::device::Device;
use stylo::media_queries::MediaList;
use stylo::servo_arc::Arc;
use stylo::shared_lock::{SharedRwLock, StylesheetGuards};
pub use stylo::stylesheets::Origin as StylesheetOrigin;
use stylo::stylesheets::{AllowImportRules, DocumentStyleSheet, Origin, Stylesheet, UrlExtraData};
use stylo::stylist::Stylist;

use crate::{Document, Node};

/// The private stylo state owned by exactly one [`Document`].
pub(crate) struct StyleEngine {
    stylist: Stylist,
    lock: StdArc<SharedRwLock>,
    url_data: UrlExtraData,
}

impl std::fmt::Debug for StyleEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StyleEngine")
            .field("viewport", &self.stylist.device().viewport_size())
            .field(
                "device_pixel_ratio",
                &self.stylist.device().device_pixel_ratio(),
            )
            .finish_non_exhaustive()
    }
}

impl StyleEngine {
    #[must_use]
    pub(crate) fn new(device: Device, url_data: UrlExtraData) -> Self {
        Self {
            stylist: Stylist::new(device, QuirksMode::NoQuirks),
            lock: StdArc::new(SharedRwLock::new()),
            url_data,
        }
    }

    pub(crate) fn lock(&self) -> StdArc<SharedRwLock> {
        StdArc::clone(&self.lock)
    }

    pub(crate) fn url_data(&self) -> UrlExtraData {
        self.url_data.clone()
    }

    pub(crate) fn stylist(&self) -> &Stylist {
        &self.stylist
    }

    pub(crate) fn shared_lock(&self) -> &SharedRwLock {
        &self.lock
    }

    #[must_use]
    pub(crate) fn device(&self) -> &Device {
        self.stylist.device()
    }

    pub(crate) fn update_device(&mut self, update: impl FnOnce(&mut Device)) {
        update(self.stylist.device_mut());
        self.refresh_device();
    }

    pub(crate) fn set_viewport(&mut self, width: f32, height: f32) {
        self.update_device(|device| {
            let dpr = device.device_pixel_ratio().get();
            device.set_viewport_size(euclid::Size2D::new(width, height));
            device.set_device_size(euclid::Size2D::new(width * dpr, height * dpr));
        });
    }

    pub(crate) fn set_device_pixel_ratio(&mut self, device_pixel_ratio: f32) {
        self.update_device(|device| {
            device.set_device_pixel_ratio(euclid::Scale::new(device_pixel_ratio));
            let viewport = device.viewport_size();
            device.set_device_size(euclid::Size2D::new(
                viewport.width * device_pixel_ratio,
                viewport.height * device_pixel_ratio,
            ));
        });
    }

    pub(crate) fn add_stylesheet(&mut self, css: &str, origin: Origin) {
        let media = Arc::new(self.lock.wrap(MediaList::empty()));
        let sheet = Stylesheet::from_str(
            css,
            self.url_data.clone(),
            origin,
            media,
            self.lock.as_ref().clone(),
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::Yes,
        );
        self.install_stylesheet(sheet);
    }

    fn install_stylesheet(&mut self, sheet: Stylesheet) {
        let guard = self.lock.read();
        self.stylist
            .append_stylesheet(DocumentStyleSheet(Arc::new(sheet)), &guard);
        self.stylist.flush(&StylesheetGuards::same(&guard));
    }

    fn refresh_device(&mut self) {
        let guard = self.lock.read();
        let guards = StylesheetGuards::same(&guard);
        let changed = self
            .stylist
            .media_features_change_changed_style(&guards, self.stylist.device());
        if !changed.is_empty() {
            self.stylist.force_stylesheet_origins_dirty(changed);
            self.stylist.flush(&guards);
        }
    }
}

impl<T> Document<T> {
    #[must_use]
    pub(crate) fn device(&self) -> &Device {
        self.style_engine().device()
    }

    /// The viewport in CSS px.
    #[must_use]
    pub fn viewport_size(&self) -> crate::Size2D<f32> {
        let size = self.device().viewport_size();
        crate::Size2D::new(size.width, size.height)
    }

    /// CSS-px → device-px scale factor.
    #[must_use]
    pub fn device_pixel_ratio(&self) -> f32 {
        self.device().device_pixel_ratio().get()
    }

    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.change_style_context(|engine| engine.set_viewport(width, height));
    }

    pub fn set_device_pixel_ratio(&mut self, device_pixel_ratio: f32) {
        self.change_style_context(|engine| engine.set_device_pixel_ratio(device_pixel_ratio));
    }

    pub fn add_stylesheet(&mut self, css: &str, origin: Origin) {
        self.change_style_context(|engine| engine.add_stylesheet(css, origin));
    }

    fn change_style_context(&mut self, change: impl FnOnce(&mut StyleEngine)) {
        self.note_visual_mutation();
        change(self.style_engine_mut());
        if let Some(root) = self.root_element().map(Node::id) {
            self.mark_subtree_dirty(root);
        }
    }
}
