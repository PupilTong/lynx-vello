//! DOM selector queries — `matches`, `closest`, `querySelector`, and
//! `querySelectorAll` — answered against the node tree.
//!
//! The matching is Stylo's [`dom_apis`], the same generic implementation Gecko
//! reaches through `Servo_SelectorList_QueryFirst`/`QueryAll` (see
//! `nsINode::QuerySelector`) and Servo reaches through
//! `scope_match_a_selectors_string`. It is generic over the `TElement`/
//! `Element` traits [`Node`] already implements for the cascade, so a query
//! resolves selectors exactly the way a stylesheet does — there is no second
//! matcher here to drift from the first. Blitz's `blitz-dom` delegates the same
//! way; the parse step is Servo's `SelectorParser::parse_author_origin_no_namespace`
//! in all three.
//!
//! Queries never run the invalidation-driven path
//! ([`MayUseInvalidation::No`]), which is what Gecko
//! (`const bool useInvalidation = false`) and Servo both pass: every query is a
//! pre-order walk of the light tree with the full selector list matched at each
//! element. That path is also the one whose scoping is unconditionally correct
//! — `dom_apis` only reaches for invalidation when the root is a document or
//! shadow root, and warns there that a scoped `#a div` query needs extra work
//! it does not do.
//!
//! Two indexes the browsers keep are deliberately absent, and [`dom_apis`]
//! degrades to correct-but-linear behavior without either:
//!
//! - **No id map.** [`TDocument::elements_with_id`](stylo::dom::TDocument::elements_with_id) keeps
//!   its default `Err(())`, so `#id` is a subtree walk filtered by `has_id` rather than a hash
//!   lookup. Gecko and Servo maintain that index for `getElementById` and fragment navigation
//!   anyway; here it would be a new side table on every mutation path, which this crate does not
//!   add without a benchmark that asks for it.
//! - **No subtree bloom filter.**
//!   [`TElement::subtree_bloom_filter`](stylo::dom::TElement::subtree_bloom_filter) keeps its
//!   default all-ones, so the `RejectSkippingChildren` subtree skip never fires and a query visits
//!   every descendant.
//!
//! One parse-level gap is shared with the cascade rather than specific to
//! queries: the vendored Servo selector parser keeps `parse_has` and
//! `parse_nth_child_of` disabled, so `:has(...)` and `:nth-child(An+B of S)`
//! — which current browsers parse and match — are reported as
//! [`InvalidSelector`] here, exactly as a stylesheet rule using them is
//! dropped at parse time.
//!
//! Shadow trees follow the spec by construction: a shadow root is not among its
//! host's children, so a pre-order walk of the node tree never descends into
//! one. Rooting a query at the shadow root itself queries that tree instead.

use std::fmt;

use selectors::SelectorList;
use stylo::context::QuirksMode;
use stylo::dom::TDocument;
use stylo::dom_apis::{self, MayUseInvalidation, QueryAll, QueryFirst};
use stylo::selector_parser::{SelectorImpl, SelectorParser};

use crate::tree::document::{Document, NodeId};
use crate::tree::node::Node;

/// Selector text that failed to parse.
///
/// The DOM query methods report a parse failure instead of matching nothing:
/// every one of them is specified to throw a `SyntaxError` `DOMException`, so
/// the distinction has to survive up to the script boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidSelector {
    selectors: String,
}

impl InvalidSelector {
    /// The selector text as it was passed in.
    #[must_use]
    pub fn selectors(&self) -> &str {
        &self.selectors
    }
}

impl fmt::Display for InvalidSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a valid selector", self.selectors)
    }
}

impl std::error::Error for InvalidSelector {}

impl<T: Sync> Document<T> {
    /// The first descendant of `root` matching `selectors`, in tree order.
    ///
    /// `root` may be the document, an element, or a shadow root; it is
    /// itself never a candidate. Only an element root is the `:scope`
    /// element: rooted at the document or a shadow root no scope element is
    /// set, and `:scope` falls back to matching the document element — so
    /// inside a shadow tree it matches nothing. Gecko and Servo apply the
    /// same fallback through this [`dom_apis`] path.
    ///
    /// <https://dom.spec.whatwg.org/#dom-parentnode-queryselector>
    ///
    /// # Errors
    ///
    /// [`InvalidSelector`] when `selectors` does not parse.
    pub fn query_selector(
        &self,
        root: NodeId,
        selectors: &str,
    ) -> Result<Option<NodeId>, InvalidSelector> {
        let list = self.parse_selectors(selectors)?;
        let mut first = None;
        dom_apis::query_selector::<&Node<T>, QueryFirst>(
            self.query_root(root),
            &list,
            &mut first,
            MayUseInvalidation::No,
        );
        Ok(first.map(Node::id))
    }

    /// Every descendant of `root` matching `selectors`, in tree order.
    ///
    /// <https://dom.spec.whatwg.org/#dom-parentnode-queryselectorall>
    ///
    /// # Errors
    ///
    /// [`InvalidSelector`] when `selectors` does not parse.
    pub fn query_selector_all(
        &self,
        root: NodeId,
        selectors: &str,
    ) -> Result<Vec<NodeId>, InvalidSelector> {
        let list = self.parse_selectors(selectors)?;
        let mut all = dom_apis::QuerySelectorAllResult::new();
        dom_apis::query_selector::<&Node<T>, QueryAll>(
            self.query_root(root),
            &list,
            &mut all,
            MayUseInvalidation::No,
        );
        Ok(all.into_iter().map(Node::id).collect())
    }

    /// Whether `element` matches `selectors`, with itself as the `:scope`
    /// element.
    ///
    /// <https://dom.spec.whatwg.org/#dom-element-matches>
    ///
    /// # Errors
    ///
    /// [`InvalidSelector`] when `selectors` does not parse.
    pub fn matches(&self, element: NodeId, selectors: &str) -> Result<bool, InvalidSelector> {
        let list = self.parse_selectors(selectors)?;
        Ok(dom_apis::element_matches(
            &self.live_element(element),
            &list,
            self.quirks_mode(),
        ))
    }

    /// The nearest inclusive ancestor of `element` matching `selectors`.
    ///
    /// The walk stops at the containing tree's root, so it never leaves a
    /// shadow tree.
    ///
    /// <https://dom.spec.whatwg.org/#dom-element-closest>
    ///
    /// # Errors
    ///
    /// [`InvalidSelector`] when `selectors` does not parse.
    pub fn closest(
        &self,
        element: NodeId,
        selectors: &str,
    ) -> Result<Option<NodeId>, InvalidSelector> {
        let list = self.parse_selectors(selectors)?;
        Ok(
            dom_apis::element_closest(self.live_element(element), &list, self.quirks_mode())
                .map(Node::id),
        )
    }

    fn parse_selectors(
        &self,
        selectors: &str,
    ) -> Result<SelectorList<SelectorImpl>, InvalidSelector> {
        SelectorParser::parse_author_origin_no_namespace(
            selectors,
            self.root_node().document_url_data(),
        )
        .map_err(|_| InvalidSelector {
            selectors: selectors.to_owned(),
        })
    }

    fn quirks_mode(&self) -> QuirksMode {
        TDocument::quirks_mode(&self.root_node())
    }

    /// The scoping root of a query — the spec's `ParentNode` receivers.
    fn query_root(&self, root: NodeId) -> &Node<T> {
        let node = self.live(root);
        assert!(
            node.is_document() || node.is_element() || node.is_shadow_root(),
            "Document selector queries are rooted at a document, element, or shadow root"
        );
        node
    }
}
