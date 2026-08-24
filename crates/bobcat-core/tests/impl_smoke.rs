//! The opaque view over a host implementation of the injected contracts.

mod support;

use std::sync::Arc;

use bobcat_core::image::{AlphaType, DecodedImage, ImageFormat};
use bobcat_core::resource::ResourceFetcher;
use bobcat_core::{LynxView, NoWindow, PageConfig};
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
    let image = DecodedImage::from_rgba8(
        1,
        1,
        AlphaType::Straight,
        vec![0, 0, 0, 255],
        ImageFormat::Png,
    )
    .expect("decoded image");
    view.register_image_url("app:///pixel.png", &image)
        .expect("available document");
}
