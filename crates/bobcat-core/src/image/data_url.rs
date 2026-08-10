//! The `data:` branch.
//!
//! Handled in-crate rather than through the resource protocol. That is what
//! native Lynx does (`data_image_loader.cc` is a distinct, synchronous-capable
//! loader), it avoids taxing every embedder with base64 and mediatype parsing
//! just to serve bytes the caller already holds, and — because it needs no
//! transport — it is the one path that can produce a natural size before the
//! first layout rather than one frame later.
//!
//! Resolution still runs first: a host's rewrite hook is entitled to turn some
//! other specifier *into* a `data:` URL, and only the fetcher knows that.

use url::Url;

use crate::image::error::ImageError;

/// Whether this resolved URL should bypass the transport entirely.
#[must_use]
pub(crate) fn is_data_url(url: &Url) -> bool {
    url.scheme() == "data"
}

/// Decodes a `data:` URL's payload to raw bytes, refusing to buffer more than
/// `max_bytes`.
///
/// The mediatype is deliberately ignored: the container is identified by
/// sniffing the decoded bytes, which is both what browsers do and the only thing
/// that survives a mislabelled `data:image/png` carrying a JPEG.
///
/// The ceiling is enforced **during** decoding, through `data_url`'s streaming
/// callback, rather than by measuring the result. `decode_to_vec` would
/// materialise the whole body first, so a `data:` URL would be the one path that
/// walks straight past the loader's `max_encoded_bytes` budget — the same
/// ceiling every transport branch is held to. Bailing mid-stream means the peak
/// allocation is bounded by the limit rather than by whatever the caller was
/// handed.
///
/// # Errors
///
/// [`ImageError::MalformedDataUrl`] when the URL is not a well-formed `data:`
/// URL, its payload is empty, or its base64 does not decode;
/// [`ImageError::EncodedTooLarge`] when the decoded body exceeds `max_bytes`.
pub(crate) fn decode(url: &Url, max_bytes: u64) -> Result<Vec<u8>, ImageError> {
    let parsed = data_url::DataUrl::process(url.as_str())
        .map_err(|error| ImageError::MalformedDataUrl(format!("{error:?}").into()))?;

    let limit = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    // Grown by the callback below and never past `limit`, so peak allocation is
    // bounded by the budget rather than by the payload we were handed.
    let mut body = Vec::new();
    let outcome = parsed.decode::<_, TooLarge>(|chunk| {
        if body.len() + chunk.len() > limit {
            return Err(TooLarge);
        }
        body.extend_from_slice(chunk);
        Ok(())
    });

    match outcome {
        Ok(_fragment) => {}
        Err(data_url::forgiving_base64::DecodeError::WriteError(TooLarge)) => {
            return Err(ImageError::EncodedTooLarge { limit: max_bytes });
        }
        Err(error) => {
            return Err(ImageError::MalformedDataUrl(format!("{error:?}").into()));
        }
    }

    if body.is_empty() {
        return Err(ImageError::MalformedDataUrl("empty payload".into()));
    }
    Ok(body)
}

/// The sentinel the streaming callback aborts with once the budget is spent.
#[derive(Debug)]
struct TooLarge;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use url::Url;

    use super::{decode, is_data_url};
    use crate::image::error::ImageError;

    const LIMIT: u64 = 1 << 20;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("test URL")
    }

    #[test]
    fn recognises_the_data_scheme() {
        assert!(is_data_url(&url("data:image/png;base64,iVBORw0KGgo=")));
        assert!(!is_data_url(&url("https://example.com/a.png")));
        assert!(!is_data_url(&url("file:///tmp/a.png")));
    }

    #[test]
    fn decodes_base64_and_percent_encoded_payloads() {
        // "Hi" both ways.
        let base64 = decode(&url("data:text/plain;base64,SGk="), LIMIT).expect("base64 payload");
        assert_eq!(base64, b"Hi");
        let percent = decode(&url("data:text/plain,Hi"), LIMIT).expect("percent payload");
        assert_eq!(percent, b"Hi");
    }

    #[test]
    fn ignores_a_mislabelled_mediatype() {
        // Bytes win over the declared type: this is a PNG signature announced
        // as a JPEG, and sniffing the payload is what browsers do too.
        let bytes = decode(
            &url("data:image/jpeg;base64,iVBORw0KGgoAAAANSUhEUg=="),
            LIMIT,
        )
        .expect("payload decodes regardless of the label");
        assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G']);
        assert_eq!(
            crate::image::format::sniff(&bytes),
            Some(crate::image::format::ImageFormat::Png)
        );
    }

    #[test]
    fn refuses_a_payload_past_the_encoded_byte_budget() {
        // The budget has to bite on this path too: it is the one branch that
        // never touches a transport, so nothing else would enforce it.
        let sixteen_bytes = "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAAAAAA==";
        assert!(decode(&url(sixteen_bytes), LIMIT).is_ok());

        let error = decode(&url(sixteen_bytes), 4)
            .expect_err("16 decoded bytes must not pass a 4-byte budget");
        assert!(
            matches!(error, ImageError::EncodedTooLarge { limit: 4 }),
            "got {error:?}"
        );
    }

    #[test]
    fn a_payload_exactly_at_the_budget_is_accepted() {
        // Off-by-one guard: the limit is inclusive, so a body that exactly
        // fills the budget is legal.
        let four = decode(&url("data:text/plain,abcd"), 4).expect("exactly at the budget");
        assert_eq!(four, b"abcd");
        decode(&url("data:text/plain,abcde"), 4).expect_err("one byte over");
    }

    #[test]
    fn rejects_malformed_and_empty_payloads() {
        decode(&url("data:image/png;base64,!!!not-base64!!!"), LIMIT)
            .expect_err("invalid base64 must not reach a decoder");
        decode(&url("data:,"), LIMIT).expect_err("an empty payload is not an image");
    }
}
