//! `STYLE_OBJECT` section placeholder.

use crate::{error::Result, model::TemplateBundle, reader::Reader};

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // Run 2: decode simple-styling objects; this run preserves the body bytes.
    bundle.raw_style_object = Some(reader.take(reader.remaining())?);
    Ok(())
}
