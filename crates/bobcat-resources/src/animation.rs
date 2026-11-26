//! Animated pictures: the frames of each animated source, decoded once and
//! a bounded window ahead of the playheads that play them; one playhead per
//! source shared by every draw of its URL in every view of the scope; and a
//! playhead of its own for each `Id` picture an element asked to play the
//! source ([`PlaybackRequest`](bobcat_core::PlaybackRequest)).
//!
//! The playback policy is `vant`'s ([`vant::image::animation`]): the
//! timeline with its 100 ms rule, the playhead that advances only while
//! drawn and never skips a frame, the loop count, and the four controls.
//! What is here is the store around it, on the painter's thread: which
//! frames are held, which decode next, when the host has to take a turn,
//! and what an `Id` picture's slot is handed.
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
//! the host. When no playhead of a source is drawn any more the decoder is
//! told it is asked no more frames of those bytes for now
//! (`ImageDecoder::forget_frames`), so a decoder that keeps an animation open
//! between frames can close it.
//!
//! # Playheads
//!
//! A source's URL picture has one playhead, under the count its file states
//! ([`PlayPolicy::stated`]); its draws describe it by URL and are lent the
//! frame shown. An `Id` picture playing the source has a playhead of its own
//! under the policy its owner asked for, which the owner's controls drive
//! ([`Animations::control`]); the store publishes the frame that playhead
//! shows into the picture's slot ([`PictureSlot::publish`]) once per
//! change of it, and the painter takes it into the picture's own texture.
//! Every loop such a playhead completes is reported to the view that asked
//! for the playback, as [`PlaybackReport::LoopComplete`], and the final loop
//! of a finite count as [`PlaybackReport::FinalLoopComplete`] after it.
//!
//! # What is held, and for how long
//!
//! While a source is drawn its store holds, for each drawn playhead, the
//! frame it plays from and the [`AHEAD`] frames after it, decoded in playing
//! order; a frame no drawn playhead's window includes is dropped at once.
//! The URL playhead's window also holds the frame it shows, until the painter
//! says the rasterizer holds its upload ([`Animations::release`]); an `Id`
//! picture's holds the frame it shows until that frame is published, since
//! the slot then holds it. An animation of at most `AHEAD + 1` frames is the
//! exception: all of its frames are in every window, so they are held for
//! as long as it is drawn and decoded once, and the URL picture's uploads
//! are found again on every loop. A source no playhead draws
//! holds nothing beyond frame 0 in the residency; its playheads keep their
//! places. So the decoded bytes an animation costs are bounded by its drawn
//! playheads' windows at frame 0's decoded size, and none of them count
//! against the residency's budget, as the pinned bitmaps of a drawn frame do
//! not either.
//!
//! On the GPU each frame of the URL picture is a bitmap of its own, uploaded
//! and leased as any bitmap is: a painter leases the upload of the frame it
//! draws and lets go of the previous frame's when the frame changes, so a
//! frame no target has drawn recently is released under the rasterizer's
//! budget. A frame decoded again is a new bitmap, uploaded again. An `Id`
//! picture has one texture, which each frame published rewrites.
//!
//! # Time
//!
//! A playhead runs while its picture is drawn: from the first describe of
//! it, until a drawn set no view's frame includes it in
//! ([`Animations::retain`]). Each describe advances the playhead to the
//! present, at most one frame, onto a frame that is held; and while it runs,
//! the store arms a timer on its own executor at the instant the next frame
//! is due, which wakes the host the way a finished load does. The next
//! adoption describes the picture again, and the frame moves. A frame that is
//! due but not decoded yet is waited for: its decode's completion is what
//! wakes the host then, and the timer is not armed for an instant already
//! past.
//!
//! # Platform frames
//!
//! `F` is the platform decoder's frame type. A frame is CPU pixels or one of
//! the decoder's own ([`PictureFrame`]), named by the descriptor its decode
//! was given ([`DecodedImage::into_frame`]); everything above holds for
//! either, and a platform frame's bytes are counted from its size. Dropping
//! the store's copy is what lets the platform free one: the browser's closes
//! its `ImageBitmap` when its last copy goes.

use std::sync::Arc;
use std::time::Duration;

use bobcat_core::{
    ClockInstant, FrameImage, ImageReports, ImageResidency, ImageSizeHint, PictureFrame, PictureId,
    PictureSlot, PlayPolicy, PlaybackControl, PlaybackReport, TakenFrame,
};
use bytes::Bytes;
use rustc_hash::{FxHashMap, FxHashSet};
use vant::image::animation::{ImageFrames, Loops, Playhead, Step, Timeline};
use vant::image::residency::{Described, Refinement, ViewKey};
use vant::image::{DecodedImage, ImageDescriptor, frame_bytes};

use crate::image_header::ImageHeader;
use crate::mime::ImageFormat;

pub(crate) const AHEAD: usize = 2;

#[derive(Clone, Debug)]
pub(crate) struct FrameDecode {
    pub(crate) bytes: Bytes,
    pub(crate) format: ImageFormat,
    pub(crate) header: Option<ImageHeader>,
    pub(crate) index: usize,
    pub(crate) max: (u32, u32),
}

#[derive(Debug)]
pub(crate) enum Job {
    Frame {
        source: Arc<str>,
        decode: FrameDecode,
    },
    Refine {
        source: Arc<str>,
        refinement: Refinement,
    },
    Timer {
        source: Arc<str>,
        due: ClockInstant,
        wait: Duration,
    },
}

#[derive(Debug)]
struct Head {
    playhead: Playhead,
    wanted: bool,
    armed: Option<ClockInstant>,
}

impl Head {
    const fn new(policy: PlayPolicy) -> Self {
        Self {
            playhead: Playhead::new(policy),
            wanted: false,
            armed: None,
        }
    }

    fn in_last_loop(&self) -> bool {
        match self.playhead.policy().loops {
            Loops::Infinite => false,
            Loops::Plays(plays) => self.playhead.loops_completed() + 1 >= plays.get(),
        }
    }

    fn window(&self, count: usize, failed: bool, shown: bool) -> impl Iterator<Item = usize> {
        let frame = self.playhead.frame();
        let current = self.playhead.position();
        let ahead = if self.playhead.is_finished() || failed {
            0
        } else {
            AHEAD.min(count - 1)
        };
        let last_loop = self.in_last_loop();
        let shown = shown.then_some(frame);
        let position = (current != frame).then_some(current);
        shown.into_iter().chain(position).chain(
            (1..=ahead)
                .map(move |offset| current + offset)
                .take_while(move |&frame| !(last_loop && frame >= count))
                .map(move |frame| frame % count),
        )
    }

    fn step<F: Clone>(
        &mut self,
        timeline: &Timeline,
        held: &[Option<PictureFrame<F>>],
        residency: &ImageResidency<F>,
        source: &str,
        at: Duration,
    ) -> Step {
        let step = self.playhead.advance(timeline, at, |next| {
            if next == 0 {
                residency.is_resident(source)
            } else {
                held[next].is_some()
            }
        });
        if matches!(step, Step::Advanced { .. } | Step::Finished) {
            self.wanted = false;
        }
        step
    }

    fn arm(
        &mut self,
        timeline: &Timeline,
        source: &Arc<str>,
        origin: ClockInstant,
        at: Duration,
        jobs: &mut Vec<Job>,
    ) {
        if !self.playhead.is_drawn() {
            return;
        }
        if let Some(due) = self.playhead.due(timeline)
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
    }

    fn undrawn(&mut self) {
        self.wanted = false;
        self.armed = None;
    }
}

#[derive(Debug)]
struct Owned<F> {
    head: Head,
    slot: PictureSlot<F>,
    published: Option<u64>,
    reports: ImageReports,
    view: ViewKey,
    hint: ImageSizeHint,
}

impl<F> Owned<F> {
    fn unpublished(&self) -> bool {
        self.published != Some(self.head.playhead.steps())
    }
}

#[derive(Debug)]
struct Animation<F> {
    timeline: Timeline,
    shared: Head,
    pictures: FxHashMap<PictureId, Owned<F>>,
    bytes: Bytes,
    format: ImageFormat,
    header: Option<ImageHeader>,
    held: Vec<Option<PictureFrame<F>>>,
    decoded: Vec<Option<ImageDescriptor>>,
    decoding: Option<usize>,
    size: (u32, u32),
    hint: ImageSizeHint,
    failed: bool,
    lost: bool,
}

impl<F: Clone> Animation<F> {
    fn held_whole(&self) -> bool {
        self.timeline.len() <= AHEAD + 1
    }

    fn is_drawn(&self) -> bool {
        self.shared.playhead.is_drawn()
            || self
                .pictures
                .values()
                .any(|owned| owned.head.playhead.is_drawn())
    }

    fn windows(&self) -> Vec<(usize, bool)> {
        let count = self.timeline.len();
        let mut frames = Vec::new();
        if self.shared.playhead.is_drawn() {
            let shown = self.shared.playhead.frame();
            frames.extend(
                self.shared
                    .window(count, self.failed, true)
                    .map(|frame| (frame, frame != shown || self.shared.wanted)),
            );
        }
        for owned in self.pictures.values() {
            if owned.head.playhead.is_drawn() {
                let shown = self.held_whole() || owned.unpublished() || owned.head.wanted;
                frames.extend(
                    owned
                        .head
                        .window(count, self.failed, shown)
                        .map(|frame| (frame, true)),
                );
            }
        }
        frames
    }

    fn pictures_hold(&self, frame: usize) -> bool {
        let count = self.timeline.len();
        self.pictures.values().any(|owned| {
            owned.head.playhead.is_drawn()
                && owned
                    .head
                    .window(
                        count,
                        self.failed,
                        self.held_whole() || owned.unpublished() || owned.head.wanted,
                    )
                    .any(|held| held == frame)
        })
    }

    fn trim(&mut self) {
        let keep: FxHashSet<usize> = self.windows().into_iter().map(|(frame, _)| frame).collect();
        for (frame, held) in self.held.iter_mut().enumerate() {
            if !keep.contains(&frame) {
                *held = None;
            }
        }
    }

    fn shown(
        &self,
        residency: &mut ImageResidency<F>,
        source: &Arc<str>,
        jobs: &mut Vec<Job>,
    ) -> Option<FrameImage<F>> {
        let frame = self.shared.playhead.frame();
        if frame == 0 {
            let (image, refine) = match residency.describe(source, self.hint) {
                Described::Unavailable => return None,
                Described::Resident {
                    image,
                    descriptor,
                    refine,
                } => (FrameImage::held(image, descriptor), refine),
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
            (Some(pixels), Some(descriptor)) => Some(FrameImage::held(pixels.clone(), descriptor)),
            (None, Some(descriptor)) => Some(FrameImage::released(descriptor)),
            (_, None) => None,
        }
    }

    fn publish(
        &mut self,
        picture: PictureId,
        residency: &mut ImageResidency<F>,
        source: &Arc<str>,
        origin: ClockInstant,
        jobs: &mut Vec<Job>,
    ) -> bool {
        let Some(owned) = self.pictures.get_mut(&picture) else {
            return false;
        };
        if !owned.unpublished() {
            return false;
        }
        let frame = owned.head.playhead.frame();
        let pixels = if frame == 0 {
            match residency.describe(source, owned.hint) {
                Described::Resident {
                    image,
                    descriptor,
                    refine,
                } => {
                    if let Some(refinement) = refine {
                        jobs.push(Job::Refine {
                            source: Arc::clone(source),
                            refinement,
                        });
                    }
                    Some((image, descriptor))
                }
                Described::Absent { .. } | Described::Unavailable => None,
            }
        } else {
            self.held[frame].clone().zip(self.decoded[frame])
        };
        let Some((pixels, descriptor)) = pixels else {
            return false;
        };
        let next = owned
            .head
            .playhead
            .due(&self.timeline)
            .map(|due| origin + due);
        owned.published = Some(owned.head.playhead.steps());
        owned.head.wanted = false;
        let wake = owned.slot.publish(pixels, descriptor, next);
        self.trim();
        wake
    }

    fn plan(
        &mut self,
        residency: &mut ImageResidency<F>,
        source: &Arc<str>,
        origin: ClockInstant,
        at: Duration,
        jobs: &mut Vec<Job>,
    ) {
        if self.failed {
            return;
        }
        self.shared.arm(&self.timeline, source, origin, at, jobs);
        for owned in self.pictures.values_mut() {
            owned.head.arm(&self.timeline, source, origin, at, jobs);
        }
        if self.decoding.is_some() {
            return;
        }
        if let Some(size) = residency.resident_size(source) {
            self.size = size;
        }
        let shared_frame = self
            .shared
            .playhead
            .is_drawn()
            .then(|| self.shared.playhead.frame());
        let hint = self
            .pictures
            .values()
            .find(|owned| owned.head.playhead.is_drawn())
            .map_or(self.hint, |owned| owned.hint);
        for (frame, needed) in self.windows() {
            if !needed {
                continue;
            }
            if frame == 0 {
                if (shared_frame != Some(0) || self.pictures_hold(0))
                    && !residency.is_resident(source)
                    && let Some(refinement) = residency.want(source, hint)
                {
                    jobs.push(Job::Refine {
                        source: Arc::clone(source),
                        refinement,
                    });
                }
                continue;
            }
            if self.held[frame].is_none() {
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

#[derive(Debug, Default)]
struct Drawn {
    sources: Vec<Arc<str>>,
    pictures: Vec<PictureId>,
}

#[derive(Debug)]
pub(crate) struct Animations<F> {
    entries: FxHashMap<Arc<str>, Animation<F>>,
    pictures: FxHashMap<PictureId, Arc<str>>,
    views: FxHashMap<ViewKey, Drawn>,
    origin: ClockInstant,
}

impl<F: Clone> Animations<F> {
    pub(crate) fn new(origin: ClockInstant) -> Self {
        Self {
            entries: FxHashMap::default(),
            pictures: FxHashMap::default(),
            views: FxHashMap::default(),
            origin,
        }
    }

    fn at(&self, now: ClockInstant) -> Duration {
        now.saturating_duration_since(self.origin)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn contains(&self, source: &str) -> bool {
        !self.entries.is_empty() && self.entries.contains_key(source)
    }

    pub(crate) fn sources_of(&self, pictures: &[PictureId]) -> Vec<Arc<str>> {
        pictures
            .iter()
            .filter_map(|picture| self.pictures.get(picture).cloned())
            .collect()
    }

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
                shared: Head::new(PlayPolicy::stated(frames)),
                pictures: FxHashMap::default(),
                bytes,
                format,
                header,
                held: vec![None; count],
                decoded: vec![None; count],
                decoding: None,
                size,
                hint: ImageSizeHint::UNBOUNDED,
                failed: false,
                lost: false,
            },
        );
    }

    pub(crate) fn describe(
        &mut self,
        residency: &mut ImageResidency<F>,
        source: &Arc<str>,
        hint: ImageSizeHint,
        now: ClockInstant,
        jobs: &mut Vec<Job>,
    ) -> Option<FrameImage<F>> {
        let (origin, at) = (self.origin, self.at(now));
        let animation = self.entries.get_mut(source)?;
        animation.hint = hint;
        animation
            .shared
            .playhead
            .set_drawn(true, &animation.timeline, at);
        let step = if animation.failed {
            Step::Stay
        } else {
            animation
                .shared
                .step(&animation.timeline, &animation.held, residency, source, at)
        };
        if matches!(step, Step::Advanced { .. } | Step::Finished) {
            animation.trim();
        }
        let mut image = animation.shown(residency, source, jobs)?;
        image.generation = animation.shared.playhead.steps();
        image.next_change = animation
            .shared
            .playhead
            .due(&animation.timeline)
            .map(|due| origin + due);
        animation.plan(residency, source, origin, at, jobs);
        Some(image)
    }

    pub(crate) fn describe_picture(
        &mut self,
        residency: &mut ImageResidency<F>,
        picture: PictureId,
        hint: ImageSizeHint,
        now: ClockInstant,
        jobs: &mut Vec<Job>,
    ) -> (Option<FrameImage<F>>, bool) {
        let (origin, at) = (self.origin, self.at(now));
        let Some(source) = self.pictures.get(&picture).cloned() else {
            return (None, false);
        };
        let Some(animation) = self.entries.get_mut(&source) else {
            return (None, false);
        };
        let Animation {
            timeline,
            pictures,
            held,
            failed,
            ..
        } = animation;
        let Some(owned) = pictures.get_mut(&picture) else {
            return (None, false);
        };
        owned.hint = hint;
        owned.head.playhead.set_drawn(true, timeline, at);
        let step = if *failed {
            Step::Stay
        } else {
            owned.head.step(timeline, held, residency, &source, at)
        };
        let done = step.loops_done();
        let mut wake = false;
        if done.completed > 0 {
            owned
                .reports
                .playback(picture, PlaybackReport::LoopComplete);
            if done.finished {
                owned
                    .reports
                    .playback(picture, PlaybackReport::FinalLoopComplete);
            }
            wake = true;
        }
        let _ = animation.publish(picture, residency, &source, origin, jobs);
        if !matches!(step, Step::Stay | Step::Waiting) {
            animation.trim();
        }
        let Some(owned) = animation.pictures.get(&picture) else {
            return (None, wake);
        };
        let image = owned.slot.describe().map(|mut image| {
            image.next_change = owned
                .head
                .playhead
                .due(&animation.timeline)
                .map(|due| origin + due);
            image
        });
        animation.plan(residency, &source, origin, at, jobs);
        (image, wake)
    }

    pub(crate) fn want(
        &mut self,
        residency: &mut ImageResidency<F>,
        source: &Arc<str>,
        hint: ImageSizeHint,
        now: ClockInstant,
        jobs: &mut Vec<Job>,
    ) -> bool {
        let (origin, at) = (self.origin, self.at(now));
        let Some(animation) = self.entries.get_mut(source) else {
            return false;
        };
        if animation.shared.playhead.frame() == 0 {
            return false;
        }
        animation.hint = hint;
        animation.shared.wanted = true;
        animation.plan(residency, source, origin, at, jobs);
        true
    }

    pub(crate) fn want_picture(
        &mut self,
        residency: &mut ImageResidency<F>,
        picture: PictureId,
        hint: ImageSizeHint,
        now: ClockInstant,
        jobs: &mut Vec<Job>,
    ) {
        let (origin, at) = (self.origin, self.at(now));
        let Some(source) = self.pictures.get(&picture).cloned() else {
            return;
        };
        let Some(animation) = self.entries.get_mut(&source) else {
            return;
        };
        if let Some(owned) = animation.pictures.get_mut(&picture) {
            owned.hint = hint;
            owned.head.wanted = true;
            owned.published = None;
        }
        animation.plan(residency, &source, origin, at, jobs);
    }

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
            let frame = animation.shared.playhead.frame();
            if animation.pictures_hold(frame) {
                continue;
            }
            match frame {
                0 => residency.push(Arc::clone(source)),
                frame => animation.held[frame] = None,
            }
        }
        residency
    }

    pub(crate) fn retain(
        &mut self,
        view: ViewKey,
        sources: &[Arc<str>],
        pictures: &[PictureId],
        now: ClockInstant,
    ) -> Vec<Bytes> {
        if self.entries.is_empty() {
            return Vec::new();
        }
        let drawn = Drawn {
            sources: sources
                .iter()
                .filter(|source| self.entries.contains_key(&***source))
                .cloned()
                .collect(),
            pictures: pictures
                .iter()
                .filter(|picture| self.pictures.contains_key(picture))
                .copied()
                .collect(),
        };
        self.views.insert(view, drawn);
        self.restate(now)
    }

    pub(crate) fn release_view(&mut self, view: ViewKey, now: ClockInstant) -> Vec<Bytes> {
        let ended: Vec<PictureId> = self
            .entries
            .values()
            .flat_map(|animation| {
                animation
                    .pictures
                    .iter()
                    .filter(|(_, owned)| owned.view == view)
                    .map(|(picture, _)| *picture)
            })
            .collect();
        let mut stopped: Vec<Bytes> = ended
            .into_iter()
            .filter_map(|picture| self.end(picture))
            .collect();
        if self.views.remove(&view).is_some() {
            stopped.extend(self.restate(now));
        }
        stopped
    }

    fn restate(&mut self, now: ClockInstant) -> Vec<Bytes> {
        let at = self.at(now);
        let mut stopped = Vec::new();
        let sources: FxHashSet<&str> = self
            .views
            .values()
            .flat_map(|drawn| drawn.sources.iter())
            .map(|source| &**source)
            .collect();
        let pictures: FxHashSet<PictureId> = self
            .views
            .values()
            .flat_map(|drawn| drawn.pictures.iter().copied())
            .collect();
        for (source, animation) in &mut self.entries {
            let was_drawn = animation.is_drawn();
            let mut changed = false;
            let is_drawn = sources.contains(&**source);
            if animation.shared.playhead.is_drawn() != is_drawn {
                animation
                    .shared
                    .playhead
                    .set_drawn(is_drawn, &animation.timeline, at);
                if !is_drawn {
                    animation.shared.undrawn();
                }
                changed = true;
            }
            for (picture, owned) in &mut animation.pictures {
                let is_drawn = pictures.contains(picture);
                if owned.head.playhead.is_drawn() != is_drawn {
                    owned
                        .head
                        .playhead
                        .set_drawn(is_drawn, &animation.timeline, at);
                    if !is_drawn {
                        owned.head.undrawn();
                    }
                    changed = true;
                }
            }
            if changed {
                animation.trim();
            }
            if was_drawn && !animation.is_drawn() {
                stopped.push(animation.bytes.clone());
            }
        }
        stopped
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the request's three parts, its view's two, and the store's three"
    )]
    pub(crate) fn play(
        &mut self,
        residency: &mut ImageResidency<F>,
        picture: PictureId,
        source: &str,
        policy: PlayPolicy,
        reports: ImageReports,
        view: ViewKey,
        now: ClockInstant,
        jobs: &mut Vec<Job>,
    ) -> bool {
        let (origin, at) = (self.origin, self.at(now));
        if self.pictures.contains_key(&picture) {
            return false;
        }
        let Some((source, animation)) = self.entries.get_key_value(source) else {
            return false;
        };
        let source = Arc::clone(source);
        let hint = animation.hint;
        self.pictures.insert(picture, Arc::clone(&source));
        let animation = self
            .entries
            .get_mut(&source)
            .expect("the source was just found");
        animation.pictures.insert(
            picture,
            Owned {
                head: Head::new(policy),
                slot: PictureSlot::new(),
                published: None,
                reports,
                view,
                hint,
            },
        );
        let wake = animation.publish(picture, residency, &source, origin, jobs);
        if !wake
            && !residency.is_resident(&source)
            && let Some(refinement) = residency.want(&source, hint)
        {
            jobs.push(Job::Refine {
                source: Arc::clone(&source),
                refinement,
            });
        }
        animation.plan(residency, &source, origin, at, jobs);
        wake
    }

    pub(crate) fn control(
        &mut self,
        picture: PictureId,
        control: PlaybackControl,
        now: ClockInstant,
    ) -> bool {
        let at = self.at(now);
        let Some(source) = self.pictures.get(&picture) else {
            return false;
        };
        let Some(animation) = self.entries.get_mut(source) else {
            return false;
        };
        let Some(owned) = animation.pictures.get_mut(&picture) else {
            return false;
        };
        owned
            .head
            .playhead
            .control(control, &animation.timeline, at);
        owned.head.armed = None;
        animation.trim();
        true
    }

    pub(crate) fn end(&mut self, picture: PictureId) -> Option<Bytes> {
        let source = self.pictures.remove(&picture)?;
        for drawn in self.views.values_mut() {
            drawn.pictures.retain(|held| *held != picture);
        }
        let animation = self.entries.get_mut(&source)?;
        let was_drawn = animation.is_drawn();
        animation.pictures.remove(&picture);
        animation.trim();
        (was_drawn && !animation.is_drawn()).then(|| animation.bytes.clone())
    }

    pub(crate) fn plan(
        &mut self,
        residency: &mut ImageResidency<F>,
        now: ClockInstant,
        jobs: &mut Vec<Job>,
    ) {
        let (origin, at) = (self.origin, self.at(now));
        for (source, animation) in &mut self.entries {
            animation.plan(residency, source, origin, at, jobs);
        }
    }

    pub(crate) fn decoded(&mut self, source: &str, index: usize, bitmap: DecodedImage<F>) {
        let Some(animation) = self.entries.get_mut(source) else {
            return;
        };
        if animation.decoding == Some(index) {
            animation.decoding = None;
        }
        if index == 0 || index >= animation.held.len() {
            return;
        }
        let (image, descriptor) = bitmap.into_frame();
        animation.decoded[index] = Some(descriptor);
        if index == animation.shared.playhead.frame() {
            animation.shared.wanted = false;
        }
        if animation
            .windows()
            .into_iter()
            .any(|(frame, _)| frame == index)
        {
            animation.held[index] = Some(image);
        }
    }

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
        animation.shared.playhead.pause(&animation.timeline, at);
        animation.failed = true;
        animation.lost |= index == animation.shared.playhead.frame();
        animation.shared.undrawn();
        for owned in animation.pictures.values_mut() {
            owned.head.wanted = false;
            owned.head.armed = None;
        }
        animation.trim();
        Some(animation.bytes.clone())
    }

    pub(crate) fn fired(&mut self, source: &str, due: ClockInstant) {
        if let Some(animation) = self.entries.get_mut(source) {
            for head in std::iter::once(&mut animation.shared)
                .chain(animation.pictures.values_mut().map(|owned| &mut owned.head))
            {
                if head.armed == Some(due) {
                    head.armed = None;
                }
            }
        }
    }

    pub(crate) fn held_bytes(&self) -> usize {
        self.entries
            .values()
            .flat_map(|animation| animation.held.iter().zip(&animation.decoded))
            .map(|(held, descriptor)| match (held, descriptor) {
                (Some(PictureFrame::Pixels(image)), _) => image.data.data().len(),
                (Some(PictureFrame::Platform(_)), Some(descriptor)) => {
                    frame_bytes(descriptor.width, descriptor.height)
                }
                (Some(PictureFrame::Platform(_)) | None, _) => 0,
            })
            .sum()
    }

    pub(crate) fn take_frame(&self, picture: PictureId) -> Option<TakenFrame<F>> {
        let source = self.pictures.get(&picture)?;
        self.entries
            .get(source)?
            .pictures
            .get(&picture)?
            .slot
            .take()
    }

    #[cfg(test)]
    pub(crate) fn position(&self, source: &str) -> Option<(usize, u32, bool)> {
        self.entries.get(source).map(|animation| {
            (
                animation.shared.playhead.frame(),
                animation.shared.playhead.loops_completed(),
                animation.shared.playhead.is_drawn(),
            )
        })
    }

    #[cfg(test)]
    pub(crate) fn picture_position(&self, picture: PictureId) -> Option<(usize, usize, u32, bool)> {
        let source = self.pictures.get(&picture)?;
        let owned = self.entries.get(source)?.pictures.get(&picture)?;
        let playhead = &owned.head.playhead;
        Some((
            playhead.frame(),
            playhead.position(),
            playhead.loops_completed(),
            playhead.is_drawn(),
        ))
    }

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
