//! `CONFIG` section.

use crate::{error::Result, model::TemplateBundle, reader::Reader};

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // C++: DecodeConfigSection reads one inline string
    // (lynx_binary_base_template_reader.cc:62-72).
    let raw_json = reader.lstr()?;
    bundle.compile_options.enable_css_rule = serde_json::from_str::<serde_json::Value>(raw_json)
        .ok()
        .and_then(|value| {
            value
                .get("enableCSSRule")
                .and_then(serde_json::Value::as_bool)
        })
        .unwrap_or(false);
    bundle.page_config = Some(crate::model::PageConfig { raw_json });
    Ok(())
}
