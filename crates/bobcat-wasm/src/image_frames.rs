//! What the browser's decoder answers for an image's frames, from what
//! `WebCodecs`' `ImageDecoder` says of them: the pure half of
//! `platform::decode`, which the native test build checks too.
//!
//! An `ImageTrack` states its `repetitionCount` — how many times the
//! animation repeats after its first play, `Infinity` for forever — and each
//! frame's `VideoFrame` its `duration` in microseconds, or `null` when the
//! file states none (`WebCodecs`, "Image Decoding"). `vant` counts plays rather
//! than repetitions ([`Loops`]), so a count of `n` is `n + 1` plays, and a
//! frame with no stated duration is a delay of zero, which a playhead shows
//! for 100 ms as every browser does.

use std::time::Duration;

use vant::image::animation::{ImageFrames, Loops};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Track {
    pub(crate) durations: Vec<Option<f64>>,
    pub(crate) repetition_count: f64,
}

pub(crate) fn frames(track: Option<&Track>) -> ImageFrames {
    let Some(track) = track else {
        return ImageFrames::still();
    };
    if track.durations.len() < 2 {
        return ImageFrames::still();
    }
    let delays = track
        .durations
        .iter()
        .map(|duration| {
            duration
                .filter(|micros| micros.is_finite() && *micros > 0.0)
                .map_or(Duration::ZERO, |micros| {
                    Duration::from_secs_f64(micros / 1_000_000.0)
                })
        })
        .collect();
    ImageFrames::new(delays, loops(track.repetition_count))
}

fn loops(repetition_count: f64) -> Loops {
    if !repetition_count.is_finite() {
        return Loops::Infinite;
    }
    let repetitions = repetition_count.max(0.0).floor();
    if repetitions >= f64::from(u32::MAX) {
        return Loops::Infinite;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "floored and clamped to a non-negative u32 above"
    )]
    let repetitions = repetitions as u32;
    Loops::from_plays(repetitions + 1)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use vant::image::animation::{ImageFrames, Loops};

    use super::{Track, frames};

    #[test]
    fn a_browser_without_an_image_decoder_answers_one_frame() {
        assert_eq!(frames(None), ImageFrames::still());
        assert!(!frames(None).is_animated());
    }

    #[test]
    fn one_frame_is_still() {
        let track = Track {
            durations: vec![Some(100_000.0)],
            repetition_count: f64::INFINITY,
        };
        assert_eq!(frames(Some(&track)), ImageFrames::still());
    }

    #[test]
    fn durations_and_repetitions_become_delays_and_plays() {
        let track = Track {
            durations: vec![Some(100_000.0), None, Some(300_000.0)],
            repetition_count: 1.0,
        };
        let answer = frames(Some(&track));
        assert_eq!(
            answer.stated_delays(),
            [
                Duration::from_millis(100),
                Duration::ZERO,
                Duration::from_millis(300)
            ]
        );
        assert_eq!(answer.loops(), Loops::from_plays(2), "repeated once");

        let once = Track {
            repetition_count: 0.0,
            ..track.clone()
        };
        assert_eq!(frames(Some(&once)).loops(), Loops::ONCE);
        let forever = Track {
            repetition_count: f64::INFINITY,
            ..track
        };
        assert_eq!(frames(Some(&forever)).loops(), Loops::Infinite);
    }
}
