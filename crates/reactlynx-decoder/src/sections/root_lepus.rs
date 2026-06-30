//! `ROOT_LEPUS` section.

use crate::{
    error::Result,
    model::{LepusContext, TemplateBundle},
    reader::Reader,
};

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // C++: DecodeContextBundle for LepusNG reads CompactU64 code length and
    // raw code bytes (core/runtime/lepus/base_binary_reader.cc:410-423).
    let len = reader.compact_u64()? as usize;
    let code = reader.take(len)?;
    bundle.root_lepus = Some(LepusContext { code });
    Ok(())
}
