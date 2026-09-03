//! The MIME-keyed preprocessing pipeline: what happens to fetched bytes
//! between the transport and the engine.
//!
//! Every resource goes through the same three steps. Its media type is
//! settled by [`mime::sniff`], from the label it came with, the extension of
//! its URL, or its own bytes. That type classifies it. The class then picks
//! the treatment: text is transcoded to UTF-8 with its BOM removed, so the
//! engine's strict UTF-8 validation of a script or stylesheet only ever sees
//! what a browser's decoder would have produced; JSON is text that must also
//! parse; an image is container-sniffed and its header probed for the size
//! layout wants, while its pixels stay encoded for the platform decoder;
//! everything else passes through untouched.

use std::borrow::Cow;
use std::sync::Arc;

use bytes::Bytes;
use url::Url;

use crate::image_header::{self, ImageHeader};
use crate::mime::{self, ImageFormat, MediaType, ResourceClass};

/// The typed result of preprocessing one resource.
#[derive(Clone, Debug)]
pub enum Payload {
    /// UTF-8 text, transcoded from the declared charset with any BOM removed.
    Text(Arc<str>),
    /// Syntactically valid JSON text.
    Json(Arc<str>),
    /// An image container: its format and, when the header could be read,
    /// its intrinsic dimensions. The bytes stay encoded — decoding them is
    /// the platform decoder's job, later and elsewhere.
    Image {
        format: ImageFormat,
        header: Option<ImageHeader>,
    },
    /// Anything else, untouched.
    Binary,
}

/// One preprocessed resource.
#[derive(Clone, Debug)]
pub struct Preprocessed {
    /// The bytes downstream consumers receive: for text and JSON the UTF-8
    /// re-encoding, which is the input itself when that already was UTF-8
    /// without a BOM; otherwise the original bytes.
    pub bytes: Bytes,
    /// The effective media type after sniffing, carrying `charset=utf-8` for
    /// text and JSON output.
    pub media_type: MediaType,
    pub payload: Payload,
}

#[derive(Debug, thiserror::Error)]
pub enum PreprocessError {
    #[error("unsupported charset `{0}`")]
    UnsupportedCharset(String),
    #[error("invalid JSON: {0}")]
    Json(String),
}

/// Runs the pipeline over `bytes`, labelled `declared` by whoever produced
/// them, located at `url` when a URL exists to read an extension from.
pub fn preprocess(
    bytes: Bytes,
    declared: Option<&MediaType>,
    url: Option<&Url>,
) -> Result<Preprocessed, PreprocessError> {
    let from_extension =
        url.and_then(|url| mime::from_extension(url.path()).filter(|_| declared.is_none()));
    let media_type = mime::sniff(&bytes, declared.or(from_extension.as_ref()));
    match media_type.class() {
        ResourceClass::Text => {
            let (text, bytes) = decode_text(bytes, media_type.charset())?;
            Ok(Preprocessed {
                bytes,
                media_type: media_type.with_parameter("charset", "utf-8"),
                payload: Payload::Text(text),
            })
        }
        ResourceClass::Json => {
            let (text, bytes) = decode_text(bytes, media_type.charset())?;
            serde_json::from_str::<serde_json::Value>(&text)
                .map_err(|error| PreprocessError::Json(error.to_string()))?;
            Ok(Preprocessed {
                bytes,
                media_type: media_type.with_parameter("charset", "utf-8"),
                payload: Payload::Json(text),
            })
        }
        ResourceClass::Image(format) => {
            let header = image_header::probe(&bytes);
            Ok(Preprocessed {
                bytes,
                media_type,
                payload: Payload::Image {
                    format: header.map_or(format, |header| header.format),
                    header,
                },
            })
        }
        ResourceClass::Binary => Ok(Preprocessed {
            bytes,
            media_type,
            payload: Payload::Binary,
        }),
    }
}

/// Decodes `bytes` under `charset` to UTF-8, returning the text and the
/// bytes to hand on — the same allocation when nothing changed.
fn decode_text(bytes: Bytes, charset: Option<&str>) -> Result<(Arc<str>, Bytes), PreprocessError> {
    let decoded = decode_charset(&bytes, charset)?;
    let text: Arc<str> = Arc::from(&*decoded);
    let bytes = match decoded {
        // Borrowed from the input, but not necessarily all of it: a stripped
        // BOM leaves a borrow that starts three bytes in.
        Cow::Borrowed(borrowed) if borrowed.len() == bytes.len() => bytes,
        Cow::Borrowed(borrowed) => Bytes::copy_from_slice(borrowed.as_bytes()),
        Cow::Owned(owned) => Bytes::from(owned.into_bytes()),
    };
    Ok((text, bytes))
}

/// The encoding-standard decode, narrowed to the charsets a card is realistic
/// to ship in: a byte-order mark outranks the label, invalid sequences
/// become U+FFFD as in a browser, and the BOM itself never reaches the text.
pub fn decode_charset<'a>(
    bytes: &'a [u8],
    charset: Option<&str>,
) -> Result<Cow<'a, str>, PreprocessError> {
    let label = mime::bom_charset(bytes)
        .map(str::to_owned)
        .or_else(|| charset.map(|label| label.trim().to_ascii_lowercase()))
        .unwrap_or_else(|| "utf-8".to_owned());
    match label.as_str() {
        "utf-8" | "utf8" | "unicode-1-1-utf-8" => {
            let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
            Ok(String::from_utf8_lossy(body))
        }
        "us-ascii" | "ascii" | "ansi_x3.4-1968" | "iso-8859-1" | "iso8859-1" | "iso_8859-1"
        | "latin1" | "l1" | "windows-1252" | "cp1252" | "x-cp1252" => Ok(Cow::Owned(
            bytes.iter().map(|byte| windows_1252(*byte)).collect(),
        )),
        "utf-16" | "utf-16le" => {
            let body = bytes.strip_prefix(b"\xFF\xFE").unwrap_or(bytes);
            Ok(Cow::Owned(decode_utf16(body, u16::from_le_bytes)))
        }
        "utf-16be" => {
            let body = bytes.strip_prefix(b"\xFE\xFF").unwrap_or(bytes);
            Ok(Cow::Owned(decode_utf16(body, u16::from_be_bytes)))
        }
        other => Err(PreprocessError::UnsupportedCharset(other.to_owned())),
    }
}

/// The encoding standard's `windows-1252` index, which is also what `latin1`
/// and `us-ascii` labels resolve to there.
fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}',
        '\u{017D}', '\u{008F}', '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
    ];
    match byte {
        0x80..=0x9F => HIGH[usize::from(byte - 0x80)],
        other => char::from(other),
    }
}

fn decode_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units = bytes.chunks_exact(2).map(|pair| unit([pair[0], pair[1]]));
    let mut text: String = char::decode_utf16(units)
        .map(|result| result.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    if bytes.len() % 2 == 1 {
        text.push(char::REPLACEMENT_CHARACTER);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media_type(value: &str) -> MediaType {
        MediaType::parse(value).expect("a media type")
    }

    #[test]
    fn utf8_text_passes_through_the_same_allocation() {
        let bytes = Bytes::from_static(b"body { color: red }");
        let result =
            preprocess(bytes.clone(), Some(&media_type("text/css")), None).expect("preprocess");
        assert!(matches!(&result.payload, Payload::Text(text) if &**text == "body { color: red }"));
        assert_eq!(result.media_type.to_string(), "text/css; charset=utf-8");
        assert_eq!(result.bytes.as_ptr(), bytes.as_ptr(), "no copy was made");
    }

    #[test]
    fn a_bom_is_removed_and_outranks_the_label() {
        let result = preprocess(
            Bytes::from_static(b"\xEF\xBB\xBFlet x = 1;"),
            Some(&media_type("text/javascript; charset=latin1")),
            None,
        )
        .expect("preprocess");
        assert_eq!(&result.bytes[..], b"let x = 1;");
        let result = preprocess(
            Bytes::from_static(b"\xFF\xFEh\x00i\x00"),
            Some(&media_type("text/plain")),
            None,
        )
        .expect("preprocess");
        assert_eq!(&result.bytes[..], b"hi");
    }

    #[test]
    fn legacy_charsets_are_transcoded_and_unknown_ones_refused() {
        let result = preprocess(
            Bytes::from_static(b"caf\xE9 \x80"),
            Some(&media_type("text/plain; charset=ISO-8859-1")),
            None,
        )
        .expect("preprocess");
        assert!(matches!(&result.payload, Payload::Text(text) if &**text == "café €"));
        assert_eq!(result.media_type.charset(), Some("utf-8"));

        let result = preprocess(
            Bytes::from_static(b"\x00h\x00i\x00"),
            Some(&media_type("text/plain; charset=utf-16be")),
            None,
        )
        .expect("preprocess");
        assert!(
            matches!(&result.payload, Payload::Text(text) if &**text == "hi\u{FFFD}"),
            "a trailing odd byte is a replacement character"
        );

        let error = preprocess(
            Bytes::from_static(b"x"),
            Some(&media_type("text/plain; charset=shift_jis")),
            None,
        )
        .expect_err("unsupported");
        assert!(
            matches!(error, PreprocessError::UnsupportedCharset(label) if label == "shift_jis")
        );
    }

    #[test]
    fn invalid_utf8_decodes_with_replacement_like_a_browser() {
        let result = preprocess(
            Bytes::from_static(b"a\xFFb"),
            Some(&media_type("text/javascript")),
            None,
        )
        .expect("preprocess");
        assert_eq!(&result.bytes[..], "a\u{FFFD}b".as_bytes());
    }

    #[test]
    fn json_is_validated() {
        let result = preprocess(
            Bytes::from_static(b"{\"a\": [1, 2]}"),
            Some(&media_type("application/json")),
            None,
        )
        .expect("preprocess");
        assert!(matches!(result.payload, Payload::Json(_)));
        let error = preprocess(
            Bytes::from_static(b"{\"a\": "),
            Some(&media_type("application/ld+json")),
            None,
        )
        .expect_err("truncated JSON");
        assert!(matches!(error, PreprocessError::Json(_)));
    }

    #[test]
    fn images_keep_their_bytes_and_report_their_header() {
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0DIHDR".to_vec();
        png.extend_from_slice(&64_u32.to_be_bytes());
        png.extend_from_slice(&32_u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        let bytes = Bytes::from(png);
        let result =
            preprocess(bytes.clone(), Some(&media_type("text/plain")), None).expect("preprocess");
        assert_eq!(
            result.media_type.essence(),
            "image/png",
            "magic beats the label"
        );
        assert_eq!(
            result.bytes.as_ptr(),
            bytes.as_ptr(),
            "pixels are never touched"
        );
        match result.payload {
            Payload::Image { format, header } => {
                assert_eq!(format, ImageFormat::Png);
                assert_eq!(
                    header.map(|header| (header.width, header.height)),
                    Some((64, 32))
                );
            }
            other => panic!("expected an image payload, got {other:?}"),
        }
    }

    #[test]
    fn an_image_label_without_a_readable_header_keeps_the_declared_format() {
        let result = preprocess(
            Bytes::from_static(b"\x00\x00\x00\x1cftypavif"),
            Some(&media_type("image/avif")),
            None,
        )
        .expect("preprocess");
        assert!(matches!(
            result.payload,
            Payload::Image {
                format: ImageFormat::Other,
                header: None
            }
        ));
    }

    #[test]
    fn the_url_extension_labels_unlabelled_bytes_and_binary_passes_through() {
        let url = Url::parse("file:///cards/app/main.js").expect("a URL");
        let result =
            preprocess(Bytes::from_static(b"1 + 1"), None, Some(&url)).expect("preprocess");
        assert_eq!(result.media_type.essence(), "text/javascript");
        assert!(matches!(result.payload, Payload::Text(_)));

        let url = Url::parse("file:///cards/app/font.woff2").expect("a URL");
        let bytes = Bytes::from_static(b"wOF2\x00\x01");
        let result = preprocess(bytes.clone(), None, Some(&url)).expect("preprocess");
        assert!(matches!(result.payload, Payload::Binary));
        assert_eq!(result.media_type.essence(), "font/woff2");
        assert_eq!(result.bytes.as_ptr(), bytes.as_ptr());

        let result = preprocess(Bytes::from_static(b"\x00\x01"), None, None).expect("preprocess");
        assert_eq!(result.media_type.essence(), "application/octet-stream");
        assert!(matches!(result.payload, Payload::Binary));
    }
}
