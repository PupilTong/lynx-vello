//! [`BrowserMedia`]: the browser's video player, an `HTMLVideoElement` on the
//! page's main thread whose frames are copied into the engine's textures in
//! the Render Worker.
//!
//! An `HTMLVideoElement` exists only where a `Document` does, so the player
//! has two halves. This one, in the Render Worker, holds each open video's
//! sink and posts the facade every instruction (`bobcat-media`: open, play,
//! pause, loop, muted, close; `js/protocol.d.ts`). The facade's half
//! (`js/media.ts`) owns a detached `<video>` per video, takes each frame it
//! presents with `requestVideoFrameCallback`, makes a `VideoFrame` of it and
//! transfers that here (`bobcat-media-frame`), and posts what the element
//! reports (`bobcat-media-event`): its first frame (`loadeddata`, with
//! `videoWidth`, `videoHeight` and `duration`), its end (`ended`, which a
//! looping element never fires) and a failure (`error`, or a `play()` the
//! browser rejected, such as an unmuted play outside a user gesture, which
//! is reported as it is rather than worked around). The Render Worker's
//! script hands both to [`deliver_media_frame`] and [`deliver_media_event`],
//! which find the video's sink by its number.
//!
//! A frame becomes a [`BrowserFrame`] of the `VideoFrame`, written with
//! `copyExternalImageToTexture` and closed when its last copy goes. The
//! element's own clock paces the frames: the browser plays its audio and
//! presents each frame against it, and the frame callback runs as it does.
//! A frame that arrives for a video nobody draws is closed at once by the
//! sink, which keeps only the newest.
//!
//! The player reads `http` and `https` URLs itself; anything else is handed
//! over as its bytes, which the facade plays from a `blob:` URL.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use bobcat_core::{ImageDescriptor, PictureFrame};
use js_sys::{Object, Reflect, Uint8Array};
use vant::media::{
    Media, MediaError, MediaErrorKind, MediaEvent, MediaPlayer, MediaSettings, MediaSink,
    MediaSource,
};
use wasm_bindgen::prelude::*;

use super::BrowserFrame;

trait OpenVideo {
    fn frame(&self, frame: BrowserFrame);
    fn event(&self, event: MediaEvent);
}

impl<S: MediaSink<BrowserFrame>> OpenVideo for S {
    fn frame(&self, frame: BrowserFrame) {
        let (width, height) = frame.size();
        MediaSink::frame(
            self,
            PictureFrame::Platform(frame),
            ImageDescriptor::platform(width, height),
        );
    }

    fn event(&self, event: MediaEvent) {
        MediaSink::event(self, event);
    }
}

thread_local! {
    static OPEN: RefCell<HashMap<u32, Rc<dyn OpenVideo>>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u32> = const { Cell::new(1) };
}

#[derive(Clone, Debug)]
pub struct BrowserMedia {
    post: js_sys::Function,
}

impl BrowserMedia {
    #[must_use]
    pub fn new(post: js_sys::Function) -> Self {
        Self { post }
    }
}

fn post(function: &js_sys::Function, player: u32, op: &str, fields: &[(&str, JsValue)]) {
    let message = Object::new();
    let set = |key: &str, value: &JsValue| {
        let _ = Reflect::set(&message, &JsValue::from_str(key), value);
    };
    set("type", &JsValue::from_str("bobcat-media"));
    set("player", &JsValue::from(player));
    set("op", &JsValue::from_str(op));
    for (key, value) in fields {
        set(key, value);
    }
    let _ = function.call1(&JsValue::NULL, &message);
}

impl Media<BrowserFrame> for BrowserMedia {
    type Player = BrowserPlayer;

    fn open<S: MediaSink<BrowserFrame>>(
        &self,
        source: MediaSource,
        settings: MediaSettings,
        sink: S,
    ) -> Result<BrowserPlayer, MediaError> {
        let player = NEXT.with(|next| {
            let player = next.get();
            next.set(player.wrapping_add(1).max(1));
            player
        });
        let (url, bytes, media_type) = match source {
            MediaSource::Url(url) => (url, JsValue::NULL, JsValue::NULL),
            MediaSource::Bytes {
                url,
                bytes,
                media_type,
            } => (
                url,
                Uint8Array::from(&bytes[..]).into(),
                media_type.map_or(JsValue::NULL, |media_type| JsValue::from_str(&media_type)),
            ),
        };
        OPEN.with(|open| {
            open.borrow_mut()
                .insert(player, Rc::new(sink) as Rc<dyn OpenVideo>);
        });
        post(
            &self.post,
            player,
            "open",
            &[
                ("url", JsValue::from_str(url.as_str())),
                ("bytes", bytes),
                ("mediaType", media_type),
                ("looping", JsValue::from_bool(settings.looping)),
                ("muted", JsValue::from_bool(settings.muted)),
            ],
        );
        Ok(BrowserPlayer {
            player,
            post: self.post.clone(),
        })
    }
}

#[derive(Debug)]
pub struct BrowserPlayer {
    player: u32,
    post: js_sys::Function,
}

impl MediaPlayer for BrowserPlayer {
    fn play(&self) {
        post(&self.post, self.player, "play", &[]);
    }

    fn pause(&self) {
        post(&self.post, self.player, "pause", &[]);
    }

    fn set_loop(&self, looping: bool) {
        post(
            &self.post,
            self.player,
            "loop",
            &[("value", JsValue::from_bool(looping))],
        );
    }

    fn set_muted(&self, muted: bool) {
        post(
            &self.post,
            self.player,
            "muted",
            &[("value", JsValue::from_bool(muted))],
        );
    }
}

impl Drop for BrowserPlayer {
    fn drop(&mut self) {
        OPEN.with(|open| open.borrow_mut().remove(&self.player));
        post(&self.post, self.player, "close", &[]);
    }
}

fn open_video(player: u32) -> Option<Rc<dyn OpenVideo>> {
    OPEN.with(|open| open.borrow().get(&player).cloned())
}

#[wasm_bindgen(js_name = deliverMediaFrame)]
pub fn deliver_media_frame(player: u32, frame: web_sys::VideoFrame) {
    let frame = BrowserFrame::video(frame);
    if let Some(video) = open_video(player) {
        video.frame(frame);
    }
}

#[wasm_bindgen(js_name = deliverMediaEvent)]
pub fn deliver_media_event(
    player: u32,
    event: &str,
    width: u32,
    height: u32,
    duration: f64,
    kind: &str,
    message: String,
) {
    let Some(video) = open_video(player) else {
        return;
    };
    let event = match event {
        "firstframe" => MediaEvent::FirstFrame {
            width,
            height,
            duration: Duration::try_from_secs_f64(duration).unwrap_or(Duration::ZERO),
        },
        "ended" => MediaEvent::Ended,
        "error" => MediaEvent::Failed(MediaError::new(
            if kind == "source" {
                MediaErrorKind::Source
            } else {
                MediaErrorKind::Playback
            },
            message,
        )),
        _ => return,
    };
    video.event(event);
}
