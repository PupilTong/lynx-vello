//! The opaque view over a host implementation of the injected contracts.

mod support;

use std::rc::Rc;
use std::sync::Arc;

use bobcat_core::{
    DrawTarget, EngineError, FontBlob, LynxView, LynxViewError, NoWakeup, ViewSources,
};
use support::{FetcherDouble, wait_for_script};

const ENTRY: &str = "main.js";

async fn view(
    resources: impl FnOnce(bobcat_core::ImageReports) -> Rc<FetcherDouble>,
    sources: ViewSources,
) -> Result<LynxView<Rc<FetcherDouble>>, LynxViewError> {
    LynxView::new(
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

fn fetcher() -> impl FnOnce(bobcat_core::ImageReports) -> Rc<FetcherDouble> {
    |_sink| Rc::new(FetcherDouble::new(Vec::new()))
}

#[tokio::test]
async fn host_capabilities_compose_into_the_opaque_view() {
    let images = Rc::new(flashbulb::TestImages::new());
    images.insert_rgba8("app:///pixel.png", 1, 1, vec![0, 0, 0, 255]);

    let mut view = view(
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
        images.was_asked_for("app:///missing.png"),
        "a source with no pixels is still asked for exactly once"
    );
    assert!(
        images.was_asked_for("app:///pixel.png"),
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
        ..ViewSources::new(ENTRY)
    };
    assert!(matches!(
        view(fetcher(), unusable)
            .await
            .expect_err("no usable face registered"),
        LynxViewError::Engine(EngineError::UnknownFontFamily(_))
    ));
}
