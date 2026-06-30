//! `PARSED_STYLES` section placeholder.

use crate::{error::Result, model::TemplateBundle, reader::Reader};

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // Run 2: decode parsed style maps; this run preserves the body bytes.
    bundle.raw_parsed_styles = Some(reader.take(reader.remaining())?);
    Ok(())
}
