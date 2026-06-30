//! `LEPUS_CHUNK` section.

use crate::{
    error::{DecodeError, Result},
    model::{LepusChunk, TemplateBundle},
    reader::Reader,
};

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // C++: DecodeLepusChunkRoute reads path/start/end entries, then rebases
    // them by descriptor_offset (lynx_binary_reader.cc:227-246).
    let count = reader.compact_u32()? as usize;
    let mut routes = Vec::new();
    routes
        .try_reserve(count)
        .map_err(|_| DecodeError::Malformed("lepus chunk route is too large"))?;
    for _ in 0..count {
        let path = reader.lstr()?;
        let start = reader.compact_u32()? as usize;
        let end = reader.compact_u32()? as usize;
        routes.push((path, start, end));
    }

    let descriptor_offset = reader.pos();
    bundle
        .lepus_chunks
        .try_reserve(routes.len())
        .map_err(|_| DecodeError::Malformed("lepus chunks are too large"))?;
    for (path, start, end) in routes {
        if start > end {
            return Err(DecodeError::Malformed("lepus chunk route is inverted"));
        }
        let absolute = descriptor_offset
            .checked_add(start)
            .ok_or(DecodeError::Malformed("lepus chunk start overflow"))?;
        let mut chunk = reader.clone();
        chunk.seek(absolute)?;
        let len = chunk.compact_u64()? as usize;
        let code = chunk.take(len)?;
        bundle.lepus_chunks.push(LepusChunk { path, code });
    }
    Ok(())
}
