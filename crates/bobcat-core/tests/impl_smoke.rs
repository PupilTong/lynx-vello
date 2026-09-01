//! The opaque view over a host implementation of the injected contracts.

mod support;

use std::sync::Arc;

use bobcat_core::{
    DrawTarget, EngineError, FontBlob, LynxView, LynxViewError, NoWakeup, PageConfig, ViewSources,
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
        DrawTarget::Offscreen,
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

    let store = Arc::clone(&images);
    let mut view = view(ViewSources {
        image_store: Some(Box::new(move |sink| {
            store.attach(sink);
            flashbulb::shared(&store)
        })),
        ..sources()
    })
    .await
    .expect("opaque view");
    wait_for_script(&mut view).expect("the empty entry module boots");

    assert_eq!(view.frame_size().width, 786);
    assert_eq!(view.frame_size().height, 1454);

    // Warming is the only image call an embedder makes now: there is no
    // "load and tell me when", because the paint walk discovers sources by
    // itself. A source the store carries and one it does not are both
    // accepted — a missing image is a load failure the document records, not
    // an error the host has to handle.
    //
    // It applies immediately: the painter is this thread.
    view.prefetch_images(["app:///pixel.png", "app:///missing.png"]);
    assert!(
        images.id_of("app:///missing.png").is_some(),
        "a source with no pixels is still named, so it is asked for once"
    );
    assert!(
        images.id_of("app:///pixel.png").is_some(),
        "and so is one the store carries"
    );
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
