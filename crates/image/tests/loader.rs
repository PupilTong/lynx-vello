//! The loader: transports, the `data:` short-circuit, caching and cancellation.

mod support;

use std::sync::Arc;

use bobcat_engine::resource::ResourceCapability;
use image::{
    BackendRegistry, ImageError, ImageLoader, ImagePrefetchTarget, LoaderConfig, PixelSize,
};
use support::{FetcherDouble, checker_png};
use tokio_util::sync::CancellationToken;

fn loader(double: Arc<FetcherDouble>) -> ImageLoader {
    ImageLoader::with_registry(
        double,
        LoaderConfig::default(),
        BackendRegistry::software_only(),
    )
    .expect("the double advertises a usable transport")
}

#[tokio::test]
async fn loads_and_decodes_through_the_buffered_transport() {
    let double = Arc::new(FetcherDouble::new(checker_png(4)));
    let loader = loader(Arc::clone(&double));

    let response = loader
        .load("icon.png", None, CancellationToken::new())
        .await
        .expect("load");

    assert_eq!((response.image.width(), response.image.height()), (4, 4));
    assert_eq!(double.resolve_count(), 1);
    assert_eq!(double.fetch_count(), 1);
}

#[tokio::test]
async fn every_advertised_transport_yields_the_same_bytes() {
    // The capability ladder is the only reason a host that cannot buffer still
    // works, so each rung has to actually carry an image.
    for capability in [
        ResourceCapability::BufferedResource,
        ResourceCapability::ResourceStream,
        ResourceCapability::ResourcePath,
    ] {
        let double =
            Arc::new(FetcherDouble::new(checker_png(4)).with_capabilities(vec![capability]));
        let response = loader(Arc::clone(&double))
            .load("icon.png", None, CancellationToken::new())
            .await
            .unwrap_or_else(|error| panic!("{capability:?}: {error}"));
        assert_eq!(
            (response.image.width(), response.image.height()),
            (4, 4),
            "{capability:?}"
        );
    }
}

#[tokio::test]
async fn a_fetcher_with_no_usable_transport_is_refused_at_construction() {
    // Only `resolve_locator` and `cancel_request` are mandatory in the
    // protocol, so this is a real configuration a host can present — and
    // failing once at construction beats failing per image.
    let double = Arc::new(
        FetcherDouble::new(Vec::new())
            .with_capabilities(vec![ResourceCapability::Http, ResourceCapability::Prefetch]),
    );
    let error = ImageLoader::with_registry(
        double,
        LoaderConfig::default(),
        BackendRegistry::software_only(),
    )
    .expect_err("no transport this crate can read bytes through");
    assert!(matches!(error, ImageError::NoTransport));
}

#[tokio::test]
async fn a_data_url_resolves_but_never_reaches_the_transport() {
    let png = checker_png(4);
    // A host rewrite that turns a specifier into a `data:` URL is exactly why
    // resolution still runs first.
    let encoded = base64(&png);
    let double = Arc::new(
        FetcherDouble::new(Vec::new()).resolving_to(&format!("data:image/png;base64,{encoded}")),
    );
    let loader = loader(Arc::clone(&double));

    let response = loader
        .load("inline", None, CancellationToken::new())
        .await
        .expect("data: URL decodes in-crate");

    assert_eq!((response.image.width(), response.image.height()), (4, 4));
    assert_eq!(double.resolve_count(), 1, "the rewrite hook still ran");
    assert_eq!(double.fetch_count(), 0, "the transport was bypassed");
}

#[tokio::test]
async fn a_cache_hit_avoids_a_second_fetch() {
    let double = Arc::new(FetcherDouble::new(checker_png(4)));
    let loader = loader(Arc::clone(&double));

    loader
        .load("icon.png", None, CancellationToken::new())
        .await
        .expect("cold load");
    let fetches_after_cold = double.fetch_count();

    // The sync probe is what a caller inside a frame commit uses.
    let cached = loader.cached("icon.png", None).expect("decode cache hit");
    assert_eq!((cached.width(), cached.height()), (4, 4));
    assert_eq!(double.fetch_count(), fetches_after_cold, "no refetch");

    // And a second `load` must consult the cache before the transport, not
    // merely populate it afterwards.
    let warm = loader
        .load("icon.png", None, CancellationToken::new())
        .await
        .expect("warm load");
    assert_eq!(warm.backend, "cache");
    assert_eq!(
        double.fetch_count(),
        fetches_after_cold,
        "a warm load must not reach the transport"
    );

    // And the natural size survives independently, which is what lets a second
    // mount lay out final on its first frame.
    let header = loader.cached_header("icon.png").expect("header cache hit");
    assert_eq!(
        header.natural_size,
        PixelSize {
            width: 4,
            height: 4
        }
    );
}

#[tokio::test]
async fn the_sync_probe_misses_until_the_specifier_has_been_resolved_once() {
    let double = Arc::new(FetcherDouble::new(checker_png(4)));
    let loader = loader(double);

    // Documented behaviour: the cache is keyed on the RESOLVED source, and
    // resolving is async, so a probe before the first load cannot hit.
    assert!(loader.cached("icon.png", None).is_none());
    assert!(loader.cached_header("icon.png").is_none());
}

#[tokio::test]
async fn a_host_supplied_cache_key_is_what_entries_are_keyed_on() {
    // Two specifiers the host resolves to one resource must share one entry —
    // that is the whole point of `ResolvedLocator::cache_key`.
    let double = Arc::new(FetcherDouble::new(checker_png(4)).with_cache_key("asset:42"));
    let loader = loader(Arc::clone(&double));

    loader
        .load("a.png", None, CancellationToken::new())
        .await
        .expect("first specifier");
    let after_first = double.fetch_count();

    loader
        .load("b.png", None, CancellationToken::new())
        .await
        .expect("second specifier");
    assert_eq!(
        double.fetch_count(),
        after_first,
        "the second specifier hit the first's cache entry"
    );
    assert!(loader.cached("b.png", None).is_some());
}

#[tokio::test]
async fn different_decode_targets_are_different_cache_entries() {
    let double = Arc::new(FetcherDouble::new(checker_png(16)));
    let loader = loader(Arc::clone(&double));
    let target = |side| {
        Some(PixelSize {
            width: side,
            height: side,
        })
    };

    loader
        .load("icon.png", target(4), CancellationToken::new())
        .await
        .expect("small");
    loader
        .load("icon.png", target(8), CancellationToken::new())
        .await
        .expect("large");

    assert_eq!(
        loader.cached("icon.png", target(4)).expect("small").width(),
        4
    );
    assert_eq!(
        loader.cached("icon.png", target(8)).expect("large").width(),
        8
    );
}

#[tokio::test]
async fn a_cancelled_load_reports_cancellation_rather_than_publishing_pixels() {
    let double = Arc::new(FetcherDouble::new(checker_png(4)));
    let loader = loader(Arc::clone(&double));

    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = loader
        .load("icon.png", None, cancel)
        .await
        .expect_err("a cancelled load must not succeed");
    assert!(matches!(error, ImageError::Cancelled), "got {error:?}");

    // Nothing was published for a torn-down node.
    assert!(loader.cached("icon.png", None).is_none());
}

#[tokio::test]
async fn a_header_load_probes_without_decoding_and_populates_the_header_cache() {
    let double = Arc::new(FetcherDouble::new(checker_png(16)));
    let loader = loader(Arc::clone(&double));

    let header = loader
        .header("icon.png", CancellationToken::new())
        .await
        .expect("header load");

    assert_eq!(
        header.natural_size,
        PixelSize {
            width: 16,
            height: 16
        }
    );
    assert!(loader.cached_header("icon.png").is_some());
    // The pixels were never decoded, so the decode cache stays empty.
    assert!(loader.cached("icon.png", None).is_none());
}

#[tokio::test]
async fn clearing_the_caches_drops_both_pixels_and_natural_sizes() {
    let double = Arc::new(FetcherDouble::new(checker_png(4)));
    let loader = loader(double);
    loader
        .load("icon.png", None, CancellationToken::new())
        .await
        .expect("load");

    loader.clear_caches();
    assert!(loader.cached("icon.png", None).is_none());
    assert!(loader.cached_header("icon.png").is_none());
}

#[tokio::test]
async fn prefetch_warms_the_decode_cache_or_delegates_to_the_host() {
    let double = Arc::new(FetcherDouble::new(checker_png(4)));
    let decoding = loader(Arc::clone(&double));

    decoding
        .prefetch("icon.png", ImagePrefetchTarget::Decoded { target: None })
        .await
        .expect("decoded prefetch");
    assert!(
        decoding.cached("icon.png", None).is_some(),
        "a decoded prefetch is exactly a load whose result you keep"
    );

    let prefetching = Arc::new(FetcherDouble::new(checker_png(4)).with_capabilities(vec![
        ResourceCapability::BufferedResource,
        ResourceCapability::Prefetch,
    ]));
    let delegating = loader(Arc::clone(&prefetching));
    delegating
        .prefetch(
            "icon.png",
            ImagePrefetchTarget::Encoded(bobcat_engine::resource::CacheTarget::Disk),
        )
        .await
        .expect("encoded prefetch");
    assert_eq!(
        prefetching
            .prefetches
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "encoded bytes are the fetcher's cache, not ours"
    );
}

#[tokio::test]
async fn undecodable_bytes_surface_as_a_typed_error() {
    let double = Arc::new(FetcherDouble::new(b"not an image at all".to_vec()));
    let error = loader(double)
        .load("junk.bin", None, CancellationToken::new())
        .await
        .expect_err("garbage must not decode");
    assert!(matches!(error, ImageError::UnknownFormat), "got {error:?}");
}

/// Minimal base64, so the test does not need an encoder dependency.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
