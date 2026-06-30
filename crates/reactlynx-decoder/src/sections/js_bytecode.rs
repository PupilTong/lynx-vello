//! `JS_BYTECODE` section.

use crate::{
    error::{DecodeError, Result},
    model::{JsBytecode, TemplateBundle},
    reader::Reader,
};

const QUICKJS_RUNTIME_TYPE: u32 = 2;

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // C++: DeserializeJSBytecodeSection checks engine == quickjs, then reads
    // U32 count and path/len/bytes entries
    // (lynx_binary_base_template_reader_impl.cc:541-560).
    let engine = reader.u32()?;
    if engine != QUICKJS_RUNTIME_TYPE {
        return Err(DecodeError::Malformed("js bytecode engine is not quickjs"));
    }
    let count = reader.u32()? as usize;
    bundle
        .js_bytecode
        .try_reserve(count)
        .map_err(|_| DecodeError::Malformed("js bytecode section is too large"))?;
    for _ in 0..count {
        let path = reader.lstr()?;
        let len = reader.compact_u64()? as usize;
        let data = reader.take(len)?;
        bundle.js_bytecode.push(JsBytecode { path, data });
    }
    Ok(())
}
