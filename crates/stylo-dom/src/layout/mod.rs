//! DOM-owned layout integration for neutron-star.
//!
//! [`DomLayoutSource`] lends an immutable formatting-tree projection and
//! [`DomLayoutSession`] owns the disjoint mutable box caches, rounded layouts,
//! Parley context, and retained text artifacts. Ordinary CSS flow currently
//! uses neutron-star's single-axis Linear algorithm as an explicit fallback;
//! that branch is not CSS Block or Inline Layout conformance.

mod session;
mod source;
mod style;

pub use session::DomLayoutSession;
pub use source::{
    DomLayoutDisplay, DomLayoutSource, DomLayoutSourceError, DomLayoutStyle, DomTextRuns,
    LayoutNodePolicy, LayoutNodeRole, OptionalStyleIter,
};
pub use style::{
    ComputedFontFamilies, ComputedFontFeatureSettings, ComputedFontVariationSettings,
    ComputedGridRepetition, ComputedGridTemplateTracks, ComputedGridTrackSlice, ComputedGridTracks,
    ComputedLayoutStyle, ComputedTextRunStyle, LayoutDisplay,
};
