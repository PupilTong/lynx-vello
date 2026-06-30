//! The decoded data model produced by [`crate::decode_template`].
//!
//! All borrowed data is tied to the input buffer lifetime so section payloads
//! can stay zero-copy until deeper decoders are implemented.

pub mod element;
pub mod style;

pub use element::{
    AttributeBinding, AttributeBindingType, ElementBuiltInAttribute, ElementBuiltInTag,
    ElementNode, ElementSection, ElementTag, ElementTemplates, EventBinding, EventType,
    ParsedStyleEntry, PiperEventBinding,
};
pub use style::{CssDescriptor, ParsedStyles, StyleObjects};

use crate::{value::Value, version::Version};

/// A fully decoded native template bundle.
#[derive(Debug, Clone)]
pub struct TemplateBundle<'a> {
    /// The raw bundle bytes the rest of the model borrows from.
    pub raw: &'a [u8],
    /// Header fields and version strings.
    pub header: Header<'a>,
    /// Compile options decoded from `header_ext_info`.
    pub compile_options: CompileOptions<'a>,
    /// Header-mode template info value (`target_sdk >= 2.7`).
    pub template_info: Option<Value<'a>>,
    /// Application kind from the header.
    pub app_type: AppType,
    /// Snapshot flag byte. The C++ reader decodes it and otherwise ignores it.
    pub snapshot: bool,
    /// Page config JSON from the `CONFIG` section.
    pub page_config: Option<PageConfig<'a>>,
    /// JavaScript source assets.
    pub js_sources: Vec<JsSource<'a>>,
    /// Custom section payloads.
    pub custom_sections: Vec<CustomSection<'a>>,
    /// Decoded fiber element templates from `NEW_ELEMENT_TEMPLATE`.
    pub element_templates: ElementTemplates<'a>,
    /// Run 2: decode `CSS`; for now this is the raw section body.
    pub raw_css: Option<&'a [u8]>,
    /// Run 2: decode `STYLE_OBJECT`; for now this is the raw section body.
    pub raw_style_object: Option<&'a [u8]>,
    /// Run 2: decode `PARSED_STYLES`; for now this is the raw section body.
    pub raw_parsed_styles: Option<&'a [u8]>,
}

impl<'a> TemplateBundle<'a> {
    /// Create an empty bundle shell after the container header has been read.
    #[must_use]
    pub fn new(
        raw: &'a [u8],
        header: Header<'a>,
        compile_options: CompileOptions<'a>,
        template_info: Option<Value<'a>>,
        app_type: AppType,
        snapshot: bool,
    ) -> Self {
        Self {
            raw,
            header,
            compile_options,
            template_info,
            app_type,
            snapshot,
            page_config: None,
            js_sources: Vec::new(),
            custom_sections: Vec::new(),
            element_templates: ElementTemplates::default(),
            raw_css: None,
            raw_style_object: None,
            raw_parsed_styles: None,
        }
    }
}

/// Top-level header fields.
#[derive(Debug, Clone)]
pub struct Header<'a> {
    /// Declared total bundle size.
    pub total_size: u32,
    /// Magic word. The supported value is `0x00241922`.
    pub magic: u32,
    /// Deprecated Lepus version string.
    pub lepus_version: &'a str,
    /// Deprecated CLI version string, present for modern bundles.
    pub cli_version: Option<&'a str>,
    /// Deprecated iOS version string; this is the target SDK in practice.
    pub ios_version: Option<&'a str>,
    /// Deprecated Android version string.
    pub android_version: Option<&'a str>,
    /// Parsed target SDK used for version gates.
    pub target_sdk: Version,
}

/// Compile options carried by the `header_ext_info` block.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct CompileOptions<'a> {
    /// Parsed target SDK from compile options or the legacy header.
    pub target_sdk: Version,
    /// Raw target SDK string.
    pub target_sdk_str: &'a str,
    /// Whether the template uses fiber architecture.
    pub enable_fiber_arch: bool,
    /// Whether the body uses a route table.
    pub enable_flexible_template: bool,
    /// Whether parsed CSS values are enabled.
    pub enable_css_parser: bool,
    /// Whether CSS variables are enabled.
    pub enable_css_variable: bool,
    /// Whether standard CSS selectors are enabled.
    pub enable_css_selector: bool,
    /// Whether simple styling / `STYLE_OBJECT` is enabled.
    pub enable_simple_styling: bool,
    /// Whether a trial-options value follows template info in the header.
    pub enable_trial_options: bool,
    /// Whether rule-based CSS is enabled. This is derived from page config later.
    pub enable_css_rule: bool,
    /// Architecture option from header-ext key 28 (`FIBER_ARCH` is 1).
    pub arch_option: u8,
    /// Raw decoded header-ext fields for options this run does not model yet.
    pub raw_fields: Vec<CompileOptionField<'a>>,
}

impl<'a> CompileOptions<'a> {
    /// Defaults mirror `CompileOptions` in the C++ codec.
    #[must_use]
    pub fn defaults(target_sdk_str: &'a str) -> Self {
        Self {
            target_sdk: Version::parse(target_sdk_str),
            target_sdk_str,
            enable_fiber_arch: false,
            enable_flexible_template: false,
            enable_css_parser: false,
            enable_css_variable: true,
            enable_css_selector: false,
            enable_simple_styling: false,
            enable_trial_options: false,
            enable_css_rule: false,
            arch_option: 0,
            raw_fields: Vec::new(),
        }
    }
}

/// A raw field from the `header_ext_info` map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompileOptionField<'a> {
    /// Header-ext value type.
    pub field_type: u8,
    /// Compile-options key id.
    pub key_id: u8,
    /// Absolute payload offset in the original buffer.
    pub payload_offset: usize,
    /// Raw payload bytes.
    pub payload: &'a [u8],
}

/// The application type string mapped the same way as the C++ reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppType {
    /// `"card"` and all unknown strings.
    Card,
    /// `"DynamicComponent"`.
    DynamicComponent,
}

/// Page configuration JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageConfig<'a> {
    /// Raw inline JSON string.
    pub raw_json: &'a str,
}

/// JavaScript source entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsSource<'a> {
    /// Source path.
    pub path: &'a str,
    /// JavaScript source text.
    pub content: &'a str,
}

/// Decoded custom section.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomSection<'a> {
    /// Section key.
    pub key: &'a str,
    /// Header metadata value.
    pub header: Value<'a>,
    /// Decoded content value.
    pub content: Value<'a>,
}
