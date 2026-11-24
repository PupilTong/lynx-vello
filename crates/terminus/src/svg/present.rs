//! The document's side table of SVG recordings.
//!
//! An SVG image's isolated document (`super::image`) files its root's
//! recording in its own table ([`Document::keep_svg_recording`]), recorded
//! with [`RootGroup::Recorded`](super::record::RootGroup::Recorded) and
//! shaped with the host document's text context, which is where the plan
//! and the paint walk read it. The table, [`SvgRoots`], is empty and
//! unallocated while the document has none, and a freed root leaves it in
//! `Document::free_node`.

use rustc_hash::FxHashMap;

use super::recording::Recording;
use crate::tree::document::{Document, NodeId};

/// The outermost `svg` elements of a document and what each draws.
#[derive(Debug, Default)]
pub(crate) struct SvgRoots {
    entries: FxHashMap<NodeId, SvgRoot>,
}

#[derive(Debug, Default)]
struct SvgRoot {
    /// The last record, its text shaped; `None` before the first.
    recording: Option<Recording>,
}

impl SvgRoots {
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Takes `id` out of the table, giving the table's storage back once it
    /// is empty.
    fn remove(&mut self, id: NodeId) {
        if self.entries.remove(&id).is_some() && self.entries.is_empty() {
            *self = Self::default();
        }
    }
}

impl<T> Document<T> {
    /// Files `recording`, its text shaped, as what the outermost `svg`
    /// element `id` draws.
    pub(crate) fn keep_svg_recording(&mut self, id: NodeId, recording: Recording) {
        let entry = self.svg_roots.entries.entry(id).or_default();
        entry.recording = Some(recording);
    }

    /// What the outermost `svg` element `id` draws, as last recorded.
    #[inline]
    pub(crate) fn svg_recording(&self, id: NodeId) -> Option<&Recording> {
        if self.svg_roots.is_empty() {
            return None;
        }
        self.svg_roots.entries.get(&id)?.recording.as_ref()
    }

    /// Takes a freed node out of the table. One test on a document with no
    /// `svg` root.
    #[inline]
    pub(crate) fn forget_svg_root(&mut self, id: NodeId) {
        if !self.svg_roots.is_empty() {
            self.svg_roots.remove(id);
        }
    }
}
