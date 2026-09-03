//! Linux: the desktop's gdk-pixbuf, loaded at runtime.
//!
//! gdk-pixbuf is the image decoder GTK and every GNOME application share,
//! present on any desktop Linux and reached here through `dlopen` so the
//! crate neither links it at build time nor needs its headers — a host
//! without it reports [`DecodeError::Unavailable`] and draws no images
//! rather than failing to start. It decodes PNG, JPEG, GIF, BMP, TIFF and,
//! with the usual loader modules, WebP and SVG.
//!
//! Downsampling uses `gdk_pixbuf_loader_set_size`, which scales during load
//! when the target size is known before the first byte is written — which
//! is what the header probe is for. A container the probe cannot read is
//! decoded at full size and scaled afterwards.

#![expect(
    unsafe_code,
    reason = "a C library loaded at runtime is reached only through FFI; every call site \
              states the invariant it relies on"
)]

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::OnceLock;

use libloading::Library;

use super::{Bitmap, DecodeError, oriented_source_size, pack_rgba, target_size};
use crate::image_header::ImageHeader;

type Object = *mut c_void;

#[repr(C)]
struct GError {
    domain: u32,
    code: c_int,
    message: *mut c_char,
}

const GDK_INTERP_BILINEAR: c_int = 2;

struct Symbols {
    loader_new: unsafe extern "C" fn() -> Object,
    loader_set_size: unsafe extern "C" fn(Object, c_int, c_int),
    loader_write: unsafe extern "C" fn(Object, *const u8, usize, *mut *mut GError) -> c_int,
    loader_close: unsafe extern "C" fn(Object, *mut *mut GError) -> c_int,
    loader_get_pixbuf: unsafe extern "C" fn(Object) -> Object,
    apply_embedded_orientation: unsafe extern "C" fn(Object) -> Object,
    scale_simple: unsafe extern "C" fn(Object, c_int, c_int, c_int) -> Object,
    get_width: unsafe extern "C" fn(Object) -> c_int,
    get_height: unsafe extern "C" fn(Object) -> c_int,
    get_rowstride: unsafe extern "C" fn(Object) -> c_int,
    get_n_channels: unsafe extern "C" fn(Object) -> c_int,
    get_pixels: unsafe extern "C" fn(Object) -> *const u8,
    object_ref: unsafe extern "C" fn(Object) -> Object,
    object_unref: unsafe extern "C" fn(Object),
    error_free: unsafe extern "C" fn(*mut GError),
}

struct Loaded {
    symbols: Symbols,
    _libraries: Vec<Library>,
}

static LOADED: OnceLock<Result<Loaded, String>> = OnceLock::new();

// SAFETY: every function pointer refers into libraries this struct keeps
// mapped for its whole life, and gdk-pixbuf's API is callable from any
// thread as long as each object is used from one thread at a time, which
// `decode` guarantees by never sharing one.
unsafe impl Send for Loaded {}
// SAFETY: as above — the struct holds only immutable function pointers and
// the mapped libraries, which any thread may read concurrently.
unsafe impl Sync for Loaded {}

fn loaded() -> Result<&'static Loaded, DecodeError> {
    LOADED
        .get_or_init(|| load().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|message| DecodeError::Unavailable(message.clone()))
}

fn load() -> Result<Loaded, DecodeError> {
    let open = |candidates: &[&str]| -> Result<Library, DecodeError> {
        let mut failures = Vec::new();
        for candidate in candidates {
            // SAFETY: loading a system library by its well-known name; its
            // constructors are the ordinary GLib ones.
            match unsafe { Library::new(candidate) } {
                Ok(library) => return Ok(library),
                Err(error) => failures.push(format!("{candidate}: {error}")),
            }
        }
        Err(DecodeError::Unavailable(failures.join("; ")))
    };
    let pixbuf = open(&["libgdk_pixbuf-2.0.so.0", "libgdk_pixbuf-2.0.so"])?;
    let gobject = open(&["libgobject-2.0.so.0", "libgobject-2.0.so"])?;
    let glib = open(&["libglib-2.0.so.0", "libglib-2.0.so"])?;
    macro_rules! symbol {
        ($library:expr, $name:literal) => {{
            // SAFETY: the symbol is declared with the signature the library's
            // public header gives it, and copied into a struct that keeps
            // the library mapped for as long as the pointer can be called.
            let looked_up = unsafe { $library.get::<super::gdk_pixbuf::Raw<_>>($name) };
            let symbol = looked_up.map_err(|error| {
                DecodeError::Unavailable(format!(
                    "{} is missing: {error}",
                    String::from_utf8_lossy(&$name[..$name.len() - 1])
                ))
            })?;
            *symbol
        }};
    }
    let symbols = Symbols {
        loader_new: symbol!(pixbuf, b"gdk_pixbuf_loader_new\0"),
        loader_set_size: symbol!(pixbuf, b"gdk_pixbuf_loader_set_size\0"),
        loader_write: symbol!(pixbuf, b"gdk_pixbuf_loader_write\0"),
        loader_close: symbol!(pixbuf, b"gdk_pixbuf_loader_close\0"),
        loader_get_pixbuf: symbol!(pixbuf, b"gdk_pixbuf_loader_get_pixbuf\0"),
        apply_embedded_orientation: symbol!(pixbuf, b"gdk_pixbuf_apply_embedded_orientation\0"),
        scale_simple: symbol!(pixbuf, b"gdk_pixbuf_scale_simple\0"),
        get_width: symbol!(pixbuf, b"gdk_pixbuf_get_width\0"),
        get_height: symbol!(pixbuf, b"gdk_pixbuf_get_height\0"),
        get_rowstride: symbol!(pixbuf, b"gdk_pixbuf_get_rowstride\0"),
        get_n_channels: symbol!(pixbuf, b"gdk_pixbuf_get_n_channels\0"),
        get_pixels: symbol!(pixbuf, b"gdk_pixbuf_get_pixels\0"),
        object_ref: symbol!(gobject, b"g_object_ref\0"),
        object_unref: symbol!(gobject, b"g_object_unref\0"),
        error_free: symbol!(glib, b"g_error_free\0"),
    };
    Ok(Loaded {
        symbols,
        _libraries: vec![pixbuf, gobject, glib],
    })
}

/// A raw symbol type marker for the `symbol!` macro's generic request.
pub(crate) type Raw<T> = T;

pub(crate) fn available() -> Result<(), DecodeError> {
    loaded().map(|_| ())
}

/// An owned `GObject` reference, released on drop.
struct Ref<'a> {
    object: Object,
    symbols: &'a Symbols,
}

impl Drop for Ref<'_> {
    fn drop(&mut self) {
        if !self.object.is_null() {
            // SAFETY: this struct owns exactly one reference to a live object.
            unsafe { (self.symbols.object_unref)(self.object) };
        }
    }
}

/// Takes the message out of a `GError` and frees it.
fn take_error(symbols: &Symbols, error: *mut GError) -> String {
    if error.is_null() {
        return "unknown gdk-pixbuf failure".to_owned();
    }
    // SAFETY: a non-null `GError` written by gdk-pixbuf owns a NUL-terminated
    // message, and is freed exactly once here.
    unsafe {
        let message = if (*error).message.is_null() {
            "unknown gdk-pixbuf failure".to_owned()
        } else {
            CStr::from_ptr((*error).message)
                .to_string_lossy()
                .into_owned()
        };
        (symbols.error_free)(error);
        message
    }
}

pub(crate) fn decode(
    bytes: &[u8],
    header: Option<ImageHeader>,
    max: (u32, u32),
) -> Result<Bitmap, DecodeError> {
    let symbols = &loaded()?.symbols;
    let pixbuf = load_pixbuf(symbols, bytes, header, max)?;
    // SAFETY: `apply_embedded_orientation` returns a new reference — to a
    // rotated copy, or to the same pixbuf retained again — or null on
    // failure, in which case the unrotated image stands.
    let pixbuf = unsafe {
        let oriented = (symbols.apply_embedded_orientation)(pixbuf.object);
        if oriented.is_null() {
            pixbuf
        } else {
            Ref {
                object: oriented,
                symbols,
            }
        }
    };
    // SAFETY: plain accessors on a live pixbuf.
    let (decoded_width, decoded_height) = unsafe {
        (
            u32::try_from((symbols.get_width)(pixbuf.object)).unwrap_or(0),
            u32::try_from((symbols.get_height)(pixbuf.object)).unwrap_or(0),
        )
    };
    if decoded_width == 0 || decoded_height == 0 {
        return Err(DecodeError::Malformed(
            "the decoded image is empty".to_owned(),
        ));
    }
    let (source_width, source_height) = match header {
        Some(header) => oriented_source_size(
            (header.width, header.height),
            (decoded_width, decoded_height),
        ),
        None => (decoded_width, decoded_height),
    };
    let (width, height) = target_size(decoded_width, decoded_height, max);
    let pixbuf = if (width, height) == (decoded_width, decoded_height) {
        pixbuf
    } else {
        // SAFETY: `scale_simple` returns a new reference or null.
        let scaled = unsafe {
            (symbols.scale_simple)(
                pixbuf.object,
                c_int::try_from(width).unwrap_or(c_int::MAX),
                c_int::try_from(height).unwrap_or(c_int::MAX),
                GDK_INTERP_BILINEAR,
            )
        };
        if scaled.is_null() {
            return Err(DecodeError::Malformed(
                "the image could not be scaled".to_owned(),
            ));
        }
        Ref {
            object: scaled,
            symbols,
        }
    };
    let rgba = read_pixels(&pixbuf)?;
    Ok(Bitmap {
        width,
        height,
        source_width,
        source_height,
        premultiplied: false,
        rgba,
    })
}

/// Runs the loader over `bytes`, scaling during load when the header says
/// how large the image is, and hands back the pixbuf it produced.
fn load_pixbuf<'a>(
    symbols: &'a Symbols,
    bytes: &[u8],
    header: Option<ImageHeader>,
    max: (u32, u32),
) -> Result<Ref<'a>, DecodeError> {
    // SAFETY: a fresh loader object, owned by the `Ref` that releases it.
    let object = unsafe { (symbols.loader_new)() };
    let loader = Ref { object, symbols };
    if loader.object.is_null() {
        return Err(DecodeError::Unavailable(
            "gdk_pixbuf_loader_new returned null".to_owned(),
        ));
    }
    if let Some(header) = header {
        let (width, height) = target_size(header.width, header.height, max);
        if (width, height) != (header.width, header.height) {
            // SAFETY: set before any byte is written, which is the window the
            // API accepts a size in; the values are positive and in range.
            unsafe {
                (symbols.loader_set_size)(
                    loader.object,
                    c_int::try_from(width).unwrap_or(c_int::MAX),
                    c_int::try_from(height).unwrap_or(c_int::MAX),
                );
            }
        }
    }
    let mut error: *mut GError = std::ptr::null_mut();
    // SAFETY: `bytes` is readable for its whole length; the error out-pointer
    // is written only on failure and taken below.
    let written = unsafe {
        (symbols.loader_write)(loader.object, bytes.as_ptr(), bytes.len(), &raw mut error)
    };
    if written == 0 {
        let message = take_error(symbols, error);
        // SAFETY: closing a loader that failed a write is required before it
        // is released; its own error, if any, is dropped.
        unsafe { (symbols.loader_close)(loader.object, std::ptr::null_mut()) };
        return Err(DecodeError::Malformed(message));
    }
    // SAFETY: as for the write.
    let closed = unsafe { (symbols.loader_close)(loader.object, &raw mut error) };
    if closed == 0 {
        return Err(DecodeError::Malformed(take_error(symbols, error)));
    }
    // SAFETY: the loader is closed and complete; `get_pixbuf` returns a
    // borrowed reference the loader owns, so it is retained before the
    // loader can be released.
    let pixbuf = unsafe {
        let borrowed = (symbols.loader_get_pixbuf)(loader.object);
        if borrowed.is_null() {
            return Err(DecodeError::Malformed(
                "the loader produced no image".to_owned(),
            ));
        }
        Ref {
            object: (symbols.object_ref)(borrowed),
            symbols,
        }
    };
    drop(loader);
    Ok(pixbuf)
}

/// Packs a pixbuf's rows into tightly packed RGBA8.
fn read_pixels(pixbuf: &Ref<'_>) -> Result<Vec<u8>, DecodeError> {
    let symbols = pixbuf.symbols;
    // SAFETY: accessors on a live pixbuf; the pixel pointer is valid for
    // `rowstride * (height - 1) + width * channels` bytes, and the slice
    // taken covers exactly the rows and channels the accessors report.
    let packed = unsafe {
        let width = usize::try_from((symbols.get_width)(pixbuf.object)).unwrap_or(0);
        let height = usize::try_from((symbols.get_height)(pixbuf.object)).unwrap_or(0);
        let stride = usize::try_from((symbols.get_rowstride)(pixbuf.object)).unwrap_or(0);
        let channels = usize::try_from((symbols.get_n_channels)(pixbuf.object)).unwrap_or(0);
        let pixels = (symbols.get_pixels)(pixbuf.object);
        if width == 0 || height == 0 || pixels.is_null() || !(3..=4).contains(&channels) {
            return Err(DecodeError::Malformed(
                "the decoded image has no pixels".to_owned(),
            ));
        }
        let length = stride * (height - 1) + width * channels;
        pack_rgba(
            std::slice::from_raw_parts(pixels, length),
            width,
            height,
            stride,
            channels,
        )
    };
    packed.ok_or_else(|| DecodeError::Malformed("the decoded rows are inconsistent".to_owned()))
}
