//! A commit's timing: when [`Document::render`] flushed style, laid out and
//! built the frame it published, in the vocabulary of Long Animation Frames
//! (`styleAndLayoutStart`, split here because this engine can split it).
//!
//! Off by default and opt-in per document
//! ([`Document::set_commit_timing`]). Off, a render reads one `bool` and
//! takes no instant; on, it takes six [`ClockInstant`] readings into a fixed
//! `Copy` record and allocates nothing. The presenting side's half — when the
//! frame was composed and presented — is the painter's
//! ([`crate::painter::PresentedFrame`]); the two meet on the commit id.

use crate::clock::ClockInstant;
use crate::tree::document::Document;

/// When one [`Document::render`] that built a frame ran each of its phases.
///
/// `style_*` brackets the style flushes of the render's layout passes and
/// `layout_*` their layout runs, each from the first pass's start to the last
/// pass's end. A render runs more than one pass when a size query container
/// or a scroll-driven animation needs another, and then the two brackets
/// interleave (the second pass's flush ends after the first pass's layout
/// started); with one pass, `style_end <= layout_start`. A render whose
/// passes ran no layout — nothing that layout reads changed — reports an
/// empty layout bracket at `style_end`. `frame_build_*` brackets the
/// paint-order build through the scene encode, and includes any further
/// layout the build itself asks for (`content-visibility: auto` relevance,
/// an initial scroll target).
#[derive(Clone, Copy, Debug)]
pub struct CommitTiming {
    /// The id of the frame this render published
    /// ([`crate::visual::CommittedFrame::commit_id`]).
    pub commit_id: u64,
    /// The first style flush started.
    pub style_start: ClockInstant,
    /// The last style flush ended.
    pub style_end: ClockInstant,
    /// The first layout run started.
    pub layout_start: ClockInstant,
    /// The last layout run ended.
    pub layout_end: ClockInstant,
    /// The paint-order build started.
    pub frame_build_start: ClockInstant,
    /// The frame was encoded and retained.
    pub frame_build_end: ClockInstant,
}

/// The document's commit-timing state: the switch, the marks of the render
/// in progress, and the last finished record.
#[derive(Debug, Default)]
pub(crate) struct CommitTimer {
    enabled: bool,
    /// Whether a timed render is between its start and its frame build: the
    /// only window in which a layout pass marks. Never set while timing is
    /// off, so an untimed pass reads this one `bool` and nothing else.
    marking: bool,
    style: Option<(ClockInstant, ClockInstant)>,
    layout: Option<(ClockInstant, ClockInstant)>,
    last: Option<CommitTiming>,
}

impl CommitTimer {
    /// Whether a layout pass marks its phases.
    #[inline]
    pub(crate) const fn marking(&self) -> bool {
        self.marking
    }

    /// Opens the marks of a render that will build a frame.
    #[inline]
    pub(crate) fn open(&mut self) {
        if self.enabled {
            self.marking = true;
            self.style = None;
            self.layout = None;
        }
    }

    /// Records one style flush.
    pub(crate) fn mark_style(&mut self, start: ClockInstant, end: ClockInstant) {
        self.style = Some((self.style.map_or(start, |(first, _)| first), end));
    }

    /// Records one layout run.
    pub(crate) fn mark_layout(&mut self, start: ClockInstant, end: ClockInstant) {
        self.layout = Some((self.layout.map_or(start, |(first, _)| first), end));
    }

    /// Closes the style and layout marks, answering the frame build's start
    /// when this render is timed.
    #[inline]
    pub(crate) fn close(&mut self) -> Option<ClockInstant> {
        if !self.marking {
            return None;
        }
        self.marking = false;
        Some(ClockInstant::now())
    }

    /// Finishes the record of the render whose build started at
    /// `frame_build_start`.
    pub(crate) fn finish(&mut self, commit_id: u64, frame_build_start: ClockInstant) {
        let frame_build_end = ClockInstant::now();
        // Every timed render flushes style at least once; the fallbacks keep
        // the record ordered should a future render path not.
        let (style_start, style_end) = self
            .style
            .take()
            .unwrap_or((frame_build_start, frame_build_start));
        let (layout_start, layout_end) = self.layout.take().unwrap_or((style_end, style_end));
        self.last = Some(CommitTiming {
            commit_id,
            style_start,
            style_end,
            layout_start,
            layout_end,
            frame_build_start,
            frame_build_end,
        });
    }
}

impl<T> Document<T> {
    /// Turns commit timing on or off for the renders that follow.
    ///
    /// While it is on, every [`Self::render`] that builds a frame records a
    /// [`CommitTiming`] ([`Self::last_commit_timing`]). Turning it off keeps
    /// the last record; a render while it is off takes no instant and leaves
    /// the record as it was, which its `commit_id` tells apart from the
    /// frame since published.
    pub fn set_commit_timing(&mut self, enabled: bool) {
        self.commit_timer.enabled = enabled;
    }

    /// The timing of the last [`Self::render`] that built a frame while
    /// timing was on; `None` when none has. A render that built no frame —
    /// nothing was stale — leaves it as it was.
    #[must_use]
    pub const fn last_commit_timing(&self) -> Option<CommitTiming> {
        self.commit_timer.last
    }
}
