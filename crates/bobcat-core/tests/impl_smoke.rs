//! The opaque view over a host implementation of the injected contracts.

mod support;

use std::sync::Arc;

use bobcat_core::resource::ResourceFetcher;
use bobcat_core::{ImageStore, LynxView, NoWindow, PageConfig};
use support::FetcherDouble;

#[test]
fn host_capabilities_compose_into_the_opaque_view() {
    let resources: Arc<dyn ResourceFetcher> = Arc::new(FetcherDouble::new(Vec::new()));
    let mut view = LynxView::<NoWindow>::new(
        PageConfig::default(),
        resources,
        Arc::new(|| {}),
        393.0,
        727.0,
        2.0,
    )
    .expect("opaque view");

    assert_eq!(view.frame_size().width, 786);
    assert_eq!(view.frame_size().height, 1454);

    assert_eq!(
        view.register_fonts(Vec::from(b"not a font"))
            .expect("available document"),
        0
    );
    assert!(
        !view
            .set_default_font_family("missing")
            .expect("available document")
    );
    let images = Arc::new(flashbulb::TestImages::new());
    images.insert_rgba8("app:///pixel.png", 1, 1, vec![0, 0, 0, 255]);
    view.set_image_store(Arc::clone(&images) as Arc<dyn ImageStore>)
        .expect("available document");
    view.prefetch_image("app:///pixel.png")
        .expect("available document");
    pollster::block_on(view.load_image("app:///pixel.png")).expect("published source");
    pollster::block_on(view.load_image("app:///missing.png"))
        .expect_err("a source the store does not carry cannot load");
}
