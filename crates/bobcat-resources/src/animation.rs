//! Animated URL pictures: one playhead per source, shared by every draw of
//! it in every view of the scope, and the frames it plays decoded a bounded
//! window ahead of it.
//!
//! The playback policy is `vant`'s ([`vant::image::animation`]): the
//! timeline with its 100 ms rule, the playhead that advances only while
//! drawn and never skips a frame, and the loop count the file states. What
//! is here is the store around it, on the painter's thread: which frames are
//! held, which decode next, and when the host has to take a turn.
//!
//! # Frames
//!
//! Frame 0 is the source's bitmap in the residency, exactly as a still
//! image's is: decoded by the load, refined to the drawn size, released once
//! the rasterizer holds its upload, restored when wanted. Every other frame
//! is decoded by index, at frame 0's decoded size, from the encoded bytes,
//! which an animated source always keeps. A frame decode is a job like a
//! refinement: off the painter's thread, under a decode permit, one in
//! flight per source at a time, in playing order, and its completion wakes
//! the host. When a source stops being drawn the decoder is told it is asked
//! no more frames of those bytes for now (`ImageDecoder::forget_frames`), so
//! a decoder that keeps an animation open between frames can close it.
//!
//! # What is held, and for how long
//!
//! While a source is drawn its store holds the frame shown and the
//! [`AHEAD`] frames after it, decoded in playing order; a frame the playhead
//! has moved past is dropped at once, and so is the frame shown once the
//! painter says the rasterizer holds its upload ([`Animations::release`]).
//! An animation of at most `AHEAD + 1` frames is the exception: all of its
//! frames are in that window, so they are held for as long as it is drawn,
//! each keeps one bitmap, and their uploads are found again on every loop.
//! A source that is not drawn holds nothing beyond frame 0 in the residency;
//! its playhead keeps its place. So the decoded bytes an animation costs are
//! at most `AHEAD + 1` frames at frame 0's decoded size while it is drawn,
//! and none of them count against the residency's budget, as the pinned
//! bitmaps of a drawn frame do not either.
//!
//! On the GPU each frame is a bitmap of its own, uploaded and leased as any
//! bitmap is: a painter leases the upload of the frame it draws and lets go
//! of the previous frame's when the frame changes, so a frame no target has
//! drawn recently is released under the rasterizer's budget. A frame decoded
//! again is a new bitmap, uploaded again.
//!
//! # Time
//!
//! A playhead runs while its source is drawn: from the first describe of
//! it, until a drawn set no view's frame includes it in
//! ([`Animations::retain`]). Each describe advances the playhead to the
//! present, at most one frame, onto a frame that is held; and while it runs,
//! the store arms a timer on its own executor at the instant the next frame
//! is due, which wakes the host the way a finished load does. The next
//! adoption describes the source again, and the frame moves. A frame that is
//! due but not decoded yet is waited for: its decode's completion is what
//! wakes the host then, and the timer is not armed for an instant already
//! past.

use std::sync::Arc;
use std::time::Duration;

use bobcat_core::peniko::ImageData;
use bobcat_core::{ClockInstant, FrameImage, ImageResidency, ImageSizeHint};
use bytes::Bytes;
use rustc_hash::{FxHashMap, FxHashSet};
use vant::image::animation::{ImageFrames, Loops, PlayPolicy, Playhead, Step, Timeline};
use vant::image::residency::{Described, Refinement, ViewKey};
use vant::image::{Bitmap, ImageDescriptor};

use crate::image_header::ImageHeader;
use crate::mime::ImageFormat;

/// How many frames after the one shown are kept decoded while a source is
/// drawn.
pub(crate) const AHEAD: usize = 2;

/// What decodes one frame: the encoded bytes and what they are, the frame,
/// and the size to decode it at.
#[derive(Clone, Debug)]
pub(crate) struct FrameDecode {
    pub(crate) bytes: Bytes,
    pub(crate) format: ImageFormat,
    pub(crate) header: Option<ImageHeader>,
    pub(crate) index: usize,
    pub(crate) max: (u32, u32),
}

/// Work the store asks for, started once the painter thread lets go of the
/// state.
#[derive(Debug)]
pub(crate) enum Job {
    /// Decode a frame other than the first.
    Frame {
        source: Arc<str>,
        decode: FrameDecode,
    },
    /// Decode the first frame again: a refinement or a restore the
    /// residency planned.
    Refine {
        source: Arc<str>,
        refinement: Refinement,
    },
    /// Wake the host at `due`, `wait` from now.
    Timer {
        source: Arc<str>,
        due: ClockInstant,
        wait: Duration,
    },
}

/// One animated source.
#[derive(Debug)]
struct Animation {
    timeline: Timeline,
    playhead: Playhead,
    bytes: Bytes,
    format: ImageFormat,
    header: Option<ImageHeader>,
    /// Per frame, its pixels while held. Frame 0's are the residency's, so
    /// its slot stays empty.
    held: Vec<Option<ImageData>>,
    /// Per frame, the bitmap it was last decoded as, held or not.
    decoded: Vec<Option<ImageDescriptor>>,
    /// The frame whose decode is in flight.
    decoding: Option<usize>,
    /// The size frames decode at: frame 0's, as last known.
    size: (u32, u32),
    /// How large the frame last drew it.
    hint: ImageSizeHint,
    /// The frame shown has neither pixels here nor an upload in the
    /// painter's rasterizer: it is decoded before any frame ahead.
    wanted: bool,
    /// The instant a timer is armed for.
    armed: Option<ClockInstant>,
    /// A frame failed to decode: the playhead stays where it is.
    failed: bool,
    /// The frame shown failed to decode: there is nothing to draw it from.
    lost: bool,
}

impl Animation {
    /// Whether every frame fits in the window, so every frame is held while
    /// the source is drawn.
    fn held_whole(&self) -> bool {
        self.timeline.len() <= AHEAD + 1
    }

    /// Whether the playhead is in the last loop of a finite count, so it
    /// will not wrap to the first frame again.
    fn in_last_loop(&self) -> bool {
        match self.playhead.policy().loops {
            Loops::Infinite => false,
            Loops::Plays(plays) => self.playhead.loops_completed() + 1 >= plays.get(),
        }
    }

    /// The frames held while drawn, in the order they are decoded: the one
    /// shown, then up to [`AHEAD`] after it, none past the last frame of the
    /// last loop and none once the playhead has finished.
    fn window(&self) -> impl Iterator<Item = usize> + '_ {
        let current = self.playhead.frame();
        let ahead = if self.playhead.is_finished() || self.failed {
            0
        } else {
            AHEAD.min(self.timeline.len() - 1)
        };
        let last_loop = self.in_last_loop();
        let count = self.timeline.len();
        std::iter::once(current).chain(
            (1..=ahead)
                .map(move |offset| current + offset)
                .take_while(move |&frame| !(last_loop && frame >= count))
                .map(move |frame| frame % count),
        )
    }

    /// Drops the pixels of every frame outside the window: all of them when
    /// the source is not drawn.
    fn trim(&mut self) {
        let keep: FxHashSet<usize> = if self.playhead.is_drawn() {
            self.window().collect()
        } else {
            FxHashSet::default()
        };
        for (frame, held) in self.held.iter_mut().enumerate() {
            if !keep.contains(&frame) {
                *held = None;
            }
        }
    }

    /// Advances the playhead to `at`, onto a frame that is held: the first
    /// frame when the residency holds it, another when this store does.
    fn step(&mut self, residency: &ImageResidency, source: &str, at: Duration) {
        let held = &self.held;
        let step = self.playhead.advance(&self.timeline, at, |next| {
            if next == 0 {
                residency.is_resident(source)
            } else {
                held[next].is_some()
            }
        });
        if matches!(step, Step::Advanced { .. } | Step::Finished) {
            self.wanted = false;
            self.trim();
        }
    }

    /// What the frame shown draws as, without its generation and next
    /// change; `None` when there is nothing to draw it from.
    fn shown(
        &self,
        residency: &mut ImageResidency,
        source: &Arc<str>,
        jobs: &mut Vec<Job>,
    ) -> Option<FrameImage> {
        let frame = self.playhead.frame();
        if frame == 0 {
            let (image, refine) = match residency.describe(source, self.hint) {
                Described::Unavailable => return None,
                Described::Resident { image, refine } => (FrameImage::resident(image), refine),
                Described::Absent { descriptor, refine } => {
                    (FrameImage::released(descriptor), refine)
                }
            };
            if let Some(refinement) = refine {
                jobs.push(Job::Refine {
                    source: Arc::clone(source),
                    refinement,
                });
            }
            return Some(image);
        }
        if self.lost {
            return None;
        }
        match (&self.held[frame], self.decoded[frame]) {
            (Some(pixels), _) => Some(FrameImage::resident(pixels.clone())),
            (None, Some(descriptor)) => Some(FrameImage::released(descriptor)),
            (None, None) => None,
        }
    }

    /// Plans what the source needs while drawn: the timer for its next
    /// frame, and the next decode its window lacks.
    fn plan(
        &mut self,
        residency: &mut ImageResidency,
        source: &Arc<str>,
        origin: ClockInstant,
        at: Duration,
        jobs: &mut Vec<Job>,
    ) {
        if !self.playhead.is_drawn() || self.failed {
            return;
        }
        if let Some(due) = self.playhead.due(&self.timeline)
            && due > at
        {
            let instant = origin + due;
            if self.armed != Some(instant) {
                self.armed = Some(instant);
                jobs.push(Job::Timer {
                    source: Arc::clone(source),
                    due: instant,
                    wait: due.saturating_sub(at),
                });
            }
        }
        if self.decoding.is_some() {
            return;
        }
        if let Some(size) = residency.resident_size(source) {
            self.size = size;
        }
        let current = self.playhead.frame();
        let frames: Vec<usize> = self.window().collect();
        for frame in frames {
            if frame == 0 {
                // The first frame is the residency's, and comes back as a
                // restore. Only one ahead needs it: the one shown is either
                // drawn from its upload or wanted through the residency.
                if frame != current
                    && !residency.is_resident(source)
                    && let Some(refinement) = residency.want(source, self.hint)
                {
                    jobs.push(Job::Refine {
                        source: Arc::clone(source),
                        refinement,
                    });
                }
                continue;
            }
            // The frame shown is decoded again only when wanted: otherwise
            // the painter draws it from its upload.
            let needed = self.held[frame].is_none() && (frame != current || self.wanted);
            if needed {
                self.decoding = Some(frame);
                jobs.push(Job::Frame {
                    source: Arc::clone(source),
                    decode: FrameDecode {
                        bytes: self.bytes.clone(),
                        format: self.format,
                        header: self.header,
                        index: frame,
                        max: self.size,
                    },
                });
                return;
            }
        }
    }
}

/// Every animated source of one scope, the drawn set each view last
/// reported of them, and the origin their playheads' time is measured
/// from.
#[derive(Debug)]
pub(crate) struct Animations {
    entries: FxHashMap<Arc<str>, Animation>,
    /// Each view's drawn animated sources: a playhead runs while the union
    /// includes it.
    views: FxHashMap<ViewKey, Vec<Arc<str>>>,
    origin: ClockInstant,
}

impl Animations {
    /// No animated source yet; every playhead's time is measured from
    /// `origin`.
    pub(crate) fn new(origin: ClockInstant) -> Self {
        Self {
            entries: FxHashMap::default(),
            views: FxHashMap::default(),
            origin,
        }
    }

    /// The playheads' time at `now`.
    fn at(&self, now: ClockInstant) -> Duration {
        now.saturating_duration_since(self.origin)
    }

    /// Whether no source of the scope animates.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether `source` is an animated source: the one constant check a
    /// still image pays.
    pub(crate) fn contains(&self, source: &str) -> bool {
        !self.entries.is_empty() && self.entries.contains_key(source)
    }

    /// `source` loaded as an animation of `frames`, frame 0 decoded at
    /// `size`, from `bytes` of `format`: its playhead starts on frame 0,
    /// under the count the file states, not drawn yet.
    pub(crate) fn insert(
        &mut self,
        source: Arc<str>,
        frames: &ImageFrames,
        bytes: Bytes,
        format: ImageFormat,
        header: Option<ImageHeader>,
        size: (u32, u32),
    ) {
        let count = frames.count();
        self.entries.insert(
            source,
            Animation {
                timeline: Timeline::new(frames),
                playhead: Playhead::new(PlayPolicy::stated(frames)),
                bytes,
                format,
                header,
                held: vec![None; count],
                decoded: vec![None; count],
                decoding: None,
                size,
                hint: ImageSizeHint::UNBOUNDED,
                wanted: false,
                armed: None,
                failed: false,
                lost: false,
            },
        );
    }

    /// What a frame drawing `source` at `hint` at `now` draws: the frame its
    /// playhead is on once advanced to `now`, at the playhead's generation,
    /// with the instant its next frame is due while it runs. A describe is
    /// a draw, so it starts the playhead if it was not running. Plans the
    /// decodes, the restore and the timer the source needs into `jobs`.
    pub(crate) fn describe(
        &mut self,
        residency: &mut ImageResidency,
        source: &Arc<str>,
        hint: ImageSizeHint,
        now: ClockInstant,
        jobs: &mut Vec<Job>,
    ) -> Option<FrameImage> {
        let (origin, at) = (self.origin, self.at(now));
        let animation = self.entries.get_mut(source)?;
        animation.hint = hint;
        animation.playhead.set_drawn(true, &animation.timeline, at);
        animation.step(residency, source, at);
        let mut image = animation.shown(residency, source, jobs)?;
        image.generation = animation.playhead.steps();
        image.next_change = animation
            .playhead
            .due(&animation.timeline)
            .map(|due| origin + due);
        animation.plan(residency, source, origin, at, jobs);
        Some(image)
    }

    /// A frame draws `source` and has nothing to draw it from. The first
    /// frame is the residency's to restore, so `false` is answered for it;
    /// another is decoded again here, before any frame ahead. Answers
    /// whether `source` is animated and on a frame of its own.
    pub(crate) fn want(
        &mut self,
        residency: &mut ImageResidency,
        source: &Arc<str>,
        hint: ImageSizeHint,
        now: ClockInstant,
        jobs: &mut Vec<Job>,
    ) -> bool {
        let (origin, at) = (self.origin, self.at(now));
        let Some(animation) = self.entries.get_mut(source) else {
            return false;
        };
        if animation.playhead.frame() == 0 {
            return false;
        }
        animation.hint = hint;
        animation.wanted = true;
        animation.plan(residency, source, origin, at, jobs);
        true
    }

    /// The painter's rasterizer holds the uploads of `sources`' frames
    /// shown. An animated source on a frame of its own drops that frame's
    /// pixels, unless every frame is held for as long as it is drawn;
    /// answers the sources whose bitmap the residency drops.
    pub(crate) fn release(&mut self, sources: &[Arc<str>]) -> Vec<Arc<str>> {
        if self.entries.is_empty() {
            return sources.to_vec();
        }
        let mut residency = Vec::with_capacity(sources.len());
        for source in sources {
            let Some(animation) = self.entries.get_mut(source) else {
                residency.push(Arc::clone(source));
                continue;
            };
            if animation.held_whole() {
                continue;
            }
            match animation.playhead.frame() {
                0 => residency.push(Arc::clone(source)),
                frame => animation.held[frame] = None,
            }
        }
        residency
    }

    /// Records `view`'s drawn set, at `now`: a playhead runs while some
    /// view's set includes its source, and stops, its frames dropped, when
    /// none does. Answers the encoded bytes of every source that stopped,
    /// whose frames the decoder may forget.
    pub(crate) fn retain(
        &mut self,
        view: ViewKey,
        drawn: &[Arc<str>],
        now: ClockInstant,
    ) -> Vec<Bytes> {
        if self.entries.is_empty() {
            return Vec::new();
        }
        let animated: Vec<Arc<str>> = drawn
            .iter()
            .filter(|source| self.entries.contains_key(&***source))
            .cloned()
            .collect();
        self.views.insert(view, animated);
        self.restate(now)
    }

    /// Forgets `view`'s drawn set, at `now`; answers as [`Self::retain`].
    pub(crate) fn release_view(&mut self, view: ViewKey, now: ClockInstant) -> Vec<Bytes> {
        if self.views.remove(&view).is_some() {
            self.restate(now)
        } else {
            Vec::new()
        }
    }

    /// Moves every playhead to whether the union of the views' sets
    /// includes its source; answers the bytes of those that stopped.
    fn restate(&mut self, now: ClockInstant) -> Vec<Bytes> {
        let at = self.at(now);
        let mut stopped = Vec::new();
        let drawn: FxHashSet<&str> = self
            .views
            .values()
            .flatten()
            .map(|source| &**source)
            .collect();
        for (source, animation) in &mut self.entries {
            let is_drawn = drawn.contains(&**source);
            if animation.playhead.is_drawn() == is_drawn {
                continue;
            }
            animation
                .playhead
                .set_drawn(is_drawn, &animation.timeline, at);
            if !is_drawn {
                animation.wanted = false;
                animation.armed = None;
                animation.trim();
                stopped.push(animation.bytes.clone());
            }
        }
        stopped
    }

    /// Plans what every drawn source needs at `now`: after completions,
    /// which free a source's decode for the next frame its window lacks.
    pub(crate) fn plan(
        &mut self,
        residency: &mut ImageResidency,
        now: ClockInstant,
        jobs: &mut Vec<Job>,
    ) {
        let (origin, at) = (self.origin, self.at(now));
        for (source, animation) in &mut self.entries {
            animation.plan(residency, source, origin, at, jobs);
        }
    }

    /// Frame `index` of `source` decoded as `bitmap`: held while it is in
    /// the window of a drawn source, and dropped otherwise.
    pub(crate) fn decoded(&mut self, source: &str, index: usize, bitmap: Bitmap) {
        let Some(animation) = self.entries.get_mut(source) else {
            return;
        };
        if animation.decoding == Some(index) {
            animation.decoding = None;
        }
        if index == 0 || index >= animation.held.len() {
            return;
        }
        let image = bitmap.into_image_data();
        animation.decoded[index] = Some(ImageDescriptor::of(&image));
        if index == animation.playhead.frame() {
            animation.wanted = false;
        }
        if animation.playhead.is_drawn() && animation.window().any(|frame| frame == index) {
            animation.held[index] = Some(image);
        }
    }

    /// Frame `index` of `source` failed to decode at `now`: its playhead
    /// stops on the frame it is on, so it names no next change and the
    /// painter stops asking, and when that is the frame that failed, there
    /// is nothing to draw it from. Nothing retries it, as nothing retries a
    /// failed restore.
    /// Answers the encoded bytes, whose frames the decoder may forget.
    pub(crate) fn decode_failed(
        &mut self,
        source: &str,
        index: usize,
        now: ClockInstant,
    ) -> Option<Bytes> {
        let at = self.at(now);
        let animation = self.entries.get_mut(source)?;
        if animation.decoding == Some(index) {
            animation.decoding = None;
        }
        animation.playhead.pause(&animation.timeline, at);
        animation.failed = true;
        animation.lost |= index == animation.playhead.frame();
        animation.wanted = false;
        animation.armed = None;
        animation.trim();
        Some(animation.bytes.clone())
    }

    /// The timer armed for `due` on `source` fired.
    pub(crate) fn fired(&mut self, source: &str, due: ClockInstant) {
        if let Some(animation) = self.entries.get_mut(source)
            && animation.armed == Some(due)
        {
            animation.armed = None;
        }
    }

    /// Bytes of decoded frames held, frame 0 aside: those are the
    /// residency's.
    pub(crate) fn held_bytes(&self) -> usize {
        self.entries
            .values()
            .flat_map(|animation| animation.held.iter().flatten())
            .map(|image| image.data.data().len())
            .sum()
    }

    /// The frame `source`'s playhead is on and the loops it has completed,
    /// for tests.
    #[cfg(test)]
    pub(crate) fn position(&self, source: &str) -> Option<(usize, u32, bool)> {
        self.entries.get(source).map(|animation| {
            (
                animation.playhead.frame(),
                animation.playhead.loops_completed(),
                animation.playhead.is_drawn(),
            )
        })
    }

    /// The frames `source` holds, for tests.
    #[cfg(test)]
    pub(crate) fn held_frames(&self, source: &str) -> Vec<usize> {
        self.entries.get(source).map_or_else(Vec::new, |animation| {
            animation
                .held
                .iter()
                .enumerate()
                .filter_map(|(frame, held)| held.as_ref().map(|_| frame))
                .collect()
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "animation_tests.rs"]
mod tests;
