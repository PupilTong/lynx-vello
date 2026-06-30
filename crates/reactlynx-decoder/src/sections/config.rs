//! `CONFIG` section.

use crate::{error::Result, model::TemplateBundle, reader::Reader};

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // C++: DecodeConfigSection reads one inline string
    // (lynx_binary_base_template_reader.cc:62-72).
    let raw_json = reader.lstr()?;
    bundle.page_config = Some(crate::model::PageConfig { raw_json });
    Ok(())
}
