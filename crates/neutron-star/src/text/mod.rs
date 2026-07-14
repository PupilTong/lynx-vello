//! Optional Parley text measurement core.
//!
//! This module shapes host-assembled [`TextRun`](crate::style::TextRun)
//! sequences, re-breaks retained layouts for intrinsic and definite
//! constraints, and lends their metrics through
//! [`LeafMeasurer`](crate::compute::LeafMeasurer). It owns no widget tree,
//! computed-style storage, resource fetching, box cache, or paint policy.
//!
//! # Host leaf dispatch
//!
//! A real host obtains `container_style` and runs from its immutable source,
//! then borrows `text_context` and the node's artifacts from its mutable
//! session inside `LayoutSession::compute_child_layout`. This compact source /
//! session pair mirrors that dispatch boundary:
//!
//! ```
//! use neutron_star::compute::compute_leaf_layout;
//! use neutron_star::style::{
//!     CoreStyle, FontFamily, FontFeatureSetting, FontVariationSetting, TextContainerStyle,
//!     TextRun, TextRunStyle,
//! };
//! use neutron_star::text::{ArtifactSlots, TextContext, TextMeasurer};
//! use neutron_star::tree::LayoutInput;
//!
//! #[derive(Default)]
//! struct BoxStyle;
//! impl CoreStyle for BoxStyle {}
//! impl TextContainerStyle for BoxStyle {}
//!
//! struct RunStyle;
//! impl TextRunStyle for RunStyle {
//!     type FontFamilies<'a> = core::iter::Once<FontFamily<'a>>;
//!     type FontFeatureSettings<'a> = core::iter::Empty<FontFeatureSetting>;
//!     type FontVariationSettings<'a> = core::iter::Empty<FontVariationSetting>;
//!
//!     fn font_families(&self) -> Self::FontFamilies<'_> {
//!         core::iter::once(FontFamily::Generic(Default::default()))
//!     }
//!     fn font_feature_settings(&self) -> Self::FontFeatureSettings<'_> {
//!         core::iter::empty()
//!     }
//!     fn font_variation_settings(&self) -> Self::FontVariationSettings<'_> {
//!         core::iter::empty()
//!     }
//! }
//!
//! struct TextSource {
//!     container_style: BoxStyle,
//!     run_style: RunStyle,
//!     text: &'static str,
//! }
//!
//! struct TextSession {
//!     text_context: TextContext,
//!     artifacts: ArtifactSlots,
//! }
//!
//! impl TextSession {
//!     // A host calls an equivalent branch from compute_child_layout.
//!     fn compute_text_leaf(
//!         &mut self,
//!         source: &TextSource,
//!         input: LayoutInput,
//!     ) -> neutron_star::tree::LayoutOutput {
//!         let runs = [TextRun {
//!             text: source.text,
//!             style: &source.run_style,
//!             preserve_newlines: false,
//!         }];
//!         let mut measurer = TextMeasurer::new(
//!             &mut self.text_context,
//!             &mut self.artifacts,
//!             &source.container_style,
//!             runs.into_iter(),
//!             |_, _| unreachable!("this style contains no calc()"),
//!         );
//!         compute_leaf_layout(
//!             input,
//!             &source.container_style,
//!             |_, _| unreachable!("this box style contains no calc()"),
//!             &mut measurer,
//!         )
//!     }
//! }
//!
//! let source = TextSource {
//!     container_style: BoxStyle,
//!     run_style: RunStyle,
//!     text: "Hello from a host-owned run",
//! };
//! let mut session = TextSession {
//!     text_context: TextContext::new(),
//!     artifacts: ArtifactSlots::default(),
//! };
//! let output = session.compute_text_leaf(&source, LayoutInput::default());
//! assert!(output.size.width >= 0.0);
//! ```

mod content;
mod context;
mod layout;
mod measure;

pub use context::TextContext;
pub use layout::{ArtifactSlots, TextLayout, TextLayoutView};
pub use measure::TextMeasurer;
