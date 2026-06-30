//! `JS` source section.

use crate::{
    error::{DecodeError, Result},
    model::{JsSource, TemplateBundle},
    reader::Reader,
};

pub(crate) fn decode_source<'a>(
    reader: &mut Reader<'a>,
    bundle: &mut TemplateBundle<'a>,
) -> Result<()> {
    // C++: DeserializeJSSourceSection reads U32 count, then path/content
    // strings (lynx_binary_base_template_reader_impl.cc:528-539).
    let count = reader.u32()? as usize;
    bundle
        .js_sources
        .try_reserve(count)
        .map_err(|_| DecodeError::Malformed("js source section is too large"))?;
    for _ in 0..count {
        let path = reader.lstr()?;
        let content = reader.lstr()?;
        bundle.js_sources.push(JsSource { path, content });
    }
    Ok(())
}
