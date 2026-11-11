//! `data:` URLs, decoded the way the WHATWG fetch standard's "data: URL
//! processor" decodes them, including forgiving base64.
//!
//! A `data:` URL is the one transport that needs no IO at all, so it is
//! handled before any other and never touches a cache: the bytes are the
//! URL. The base64 decoder is written here rather than pulled in because the
//! forgiving variant — whitespace anywhere, padding optional — is a dozen
//! lines, and this crate's dependency policy is to add nothing it does not
//! need.

use url::Url;

use crate::mime::MediaType;

/// A decoded `data:` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataUrl {
    pub media_type: MediaType,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DataUrlError {
    #[error("not a data: URL")]
    NotDataUrl,
    #[error("a data: URL needs a comma between its media type and its body")]
    MissingComma,
    #[error("the base64 body of the data: URL is malformed")]
    InvalidBase64,
}

/// Decodes `url` per the fetch standard: an optional media type (defaulting
/// to `text/plain;charset=US-ASCII`, and to `text/plain` in front of a bare
/// `;charset=...`), a `;base64` suffix selecting forgiving base64, and a
/// percent-encoded body otherwise.
pub fn parse(url: &Url) -> Result<DataUrl, DataUrlError> {
    if url.scheme() != "data" {
        return Err(DataUrlError::NotDataUrl);
    }
    // The serialization without its fragment, minus the `data:` prefix — the
    // spec's input, which the `Url` type already holds normalized.
    let serialized = url.as_str();
    let without_fragment = serialized
        .find('#')
        .map_or(serialized, |index| &serialized[..index]);
    let input = &without_fragment["data:".len()..];

    let comma = input.find(',').ok_or(DataUrlError::MissingComma)?;
    let mut media_type = input[..comma].trim_matches(is_ascii_whitespace).to_owned();
    let body = percent_decode(&input.as_bytes()[comma + 1..]);

    let bytes = match strip_base64_suffix(&media_type) {
        Some(stripped) => {
            media_type = stripped;
            let text = String::from_utf8_lossy(&body);
            decode_forgiving_base64(&text).ok_or(DataUrlError::InvalidBase64)?
        }
        None => body,
    };

    if media_type.starts_with(';') {
        media_type.insert_str(0, "text/plain");
    }
    let media_type = MediaType::parse(&media_type).unwrap_or_else(|| {
        MediaType::parse("text/plain; charset=US-ASCII").expect("the default media type parses")
    });
    Ok(DataUrl { media_type, bytes })
}

/// `mimeType` ending in `;` + spaces + `base64` (any case), with that suffix
/// removed — the spec's exact rule, which is why a `; base64` with spaces is
/// accepted while a `;base64 ` with a trailing space was already trimmed.
fn strip_base64_suffix(media_type: &str) -> Option<String> {
    let trimmed_length = media_type.len().checked_sub("base64".len())?;
    let (head, tail) = media_type.split_at(trimmed_length);
    if !tail.eq_ignore_ascii_case("base64") {
        return None;
    }
    let head = head.trim_end_matches(' ');
    let head = head.strip_suffix(';')?;
    Some(head.to_owned())
}

fn is_ascii_whitespace(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0C')
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        let byte = input[index];
        if byte == b'%'
            && let (Some(high), Some(low)) = (
                input.get(index + 1).and_then(|byte| hex_value(*byte)),
                input.get(index + 2).and_then(|byte| hex_value(*byte)),
            )
        {
            output.push(high << 4 | low);
            index += 3;
        } else {
            output.push(byte);
            index += 1;
        }
    }
    output
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// The WHATWG "forgiving-base64 decode": ASCII whitespace is removed, up to
/// two trailing `=` are dropped when the length is a multiple of four, a
/// length of one modulo four is a failure, and so is any byte outside the
/// standard alphabet.
#[must_use]
pub fn decode_forgiving_base64(input: &str) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = input
        .bytes()
        .filter(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C'))
        .collect();
    if data.len().is_multiple_of(4) {
        if data.last() == Some(&b'=') {
            data.pop();
        }
        if data.last() == Some(&b'=') {
            data.pop();
        }
    }
    if data.len() % 4 == 1 {
        return None;
    }
    let mut output = Vec::with_capacity(data.len() / 4 * 3 + 2);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in data {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(url: &str) -> DataUrl {
        parse(&Url::parse(url).expect("a URL")).expect("a data: URL")
    }

    #[test]
    fn a_bare_body_is_ascii_text() {
        let decoded = data("data:,Hello%2C%20World%21");
        assert_eq!(decoded.bytes, b"Hello, World!");
        assert_eq!(decoded.media_type.essence(), "text/plain");
        assert_eq!(decoded.media_type.charset(), Some("US-ASCII"));
    }

    #[test]
    fn base64_bodies_decode_forgivingly() {
        assert_eq!(data("data:text/plain;base64,SGVsbG8=").bytes, b"Hello");
        assert_eq!(
            data("data:text/plain;base64,SGVs bG8").bytes,
            b"Hello",
            "whitespace and missing padding are forgiven"
        );
        assert_eq!(
            data("data:text/plain; BASE64,SGVsbG8=").bytes,
            b"Hello",
            "the suffix is case-insensitive and may follow spaces"
        );
        assert_eq!(
            data("data:text/plain;base64,SGVsbG8%3D").bytes,
            b"Hello",
            "the body is percent-decoded before base64"
        );
        let media_type = data("data:image/png;base64,iVBORw0KGgo=").media_type;
        assert_eq!(media_type.essence(), "image/png");
        assert!(
            media_type.parameter("base64").is_none(),
            "the suffix is not a parameter"
        );
    }

    #[test]
    fn malformed_base64_is_an_error() {
        let url = Url::parse("data:text/plain;base64,SGV*sbG8=").expect("a URL");
        assert_eq!(parse(&url), Err(DataUrlError::InvalidBase64));
        let url = Url::parse("data:text/plain;base64,SGVsb").expect("a URL");
        assert_eq!(
            parse(&url),
            Err(DataUrlError::InvalidBase64),
            "length 1 mod 4"
        );
    }

    #[test]
    fn a_charset_without_a_type_gets_text_plain_and_garbage_gets_the_default() {
        let decoded = data("data:;charset=utf-8,caf%C3%A9");
        assert_eq!(decoded.media_type.essence(), "text/plain");
        assert_eq!(decoded.media_type.charset(), Some("utf-8"));
        assert_eq!(decoded.bytes, "café".as_bytes());
        let decoded = data("data:not-a-type,x");
        assert_eq!(
            decoded.media_type.to_string(),
            "text/plain; charset=US-ASCII"
        );
    }

    #[test]
    fn the_fragment_is_not_part_of_the_body_and_the_comma_is_required() {
        assert_eq!(data("data:,abc#frag").bytes, b"abc");
        let url = Url::parse("data:text/plain").expect("a URL");
        assert_eq!(parse(&url), Err(DataUrlError::MissingComma));
        let url = Url::parse("https://example.test/").expect("a URL");
        assert_eq!(parse(&url), Err(DataUrlError::NotDataUrl));
    }

    #[test]
    fn forgiving_base64_edge_cases() {
        assert_eq!(decode_forgiving_base64(""), Some(Vec::new()));
        assert_eq!(decode_forgiving_base64("YQ=="), Some(b"a".to_vec()));
        assert_eq!(decode_forgiving_base64("YQ"), Some(b"a".to_vec()));
        assert_eq!(decode_forgiving_base64("YWI="), Some(b"ab".to_vec()));
        assert_eq!(decode_forgiving_base64("YWJj"), Some(b"abc".to_vec()));
        assert_eq!(decode_forgiving_base64("Y"), None);
        assert_eq!(
            decode_forgiving_base64("YQ==="),
            None,
            "three pads leave a stray"
        );
        assert_eq!(
            decode_forgiving_base64("YQ-_"),
            None,
            "the URL-safe alphabet is not accepted"
        );
        assert_eq!(decode_forgiving_base64("//8="), Some(vec![0xFF, 0xFF]));
    }

    #[test]
    fn percent_decoding_leaves_malformed_escapes_alone() {
        assert_eq!(percent_decode(b"a%2Gb%2"), b"a%2Gb%2");
        assert_eq!(percent_decode(b"%41%62"), b"Ab");
    }
}
