//! Per-view ownership of Lynx widget and script runtime state.
//!
//! Every [`LynxView`] owns one primary [`ScriptEngine`] realm instance and one
//! coupled [`LynxWidgetApi`] instance. Creating another view therefore creates a new
//! widget arena, style context, global object and module graph. The external
//! [`ResourceFetcher`] may be shared because request identity and runtime
//! state remain view-owned at higher layers.
//!
//! This is the initial per-view ownership shell requested by the generic
//! `LynxView<R, E>` shape. It does not yet model Lynx's complete MTS/BTS
//! dual-realm scheduler or cross-realm message protocol.

use std::any::type_name;
use std::fmt;
use std::sync::Arc;

pub use lynx_widget::{EngineMetrics, PageConfig, WidgetTree};

use crate::resource::ResourceFetcher;
use crate::script::ScriptEngine;

/// One view-local `lynx-widget` document and Element PAPI instance.
#[derive(Debug)]
pub struct LynxWidgetApi {
    tree: WidgetTree,
}

impl LynxWidgetApi {
    /// Create an empty view-local widget API with the default page config.
    #[must_use]
    pub fn new(metrics: EngineMetrics) -> Self {
        Self::with_page_config(metrics, PageConfig::default())
    }

    /// Create an empty view-local widget API with explicit page defaults.
    #[must_use]
    pub fn with_page_config(metrics: EngineMetrics, page_config: PageConfig) -> Self {
        Self {
            tree: WidgetTree::with_page_config(metrics, page_config),
        }
    }

    /// Borrow this view's Element-PAPI tree.
    #[must_use]
    pub const fn tree(&self) -> &WidgetTree {
        &self.tree
    }

    /// Mutably borrow this view's Element-PAPI tree.
    pub const fn tree_mut(&mut self) -> &mut WidgetTree {
        &mut self.tree
    }

    /// Flush pending style work in this view's own tree.
    pub fn flush_styles(&mut self) {
        self.tree.flush_styles();
    }

    /// Update this view's viewport and schedule its tree for restyling.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.tree.set_viewport(width, height);
    }

    /// Update this view's device-pixel ratio and schedule its tree for
    /// restyling.
    pub fn set_device_pixel_ratio(&mut self, device_pixel_ratio: f32) {
        self.tree.set_device_pixel_ratio(device_pixel_ratio);
    }
}

/// An independent Lynx runtime view.
///
/// `E` is consumed by the constructor and is never cloned, so one primary
/// engine/realm instance has one view owner. Each constructor also creates a fresh
/// [`LynxWidgetApi`]. A host can share its stateless or internally synchronized
/// resource service between views through [`Self::with_shared_resource_fetcher`]
/// without sharing script or widget state.
pub struct LynxView<R: ResourceFetcher + ?Sized, E: ScriptEngine> {
    resource_fetcher: Arc<R>,
    script_engine: E,
    widget_api: LynxWidgetApi,
}

impl<R: ResourceFetcher, E: ScriptEngine> LynxView<R, E> {
    /// Create a view that takes ownership of a resource fetcher and script
    /// engine instance.
    #[must_use]
    pub fn new(resource_fetcher: R, script_engine: E, metrics: EngineMetrics) -> Self {
        Self::with_shared_resource_fetcher(Arc::new(resource_fetcher), script_engine, metrics)
    }

    /// Create a view with explicit Lynx page defaults.
    #[must_use]
    pub fn with_page_config(
        resource_fetcher: R,
        script_engine: E,
        metrics: EngineMetrics,
        page_config: PageConfig,
    ) -> Self {
        Self::with_shared_resource_fetcher_and_page_config(
            Arc::new(resource_fetcher),
            script_engine,
            metrics,
            page_config,
        )
    }
}

impl<R: ResourceFetcher + ?Sized, E: ScriptEngine> LynxView<R, E> {
    /// Create a view using a resource fetcher shared with other views.
    ///
    /// Only `R` is shared. The script engine is consumed and the widget API is
    /// freshly allocated for this view. Callers sharing a fetcher must assign
    /// each view a distinct [`crate::resource::RequestId::namespace`]; the
    /// view does not allocate or coordinate request IDs.
    #[must_use]
    pub fn with_shared_resource_fetcher(
        resource_fetcher: Arc<R>,
        script_engine: E,
        metrics: EngineMetrics,
    ) -> Self {
        Self::from_parts(resource_fetcher, script_engine, LynxWidgetApi::new(metrics))
    }

    /// Create a view using a shared fetcher and explicit page defaults.
    ///
    /// As with [`Self::with_shared_resource_fetcher`], callers own request-ID
    /// namespace allocation across all views sharing this fetcher.
    #[must_use]
    pub fn with_shared_resource_fetcher_and_page_config(
        resource_fetcher: Arc<R>,
        script_engine: E,
        metrics: EngineMetrics,
        page_config: PageConfig,
    ) -> Self {
        Self::from_parts(
            resource_fetcher,
            script_engine,
            LynxWidgetApi::with_page_config(metrics, page_config),
        )
    }

    fn from_parts(resource_fetcher: Arc<R>, script_engine: E, widget_api: LynxWidgetApi) -> Self {
        Self {
            resource_fetcher,
            script_engine,
            widget_api,
        }
    }

    /// Borrow the host resource fetcher.
    #[must_use]
    pub fn resource_fetcher(&self) -> &R {
        self.resource_fetcher.as_ref()
    }

    /// Cloneable shared ownership of the host resource fetcher.
    #[must_use]
    pub const fn shared_resource_fetcher(&self) -> &Arc<R> {
        &self.resource_fetcher
    }

    /// Borrow this view's script-engine instance.
    #[must_use]
    pub const fn script_engine(&self) -> &E {
        &self.script_engine
    }

    /// Mutably borrow this view's script-engine instance.
    pub const fn script_engine_mut(&mut self) -> &mut E {
        &mut self.script_engine
    }

    /// Borrow this view's widget API instance.
    #[must_use]
    pub const fn widget_api(&self) -> &LynxWidgetApi {
        &self.widget_api
    }

    /// Mutably borrow this view's widget API instance.
    pub const fn widget_api_mut(&mut self) -> &mut LynxWidgetApi {
        &mut self.widget_api
    }

    /// Borrow the script and widget instances together for embedder-specific
    /// adapter code.
    ///
    /// This avoids storing a self-referential pointer from the engine into the
    /// widget tree. The base [`ScriptEngine`] protocol deliberately does not
    /// prescribe how a concrete engine installs Lynx host globals.
    pub const fn execution_parts_mut(&mut self) -> (&mut E, &mut LynxWidgetApi) {
        (&mut self.script_engine, &mut self.widget_api)
    }

    /// Consume the view and return its independently owned parts.
    #[must_use]
    pub fn into_parts(self) -> (Arc<R>, E, LynxWidgetApi) {
        (self.resource_fetcher, self.script_engine, self.widget_api)
    }
}

impl<R: ResourceFetcher + ?Sized, E: ScriptEngine> fmt::Debug for LynxView<R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LynxView")
            .field("resource_fetcher", &type_name::<R>())
            .field("script_engine", &type_name::<E>())
            .field("widget_api", &self.widget_api)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use lynx_widget::EngineMetrics;

    use super::LynxWidgetApi;

    #[test]
    fn widget_api_instances_have_independent_trees() {
        let metrics = EngineMetrics::new(390.0, 844.0, 3.0);
        let mut first = LynxWidgetApi::new(metrics);
        let mut second = LynxWidgetApi::new(metrics);

        let first_page = first.tree_mut().create_page();
        let second_page = second.tree_mut().create_page();
        let first_child = first.tree_mut().create_view();

        assert_eq!(first.tree().get_element_unique_id(&first_page), Some(1));
        assert_eq!(second.tree().get_element_unique_id(&second_page), Some(1));
        assert_eq!(first.tree().get_element_unique_id(&first_child), Some(2));
        assert_eq!(second.tree().element_by_unique_id(2), None);
    }
}
