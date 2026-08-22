//! Shared helpers for the fuzz targets.
//!
//! These live in a library rather than in each target so that a change to the
//! wire envelope or to the accessor walk lands in one place, and so that
//! `cargo test -p lynx-vello-fuzz` can check the envelope builder itself — a
//! silently wrong envelope would make `template_style_info` fuzz nothing while
//! still reporting executions.

use lynx_template_decoder::style_info::{Rule, StyleInfo};
use lynx_template_decoder::{MAGIC_0, MAGIC_1, SectionLabel};

/// Wraps `section` in the smallest container `lynx_template_decoder::decode`
/// accepts, so a target can drive the `StyleInfo` section decoder directly.
///
/// Returns `None` when the section cannot be described by the format's `u32`
/// length field; libFuzzer never produces such an input, but truncating the
/// length would silently fuzz a different payload than the one generated.
#[must_use]
pub fn wrap_style_info_section(section: &[u8]) -> Option<Vec<u8>> {
    let length = u32::try_from(section.len()).ok()?;

    let mut bundle = Vec::with_capacity(20 + section.len());
    bundle.extend_from_slice(&MAGIC_0.to_le_bytes());
    bundle.extend_from_slice(&MAGIC_1.to_le_bytes());
    bundle.extend_from_slice(&1u32.to_le_bytes()); // version
    bundle.extend_from_slice(&(SectionLabel::StyleInfo as u32).to_le_bytes());
    bundle.extend_from_slice(&length.to_le_bytes());
    bundle.extend_from_slice(section);
    Some(bundle)
}

/// Calls every public accessor on a decoded [`StyleInfo`].
///
/// Decoding is only half the attack surface. The selector and value writers
/// walk `Vec` indices and slice ranges that came out of the archive, so a
/// validation gap shows up here rather than in `decode`. Results are consumed
/// with `black_box` so a release build cannot delete the walk.
pub fn walk_style_info(style_info: &StyleInfo) {
    let _ = std::hint::black_box(style_info.style_text_size_hint);
    for sheet in style_info.css_id_to_style_sheet.values() {
        let _ = std::hint::black_box(sheet.imports.len());
        for rule in &sheet.rules {
            walk_rule(rule);
        }
    }
}

fn walk_rule(rule: &Rule) {
    let _ = std::hint::black_box(rule.kind);
    for selector in &rule.prelude.selectors {
        let _ = std::hint::black_box(selector.to_css_string());
    }
    for declaration in &rule.declaration_block.declarations {
        let _ = std::hint::black_box(declaration.property.name());
        let _ = std::hint::black_box(declaration.value_text());
        let _ = std::hint::black_box(declaration.value_and_importance());
    }
    for child in &rule.children {
        walk_rule(child);
    }
}

/// Whether `part` points into `whole`.
///
/// An empty `&str` is exempt: a zero-length slice of the source and a `""`
/// literal are indistinguishable at runtime, and the parser is allowed to
/// return either for a present-but-empty section.
#[must_use]
pub fn borrows_from(part: &str, whole: &str) -> bool {
    if part.is_empty() {
        return true;
    }
    let start = whole.as_ptr() as usize;
    let offset = part.as_ptr() as usize;
    offset >= start && offset + part.len() <= start + whole.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The envelope must actually reach the section decoder. If this breaks,
    /// `template_style_info` still reports millions of executions while
    /// exercising only the container header.
    #[test]
    fn the_envelope_reaches_the_style_info_decoder() {
        // Not a valid rkyv archive: the point is that decoding got far enough
        // to reject it *as a StyleInfo section* rather than as bad framing.
        let bundle = wrap_style_info_section(&[0xff; 32]).expect("length fits");
        let error = lynx_template_decoder::decode(&bundle).expect_err("not an archive");
        assert!(
            matches!(error, lynx_template_decoder::DecodeError::StyleInfo(_)),
            "expected the StyleInfo decoder to reject this, got {error:?}"
        );
    }

    #[test]
    fn an_empty_section_still_reaches_the_style_info_decoder() {
        let bundle = wrap_style_info_section(&[]).expect("length fits");
        let error = lynx_template_decoder::decode(&bundle).expect_err("not an archive");
        assert!(matches!(
            error,
            lynx_template_decoder::DecodeError::StyleInfo(_)
        ));
    }

    #[test]
    fn borrowing_is_decided_by_address_range() {
        let source = "hello world";
        assert!(borrows_from(&source[6..], source));
        assert!(borrows_from("", source));
        // Heap-allocated on purpose: two equal string literals are merged into
        // one address, so a literal copy would pass by accident.
        let elsewhere = String::from("hello world");
        assert!(!borrows_from(elsewhere.as_str(), source));
    }
}
