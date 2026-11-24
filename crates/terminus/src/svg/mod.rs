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
//! - `ua`: the user-agent rules SVG content needs, in one sheet a document mounts explicitly.

mod attributes;
pub(crate) mod presentation;
pub(crate) mod root;
pub(crate) mod ua;

/// The SVG namespace.
pub(crate) const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

/// The local name of SVG's `svg` element, which `Document::create_element`
/// creates in the SVG namespace and `Document::define` refuses.
pub(crate) const SVG_TAG: &str = "svg";
