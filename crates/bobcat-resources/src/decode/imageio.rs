//! macOS: `ImageIO` and `CoreGraphics`, the decoder every Apple image view uses.
//!
//! `CGImageSourceCreateThumbnailAtIndex` with a maximum pixel size decodes a
//! downsampled image directly — for JPEG, at a reduced DCT scale — which is
//! what makes a large photo shown small cost its shown size. The result is
//! drawn once into an RGBA bitmap context, which is how `CoreGraphics` hands
//! pixels out; that context is premultiplied, and the bitmap says so.
//!
//! An animated GIF, APNG or WebP is decoded frame by frame through the same
//! call at the frame's index ([`decode_frame`]): `ImageIO` hands back the
//! whole canvas as it is shown at that frame, earlier frames' disposal and
//! blending applied. What the frames are ([`frames`]) is read from the
//! properties without decoding a pixel: the count, each frame's
//! `UnclampedDelayTime` (the delay the file states; `DelayTime` is `ImageIO`'s
//! own clamped reading, which this crate does not use, since the clamp is
//! `vant`'s), and the container's `LoopCount`, which `ImageIO` answers as
//! the number of plays with zero for forever for all three formats — for a
//! GIF it is the NETSCAPE repeat count plus one, and one without the
//! extension, which is how browsers read a GIF too.
//!
//! Reached through raw FFI declarations against the system frameworks
//! rather than a binding crate: the handful of C entry points used here
//! are stable, and declaring them costs less than a dependency.

#![expect(
    unsafe_code,
    reason = "system frameworks are reached only through FFI; every call site states the \
              invariant it relies on"
)]

use std::ffi::c_void;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use vant::image::animation::{ImageFrames, Loops};

use super::{Bitmap, DecodeError, target_size};
use crate::image_header::ImageHeader;

type CFTypeRef = *const c_void;
type CFIndex = isize;
type CGContextRef = *mut c_void;

#[repr(C)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[repr(C)]
struct CFDictionaryCallBacks {
    version: CFIndex,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
    equal: *const c_void,
    hash: *const c_void,
}

const K_CF_NUMBER_SINT32_TYPE: CFIndex = 3;
const K_CF_NUMBER_SINT64_TYPE: CFIndex = 4;
const K_CF_NUMBER_FLOAT64_TYPE: CFIndex = 6;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
/// `kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big`: bytes in
/// memory order R, G, B, A with premultiplied alpha.
const RGBA_PREMULTIPLIED: u32 = 1 | (4 << 12);

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: CFDictionaryCallBacks;
    static kCFTypeDictionaryValueCallBacks: CFDictionaryCallBacks;
    static kCFBooleanTrue: CFTypeRef;
    fn CFDataCreate(allocator: CFTypeRef, bytes: *const u8, length: CFIndex) -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        num_values: CFIndex,
        key_callbacks: *const CFDictionaryCallBacks,
        value_callbacks: *const CFDictionaryCallBacks,
    ) -> CFTypeRef;
    fn CFDictionaryGetValue(dictionary: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFNumberCreate(
        allocator: CFTypeRef,
        number_type: CFIndex,
        value: *const c_void,
    ) -> CFTypeRef;
    fn CFNumberGetValue(number: CFTypeRef, number_type: CFIndex, value: *mut c_void) -> u8;
    fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        string: *const std::ffi::c_char,
        encoding: u32,
    ) -> CFTypeRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGImageGetWidth(image: CFTypeRef) -> usize;
    fn CGImageGetHeight(image: CFTypeRef) -> usize;
    fn CGImageRelease(image: CFTypeRef);
    fn CGColorSpaceCreateDeviceRGB() -> CFTypeRef;
    fn CGColorSpaceRelease(space: CFTypeRef);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CFTypeRef,
        bitmap_info: u32,
    ) -> CGContextRef;
    fn CGContextDrawImage(context: CGContextRef, rect: CGRect, image: CFTypeRef);
    fn CGContextRelease(context: CGContextRef);
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    static kCGImageSourceThumbnailMaxPixelSize: CFTypeRef;
    static kCGImageSourceCreateThumbnailFromImageAlways: CFTypeRef;
    static kCGImageSourceCreateThumbnailWithTransform: CFTypeRef;
    static kCGImagePropertyPixelWidth: CFTypeRef;
    static kCGImagePropertyPixelHeight: CFTypeRef;
    static kCGImagePropertyOrientation: CFTypeRef;
    fn CGImageSourceCreateWithData(data: CFTypeRef, options: CFTypeRef) -> CFTypeRef;
    fn CGImageSourceGetCount(source: CFTypeRef) -> usize;
    fn CGImageSourceCopyProperties(source: CFTypeRef, options: CFTypeRef) -> CFTypeRef;
    fn CGImageSourceCopyPropertiesAtIndex(
        source: CFTypeRef,
        index: usize,
        options: CFTypeRef,
    ) -> CFTypeRef;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: CFTypeRef,
        index: usize,
        options: CFTypeRef,
    ) -> CFTypeRef;
}

/// A `CoreFoundation` object released on drop.
struct Owned(CFTypeRef);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this struct owns exactly one reference (a `Create` or
            // `Copy` result) to a live CF object.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// Reads a `CFNumber` property out of an `ImageIO` property dictionary.
fn number_property(properties: CFTypeRef, key: CFTypeRef) -> Option<i64> {
    // SAFETY: `properties` is a live dictionary; the value under an ImageIO
    // pixel-size or orientation key is a CFNumber, which
    // `CFNumberGetValue` converts to a 64-bit integer.
    unsafe {
        let value = CFDictionaryGetValue(properties, key);
        if value.is_null() {
            return None;
        }
        let mut number: i64 = 0;
        (CFNumberGetValue(
            value,
            K_CF_NUMBER_SINT64_TYPE,
            (&raw mut number).cast::<c_void>(),
        ) != 0)
            .then_some(number)
    }
}

/// Reads a `CFNumber` property out of an `ImageIO` property dictionary as a
/// double.
fn float_property(properties: CFTypeRef, key: CFTypeRef) -> Option<f64> {
    // SAFETY: `properties` is a live dictionary; a delay is a CFNumber,
    // which `CFNumberGetValue` converts to a double.
    unsafe {
        let value = CFDictionaryGetValue(properties, key);
        if value.is_null() {
            return None;
        }
        let mut number: f64 = 0.0;
        (CFNumberGetValue(
            value,
            K_CF_NUMBER_FLOAT64_TYPE,
            (&raw mut number).cast::<c_void>(),
        ) != 0)
            .then_some(number)
    }
}

/// A `CFString` of `text`, released on drop.
fn cf_string(text: &std::ffi::CStr) -> Owned {
    // SAFETY: `text` is NUL-terminated UTF-8, which the call copies.
    Owned(unsafe {
        CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), K_CF_STRING_ENCODING_UTF8)
    })
}

/// The per-format dictionaries an animated container's properties are
/// under: the values of `kCGImagePropertyGIFDictionary`,
/// `kCGImagePropertyPNGDictionary` and `kCGImagePropertyWebPDictionary`. Named
/// by value rather than linked, because the WebP constants exist only from
/// macOS 11 and an x86-64 build may run on an older one.
const FORMAT_DICTIONARIES: [&std::ffi::CStr; 3] = [c"{GIF}", c"{PNG}", c"{WebP}"];

/// Decodes the first frame: the image itself, or an animated one's first
/// frame.
pub(crate) fn decode(
    bytes: &[u8],
    header: Option<ImageHeader>,
    max: (u32, u32),
) -> Result<Bitmap, DecodeError> {
    decode_from(&ImageSource::open(bytes)?, header, 0, max)
}

/// What an animated image's frames are, from its properties alone: one
/// frame unless `ImageIO` counts more, then each frame's stated delay and
/// the container's loop count.
pub(crate) fn frames(bytes: &[u8]) -> Result<ImageFrames, DecodeError> {
    let source = ImageSource::open(bytes)?;
    // SAFETY: the source is live.
    let count = unsafe { CGImageSourceGetCount(source.source.0) };
    if count <= 1 {
        return Ok(ImageFrames::still());
    }
    let keys: Vec<Owned> = FORMAT_DICTIONARIES
        .iter()
        .map(|key| cf_string(key))
        .collect();
    let (unclamped, clamped, loop_count) = (
        cf_string(c"UnclampedDelayTime"),
        cf_string(c"DelayTime"),
        cf_string(c"LoopCount"),
    );
    if keys
        .iter()
        .chain([&unclamped, &clamped, &loop_count])
        .any(|key| key.0.is_null())
    {
        return Err(DecodeError::Malformed(
            "CFStringCreateWithCString failed".to_owned(),
        ));
    }
    // The dictionary of whichever container this is, out of `properties`.
    let format = |properties: CFTypeRef| {
        keys.iter().find_map(|key| {
            // SAFETY: a live dictionary and a live key; the value is
            // borrowed from the dictionary.
            let value = unsafe { CFDictionaryGetValue(properties, key.0) };
            (!value.is_null()).then_some(value)
        })
    };
    // SAFETY: the source is live; a null dictionary is handled.
    let container =
        Owned(unsafe { CGImageSourceCopyProperties(source.source.0, std::ptr::null()) });
    let loops = (!container.0.is_null())
        .then(|| format(container.0))
        .flatten()
        .and_then(|dictionary| number_property(dictionary, loop_count.0))
        .map_or(Loops::ONCE, |plays| {
            Loops::from_plays(u32::try_from(plays.max(0)).unwrap_or(u32::MAX))
        });
    let delays = (0..count)
        .map(|index| {
            // SAFETY: the source is live and `index` is below its count; a
            // null dictionary is handled.
            let properties = Owned(unsafe {
                CGImageSourceCopyPropertiesAtIndex(source.source.0, index, std::ptr::null())
            });
            let seconds = (!properties.0.is_null())
                .then(|| format(properties.0))
                .flatten()
                .and_then(|dictionary| {
                    float_property(dictionary, unclamped.0)
                        .or_else(|| float_property(dictionary, clamped.0))
                })
                .unwrap_or(0.0);
            if seconds.is_finite() && seconds > 0.0 {
                Duration::try_from_secs_f64(seconds).unwrap_or(Duration::ZERO)
            } else {
                Duration::ZERO
            }
        })
        .collect();
    Ok(ImageFrames::new(delays, loops))
}

/// Decodes frame `index` of an animation: the whole canvas as it is shown
/// at that frame, from the source kept open for `bytes` ([`Opened`]).
pub(crate) fn decode_frame(
    bytes: &Bytes,
    header: Option<ImageHeader>,
    index: usize,
    max: (u32, u32),
) -> Result<Bitmap, DecodeError> {
    let source = Opened::source(bytes)?;
    let source = source.lock().unwrap_or_else(PoisonError::into_inner);
    decode_from(&source, header, index, max)
}

/// No more frames of `bytes` are asked for: their source is closed.
pub(crate) fn forget_frames(bytes: &Bytes) {
    Opened::forget(bytes);
}

/// How many animations' sources stay open at once; past it, the least
/// recently decoded is closed, and opened again by its next frame.
const OPEN_ANIMATIONS: usize = 16;

/// Every animation's source kept open between its frame decodes, keyed by
/// the allocation of the encoded bytes it was opened over.
///
/// `ImageIO` composites a GIF frame from the frames before it, and a source
/// decoding its frames in order carries that work from one to the next: on
/// macOS 26 frame 299 of a 320 × 240, 300-frame GIF costs about 180 ms from
/// a fresh source and under 1 ms from one that decoded frame 298. A source
/// is used by one decode at a time, behind its mutex.
static OPENED: Mutex<Vec<Opened>> = Mutex::new(Vec::new());

/// One animation's open source.
struct Opened {
    /// The bytes the source was opened over, kept so the allocation the
    /// entry is keyed on cannot be freed and reused by another image while
    /// the entry exists.
    bytes: Bytes,
    source: Arc<Mutex<ImageSource>>,
    /// When a frame of it was last asked for, in lookups.
    used: u64,
}

impl Opened {
    /// Whether this entry was opened over `bytes`' allocation.
    fn is(&self, bytes: &Bytes) -> bool {
        self.bytes.as_ptr() == bytes.as_ptr() && self.bytes.len() == bytes.len()
    }

    /// The source open for `bytes`, opened now if it is not.
    fn source(bytes: &Bytes) -> Result<Arc<Mutex<ImageSource>>, DecodeError> {
        static LOOKUPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let used = LOOKUPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let mut opened = OPENED.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(entry) = opened.iter_mut().find(|entry| entry.is(bytes)) {
                entry.used = used;
                return Ok(Arc::clone(&entry.source));
            }
        }
        // Opened outside the table's lock, since it copies the bytes.
        let source = Arc::new(Mutex::new(ImageSource::open(bytes)?));
        let mut opened = OPENED.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = opened.iter().find(|entry| entry.is(bytes)) {
            return Ok(Arc::clone(&entry.source));
        }
        if opened.len() >= OPEN_ANIMATIONS
            && let Some(oldest) = opened
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(index, _)| index)
        {
            opened.swap_remove(oldest);
        }
        opened.push(Self {
            bytes: bytes.clone(),
            source: Arc::clone(&source),
            used,
        });
        Ok(source)
    }

    fn forget(bytes: &Bytes) {
        OPENED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|entry| !entry.is(bytes));
    }

    /// Whether a source is open for `bytes`, for tests.
    #[cfg(test)]
    fn is_open(bytes: &Bytes) -> bool {
        OPENED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|entry| entry.is(bytes))
    }
}

/// Decodes frame `index` of `source`, at most `max` large.
fn decode_from(
    source: &ImageSource,
    header: Option<ImageHeader>,
    index: usize,
    max: (u32, u32),
) -> Result<Bitmap, DecodeError> {
    // SAFETY: the source is live.
    if index >= unsafe { CGImageSourceGetCount(source.source.0) } {
        return Err(DecodeError::Malformed(format!(
            "the image has no frame {index}"
        )));
    }
    let (source_width, source_height) = source.intrinsic_size(header);
    let image = source.thumbnail(index, max_pixel_for((source_width, source_height), max))?;
    let (width, height) = image.size()?;
    let rgba = render_rgba(&image, width, height)?;
    Ok(Bitmap {
        width,
        height,
        source_width: if source_width == 0 {
            width
        } else {
            source_width
        },
        source_height: if source_height == 0 {
            height
        } else {
            source_height
        },
        premultiplied: true,
        rgba,
    })
}

/// The `kCGImageSourceThumbnailMaxPixelSize` that fits a `source`-sized
/// image inside `max`: the longer side of the target, since the thumbnail
/// keeps the ratio. Without a known source size the bound itself stands.
fn max_pixel_for(source: (u32, u32), max: (u32, u32)) -> i32 {
    let longest = if source.0 == 0 || source.1 == 0 {
        max.0.max(max.1)
    } else {
        let (width, height) = target_size(source.0, source.1, max);
        width.max(height)
    };
    i32::try_from(longest).unwrap_or(i32::MAX)
}

/// An `ImageIO` source over a copy of the encoded bytes.
struct ImageSource {
    source: Owned,
    _data: Owned,
}

// SAFETY: a `CoreFoundation` object's reference count is atomic, so the
// source and its data may be released on any thread, and an `ImageIO` source
// is used from one thread at a time: by the decode that opened it, or behind
// the mutex of its `Opened` entry.
unsafe impl Send for ImageSource {}

impl ImageSource {
    fn open(bytes: &[u8]) -> Result<Self, DecodeError> {
        let length = CFIndex::try_from(bytes.len()).map_err(|_| {
            DecodeError::Malformed("the image is larger than CFData can hold".to_owned())
        })?;
        // SAFETY: `CFDataCreate` copies `length` bytes readable at the pointer.
        let data = Owned(unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), length) });
        if data.0.is_null() {
            return Err(DecodeError::Malformed("CFDataCreate failed".to_owned()));
        }
        // SAFETY: the data object is live; a null source means `ImageIO` does
        // not recognise the container at all.
        let source = Owned(unsafe { CGImageSourceCreateWithData(data.0, std::ptr::null()) });
        if source.0.is_null() {
            return Err(DecodeError::Unsupported(
                "ImageIO does not recognise this image".to_owned(),
            ));
        }
        // SAFETY: the source is live.
        if unsafe { CGImageSourceGetCount(source.0) } == 0 {
            return Err(DecodeError::Malformed("the image has no frames".to_owned()));
        }
        Ok(Self {
            source,
            _data: data,
        })
    }

    /// The image's own size, oriented upright, read from its properties
    /// without decoding; the header's size when the properties say nothing.
    fn intrinsic_size(&self, header: Option<ImageHeader>) -> (u32, u32) {
        let fallback = header.map_or((0, 0), |header| (header.width, header.height));
        // SAFETY: the source is live; a null dictionary is handled.
        let properties = Owned(unsafe {
            CGImageSourceCopyPropertiesAtIndex(self.source.0, 0, std::ptr::null())
        });
        if properties.0.is_null() {
            return fallback;
        }
        // SAFETY: reading framework-exported constant keys from a live
        // dictionary.
        let (width, height, orientation) = unsafe {
            (
                number_property(properties.0, kCGImagePropertyPixelWidth),
                number_property(properties.0, kCGImagePropertyPixelHeight),
                number_property(properties.0, kCGImagePropertyOrientation).unwrap_or(1),
            )
        };
        let axis = |value: Option<i64>| value.and_then(|value| u32::try_from(value).ok());
        let (Some(width), Some(height)) = (axis(width), axis(height)) else {
            return fallback;
        };
        if width == 0 || height == 0 {
            return fallback;
        }
        // Orientations 5 to 8 are the transposing ones.
        if (5..=8).contains(&orientation) {
            (height, width)
        } else {
            (width, height)
        }
    }

    /// Decodes frame `index`, downsampled so its longer side is at most
    /// `max_pixel`, with any EXIF orientation applied.
    fn thumbnail(&self, index: usize, max_pixel: i32) -> Result<CgImage, DecodeError> {
        // SAFETY: `CFNumberCreate` copies the 32-bit integer at the pointer.
        let max_pixel_number = Owned(unsafe {
            CFNumberCreate(
                std::ptr::null(),
                K_CF_NUMBER_SINT32_TYPE,
                (&raw const max_pixel).cast::<c_void>(),
            )
        });
        // SAFETY: the keys and `kCFBooleanTrue` are framework-exported
        // constants, the arrays are three live values each, and the CFType
        // callbacks retain them for the dictionary's life.
        let options = unsafe {
            let keys: [CFTypeRef; 3] = [
                kCGImageSourceThumbnailMaxPixelSize,
                kCGImageSourceCreateThumbnailFromImageAlways,
                kCGImageSourceCreateThumbnailWithTransform,
            ];
            let values: [CFTypeRef; 3] = [max_pixel_number.0, kCFBooleanTrue, kCFBooleanTrue];
            Owned(CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                3,
                &raw const kCFTypeDictionaryKeyCallBacks,
                &raw const kCFTypeDictionaryValueCallBacks,
            ))
        };
        if options.0.is_null() {
            return Err(DecodeError::Malformed(
                "CFDictionaryCreate failed".to_owned(),
            ));
        }
        // SAFETY: the source and options are live; a null image is a decode
        // failure.
        let image = unsafe { CGImageSourceCreateThumbnailAtIndex(self.source.0, index, options.0) };
        if image.is_null() {
            return Err(DecodeError::Malformed(
                "ImageIO could not decode the image".to_owned(),
            ));
        }
        Ok(CgImage(image))
    }
}

/// Draws `image` into a `width` x `height` premultiplied RGBA8 buffer.
fn render_rgba(image: &CgImage, width: u32, height: u32) -> Result<Vec<u8>, DecodeError> {
    let too_large = || DecodeError::Malformed("the image is too large".to_owned());
    let columns = usize::try_from(width).map_err(|_| too_large())?;
    let rows = usize::try_from(height).map_err(|_| too_large())?;
    let stride = columns.checked_mul(4).ok_or_else(too_large)?;
    let mut rgba = vec![0_u8; stride.checked_mul(rows).ok_or_else(too_large)?];
    // SAFETY: the bitmap context draws into `rgba`, which is exactly
    // `stride * rows` bytes and outlives the context; the colour space and
    // context are released after the draw.
    unsafe {
        let space = CGColorSpaceCreateDeviceRGB();
        let context = CGBitmapContextCreate(
            rgba.as_mut_ptr().cast::<c_void>(),
            columns,
            rows,
            8,
            stride,
            space,
            RGBA_PREMULTIPLIED,
        );
        if context.is_null() {
            CGColorSpaceRelease(space);
            return Err(DecodeError::Malformed(
                "CGBitmapContextCreate failed".to_owned(),
            ));
        }
        CGContextDrawImage(
            context,
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: f64::from(width),
                    height: f64::from(height),
                },
            },
            image.0,
        );
        CGContextRelease(context);
        CGColorSpaceRelease(space);
    }
    Ok(rgba)
}

/// A `CGImage` released on drop.
struct CgImage(CFTypeRef);

impl CgImage {
    /// The bitmap's size, refused when either axis is empty.
    fn size(&self) -> Result<(u32, u32), DecodeError> {
        // SAFETY: accessors on a live image.
        let (width, height) = unsafe { (CGImageGetWidth(self.0), CGImageGetHeight(self.0)) };
        let width = u32::try_from(width)
            .map_err(|_| DecodeError::Malformed("the image is too wide".to_owned()))?;
        let height = u32::try_from(height)
            .map_err(|_| DecodeError::Malformed("the image is too tall".to_owned()))?;
        if width == 0 || height == 0 {
            return Err(DecodeError::Malformed(
                "the decoded image is empty".to_owned(),
            ));
        }
        Ok((width, height))
    }
}

impl Drop for CgImage {
    fn drop(&mut self) {
        // SAFETY: this struct owns the one reference `Create` returned.
        unsafe { CGImageRelease(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::{Opened, decode_frame, forget_frames};

    /// An animation's source stays open between its frames, keyed by its
    /// bytes' allocation, and closes when its frames are forgotten.
    #[test]
    fn an_animation_s_source_stays_open_until_its_frames_are_forgotten() {
        let bytes = Bytes::from(
            std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/animated/rgb-twice.gif"),
            )
            .expect("the GIF fixture"),
        );
        assert!(!Opened::is_open(&bytes));
        for index in 0..3 {
            decode_frame(&bytes, None, index, (8, 8)).expect("a frame");
        }
        assert!(Opened::is_open(&bytes));
        let copy = Bytes::from(bytes.to_vec());
        assert!(!Opened::is_open(&copy), "keyed by allocation, not content");
        forget_frames(&bytes);
        assert!(!Opened::is_open(&bytes));
    }
}
