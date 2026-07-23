//! Lynx widget kinds and the tag-name mapping.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WidgetKind {
    Page,
    View,
    Text,
    RawText,
    Image,
    ScrollView,
    List,
    ListItem,
    Wrapper,
    NoneElement,
    Unknown,
}

impl WidgetKind {
    #[must_use]
    pub fn from_tag_name(tag: &str) -> Self {
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
