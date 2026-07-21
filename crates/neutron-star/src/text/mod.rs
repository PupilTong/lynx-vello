//! Parley text measurement core.
//!
//! This module shapes host-assembled [`TextRun`](crate::style::TextRun)
//! sequences, re-breaks retained layouts for intrinsic and definite
//! constraints, and feeds the fixed Parley path into leaf box layout. It owns no widget tree,
//! computed-style storage, resource fetching, box cache, or paint policy.
//!
//! # Host leaf dispatch
//!
//! A real host obtains `container_style` and runs from its immutable node
//! data, then borrows `text_context` and the node's artifacts from
//! interior-mutable slots inside
//! [`LayoutNode::compute_child_layout`](crate::tree::LayoutNode::compute_child_layout);
//! the borrows are node-scoped and end before the cache wrapper stores the
//! result. This compact example mirrors that dispatch boundary:
//!
//! ```
//! use std::cell::RefCell;
//!
//! use neutron_star::style::{
//!     CoreStyle, Display, FontFamily, TextContainerStyle, TextRun, TextRunStyle,
//! };
//! use neutron_star::text::{ArtifactSlots, TextContext, TextMeasurer};
//! use neutron_star::tree::LayoutInput;
//! use stylo::values::computed::font::GenericFontFamily;
//!
//! #[derive(Default)]
//! struct BoxStyle;
//! impl CoreStyle for BoxStyle {
//!     fn display(&self) -> Display {
//!         Display::Flex
//!     }
//! }
//! impl TextContainerStyle for BoxStyle {}
//!
//! struct RunStyle;
//! impl TextRunStyle for RunStyle {
//!     fn font_family(&self) -> FontFamily {
//!         FontFamily::generic(GenericFontFamily::SansSerif).clone()
//!     }
//! }
//!
//! /// One text node: immutable epoch data plus the node's interior-mutable
//! /// artifact slot, exactly as a host embeds them on its tree nodes.
//! struct TextNode {
//!     container_style: BoxStyle,
//!     run_style: RunStyle,
//!     text: &'static str,
//!     artifacts: RefCell<ArtifactSlots>,
//! }
//!
//! // A host runs an equivalent arm inside LayoutNode::compute_child_layout:
//! // node-scoped RefMut borrows feed the measurer and drop with it, before
//! // the cache wrapper stores the result. `text_context` lives in a
//! // tree-level slot.
//! fn compute_text_leaf(
//!     node: &TextNode,
//!     text_context: &RefCell<TextContext>,
//!     input: LayoutInput,
//! ) -> neutron_star::tree::LayoutOutput {
//!     let runs = [TextRun {
//!         text: node.text,
//!         style: &node.run_style,
//!         preserve_newlines: false,
//!     }];
//!     let mut context = text_context.borrow_mut();
//!     let mut artifacts = node.artifacts.borrow_mut();
//!     let mut measurer = TextMeasurer::new(
//!         &mut context,
//!         &mut artifacts,
//!         &node.container_style,
//!         runs.into_iter(),
//!     );
//!     measurer.compute_layout(input)
//! }
//!
//! let node = TextNode {
//!     container_style: BoxStyle,
//!     run_style: RunStyle,
//!     text: "Hello from a host-owned run",
//!     artifacts: RefCell::new(ArtifactSlots::default()),
//! };
//! let text_context = RefCell::new(TextContext::new());
//! let output = compute_text_leaf(&node, &text_context, LayoutInput::default());
//! assert!(output.size.width >= 0.0);
//! ```

mod content;
mod context;
mod layout;
mod measure;

pub use context::TextContext;
pub use layout::{ArtifactSlots, TextLayout, TextLayoutView};
pub use measure::TextMeasurer;
