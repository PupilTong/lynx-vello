//! `NEW_ELEMENT_TEMPLATE` section placeholder.

use crate::{error::Result, model::TemplateBundle, reader::Reader};

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // Run 2: decode the fiber element-template tree; this run preserves the
    // body bytes.
    bundle.raw_new_element_template = Some(reader.take(reader.remaining())?);
    Ok(())
}
