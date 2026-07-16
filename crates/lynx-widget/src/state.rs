//! The Lynx external state carried by every widget ([`WidgetState`]), plus the
//! event-registration types.
//!
//! `w3c-dom`'s [`Node`](w3c_dom::Node) covers only the W3C-DOM
//! subset; everything Lynx-specific about a widget — its [`WidgetKind`], the
//! `unique_id`, the `css_id` style scope, the `data-*` dataset, and event
//! bindings — lives here, in the node's `ext` payload. The
//! [`ExternalState`] impl is what feeds the Lynx-specific bits back into
//! selector matching (`:root` = the `<page>` kind, the synthetic `l-css-id`
//! attribute, `data-*` dataset reflection).

use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use stylo::LocalName;
use w3c_dom::ExternalState;

use crate::kind::WidgetKind;

/// Which bind/catch channel an event handler was authored on.
///
/// These mirror Lynx's five mutually-exclusive event attribute namespaces
/// (`bindEvent`, `catchEvent`, `capture-bind`, `capture-catch`,
/// `global-bindEvent`). The phase/propagation semantics they imply are the
/// runtime's concern; here they are stored verbatim.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventKind {
    /// `bind*` — bubble-phase listener.
    Bind,
    /// `catch*` — bubble-phase listener that also halts propagation.
    Catch,
    /// `capture-bind*` — capture-phase listener.
    CaptureBind,
    /// `capture-catch*` — capture-phase listener that also halts propagation.
    CaptureCatch,
    /// `global-bind*` — page/component-wide listener, no capture/bubble phase.
    GlobalBind,
}

/// A single event binding on a widget.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EventReg {
    /// The event name (e.g. `"tap"`).
    pub name: Box<str>,
    /// The channel this handler was authored on.
    pub kind: EventKind,
    /// Opaque handler identifier. A later runtime crate resolves this to an
    /// actual callback; for now it is stored as an uninterpreted string.
    pub handler: Box<str>,
}

/// The Lynx-specific per-widget state, carried as the `ext` payload of
/// [`Widget`](crate::Widget) (= `w3c_dom::Node<WidgetState>`).
#[derive(Debug)]
pub struct WidgetState {
    /// The widget kind (the Lynx tag classification).
    pub kind: WidgetKind,
    /// The Lynx `unique_id`, assigned by the
    /// [`WidgetTree`](crate::WidgetTree) on creation (1-based, monotonically
    /// increasing).
    pub unique_id: i32,
    /// The `css_id` scoping this widget's styles: `0` means unset / global.
    /// Stamped directly by Lynx's `__SetCSSId` (there is no `<component>`
    /// element in this engine to inherit it from). It is also exposed to
    /// selector matching as the synthetic `l-css-id` attribute (see the
    /// [`ExternalState`] impl below).
    pub css_id: i32,
    /// `data-*` dataset entries (keys stored without the `data-` prefix),
    /// reflected as `data-*` attributes via the [`ExternalState`] hooks.
    pub dataset: FxHashMap<Box<str>, String>,
    /// Event bindings on this widget.
    pub events: SmallVec<[EventReg; 2]>,
}

impl WidgetState {
    /// Create the state for a freshly created widget of `kind` with the given
    /// Lynx `unique_id`.
    #[must_use]
    pub fn new(kind: WidgetKind, unique_id: i32) -> Self {
        Self {
            kind,
            unique_id,
            css_id: 0,
            dataset: FxHashMap::default(),
            events: SmallVec::new(),
        }
    }
}

impl ExternalState for WidgetState {
    fn is_root(&self) -> bool {
        // `:root` is exactly the `<page>` element (web-core rewrites `:root`
        // to the page part). Checking the kind — not just parentlessness —
        // keeps a detached subtree's root from matching `:root` during
        // resolve.
        self.kind == WidgetKind::Page
    }

    fn extra_attr_value(&self, name: &str) -> Option<String> {
        if name == "l-css-id" {
            // The synthetic `l-css-id` attribute exposes the widget's style
            // scope for the future scoped-CSS mode.
            return Some(self.css_id.to_string());
        }
        // web-core reflects dataset entries as `data-*` attributes (DOM
        // dataset reflection), so `[data-x]` selectors must see them too.
        // Keys are matched verbatim after the `data-` prefix; camelCase↔kebab
        // reflection is revisited with StyleInfo ingestion (M3) if ReactLynx
        // turns out to emit camelCase dataset keys.
        name.strip_prefix("data-")
            .and_then(|key| self.dataset.get(key).cloned())
    }

    fn each_extra_attr_name(&self, callback: &mut dyn FnMut(&LocalName)) {
        // Dataset entries are reflected as `data-*` attributes (web-core
        // parity; see `extra_attr_value`).
        for key in self.dataset.keys() {
            callback(&LocalName::from(format!("data-{key}").as_str()));
        }
        // Expose the synthetic `l-css-id` attribute (see `extra_attr_value`)
        // so the bloom filter accounts for it in the future scoped-CSS mode.
        callback(&LocalName::from("l-css-id"));
    }
}
