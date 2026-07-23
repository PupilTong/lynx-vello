//! Per-view ownership of Lynx widget and script runtime state.

use std::any::type_name;
use std::fmt;
use std::sync::Arc;

use lynx_widget::StyleEngine;
pub use lynx_widget::{PageConfig, ViewMetrics, WidgetTree};

use crate::resource::ResourceFetcher;
use crate::script::ScriptEngine;

/// One view-local instance of the `lynx-widget` Element PAPI and style engine.
#[derive(Debug)]
pub struct LynxWidgetApi {
    style_engine: StyleEngine,
    tree: WidgetTree,
}

impl LynxWidgetApi {
    #[must_use]
    pub fn new(metrics: ViewMetrics) -> Self {
        Self::with_page_config(metrics, PageConfig::default())
    }

    #[must_use]
    pub fn with_page_config(metrics: ViewMetrics, page_config: PageConfig) -> Self {
        let style_engine = StyleEngine::with_page_config(metrics, page_config);
        let tree = style_engine.new_tree();
        Self { style_engine, tree }
    }

    #[must_use]
    pub const fn tree(&self) -> &WidgetTree {
        &self.tree
    }

    pub const fn tree_mut(&mut self) -> &mut WidgetTree {
        &mut self.tree
    }

    pub fn flush_styles(&mut self) {
        self.style_engine.flush_styles(&mut self.tree);
    }

    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.style_engine
            .set_viewport(&mut self.tree, width, height);
    }

    pub fn set_device_pixel_ratio(&mut self, device_pixel_ratio: f32) {
        self.style_engine
            .set_device_pixel_ratio(&mut self.tree, device_pixel_ratio);
    }
}

/// An independent Lynx runtime view.
pub struct LynxView<R: ResourceFetcher + ?Sized, E: ScriptEngine> {
    resource_fetcher: Arc<R>,
    script_engine: E,
    widget_api: LynxWidgetApi,
}

/// The independently owned services recovered from a consumed Lynx view.
#[derive(Debug)]
pub struct LynxViewParts<R: ResourceFetcher + ?Sized, E: ScriptEngine> {
    pub resource_fetcher: Arc<R>,
    pub script_engine: E,
    pub widget_api: LynxWidgetApi,
}

impl<R: ResourceFetcher, E: ScriptEngine> LynxView<R, E> {
    #[must_use]
    pub fn new(resource_fetcher: R, script_engine: E, metrics: ViewMetrics) -> Self {
        Self::from_shared_resource_fetcher(Arc::new(resource_fetcher), script_engine, metrics)
    }

    #[must_use]
    pub fn with_page_config(
        resource_fetcher: R,
        script_engine: E,
        metrics: ViewMetrics,
        page_config: PageConfig,
    ) -> Self {
        Self::from_shared_resource_fetcher_with_page_config(
            Arc::new(resource_fetcher),
            script_engine,
            metrics,
            page_config,
        )
    }
}

impl<R: ResourceFetcher + ?Sized, E: ScriptEngine> LynxView<R, E> {
    #[must_use]
    pub fn from_shared_resource_fetcher(
        resource_fetcher: Arc<R>,
        script_engine: E,
        metrics: ViewMetrics,
    ) -> Self {
        Self::from_parts(resource_fetcher, script_engine, LynxWidgetApi::new(metrics))
    }

    #[must_use]
    pub fn from_shared_resource_fetcher_with_page_config(
        resource_fetcher: Arc<R>,
        script_engine: E,
        metrics: ViewMetrics,
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

    #[must_use]
    pub fn resource_fetcher(&self) -> &R {
        self.resource_fetcher.as_ref()
    }

    #[must_use]
    pub const fn shared_resource_fetcher(&self) -> &Arc<R> {
        &self.resource_fetcher
    }

    #[must_use]
    pub const fn script_engine(&self) -> &E {
        &self.script_engine
    }

    pub const fn script_engine_mut(&mut self) -> &mut E {
        &mut self.script_engine
    }

    #[must_use]
    pub const fn widget_api(&self) -> &LynxWidgetApi {
        &self.widget_api
    }

    pub const fn widget_api_mut(&mut self) -> &mut LynxWidgetApi {
        &mut self.widget_api
    }

    #[must_use]
    pub fn into_parts(self) -> LynxViewParts<R, E> {
        LynxViewParts {
            resource_fetcher: self.resource_fetcher,
            script_engine: self.script_engine,
            widget_api: self.widget_api,
        }
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
    use lynx_widget::ViewMetrics;

    use super::LynxWidgetApi;

    #[test]
    fn widget_api_instances_have_independent_trees() {
        let metrics = ViewMetrics::new(390.0, 844.0, 3.0);
        let mut first = LynxWidgetApi::new(metrics);
        let mut second = LynxWidgetApi::new(metrics);

        let first_page = first.tree_mut().create_page();
        let second_page = second.tree_mut().create_page();
        let first_child = first.tree_mut().create_view();
        let second_child = second.tree_mut().create_view();

        assert_eq!(first.tree().unique_id(&first_page).unwrap(), 1);
        assert_eq!(second.tree().unique_id(&second_page).unwrap(), 1);
        assert_eq!(first.tree().unique_id(&first_child).unwrap(), 2);
        assert_eq!(second.tree().unique_id(&second_child).unwrap(), 2);
        assert_ne!(first_child, second_child);
        assert_eq!(
            first.tree().widget_by_unique_id(2),
            Some(first_child.clone())
        );
        assert_eq!(
            second.tree().widget_by_unique_id(2),
            Some(second_child.clone())
        );
    }
}
