//! macOS: ImageIO and CoreGraphics, the decoder every Apple image view uses.
//!
//! `CGImageSourceCreateThumbnailAtIndex` with a maximum pixel size decodes a
//! downsampled image directly — for JPEG, at a reduced DCT scale — which is
//! what makes a large photo shown small cost its shown size. The result is
//! drawn once into an RGBA bitmap context, which is how CoreGraphics hands
//! pixels out; that context is premultiplied, and the bitmap says so.
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

/// A CoreFoundation object released on drop.
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

/// Reads a CFNumber property out of an ImageIO property dictionary.
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

pub(crate) fn decode(
    bytes: &[u8],
    header: Option<ImageHeader>,
    max: (u32, u32),
) -> Result<Bitmap, DecodeError> {
    let length = CFIndex::try_from(bytes.len()).map_err(|_| {
        DecodeError::Malformed("the image is larger than CFData can hold".to_owned())
    })?;
    // SAFETY: `CFDataCreate` copies `length` bytes readable at the pointer.
    let data = Owned(unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), length) });
    if data.0.is_null() {
        return Err(DecodeError::Malformed("CFDataCreate failed".to_owned()));
    }
    // SAFETY: the data object is live; a null source means ImageIO does not
    // recognise the container at all.
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

    // Intrinsic size and orientation from the properties, which ImageIO
    // reads from the header without decoding.
    // SAFETY: the source is live; a null dictionary is handled.
    let properties =
        Owned(unsafe { CGImageSourceCopyPropertiesAtIndex(source.0, 0, std::ptr::null()) });
    let (mut source_width, mut source_height) = if properties.0.is_null() {
        header.map_or((0, 0), |header| (header.width, header.height))
    } else {
        // SAFETY: reading framework-exported constant keys.
        let (width, height, orientation) = unsafe {
            (
                number_property(properties.0, kCGImagePropertyPixelWidth),
                number_property(properties.0, kCGImagePropertyPixelHeight),
                number_property(properties.0, kCGImagePropertyOrientation).unwrap_or(1),
            )
        };
        let width = width
            .and_then(|width| u32::try_from(width).ok())
            .unwrap_or(0);
        let height = height
            .and_then(|height| u32::try_from(height).ok())
            .unwrap_or(0);
        if (5..=8).contains(&orientation) {
            (height, width)
        } else {
            (width, height)
        }
    };
    drop(properties);
    if source_width == 0 || source_height == 0 {
        if let Some(header) = header {
            (source_width, source_height) = (header.width, header.height);
        }
    }

    let max_pixel = if source_width == 0 || source_height == 0 {
        max.0.max(max.1)
    } else {
        let (width, height) = target_size(source_width, source_height, max);
        width.max(height)
    };
    let max_pixel = i32::try_from(max_pixel).unwrap_or(i32::MAX);
    // SAFETY: `CFNumberCreate` copies the 32-bit integer at the pointer.
    let max_pixel_number = Owned(unsafe {
        CFNumberCreate(
            std::ptr::null(),
            K_CF_NUMBER_SINT32_TYPE,
            (&raw const max_pixel).cast::<c_void>(),
        )
    });
    // SAFETY: the keys and `kCFBooleanTrue` are framework-exported constants,
    // the arrays are three live values each, and the CFType callbacks retain
    // them for the dictionary's life.
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
    let image = unsafe { CGImageSourceCreateThumbnailAtIndex(source.0, 0, options.0) };
    if image.is_null() {
        return Err(DecodeError::Malformed(
            "ImageIO could not decode the image".to_owned(),
        ));
    }
    let image = CgImage(image);
    // SAFETY: accessors on a live image.
    let (width, height) = unsafe { (CGImageGetWidth(image.0), CGImageGetHeight(image.0)) };
    if width == 0 || height == 0 {
        return Err(DecodeError::Malformed(
            "the decoded image is empty".to_owned(),
        ));
    }
    let stride = width
        .checked_mul(4)
        .ok_or_else(|| DecodeError::Malformed("the image is too wide".to_owned()))?;
    let mut rgba =
        vec![
            0_u8;
            stride
                .checked_mul(height)
                .ok_or_else(|| DecodeError::Malformed("the image is too large".to_owned()))?
        ];
    // SAFETY: the bitmap context draws into `rgba`, which is exactly
    // `stride * height` bytes and outlives the context; the colour space and
    // context are released after the draw.
    unsafe {
        let space = CGColorSpaceCreateDeviceRGB();
        let context = CGBitmapContextCreate(
            rgba.as_mut_ptr().cast::<c_void>(),
            width,
            height,
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
                    width: width as f64,
                    height: height as f64,
                },
            },
            image.0,
        );
        CGContextRelease(context);
        CGColorSpaceRelease(space);
    }
    let (width, height) = (
        u32::try_from(width)
            .map_err(|_| DecodeError::Malformed("the image is too wide".to_owned()))?,
        u32::try_from(height)
            .map_err(|_| DecodeError::Malformed("the image is too tall".to_owned()))?,
    );
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

/// A `CGImage` released on drop.
struct CgImage(CFTypeRef);

impl Drop for CgImage {
    fn drop(&mut self) {
        // SAFETY: this struct owns the one reference `Create` returned.
        unsafe { CGImageRelease(self.0) };
    }
}
