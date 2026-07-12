//! Lynx-specific layout protocols and host-side algorithms over
//! [`neutron_star`].
//!
//! neutron-star owns the CSS algorithms and generic layout machinery while
//! deliberately leaving display dispatch open. This crate owns the peer
//! algorithms whose vocabulary is specific to Lynx, beginning with
//! `display: linear`. It uses neutron-star's immutable-source/mutable-session
//! split and static dispatch throughout; Lynx modes do not become variants in
//! neutron-star itself.
//!
//! The linear algorithm and its computed-style/source protocols are usable
//! without stylo or widget storage. A concrete `lynx-widget` adapter can be
//! layered on later without changing the algorithm or neutron-star's open
//! dispatch boundary.

mod linear;
pub mod style;
pub mod tree;

pub use linear::compute_linear_layout;
pub use style::{
    LinearContainerStyle, LinearCrossGravity, LinearDirection, LinearGravity, LinearItemStyle,
    LinearLayoutGravity, LinearOrientation,
};
pub use tree::LinearSource;
