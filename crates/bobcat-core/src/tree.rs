//! Lynx page policy over the generic document: the `page` root tag, the UA
//! cascade defaults, the components the engine defines, and view metrics.
//! Everything else the runtime does goes
//! straight to [`dom::Document`] — element identity is the DOM [`NodeId`],
//! which is also the element's Lynx `unique_id`: one number, issued by the
//! DOM, never reissued after the element is freed. Script therefore cannot
//! name a stranger by holding an id too long, only something that no longer
//! exists. The private host boundary still validates script-provided IDs and
//! mutation preconditions before entering `dom`, returning misuse as a
//! JavaScript error.
//!
//! [`NodeId`]: dom::NodeId

pub(crate) mod raw_text;

use dom::{Document, StylesheetOrigin};

/// The one document shape the runtime speaks.
pub(crate) type LynxDocument = Document<()>;

pub(crate) const PAGE_TAG: &str = "page";

/// Page configuration that controls the Lynx UA cascade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageConfig {
    /// Whether elements default to `display: linear`.
    pub default_display_linear: bool,
    /// Whether elements default to visible overflow.
    pub default_overflow_visible: bool,
    /// Whether author CSS selector matching is enabled.
    pub enable_css_selector: bool,
}

impl Default for PageConfig {
    fn default() -> Self {
        Self {
            default_display_linear: true,
            default_overflow_visible: true,
            enable_css_selector: true,
        }
    }
}

/// The Lynx UA stylesheet: embedder cascade policy `dom` must not know.
///
/// `text` is a flex container whatever `defaultDisplayLinear` says, and
/// `wrapper` generates no box — both from `web-elements`' own sheet, where
/// the linear toggle covers container tags only. Where the runs a `text`
/// carries lay out is [`raw_text::UA_RULES`].
#[must_use]
fn ua_stylesheet(config: PageConfig) -> String {
    let display = if config.default_display_linear {
        "display: linear;"
    } else {
        ""
    };
    let overflow = if config.default_overflow_visible {
        ""
    } else {
        "overflow: hidden;"
    };
    format!(
        "page, view {{ box-sizing: border-box; {display} {overflow} }}\n\
         page {{ width: 100%; height: 100%; }}\n\
         wrapper {{ display: contents; }}\n\
         text {{ box-sizing: border-box; display: flex; }}\n\
         {rules}",
        rules = raw_text::UA_RULES,
    )
}

/// A viewport measured in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Viewport {
    /// Viewport width in CSS pixels.
    pub width: f32,
    /// Viewport height in CSS pixels.
    pub height: f32,
    /// Physical pixels per CSS pixel.
    pub device_pixel_ratio: f32,
}

impl Viewport {
    /// Creates a viewport with a device-pixel ratio of 1.
    #[must_use]
    pub(crate) const fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            device_pixel_ratio: 1.0,
        }
    }

    #[must_use]
    /// Returns this viewport with a new device-pixel ratio.
    pub(crate) const fn with_device_pixel_ratio(mut self, device_pixel_ratio: f32) -> Self {
        self.device_pixel_ratio = device_pixel_ratio;
        self
    }

    fn device(self) -> dom::Device {
        dom::Device::new(self.width, self.height, self.device_pixel_ratio)
    }
}

/// Creates the document with its permanent `page` element, the components the
/// engine defines, and the UA cascade.
#[must_use]
pub(crate) fn new_document(viewport: Viewport, config: PageConfig) -> LynxDocument {
    let mut document = Document::new(viewport.device(), PAGE_TAG, ());
    raw_text::define(&mut document);
    document.add_stylesheet(&ua_stylesheet(config), StylesheetOrigin::UserAgent);
    document
}

#[cfg(test)]
mod tests {
    use super::{LynxDocument, PageConfig, Viewport, new_document, ua_stylesheet};

    fn document() -> LynxDocument {
        new_document(Viewport::new(393.0, 727.0), PageConfig::default())
    }

    #[test]
    fn a_layout_pass_sizes_the_page_to_the_viewport() {
        let mut document = document();
        let page = document.document_element().id();
        document.layout();
        let layout = document
            .rounded_layout(page)
            .expect("the page is laid out after the pass");
        assert!((layout.size.width - 393.0).abs() < f32::EPSILON);
        assert!((layout.size.height - 727.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_ua_sheet_gives_every_element_lynx_defaults() {
        let mut document = document();
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.layout();

        let style = document.get(view).unwrap().computed_style().unwrap();
        assert_eq!(
            style.clone_box_sizing(),
            dom::stylo::computed_values::box_sizing::T::BorderBox
        );
        assert_eq!(
            style.clone_display(),
            dom::stylo::values::computed::Display::Linear
        );
    }

    #[test]
    fn the_display_page_config_switch_reaches_computed_style() {
        let mut document = new_document(
            Viewport::new(393.0, 727.0),
            PageConfig {
                default_display_linear: false,
                ..PageConfig::default()
            },
        );
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.layout();

        let style = document.get(view).unwrap().computed_style().unwrap();
        assert_eq!(
            style.clone_display(),
            dom::stylo::values::computed::Display::Flex
        );
        assert_eq!(
            style.clone_box_sizing(),
            dom::stylo::computed_values::box_sizing::T::BorderBox
        );
    }

    #[test]
    fn the_overflow_page_config_switch_reaches_computed_style() {
        for (visible, expected) in [
            (true, dom::stylo::values::computed::Overflow::Visible),
            (false, dom::stylo::values::computed::Overflow::Hidden),
        ] {
            let mut document = new_document(
                Viewport::new(393.0, 727.0),
                PageConfig {
                    default_overflow_visible: visible,
                    ..PageConfig::default()
                },
            );
            let page = document.document_element().id();
            let view = document.create_element("view", ());
            document.insert_before(page, view, None);
            document.layout();

            let style = document.get(view).unwrap().computed_style().unwrap();
            assert_eq!(style.clone_overflow_x(), expected, "visible={visible}");
            assert_eq!(style.clone_overflow_y(), expected, "visible={visible}");
        }
    }

    #[test]
    fn default_config_is_linear_and_overflow_visible() {
        let config = PageConfig::default();
        assert!(config.default_display_linear);
        assert!(config.default_overflow_visible);

        let sheet = ua_stylesheet(config);
        assert!(sheet.contains("display: linear;"));
        assert!(!sheet.contains("overflow: hidden;"));
        assert!(sheet.contains("box-sizing: border-box;"));
    }

    #[test]
    fn ua_switches_drop_the_declarations_they_gate() {
        let sheet = ua_stylesheet(PageConfig {
            default_display_linear: false,
            default_overflow_visible: false,
            enable_css_selector: true,
        });
        assert!(!sheet.contains("display: linear;"));
        assert!(sheet.contains("overflow: hidden;"));
    }
}
