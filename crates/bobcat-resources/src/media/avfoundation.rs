//! macOS: `AVFoundation`, reading a video with `AVAssetReader` (its video
//! track asked for BGRA, each frame handed over as CPU pixels) and timing it
//! with an `AVSampleBufferRenderSynchronizer`, whose clock an
//! `AVSampleBufferAudioRenderer` playing the audio track drives.
//!
//! # Why not `AVPlayer`
//!
//! An `AVPlayerItem` loads, and an `AVPlayer` advances, only while the
//! process's main run loop runs: on a thread of its own the item stays at
//! `AVPlayerItemStatusUnknown` and the player's time at zero, whatever that
//! thread does with its own run loop, and both move as soon as the main
//! run loop is run (verified on macOS 26). An embedder's main thread is its
//! own — a test harness, a server or a command-line host runs none — so
//! the player is built from the parts that run on any thread: a reader that
//! decodes on the calling thread, and a synchronizer and an audio renderer
//! that run on their own.
//!
//! # The clock
//!
//! The synchronizer owns it. A video with an audio track has an audio
//! renderer added to it, so the synchronizer's time follows the audio
//! device, and audio is the master; a video without one runs on the
//! synchronizer's own clock. Each video frame is handed over when the
//! synchronizer's time reaches its presentation time; a frame whose time
//! passed before it was shown is replaced by the newest one due, so frames
//! are dropped rather than the video falling behind its audio.
//!
//! # The thread
//!
//! Each open video is driven by a thread of its own (`bobcat-video`), which
//! every message to it is sent from. It waits for a command from the
//! [`AvPlayer`] handle, at most until the next frame is due, at most
//! [`PLAYING_POLL`] while playing and [`PAUSED_POLL`] while paused, and then
//! hands over the frame due, keeps the audio renderer fed, and notices the
//! end: [`MediaEvent::Ended`], or a new reader from the start when it
//! loops. The first frame is handed over and reported
//! ([`MediaEvent::FirstFrame`], the frame's size and the asset's duration)
//! as soon as the video opens, paused; later frames are converted only when
//! the sink wants them, except the one the video stops on (a pause, the end,
//! a failure), which is handed over wanted or not, so a picture drawn again
//! after its video stopped shows where it stopped.
//!
//! Bytes ([`MediaSource::Bytes`]) are written to a file in the temporary
//! directory named for their type, which `AVFoundation` reads by its
//! extension, and the file is removed when the thread ends. The player reads
//! `file` URLs itself and nothing else ([`AvMedia::reads`]): an `http` video
//! is fetched by the resource system first, since an asset reader is not
//! meant for a network source.
//!
//! Dropping the [`AvPlayer`] tells the thread to stop; it lets go of every
//! object as it leaves, without the dropping thread waiting for it.
//!
//! Messages go through `objc2`'s `msg_send!`; the C entry points of
//! `CoreFoundation`, `CoreMedia` and `CoreVideo` are declared here, as `ImageIO`'s
//! are in `decode/imageio.rs`.

#![expect(
    unsafe_code,
    reason = "AVFoundation is reached only through Objective-C messages and FFI; every call \
              site states the invariant it relies on"
)]

use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use bobcat_core::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use bobcat_core::{ImageDescriptor, PictureFrame};
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use vant::media::{
    Media, MediaCommand, MediaError, MediaErrorKind, MediaEvent, MediaPlayer, MediaSettings,
    MediaSink, MediaSource,
};

/// The longest a playing video's thread waits before it looks at its clock
/// again, when no frame is due sooner: twice per frame of a 60 Hz video.
pub const PLAYING_POLL: Duration = Duration::from_millis(8);
/// How long a paused video's thread waits for a command.
pub const PAUSED_POLL: Duration = Duration::from_millis(100);

/// `CMTime`, by value, as `CoreMedia` lays it out.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

// SAFETY: `CMTime` is the C struct `{?=qiIq}`, an anonymous struct typedef,
// which is how the Objective-C runtime encodes it in every `AVFoundation`
// method signature.
unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[
            Encoding::LongLong,
            Encoding::Int,
            Encoding::UInt,
            Encoding::LongLong,
        ],
    );
}

// SAFETY: a pointer to a `CMTime` is `^{?=qiIq}`.
unsafe impl RefEncode for CMTime {
    const ENCODING_REF: Encoding = Encoding::Pointer(&<Self as Encode>::ENCODING);
}

/// A `CMSampleBufferRef` as a message answers it: `^{opaqueCMSampleBuffer=}`.
#[repr(transparent)]
#[derive(Debug)]
struct SampleBuffer(*mut c_void);

// SAFETY: a `CMSampleBufferRef` is a pointer to the opaque `struct
// opaqueCMSampleBuffer`, which the runtime encodes as
// `^{opaqueCMSampleBuffer=}`.
unsafe impl Encode for SampleBuffer {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("opaqueCMSampleBuffer", &[]));
}

/// A `CMSampleBufferRef` borrowed for a message's argument.
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
struct SampleBufferRef(*mut c_void);

// SAFETY: as for [`SampleBuffer`].
unsafe impl Encode for SampleBufferRef {
    const ENCODING: Encoding = <SampleBuffer as Encode>::ENCODING;
}

impl SampleBuffer {
    /// The buffer, for a message that does not take it over.
    const fn borrowed(&self) -> SampleBufferRef {
        SampleBufferRef(self.0)
    }

    /// Its presentation time, in seconds.
    fn time(&self) -> f64 {
        // SAFETY: a live sample buffer this value owns.
        unsafe { CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(self.0)) }
    }
}

impl Drop for SampleBuffer {
    fn drop(&mut self) {
        // SAFETY: `copyNextSampleBuffer` answered it at +1; this is its one
        // release.
        unsafe { CFRelease(self.0) };
    }
}

/// `kCMTimeFlags_Valid`.
const TIME_VALID: u32 = 1;
/// `kCVPixelFormatType_32BGRA`, `'BGRA'`.
const PIXEL_FORMAT_BGRA: u32 = u32::from_be_bytes(*b"BGRA");
/// `kAudioFormatLinearPCM`, `'lpcm'`.
const AUDIO_FORMAT_LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
/// `kCVPixelBufferLock_ReadOnly`.
const LOCK_READ_ONLY: u64 = 1;
/// `AVAssetReaderStatusFailed`.
const READER_FAILED: isize = 3;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    static kCMTimeZero: CMTime;
    fn CMTimeGetSeconds(time: CMTime) -> f64;
    fn CMSampleBufferGetPresentationTimeStamp(buffer: *mut c_void) -> CMTime;
    fn CMSampleBufferGetImageBuffer(buffer: *mut c_void) -> *mut c_void;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: *const c_void;
    fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetWidth(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetPixelFormatType(buffer: *mut c_void) -> u32;
}

#[link(name = "AVFAudio", kind = "framework")]
unsafe extern "C" {
    static AVFormatIDKey: *const c_void;
}

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVMediaTypeVideo: *const c_void;
    static AVMediaTypeAudio: *const c_void;
}

/// The macOS [`Media`]: `AVFoundation`, one thread per open video.
#[derive(Clone, Copy, Debug, Default)]
pub struct AvMedia;

impl<F: 'static> Media<F> for AvMedia {
    type Player = AvPlayer;

    /// A `file` URL; anything else is handed over as its bytes.
    fn reads(&self, url: &url::Url) -> bool {
        url.scheme() == "file"
    }

    fn open<S: MediaSink<F>>(
        &self,
        source: MediaSource,
        settings: MediaSettings,
        sink: S,
    ) -> Result<AvPlayer, MediaError> {
        let (path, file) = match source {
            MediaSource::Url(url) => {
                let path = url.to_file_path().map_err(|()| {
                    MediaError::new(
                        MediaErrorKind::Source,
                        format!("`{url}` is not a file AVFoundation can read"),
                    )
                })?;
                (path, None)
            }
            MediaSource::Bytes {
                url,
                bytes,
                media_type,
            } => {
                let path = temporary_file(media_type.as_deref(), &url);
                std::fs::write(&path, &bytes).map_err(|error| {
                    MediaError::new(
                        MediaErrorKind::Source,
                        format!("`{url}` could not be handed to AVFoundation: {error}"),
                    )
                })?;
                (path.clone(), Some(path))
            }
        };
        let (commands, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("bobcat-video".to_owned())
            .spawn(move || {
                let _file = file.map(TemporaryFile);
                run(&path, settings, &receiver, &sink);
            })
            .map_err(|error| {
                MediaError::new(
                    MediaErrorKind::Playback,
                    format!("the video thread could not start: {error}"),
                )
            })?;
        Ok(AvPlayer { commands })
    }
}

/// An open video: what the store's commands reach. Dropping it stops its
/// thread.
#[derive(Debug)]
pub struct AvPlayer {
    commands: mpsc::Sender<Command>,
}

#[derive(Debug)]
enum Command {
    Media(MediaCommand),
    Close,
}

impl MediaPlayer for AvPlayer {
    fn play(&self) {
        let _ = self.commands.send(Command::Media(MediaCommand::Play));
    }

    fn pause(&self) {
        let _ = self.commands.send(Command::Media(MediaCommand::Pause));
    }

    fn set_loop(&self, looping: bool) {
        let _ = self
            .commands
            .send(Command::Media(MediaCommand::Loop(looping)));
    }

    fn set_muted(&self, muted: bool) {
        let _ = self
            .commands
            .send(Command::Media(MediaCommand::Muted(muted)));
    }
}

impl Drop for AvPlayer {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Close);
    }
}

/// A file this player wrote, removed when its thread ends.
struct TemporaryFile(std::path::PathBuf);

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A fresh path in the temporary directory with the extension `AVFoundation`
/// reads `media_type`, or else `url`'s path, as.
fn temporary_file(media_type: Option<&str>, url: &url::Url) -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let essence = media_type.map(|media_type| {
        media_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
    });
    let extension = match essence.as_deref() {
        Some("video/quicktime") => "mov".to_owned(),
        Some("video/x-m4v") => "m4v".to_owned(),
        Some(essence) if essence.starts_with("video/") || essence.starts_with("audio/") => {
            "mp4".to_owned()
        }
        _ => std::path::Path::new(url.path())
            .extension()
            .and_then(|extension| extension.to_str())
            .filter(|extension| extension.chars().all(|c| c.is_ascii_alphanumeric()))
            .map_or_else(|| "mp4".to_owned(), str::to_ascii_lowercase),
    };
    std::env::temp_dir().join(format!(
        "bobcat-video-{}-{}.{extension}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// An `NSString` of `text`.
fn ns_string(text: &str) -> Option<Retained<AnyObject>> {
    let text = std::ffi::CString::new(text).ok()?;
    // SAFETY: `stringWithUTF8String:` copies the NUL-terminated string,
    // which lives for the call.
    unsafe { msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()] }
}

/// An `NSString`'s text.
fn text_of(string: &AnyObject) -> String {
    // SAFETY: `UTF8String` answers a NUL-terminated string the object owns,
    // read before the object is let go of; it may be null.
    let utf8: *const c_char = unsafe { msg_send![string, UTF8String] };
    if utf8.is_null() {
        return String::new();
    }
    // SAFETY: non-null, NUL-terminated, alive for the borrow of `string`.
    unsafe { CStr::from_ptr(utf8) }
        .to_string_lossy()
        .into_owned()
}

/// An `NSError`'s description, or `fallback` without one.
fn describe_error(error: Option<Retained<AnyObject>>, fallback: &str) -> String {
    error
        .and_then(|error| {
            // SAFETY: `localizedDescription` answers an `NSString`.
            let text: Option<Retained<AnyObject>> =
                unsafe { msg_send![&*error, localizedDescription] };
            text
        })
        .map_or_else(|| fallback.to_owned(), |text| text_of(&text))
}

/// A one-entry `NSDictionary` of `key` to the unsigned number `value`.
///
/// # Safety
///
/// `key` must be a `CFStringRef` or `NSString` alive for the process.
unsafe fn settings(key: *const c_void, value: u32) -> Option<Retained<AnyObject>> {
    // SAFETY: `numberWithUnsignedInt:` answers an autoreleased `NSNumber`;
    // `dictionaryWithObject:forKey:` an autoreleased `NSDictionary`. The key
    // is the caller's, toll-free bridged to `NSString`.
    unsafe {
        let number: Option<Retained<AnyObject>> =
            msg_send![class!(NSNumber), numberWithUnsignedInt: value];
        let number = number?;
        let key = &*key.cast::<AnyObject>();
        msg_send![class!(NSDictionary), dictionaryWithObject: &*number, forKey: key]
    }
}

/// The first track of `asset` of `kind` (`AVMediaTypeVideo`,
/// `AVMediaTypeAudio`).
fn first_track(asset: &AnyObject, kind: *const c_void) -> Option<Retained<AnyObject>> {
    // SAFETY: `tracksWithMediaType:` takes an `AVMediaType` (an `NSString`
    // constant alive for the process) and answers an `NSArray`, loading the
    // tracks synchronously on this thread; `firstObject` answers its first
    // element or nil.
    unsafe {
        let kind = &*kind.cast::<AnyObject>();
        let tracks: Option<Retained<AnyObject>> = msg_send![asset, tracksWithMediaType: kind];
        msg_send![&*tracks?, firstObject]
    }
}

/// The asset and its tracks: what each new reader is made from.
struct Asset {
    handle: Retained<AnyObject>,
    video: Retained<AnyObject>,
    audio: Option<Retained<AnyObject>>,
    duration: f64,
}

impl Asset {
    fn open(path: &std::path::Path) -> Result<Self, MediaError> {
        let failed = |message: &str| MediaError::new(MediaErrorKind::Playback, message.to_owned());
        let path = ns_string(&path.to_string_lossy())
            .ok_or_else(|| failed("the video's path is not a string AVFoundation reads"))?;
        // SAFETY: `fileURLWithPath:` takes an `NSString` and answers an
        // autoreleased `NSURL`; `URLAssetWithURL:options:` an `NSURL` and a
        // nullable `NSDictionary`, answering an autoreleased `AVURLAsset`;
        // `duration` a `CMTime`.
        let (asset, duration) = unsafe {
            let url: Option<Retained<AnyObject>> =
                msg_send![class!(NSURL), fileURLWithPath: &*path];
            let url = url.ok_or_else(|| failed("the video's path is not a URL"))?;
            let asset: Option<Retained<AnyObject>> = msg_send![
                class!(AVURLAsset),
                URLAssetWithURL: &*url,
                options: std::ptr::null::<AnyObject>()
            ];
            let asset = asset.ok_or_else(|| failed("AVFoundation made no asset of the video"))?;
            let duration: CMTime = msg_send![&*asset, duration];
            (asset, duration)
        };
        // SAFETY: the static is `CoreMedia`'s constant.
        let video = first_track(&asset, unsafe { AVMediaTypeVideo })
            .ok_or_else(|| failed("AVFoundation found no video track to play"))?;
        // SAFETY: as above.
        let audio = first_track(&asset, unsafe { AVMediaTypeAudio });
        let duration = if duration.flags & TIME_VALID == 0 {
            0.0
        } else {
            // SAFETY: a `CMTime` by value.
            unsafe { CMTimeGetSeconds(duration) }
        };
        Ok(Self {
            handle: asset,
            video,
            audio,
            duration: if duration.is_finite() { duration } else { 0.0 },
        })
    }
}

/// One pass of the asset from its start: a reader with a BGRA video output
/// and, when the asset has audio, a linear PCM audio output.
struct Reader {
    handle: Retained<AnyObject>,
    video: Retained<AnyObject>,
    audio: Option<Retained<AnyObject>>,
}

impl Reader {
    fn start(asset: &Asset) -> Result<Self, MediaError> {
        let failed = |message: String| MediaError::new(MediaErrorKind::Playback, message);
        let mut error: *mut AnyObject = std::ptr::null_mut();
        // SAFETY: `assetReaderWithAsset:error:` takes an `AVAsset` and an
        // `NSError **`, answering an autoreleased `AVAssetReader` or nil
        // with the error written, autoreleased.
        let reader: Option<Retained<AnyObject>> = unsafe {
            msg_send![class!(AVAssetReader), assetReaderWithAsset: &*asset.handle, error: &mut error]
        };
        let Some(reader) = reader else {
            // SAFETY: an autoreleased `NSError` or null, retained here.
            let error = unsafe { Retained::retain(error) };
            return Err(failed(describe_error(error, "no reader for the video")));
        };
        // SAFETY: the settings' keys are `CoreVideo`'s and `AVFAudio`'s
        // constants; `assetReaderTrackOutputWithTrack:outputSettings:` takes
        // an `AVAssetTrack` and an `NSDictionary`, answering an autoreleased
        // output; `canAddOutput:` and `addOutput:` take it; `startReading`
        // answers a `BOOL`.
        unsafe {
            let video_settings = settings(kCVPixelBufferPixelFormatTypeKey, PIXEL_FORMAT_BGRA)
                .ok_or_else(|| failed("no video output settings".to_owned()))?;
            let video: Option<Retained<AnyObject>> = msg_send![
                class!(AVAssetReaderTrackOutput),
                assetReaderTrackOutputWithTrack: &*asset.video,
                outputSettings: &*video_settings
            ];
            let video = video.ok_or_else(|| failed("no video output".to_owned()))?;
            let _: () = msg_send![&*reader, addOutput: &*video];
            let audio = match &asset.audio {
                Some(track) => {
                    let audio_settings = settings(AVFormatIDKey, AUDIO_FORMAT_LINEAR_PCM)
                        .ok_or_else(|| failed("no audio output settings".to_owned()))?;
                    let audio: Option<Retained<AnyObject>> = msg_send![
                        class!(AVAssetReaderTrackOutput),
                        assetReaderTrackOutputWithTrack: &**track,
                        outputSettings: &*audio_settings
                    ];
                    let addable: Bool = match &audio {
                        Some(audio) => msg_send![&*reader, canAddOutput: &**audio],
                        None => Bool::NO,
                    };
                    if addable.as_bool() {
                        let audio = audio.expect("checked above");
                        let _: () = msg_send![&*reader, addOutput: &*audio];
                        Some(audio)
                    } else {
                        None
                    }
                }
                None => None,
            };
            let started: Bool = msg_send![&*reader, startReading];
            if !started.as_bool() {
                let error: Option<Retained<AnyObject>> = msg_send![&*reader, error];
                return Err(failed(describe_error(error, "the video could not be read")));
            }
            Ok(Self {
                handle: reader,
                video,
                audio,
            })
        }
    }

    /// The next decoded sample of `output`, or `None` at its end.
    fn next(output: &AnyObject) -> Option<SampleBuffer> {
        // SAFETY: `copyNextSampleBuffer` answers a +1 `CMSampleBufferRef`
        // or null at the end of the output, decoding on this thread.
        let buffer: SampleBuffer = unsafe { msg_send![output, copyNextSampleBuffer] };
        if buffer.0.is_null() {
            std::mem::forget(buffer);
            return None;
        }
        Some(buffer)
    }

    fn next_video(&self) -> Option<SampleBuffer> {
        Self::next(&self.video)
    }

    /// Why the reader stopped, when it failed rather than ended.
    fn failure(&self) -> Option<String> {
        // SAFETY: `status` answers an `NSInteger`, `error` an `NSError` or
        // nil.
        unsafe {
            let status: isize = msg_send![&*self.handle, status];
            (status == READER_FAILED).then(|| {
                let error: Option<Retained<AnyObject>> = msg_send![&*self.handle, error];
                describe_error(error, "the video could not be decoded")
            })
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: `cancelReading` takes nothing; a reader that finished
        // ignores it.
        unsafe {
            let _: () = msg_send![&*self.handle, cancelReading];
        }
    }
}

/// The clock, and the audio renderer it follows when there is one.
struct Clock {
    synchronizer: Retained<AnyObject>,
    audio: Option<Retained<AnyObject>>,
}

impl Clock {
    fn new(audio: bool, muted: bool) -> Result<Self, MediaError> {
        // SAFETY: `new` answers a +1 object; `addRenderer:` takes an
        // `AVQueuedSampleBufferRendering`.
        unsafe {
            let synchronizer: Option<Retained<AnyObject>> =
                msg_send![class!(AVSampleBufferRenderSynchronizer), new];
            let synchronizer = synchronizer.ok_or_else(|| {
                MediaError::new(MediaErrorKind::Playback, "AVFoundation made no clock")
            })?;
            let audio = if audio {
                let renderer: Option<Retained<AnyObject>> =
                    msg_send![class!(AVSampleBufferAudioRenderer), new];
                if let Some(renderer) = &renderer {
                    let _: () = msg_send![&*synchronizer, addRenderer: &**renderer];
                }
                renderer
            } else {
                None
            };
            let clock = Self {
                synchronizer,
                audio,
            };
            clock.set_muted(muted);
            Ok(clock)
        }
    }

    /// The clock's time, in seconds.
    fn now(&self) -> f64 {
        // SAFETY: `currentTime` answers a `CMTime`.
        unsafe {
            let time: CMTime = msg_send![&*self.synchronizer, currentTime];
            CMTimeGetSeconds(time)
        }
    }

    /// Runs the clock at `rate` from where it is.
    fn set_rate(&self, rate: f32) {
        // SAFETY: `setRate:` takes a `float`.
        unsafe {
            let _: () = msg_send![&*self.synchronizer, setRate: rate];
        }
    }

    /// Stops the clock at zero and drops the audio queued for the pass
    /// before.
    fn rewind(&self) {
        // SAFETY: `setRate:time:` takes a `float` and a `CMTime`, `flush`
        // nothing.
        unsafe {
            let _: () = msg_send![&*self.synchronizer, setRate: 0.0_f32, time: kCMTimeZero];
            if let Some(audio) = &self.audio {
                let _: () = msg_send![&**audio, flush];
            }
        }
    }

    fn set_muted(&self, muted: bool) {
        if let Some(audio) = &self.audio {
            // SAFETY: `setMuted:` takes a `BOOL`.
            unsafe {
                let _: () = msg_send![&**audio, setMuted: Bool::new(muted)];
            }
        }
    }

    /// Queues the reader's audio while the renderer takes more. Answers
    /// whether the audio has ended.
    fn feed(&self, reader: &Reader) -> bool {
        let (Some(renderer), Some(output)) = (&self.audio, &reader.audio) else {
            return true;
        };
        loop {
            // SAFETY: `isReadyForMoreMediaData` answers a `BOOL`, and
            // `enqueueSampleBuffer:` takes a `CMSampleBufferRef`, which the
            // renderer retains.
            unsafe {
                let ready: Bool = msg_send![&**renderer, isReadyForMoreMediaData];
                if !ready.as_bool() {
                    return false;
                }
                let Some(buffer) = Reader::next(output) else {
                    return true;
                };
                let _: () = msg_send![&**renderer, enqueueSampleBuffer: buffer.borrowed()];
            }
        }
    }
}

/// `buffer`'s image as premultiplied RGBA. A video frame is opaque, so its
/// BGRA is premultiplied as it stands, and only the channels move.
fn pixels(buffer: &SampleBuffer) -> Option<ImageData> {
    // SAFETY: the image buffer is the sample's, alive while `buffer` is. Its
    // base address is read only between the lock and the unlock, `height`
    // rows of `stride` bytes each, of which the first `width * 4` are BGRA
    // texels, as the output was asked for (checked by the format).
    unsafe {
        let image = CMSampleBufferGetImageBuffer(buffer.0);
        if image.is_null()
            || CVPixelBufferGetPixelFormatType(image) != PIXEL_FORMAT_BGRA
            || CVPixelBufferLockBaseAddress(image, LOCK_READ_ONLY) != 0
        {
            return None;
        }
        let width = CVPixelBufferGetWidth(image);
        let height = CVPixelBufferGetHeight(image);
        let stride = CVPixelBufferGetBytesPerRow(image);
        let base = CVPixelBufferGetBaseAddress(image).cast::<u8>();
        let mut converted = None;
        if !base.is_null() && width > 0 && height > 0 && stride >= width * 4 {
            let mut rgba = vec![0_u8; width * height * 4];
            for row in 0..height {
                let source = std::slice::from_raw_parts(base.add(row * stride), width * 4);
                let target = &mut rgba[row * width * 4..(row + 1) * width * 4];
                for (texel, bgra) in target
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(source.as_chunks::<4>().0)
                {
                    *texel = [bgra[2], bgra[1], bgra[0], bgra[3]];
                }
            }
            converted = u32::try_from(width)
                .ok()
                .zip(u32::try_from(height).ok())
                .map(|(width, height)| ImageData {
                    data: Blob::new(Arc::new(rgba)),
                    format: ImageFormat::Rgba8,
                    alpha_type: ImageAlphaType::AlphaPremultiplied,
                    width,
                    height,
                });
        }
        CVPixelBufferUnlockBaseAddress(image, LOCK_READ_ONLY);
        converted
    }
}

/// Hands `buffer`'s image to `sink`, answering its size.
fn hand_over<F, S: MediaSink<F>>(sink: &S, buffer: &SampleBuffer) -> Option<(u32, u32)> {
    let pixels = pixels(buffer)?;
    let size = (pixels.width, pixels.height);
    let descriptor = ImageDescriptor::of(&pixels);
    sink.frame(PictureFrame::Pixels(pixels), descriptor);
    Some(size)
}

/// The video's whole life on its thread: open it, then serve commands and
/// follow the clock until the handle is dropped.
fn run<F, S: MediaSink<F>>(
    path: &std::path::Path,
    settings: MediaSettings,
    commands: &mpsc::Receiver<Command>,
    sink: &S,
) {
    let Some(mut video) = autoreleasepool(|_| match Video::open(path, settings, sink) {
        Ok(video) => Some(video),
        Err(error) => {
            sink.event(MediaEvent::Failed(error));
            None
        }
    }) else {
        return;
    };
    loop {
        let wait = video.wait();
        match commands.recv_timeout(wait) {
            Ok(Command::Close) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Command::Media(command)) => autoreleasepool(|_| video.apply(command, sink)),
            Err(RecvTimeoutError::Timeout) => {}
        }
        autoreleasepool(|_| video.advance(sink));
    }
    autoreleasepool(|_| drop(video));
}

/// Where a video's playback is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Paused,
    Playing,
    /// It played to its end and stopped there; a `play` starts it over.
    Ended,
    /// It stopped for good on a failure, which was reported.
    Failed,
}

/// One open video on its thread.
struct Video {
    asset: Asset,
    reader: Reader,
    clock: Clock,
    /// The next frame, not shown yet.
    next: Option<SampleBuffer>,
    /// The newest frame that came due while the sink wanted none, so it was
    /// not converted. Handed over when the video stops on it (a pause, the
    /// end, a failure): the sink keeps it for the draw that comes.
    skipped: Option<SampleBuffer>,
    /// The audio output has nothing more for this pass.
    audio_done: bool,
    looping: bool,
    phase: Phase,
}

impl Video {
    /// Opens the video paused, and hands over and reports its first frame.
    fn open<F, S: MediaSink<F>>(
        path: &std::path::Path,
        settings: MediaSettings,
        sink: &S,
    ) -> Result<Self, MediaError> {
        let asset = Asset::open(path)?;
        let reader = Reader::start(&asset)?;
        let clock = Clock::new(reader.audio.is_some(), settings.muted)?;
        let first = reader.next_video().ok_or_else(|| {
            MediaError::new(
                MediaErrorKind::Playback,
                reader
                    .failure()
                    .unwrap_or_else(|| "the video has no frame".to_owned()),
            )
        })?;
        let (width, height) = hand_over(sink, &first).ok_or_else(|| {
            MediaError::new(
                MediaErrorKind::Playback,
                "the video's first frame could not be read",
            )
        })?;
        sink.event(MediaEvent::FirstFrame {
            width,
            height,
            duration: Duration::try_from_secs_f64(asset.duration).unwrap_or(Duration::ZERO),
        });
        let next = reader.next_video();
        Ok(Self {
            asset,
            reader,
            clock,
            next,
            skipped: None,
            audio_done: false,
            looping: settings.looping,
            phase: Phase::Paused,
        })
    }

    /// How long to wait for a command before looking at the clock again.
    fn wait(&self) -> Duration {
        if self.phase != Phase::Playing {
            return PAUSED_POLL;
        }
        // The next frame's time, or the end once every frame is shown.
        let due = self
            .next
            .as_ref()
            .map_or(self.asset.duration, SampleBuffer::time)
            - self.clock.now();
        Duration::try_from_secs_f64(due)
            .unwrap_or(Duration::ZERO)
            .clamp(Duration::from_millis(1), PLAYING_POLL)
    }

    fn apply<F, S: MediaSink<F>>(&mut self, command: MediaCommand, sink: &S) {
        match command {
            MediaCommand::Play => match self.phase {
                Phase::Failed | Phase::Playing => {}
                Phase::Ended | Phase::Paused => {
                    if self.phase == Phase::Ended {
                        self.restart(sink);
                    }
                    if self.phase != Phase::Failed {
                        self.phase = Phase::Playing;
                        self.clock.set_rate(1.0);
                    }
                }
            },
            MediaCommand::Pause => {
                if self.phase == Phase::Playing {
                    self.phase = Phase::Paused;
                    self.settle(sink);
                }
                self.clock.set_rate(0.0);
            }
            MediaCommand::Loop(looping) => self.looping = looping,
            MediaCommand::Muted(muted) => self.clock.set_muted(muted),
        }
    }

    /// Shows `frame`, which came due: handed over when the sink wants
    /// frames, kept as the one skipped otherwise.
    fn show<F, S: MediaSink<F>>(&mut self, frame: SampleBuffer, sink: &S) {
        if sink.wants_frames() {
            let _ = hand_over(sink, &frame);
            self.skipped = None;
        } else {
            self.skipped = Some(frame);
        }
    }

    /// The video stops on the frame it shows: one that was skipped is
    /// handed over now, wanted or not, so the sink's newest frame is the
    /// one a later draw should show.
    fn settle<F, S: MediaSink<F>>(&mut self, sink: &S) {
        if let Some(frame) = self.skipped.take() {
            let _ = hand_over(sink, &frame);
        }
    }

    /// A new pass from the start, the clock stopped at zero and the first
    /// frame shown.
    fn restart<F, S: MediaSink<F>>(&mut self, sink: &S) {
        self.next = None;
        self.clock.rewind();
        match Reader::start(&self.asset) {
            Ok(reader) => {
                self.reader = reader;
                self.audio_done = false;
                if let Some(first) = self.reader.next_video() {
                    self.show(first, sink);
                    self.next = self.reader.next_video();
                }
            }
            Err(error) => self.fail(error, sink),
        }
    }

    fn fail<F, S: MediaSink<F>>(&mut self, error: MediaError, sink: &S) {
        self.phase = Phase::Failed;
        self.clock.set_rate(0.0);
        self.settle(sink);
        sink.event(MediaEvent::Failed(error));
    }

    /// Hands over the newest frame the clock has reached, keeps the audio
    /// fed, and ends or loops at the end.
    fn advance<F, S: MediaSink<F>>(&mut self, sink: &S) {
        if self.phase != Phase::Playing {
            return;
        }
        if !self.audio_done {
            self.audio_done = self.clock.feed(&self.reader);
        }
        let now = self.clock.now();
        let mut due = None;
        while let Some(next) = self.next.take() {
            if next.time() > now {
                self.next = Some(next);
                break;
            }
            due = Some(next);
            self.next = self.reader.next_video();
        }
        if let Some(due) = due {
            self.show(due, sink);
        }
        if self.next.is_some() {
            return;
        }
        if let Some(message) = self.reader.failure() {
            self.fail(MediaError::new(MediaErrorKind::Playback, message), sink);
            return;
        }
        if now < self.asset.duration {
            return;
        }
        if self.looping {
            self.restart(sink);
            if self.phase == Phase::Playing {
                self.clock.set_rate(1.0);
            }
        } else {
            self.phase = Phase::Ended;
            self.clock.set_rate(0.0);
            self.settle(sink);
            sink.event(MediaEvent::Ended);
        }
    }
}

#[cfg(test)]
#[path = "avfoundation_tests.rs"]
mod tests;
