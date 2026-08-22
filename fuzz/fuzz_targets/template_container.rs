//! `.web.bundle` container decode over arbitrary bytes.
//!
//! [`lynx_template_decoder::decode`] is the first code a downloaded bundle
//! reaches. Its contract is that every malformed input leaves through
//! `DecodeError`, so any panic — a slice out of range, an arithmetic overflow
//! while folding an attacker-chosen length field, an `unwrap` on a section the
//! grammar does not actually guarantee — is a bug in the decoder, not in the
//! input. This target asserts exactly that and nothing else.
//!
//! Seed the corpus from `crates/lynx-template-decoder/tests/fixtures` (see
//! `fuzz/seed-corpus.sh`): without a seed almost every execution stops at the
//! `SDRA`/`WROF` magic and the section decoders are never reached.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(template) = lynx_template_decoder::decode(data) else {
        return;
    };

    // A decode that reported success must also survive the accessors an
    // embedder reaches for immediately afterwards.
    let _ = template.config_str("pageConfig");
    let _ = template.config_flag("enableCSSSelector");
    if let Some(style_info) = &template.style_info {
        lynx_vello_fuzz::walk_style_info(style_info);
    }
});
