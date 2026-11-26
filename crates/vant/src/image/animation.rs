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

pub const SHORT_DELAY_LIMIT: Duration = Duration::from_millis(10);

pub const SHORT_DELAY_PLAYS_AS: Duration = Duration::from_millis(100);

#[must_use]
pub fn played_delay(stated: Duration) -> Duration {
    if stated <= SHORT_DELAY_LIMIT {
        SHORT_DELAY_PLAYS_AS
    } else {
        stated
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Loops {
    Infinite,
    Plays(NonZeroU32),
}

impl Loops {
    pub const ONCE: Self = Self::Plays(NonZeroU32::MIN);

    #[must_use]
    pub fn from_plays(plays: u32) -> Self {
        NonZeroU32::new(plays).map_or(Self::Infinite, Self::Plays)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageFrames {
    delays: Arc<[Duration]>,
    loops: Loops,
}

impl ImageFrames {
    #[must_use]
    pub fn still() -> Self {
        Self {
            delays: Arc::from([Duration::ZERO]),
            loops: Loops::ONCE,
        }
    }

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

    #[must_use]
    pub fn count(&self) -> usize {
        self.delays.len()
    }

    #[must_use]
    pub fn is_animated(&self) -> bool {
        self.count() > 1
    }

    #[must_use]
    pub fn stated_delays(&self) -> &[Duration] {
        &self.delays
    }

    #[must_use]
    pub const fn loops(&self) -> Loops {
        self.loops
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timeline {
    delays: Arc<[Duration]>,
}

impl Timeline {
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

    #[must_use]
    pub fn len(&self) -> usize {
        self.delays.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.delays.is_empty()
    }

    #[must_use]
    pub fn delay(&self, frame: usize) -> Duration {
        self.delays[frame]
    }

    #[must_use]
    pub fn next(&self, frame: usize) -> usize {
        (frame + 1) % self.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayPolicy {
    pub loops: Loops,
    pub paused: bool,
}

impl PlayPolicy {
    #[must_use]
    pub const fn stated(frames: &ImageFrames) -> Self {
        Self {
            loops: frames.loops(),
            paused: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Control {
    Restart,
    Pause,
    Resume,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopsDone {
    pub completed: u32,
    pub finished: bool,
}

impl Step {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Stay,
    Waiting,
    Advanced { looped: bool },
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Playing,
    Paused,
    Stopped,
    Finished,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playhead {
    frame: usize,
    loops: u32,
    state: State,
    drawn: bool,
    shown: Duration,
    since: Option<Duration>,
    steps: u64,
    policy: PlayPolicy,
}

impl Playhead {
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

    #[must_use]
    pub const fn frame(&self) -> usize {
        self.frame
    }

    #[must_use]
    pub const fn position(&self) -> usize {
        if self.is_stopped() { 0 } else { self.frame }
    }

    #[must_use]
    pub const fn is_stopped(&self) -> bool {
        matches!(self.state, State::Stopped)
    }

    #[must_use]
    pub const fn loops_completed(&self) -> u32 {
        self.loops
    }

    #[must_use]
    pub const fn steps(&self) -> u64 {
        self.steps
    }

    #[must_use]
    pub const fn is_paused(&self) -> bool {
        matches!(self.state, State::Paused | State::Stopped)
    }

    #[must_use]
    pub const fn is_drawn(&self) -> bool {
        self.drawn
    }

    #[must_use]
    pub const fn is_finished(&self) -> bool {
        matches!(self.state, State::Finished)
    }

    #[must_use]
    pub const fn policy(&self) -> PlayPolicy {
        self.policy
    }

    #[must_use]
    pub fn is_running(&self, timeline: &Timeline) -> bool {
        self.drawn && self.state == State::Playing && timeline.len() > 1
    }

    #[must_use]
    pub fn shown(&self, timeline: &Timeline, now: Duration) -> Duration {
        let running = self
            .since
            .map_or(Duration::ZERO, |since| now.saturating_sub(since));
        (self.shown + running).min(timeline.delay(self.frame))
    }

    #[must_use]
    pub fn due(&self, timeline: &Timeline) -> Option<Duration> {
        let since = self.since?;
        Some(since + timeline.delay(self.frame).saturating_sub(self.shown))
    }

    pub fn set_drawn(&mut self, drawn: bool, timeline: &Timeline, now: Duration) {
        if self.drawn == drawn {
            return;
        }
        self.stop(timeline, now);
        self.drawn = drawn;
        self.start(timeline, now);
    }

    pub fn pause(&mut self, timeline: &Timeline, now: Duration) {
        self.stop(timeline, now);
        if self.state == State::Playing {
            self.state = State::Paused;
        }
    }

    pub fn resume(&mut self, timeline: &Timeline, now: Duration) {
        if self.state == State::Paused {
            self.state = State::Playing;
        }
        self.start(timeline, now);
    }

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

    fn play_from_start(&mut self, timeline: &Timeline, now: Duration) {
        self.state = State::Playing;
        self.restart(timeline, now);
    }

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

    fn stop(&mut self, timeline: &Timeline, now: Duration) {
        self.shown = self.shown(timeline, now);
        self.since = None;
    }

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

    fn three(loops: Loops) -> (ImageFrames, Timeline) {
        let frames = ImageFrames::new(vec![ms(100), ms(200), ms(300)], loops);
        let timeline = Timeline::new(&frames);
        (frames, timeline)
    }

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

    #[test]
    fn a_playhead_not_drawn_stops_and_continues_from_where_it_stopped() {
        let (frames, timeline) = three(Loops::Infinite);
        let mut playhead = drawn(&frames, &timeline, ms(0));
        assert_eq!(
            playhead.advance(&timeline, ms(100), READY),
            Step::Advanced { looped: false }
        );
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

    fn element(plays: u32, paused: bool) -> (Timeline, Playhead) {
        let (_, timeline) = three(Loops::Infinite);
        let mut playhead = Playhead::new(PlayPolicy {
            loops: Loops::from_plays(plays),
            paused,
        });
        playhead.set_drawn(true, &timeline, ms(0));
        (timeline, playhead)
    }

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

    fn loop_events(steps: &[Step]) -> Vec<bool> {
        steps
            .iter()
            .map(|step| step.loops_done())
            .filter(|done| done.completed > 0)
            .map(|done| done.finished)
            .collect()
    }

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

    #[test]
    fn an_infinite_count_never_finishes() {
        let (timeline, mut playhead) = element(0, false);
        let steps = play(&mut playhead, &timeline, ms(6000));
        assert_eq!(loop_events(&steps), vec![false; 10]);
        assert!(!playhead.is_finished());
    }

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

    #[test]
    fn a_playhead_that_starts_paused_shows_the_first_frame() {
        let (timeline, mut playhead) = element(0, true);
        assert_eq!(playhead.due(&timeline), None);
        assert_eq!(playhead.advance(&timeline, ms(10_000), READY), Step::Stay);
        assert_eq!((playhead.frame(), playhead.steps()), (0, 0));
        playhead.control(Control::Resume, &timeline, ms(10_000));
        assert_eq!(playhead.due(&timeline), Some(ms(10_100)));
    }

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

    #[test]
    fn a_stop_owes_no_final_loop() {
        let (timeline, mut playhead) = element(1, false);
        playhead.advance(&timeline, ms(100), READY);
        playhead.control(Control::Stop, &timeline, ms(150));
        assert!(!playhead.is_finished());
        assert_eq!(playhead.loops_completed(), 0);
    }

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

    #[test]
    fn start_animate_restarts_from_any_state() {
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
