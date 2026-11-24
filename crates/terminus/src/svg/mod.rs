//! SVG-namespace elements in the document tree.
//!
//! An element is in the HTML namespace or in the SVG namespace
//! ([`SVG_NAMESPACE`]), one node bit set at creation
//! (`Node::in_svg_namespace`). `Document::create_element("svg", …)` makes
//! the SVG `svg` element, as HTML's tree construction does for an `svg`
//! start tag in HTML content; every other tag is HTML. An SVG element keeps
//! its local name's case, matches type and attribute selectors
//! case-sensitively (it is not an HTML element in an HTML document), answers
//! the SVG namespace to `@namespace` selectors, and never takes a custom
//! element definition.
//!
//! What this module adds on top of the tree:
//!
//! - [`presentation`]: SVG presentation attributes, reflected into the element's
//!   presentational-hint declaration block on every `set_attribute` and `remove_attribute`, so
//!   stylo cascades them at the presentational-hint origin (below author rules and the `style`
//!   attribute).
//! - [`root`]: every SVG `svg` element is replaced content whose natural size follows its `width`,
//!   `height` and `viewBox` attributes; its descendants generate no box.
//! - `ua`: the user-agent rules SVG content needs, in one sheet a document mounts explicitly, and
//!   the sheet of an isolated SVG image document.
//! - [`build`], [`use_element`]: markup parsed by the tree's XML layer ([`crate::tree::xml`]) and
//!   built into an SVG subtree with it, then its `style` elements mounted as document sheets and
//!   its `use` elements instantiated; `href` (`attributes`) reads an element's `href`, else its
//!   `xlink:href`. [`image`] decides how a document's markup is read (an element in no namespace is
//!   SVG; the root must be `svg`).
//! - [`record`]: a cascaded subtree as a [`recording::Recording`], every value from computed style;
//!   [`gradient`] is its paint-server half and [`paint_server`] the brush a resolved gradient
//!   paints with, `layers` its group effects, [`shapes`] the basic shapes and path data,
//!   [`attributes`] the attribute values it reads, [`text`] the text chunk model, its whitespace
//!   rule and the shaping into the recording. Drawing a recording is `paint::svg`'s.
//! - [`image`]: an SVG document used as an image, built as an isolated document, recorded, and
//!   planned and painted as an inline root's recording is.
//! - [`present`]: an outermost `svg` element draws its recorded subtree into its box, recorded
//!   again when something under it changes.

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod animation_tests;
mod attributes;
pub(crate) mod build;
pub(crate) mod gradient;
pub(crate) mod image;
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod inline_tests;
mod layers;
pub(crate) mod paint_server;
pub(crate) mod present;
pub(crate) mod presentation;
pub(crate) mod record;
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod record_tests;
pub(crate) mod recording;
pub(crate) mod root;
pub(crate) mod shapes;
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
pub(crate) mod text;
pub(crate) mod ua;
pub(crate) mod use_element;

pub(crate) use layers::ink_extent;
pub(crate) use record::context_matrix;

/// The SVG namespace.
pub(crate) const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

/// The local name of SVG's `svg` element, which `Document::create_element`
/// creates in the SVG namespace and `Document::define` refuses.
pub(crate) const SVG_TAG: &str = "svg";

/// The local name of SVG's `use` element.
pub(crate) const USE_TAG: &str = "use";

/// Whether `node` is SVG's `use` element.
pub(crate) fn is_use<T>(node: &crate::tree::node::Node<T>) -> bool {
    node.in_svg_namespace() && node.tag_name() == Some(USE_TAG)
}
