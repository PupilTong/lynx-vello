//! The opaque view over a host implementation of the injected contracts.

mod support;

use std::sync::Arc;

use bobcat_core::{
    EngineError, FontBlob, ImageStore, LynxView, LynxViewError, NoWakeup, PageConfig, ViewSources,
};
use support::{FetcherDouble, wait_for_script};

const ENTRY: &str = "main.js";

async fn view(sources: ViewSources) -> Result<LynxView, LynxViewError> {
    LynxView::new(
        PageConfig::default(),
        Arc::new(NoWakeup),
        393.0,
        727.0,
        2.0,
        sources,
    )
    .await
}

fn sources() -> ViewSources {
    ViewSources::new(Arc::new(FetcherDouble::new(Vec::new())), ENTRY)
}

#[tokio::test]
async fn host_capabilities_compose_into_the_opaque_view() {
    let images = Arc::new(flashbulb::TestImages::new());
    images.insert_rgba8("app:///pixel.png", 1, 1, vec![0, 0, 0, 255]);

    let mut view = view(ViewSources {
        image_store: Some(Arc::clone(&images) as Arc<dyn ImageStore>),
        ..sources()
    })
    .await
    .expect("opaque view");
    // Settled first: only the repaint half of `load_image` needs the document,
    // and an open batch would refuse it.
    wait_for_script(&mut view).expect("the empty entry module boots");

    assert_eq!(view.frame_size().width, 786);
    assert_eq!(view.frame_size().height, 1454);

    view.prefetch_image("app:///pixel.png");
    view.load_image("app:///pixel.png")
        .await
        .expect("published source");
    view.load_image("app:///missing.png")
        .await
        .expect_err("a source the store does not carry cannot load");
}

/// A default family nothing provides fails the construction rather than being
/// silently ignored — including when the reason is a font container that
/// carried no usable face. The view that would have rendered in the wrong font
/// is never built.
#[tokio::test]
async fn a_default_family_nothing_provides_fails_construction() {
    let unusable = ViewSources {
        fonts: vec![FontBlob::from_static(b"not a font")],
        default_font_family: Some("Ahem".to_owned()),
        ..sources()
    };
    assert!(matches!(
        view(unusable).await.expect_err("no usable face registered"),
        LynxViewError::Engine(EngineError::UnknownFontFamily(_))
    ));
}
