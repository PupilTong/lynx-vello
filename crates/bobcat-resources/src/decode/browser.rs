//! Browser: `createImageBitmap` in a dedicated image worker, with a
//! shared-memory mailbox so a read that must not miss can wait for it.
//!
//! The browser's decoder is asynchronous and lives behind `createImageBitmap`
//! and an `OffscreenCanvas`, neither of which the Render Worker can call
//! synchronously — and a load's first decode does not need to. A restore
//! does: after an eviction, `FrameImages::read` has to hand back pixels
//! inside the call. So decoding runs in its own Worker, which never blocks,
//! and every job has a mailbox in Wasm memory — a few `i32` words the two
//! sides read with `Atomics`. The image worker writes the decoded size and
//! signals; this side allocates the pixel buffer in its own heap and
//! signals back; the image worker copies the pixels straight into that
//! buffer and signals once more. An asynchronous load waits for those
//! signals through `postMessage` echoes on the event loop; a restore waits
//! on them with `Atomics.wait`, which a dedicated Worker may do. The
//! protocol lives in `image-worker.js`, shipped by `bobcat-wasm`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicI32, Ordering, fence};
use std::time::Duration;

use js_sys::{Array, Int32Array, Object, Reflect, Uint8Array};
use rustc_hash::FxHashMap;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{MessageEvent, Worker};

use super::{Bitmap, DecodeError};
use crate::image_header::ImageHeader;

const STATE_DECODING: i32 = 0;
const STATE_DIMENSIONS: i32 = 1;
const STATE_BUFFER: i32 = 2;
const STATE_DONE: i32 = 3;
const STATE_FAILED: i32 = -1;

const WORD_STATE: usize = 0;
const WORD_WIDTH: usize = 1;
const WORD_HEIGHT: usize = 2;
const WORD_SOURCE_WIDTH: usize = 3;
const WORD_SOURCE_HEIGHT: usize = 4;
const WORD_POINTER: usize = 5;
const WORD_LENGTH: usize = 6;
const WORDS: usize = 8;
/// The mailbox's word count and state index as the JS typed-array API
/// wants them.
const MAILBOX_WORDS: u32 = 8;
const STATE_INDEX: u32 = 0;

/// How long a blocking restore waits before giving the frame up.
const RESTORE_TIMEOUT: Duration = Duration::from_secs(10);

type Mailbox = Box<[AtomicI32; WORDS]>;

struct Job {
    mailbox: Mailbox,
    /// The pixel buffer, allocated once the size is known; the image worker
    /// writes into it directly.
    buffer: Option<Vec<u8>>,
    /// Where an asynchronous decode's result goes.
    reply: Option<flume::Sender<Result<Bitmap, DecodeError>>>,
}

/// The image worker and the jobs in flight on it.
pub(crate) struct ImageWorker {
    worker: Worker,
    memory: js_sys::WebAssembly::Memory,
    jobs: RefCell<FxHashMap<u32, Job>>,
    next_id: Cell<u32>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

impl std::fmt::Debug for ImageWorker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImageWorker")
            .field("jobs", &self.jobs.borrow().len())
            .finish_non_exhaustive()
    }
}

impl ImageWorker {
    /// Starts the worker at `url` and hands it this instance's memory.
    pub(crate) fn new(url: &str) -> Result<Rc<Self>, String> {
        let worker = Worker::new(url).map_err(|error| describe(&error))?;
        let memory = wasm_bindgen::memory().unchecked_into::<js_sys::WebAssembly::Memory>();
        let init = Object::new();
        set(&init, "type", &"init".into());
        set(&init, "memory", &memory);
        worker
            .post_message(&init)
            .map_err(|error| describe(&error))?;
        Ok(Rc::new_cyclic(|weak: &Weak<Self>| {
            let weak = weak.clone();
            let on_message = Closure::new(move |event: MessageEvent| {
                if let Some(worker) = weak.upgrade() {
                    worker.on_message(&event.data());
                }
            });
            worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            Self {
                worker,
                memory,
                jobs: RefCell::new(FxHashMap::default()),
                next_id: Cell::new(1),
                _on_message: on_message,
            }
        }))
    }

    /// Decodes on the event loop: the job's echoes drive it to completion.
    pub(crate) async fn decode(
        &self,
        bytes: &[u8],
        header: Option<ImageHeader>,
        max: (u32, u32),
    ) -> Result<Bitmap, DecodeError> {
        let (sender, receiver) = flume::bounded(1);
        self.start(bytes, header, max, Some(sender))?;
        receiver.recv_async().await.unwrap_or_else(|_| {
            Err(DecodeError::Unavailable(
                "the image worker went away before answering".to_owned(),
            ))
        })
    }

    /// Decodes while blocking this Worker on the mailbox, for a restore.
    pub(crate) fn decode_blocking(
        &self,
        bytes: &[u8],
        header: Option<ImageHeader>,
        max: (u32, u32),
    ) -> Result<Bitmap, DecodeError> {
        let id = self.start(bytes, header, max, None)?;
        let view = self.mailbox_view(id)?;
        let deadline = web_time::Instant::now() + RESTORE_TIMEOUT;
        let outcome = (|| {
            let state = wait_until(&view, deadline, |state| state != STATE_DECODING)?;
            if state == STATE_FAILED {
                return Err(DecodeError::Malformed(
                    "the image worker could not decode the image".to_owned(),
                ));
            }
            self.provide_buffer(id)?;
            let state = wait_until(&view, deadline, |state| {
                state == STATE_DONE || state == STATE_FAILED
            })?;
            if state == STATE_FAILED {
                return Err(DecodeError::Malformed(
                    "the image worker could not deliver the pixels".to_owned(),
                ));
            }
            Ok(())
        })();
        match outcome {
            Ok(()) => self
                .finish(id)
                .ok_or_else(|| DecodeError::Unavailable("the decode job vanished".to_owned())),
            Err(error) => {
                // On a timeout the worker may still write into the mailbox
                // and buffer later, so the job — and its allocations — must
                // stay until an echo retires it.
                if !matches!(&error, DecodeError::Unavailable(message) if message.contains("timed out"))
                {
                    self.jobs.borrow_mut().remove(&id);
                }
                Err(error)
            }
        }
    }

    fn start(
        &self,
        bytes: &[u8],
        // The worker sizes its resize from the bitmap it decoded rather than
        // from the header, which cannot see an EXIF orientation; the header
        // is accepted for parity with the native decoders and otherwise
        // unused here.
        _header: Option<ImageHeader>,
        max: (u32, u32),
        reply: Option<flume::Sender<Result<Bitmap, DecodeError>>>,
    ) -> Result<u32, DecodeError> {
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1).max(1));
        let mailbox: Mailbox = Box::new(std::array::from_fn(|_| AtomicI32::new(0)));
        let address = address_word(mailbox.as_ptr().addr())?.cast_unsigned();
        let payload = Uint8Array::from(bytes);
        let message = Object::new();
        set(&message, "type", &"decode".into());
        set(&message, "id", &JsValue::from_f64(f64::from(id)));
        set(&message, "mailbox", &JsValue::from_f64(f64::from(address)));
        set(&message, "bytes", &payload);
        set(&message, "maxWidth", &JsValue::from_f64(f64::from(max.0)));
        set(&message, "maxHeight", &JsValue::from_f64(f64::from(max.1)));
        let transfer = Array::new();
        transfer.push(&payload.buffer());
        self.jobs.borrow_mut().insert(
            id,
            Job {
                mailbox,
                buffer: None,
                reply,
            },
        );
        if let Err(error) = self.worker.post_message_with_transfer(&message, &transfer) {
            self.jobs.borrow_mut().remove(&id);
            return Err(DecodeError::Unavailable(describe(&error)));
        }
        Ok(id)
    }

    /// The image worker's echoes: `dims`, `done`, `error`.
    fn on_message(&self, data: &JsValue) {
        let kind = Reflect::get(data, &"type".into())
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default();
        let Some(id) = Reflect::get(data, &"id".into())
            .ok()
            .and_then(|value| value.as_f64())
            .and_then(job_id)
        else {
            return;
        };
        match kind.as_str() {
            "dims" => {
                let state = self
                    .jobs
                    .borrow()
                    .get(&id)
                    .map(|job| job.mailbox[WORD_STATE].load(Ordering::SeqCst));
                // A blocking restore may already have provided the buffer.
                if state == Some(STATE_DIMENSIONS)
                    && let Err(error) = self.provide_buffer(id)
                {
                    self.fail(id, error);
                }
            }
            "done" => {
                if let Some((bitmap, reply)) = self.finish(id).zip(self.take_reply(id)) {
                    let _ = reply.send(Ok(bitmap));
                }
            }
            "error" => {
                let message = Reflect::get(data, &"message".into())
                    .ok()
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| "unknown image worker failure".to_owned());
                self.fail(id, DecodeError::Malformed(message));
            }
            _ => {}
        }
    }

    /// Allocates the pixel buffer the mailbox's size asks for and tells the
    /// image worker where it is.
    fn provide_buffer(&self, id: u32) -> Result<(), DecodeError> {
        let mut jobs = self.jobs.borrow_mut();
        let Some(job) = jobs.get_mut(&id) else {
            return Ok(());
        };
        let width = job.mailbox[WORD_WIDTH].load(Ordering::SeqCst);
        let height = job.mailbox[WORD_HEIGHT].load(Ordering::SeqCst);
        let (Ok(width), Ok(height)) = (usize::try_from(width), usize::try_from(height)) else {
            return Err(DecodeError::Malformed(
                "the decoded size is negative".to_owned(),
            ));
        };
        let length = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .filter(|length| *length > 0)
            .and_then(|length| i32::try_from(length).ok())
            .ok_or_else(|| {
                DecodeError::Malformed("the decoded size is empty or too large".to_owned())
            })?;
        let mut buffer = vec![0_u8; length.unsigned_abs() as usize];
        job.mailbox[WORD_POINTER]
            .store(address_word(buffer.as_mut_ptr().addr())?, Ordering::SeqCst);
        job.mailbox[WORD_LENGTH].store(length, Ordering::SeqCst);
        job.buffer = Some(buffer);
        job.mailbox[WORD_STATE].store(STATE_BUFFER, Ordering::SeqCst);
        drop(jobs);
        let message = Object::new();
        set(&message, "type", &"buffer".into());
        set(&message, "id", &JsValue::from_f64(f64::from(id)));
        self.worker
            .post_message(&message)
            .map_err(|error| DecodeError::Unavailable(describe(&error)))
    }

    /// Takes a finished job's pixels as a bitmap.
    fn finish(&self, id: u32) -> Option<Bitmap> {
        let mut jobs = self.jobs.borrow_mut();
        let job = jobs.get_mut(&id)?;
        if job.mailbox[WORD_STATE].load(Ordering::SeqCst) != STATE_DONE {
            return None;
        }
        // The image worker wrote the buffer before it stored `DONE`; the
        // acquire above orders those writes before the reads below.
        fence(Ordering::SeqCst);
        let buffer = job.buffer.take()?;
        let word =
            |index: usize| u32::try_from(job.mailbox[index].load(Ordering::SeqCst)).unwrap_or(0);
        let bitmap = Bitmap {
            width: word(WORD_WIDTH),
            height: word(WORD_HEIGHT),
            source_width: word(WORD_SOURCE_WIDTH),
            source_height: word(WORD_SOURCE_HEIGHT),
            premultiplied: false,
            rgba: buffer,
        };
        // The reply, if any, is taken by the caller; the job itself retires.
        let reply = job.reply.take();
        jobs.remove(&id);
        if let Some(reply) = reply {
            jobs.insert(
                id,
                Job {
                    mailbox: Box::new(std::array::from_fn(|_| AtomicI32::new(STATE_DONE))),
                    buffer: None,
                    reply: Some(reply),
                },
            );
        }
        Some(bitmap)
    }

    fn take_reply(&self, id: u32) -> Option<flume::Sender<Result<Bitmap, DecodeError>>> {
        self.jobs.borrow_mut().remove(&id).and_then(|job| job.reply)
    }

    fn fail(&self, id: u32, error: DecodeError) {
        if let Some(job) = self.jobs.borrow_mut().remove(&id)
            && let Some(reply) = job.reply
        {
            let _ = reply.send(Err(error));
        }
    }

    fn mailbox_view(&self, id: u32) -> Result<Int32Array, DecodeError> {
        let jobs = self.jobs.borrow();
        let job = jobs
            .get(&id)
            .ok_or_else(|| DecodeError::Unavailable("the decode job vanished".to_owned()))?;
        let address = address_word(job.mailbox.as_ptr().addr())?.cast_unsigned();
        Ok(Int32Array::new_with_byte_offset_and_length(
            &self.memory.buffer(),
            address,
            MAILBOX_WORDS,
        ))
    }
}

/// Blocks on the mailbox's state word until `done` holds for it.
fn wait_until(
    view: &Int32Array,
    deadline: web_time::Instant,
    done: impl Fn(i32) -> bool,
) -> Result<i32, DecodeError> {
    loop {
        let state = js_sys::Atomics::load(view, STATE_INDEX)
            .map_err(|error| DecodeError::Unavailable(describe(&error)))?;
        if done(state) {
            return Ok(state);
        }
        let remaining = deadline.saturating_duration_since(web_time::Instant::now());
        if remaining.is_zero() {
            return Err(DecodeError::Unavailable(
                "the image worker timed out".to_owned(),
            ));
        }
        js_sys::Atomics::wait_with_timeout(
            view,
            STATE_INDEX,
            state,
            remaining.as_secs_f64() * 1000.0,
        )
        .map_err(|error| DecodeError::Unavailable(describe(&error)))?;
    }
}

/// A Wasm address as the `i32` a mailbox word holds: the linear memory is
/// at most 4 GiB, so every address fits the bit pattern.
fn address_word(address: usize) -> Result<i32, DecodeError> {
    u32::try_from(address)
        .map(u32::cast_signed)
        .map_err(|_| DecodeError::Unavailable("an address does not fit a u32".to_owned()))
}

/// A job id from the JS number the echo carries.
fn job_id(id: f64) -> Option<u32> {
    if id.is_finite() && id >= 0.0 && id <= f64::from(u32::MAX) && id.fract() == 0.0 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked above"
        )]
        Some(id as u32)
    } else {
        None
    }
}

fn set(object: &Object, key: &str, value: &JsValue) {
    let _ = Reflect::set(object, &key.into(), value);
}

fn describe(error: &JsValue) -> String {
    error
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .or_else(|| error.as_string())
        .unwrap_or_else(|| format!("{error:?}"))
}
