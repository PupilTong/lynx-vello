//! rkyv `StyleInfo` section validation and deserialization over arbitrary
//! bytes.
//!
//! The section is a zero-copy rkyv archive: `decode_style_info` runs
//! `check_archived_root` over bytes it did not produce and then deserializes
//! the validated archive. That validation is the only thing standing between a
//! hostile bundle and an out-of-bounds relative pointer, so it is worth far
//! more fuzzing budget than the container framing around it.
//!
//! Reaching it through [`lynx_template_decoder::decode`] alone would spend
//! nearly every execution on the container header, so this target *builds* the
//! smallest container that carries one `StyleInfo` section and puts the
//! fuzzer's bytes inside it. The decoder under test is the real one; only the
//! 20-byte envelope is synthetic.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|section: &[u8]| {
    let Some(bundle) = lynx_vello_fuzz::wrap_style_info_section(section) else {
        return;
    };
    let Ok(template) = lynx_template_decoder::decode(&bundle) else {
        return;
    };
    let Some(style_info) = &template.style_info else {
        return;
    };

    lynx_vello_fuzz::walk_style_info(style_info);
});
