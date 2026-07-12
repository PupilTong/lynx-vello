//! The [`ExternalState`] payload trait — the embedder's hooks into the generic
//! DOM.
//!
//! [`Element`](crate::Element) is generic over an external-state payload `T`,
//! carried in its [`ext`](crate::Element::ext) field. The payload is opaque to
//! this crate except through the hooks defined here, which the generic stylo
//! trait impls ([`crate::traits`]) consult wherever embedder-specific data can
//! influence selector matching:
//!
//! - [`is_root`](ExternalState::is_root) — whether the element may match `:root` (combined with
//!   parentlessness).
//! - [`extra_attr_value`](ExternalState::extra_attr_value) /
//!   [`each_extra_attr_name`](ExternalState::each_extra_attr_name) — synthetic or reflected
//!   attributes beyond the element's real [`attrs`](crate::Element::attrs) map.
//!
//! Every hook has a neutral default, so `()` (implemented below) is a valid
//! payload for embedders — and tests — that need none of them.

use stylo::LocalName;

/// The embedder-supplied external state carried by every
/// [`Element`](crate::Element).
///
/// Implementations hold whatever per-element data the embedder needs alongside
/// the HTML-DOM-subset fields, and override the hooks below where that data
/// should participate in selector matching. All hooks default to "no effect".
///
/// `Sync` is required because the restyle traversal may run in parallel:
/// rayon workers call the hooks concurrently through shared references.
pub trait ExternalState: Sync {
    /// Whether this element may match `:root`.
    ///
    /// [`selectors::Element::is_root`] matches an element that is parentless
    /// **and** passes this hook. The default (`true`) keeps the HTML-ish rule
    /// "parentless ⇒ root"; an embedder whose root is a distinguished element
    /// narrows this so a detached subtree's parentless top does not match
    /// `:root` during resolve.
    fn is_root(&self) -> bool {
        true
    }

    /// The value of a synthetic or reflected attribute named `name`, if any.
    ///
    /// Consulted by attribute matching ([`selectors::Element::attr_matches`]
    /// and [`TElement::get_attr`](stylo::dom::TElement::get_attr)) only
    /// **after** the element's real [`attrs`](crate::Element::attrs) map misses
    /// `name`. The default exposes nothing.
    fn extra_attr_value(&self, _name: &str) -> Option<String> {
        None
    }

    /// Enumerate the attribute names [`extra_attr_value`] can serve.
    ///
    /// Feeds the bloom-filter attribute-name enumeration
    /// ([`TElement::each_attr_name`](stylo::dom::TElement::each_attr_name)),
    /// which must account for every synthetic/reflected attribute. The default
    /// yields nothing.
    ///
    /// [`extra_attr_value`]: ExternalState::extra_attr_value
    fn each_extra_attr_name(&self, _callback: &mut dyn FnMut(&LocalName)) {}
}

/// The no-op payload: every hook keeps its neutral default. Used by this
/// crate's own tests and by embedders that need no external state.
impl ExternalState for () {}
