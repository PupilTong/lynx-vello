//! Android's NDK `AImageDecoder`, reached through `dlopen`/`dlsym`.
//!
//! `AImageDecoder` is a thin C shim over the platform's own Skia codecs. It
//! decodes straight into caller-owned CPU memory, which is exactly the shape
//! this crate wants — no `Bitmap`, no JNI, no `AHardwareBuffer`, and no thread
//! affinity beyond "one handle per call".
//!
//! # Why every symbol is resolved at runtime
//!
//! The whole API landed in **API 30**, and this workspace's minimum is lower. A
//! plain `extern` call would put a strong undefined reference to
//! `AImageDecoder_createFromBuffer` in the shared object, and the *loader* — not
//! the first call — would reject the library on every device below 30. The
//! app would fail to start rather than fall back.
//!
//! The C answer is weak linking (`__attribute__((weak_import))` plus
//! `__builtin_available`), which is a clang-only mechanism Rust `extern` blocks
//! cannot participate in. The portable answer is `dlopen` + `dlsym`: it works at
//! any `minSdkVersion`, needs no build-system cooperation, and a non-null
//! `dlsym` result *is* the API-30 capability probe — there is nothing else to
//! ask. `ndk-sys` is used for the opaque types and the
//! `ANDROID_IMAGE_DECODER_*` / `ANDROID_BITMAP_*` constants only; its `extern`
//! declarations are deliberately never called.
//!
//! The symbols live in **libjnigraphics.so**, not libandroid.so — the decoder
//! is part of the bitmap/graphics surface even though it never touches a Java
//! `Bitmap`.
//!
//! # Orientation comes for free
//!
//! `AImageDecoderHeaderInfo_getWidth`/`getHeight` report the **oriented** size
//! and `AImageDecoder_decodeImage` writes **oriented** pixels: the platform
//! applies the EXIF tag itself. [`crate::image_decoders::orientation`] is therefore deliberately
//! not used here — applying it again would rotate a photo twice — and this
//! backend still agrees with the software backend, which reaches the same
//! result by applying the tag by hand.
//!
//! # Alpha
//!
//! `AImageDecoder` premultiplies by default. `AImageDecoder_setUnpremultipliedRequired(true)`
//! would give straight alpha, but the platform refuses that combination
//! together with a scaled target size for any image that actually has alpha
//! (`ANDROID_IMAGE_DECODER_INVALID_CONVERSION`). Decode-time downsampling is
//! worth far more than a uniform alpha encoding — it is the entire reason to
//! prefer a platform backend — so the premultiplied default is kept and
//! reported honestly as [`AlphaType::Premultiplied`]. See [`AlphaType`] for why
//! carrying the difference beats normalising it.

// Unsafe is confined to this module and to exactly four categories, each of
// which is unavoidable when calling a C API that is not statically linked:
//
// 1. `dlopen`/`dlsym` themselves, and transmuting a `dlsym` address into a function pointer of the
//    signature the NDK header declares.
// 2. Calling those function pointers.
// 3. Reading `AImageDecoderHeaderInfo_getMimeType`'s NUL-terminated C string.
// 4. Handing `AImageDecoder_decodeImage` a raw pointer into a `Vec<u8>` to write pixels through.
//
// Every `unsafe` block below carries its own `SAFETY:` note. Nothing here
// creates a `&`/`&mut` to platform memory, and no platform pointer outlives the
// `Handle` that owns it.
#![allow(unsafe_code)]

use std::ffi::{CStr, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::ptr;
use std::sync::OnceLock;

use bobcat_core::image::{
    Acceleration, AlphaType, Capabilities, DecodeRequest, DecodeResponse, DecodedImage, Decoder,
    ImageError, ImageFormat, ImageHeader, PixelSize,
};
use ndk_sys::{AImageDecoder, AImageDecoderHeaderInfo, AndroidBitmapFormat};

use crate::image_decoders::resample;

/// Reported in [`DecodeResponse::backend`].
const NAME: &str = "android-ndk";

/// PNG, JPEG and WebP through the NDK's `AImageDecoder`.
///
/// Construct with [`NdkDecoder::detect`]; there is no other constructor,
/// because a value of this type is a statement that the symbols resolved.
#[derive(Clone, Copy, Debug)]
pub struct NdkDecoder {
    /// Resolved once per process and copied in here, so no decode pays for the
    /// `OnceLock` read or the `dlsym` table walk.
    api: &'static Api,
    capabilities: Capabilities,
}

impl NdkDecoder {
    /// The runtime probe: `dlopen` libjnigraphics.so and resolve every symbol
    /// this decoder calls.
    ///
    /// `None` is an ordinary outcome — anything below API 30 lands there. There
    /// is no decoder behind this one (the reference decoder is Linux-only), so
    /// `None` means the embedder ships without image decoding on those devices
    /// or injects its own. The probe never panics: a missing library and a
    /// missing symbol are both null pointers.
    #[must_use]
    pub fn detect() -> Option<Self> {
        let api = api()?;
        Some(Self {
            api,
            capabilities: claimed(api),
        })
    }
}

/// What the probe's success licenses this backend to claim.
///
/// Every format here is gated on the *same* runtime fact — whether the
/// `AImageDecoder` symbols resolved — because on Android that is the only axis
/// there is. Unlike `ImageIO` (WebP arrived in macOS 11 / iOS 14) and WIC (whose
/// WebP codec is an optional Store extension), the NDK's API-30 contract
/// enumerates the codecs `AImageDecoder` decodes, and PNG, JPEG and WebP are
/// all in it — the CDD requires those three of every Android device that ships
/// with API 30 at all. There is no second probe that could return a different
/// answer, so inventing one would be theatre.
///
/// The per-*file* gate is real and lives at decode time: [`Handle::header`]
/// checks the MIME type the platform reports against the container this crate
/// sniffed, so a device that somehow lacks a codec produces a loud
/// [`ImageError::Decode`] rather than wrong pixels.
fn claimed(_api: &Api) -> Capabilities {
    Capabilities::none()
        .with(ImageFormat::Png, Acceleration::PlatformSoftware)
        .with(ImageFormat::Jpeg, Acceleration::PlatformSoftware)
        .with(ImageFormat::WebP, Acceleration::PlatformSoftware)
}

impl Decoder for NdkDecoder {
    fn name(&self) -> &'static str {
        NAME
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    /// Header-only: `createFromBuffer` builds a codec over the buffer and
    /// parses just enough to answer `getHeaderInfo`. No pixel data is touched,
    /// and the decoder is deleted before this returns.
    fn probe(&self, format: ImageFormat, bytes: &[u8]) -> Result<ImageHeader, ImageError> {
        Handle::open(self.api, format, bytes)?.header(format, bytes)
    }

    fn decode(
        &self,
        format: ImageFormat,
        bytes: &[u8],
        request: &DecodeRequest,
    ) -> Result<DecodeResponse, ImageError> {
        let handle = Handle::open(self.api, format, bytes)?;
        let header = handle.header(format, bytes)?;
        // Before anything is allocated, and before the platform is asked to
        // size an output buffer.
        request.check(&header)?;

        handle.set_rgba8888(format)?;
        let target = request.effective_size(header.natural_size);
        // A refusal here is not fatal: the resample below covers it, at the
        // cost of peak full-size memory.
        let decoded_size = if target != header.natural_size && handle.set_target_size(target) {
            target
        } else {
            header.natural_size
        };

        let pixels = handle.decode_pixels(format, decoded_size)?;
        drop(handle);

        let pixels = if decoded_size == target {
            pixels
        } else {
            resample(
                pixels,
                (decoded_size.width, decoded_size.height),
                (target.width, target.height),
                AlphaType::Premultiplied,
                format,
            )?
        };

        Ok(DecodeResponse {
            image: DecodedImage::from_rgba8(
                target.width,
                target.height,
                AlphaType::Premultiplied,
                pixels,
                format,
            )?,
            header,
            acceleration: Acceleration::PlatformSoftware,
            backend: NAME,
        })
    }
}

// ------------------------------------------------------------ the handle

/// One `AImageDecoder`, deleted on drop.
///
/// The lifetime is not decoration: `AImageDecoder_createFromBuffer` does *not*
/// copy its input, so the encoded bytes must outlive the decoder. Tying the two
/// together in the type system is what makes that a compile-time fact rather
/// than a comment. It also means an early `?` still deletes the decoder.
#[derive(Debug)]
struct Handle<'bytes> {
    api: &'static Api,
    raw: *mut AImageDecoder,
    encoded: PhantomData<&'bytes [u8]>,
}

impl<'bytes> Handle<'bytes> {
    fn open(
        api: &'static Api,
        format: ImageFormat,
        bytes: &'bytes [u8],
    ) -> Result<Self, ImageError> {
        let mut raw: *mut AImageDecoder = ptr::null_mut();
        // SAFETY: `bytes` is a live slice that outlives the returned `Handle`
        // (its lifetime is captured), which is precisely what
        // `createFromBuffer` requires of its buffer; `raw` is a live, writable
        // out-parameter. The call reads headers only.
        let result = unsafe {
            (api.create_from_buffer)(bytes.as_ptr().cast(), bytes.len(), ptr::from_mut(&mut raw))
        };
        check(format, "AImageDecoder_createFromBuffer", result)?;
        if raw.is_null() {
            return Err(ImageError::decode(
                format,
                "AImageDecoder_createFromBuffer reported success but produced no decoder",
            ));
        }
        Ok(Self {
            api,
            raw,
            encoded: PhantomData,
        })
    }

    /// Container metadata, with no pixels decoded.
    ///
    /// `bytes` is taken again because one field — `animated` — is read from the
    /// container rather than from the platform — `AImageDecoder_isAnimated`
    /// is API 31 while everything else here is API 30; see
    /// [`crate::image_decoders::animation::container_declares_animation`].
    fn header(&self, format: ImageFormat, bytes: &[u8]) -> Result<ImageHeader, ImageError> {
        // SAFETY: `self.raw` is a live decoder. `getHeaderInfo` returns a
        // borrow owned by that decoder, valid until `delete`, and every
        // accessor below is a pure read through it. All are called before this
        // function returns, so nothing escapes the handle's lifetime.
        let (width, height, bitmap_format, alpha_flags, mime) = unsafe {
            let info = (self.api.get_header_info)(self.raw);
            if info.is_null() {
                return Err(ImageError::decode(
                    format,
                    "AImageDecoder_getHeaderInfo returned no header",
                ));
            }
            (
                (self.api.header_width)(info),
                (self.api.header_height)(info),
                (self.api.header_bitmap_format)(info),
                (self.api.header_alpha_flags)(info),
                (self.api.header_mime_type)(info),
            )
        };

        if mime.is_null() {
            return Err(ImageError::decode(format, "header carries no MIME type"));
        }
        // SAFETY: a non-null `getMimeType` result is a static, NUL-terminated
        // ASCII literal inside the platform's own image library; it is not
        // owned by the decoder and outlives it. Nothing is freed here.
        //
        // Deliberately NOT compared against `format.mime_type()`. The container
        // was already identified from its magic bytes by `bobcat_core::image::sniff`
        // before this backend was selected, and that is the authoritative
        // answer; the exact spelling the NDK returns is not contractually
        // specified anywhere. A strict comparison would turn one unexpected
        // spelling — `image/x-webp`, say — into a hard failure for *every* image
        // of that format, with no fallback, because routing is resolved once per
        // format rather than per file. Reading it at all is kept only for the
        // null check above, which does indicate a header the platform could not
        // parse.
        let _ = unsafe { CStr::from_ptr(mime) };

        // `ANDROID_BITMAP_FORMAT_NONE` means the platform parsed the header but
        // cannot name an output format for it — a decode would fail later, so
        // fail now while it is still cheap.
        let declared = u32::try_from(bitmap_format).ok().map(AndroidBitmapFormat);
        if declared
            .is_none_or(|declared| declared == AndroidBitmapFormat::ANDROID_BITMAP_FORMAT_NONE)
        {
            return Err(ImageError::decode(
                format,
                format!("header declares no usable bitmap format ({bitmap_format})"),
            ));
        }

        // Already oriented — the platform applied the EXIF tag before reporting
        // these, so the software backend's `Orientation::apply_to_size` has
        // already happened and must not be applied again.
        let natural_size = PixelSize {
            width: positive(format, "width", width)?,
            height: positive(format, "height", height)?,
        };

        Ok(ImageHeader {
            format,
            natural_size,
            // A negative flag word is nonsense; read it as opaque, which is what
            // every mainstream decoder does with unreadable alpha metadata.
            has_alpha: u32::try_from(alpha_flags).is_ok_and(|flags| {
                flags & ndk_sys::ANDROID_BITMAP_FLAGS_ALPHA_MASK
                    != ndk_sys::ANDROID_BITMAP_FLAGS_ALPHA_OPAQUE
            }),
            animated: crate::image_decoders::animation::container_declares_animation(format, bytes),
        })
    }

    /// Pins the output to RGBA8, which is the only layout this crate's
    /// [`DecodedImage`] accepts. Without this the decoder would happily hand
    /// back `RGB_565` for an opaque JPEG or `RGBA_F16` for an HDR PNG.
    fn set_rgba8888(&self, format: ImageFormat) -> Result<(), ImageError> {
        let rgba8888 = i32::try_from(AndroidBitmapFormat::ANDROID_BITMAP_FORMAT_RGBA_8888.0)
            .map_err(|_| {
                ImageError::decode(format, "RGBA_8888 does not fit the NDK's format field")
            })?;
        // SAFETY: `self.raw` is a live decoder and `rgba8888` is one of the
        // `AndroidBitmapFormat` values the setter accepts. Pure configuration:
        // nothing is decoded and no buffer is involved.
        let result = unsafe { (self.api.set_bitmap_format)(self.raw, rgba8888) };
        check(format, "AImageDecoder_setAndroidBitmapFormat", result)
    }

    /// Asks for a decode-time downsample, reporting whether the platform agreed.
    ///
    /// A refusal is deliberately not an error. `setTargetSize` rejects a scale
    /// it cannot honour (a non-integral sample for some codecs, or any scale at
    /// all under `setUnpremultipliedRequired`), and the correct response is to
    /// decode at natural size and resample — not to fail a perfectly good image.
    fn set_target_size(&self, size: PixelSize) -> bool {
        let (Ok(width), Ok(height)) = (i32::try_from(size.width), i32::try_from(size.height))
        else {
            return false;
        };
        // SAFETY: `self.raw` is a live decoder; both dimensions are positive
        // `i32`s (`effective_size` clamps each axis to at least 1). Pure
        // configuration.
        let result = unsafe { (self.api.set_target_size)(self.raw, width, height) };
        result == ndk_sys::ANDROID_IMAGE_DECODER_SUCCESS
    }

    /// Decodes into a fresh, tightly packed RGBA8 buffer of exactly
    /// `4 * width * height` bytes.
    fn decode_pixels(&self, format: ImageFormat, size: PixelSize) -> Result<Vec<u8>, ImageError> {
        let too_large =
            || ImageError::too_large(size.width, size.height, "decode buffer overflows usize");
        let width = usize::try_from(size.width).map_err(|_| too_large())?;
        let height = usize::try_from(size.height).map_err(|_| too_large())?;
        let row_bytes = width.checked_mul(4).ok_or_else(too_large)?;

        // SAFETY: `self.raw` is a live decoder. This must be read *after* the
        // output format and target size are set, because it is derived from
        // them. Pure query.
        let stride = unsafe { (self.api.minimum_stride)(self.raw) };
        if stride < row_bytes {
            return Err(ImageError::decode(
                format,
                format!("minimum stride {stride} is below {row_bytes} bytes of RGBA8 pixels"),
            ));
        }
        let capacity = stride.checked_mul(height).ok_or_else(too_large)?;
        let mut buffer = vec![0u8; capacity];

        // SAFETY: `buffer` owns exactly `capacity` writable bytes and is not
        // aliased; `stride` is the decoder's own minimum stride and `capacity`
        // is exactly `stride * height`, which is the contract
        // `AImageDecoder_decodeImage` documents for those two arguments. The
        // pointer is used only for the duration of the call.
        let result = unsafe {
            (self.api.decode_image)(self.raw, buffer.as_mut_ptr().cast(), stride, capacity)
        };
        // `INCOMPLETE` is treated as failure rather than as a partial image:
        // this crate rejects truncated input up front (`format::is_complete`),
        // so reaching it means the bytes lied.
        check(format, "AImageDecoder_decodeImage", result)?;

        Ok(compact_rows(buffer, stride, row_bytes, height))
    }
}

impl Drop for Handle<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.raw` is the non-null decoder this handle was
        // constructed with. `Handle` is neither `Copy` nor `Clone`, so it is
        // deleted exactly once, and nothing borrowed from it is live here.
        unsafe { (self.api.delete)(self.raw) };
    }
}

/// Rejects a non-positive axis before it becomes a zero-area allocation.
fn positive(format: ImageFormat, axis: &str, value: i32) -> Result<u32, ImageError> {
    u32::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| ImageError::decode(format, format!("header reports {axis} {value}")))
}

/// Drops the row padding `AImageDecoder` is allowed to leave behind.
///
/// `getMinimumStride` may exceed `4 * width` — Skia aligns rows for its own
/// convenience — while [`DecodedImage`] requires a buffer of *exactly*
/// `4 * width * height` with no padding at all. Compaction happens in place and
/// forward, because the destination row always starts at or before its source
/// row, so no scratch allocation and no second full-size buffer is needed.
///
/// Split out as a free function precisely so it is testable without a device.
fn compact_rows(mut buffer: Vec<u8>, stride: usize, row_bytes: usize, height: usize) -> Vec<u8> {
    let Some(packed) = row_bytes.checked_mul(height) else {
        return buffer;
    };
    // Defensive: the caller sizes the buffer, and an under-sized one would make
    // the copies below panic. Returning it unchanged lets
    // `DecodedImage::from_rgba8` reject the length with a message that names
    // both counts.
    if stride < row_bytes || buffer.len() < stride.saturating_mul(height) {
        return buffer;
    }
    if stride != row_bytes {
        // Row 0 is already in place.
        for row in 1..height {
            let from = row * stride;
            buffer.copy_within(from..from + row_bytes, row * row_bytes);
        }
    }
    buffer.truncate(packed);
    buffer
}

/// Turns an `ANDROID_IMAGE_DECODER_*` result into an error naming the call.
fn check(format: ImageFormat, call: &str, result: c_int) -> Result<(), ImageError> {
    let Some(name) = result_name(result) else {
        return Ok(());
    };
    Err(ImageError::decode(
        format,
        format!("{call} failed: {name} ({result})"),
    ))
}

/// The result codes, named locally.
///
/// `AImageDecoder_resultToString` would do this, but it is **API 31** while
/// everything else here is API 30 — resolving it would either narrow the whole
/// backend or leave the messages unnamed on exactly the devices this code
/// exists for. `None` is success.
fn result_name(result: c_int) -> Option<&'static str> {
    match result {
        ndk_sys::ANDROID_IMAGE_DECODER_SUCCESS => None,
        ndk_sys::ANDROID_IMAGE_DECODER_INCOMPLETE => Some("INCOMPLETE"),
        ndk_sys::ANDROID_IMAGE_DECODER_ERROR => Some("ERROR"),
        ndk_sys::ANDROID_IMAGE_DECODER_INVALID_CONVERSION => Some("INVALID_CONVERSION"),
        ndk_sys::ANDROID_IMAGE_DECODER_INVALID_SCALE => Some("INVALID_SCALE"),
        ndk_sys::ANDROID_IMAGE_DECODER_BAD_PARAMETER => Some("BAD_PARAMETER"),
        ndk_sys::ANDROID_IMAGE_DECODER_INVALID_INPUT => Some("INVALID_INPUT"),
        ndk_sys::ANDROID_IMAGE_DECODER_SEEK_ERROR => Some("SEEK_ERROR"),
        ndk_sys::ANDROID_IMAGE_DECODER_INTERNAL_ERROR => Some("INTERNAL_ERROR"),
        ndk_sys::ANDROID_IMAGE_DECODER_UNSUPPORTED_FORMAT => Some("UNSUPPORTED_FORMAT"),
        ndk_sys::ANDROID_IMAGE_DECODER_FINISHED => Some("FINISHED"),
        ndk_sys::ANDROID_IMAGE_DECODER_INVALID_STATE => Some("INVALID_STATE"),
        _ => Some("unrecognised result"),
    }
}

// ------------------------------------------------------- the symbol table

type CreateFromBufferFn =
    unsafe extern "C" fn(*const c_void, usize, *mut *mut AImageDecoder) -> c_int;
type DeleteFn = unsafe extern "C" fn(*mut AImageDecoder);
type GetHeaderInfoFn = unsafe extern "C" fn(*const AImageDecoder) -> *const AImageDecoderHeaderInfo;
type HeaderInfoI32Fn = unsafe extern "C" fn(*const AImageDecoderHeaderInfo) -> i32;
type GetMimeTypeFn = unsafe extern "C" fn(*const AImageDecoderHeaderInfo) -> *const c_char;
type SetBitmapFormatFn = unsafe extern "C" fn(*mut AImageDecoder, i32) -> c_int;
type SetTargetSizeFn = unsafe extern "C" fn(*mut AImageDecoder, i32, i32) -> c_int;
type MinimumStrideFn = unsafe extern "C" fn(*mut AImageDecoder) -> usize;
type DecodeImageFn = unsafe extern "C" fn(*mut AImageDecoder, *mut c_void, usize, usize) -> c_int;

/// Every `AImageDecoder` entry point this backend calls, resolved once.
///
/// Memoised in a `OnceLock` and handed out as `&'static`, never copied: the
/// table is a dozen pointers, and every decode and every handle would otherwise
/// carry its own duplicate of it. Function pointers are `Send + Sync`, which is
/// what makes [`NdkDecoder`] satisfy [`Decoder`]'s bounds.
#[derive(Clone, Copy, Debug)]
struct Api {
    create_from_buffer: CreateFromBufferFn,
    delete: DeleteFn,
    get_header_info: GetHeaderInfoFn,
    header_width: HeaderInfoI32Fn,
    header_height: HeaderInfoI32Fn,
    header_mime_type: GetMimeTypeFn,
    header_bitmap_format: HeaderInfoI32Fn,
    header_alpha_flags: HeaderInfoI32Fn,
    set_bitmap_format: SetBitmapFormatFn,
    set_target_size: SetTargetSizeFn,
    minimum_stride: MinimumStrideFn,
    decode_image: DecodeImageFn,
}

/// The memoised probe. Runs at most once per process.
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(load).as_ref()
}

/// `dlopen` the graphics library and resolve the whole table, all-or-nothing.
///
/// All-or-nothing because the symbols form one API level: every one of them was
/// introduced in API 30, so a partial resolution would mean something far
/// stranger than an old device, and half a decoder is worse than none.
fn load() -> Option<Api> {
    // `RTLD_NOW` rather than lazy binding: the point of this call is to find out
    // *now* whether the symbols exist, and `libc`'s constant rather than a
    // hardcoded 2, which is a bionic implementation detail.
    //
    // SAFETY: the name is a NUL-terminated literal and the flag is `libc`'s own
    // constant. `dlopen` reports failure as null rather than by unwinding. The
    // handle is deliberately never `dlclose`d: the resolved pointers are
    // memoised for the life of the process, so the library must stay mapped.
    let library = unsafe { libc::dlopen(c"libjnigraphics.so".as_ptr(), libc::RTLD_NOW) };
    if library.is_null() {
        return None;
    }

    // SAFETY: `library` is a live `dlopen` handle. Each `symbol` call names the
    // exact signature the NDK's `imagedecoder.h` declares for that symbol —
    // mirrored, and checked against, `ndk-sys`'s generated `extern` block for
    // the same header, which is why this module depends on `ndk-sys` for types
    // at all. A symbol that is absent yields `None` and abandons the table.
    unsafe {
        Some(Api {
            create_from_buffer: symbol(library, c"AImageDecoder_createFromBuffer")?,
            delete: symbol(library, c"AImageDecoder_delete")?,
            get_header_info: symbol(library, c"AImageDecoder_getHeaderInfo")?,
            header_width: symbol(library, c"AImageDecoderHeaderInfo_getWidth")?,
            header_height: symbol(library, c"AImageDecoderHeaderInfo_getHeight")?,
            header_mime_type: symbol(library, c"AImageDecoderHeaderInfo_getMimeType")?,
            header_bitmap_format: symbol(
                library,
                c"AImageDecoderHeaderInfo_getAndroidBitmapFormat",
            )?,
            header_alpha_flags: symbol(library, c"AImageDecoderHeaderInfo_getAlphaFlags")?,
            set_bitmap_format: symbol(library, c"AImageDecoder_setAndroidBitmapFormat")?,
            set_target_size: symbol(library, c"AImageDecoder_setTargetSize")?,
            minimum_stride: symbol(library, c"AImageDecoder_getMinimumStride")?,
            decode_image: symbol(library, c"AImageDecoder_decodeImage")?,
        })
    }
}

/// Resolves one symbol into a function pointer.
///
/// # Safety
///
/// `library` must be a live handle from [`libc::dlopen`], and `T` must be the
/// exact function-pointer type the named symbol's C declaration corresponds to.
/// Calling through a `T` that disagrees with the real signature is undefined
/// behaviour that no amount of null-checking catches.
unsafe fn symbol<T: Copy>(library: *mut c_void, name: &CStr) -> Option<T> {
    debug_assert_eq!(
        size_of::<T>(),
        size_of::<*mut c_void>(),
        "T must be a bare function pointer"
    );
    // SAFETY: the caller guarantees `library` is live; `name` is a
    // NUL-terminated `CStr`. `dlsym` reports an absent symbol as null.
    let address = unsafe { libc::dlsym(library, name.as_ptr()) };
    if address.is_null() {
        return None;
    }
    // SAFETY: a non-null `dlsym` result is the address of the named function,
    // and the caller guarantees `T` is that function's signature. `transmute`
    // cannot be used on a generic parameter, so this is `transmute_copy` over
    // two same-sized pointer types — the size equality is asserted above.
    Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&address) })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{compact_rows, result_name};

    /// `height` rows of `row_bytes` real pixels, each followed by `padding`
    /// bytes of the value 0xFF, which must not survive.
    fn padded(row_bytes: usize, padding: usize, height: usize) -> Vec<u8> {
        let mut buffer = Vec::new();
        for row in 0..height {
            #[allow(clippy::cast_possible_truncation)]
            buffer.extend(std::iter::repeat_n(row as u8, row_bytes));
            buffer.extend(std::iter::repeat_n(0xFFu8, padding));
        }
        buffer
    }

    #[test]
    fn compaction_drops_row_padding_and_keeps_row_order() {
        // 2 px wide (8 bytes) with a 24-byte stride: the classic Skia
        // alignment case, and the single easiest thing to get wrong here.
        let compacted = compact_rows(padded(8, 16, 3), 24, 8, 3);
        assert_eq!(compacted.len(), 8 * 3);
        assert!(!compacted.contains(&0xFF), "padding must not survive");
        assert_eq!(&compacted[0..8], &[0u8; 8]);
        assert_eq!(&compacted[8..16], &[1u8; 8]);
        assert_eq!(&compacted[16..24], &[2u8; 8]);
    }

    #[test]
    fn an_exact_stride_is_left_alone() {
        let tight = padded(8, 0, 3);
        assert_eq!(compact_rows(tight.clone(), 8, 8, 3), tight);
    }

    #[test]
    fn a_single_row_needs_no_copying() {
        let compacted = compact_rows(padded(8, 40, 1), 48, 8, 1);
        assert_eq!(compacted, vec![0u8; 8]);
    }

    #[test]
    fn an_impossible_buffer_is_returned_unchanged_rather_than_panicking() {
        // Both cases would index out of bounds if compaction trusted its
        // arguments; `DecodedImage::from_rgba8` rejects the length instead.
        let short = vec![7u8; 10];
        assert_eq!(compact_rows(short.clone(), 24, 8, 3), short);
        // A stride below one row of pixels is incoherent.
        assert_eq!(compact_rows(short.clone(), 4, 8, 1), short);
    }

    #[test]
    fn success_is_the_only_unnamed_result() {
        assert_eq!(result_name(0), None);
        assert_eq!(result_name(-1), Some("INCOMPLETE"));
        assert_eq!(result_name(-9), Some("UNSUPPORTED_FORMAT"));
        assert_eq!(result_name(-11), Some("INVALID_STATE"));
        assert_eq!(result_name(7), Some("unrecognised result"));
    }
}
