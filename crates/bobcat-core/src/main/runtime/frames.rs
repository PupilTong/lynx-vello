//! The `frame` binding: one PAPI over one document, with each realm's
//! rendering root its own.
//!
//! A `frame` (`main/tree/frame.rs`) is an element with an open shadow root
//! and nothing else. What renders in it is a nested page a `ShadowRealm`
//! child of the MTS realm builds, with the same host members the page realm
//! has, over the same [`DocumentSlot`] (user ruling: no second engine and no
//! scoped copy of the PAPI; the host card and its frames are one trust
//! domain, so no member checks which realm created the node it is handed).
//! What differs between the two is [`RealmRoot`], read by three members
//! alone:
//!
//! - `createPage` answers the realm's page: the document's own `page` element in the page realm,
//!   the nested page in a child, which throws until it is bound;
//! - `adoptStyleSheet` mounts on the document in the page realm and on the frame's shadow root in a
//!   child, so a nested card's sheet styles its own tree alone;
//! - `flushElementTree` parks in the page realm only: a child's flush commits when the host's first
//!   flush has already settled the listed sheets and the binding, and otherwise returns, leaving
//!   the commit to the epilogue's `commit_if_dirty`.
//!
//! Every other member is root-agnostic: a node id names one node for the
//! document's life whichever realm holds it, and each realm's
//! `bobcat:element` keeps its own handle table.
//!
//! # Binding
//!
//! `bindFrame(frame)` is a child's: it creates the nested page — a `page`
//! element with web-core's `part="page"` — appends it to the frame's shadow
//! root, makes it the calling realm's root and answers its id. A realm binds
//! once, a frame is bound once, and nesting stops at [`MAX_FRAME_DEPTH`]
//! frames. The page realm's call throws: it is bound to its document.
//!
//! # Release
//!
//! `releaseFrame(frame)` frees the nested page and everything under it —
//! the subtree is inside a shadow tree, so `dom` frees it whole — after
//! releasing every frame bound inside it, and sets the bound child's
//! `retired` flag, so the owner's next epilogue frees the child realm
//! whoever still refers to it. The sheets the child mounted on the frame's
//! shadow root are removed with it. A `dropElement` of a bound frame host
//! releases it first. A frame that is only disconnected keeps its binding
//! and its realm (`docs/tracking/deviations.md`). What a released child
//! created and never attached stays until the view ends.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dom::NodeId;
use quickjs_rust_bridge::HostValue;

use super::{
    DocumentSlot, MainThreadError, ScriptEngine, borrow_slot, install, node_id_argument,
    node_id_value, validate_live_element,
};
use crate::main::tree::{LynxDocument, PAGE_TAG, is_frame};

/// How many frames deep a nested page may sit, counting the frame it is
/// bound to.
const MAX_FRAME_DEPTH: usize = 8;

/// The part name web-core's nested page carries (`web-core/css/index.css`,
/// `[part="page"]`).
const PAGE_PART: &str = "page";

/// What one realm's `createPage`, `adoptStyleSheet` and `flushElementTree`
/// answer for: the document, or the frame the realm is bound to.
pub(super) enum RealmRoot {
    /// The page realm: the document's own `page` element, the document's
    /// sheets, and the flush that settles the listed sheets and the binding.
    Document,
    /// A `ShadowRealm` child: no root until its `bindFrame`, then the
    /// nested page and the frame's shadow root.
    Frame {
        bound: Cell<Option<FrameRoot>>,
        /// The child's own release flag, which its binding records for
        /// `releaseFrame` to set.
        retired: Rc<Cell<bool>>,
    },
}

/// A bound child's root: the nested page and the shadow root it sits in.
#[derive(Clone, Copy)]
pub(super) struct FrameRoot {
    page: NodeId,
    shadow: NodeId,
}

/// Where `adoptStyleSheet` mounts a sheet.
#[derive(Clone, Copy)]
pub(super) enum SheetTarget {
    Document,
    Shadow(NodeId),
}

/// One bound frame, keyed in [`DocumentSlot::frames`] by its host.
pub(super) struct FrameBinding {
    page: NodeId,
    retired: Rc<Cell<bool>>,
}

const UNBOUND: &str = "this ShadowRealm is bound to no frame";
const RELEASED: &str = "the frame this ShadowRealm is bound to was released";

impl RealmRoot {
    /// The root of a fresh child, bound to no frame.
    pub(super) fn frame(retired: Rc<Cell<bool>>) -> Self {
        Self::Frame {
            bound: Cell::new(None),
            retired,
        }
    }

    /// What `createPage` answers: the page realm's permanent `page`, or the
    /// child's nested page while its frame is bound.
    pub(super) fn page(&self, document: &LynxDocument) -> Result<NodeId, String> {
        match self {
            Self::Document => Ok(document.document_element().id()),
            Self::Frame { bound, .. } => {
                let root = bound.get().ok_or(UNBOUND)?;
                document
                    .get(root.page)
                    .map(dom::Node::id)
                    .ok_or_else(|| RELEASED.to_owned())
            }
        }
    }

    /// Where this realm's `adoptStyleSheet` mounts; a child bound to no
    /// frame, or whose frame was released, has nowhere.
    pub(super) fn sheets(&self, document: &LynxDocument) -> Result<SheetTarget, String> {
        match self {
            Self::Document => Ok(SheetTarget::Document),
            Self::Frame { bound, .. } => {
                let root = bound.get().ok_or(UNBOUND)?;
                if document.get(root.shadow).is_none() {
                    return Err(RELEASED.to_owned());
                }
                Ok(SheetTarget::Shadow(root.shadow))
            }
        }
    }

    /// Whether this realm's `flushElementTree` may park: the page realm's
    /// alone, whose first flush is what settles the listed sheets and the
    /// binding.
    pub(super) const fn parks(&self) -> bool {
        matches!(self, Self::Document)
    }
}

impl DocumentSlot {
    /// Binds `frame` to the child whose release flag is `retired`,
    /// answering the root the child is to take: the nested page this
    /// creates and the shadow root it sits in.
    fn bind_frame(
        &mut self,
        function: &str,
        frame: NodeId,
        retired: &Rc<Cell<bool>>,
    ) -> Result<FrameRoot, String> {
        let Self {
            document, frames, ..
        } = self;
        let document = document.as_mut().expect(super::DOCUMENT_EXISTS);
        validate_live_element(document, function, frame)?;
        if !is_frame(document, frame) {
            return Err(format!("{function} requires a frame element"));
        }
        if frames.contains_key(&frame) {
            return Err(format!("{function}: the frame is already bound"));
        }
        let shadow = document
            .shadow_root(frame)
            .expect("the frame component attaches a shadow root to every frame");
        if frame_depth(document, frame) > MAX_FRAME_DEPTH {
            return Err(format!(
                "{function}: frames nest at most {MAX_FRAME_DEPTH} deep"
            ));
        }
        let page = document.create_element(PAGE_TAG, ());
        document.set_attribute(page, "part", PAGE_PART);
        document.append_child(shadow, page);
        frames.insert(
            frame,
            FrameBinding {
                page,
                retired: Rc::clone(retired),
            },
        );
        Ok(FrameRoot { page, shadow })
    }

    /// Releases the binding of `frame`, if it has one: every frame bound
    /// inside its nested page first, then the nested page and its whole
    /// subtree, and the sheets the child mounted on the frame's shadow root,
    /// so a card loaded into the frame next is styled by its own alone. The
    /// bound child is retired.
    pub(super) fn release_frame(&mut self, frame: NodeId) {
        let Some(binding) = self.frames.remove(&frame) else {
            return;
        };
        binding.retired.set(true);
        let document = self.document.as_ref().expect(super::DOCUMENT_EXISTS);
        let shadow = document.shadow_root(frame);
        if document.get(binding.page).is_some() {
            let nested: Vec<NodeId> = self
                .frames
                .keys()
                .copied()
                .filter(|host| document.is_ancestor(binding.page, *host))
                .collect();
            for host in nested {
                self.release_frame(host);
            }
            self.document_mut().drop_element(binding.page);
        }
        // Otherwise a raw `dropElement` of the nested page's id freed it
        // while the frame was disconnected; its sheets are still mounted.
        if let Some(shadow) = shadow {
            self.document_mut().clear_shadow_stylesheets(shadow);
        }
    }

    /// How many frames this slot has bound, for a test.
    #[cfg(test)]
    pub(super) fn bound_frames(&self) -> usize {
        self.frames.len()
    }
}

/// How many frames `frame` sits in, itself included: every frame among its
/// shadow-including inclusive ancestors. A shadow root's parent is its host
/// here, so the parent chain crosses every shadow boundary.
fn frame_depth(document: &LynxDocument, frame: NodeId) -> usize {
    let mut depth = 0;
    let mut current = Some(frame);
    while let Some(node) = current {
        if is_frame(document, node) {
            depth += 1;
        }
        current = document.get(node).and_then(dom::Node::parent_id);
    }
    depth
}

/// Installs `bindFrame` and `releaseFrame`, in the page realm and in every
/// child, so that a frame's realm can bind a frame of its own.
pub(super) fn install_frame_members(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<DocumentSlot>>,
    root: &Rc<RealmRoot>,
) -> Result<(), MainThreadError> {
    let tree = Rc::clone(handle);
    let realm = Rc::clone(root);
    install(engine, "bindFrame", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.bindFrame";
        let frame = node_id_argument(NAME, arguments, 0)?;
        let RealmRoot::Frame { bound, retired } = &*realm else {
            return Err("the page realm is bound to its document".to_owned());
        };
        if bound.get().is_some() {
            return Err(format!("{NAME}: this ShadowRealm is already bound"));
        }
        let root = borrow_slot(NAME, &tree)?.bind_frame(NAME, frame, retired)?;
        bound.set(Some(root));
        Ok(node_id_value(root.page))
    })?;

    let tree = Rc::clone(handle);
    install(engine, "releaseFrame", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.releaseFrame";
        let frame = node_id_argument(NAME, arguments, 0)?;
        borrow_slot(NAME, &tree)?.release_frame(frame);
        Ok(HostValue::Undefined)
    })
}
