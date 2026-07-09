//! Lynx widget kinds and the tag-name mapping.

/// The kind of a Lynx widget.
///
/// `NoneElement` is Lynx's `<none>` element — spelled that way (rather than
/// `None`) to avoid clashing with `Option::None` at call sites that store a
/// `WidgetKind` inside an `Option`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WidgetKind {
    /// `<page>` — the root element.
    Page,
    /// `<view>` — the generic box container.
    View,
    /// `<text>` — a text container.
    Text,
    /// `<raw-text>` — a leaf holding literal text content.
    RawText,
    /// `<image>`.
    Image,
    /// `<scroll-view>`.
    ScrollView,
    /// `<list>`.
    List,
    /// `<list-item>`.
    ListItem,
    /// `<wrapper>` — a transparent grouping element.
    Wrapper,
    /// `<none>` — Lynx's explicit "no element" element.
    NoneElement,
    /// Any tag not recognised as a built-in Lynx element.
    Unknown,
}

impl WidgetKind {
    /// Classify a Lynx tag name. Unrecognised tags map to [`WidgetKind::Unknown`].
    #[must_use]
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "page" => Self::Page,
            "view" => Self::View,
            "text" => Self::Text,
            "raw-text" => Self::RawText,
            "image" => Self::Image,
            "scroll-view" => Self::ScrollView,
            "list" => Self::List,
            "list-item" => Self::ListItem,
            "wrapper" => Self::Wrapper,
            "none" => Self::NoneElement,
            _ => Self::Unknown,
        }
    }

    /// The canonical Lynx tag name for this kind.
    ///
    /// [`WidgetKind::Unknown`] has no canonical tag (the real tag string lives
    /// in the widget's `tag` field); it reports `"unknown"`.
    #[must_use]
    pub const fn tag_name(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::View => "view",
            Self::Text => "text",
            Self::RawText => "raw-text",
            Self::Image => "image",
            Self::ScrollView => "scroll-view",
            Self::List => "list",
            Self::ListItem => "list-item",
            Self::Wrapper => "wrapper",
            Self::NoneElement => "none",
            Self::Unknown => "unknown",
        }
    }
}
