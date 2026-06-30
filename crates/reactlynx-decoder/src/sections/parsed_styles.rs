//! `PARSED_STYLES` section placeholder.

use crate::{
    error::{DecodeError, Result},
    model::TemplateBundle,
    reader::Reader,
};

const FIBER_ARCH: u8 = 1;

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // C++ gates PARSED_STYLES on arch_option_ == FIBER_ARCH:
    // core/template_bundle/template_codec/binary_decoder/lynx_binary_base_template_reader_impl.cc:488
    if bundle.compile_options.arch_option != FIBER_ARCH {
        return Err(DecodeError::Malformed(
            "PARSED_STYLES requires fiber arch_option",
        ));
    }
    // Run 2: decode parsed style maps; this run preserves the body bytes.
    bundle.raw_parsed_styles = Some(reader.take(reader.remaining())?);
    Ok(())
}
