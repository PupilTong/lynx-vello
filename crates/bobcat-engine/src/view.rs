//! Per-view ownership of resource and script runtime state.

use std::any::type_name;
use std::fmt;
use std::sync::Arc;

use crate::resource::ResourceFetcher;
use crate::script::ScriptEngine;

/// An independent Lynx runtime view.
pub struct LynxView<R: ResourceFetcher + ?Sized, E: ScriptEngine> {
    resource_fetcher: Arc<R>,
    script_engine: E,
}

/// The independently owned services recovered from a consumed Lynx view.
#[derive(Debug)]
pub struct LynxViewParts<R: ResourceFetcher + ?Sized, E: ScriptEngine> {
    pub resource_fetcher: Arc<R>,
    pub script_engine: E,
}

impl<R: ResourceFetcher, E: ScriptEngine> LynxView<R, E> {
    #[must_use]
    pub fn new(resource_fetcher: R, script_engine: E) -> Self {
        Self::from_shared_resource_fetcher(Arc::new(resource_fetcher), script_engine)
    }
}

impl<R: ResourceFetcher + ?Sized, E: ScriptEngine> LynxView<R, E> {
    #[must_use]
    pub const fn from_shared_resource_fetcher(resource_fetcher: Arc<R>, script_engine: E) -> Self {
        Self {
            resource_fetcher,
            script_engine,
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
    pub fn into_parts(self) -> LynxViewParts<R, E> {
        LynxViewParts {
            resource_fetcher: self.resource_fetcher,
            script_engine: self.script_engine,
        }
    }
}

impl<R: ResourceFetcher + ?Sized, E: ScriptEngine> fmt::Debug for LynxView<R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LynxView")
            .field("resource_fetcher", &type_name::<R>())
            .field("script_engine", &type_name::<E>())
            .finish()
    }
}
