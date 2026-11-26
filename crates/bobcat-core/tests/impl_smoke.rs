//! The opaque view over a host implementation of the injected contracts.

mod support;

use std::rc::Rc;
use std::sync::Arc;

use bobcat_core::{
    DrawTarget, EngineError, FontBlob, LynxView, LynxViewError, NoWakeup, Painter, ViewSources,
};
use support::{FetcherDouble, solo_view, wait_for_script};

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const ENTRY: &str = "app:///main.js";

async fn view(
    resources: impl FnOnce(bobcat_core::ImageReports) -> Rc<FetcherDouble>,
    sources: ViewSources,
) -> Result<(LynxView<Rc<FetcherDouble>>, Painter), LynxViewError> {
    solo_view(
        Arc::new(NoWakeup),
        393.0,
        727.0,
        2.0,
        DrawTarget::Offscreen,
        resources,
        sources,
    )
    .await
}

#[tokio::test]
async fn host_capabilities_compose_into_the_opaque_view() {
    let images = Rc::new(prime_radiant::TestImages::new());
    images.insert_rgba8("app:///pixel.png", 1, 1, vec![0, 0, 0, 255]);

    let (mut view, painter) = view(
        |sink| {
            Rc::new(
                FetcherDouble::new(Vec::new())
                    .with_images(Rc::clone(&images))
                    .serving(sink),
            )
        },
        ViewSources::new("app:///", ENTRY, SCREEN, "en-US"),
    )
    .await
    .expect("opaque view");
    wait_for_script(&mut view).expect("the empty entry module boots");

    assert_eq!(painter.frame_size().width, 786);
    assert_eq!(painter.frame_size().height, 1454);

    view.prefetch_images(["app:///pixel.png", "app:///missing.png"]);
    assert!(
        images.was_asked_for("app:///missing.png"),
        "a source with no pixels is still asked for exactly once"
    );
    assert!(
        images.was_asked_for("app:///pixel.png"),
        "and so is one the store carries"
    );
}

#[tokio::test]
async fn a_default_family_nothing_provides_fails_construction() {
    let unusable = ViewSources {
        fonts: vec![FontBlob::from_static(b"not a font")],
        default_font_family: Some("Ahem".to_owned()),
        ..ViewSources::new("app:///", ENTRY, SCREEN, "en-US")
    };
    let host = Rc::new(FetcherDouble::new(Vec::new()));
    let error = view(
        {
            let host = Rc::clone(&host);
            move |_sink| host
        },
        unusable,
    )
    .await
    .expect_err("no usable face registered");
    assert!(matches!(
        error,
        LynxViewError::Engine(EngineError::UnknownFontFamily(_))
    ));
    assert_eq!(
        host.image_request_count(),
        0,
        "a view that was never built names no source against its host"
    );
    assert_eq!(host.image_service_count(), 0, "and takes no image turn");
    assert_eq!(host.resolve_count(), 0, "nor asks it for a source");
}
