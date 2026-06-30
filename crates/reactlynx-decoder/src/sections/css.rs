//! `CSS` section placeholder.

use crate::{error::Result, model::TemplateBundle, reader::Reader};

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // Run 2: decode CSS routes/fragments; this run preserves the body bytes.
    bundle.raw_css = Some(reader.take(reader.remaining())?);
    Ok(())
}
