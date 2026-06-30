//! Compile-option field application for `header_ext_info`.

use crate::{
    error::{DecodeError, Result},
    model::{CompileOptionField, CompileOptions},
    version::Version,
};

pub(super) fn apply_field<'a>(
    options: &mut CompileOptions<'a>,
    field: CompileOptionField<'a>,
) -> Result<()> {
    // C++: compile_options.h:117-153 maps key ids to fields; values are
    // memcpy-reinterpreted in lynx_binary_base_template_reader_impl.cc:24-34.
    match field.key_id {
        0 => {
            let value = core::str::from_utf8(field.payload)
                .map_err(|_| DecodeError::Utf8(field.payload_offset))?;
            options.target_sdk_str = value;
            options.target_sdk = Version::parse(value);
        }
        1 => options.enable_css_parser = read_bool(field.payload)?,
        6 => options.enable_css_variable = read_bool(field.payload)?,
        20 => options.enable_trial_options = read_bool(field.payload)?,
        25 => options.enable_fiber_arch = read_bool(field.payload)?,
        27 => options.enable_flexible_template = read_bool(field.payload)?,
        28 => options.enable_fiber_arch = read_u8(field.payload)? == 1,
        29 => options.enable_css_selector = read_bool(field.payload)?,
        33 => options.enable_simple_styling = read_bool(field.payload)?,
        _ => {}
    }
    options.raw_fields.push(field);
    Ok(())
}

fn read_bool(payload: &[u8]) -> Result<bool> {
    Ok(read_u8(payload)? != 0)
}

fn read_u8(payload: &[u8]) -> Result<u8> {
    payload
        .first()
        .copied()
        .ok_or(DecodeError::Malformed("empty header_ext_info payload"))
}
