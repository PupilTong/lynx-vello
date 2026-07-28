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

use crate::error::ImageError;

/// Whether this resolved URL should bypass the transport entirely.
#[must_use]
pub(crate) fn is_data_url(url: &Url) -> bool {
    url.scheme() == "data"
}

/// Decodes a `data:` URL's payload to raw bytes.
///
/// The mediatype is deliberately ignored: the container is identified by
/// sniffing the decoded bytes, which is both what browsers do and the only thing
/// that survives a mislabelled `data:image/png` carrying a JPEG.
///
/// # Errors
///
/// [`ImageError::MalformedDataUrl`] when the URL is not a well-formed `data:`
/// URL or its base64 payload does not decode.
pub(crate) fn decode(url: &Url) -> Result<Vec<u8>, ImageError> {
    let parsed = data_url::DataUrl::process(url.as_str())
        .map_err(|error| ImageError::MalformedDataUrl(format!("{error:?}").into()))?;
    let (body, _fragment) = parsed
        .decode_to_vec()
        .map_err(|error| ImageError::MalformedDataUrl(format!("{error:?}").into()))?;
    if body.is_empty() {
        return Err(ImageError::MalformedDataUrl("empty payload".into()));
    }
    Ok(body)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use url::Url;

    use super::{decode, is_data_url};

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
        let base64 = decode(&url("data:text/plain;base64,SGk=")).expect("base64 payload");
        assert_eq!(base64, b"Hi");
        let percent = decode(&url("data:text/plain,Hi")).expect("percent payload");
        assert_eq!(percent, b"Hi");
    }

    #[test]
    fn ignores_a_mislabelled_mediatype() {
        // Bytes win over the declared type: this is a PNG signature announced
        // as a JPEG, and sniffing the payload is what browsers do too.
        let bytes = decode(&url("data:image/jpeg;base64,iVBORw0KGgoAAAANSUhEUg=="))
            .expect("payload decodes regardless of the label");
        assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G']);
        assert_eq!(
            crate::format::sniff(&bytes),
            Some(crate::format::ImageFormat::Png)
        );
    }

    #[test]
    fn rejects_malformed_and_empty_payloads() {
        decode(&url("data:image/png;base64,!!!not-base64!!!"))
            .expect_err("invalid base64 must not reach a decoder");
        decode(&url("data:,")).expect_err("an empty payload is not an image");
    }
}
