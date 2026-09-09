//! The opaque view over a host implementation of the injected contracts.

mod support;

use std::rc::Rc;
use std::sync::Arc;

use bobcat_core::{
    DrawTarget, EngineError, FontBlob, LynxView, LynxViewError, NoWakeup, Painter, ViewSources,
};
use support::{FetcherDouble, solo_view, wait_for_script};

const ENTRY: &str = "main.js";

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
    let images = Rc::new(flashbulb::TestImages::new());
    images.insert_rgba8("app:///pixel.png", 1, 1, vec![0, 0, 0, 255]);

    let (mut view, painter) = view(
        |sink| {
            Rc::new(
                FetcherDouble::new(Vec::new())
                    .with_images(Rc::clone(&images))
                    .serving(sink),
            )
        },
        ViewSources::new(ENTRY),
    )
    .await
    .expect("opaque view");
    wait_for_script(&mut view).expect("the empty entry module boots");

    assert_eq!(painter.frame_size().width, 786);
    assert_eq!(painter.frame_size().height, 1454);

    // Warming is the only image call an embedder makes now: there is no
    // "load and tell me when", because the paint walk discovers sources by
    // itself. A source the store carries and one it does not are both
    // accepted — a missing image is a load failure the document records, not
    // an error the host has to handle.
    //
    // It applies immediately: the fetcher is this thread.
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

/// An unavailable default family fails startup through the event path,
/// including when the supplied font container carries no usable face.
///
/// And the turns after it ask the host for nothing. A failed view's document
/// will never commit again, so there is no frame for an image to be drawn in
/// and nothing for a completed load to be reported to — the same rule the
/// source requests are already held to.
#[tokio::test]
async fn a_default_family_nothing_provides_fails_startup() {
    let unusable = ViewSources {
        fonts: vec![FontBlob::from_static(b"not a font")],
        default_font_family: Some("Ahem".to_owned()),
        ..ViewSources::new(ENTRY)
    };
    let host = Rc::new(FetcherDouble::new(Vec::new()));
    let (mut view, _painter) = view(
        {
            let host = Rc::clone(&host);
            move |_sink| host
        },
        unusable,
    )
    .await
    .expect("loading view");
    assert!(matches!(
        wait_for_script(&mut view).expect_err("no usable face registered"),
        LynxViewError::Engine(EngineError::UnknownFontFamily(_))
    ));

    let serviced = host.image_service_count();
    let _ = view.pump();
    let _ = view.pump();
    assert_eq!(
        host.image_service_count(),
        serviced,
        "a failed view gives its host no image turn"
    );
    assert_eq!(
        host.image_request_count(),
        0,
        "and names no source against it"
    );
}
