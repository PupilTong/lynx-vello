//! Lynx XML source parsing over arbitrary UTF-8.
//!
//! `&str` rather than `&[u8]` because [`lynx_xml::parse`] takes `&str`:
//! `Arbitrary` hands the target the longest valid UTF-8 prefix of the raw
//! input, which keeps byte-level control without wasting executions on inputs
//! the signature cannot express.
//!
//! Beyond panic-freedom this checks the two invariants the API's callers rely
//! on and that a partial-index bug would silently break: the returned sections
//! borrow from `source`, and a `ParseError` points at a real UTF-8 boundary
//! inside it. The offsets are load-bearing — `crates/lynx-xml/src/lib.rs`
//! carries a `debug_assert!` on the char boundary that fuzz builds keep live,
//! and the UTF-16 offset is what a JavaScript-side error message consumes.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|source: &str| {
    match lynx_xml::parse(source) {
        Ok(parsed) => {
            assert!(
                lynx_vello_fuzz::borrows_from(parsed.engine_version, source),
                "engine_version does not borrow from the source"
            );
            assert!(
                lynx_vello_fuzz::borrows_from(parsed.main_thread_script, source),
                "main_thread_script does not borrow from the source"
            );
            for section in [parsed.style, parsed.background_thread_script]
                .into_iter()
                .flatten()
            {
                assert!(
                    lynx_vello_fuzz::borrows_from(section, source),
                    "an optional section does not borrow from the source"
                );
            }
        }
        Err(error) => {
            let byte_offset = error.byte_offset();
            assert!(
                byte_offset <= source.len(),
                "byte offset {byte_offset} past the end of a {}-byte source",
                source.len()
            );
            assert!(
                source.is_char_boundary(byte_offset),
                "byte offset {byte_offset} is not a UTF-8 boundary"
            );
            assert!(
                error.offset() <= source.encode_utf16().count(),
                "UTF-16 offset past the end of the source"
            );
            // The formatted message is what an embedder surfaces; it must not
            // panic on an offset the parser just produced.
            let _ = error.to_string();
        }
    }
});
