//! `header_ext_info` block decoding.

use crate::{
    container::compile_options,
    error::{DecodeError, Result},
    model::{CompileOptionField, CompileOptions},
    reader::Reader,
};

const HEADER_EXT_INFO_MAGIC: u32 = 0x494e_464f;

pub(super) fn decode<'a>(reader: &mut Reader<'a>, options: &mut CompileOptions<'a>) -> Result<()> {
    let start = reader.pos();
    // C++: header_ext_info.h:13-17 defines the 12-byte block header.
    let size = reader.u32()? as usize;
    let magic = reader.u32()?;
    if magic != HEADER_EXT_INFO_MAGIC {
        return Err(DecodeError::BadHeaderExtMagic(magic));
    }
    let field_count = reader.u32()?;

    for _ in 0..field_count {
        // C++: DecodeHeaderInfoField reads type, key_id, payload_size, then
        // payload bytes; the pointer member is not serialized
        // (lynx_binary_base_template_reader_impl.cc:299-312).
        let field_type = reader.u8()?;
        let key_id = reader.u8()?;
        let payload_size = usize::from(reader.u16()?);
        let payload_offset = reader.pos();
        let payload = reader.take(payload_size)?;
        let field = CompileOptionField {
            field_type,
            key_id,
            payload_offset,
            payload,
        };
        compile_options::apply_field(options, field)?;
    }

    reader.seek(
        start
            .checked_add(size)
            .ok_or(DecodeError::Malformed("header_ext_info size overflow"))?,
    )
}
