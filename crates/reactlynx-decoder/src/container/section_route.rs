//! Flexible section route decoding and canonical-order dispatch.

use crate::{
    error::{DecodeError, Result},
    model::TemplateBundle,
    reader::Reader,
    sections,
};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinarySection {
    String = 0,
    Css = 1,
    Component = 2,
    Page = 3,
    App = 4,
    Js = 5,
    Config = 6,
    DynamicComponent = 7,
    Themed = 8,
    UsingDynamicComponentInfo = 9,
    SectionRoute = 10,
    RootLepus = 11,
    ElementTemplate = 12,
    ParsedStyles = 13,
    JsBytecode = 14,
    LepusChunk = 15,
    CustomSections = 16,
    NewElementTemplate = 17,
    StyleObject = 18,
}

impl TryFrom<u8> for BinarySection {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::String),
            1 => Ok(Self::Css),
            2 => Ok(Self::Component),
            3 => Ok(Self::Page),
            4 => Ok(Self::App),
            5 => Ok(Self::Js),
            6 => Ok(Self::Config),
            7 => Ok(Self::DynamicComponent),
            8 => Ok(Self::Themed),
            9 => Ok(Self::UsingDynamicComponentInfo),
            10 => Ok(Self::SectionRoute),
            11 => Ok(Self::RootLepus),
            12 => Ok(Self::ElementTemplate),
            13 => Ok(Self::ParsedStyles),
            14 => Ok(Self::JsBytecode),
            15 => Ok(Self::LepusChunk),
            16 => Ok(Self::CustomSections),
            17 => Ok(Self::NewElementTemplate),
            18 => Ok(Self::StyleObject),
            other => Err(DecodeError::BadSectionType(other)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RouteEntry {
    section: BinarySection,
    start: usize,
    end: usize,
}

const FIBER_ORDER: &[BinarySection] = &[
    BinarySection::String,
    BinarySection::ParsedStyles,
    BinarySection::ElementTemplate,
    BinarySection::Css,
    BinarySection::StyleObject,
    BinarySection::Js,
    BinarySection::Config,
    BinarySection::CustomSections,
    BinarySection::NewElementTemplate,
];

pub(super) fn decode_body<'a>(reader: &Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    if !bundle.compile_options.enable_flexible_template {
        return Err(DecodeError::Malformed(
            "non-flexible template body is out of scope",
        ));
    }

    let mut body = reader.clone();
    let routes = decode_route(&mut body)?;
    for section in FIBER_ORDER {
        if let Some(route) = routes.iter().find(|entry| entry.section == *section) {
            dispatch_route(reader, *route, bundle)?;
        }
    }
    Ok(())
}

fn decode_route(reader: &mut Reader<'_>) -> Result<Vec<RouteEntry>> {
    // C++: DecodeSectionRoute reads a route type byte, count, then
    // (section,start,end) and rebases by the post-route offset
    // (lynx_binary_base_template_reader_impl.cc:395-417).
    let _section_route_type = reader.u8()?;
    let count = reader.compact_u32()? as usize;
    let mut routes = Vec::new();
    routes
        .try_reserve(count)
        .map_err(|_| DecodeError::Malformed("section route is too large"))?;
    for _ in 0..count {
        let section = BinarySection::try_from(reader.u8()?)?;
        let start = reader.compact_u32()? as usize;
        let end = reader.compact_u32()? as usize;
        routes.push(RouteEntry {
            section,
            start,
            end,
        });
    }

    let base = reader.pos();
    for route in &mut routes {
        route.start = route
            .start
            .checked_add(base)
            .ok_or(DecodeError::Malformed("section route start overflow"))?;
        route.end = route
            .end
            .checked_add(base)
            .ok_or(DecodeError::Malformed("section route end overflow"))?;
        if route.start > route.end || route.end > reader.len() {
            return Err(DecodeError::Malformed("section route out of bounds"));
        }
    }
    Ok(routes)
}

fn dispatch_route<'a>(
    root: &Reader<'a>,
    route: RouteEntry,
    bundle: &mut TemplateBundle<'a>,
) -> Result<()> {
    if route.section == BinarySection::ElementTemplate {
        return Err(DecodeError::LegacySection(route.section as u8));
    }

    let mut section = root.sub(route.start, route.end)?;
    let actual = BinarySection::try_from(section.u8()?)?;
    if actual != route.section {
        return Err(DecodeError::Malformed("section tag does not match route"));
    }

    match route.section {
        BinarySection::String
        | BinarySection::Component
        | BinarySection::Page
        | BinarySection::App
        | BinarySection::DynamicComponent
        | BinarySection::Themed
        | BinarySection::UsingDynamicComponentInfo
        // Bytecode sections are intentionally not decoded (source-only scope):
        // JS_BYTECODE (compiled JS), ROOT_LEPUS and LEPUS_CHUNK (compiled
        // LepusNG). They are recognized so the route still parses, then skipped
        // by range. Only JS source (section 5) is decoded.
        | BinarySection::SectionRoute
        | BinarySection::JsBytecode
        | BinarySection::RootLepus
        | BinarySection::LepusChunk => Ok(()),
        BinarySection::Css => sections::css::decode(&mut section, bundle),
        BinarySection::StyleObject => sections::style_object::decode(&mut section, bundle),
        BinarySection::Js => sections::js::decode_source(&mut section, bundle),
        BinarySection::Config => sections::config::decode(&mut section, bundle),
        BinarySection::CustomSections => sections::custom::decode(&mut section, bundle),
        BinarySection::NewElementTemplate => {
            sections::element_template::decode(&mut section, bundle)
        }
        BinarySection::ParsedStyles => sections::parsed_styles::decode(&mut section, bundle),
        BinarySection::ElementTemplate => unreachable!("handled before dispatch"),
    }
}
