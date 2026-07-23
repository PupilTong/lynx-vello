use std::error::Error;
use std::sync::Arc;

use bobcat_engine::resource::ResourceFetcher;
use bobcat_quickjs::{QuickJsInitializationError, QuickJsLynxView, new_quickjs_view};

#[allow(dead_code)]
fn public_view_contract<R: ResourceFetcher>(view: &mut QuickJsLynxView<R>) {
    let _: fn(R) -> Result<QuickJsLynxView<R>, QuickJsInitializationError> = new_quickjs_view::<R>;
    let _: &R = view.resource_fetcher();
    let _: &Arc<R> = view.shared_resource_fetcher();
}

#[test]
fn expected_public_error_contract_is_available() {
    fn assert_error<T: Error + Send + Sync + 'static>() {}

    assert_error::<QuickJsInitializationError>();
}
