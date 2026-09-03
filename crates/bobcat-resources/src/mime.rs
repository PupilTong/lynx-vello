//! Media types: parsing, classification, and the sniffing that decides what
//! a payload *is* before anything downstream trusts its label.
//!
//! The preprocessing pipeline is keyed on the media type, so the type has to
//! be right more often than servers are: a PNG labelled `text/plain` must
//! still decode as an image, and a `Content-Type` that is missing outright
//! must not turn every script into an opaque blob. [`sniff`] is the narrowed
//! WHATWG mimesniff rule that gives that answer — image magic beats any
//! label, a label beats a byte scan, and the scan is the last resort.

use std::fmt;
use std::str::FromStr;

use crate::image_header;

/// A parsed media type: a lowercase `type/subtype` essence plus parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaType {
    essence: String,
    slash: usize,
    parameters: Vec<(String, String)>,
}

/// What the preprocessing pipeline does with a resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceClass {
    /// Decoded to UTF-8 text.
    Text,
    /// Decoded to UTF-8 and validated as JSON.
    Json,
    /// Container-sniffed and header-probed; the pixels stay encoded for the
    /// platform decoder.
    Image(ImageFormat),
    /// Passed through untouched.
    Binary,
}

/// The image containers the platform decoders and the header probe know.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    WebP,
    Bmp,
    Svg,
    /// An `image/*` type this crate has no header knowledge of; the platform
    /// decoder may still know it.
    Other,
}

impl ImageFormat {
    /// The canonical media type of the container.
    #[must_use]
    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::WebP => "image/webp",
            Self::Bmp => "image/bmp",
            Self::Svg => "image/svg+xml",
            Self::Other => "application/octet-stream",
        }
    }

    fn from_subtype(subtype: &str) -> Self {
        match subtype {
            "png" | "apng" => Self::Png,
            "jpeg" | "jpg" | "pjpeg" => Self::Jpeg,
            "gif" => Self::Gif,
            "webp" => Self::WebP,
            "bmp" | "x-bmp" | "x-ms-bmp" => Self::Bmp,
            "svg+xml" => Self::Svg,
            _ => Self::Other,
        }
    }
}

impl MediaType {
    /// Parses a `Content-Type`-style value such as `text/html; charset=utf-8`.
    ///
    /// Type and subtype are lowercased, parameter names are lowercased, and
    /// quoted parameter values are unquoted. A malformed parameter is
    /// skipped rather than failing the whole value, which is what the
    /// mimesniff "parse a MIME type" algorithm does; a malformed essence is
    /// `None`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim_matches(is_http_whitespace);
        let (essence, rest) = match value.find(';') {
            Some(index) => (&value[..index], &value[index + 1..]),
            None => (value, ""),
        };
        let essence = essence.trim_matches(is_http_whitespace);
        let slash = essence.find('/')?;
        let (kind, subtype) = (&essence[..slash], &essence[slash + 1..]);
        if !is_token(kind) || !is_token(subtype) {
            return None;
        }
        let mut media_type = Self {
            essence: essence.to_ascii_lowercase(),
            slash,
            parameters: Vec::new(),
        };
        for (name, value) in (ParameterIter { rest }) {
            if !media_type
                .parameters
                .iter()
                .any(|(existing, _)| existing == &name)
            {
                media_type.parameters.push((name, value));
            }
        }
        Some(media_type)
    }

    /// `type/subtype`, lowercase.
    #[must_use]
    pub fn essence(&self) -> &str {
        &self.essence
    }

    /// The top-level type: `text` in `text/css`.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.essence[..self.slash]
    }

    /// The subtype: `css` in `text/css`.
    #[must_use]
    pub fn subtype(&self) -> &str {
        &self.essence[self.slash + 1..]
    }

    /// A parameter's value, by case-insensitive name.
    #[must_use]
    pub fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The `charset` parameter, if any.
    #[must_use]
    pub fn charset(&self) -> Option<&str> {
        self.parameter("charset")
    }

    /// Sets a parameter, replacing an existing one of the same name.
    #[must_use]
    pub fn with_parameter(mut self, name: &str, value: &str) -> Self {
        let name = name.to_ascii_lowercase();
        match self
            .parameters
            .iter_mut()
            .find(|(existing, _)| *existing == name)
        {
            Some((_, existing)) => value.clone_into(existing),
            None => self.parameters.push((name, value.to_owned())),
        }
        self
    }

    /// How the pipeline treats a payload of this type.
    #[must_use]
    pub fn class(&self) -> ResourceClass {
        let (kind, subtype) = (self.kind(), self.subtype());
        if kind == "image" {
            return ResourceClass::Image(ImageFormat::from_subtype(subtype));
        }
        if subtype.ends_with("+json") || matches!(self.essence(), "application/json" | "text/json")
        {
            return ResourceClass::Json;
        }
        if kind == "text"
            || subtype.ends_with("+xml")
            || matches!(
                self.essence(),
                "application/javascript"
                    | "application/ecmascript"
                    | "application/x-javascript"
                    | "application/xml"
                    | "application/x-www-form-urlencoded"
            )
        {
            return ResourceClass::Text;
        }
        ResourceClass::Binary
    }

    /// Whether this type says nothing a byte scan would not: the labels a
    /// server puts on content it did not look at.
    fn is_placeholder(&self) -> bool {
        matches!(
            self.essence(),
            "application/octet-stream" | "unknown/unknown" | "application/unknown" | "*/*"
        )
    }
}

impl fmt::Display for MediaType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.essence)?;
        for (name, value) in &self.parameters {
            write!(formatter, "; {name}=")?;
            if is_token(value) {
                formatter.write_str(value)?;
            } else {
                formatter.write_str("\"")?;
                for character in value.chars() {
                    if matches!(character, '"' | '\\') {
                        formatter.write_str("\\")?;
                    }
                    write!(formatter, "{character}")?;
                }
                formatter.write_str("\"")?;
            }
        }
        Ok(())
    }
}

/// The parse failure: the value had no well-formed `type/subtype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidMediaType;

impl fmt::Display for InvalidMediaType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("not a media type")
    }
}

impl std::error::Error for InvalidMediaType {}

impl FromStr for MediaType {
    type Err = InvalidMediaType;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or(InvalidMediaType)
    }
}

/// Walks `;`-separated `name=value` parameters, skipping malformed ones.
struct ParameterIter<'a> {
    rest: &'a str,
}

impl Iterator for ParameterIter<'_> {
    type Item = (String, String);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.rest.is_empty() {
                return None;
            }
            let rest = self.rest.trim_start_matches(is_http_whitespace);
            let name_end = rest.find([';', '=']).unwrap_or(rest.len());
            let name = rest[..name_end].to_ascii_lowercase();
            let after_name = &rest[name_end..];
            let Some(after_equals) = after_name.strip_prefix('=') else {
                // No value: skip this parameter and continue after the `;`.
                self.rest = after_name.strip_prefix(';').unwrap_or("");
                continue;
            };
            let (value, remainder) = if let Some(quoted) = after_equals.strip_prefix('"') {
                let (value, consumed) = collect_quoted(quoted);
                let remainder = quoted[consumed..]
                    .find(';')
                    .map_or("", |index| &quoted[consumed + index + 1..]);
                (value, remainder)
            } else {
                let end = after_equals.find(';').unwrap_or(after_equals.len());
                let value = after_equals[..end]
                    .trim_end_matches(is_http_whitespace)
                    .to_owned();
                let remainder = after_equals.get(end + 1..).unwrap_or("");
                (value, remainder)
            };
            self.rest = remainder;
            if !is_token(&name) || value.is_empty() || !is_quotable(&value) {
                continue;
            }
            return Some((name, value));
        }
    }
}

/// Collects an HTTP quoted-string body (the opening quote already consumed),
/// returning the unescaped value and the number of bytes consumed including
/// the closing quote when one exists.
fn collect_quoted(input: &str) -> (String, usize) {
    let mut value = String::new();
    let mut characters = input.char_indices();
    while let Some((index, character)) = characters.next() {
        match character {
            '\\' => match characters.next() {
                Some((_, escaped)) => value.push(escaped),
                None => return (value, input.len()),
            },
            '"' => return (value, index + 1),
            other => value.push(other),
        }
    }
    (value, input.len())
}

fn is_http_whitespace(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r')
}

/// RFC 9110 `token` characters.
fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// Characters an HTTP quoted-string may carry once unescaped.
fn is_quotable(value: &str) -> bool {
    value.chars().all(|character| {
        character == '\t' || (' '..='~').contains(&character) || character as u32 >= 0x80
    })
}

/// The media type a file extension conventionally carries, for `file:` URLs
/// and the like where nothing else labels the bytes.
#[must_use]
pub fn from_extension(path: &str) -> Option<MediaType> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    let value = match extension.as_str() {
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "txt" | "text" => "text/plain; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "png" | "apng" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" | "dib" => "image/bmp",
        "svg" | "svgz" => "image/svg+xml",
        "avif" => "image/avif",
        "heic" | "heif" => "image/heic",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        "wasm" => "application/wasm",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "bundle" => "application/octet-stream",
        _ => return None,
    };
    MediaType::parse(value)
}

/// Decides what `bytes` are, given what they were declared to be.
///
/// Image magic bytes always win: a server that labels a PNG `text/plain`
/// still gets an image, and a script can never be mistaken for one because
/// no text starts with a container signature. An SVG is the one image that
/// is text, so it is recognised only where nothing specific was declared.
/// Otherwise a declared type is trusted, unless it is a placeholder like
/// `application/octet-stream`. With nothing to go on, a scan for binary
/// data bytes separates `text/plain` from an opaque blob, and a Unicode BOM
/// names the text's charset.
#[must_use]
pub fn sniff(bytes: &[u8], declared: Option<&MediaType>) -> MediaType {
    let specific = declared.filter(|declared| !declared.is_placeholder());
    if let Some(format) = image_header::detect_format(bytes) {
        let declared_svg = specific.is_some_and(|declared| declared.essence() == "image/svg+xml");
        if format != ImageFormat::Svg || specific.is_none() || declared_svg {
            return MediaType::parse(format.media_type()).expect("a canonical image media type");
        }
    }
    if let Some(declared) = specific {
        return declared.clone();
    }
    if looks_binary(bytes) {
        MediaType::parse("application/octet-stream").expect("a canonical media type")
    } else {
        let text = MediaType::parse("text/plain").expect("a canonical media type");
        match bom_charset(bytes) {
            Some(charset) => text.with_parameter("charset", charset),
            None => text,
        }
    }
}

/// The charset a leading byte-order mark names, if there is one.
#[must_use]
pub fn bom_charset(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some("utf-8")
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Some("utf-16le")
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Some("utf-16be")
    } else {
        None
    }
}

/// The mimesniff "binary data byte" scan over the resource header.
fn looks_binary(bytes: &[u8]) -> bool {
    if bom_charset(bytes).is_some() {
        return false;
    }
    bytes
        .iter()
        .take(1445)
        .any(|byte| matches!(byte, 0x00..=0x08 | 0x0B | 0x0E..=0x1A | 0x1C..=0x1F))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: &str) -> MediaType {
        MediaType::parse(value).unwrap_or_else(|| panic!("`{value}` parses"))
    }

    #[test]
    fn essence_is_lowercased_and_split() {
        let media_type = parse(" Text/CSS ; Charset=UTF-8 ");
        assert_eq!(media_type.essence(), "text/css");
        assert_eq!(media_type.kind(), "text");
        assert_eq!(media_type.subtype(), "css");
        assert_eq!(media_type.charset(), Some("UTF-8"));
        assert_eq!(media_type.parameter("CHARSET"), Some("UTF-8"));
        assert_eq!(media_type.to_string(), "text/css; charset=UTF-8");
    }

    #[test]
    fn malformed_essences_are_refused() {
        for value in [
            "",
            "text",
            "/css",
            "text/",
            "te xt/css",
            "text/c ss",
            "text/css/x y",
        ] {
            assert!(
                MediaType::parse(value).is_none(),
                "`{value}` must not parse"
            );
        }
        assert_eq!("text".parse::<MediaType>(), Err(InvalidMediaType));
    }

    #[test]
    fn parameters_are_unquoted_deduplicated_and_skipped_when_malformed() {
        let media_type = parse(r#"text/plain; a="x;y\"z"; a=second; noequals; =novalue; b=; c=ok"#);
        assert_eq!(media_type.parameter("a"), Some("x;y\"z"));
        assert_eq!(media_type.parameter("b"), None);
        assert_eq!(media_type.parameter("c"), Some("ok"));
        assert_eq!(media_type.to_string(), r#"text/plain; a="x;y\"z"; c=ok"#);
    }

    #[test]
    fn an_unterminated_quote_takes_the_rest_of_the_value() {
        let media_type = parse("text/plain; a=\"open; b=1");
        assert_eq!(media_type.parameter("a"), Some("open; b=1"));
        assert_eq!(media_type.parameter("b"), None);
    }

    #[test]
    fn with_parameter_replaces_or_appends() {
        let media_type = parse("text/plain; charset=latin1")
            .with_parameter("Charset", "utf-8")
            .with_parameter("x", "1");
        assert_eq!(media_type.to_string(), "text/plain; charset=utf-8; x=1");
    }

    #[test]
    fn classification_follows_the_essence() {
        assert_eq!(parse("text/css").class(), ResourceClass::Text);
        assert_eq!(parse("application/javascript").class(), ResourceClass::Text);
        assert_eq!(parse("application/xml").class(), ResourceClass::Text);
        assert_eq!(parse("application/rss+xml").class(), ResourceClass::Text);
        assert_eq!(parse("application/json").class(), ResourceClass::Json);
        assert_eq!(parse("application/ld+json").class(), ResourceClass::Json);
        assert_eq!(parse("text/json").class(), ResourceClass::Json);
        assert_eq!(
            parse("image/png").class(),
            ResourceClass::Image(ImageFormat::Png)
        );
        assert_eq!(
            parse("image/jpg").class(),
            ResourceClass::Image(ImageFormat::Jpeg)
        );
        assert_eq!(
            parse("image/x-ms-bmp").class(),
            ResourceClass::Image(ImageFormat::Bmp)
        );
        assert_eq!(
            parse("image/svg+xml").class(),
            ResourceClass::Image(ImageFormat::Svg)
        );
        assert_eq!(
            parse("image/avif").class(),
            ResourceClass::Image(ImageFormat::Other)
        );
        assert_eq!(parse("application/wasm").class(), ResourceClass::Binary);
        assert_eq!(parse("font/woff2").class(), ResourceClass::Binary);
    }

    #[test]
    fn extensions_map_to_conventional_types() {
        assert_eq!(
            from_extension("/a/b/style.CSS").unwrap().essence(),
            "text/css"
        );
        assert_eq!(
            from_extension("main.mjs").unwrap().essence(),
            "text/javascript"
        );
        assert_eq!(
            from_extension("C:\\x\\photo.JPEG").unwrap().essence(),
            "image/jpeg"
        );
        assert_eq!(
            from_extension("data.json").unwrap().charset(),
            Some("utf-8")
        );
        assert!(from_extension("noextension").is_none());
        assert!(from_extension("weird.unknownext").is_none());
        assert!(from_extension("dir.d/file").is_none());
    }

    #[test]
    fn image_magic_beats_any_label() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
        let declared = parse("text/plain");
        assert_eq!(sniff(&png, Some(&declared)).essence(), "image/png");
        assert_eq!(sniff(&png, None).essence(), "image/png");
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE0", None).essence(), "image/jpeg");
        assert_eq!(sniff(b"GIF89a\x01\x00", None).essence(), "image/gif");
        assert_eq!(
            sniff(b"RIFF\x00\x00\x00\x00WEBPVP8 ", None).essence(),
            "image/webp"
        );
        assert_eq!(sniff(b"BM\x00\x00", None).essence(), "image/bmp");
    }

    #[test]
    fn a_specific_label_is_trusted_and_a_placeholder_is_not() {
        let css = parse("text/css; charset=utf-8");
        assert_eq!(sniff(b"body{}", Some(&css)), css);
        let octet = parse("application/octet-stream");
        assert_eq!(sniff(b"body{}", Some(&octet)).essence(), "text/plain");
        assert_eq!(
            sniff(b"\x00\x01\x02", Some(&octet)).essence(),
            "application/octet-stream"
        );
        assert_eq!(sniff(b"", None).essence(), "text/plain");
    }

    #[test]
    fn svg_is_recognised_only_where_nothing_specific_was_declared() {
        let svg = b"<?xml version=\"1.0\"?>\n<!-- c --><svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        assert_eq!(sniff(svg, None).essence(), "image/svg+xml");
        let octet = parse("application/octet-stream");
        assert_eq!(sniff(svg, Some(&octet)).essence(), "image/svg+xml");
        let xml = parse("text/xml");
        assert_eq!(sniff(svg, Some(&xml)).essence(), "text/xml");
        let declared_svg = parse("image/svg+xml");
        assert_eq!(sniff(svg, Some(&declared_svg)).essence(), "image/svg+xml");
        assert_eq!(
            sniff(b"<html>", Some(&declared_svg)).essence(),
            "image/svg+xml"
        );
    }

    #[test]
    fn a_bom_names_the_charset_of_unlabelled_text() {
        assert_eq!(sniff(b"\xEF\xBB\xBFhello", None).charset(), Some("utf-8"));
        assert_eq!(sniff(b"\xFF\xFEh\x00", None).charset(), Some("utf-16le"));
        assert_eq!(sniff(b"\xFE\xFF\x00h", None).charset(), Some("utf-16be"));
        assert_eq!(sniff(b"hello", None).charset(), None);
    }
}
