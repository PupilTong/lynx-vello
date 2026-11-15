//! Browser: the main thread's `Image` element, reached over a `MessagePort`,
//! with a mailbox in shared memory the decoded pixels are written through.
//!
//! The browser's decoder is `HTMLImageElement`, which only the main thread
//! has, and it is asynchronous, as every decode in the engine is: a load's
//! first decode, a refinement and a wanted source's restore all run in a
//! local task on the Render Worker and wait on its event loop. So the Render
//! Worker fetches the bytes and hands them to the main thread, which decodes
//! them through a Blob URL, and every job has a mailbox in Wasm memory — a
//! few `i32` words the two sides read with `Atomics`. The main thread writes
//! the decoded size and echoes a message; this side allocates the pixel
//! buffer in its own heap and posts where it is; the main thread copies the
//! pixels straight into that buffer and echoes once more. Neither side ever
//! waits on the mailbox. Its other half is `js/image-decoder.ts`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicI32, Ordering, fence};

use js_sys::{Array, Object, Reflect, Uint8Array};
use rustc_hash::FxHashMap;
use vant::image::{Bitmap, DecodeError};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{MessageEvent, MessagePort};

const STATE_DIMENSIONS: i32 = 1;
const STATE_BUFFER: i32 = 2;
const STATE_DONE: i32 = 3;

const WORD_STATE: usize = 0;
const WORD_WIDTH: usize = 1;
const WORD_HEIGHT: usize = 2;
const WORD_SOURCE_WIDTH: usize = 3;
const WORD_SOURCE_HEIGHT: usize = 4;
const WORD_POINTER: usize = 5;
const WORD_LENGTH: usize = 6;
const WORDS: usize = 8;

type Mailbox = Box<[AtomicI32; WORDS]>;

struct Job {
    mailbox: Mailbox,
    /// The pixel buffer, allocated once the size is known; the main thread
    /// writes into it directly.
    buffer: Option<Vec<u8>>,
    /// Where the decode's result goes.
    reply: tokio::sync::oneshot::Sender<Result<Bitmap, DecodeError>>,
}

/// The main thread's decoder and the jobs in flight on it.
pub(crate) struct ImageDecoder {
    port: MessagePort,
    jobs: RefCell<FxHashMap<u32, Job>>,
    next_id: Cell<u32>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

impl std::fmt::Debug for ImageDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImageDecoder")
            .field("jobs", &self.jobs.borrow().len())
            .finish_non_exhaustive()
    }
}

impl Drop for ImageDecoder {
    fn drop(&mut self) {
        self.port.close();
    }
}

impl ImageDecoder {
    /// Takes this Worker's end of the channel whose other end the main
    /// thread's decoder listens on, and hands it this instance's memory.
    pub(crate) fn new(port: MessagePort) -> Result<Rc<Self>, String> {
        let memory = wasm_bindgen::memory().unchecked_into::<js_sys::WebAssembly::Memory>();
        let init = Object::new();
        set(&init, "type", &"init".into());
        set(&init, "memory", &memory);
        port.post_message(&init).map_err(|error| describe(&error))?;
        Ok(Rc::new_cyclic(|weak: &Weak<Self>| {
            let weak = weak.clone();
            let on_message = Closure::new(move |event: MessageEvent| {
                if let Some(decoder) = weak.upgrade() {
                    decoder.on_message(&event.data());
                }
            });
            // Assigning `onmessage` is also what starts the port.
            port.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            Self {
                port,
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
        media_type: &str,
        max: (u32, u32),
    ) -> Result<Bitmap, DecodeError> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.start(bytes, media_type, max, sender)?;
        receiver.await.unwrap_or_else(|_| {
            Err(DecodeError::Unavailable(
                "the image decoder went away before answering".to_owned(),
            ))
        })
    }

    /// Posts a job: the bytes, moved rather than copied, and the size to
    /// stay within. The main thread sizes its resize from the image it
    /// decoded rather than from any header, which cannot see an EXIF
    /// orientation.
    fn start(
        &self,
        bytes: &[u8],
        media_type: &str,
        max: (u32, u32),
        reply: tokio::sync::oneshot::Sender<Result<Bitmap, DecodeError>>,
    ) -> Result<(), DecodeError> {
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
        set(&message, "mediaType", &media_type.into());
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
        if let Err(error) = self
            .port
            .post_message_with_transferable(&message, &transfer)
        {
            self.jobs.borrow_mut().remove(&id);
            return Err(DecodeError::Unavailable(describe(&error)));
        }
        Ok(())
    }

    /// The main thread's echoes: `dims`, `done`, `error`.
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
                if state == Some(STATE_DIMENSIONS)
                    && let Err(error) = self.provide_buffer(id)
                {
                    self.fail(id, error);
                }
            }
            "done" => self.finish(id),
            "error" => {
                let message = Reflect::get(data, &"message".into())
                    .ok()
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| "unknown image decoder failure".to_owned());
                self.fail(id, DecodeError::Malformed(message));
            }
            _ => {}
        }
    }

    /// Allocates the pixel buffer the mailbox's size asks for and tells the
    /// main thread where it is.
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
        self.port
            .post_message(&message)
            .map_err(|error| DecodeError::Unavailable(describe(&error)))
    }

    /// Answers a finished job with its pixels, as a bitmap.
    fn finish(&self, id: u32) {
        let Some(mut job) = self.jobs.borrow_mut().remove(&id) else {
            return;
        };
        let outcome = if job.mailbox[WORD_STATE].load(Ordering::SeqCst) == STATE_DONE
            && let Some(buffer) = job.buffer.take()
        {
            // The main thread wrote the buffer before it stored `DONE`; the
            // acquire above orders those writes before the reads below.
            fence(Ordering::SeqCst);
            let word = |index: usize| {
                u32::try_from(job.mailbox[index].load(Ordering::SeqCst)).unwrap_or(0)
            };
            Ok(Bitmap {
                width: word(WORD_WIDTH),
                height: word(WORD_HEIGHT),
                source_width: word(WORD_SOURCE_WIDTH),
                source_height: word(WORD_SOURCE_HEIGHT),
                premultiplied: false,
                rgba: buffer,
            })
        } else {
            Err(DecodeError::Unavailable(
                "the main thread finished a job it had not been handed the buffer of".to_owned(),
            ))
        };
        let _ = job.reply.send(outcome);
    }

    fn fail(&self, id: u32, error: DecodeError) {
        if let Some(job) = self.jobs.borrow_mut().remove(&id) {
            let _ = job.reply.send(Err(error));
        }
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
