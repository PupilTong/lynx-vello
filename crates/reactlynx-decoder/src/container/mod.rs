//! Bundle envelope: header -> compile options -> section route -> section bodies.

mod compile_options;
mod header;
mod header_ext_info;
mod section_route;

use crate::{
    error::Result,
    model::{AppType, TemplateBundle},
    reader::Reader,
};

/// Decode a complete bundle from `buf`.
pub(crate) fn decode_bundle(buf: &[u8]) -> Result<TemplateBundle<'_>> {
    let mut reader = Reader::new(buf);
    let header::DecodedHeader {
        header,
        compile_options,
        template_info,
    } = header::decode(&mut reader)?;

    let app_type_raw = reader.lstr()?;
    let app_type = if app_type_raw == "DynamicComponent" {
        AppType::DynamicComponent
    } else {
        AppType::Card
    };
    let snapshot = reader.bool()?;

    let mut bundle = TemplateBundle::new(
        buf,
        header,
        compile_options,
        template_info,
        app_type,
        snapshot,
    );
    section_route::decode_body(&reader, &mut bundle)?;
    Ok(bundle)
}
