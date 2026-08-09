//! Standards-oriented CSS parsing, selector matching, and cascade execution.

use std::sync::Arc as StdArc;

use stylo::author_styles::AuthorStyles;
use stylo::context::QuirksMode;
use stylo::device::Device;
use stylo::media_queries::MediaList;
use stylo::servo_arc::Arc;
use stylo::shared_lock::{SharedRwLock, StylesheetGuards};
pub use stylo::stylesheets::Origin as StylesheetOrigin;
use stylo::stylesheets::{
    AllowImportRules, CustomMediaMap, DocumentStyleSheet, Origin, Stylesheet, UrlExtraData,
};
use stylo::stylist::Stylist;

use crate::Document;
use crate::tree::document::NodeId;
use crate::tree::shadow::ShadowRootData;

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

    fn parse_stylesheet(&self, css: &str, origin: Origin) -> DocumentStyleSheet {
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
        DocumentStyleSheet(Arc::new(sheet))
    }

    pub(crate) fn add_stylesheet(&mut self, css: &str, origin: Origin) {
        let sheet = self.parse_stylesheet(css, origin);
        let guard = self.lock.read();
        self.stylist.append_stylesheet(sheet, &guard);
        self.stylist.flush(&StylesheetGuards::same(&guard));
    }

    /// Appends one author stylesheet to a shadow root's scoped set and
    /// rebuilds that set's `CascadeData` — the rule data Stylo matches a
    /// shadow tree's elements against instead of the document's author rules.
    pub(crate) fn add_scoped_stylesheet(
        &mut self,
        styles: &mut AuthorStyles<DocumentStyleSheet>,
        css: &str,
    ) {
        let sheet = self.parse_stylesheet(css, Origin::Author);
        let guard = self.lock.read();
        // No device means the set skips its own invalidation bookkeeping (and
        // never reads the custom-media map): the caller dirties the host's
        // whole subtree instead, which is both simpler and a superset.
        styles
            .stylesheets
            .append_stylesheet(None, &CustomMediaMap::default(), sheet, &guard);
        drop(styles.flush(&mut self.stylist, &guard));
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

    /// Adds an author stylesheet scoped to one shadow tree.
    ///
    /// Its rules match only inside that tree (plus `:host`, `::slotted()`, and
    /// `::part()`, which reach exactly as far across the boundary as CSS
    /// Scoping says they do), and the document's own author rules do not
    /// match inside it. This is the styling half of shadow encapsulation, and
    /// the reason it is a separate entry point from [`Self::add_stylesheet`].
    pub fn add_shadow_stylesheet(&mut self, shadow_root: NodeId, css: &str) {
        let host = self
            .shadow_host(shadow_root)
            .expect("Document::add_shadow_stylesheet: not a live shadow root");
        self.note_visual_mutation();
        {
            let (engine, shadow) = self.shadow_style_parts(shadow_root);
            engine.add_scoped_stylesheet(&mut shadow.styles, css);
        }
        // Every element in the tree can gain or lose a rule; the flat-tree
        // subtree hint under the host is exactly that set.
        self.mark_subtree_dirty(host);
    }

    /// Lends the Stylist and one shadow root's state at once — rebuilding
    /// scoped `CascadeData` needs both, and they are disjoint fields.
    fn shadow_style_parts(
        &mut self,
        shadow_root: NodeId,
    ) -> (&mut StyleEngine, &mut ShadowRootData) {
        let (engine, tree) = self.style_and_tree_parts();
        let shadow = tree
            .get_mut(shadow_root)
            .expect("stale NodeId passed to a shadow-root method")
            .shadow_data_mut()
            .expect("Document shadow methods take a shadow root");
        (engine, shadow)
    }

    fn change_style_context(&mut self, change: impl FnOnce(&mut StyleEngine)) {
        self.note_visual_mutation();
        change(self.style_engine_mut());
        let root = self.document_element().id();
        self.mark_subtree_dirty(root);
    }
}
