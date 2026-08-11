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

const NAME: &str = "android-ndk";

/// Decoder backed by Android's dynamically loaded `AImageDecoder` API.
#[derive(Clone, Copy, Debug)]
pub struct NdkDecoder {
    api: &'static Api,
    capabilities: Capabilities,
}

impl NdkDecoder {
    /// Detects the runtime API and returns its supported formats.
    #[must_use]
    pub fn detect() -> Option<Self> {
        let api = api()?;
        Some(Self {
            api,
            capabilities: claimed(api),
        })
    }
}

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
        request.check(&header)?;

        handle.set_rgba8888(format)?;
        let target = request.effective_size(header.natural_size);
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

    fn header(&self, format: ImageFormat, bytes: &[u8]) -> Result<ImageHeader, ImageError> {
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
        let _ = unsafe { CStr::from_ptr(mime) };

        let declared = u32::try_from(bitmap_format).ok().map(AndroidBitmapFormat);
        if declared
            .is_none_or(|declared| declared == AndroidBitmapFormat::ANDROID_BITMAP_FORMAT_NONE)
        {
            return Err(ImageError::decode(
                format,
                format!("header declares no usable bitmap format ({bitmap_format})"),
            ));
        }

        let natural_size = PixelSize {
            width: positive(format, "width", width)?,
            height: positive(format, "height", height)?,
        };

        Ok(ImageHeader {
            format,
            natural_size,
            has_alpha: u32::try_from(alpha_flags).is_ok_and(|flags| {
                flags & ndk_sys::ANDROID_BITMAP_FLAGS_ALPHA_MASK
                    != ndk_sys::ANDROID_BITMAP_FLAGS_ALPHA_OPAQUE
            }),
            animated: crate::image_decoders::animation::container_declares_animation(format, bytes),
        })
    }

    fn set_rgba8888(&self, format: ImageFormat) -> Result<(), ImageError> {
        let rgba8888 = i32::try_from(AndroidBitmapFormat::ANDROID_BITMAP_FORMAT_RGBA_8888.0)
            .map_err(|_| {
                ImageError::decode(format, "RGBA_8888 does not fit the NDK's format field")
            })?;
        let result = unsafe { (self.api.set_bitmap_format)(self.raw, rgba8888) };
        check(format, "AImageDecoder_setAndroidBitmapFormat", result)
    }

    fn set_target_size(&self, size: PixelSize) -> bool {
        let (Ok(width), Ok(height)) = (i32::try_from(size.width), i32::try_from(size.height))
        else {
            return false;
        };
        let result = unsafe { (self.api.set_target_size)(self.raw, width, height) };
        result == ndk_sys::ANDROID_IMAGE_DECODER_SUCCESS
    }

    fn decode_pixels(&self, format: ImageFormat, size: PixelSize) -> Result<Vec<u8>, ImageError> {
        let too_large =
            || ImageError::too_large(size.width, size.height, "decode buffer overflows usize");
        let width = usize::try_from(size.width).map_err(|_| too_large())?;
        let height = usize::try_from(size.height).map_err(|_| too_large())?;
        let row_bytes = width.checked_mul(4).ok_or_else(too_large)?;

        let stride = unsafe { (self.api.minimum_stride)(self.raw) };
        if stride < row_bytes {
            return Err(ImageError::decode(
                format,
                format!("minimum stride {stride} is below {row_bytes} bytes of RGBA8 pixels"),
            ));
        }
        let capacity = stride.checked_mul(height).ok_or_else(too_large)?;
        let mut buffer = vec![0u8; capacity];

        let result = unsafe {
            (self.api.decode_image)(self.raw, buffer.as_mut_ptr().cast(), stride, capacity)
        };
        check(format, "AImageDecoder_decodeImage", result)?;

        Ok(compact_rows(buffer, stride, row_bytes, height))
    }
}

impl Drop for Handle<'_> {
    fn drop(&mut self) {
        unsafe { (self.api.delete)(self.raw) };
    }
}

fn positive(format: ImageFormat, axis: &str, value: i32) -> Result<u32, ImageError> {
    u32::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| ImageError::decode(format, format!("header reports {axis} {value}")))
}

fn compact_rows(mut buffer: Vec<u8>, stride: usize, row_bytes: usize, height: usize) -> Vec<u8> {
    let Some(packed) = row_bytes.checked_mul(height) else {
        return buffer;
    };
    if stride < row_bytes || buffer.len() < stride.saturating_mul(height) {
        return buffer;
    }
    if stride != row_bytes {
        for row in 1..height {
            let from = row * stride;
            buffer.copy_within(from..from + row_bytes, row * row_bytes);
        }
    }
    buffer.truncate(packed);
    buffer
}

fn check(format: ImageFormat, call: &str, result: c_int) -> Result<(), ImageError> {
    let Some(name) = result_name(result) else {
        return Ok(());
    };
    Err(ImageError::decode(
        format,
        format!("{call} failed: {name} ({result})"),
    ))
}

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

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(load).as_ref()
}

fn load() -> Option<Api> {
    let library = unsafe { libc::dlopen(c"libjnigraphics.so".as_ptr(), libc::RTLD_NOW) };
    if library.is_null() {
        return None;
    }

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

unsafe fn symbol<T: Copy>(library: *mut c_void, name: &CStr) -> Option<T> {
    debug_assert_eq!(
        size_of::<T>(),
        size_of::<*mut c_void>(),
        "T must be a bare function pointer"
    );
    let address = unsafe { libc::dlsym(library, name.as_ptr()) };
    if address.is_null() {
        return None;
    }
    Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&address) })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{compact_rows, result_name};

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
        let short = vec![7u8; 10];
        assert_eq!(compact_rows(short.clone(), 24, 8, 3), short);
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
