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

#[derive(Clone, Copy, Debug)]
pub struct CommitTiming {
    pub commit_id: u64,
    pub style_start: ClockInstant,
    pub style_end: ClockInstant,
    pub layout_start: ClockInstant,
    pub layout_end: ClockInstant,
    pub frame_build_start: ClockInstant,
    pub frame_build_end: ClockInstant,
}

#[derive(Debug, Default)]
pub(crate) struct CommitTimer {
    enabled: bool,
    marking: bool,
    style: Option<(ClockInstant, ClockInstant)>,
    layout: Option<(ClockInstant, ClockInstant)>,
    last: Option<CommitTiming>,
}

impl CommitTimer {
    #[inline]
    pub(crate) const fn marking(&self) -> bool {
        self.marking
    }

    #[inline]
    pub(crate) fn open(&mut self) {
        if self.enabled {
            self.marking = true;
            self.style = None;
            self.layout = None;
        }
    }

    pub(crate) fn mark_style(&mut self, start: ClockInstant, end: ClockInstant) {
        self.style = Some((self.style.map_or(start, |(first, _)| first), end));
    }

    pub(crate) fn mark_layout(&mut self, start: ClockInstant, end: ClockInstant) {
        self.layout = Some((self.layout.map_or(start, |(first, _)| first), end));
    }

    #[inline]
    pub(crate) fn close(&mut self) -> Option<ClockInstant> {
        if !self.marking {
            return None;
        }
        self.marking = false;
        Some(ClockInstant::now())
    }

    pub(crate) fn finish(&mut self, commit_id: u64, frame_build_start: ClockInstant) {
        let frame_build_end = ClockInstant::now();
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ContentDetection {
    #[default]
    Off,
    On,
}

impl<T> Document<T> {
    pub fn set_commit_timing(&mut self, enabled: bool) {
        self.commit_timer.enabled = enabled;
    }

    pub fn set_content_detection(&mut self, enabled: bool) {
        self.content_detection = if enabled {
            ContentDetection::On
        } else {
            ContentDetection::Off
        };
    }

    #[must_use]
    pub const fn last_commit_timing(&self) -> Option<CommitTiming> {
        self.commit_timer.last
    }
}
