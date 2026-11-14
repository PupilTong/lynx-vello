//! Animated images: what a decoder says of an image's frames
//! ([`ImageFrames`]), the timeline playback makes of it ([`Timeline`]), and
//! the playhead that walks it ([`Playhead`]) under a [`PlayPolicy`].
//!
//! Everything here is synchronous and reads no clock. A caller passes the
//! time as a [`Duration`] since an origin of its own choosing, the same
//! origin for every call on one playhead, and is answered with instants on
//! that same scale.
//!
//! # The rules a playhead keeps
//!
//! - **A frame delay of 10 ms or less plays as 100 ms** ([`played_delay`]), the rule every browser
//!   applies to GIF, APNG and animated WebP alike.
//! - **It advances only while drawn.** Time does not pass for it otherwise: a playhead that stops
//!   being drawn keeps the frame it is on and the time that frame had already been shown, and
//!   continues from there when it is drawn again ([`Playhead::set_drawn`]).
//! - **A frame that is not ready when it is due is waited for.** Frames are never skipped:
//!   [`Playhead::advance`] moves at most one frame per call, and only onto a frame its caller says
//!   is ready. So a playhead that plays N loops completes exactly N, and
//!   [`Playhead::loops_completed`] counts them.
//! - **After the last loop of a finite count the last frame stays shown**, and the playhead is
//!   finished: it has no next change.
//!
//! # Policies
//!
//! A [`PlayPolicy`] is everything two semantics may differ in: how many
//! times the frames play, and whether the playhead starts paused. A CSS
//! image plays the count its file states and starts playing
//! ([`PlayPolicy::stated`]); an element with a control surface of its own
//! builds a policy from its own attributes and drives the same playhead
//! with the four [`Control`]s ([`Playhead::control`]).
//!
//! # Position and the frame shown
//!
//! A playhead's position is the frame it plays from; the frame shown is the
//! one whose pixels are drawn. They are the same frame except after
//! [`Control::Stop`], which moves the position to the first frame and leaves
//! the frame shown where it was: the next [`Control::Resume`] or
//! [`Control::Restart`] shows the first frame. [`Playhead::steps`] counts
//! changes of the frame shown, so a stop changes no pixels.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

/// The longest stated delay that is played as [`SHORT_DELAY_PLAYS_AS`].
pub const SHORT_DELAY_LIMIT: Duration = Duration::from_millis(10);

/// What a stated delay of [`SHORT_DELAY_LIMIT`] or less plays as.
pub const SHORT_DELAY_PLAYS_AS: Duration = Duration::from_millis(100);

/// How long a frame whose file states `stated` is shown: `stated`, except
/// that 10 ms or less is 100 ms.
///
/// Many files state a delay of zero or one hundredth of a second and were
/// authored for viewers that showed such frames for about a tenth; every
/// browser plays them that way, and so does this crate.
#[must_use]
pub fn played_delay(stated: Duration) -> Duration {
    if stated <= SHORT_DELAY_LIMIT {
        SHORT_DELAY_PLAYS_AS
    } else {
        stated
    }
}

/// How many times an image's frames play.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Loops {
    /// Forever.
    Infinite,
    /// This many times in all, the first play included.
    Plays(NonZeroU32),
}

impl Loops {
    /// Played once, which is what a file that states no count means.
    pub const ONCE: Self = Self::Plays(NonZeroU32::MIN);

    /// `plays` times in all; zero is [`Self::Infinite`], as every container
    /// that states a count spells it.
    #[must_use]
    pub fn from_plays(plays: u32) -> Self {
        NonZeroU32::new(plays).map_or(Self::Infinite, Self::Plays)
    }
}

/// What a decoder says of an image's frames, without their pixels: how
/// long each is shown, as the file states it, and how many times the file
/// says they play.
///
/// A decoder that knows nothing of animation answers [`Self::still`], one
/// frame: the image is drawn as the bitmap its ordinary decode produces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageFrames {
    delays: Arc<[Duration]>,
    loops: Loops,
}

impl ImageFrames {
    /// One frame: a still image, or an animated one played as its first
    /// frame.
    #[must_use]
    pub fn still() -> Self {
        Self {
            delays: Arc::from([Duration::ZERO]),
            loops: Loops::ONCE,
        }
    }

    /// Frames shown for `delays`, as the file states them, played `loops`
    /// times. No delays at all is [`Self::still`].
    #[must_use]
    pub fn new(delays: Vec<Duration>, loops: Loops) -> Self {
        if delays.is_empty() {
            return Self::still();
        }
        Self {
            delays: Arc::from(delays),
            loops,
        }
    }

    /// How many frames there are; at least one.
    #[must_use]
    pub fn count(&self) -> usize {
        self.delays.len()
    }

    /// Whether there is more than one frame to play.
    #[must_use]
    pub fn is_animated(&self) -> bool {
        self.count() > 1
    }

    /// Each frame's delay as the file states it, before
    /// [`played_delay`] applies.
    #[must_use]
    pub fn stated_delays(&self) -> &[Duration] {
        &self.delays
    }

    /// How many times the file says its frames play.
    #[must_use]
    pub const fn loops(&self) -> Loops {
        self.loops
    }
}

/// The delays a playhead walks: an image's frames, each shown for its
/// [`played_delay`].
///
/// Shared by every playhead over one image; a playhead is handed it on each
/// call that needs it rather than holding it, so one image's playheads hold
/// one timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timeline {
    delays: Arc<[Duration]>,
}

impl Timeline {
    /// The timeline `frames` plays.
    #[must_use]
    pub fn new(frames: &ImageFrames) -> Self {
        Self {
            delays: frames
                .stated_delays()
                .iter()
                .map(|&stated| played_delay(stated))
                .collect(),
        }
    }

    /// How many frames there are; at least one.
    #[must_use]
    pub fn len(&self) -> usize {
        self.delays.len()
    }

    /// Never: a timeline has at least one frame. Present for the lint that
    /// pairs it with [`Self::len`].
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.delays.is_empty()
    }

    /// How long `frame` is shown.
    ///
    /// # Panics
    ///
    /// If `frame` is not one of the timeline's.
    #[must_use]
    pub fn delay(&self, frame: usize) -> Duration {
        self.delays[frame]
    }

    /// The frame after `frame`, wrapping to the first.
    #[must_use]
    pub fn next(&self, frame: usize) -> usize {
        (frame + 1) % self.len()
    }
}

/// How a playhead plays: how many times, and whether it starts paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayPolicy {
    pub loops: Loops,
    /// Whether the playhead starts paused on its first frame.
    pub paused: bool,
}

impl PlayPolicy {
    /// The policy of a picture with no control surface (a CSS image, an HTML
    /// `<img>`): the count `frames` states, playing from the start.
    #[must_use]
    pub const fn stated(frames: &ImageFrames) -> Self {
        Self {
            loops: frames.loops(),
            paused: false,
        }
    }
}

/// One instruction of a control surface to a playhead ([`Playhead::control`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Control {
    /// Back to the first frame with no loop completed, playing, from any
    /// state.
    Restart,
    /// Stop advancing; the frame shown stays.
    Pause,
    /// Continue from the position: where a pause left it, or from the first
    /// frame after a [`Self::Stop`] or after the final loop.
    Resume,
    /// Stop advancing and move the position to the first frame with no loop
    /// completed; the frame shown stays until the playhead plays again.
    Stop,
}

/// What one [`Step`] owes a control surface's loop events: how many loops
/// the step completed (zero or one), and whether that was the final one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopsDone {
    pub completed: u32,
    pub finished: bool,
}

impl Step {
    /// The loop events this step owes: a loop completed by a move from the
    /// last frame to the first, or the final loop of a finite count. Every
    /// loop completes once, the final one included, so N loops are N
    /// completions with the last one also [`LoopsDone::finished`].
    #[must_use]
    pub const fn loops_done(self) -> LoopsDone {
        match self {
            Self::Advanced { looped: true } => LoopsDone {
                completed: 1,
                finished: false,
            },
            Self::Finished => LoopsDone {
                completed: 1,
                finished: true,
            },
            Self::Stay | Self::Waiting | Self::Advanced { looped: false } => LoopsDone {
                completed: 0,
                finished: false,
            },
        }
    }
}

/// What one [`Playhead::advance`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing was due, or the playhead is not running.
    Stay,
    /// The next frame is due and its caller said it is not ready: the
    /// playhead waits on the frame it is on.
    Waiting,
    /// The playhead moved to the next frame. `looped` when that completed a
    /// loop (the move from the last frame to the first).
    Advanced { looped: bool },
    /// The last frame of the last loop has been shown for its delay: that
    /// completed the final loop, and the last frame stays shown.
    Finished,
}

/// Whether a playhead plays, apart from whether it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Playing,
    /// Paused by its policy or by [`Control::Pause`].
    Paused,
    /// Stopped by [`Control::Stop`]: the position is the first frame while
    /// the frame shown stays.
    Stopped,
    /// The final loop of a finite count completed.
    Finished,
}

/// Where one picture's playback is: the frame it is on, the loops it has
/// completed, and whether it is playing, paused, stopped or finished, and
/// drawn.
///
/// A playhead is *running* while it is drawn, playing, and its timeline has
/// more than one frame. Only a running playhead's clock moves:
/// [`Self::shown`] of the frame it is on grows while it runs and keeps its
/// value while it does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playhead {
    /// The frame shown.
    frame: usize,
    loops: u32,
    state: State,
    drawn: bool,
    /// How long the frame it is on had been shown before the playhead last
    /// stopped running, at most that frame's delay.
    shown: Duration,
    /// When the playhead last started running, while it runs.
    since: Option<Duration>,
    /// How many times the frame shown has changed: a number that moves
    /// exactly when the pixels do.
    steps: u64,
    policy: PlayPolicy,
}

impl Playhead {
    /// A playhead on the first frame, not drawn yet, under `policy`.
    #[must_use]
    pub const fn new(policy: PlayPolicy) -> Self {
        Self {
            frame: 0,
            loops: 0,
            state: if policy.paused {
                State::Paused
            } else {
                State::Playing
            },
            drawn: false,
            shown: Duration::ZERO,
            since: None,
            steps: 0,
            policy,
        }
    }

    /// The frame shown.
    #[must_use]
    pub const fn frame(&self) -> usize {
        self.frame
    }

    /// The frame the playhead plays from: the frame shown, except after
    /// [`Control::Stop`], when it is the first frame.
    #[must_use]
    pub const fn position(&self) -> usize {
        if self.is_stopped() { 0 } else { self.frame }
    }

    /// Whether [`Control::Stop`] stopped it and nothing has played it since.
    #[must_use]
    pub const fn is_stopped(&self) -> bool {
        matches!(self.state, State::Stopped)
    }

    /// How many loops have completed.
    #[must_use]
    pub const fn loops_completed(&self) -> u32 {
        self.loops
    }

    /// How many times the frame shown has changed: what a picture answers
    /// as its generation.
    #[must_use]
    pub const fn steps(&self) -> u64 {
        self.steps
    }

    /// Whether it is paused or stopped: it would not play were it drawn.
    #[must_use]
    pub const fn is_paused(&self) -> bool {
        matches!(self.state, State::Paused | State::Stopped)
    }

    #[must_use]
    pub const fn is_drawn(&self) -> bool {
        self.drawn
    }

    /// Whether the final loop of a finite count has completed.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        matches!(self.state, State::Finished)
    }

    #[must_use]
    pub const fn policy(&self) -> PlayPolicy {
        self.policy
    }

    /// Whether the playhead's clock moves: drawn, playing, with more than
    /// one frame to walk.
    #[must_use]
    pub fn is_running(&self, timeline: &Timeline) -> bool {
        self.drawn && self.state == State::Playing && timeline.len() > 1
    }

    /// How long the frame it is on has been shown at `now`, at most its
    /// delay.
    #[must_use]
    pub fn shown(&self, timeline: &Timeline, now: Duration) -> Duration {
        let running = self
            .since
            .map_or(Duration::ZERO, |since| now.saturating_sub(since));
        (self.shown + running).min(timeline.delay(self.frame))
    }

    /// When the next frame is due, while the playhead runs: the instant the
    /// frame it is on will have been shown for its delay. It may be past,
    /// when the next frame was not ready at it.
    #[must_use]
    pub fn due(&self, timeline: &Timeline) -> Option<Duration> {
        let since = self.since?;
        Some(since + timeline.delay(self.frame).saturating_sub(self.shown))
    }

    /// The picture is drawn, or no longer is, as of `now`. A playhead that
    /// stops being drawn keeps the frame it is on and how long that frame
    /// had been shown; one drawn again continues from there.
    pub fn set_drawn(&mut self, drawn: bool, timeline: &Timeline, now: Duration) {
        if self.drawn == drawn {
            return;
        }
        self.stop(timeline, now);
        self.drawn = drawn;
        self.start(timeline, now);
    }

    /// Stops advancing at `now`; the frame shown stays. A stopped or
    /// finished playhead stays as it is.
    pub fn pause(&mut self, timeline: &Timeline, now: Duration) {
        self.stop(timeline, now);
        if self.state == State::Playing {
            self.state = State::Paused;
        }
    }

    /// Continues from the frame it is on, at `now`, when paused. A finished
    /// or stopped playhead stays as it is: [`Self::restart`] is what plays it
    /// again.
    pub fn resume(&mut self, timeline: &Timeline, now: Duration) {
        if self.state == State::Paused {
            self.state = State::Playing;
        }
        self.start(timeline, now);
    }

    /// Back to the first frame with no loop completed and not finished, at
    /// `now`, paused when it was paused or stopped and playing otherwise.
    /// The frame shown changes when it was not the first.
    pub fn restart(&mut self, timeline: &Timeline, now: Duration) {
        if self.frame != 0 {
            self.steps += 1;
        }
        self.frame = 0;
        self.loops = 0;
        self.state = if self.is_paused() {
            State::Paused
        } else {
            State::Playing
        };
        self.shown = Duration::ZERO;
        self.since = None;
        self.start(timeline, now);
    }

    /// Applies one control at `now`:
    ///
    /// - [`Control::Restart`]: [`Self::restart`], playing, from any state.
    /// - [`Control::Pause`]: [`Self::pause`]; the frame shown stays.
    /// - [`Control::Resume`]: continues from the position. After [`Control::Stop`] or the final
    ///   loop that is the first frame, with no loop completed, as a restart; otherwise
    ///   [`Self::resume`].
    /// - [`Control::Stop`]: stops advancing and moves the position to the first frame with no loop
    ///   completed and not finished. The frame shown stays, so the steps do not move.
    pub fn control(&mut self, control: Control, timeline: &Timeline, now: Duration) {
        match control {
            Control::Restart => self.play_from_start(timeline, now),
            Control::Pause => self.pause(timeline, now),
            Control::Resume if matches!(self.state, State::Stopped | State::Finished) => {
                self.play_from_start(timeline, now);
            }
            Control::Resume => self.resume(timeline, now),
            Control::Stop => {
                self.stop(timeline, now);
                self.state = State::Stopped;
                self.loops = 0;
                self.shown = Duration::ZERO;
            }
        }
    }

    /// [`Self::restart`], playing.
    fn play_from_start(&mut self, timeline: &Timeline, now: Duration) {
        self.state = State::Playing;
        self.restart(timeline, now);
    }

    /// Moves to the next frame when it is due at `now` and `ready` says it
    /// can be shown; at most one frame per call, so none is ever skipped.
    ///
    /// The next frame starts at the instant the one before it was due when
    /// it is shown within its own delay of that instant, which keeps the
    /// file's pace through a late turn; a frame shown later than that, after
    /// a wait for it to be ready or a turn that came very late, starts at
    /// `now`, so it is shown for its whole delay rather than passed over.
    pub fn advance(
        &mut self,
        timeline: &Timeline,
        now: Duration,
        ready: impl FnOnce(usize) -> bool,
    ) -> Step {
        if !self.is_running(timeline) {
            return Step::Stay;
        }
        let Some(due) = self.due(timeline) else {
            return Step::Stay;
        };
        if now < due {
            return Step::Stay;
        }
        let next = timeline.next(self.frame);
        let looped = next == 0;
        if looped
            && let Loops::Plays(plays) = self.policy.loops
            && self.loops + 1 >= plays.get()
        {
            self.loops += 1;
            self.state = State::Finished;
            self.shown = timeline.delay(self.frame);
            self.since = None;
            return Step::Finished;
        }
        if !ready(next) {
            return Step::Waiting;
        }
        if looped {
            self.loops += 1;
        }
        self.frame = next;
        self.steps += 1;
        self.shown = Duration::ZERO;
        self.since = Some(if now.saturating_sub(due) < timeline.delay(next) {
            due
        } else {
            now
        });
        Step::Advanced { looped }
    }

    /// Folds the time run so far into [`Self::shown`] and stops the clock.
    fn stop(&mut self, timeline: &Timeline, now: Duration) {
        self.shown = self.shown(timeline, now);
        self.since = None;
    }

    /// Starts the clock at `now`, when the playhead runs.
    fn start(&mut self, timeline: &Timeline, now: Duration) {
        if self.since.is_none() && self.is_running(timeline) {
            self.since = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    /// Three frames of 100, 200 and 300 ms, played `loops`.
    fn three(loops: Loops) -> (ImageFrames, Timeline) {
        let frames = ImageFrames::new(vec![ms(100), ms(200), ms(300)], loops);
        let timeline = Timeline::new(&frames);
        (frames, timeline)
    }

    /// A playhead under the file's own count, drawn from `at`.
    fn drawn(frames: &ImageFrames, timeline: &Timeline, at: Duration) -> Playhead {
        let mut playhead = Playhead::new(PlayPolicy::stated(frames));
        playhead.set_drawn(true, timeline, at);
        playhead
    }

    const READY: fn(usize) -> bool = |_| true;

    #[test]
    fn a_delay_of_ten_milliseconds_or_less_plays_as_a_hundred() {
        assert_eq!(played_delay(Duration::ZERO), ms(100));
        assert_eq!(played_delay(ms(1)), ms(100));
        assert_eq!(played_delay(ms(10)), ms(100));
        assert_eq!(played_delay(ms(11)), ms(11));
        assert_eq!(played_delay(ms(20)), ms(20));
        let frames = ImageFrames::new(vec![ms(5), ms(10), ms(11), ms(250)], Loops::Infinite);
        let timeline = Timeline::new(&frames);
        assert_eq!(
            (0..4)
                .map(|frame| timeline.delay(frame))
                .collect::<Vec<_>>(),
            [ms(100), ms(100), ms(11), ms(250)]
        );
        assert_eq!(
            frames.stated_delays(),
            [ms(5), ms(10), ms(11), ms(250)],
            "the stated delays are kept as stated"
        );
    }

    #[test]
    fn one_frame_is_a_still_image_and_never_runs() {
        let still = ImageFrames::still();
        assert_eq!(still.count(), 1);
        assert!(!still.is_animated());
        assert_eq!(ImageFrames::new(Vec::new(), Loops::Infinite), still);
        let timeline = Timeline::new(&still);
        let mut playhead = drawn(&still, &timeline, ms(0));
        assert!(!playhead.is_running(&timeline));
        assert_eq!(playhead.due(&timeline), None);
        assert_eq!(playhead.advance(&timeline, ms(10_000), READY), Step::Stay);
        assert_eq!(playhead.frame(), 0);
    }

    #[test]
    fn loop_counts_of_zero_are_infinite() {
        assert_eq!(Loops::from_plays(0), Loops::Infinite);
        assert_eq!(Loops::from_plays(1), Loops::ONCE);
        assert_eq!(
            Loops::from_plays(3),
            Loops::Plays(NonZeroU32::new(3).expect("three"))
        );
    }

    /// Each frame is shown for its delay: due at the sum of the delays
    /// before it, and not a moment earlier.
    #[test]
    fn frames_advance_at_their_delays() {
        let (frames, timeline) = three(Loops::Infinite);
        let mut playhead = drawn(&frames, &timeline, ms(1000));
        assert_eq!(playhead.due(&timeline), Some(ms(1100)));
        assert_eq!(playhead.advance(&timeline, ms(1099), READY), Step::Stay);
        assert_eq!(
            playhead.advance(&timeline, ms(1100), READY),
            Step::Advanced { looped: false }
        );
        assert_eq!((playhead.frame(), playhead.steps()), (1, 1));
        assert_eq!(playhead.due(&timeline), Some(ms(1300)));
        assert_eq!(
            playhead.advance(&timeline, ms(1305), READY),
            Step::Advanced { looped: false }
        );
        assert_eq!(
            playhead.due(&timeline),
            Some(ms(1600)),
            "a turn a little late keeps the file's pace"
        );
        assert_eq!(
            playhead.advance(&timeline, ms(1600), READY),
            Step::Advanced { looped: true }
        );
        assert_eq!((playhead.frame(), playhead.loops_completed()), (0, 1));
        assert_eq!(playhead.steps(), 3);
    }

    /// However late the turn, one call moves one frame: none is skipped,
    /// and a frame reached a whole delay late is shown for its whole delay.
    #[test]
    fn a_late_turn_moves_one_frame_and_skips_none() {
        let (frames, timeline) = three(Loops::Infinite);
        let mut playhead = drawn(&frames, &timeline, ms(0));
        assert_eq!(
            playhead.advance(&timeline, ms(5000), READY),
            Step::Advanced { looped: false }
        );
        assert_eq!(playhead.frame(), 1);
        assert_eq!(
            playhead.due(&timeline),
            Some(ms(5200)),
            "restarted from the turn"
        );
        assert_eq!(playhead.advance(&timeline, ms(5100), READY), Step::Stay);
    }

    /// A due frame that is not ready is waited for: the playhead stays on
    /// the frame it is on, and moves onto the next one at the first turn
    /// that finds it ready, showing it for its whole delay from there.
    #[test]
    fn a_frame_that_is_not_ready_is_waited_for() {
        let (frames, timeline) = three(Loops::Infinite);
        let mut playhead = drawn(&frames, &timeline, ms(0));
        assert_eq!(
            playhead.advance(&timeline, ms(100), |_| false),
            Step::Waiting
        );
        assert_eq!(
            playhead.advance(&timeline, ms(250), |_| false),
            Step::Waiting
        );
        assert_eq!((playhead.frame(), playhead.steps()), (0, 0));
        assert_eq!(
            playhead.due(&timeline),
            Some(ms(100)),
            "still due, in the past"
        );
        let mut asked = None;
        assert_eq!(
            playhead.advance(&timeline, ms(400), |frame| {
                asked = Some(frame);
                true
            }),
            Step::Advanced { looped: false }
        );
        assert_eq!(asked, Some(1), "the next frame is the one asked about");
        assert_eq!(playhead.due(&timeline), Some(ms(600)));
    }

    /// Time does not pass for a playhead that is not drawn: it keeps the
    /// frame it is on and how long that frame had been shown, and continues
    /// from there when drawn again.
    #[test]
    fn a_playhead_not_drawn_stops_and_continues_from_where_it_stopped() {
        let (frames, timeline) = three(Loops::Infinite);
        let mut playhead = drawn(&frames, &timeline, ms(0));
        assert_eq!(
            playhead.advance(&timeline, ms(100), READY),
            Step::Advanced { looped: false }
        );
        // Frame 1 is shown for 150 of its 200 ms, then not drawn for ten
        // seconds.
        playhead.set_drawn(false, &timeline, ms(250));
        assert!(!playhead.is_running(&timeline));
        assert_eq!(playhead.due(&timeline), None);
        assert_eq!(playhead.advance(&timeline, ms(10_000), READY), Step::Stay);
        assert_eq!(playhead.frame(), 1);
        assert_eq!(playhead.shown(&timeline, ms(10_000)), ms(150));

        playhead.set_drawn(true, &timeline, ms(10_250));
        assert_eq!(
            playhead.due(&timeline),
            Some(ms(10_300)),
            "the 50 ms frame 1 had left"
        );
        assert_eq!(playhead.advance(&timeline, ms(10_299), READY), Step::Stay);
        assert_eq!(
            playhead.advance(&timeline, ms(10_300), READY),
            Step::Advanced { looped: false }
        );
        assert_eq!(playhead.frame(), 2);
    }

    /// A playhead that stops while a due frame was not ready keeps no more
    /// than the frame's delay: drawn again, the next frame is due at once.
    #[test]
    fn a_playhead_stopped_while_waiting_is_due_at_once_when_drawn_again() {
        let (frames, timeline) = three(Loops::Infinite);
        let mut playhead = drawn(&frames, &timeline, ms(0));
        assert_eq!(
            playhead.advance(&timeline, ms(500), |_| false),
            Step::Waiting
        );
        playhead.set_drawn(false, &timeline, ms(600));
        assert_eq!(playhead.shown(&timeline, ms(600)), ms(100));
        playhead.set_drawn(true, &timeline, ms(900));
        assert_eq!(playhead.due(&timeline), Some(ms(900)));
    }

    /// A finite count plays exactly that many loops: the move off the last
    /// frame of the last loop completes it and the last frame stays, with
    /// nothing more due.
    #[test]
    fn after_the_last_loop_the_last_frame_stays() {
        let (frames, timeline) = three(Loops::from_plays(2));
        let mut playhead = drawn(&frames, &timeline, ms(0));
        let mut now = ms(0);
        let mut steps = Vec::new();
        while let Some(due) = playhead.due(&timeline) {
            now = due;
            steps.push(playhead.advance(&timeline, now, READY));
        }
        assert_eq!(
            steps,
            [
                Step::Advanced { looped: false },
                Step::Advanced { looped: false },
                Step::Advanced { looped: true },
                Step::Advanced { looped: false },
                Step::Advanced { looped: false },
                Step::Finished,
            ]
        );
        assert_eq!(now, ms(1200), "two loops of 600 ms");
        assert!(playhead.is_finished());
        assert_eq!(playhead.frame(), 2, "the last frame stays shown");
        assert_eq!(playhead.loops_completed(), 2);
        assert_eq!(playhead.steps(), 5, "finishing changes no pixels");
        assert_eq!(playhead.advance(&timeline, ms(60_000), READY), Step::Stay);

        // A file played once finishes without ever wrapping, and the last
        // frame needs nothing to be ready to finish.
        let (frames, timeline) = three(Loops::ONCE);
        let mut playhead = drawn(&frames, &timeline, ms(0));
        playhead.advance(&timeline, ms(100), READY);
        playhead.advance(&timeline, ms(300), READY);
        assert_eq!(
            playhead.advance(&timeline, ms(600), |_| false),
            Step::Finished
        );
        assert_eq!((playhead.frame(), playhead.loops_completed()), (2, 1));
    }

    /// An infinite count never finishes, and counts every loop it completes.
    #[test]
    fn an_infinite_count_counts_every_loop() {
        let (frames, timeline) = three(Loops::Infinite);
        let mut playhead = drawn(&frames, &timeline, ms(0));
        for _ in 0..30 {
            let due = playhead.due(&timeline).expect("running");
            playhead.advance(&timeline, due, READY);
        }
        assert_eq!(playhead.loops_completed(), 10);
        assert!(!playhead.is_finished());
    }

    /// Pausing stops the clock and keeps the frame; resuming continues from
    /// it; restarting goes back to the first frame with the loops reset.
    #[test]
    fn pause_resume_and_restart() {
        let (frames, timeline) = three(Loops::from_plays(1));
        let mut playhead = drawn(&frames, &timeline, ms(0));
        playhead.advance(&timeline, ms(100), READY);
        playhead.pause(&timeline, ms(150));
        assert!(playhead.is_paused() && !playhead.is_running(&timeline));
        assert_eq!(playhead.advance(&timeline, ms(5000), READY), Step::Stay);
        playhead.resume(&timeline, ms(5000));
        assert_eq!(playhead.due(&timeline), Some(ms(5150)));

        playhead.advance(&timeline, ms(5150), READY);
        assert_eq!(playhead.advance(&timeline, ms(5450), READY), Step::Finished);
        playhead.resume(&timeline, ms(6000));
        assert!(
            playhead.is_finished(),
            "resume does not replay a finished playhead"
        );

        let steps = playhead.steps();
        playhead.restart(&timeline, ms(7000));
        assert_eq!(
            (playhead.frame(), playhead.loops_completed()),
            (0, 0),
            "back to the first frame"
        );
        assert_eq!(playhead.steps(), steps + 1, "the frame shown changed");
        assert_eq!(playhead.due(&timeline), Some(ms(7100)));

        let paused = Playhead::new(PlayPolicy {
            loops: Loops::Infinite,
            paused: true,
        });
        assert!(paused.is_paused());
    }

    // --- the control surface of an element with one ---------------------

    /// A playhead of three frames (100, 200, 300 ms) under an element's own
    /// policy, drawn from zero.
    fn element(plays: u32, paused: bool) -> (Timeline, Playhead) {
        let (_, timeline) = three(Loops::Infinite);
        let mut playhead = Playhead::new(PlayPolicy {
            loops: Loops::from_plays(plays),
            paused,
        });
        playhead.set_drawn(true, &timeline, ms(0));
        (timeline, playhead)
    }

    /// Advances `playhead` at each instant it names until `until`, and
    /// answers the steps taken.
    fn play(playhead: &mut Playhead, timeline: &Timeline, until: Duration) -> Vec<Step> {
        let mut steps = Vec::new();
        while let Some(due) = playhead.due(timeline) {
            if due > until {
                break;
            }
            steps.push(playhead.advance(timeline, due, READY));
        }
        steps
    }

    /// Loop events of every step, in order: `true` for a final loop.
    fn loop_events(steps: &[Step]) -> Vec<bool> {
        steps
            .iter()
            .map(|step| step.loops_done())
            .filter(|done| done.completed > 0)
            .map(|done| done.finished)
            .collect()
    }

    /// N loops are exactly N loop completions, the last of them the final
    /// one, and then nothing more.
    #[test]
    fn n_loops_complete_n_times_then_once_finally() {
        for plays in 1..=4 {
            let (timeline, mut playhead) = element(plays, false);
            let steps = play(&mut playhead, &timeline, ms(60_000));
            let mut expected = vec![false; plays as usize - 1];
            expected.push(true);
            assert_eq!(loop_events(&steps), expected, "{plays} plays");
            assert_eq!(playhead.loops_completed(), plays);
            assert_eq!(playhead.frame(), 2, "the last frame stays shown");
        }
    }

    /// An infinite count completes every loop and never a final one.
    #[test]
    fn an_infinite_count_never_finishes() {
        let (timeline, mut playhead) = element(0, false);
        let steps = play(&mut playhead, &timeline, ms(6000));
        assert_eq!(loop_events(&steps), vec![false; 10]);
        assert!(!playhead.is_finished());
    }

    /// A playhead not drawn completes nothing, however long it waits, and
    /// one that waits for a frame completes nothing while it waits.
    #[test]
    fn nothing_completes_while_not_drawn_or_waiting() {
        let (timeline, mut playhead) = element(1, false);
        playhead.set_drawn(false, &timeline, ms(0));
        assert_eq!(playhead.advance(&timeline, ms(60_000), READY), Step::Stay);
        assert_eq!(playhead.loops_completed(), 0);

        let (timeline, mut playhead) = element(0, false);
        playhead.advance(&timeline, ms(100), READY);
        playhead.advance(&timeline, ms(300), READY);
        for at in [600, 1000, 5000] {
            let step = playhead.advance(&timeline, ms(at), |_| false);
            assert_eq!(step, Step::Waiting);
            assert_eq!(step.loops_done().completed, 0);
        }
        assert_eq!(playhead.loops_completed(), 0);
    }

    /// `autoplay` off: the playhead starts paused on the first frame and
    /// does not move.
    #[test]
    fn a_playhead_that_starts_paused_shows_the_first_frame() {
        let (timeline, mut playhead) = element(0, true);
        assert_eq!(playhead.due(&timeline), None);
        assert_eq!(playhead.advance(&timeline, ms(10_000), READY), Step::Stay);
        assert_eq!((playhead.frame(), playhead.steps()), (0, 0));
        playhead.control(Control::Resume, &timeline, ms(10_000));
        assert_eq!(playhead.due(&timeline), Some(ms(10_100)));
    }

    /// `pauseAnimation` stops advancing and keeps the frame shown;
    /// `resumeAnimation` continues from the paused position with the time
    /// the frame had left.
    #[test]
    fn pause_keeps_the_frame_and_resume_continues_from_it() {
        let (timeline, mut playhead) = element(0, false);
        playhead.advance(&timeline, ms(100), READY);
        playhead.control(Control::Pause, &timeline, ms(250));
        let steps = playhead.steps();
        assert_eq!(playhead.advance(&timeline, ms(9000), READY), Step::Stay);
        assert_eq!((playhead.frame(), playhead.steps()), (1, steps));
        playhead.control(Control::Resume, &timeline, ms(9000));
        assert_eq!(
            playhead.due(&timeline),
            Some(ms(9050)),
            "frame 1 had 50 ms left"
        );
        assert_eq!(playhead.frame(), 1);
    }

    /// `stopAnimation` stops advancing and moves the position to the first
    /// frame, with the loops reset, and the frame shown stays; the next
    /// `resumeAnimation` shows the first frame and plays from it.
    #[test]
    fn stop_moves_the_position_and_keeps_the_frame_shown() {
        let (timeline, mut playhead) = element(2, false);
        for at in [100, 300, 600, 700] {
            playhead.advance(&timeline, ms(at), READY);
        }
        assert_eq!((playhead.frame(), playhead.loops_completed()), (1, 1));
        let steps = playhead.steps();
        playhead.control(Control::Stop, &timeline, ms(750));
        assert!(playhead.is_stopped() && !playhead.is_running(&timeline));
        assert_eq!(playhead.position(), 0);
        assert_eq!(playhead.frame(), 1, "the frame shown stays");
        assert_eq!(playhead.steps(), steps, "so no pixels change");
        assert_eq!(playhead.loops_completed(), 0);
        assert_eq!(playhead.advance(&timeline, ms(9000), READY), Step::Stay);

        playhead.control(Control::Resume, &timeline, ms(9000));
        assert_eq!((playhead.frame(), playhead.position()), (0, 0));
        assert_eq!(playhead.steps(), steps + 1, "the first frame is shown");
        assert_eq!(playhead.due(&timeline), Some(ms(9100)));
        let steps = play(&mut playhead, &timeline, ms(60_000));
        assert_eq!(
            loop_events(&steps),
            [false, true],
            "both loops play again after the stop"
        );
    }

    /// A stop never owes a final loop: the loops it cut short are not
    /// completed.
    #[test]
    fn a_stop_owes_no_final_loop() {
        let (timeline, mut playhead) = element(1, false);
        playhead.advance(&timeline, ms(100), READY);
        playhead.control(Control::Stop, &timeline, ms(150));
        assert!(!playhead.is_finished());
        assert_eq!(playhead.loops_completed(), 0);
    }

    /// `resumeAnimation` after the final loop starts again at the first
    /// frame, with the loop count reset.
    #[test]
    fn resume_after_the_final_loop_restarts() {
        let (timeline, mut playhead) = element(1, false);
        let steps = play(&mut playhead, &timeline, ms(60_000));
        assert_eq!(loop_events(&steps), [true]);
        assert!(playhead.is_finished());
        playhead.control(Control::Resume, &timeline, ms(10_000));
        assert!(!playhead.is_finished());
        assert_eq!((playhead.frame(), playhead.loops_completed()), (0, 0));
        assert_eq!(playhead.due(&timeline), Some(ms(10_100)));
        let steps = play(&mut playhead, &timeline, ms(60_000));
        assert_eq!(loop_events(&steps), [true], "the count plays again");
    }

    /// `startAnimate` restarts at the first frame with the loops reset from
    /// every state: playing, paused, stopped and finished; and plays.
    #[test]
    fn start_animate_restarts_from_any_state() {
        /// Brings a playhead into the state a case names.
        type Enter = fn(&Timeline, &mut Playhead);
        let states: [(&str, Enter); 4] = [
            ("playing", |_, _| {}),
            ("paused", |timeline, playhead| {
                playhead.control(Control::Pause, timeline, ms(800));
            }),
            ("stopped", |timeline, playhead| {
                playhead.control(Control::Stop, timeline, ms(800));
            }),
            ("finished", |timeline, playhead| {
                play(playhead, timeline, ms(60_000));
            }),
        ];
        for (state, enter) in states {
            let (timeline, mut playhead) = element(3, false);
            for at in [100, 300, 600, 700] {
                playhead.advance(&timeline, ms(at), READY);
            }
            enter(&timeline, &mut playhead);
            playhead.control(Control::Restart, &timeline, ms(100_000));
            assert_eq!(
                (playhead.frame(), playhead.loops_completed()),
                (0, 0),
                "{state}"
            );
            assert!(!playhead.is_paused() && !playhead.is_finished(), "{state}");
            assert_eq!(playhead.due(&timeline), Some(ms(100_100)), "{state}");
        }
    }

    /// Every control answers alike whether or not the playhead was playing:
    /// pausing a paused playhead and resuming a playing one change nothing.
    #[test]
    fn controls_that_change_nothing_are_harmless() {
        let (timeline, mut playhead) = element(0, false);
        playhead.control(Control::Resume, &timeline, ms(50));
        assert_eq!(playhead.due(&timeline), Some(ms(100)));
        playhead.control(Control::Pause, &timeline, ms(50));
        playhead.control(Control::Pause, &timeline, ms(80));
        playhead.control(Control::Resume, &timeline, ms(1000));
        assert_eq!(playhead.due(&timeline), Some(ms(1050)));
        playhead.control(Control::Stop, &timeline, ms(1000));
        playhead.control(Control::Stop, &timeline, ms(1000));
        assert_eq!(playhead.frame(), 0);
    }
}
