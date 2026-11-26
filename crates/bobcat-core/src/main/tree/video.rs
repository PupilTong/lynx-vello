//! The `video` component: Lynx's video element at its smallest useful size
//! (`docs/picture-source-design.md`, ruling 8).
//!
//! The element is a replaced box presenting a picture of its own
//! ([`PictureKey::Id`]), which the platform's media player fills
//! ([`vant::media`], served by the host through
//! [`ResourceFetcher::request_playback`](crate::resource::ResourceFetcher::request_playback)).
//! This thread holds the picture's id and the element's state, never a frame.
//!
//! # What is built
//!
//! - **Attributes.** `src` opens the video (a new picture per value, the one before ended), `loop`
//!   and `muted` set the player's two switches, and `object-fit` (`contain`, `cover`, `fill`) is a
//!   UA attribute rule onto the CSS property ([`UA_RULES`]), as `image`'s `mode` is. `contain` is
//!   native's default, so the rule for the bare tag writes it.
//! - **UI methods.** `play` and `pause`, through `__InvokeUIMethod`, each answering code 0 and
//!   `{success: true}`, the data Android and iOS both answer a successful call with
//!   (`LynxUIVideo.kt:376-385`, `LynxUIVideo.m:273-284`). A `play` with no `src` answers 8,
//!   `OPERATION_ERROR`, and fires an `error`, as both platforms do (`LynxUIVideo.kt:242-248`,
//!   `LynxUIVideo.m:225-231`).
//! - **Events.** `firstframe` with `{duration}` in seconds, `ended` with `{}`, and `error` with
//!   `{errorCode, errorMsg}`: the details native gives them (`video.d.ts`). They come back with the
//!   host's image reports ([`apply_reports`]) and are queued in [`ComponentEvents`], dispatched
//!   from the page's component-events entry like every component's.
//!
//! # Playback starts on `play`
//!
//! Setting `src` opens the video paused on its first frame, which is shown and fires `firstframe`;
//! nothing plays until `play`. Both platforms load a source without playing it: iOS replaces the
//! player's item and pauses (`LynxVideoView.m:345-378`), Android prepares the `ExoPlayer` with
//! `playWhenReady` left false (`LynxVideoView.kt:204-224`), and each reports its first frame
//! before any `play` (`readyForDisplay`, `onRenderedFirstFrame`). A `play` after the end starts
//! again from the beginning on both (`LynxVideoView.m:210-232`, `LynxVideoView.kt:288-290`).
//!
//! # The natural size
//!
//! The box is sized like any replaced element, from the video's natural size, which arrives with
//! `firstframe`: until then the element presents its picture with none, and draws nothing because
//! no frame has been published. Unlike `image`, nothing contains its size.
//!
//! # Error codes
//!
//! iOS's two (`LynxVideoDefines.h:16-19`): `-1` (`LynxVideoErrorInvalidSource`) for a source that
//! could not be opened, a `play` with none included, and `-2` (`LynxVideoErrorOperation`) for
//! everything else: a source that opened and did not play, and a platform with no player. Android
//! reports `ExoPlayer`'s own codes, which name nothing a card could read on another platform, so
//! iOS's are followed.
//!
//! # Not built
//!
//! `volume`, `speed`, `mode` and `timeupdate-interval` are ignored as attributes; `stop` and
//! `seek` answer as methods the element does not have; `playing`, `paused`, `stopped`,
//! `timeupdate`, `looped` and `buffering` are never fired. `docs/tracking/deviations.md` has the
//! list and the other differences: a `play` answers at once rather than when playback starts, and
//! `src` is not restricted to `http` and `https`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use smallvec::smallvec;
use terminus::link::{
    CustomElement, ImageEvent, MediaCommand, MediaErrorKind, MediaEvent, MediaSettings, MethodCall,
    MethodError, MethodOutcome, NodeId, PictureId, PictureKey, PlaybackReport, PlaybackRequest,
};

use super::{ComponentDetail, ComponentEvent, ComponentEvents, Field, LynxDocument};
use crate::main::record::write_record_field;

pub(crate) const VIDEO_TAG: &str = "video";
const SRC_ATTRIBUTE: &str = "src";
const LOOP_ATTRIBUTE: &str = "loop";
const MUTED_ATTRIBUTE: &str = "muted";

pub(crate) const FIRST_FRAME_EVENT: &str = "firstframe";
pub(crate) const ENDED_EVENT: &str = "ended";
pub(crate) const ERROR_EVENT: &str = "error";

const INVALID_SOURCE_CODE: i32 = -1;
const OPERATION_CODE: i32 = -2;

const MISSING_SOURCE_MESSAGE: &str = "missing video source";

pub(super) const UA_RULES: &str = r#"video { display: flex; object-fit: contain; }
video[object-fit="cover"] { object-fit: cover; }
video[object-fit="fill"] { object-fit: fill; }
video > * { display: none; }
"#;

pub(super) fn define(document: &mut LynxDocument, events: ComponentEvents, videos: Videos) {
    document.define(VIDEO_TAG, Box::new(Video { events, videos }));
}

#[derive(Debug)]
struct Opened {
    source: Arc<str>,
    picture: PictureId,
    pending: bool,
}

#[derive(Debug)]
enum Queued {
    Open {
        element: NodeId,
        picture: PictureId,
        source: Arc<str>,
    },
    Request(PlaybackRequest),
}

#[derive(Debug, Default)]
struct State {
    elements: FxHashMap<NodeId, Opened>,
    players: FxHashMap<PictureId, NodeId>,
    queued: Vec<Queued>,
}

impl State {
    fn close(&mut self, element: NodeId) {
        if let Some(opened) = self.elements.remove(&element) {
            self.players.remove(&opened.picture);
            self.queued.push(Queued::Request(PlaybackRequest::End {
                picture: opened.picture,
            }));
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Videos(Rc<RefCell<State>>);

impl Videos {
    fn set_source(&self, document: &mut LynxDocument, element: NodeId, source: Option<&str>) {
        let picture = {
            let mut state = self.0.borrow_mut();
            let held = state
                .elements
                .get(&element)
                .map(|opened| Arc::clone(&opened.source));
            if held.as_deref() == source {
                return;
            }
            state.close(element);
            source.map(|source| {
                let source: Arc<str> = Arc::from(source);
                let picture = PictureId::new();
                state.elements.insert(
                    element,
                    Opened {
                        source: Arc::clone(&source),
                        picture,
                        pending: true,
                    },
                );
                state.players.insert(picture, element);
                state.queued.push(Queued::Open {
                    element,
                    picture,
                    source,
                });
                picture
            })
        };
        document.set_picture(element, picture.map(|id| (PictureKey::Id(id), None)));
    }

    fn command(&self, element: NodeId, command: MediaCommand) -> bool {
        let mut state = self.0.borrow_mut();
        let Some((picture, pending)) = state
            .elements
            .get(&element)
            .map(|opened| (opened.picture, opened.pending))
        else {
            return false;
        };
        if pending && matches!(command, MediaCommand::Loop(_) | MediaCommand::Muted(_)) {
            return true;
        }
        state
            .queued
            .push(Queued::Request(PlaybackRequest::Media { picture, command }));
        true
    }

    pub(crate) fn take_requests(&self, document: &LynxDocument) -> Vec<PlaybackRequest> {
        let queued = {
            let mut state = self.0.borrow_mut();
            for opened in state.elements.values_mut() {
                opened.pending = false;
            }
            std::mem::take(&mut state.queued)
        };
        queued
            .into_iter()
            .map(|queued| match queued {
                Queued::Request(request) => request,
                Queued::Open {
                    element,
                    picture,
                    source,
                } => {
                    let node = document.get(element);
                    let switch = |name: &str| {
                        node.and_then(|node| node.attribute(name))
                            .is_some_and(|value| is_on(Some(&value)))
                    };
                    PlaybackRequest::Open {
                        picture,
                        source,
                        settings: MediaSettings {
                            looping: switch(LOOP_ATTRIBUTE),
                            muted: switch(MUTED_ATTRIBUTE),
                        },
                    }
                }
            })
            .collect()
    }

    fn forget(&self, element: NodeId) {
        self.0.borrow_mut().close(element);
    }

    #[cfg(test)]
    pub(crate) fn picture(&self, element: NodeId) -> Option<PictureId> {
        self.0
            .borrow()
            .elements
            .get(&element)
            .map(|opened| opened.picture)
    }
}

pub(crate) fn apply_reports(
    document: &mut LynxDocument,
    videos: &Videos,
    events: &[ImageEvent],
) -> Vec<ComponentEvent> {
    let mut owed = Vec::new();
    for event in events {
        let ImageEvent::Playback {
            picture,
            report: PlaybackReport::Media(event),
        } = event
        else {
            continue;
        };
        let Some(node) = videos.0.borrow().players.get(picture).copied() else {
            continue;
        };
        match event {
            MediaEvent::FirstFrame {
                width,
                height,
                duration,
            } => {
                document.set_picture(
                    node,
                    Some((PictureKey::Id(*picture), Some((*width, *height)))),
                );
                owed.push(ComponentEvent::Detailed {
                    node,
                    name: FIRST_FRAME_EVENT,
                    detail: ComponentDetail::Fields(smallvec![(
                        "duration",
                        Field::Number(duration.as_secs_f64())
                    )]),
                });
            }
            MediaEvent::Ended => owed.push(ComponentEvent::Plain {
                node,
                name: ENDED_EVENT,
            }),
            MediaEvent::Failed(error) => owed.push(error_event(
                node,
                match error.kind {
                    MediaErrorKind::Source => INVALID_SOURCE_CODE,
                    MediaErrorKind::Playback | MediaErrorKind::Unsupported => OPERATION_CODE,
                },
                error.message.clone(),
            )),
        }
    }
    owed
}

fn error_event(node: NodeId, code: i32, message: String) -> ComponentEvent {
    ComponentEvent::Detailed {
        node,
        name: ERROR_EVENT,
        detail: ComponentDetail::Error { code, message },
    }
}

fn is_on(value: Option<&str>) -> bool {
    value.is_some_and(|value| value != "false")
}

struct Video {
    events: ComponentEvents,
    videos: Videos,
}

impl CustomElement<()> for Video {
    fn observed_attributes(&self) -> Vec<String> {
        vec![
            SRC_ATTRIBUTE.to_owned(),
            LOOP_ATTRIBUTE.to_owned(),
            MUTED_ATTRIBUTE.to_owned(),
        ]
    }

    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        _old: Option<&str>,
        new: Option<&str>,
    ) {
        match name {
            SRC_ATTRIBUTE => {
                self.videos
                    .set_source(document, element, new.filter(|value| !value.is_empty()));
            }
            LOOP_ATTRIBUTE => {
                let _ = self.videos.command(element, MediaCommand::Loop(is_on(new)));
            }
            MUTED_ATTRIBUTE => {
                let _ = self
                    .videos
                    .command(element, MediaCommand::Muted(is_on(new)));
            }
            other => debug_assert!(false, "`video` does not observe `{other}`"),
        }
    }

    fn freed(&self, element: NodeId) {
        self.videos.forget(element);
    }

    fn invoke(
        &self,
        _document: &mut LynxDocument,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        let command = match call.name {
            "play" => MediaCommand::Play,
            "pause" => MediaCommand::Pause,
            _ => return MethodOutcome::NotFound,
        };
        if !self.videos.command(element, command) && command == MediaCommand::Play {
            self.events.extend([error_event(
                element,
                INVALID_SOURCE_CODE,
                MISSING_SOURCE_MESSAGE.to_owned(),
            )]);
            return MethodOutcome::Failed(MethodError::Operation);
        }
        let mut record = String::new();
        write_record_field(&mut record, "1");
        MethodOutcome::Data(record)
    }
}

#[cfg(test)]
#[path = "video_tests.rs"]
mod tests;
