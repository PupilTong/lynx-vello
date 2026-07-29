//! The Windows Imaging Component backend.
//!
//! `WIC` is the system's own still-image codec framework: PNG and JPEG are inbox
//! components present on every supported Windows version, while `WebP` ships as
//! a Microsoft Store extension (`Microsoft.WebpImageExtension`) and is therefore
//! genuinely absent on a large share of machines. That is what makes the probe
//! here load-bearing rather than decorative — see [`WicDecoder::detect`].
//!
//! The whole decode runs through `IWICBitmapSource`, which is a *pull* pipeline:
//! constructing the decoder, the frame and the scaler parses headers only, and
//! pixels are produced when `CopyPixels` finally asks for them. That is why
//! [`Decoder::probe`] can read a natural size without decoding anything, and why
//! inserting an `IWICBitmapScaler` ahead of the format converter is a real
//! decode-time downsample rather than a full-size decode followed by a resize.
#![allow(unsafe_code)]
// Every `unsafe` block below is one of exactly three unavoidable categories, and
// nothing else:
//
// 1. Calling a COM interface method. `windows-rs` marks every one of them `unsafe` because a raw
//    vtable call cannot check that the interface pointer is live or that its out-parameters are
//    writable; ours come from `windows`' own constructors and are dropped before the function
//    returns.
// 2. Apartment lifecycle — `CoInitializeEx` / `CoUninitialize`, whose safety condition is "call
//    them in pairs, on the same thread".
// 3. `IWICStream::InitializeFromMemory`, which retains the caller's buffer pointer instead of
//    copying. The lifetime that fact demands is expressed in the type system by [`FrameSource`]'s
//    `'bytes` parameter.
//
// There is no raw pointer arithmetic, no transmute, and no `Send`/`Sync`
// assertion anywhere in this file.

use std::marker::PhantomData;
use std::ptr;
use std::sync::OnceLock;

use windows::Win32::Foundation::{CO_E_NOTINITIALIZED, RPC_E_CHANGED_MODE};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatJpeg, GUID_ContainerFormatPng,
    GUID_ContainerFormatWebp, GUID_WICPixelFormat8bppAlpha, GUID_WICPixelFormat16bppBGR555,
    GUID_WICPixelFormat16bppBGR565, GUID_WICPixelFormat16bppBGRA5551, GUID_WICPixelFormat16bppGray,
    GUID_WICPixelFormat24bppBGR, GUID_WICPixelFormat24bppRGB, GUID_WICPixelFormat32bppBGR,
    GUID_WICPixelFormat32bppBGRA, GUID_WICPixelFormat32bppCMYK, GUID_WICPixelFormat32bppPBGRA,
    GUID_WICPixelFormat32bppPRGBA, GUID_WICPixelFormat32bppRGB, GUID_WICPixelFormat32bppRGBA,
    GUID_WICPixelFormat32bppRGBA1010102, GUID_WICPixelFormat40bppCMYKAlpha,
    GUID_WICPixelFormat48bppBGR, GUID_WICPixelFormat48bppRGB, GUID_WICPixelFormat64bppBGRA,
    GUID_WICPixelFormat64bppCMYK, GUID_WICPixelFormat64bppPBGRA, GUID_WICPixelFormat64bppPRGBA,
    GUID_WICPixelFormat64bppRGB, GUID_WICPixelFormat64bppRGBA, GUID_WICPixelFormat64bppRGBAHalf,
    GUID_WICPixelFormat80bppCMYKAlpha, GUID_WICPixelFormat96bppRGBFloat,
    GUID_WICPixelFormat128bppRGBAFloat, GUID_WICPixelFormatBlackWhite, IWICBitmapDecoder,
    IWICBitmapFrameDecode, IWICBitmapSource, IWICImagingFactory, IWICStream,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::{Error as ComError, GUID, HRESULT, Interface, Result as ComResult};

use crate::backend::resample;
use crate::decode::{DecodeRequest, DecodeResponse, Decoder, ImageHeader, PixelSize};
use crate::error::ImageError;
use crate::format::ImageFormat;
use crate::orientation::{self, Orientation};
use crate::pixels::{AlphaType, DecodedImage};
use crate::registry::{Acceleration, Capabilities, probe_once};

/// PNG, JPEG and (when the Store extension is installed) `WebP` through the
/// Windows Imaging Component.
///
/// Reports [`Acceleration::PlatformSoftware`] and never
/// [`Acceleration::DedicatedHardware`]: `WIC` is a CPU codec framework with no
/// acceleration query of any kind, and its only GPU-adjacent surface —
/// `IWICPlanarBitmapSourceTransform`, which hands YCbCr planes to Direct2D — is
/// a way to *avoid* a CPU colour conversion in a Direct2D pipeline, not a route
/// to a decode ASIC. Claiming hardware here would be a claim this backend
/// cannot substantiate.
#[derive(Clone, Copy, Debug)]
pub struct WicDecoder {
    /// Resolved once by the process-wide probe and carried by value, so
    /// [`Decoder::capabilities`] is a field read rather than a lock.
    capabilities: Capabilities,
}

/// Memoised result of the per-format `CreateDecoder` probe.
static PROBE: OnceLock<Capabilities> = OnceLock::new();

impl WicDecoder {
    /// Probes `WIC` and returns a backend only if it decodes at least one
    /// format this crate supports.
    ///
    /// `None` is an ordinary outcome, not a failure: the apartment may refuse to
    /// initialise, the factory may not be creatable in a stripped-down
    /// container image, and Windows Nano Server ships without the imaging
    /// components at all. The software backend covers every format
    /// unconditionally, so this backend is only ever an upgrade.
    #[must_use]
    pub fn detect() -> Option<Self> {
        let capabilities = probe_once(&PROBE, probe_capabilities);
        if capabilities.supported_formats().is_empty() {
            return None;
        }
        Some(Self { capabilities })
    }
}

impl Decoder for WicDecoder {
    fn name(&self) -> &'static str {
        "wic"
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    fn probe(&self, format: ImageFormat, bytes: &[u8]) -> Result<ImageHeader, ImageError> {
        let factory = imaging_factory().map_err(|error| wic_error(format, "factory", &error))?;
        let source = FrameSource::open(&factory, bytes, format)?;
        read_header(&source, format, bytes)
    }

    fn decode(
        &self,
        format: ImageFormat,
        bytes: &[u8],
        request: &DecodeRequest,
    ) -> Result<DecodeResponse, ImageError> {
        let factory = imaging_factory().map_err(|error| wic_error(format, "factory", &error))?;
        // One chain for both the header and the pixels: `WIC` is lazy, so
        // reading the size off this frame costs nothing extra and re-opening the
        // stream just to probe would parse the container twice.
        let source = FrameSource::open(&factory, bytes, format)?;
        let header = read_header(&source, format, bytes)?;
        request.check(&header)?;

        let orientation = orientation_of(format, bytes);
        let target = request.effective_size(header.natural_size);
        // `natural_size` is oriented, so the target is stated in oriented space;
        // `WIC` scales in the space the file is *stored* in. `apply_to_size` is
        // its own inverse for the only thing that differs — the axis swap — so
        // applying it to the target maps it back.
        let stored_target = orientation.apply_to_size(target.width, target.height);
        let stored =
            orientation.apply_to_size(header.natural_size.width, header.natural_size.height);

        let pixels = copy_converted(&factory, &source, stored, stored_target, format)?;
        let (pixels, width, height) = orientation.apply(pixels, stored_target.0, stored_target.1);

        // A safety net rather than a second scaling pass: `resample` returns the
        // buffer untouched when the sizes already agree, which they do unless
        // `WIC` honoured the scaler request only approximately.
        let (pixels, width, height) = if (width, height) == (target.width, target.height) {
            (pixels, width, height)
        } else {
            let scaled = resample(
                pixels,
                (width, height),
                (target.width, target.height),
                AlphaType::Straight,
                format,
            )?;
            (scaled, target.width, target.height)
        };

        Ok(DecodeResponse {
            // `GUID_WICPixelFormat32bppRGBA` is straight alpha. The
            // premultiplied sibling is deliberately not used: vello's fine
            // shader premultiplies per texel anyway, so converting here would
            // only discard precision in near-transparent texels.
            image: DecodedImage::from_rgba8(width, height, AlphaType::Straight, pixels, format)?,
            header,
            acceleration: Acceleration::PlatformSoftware,
            backend: "wic",
        })
    }
}

// ------------------------------------------------------------ capability probe

/// Asks `WIC` for a decoder per container, which is the only honest way to
/// answer the `WebP` question.
///
/// `CreateDecoder` resolves a registered codec by container GUID without
/// touching a byte of image data, and answers `WINCODEC_ERR_COMPONENTNOTFOUND`
/// when none is registered — which is exactly what a machine without the
/// `WebP` Store extension reports. Any other failure is treated the same way:
/// a codec this crate cannot construct is a codec it cannot use.
fn probe_capabilities() -> Capabilities {
    let Ok(factory) = imaging_factory() else {
        return Capabilities::none();
    };
    let mut capabilities = Capabilities::none();
    for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
        if codec_present(&factory, container_guid(format)) {
            capabilities = capabilities.with(format, Acceleration::PlatformSoftware);
        }
    }
    capabilities
}

fn codec_present(factory: &IWICImagingFactory, container: &GUID) -> bool {
    // SAFETY: COM method call on a live factory. `container` points at a
    // `'static` GUID constant, and a null vendor GUID asks for whichever
    // registered codec `WIC` prefers, which is what `CreateDecoder`'s
    // documentation specifies for "no vendor preference". The returned decoder
    // is dropped immediately; nothing escapes.
    let created = unsafe { factory.CreateDecoder(container, ptr::null()) };
    created.is_ok()
}

/// The `WIC` container GUID for each format this crate sniffs.
fn container_guid(format: ImageFormat) -> &'static GUID {
    match format {
        ImageFormat::Png => &GUID_ContainerFormatPng,
        ImageFormat::Jpeg => &GUID_ContainerFormatJpeg,
        ImageFormat::WebP => &GUID_ContainerFormatWebp,
    }
}

// ------------------------------------------------------------- COM apartment

thread_local! {
    /// One apartment entry per thread, released when the thread exits.
    ///
    /// A `thread_local!` rather than a process-wide `OnceLock`: apartment
    /// membership *is* thread state, and decodes run on tokio's blocking pool,
    /// whose threads are created and retired outside this crate's control. A
    /// once-per-process guard would leave every blocking thread after the first
    /// with no apartment at all.
    static APARTMENT: ComApartment = ComApartment::enter();
}

/// The result of this thread's `CoInitializeEx`, plus whether that call owes a
/// matching `CoUninitialize`.
#[derive(Debug)]
struct ComApartment {
    entered: HRESULT,
    /// `S_OK` and `S_FALSE` both incremented the apartment's reference count and
    /// must be released; `RPC_E_CHANGED_MODE` did not, and releasing it would
    /// tear down an apartment this crate never joined.
    owns_reference: bool,
}

impl ComApartment {
    fn enter() -> Self {
        // SAFETY: apartment lifecycle. `CoInitializeEx` takes no borrowed data —
        // the reserved parameter is required to be null — and the matching
        // `CoUninitialize` runs in `Drop` on this same thread.
        let entered = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self {
            entered,
            owns_reference: entered.is_ok(),
        }
    }

    /// Whether COM is usable on this thread. An apartment the host already
    /// initialised in single-threaded mode is perfectly usable — `WIC` objects
    /// are created and consumed inside one call, so they never cross the
    /// apartment boundary that mode would police.
    fn usable(&self) -> bool {
        self.entered.is_ok() || self.entered == RPC_E_CHANGED_MODE
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.owns_reference {
            // SAFETY: apartment lifecycle. Paired one-to-one with the successful
            // `CoInitializeEx` in `enter`, on the thread that made it.
            unsafe { CoUninitialize() };
        }
    }
}

fn ensure_apartment() -> ComResult<()> {
    // `try_with` rather than `with`: a thread already running its TLS
    // destructors must produce an error, never a panic.
    APARTMENT
        .try_with(|apartment| {
            if apartment.usable() {
                Ok(())
            } else {
                Err(ComError::from_hresult(apartment.entered))
            }
        })
        .unwrap_or_else(|_| Err(ComError::from_hresult(CO_E_NOTINITIALIZED)))
}

fn imaging_factory() -> ComResult<IWICImagingFactory> {
    ensure_apartment()?;
    // SAFETY: COM method call. `CLSID_WICImagingFactory` is a `'static`
    // constant, aggregation is declined by passing no outer unknown, and the
    // interface the returned pointer is queried for is inferred from the
    // binding's own `IWICImagingFactory::IID`.
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

// ------------------------------------------------------------- decode chain

/// The `WIC` objects for one image, tied to the buffer they read through.
///
/// The `'bytes` parameter is the load-bearing part.
/// `IWICStream::InitializeFromMemory` does **not** copy: the stream keeps the
/// caller's pointer and every read — including the ones `CopyPixels` triggers
/// long after initialisation — goes back to that memory. Borrowing the slice for
/// the struct's whole life makes "the buffer outlives the decode" a compile-time
/// guarantee instead of a comment nobody re-reads.
///
/// Field order is the drop order: the frame releases before the decoder, which
/// releases before the stream.
#[derive(Debug)]
struct FrameSource<'bytes> {
    frame: IWICBitmapFrameDecode,
    decoder: IWICBitmapDecoder,
    /// Held, never called: the decoder holds its own reference, but keeping ours
    /// makes the stream's lifetime visible next to the borrow that justifies it.
    _stream: IWICStream,
    bytes: PhantomData<&'bytes [u8]>,
}

impl<'bytes> FrameSource<'bytes> {
    fn open(
        factory: &IWICImagingFactory,
        bytes: &'bytes [u8],
        format: ImageFormat,
    ) -> Result<Self, ImageError> {
        // SAFETY: COM method calls on live objects, plus the borrowed-buffer
        // case. `InitializeFromMemory` retains `bytes`' pointer rather than
        // copying it; `'bytes` outlives every object constructed here, and all
        // of them are dropped when the returned value is. The binding takes a
        // shared slice and casts the constness away because `WIC` types the
        // parameter as a writable `BYTE*`, but a decode source is only ever
        // read.
        unsafe {
            let stream = factory
                .CreateStream()
                .map_err(|error| wic_error(format, "CreateStream", &error))?;
            stream
                .InitializeFromMemory(bytes)
                .map_err(|error| wic_error(format, "InitializeFromMemory", &error))?;
            // On-demand rather than on-load metadata caching: nothing here reads
            // metadata through `WIC`, and on-load would walk every block up
            // front during what is meant to be a header probe.
            let decoder = factory
                .CreateDecoderFromStream(&stream, ptr::null(), WICDecodeMetadataCacheOnDemand)
                .map_err(|error| wic_error(format, "CreateDecoderFromStream", &error))?;
            // Frame 0. An animated `WebP`'s remaining frames are reported in the
            // header and otherwise ignored, matching the software backend.
            let frame = decoder
                .GetFrame(0)
                .map_err(|error| wic_error(format, "GetFrame(0)", &error))?;
            Ok(Self {
                frame,
                decoder,
                _stream: stream,
                bytes: PhantomData,
            })
        }
    }

    /// The frame's stored size, before EXIF orientation.
    fn stored_size(&self, format: ImageFormat) -> Result<(u32, u32), ImageError> {
        let mut width = 0u32;
        let mut height = 0u32;
        // SAFETY: COM method call. Both out-parameters point at live, writable
        // locals of exactly the `u32` the vtable expects.
        unsafe { self.frame.GetSize(&raw mut width, &raw mut height) }
            .map_err(|error| wic_error(format, "GetSize", &error))?;
        Ok((width, height))
    }
}

/// Header-only: `GetSize`, `GetPixelFormat` and `GetFrameCount` are all answered
/// from the container's own metadata, and none of them causes a single pixel to
/// be produced.
fn read_header(
    source: &FrameSource<'_>,
    format: ImageFormat,
    bytes: &[u8],
) -> Result<ImageHeader, ImageError> {
    let (width, height) = source.stored_size(format)?;
    // SAFETY: COM method calls on live objects; both return by value into
    // stack slots the binding owns.
    let pixel_format = unsafe { source.frame.GetPixelFormat() };
    let frame_count = unsafe { source.decoder.GetFrameCount() };

    let (width, height) = orientation_of(format, bytes).apply_to_size(width, height);
    Ok(ImageHeader {
        format,
        natural_size: PixelSize { width, height },
        has_alpha: pixel_format
            .map_or_else(|_| default_alpha(format), |guid| has_alpha(guid, format)),
        // A frame count is the only animation signal `WIC` offers; a codec that
        // declines to report one is treated as a still image.
        animated: frame_count.is_ok_and(|count| count > 1),
    })
}

/// Runs the scale-then-convert tail of the pipeline and pulls the pixels out.
///
/// Split out of `decode` so the pull chain — whose object lifetimes matter — is
/// readable in one screen.
fn copy_converted(
    factory: &IWICImagingFactory,
    source: &FrameSource<'_>,
    stored: (u32, u32),
    stored_target: (u32, u32),
    format: ImageFormat,
) -> Result<Vec<u8>, ImageError> {
    // SAFETY: COM method calls on live objects. Every GUID argument points at a
    // `'static` constant; `None` for the palette is what a non-indexed
    // destination format takes. The scaler and converter borrow their upstream
    // source through COM reference counting, so `source` outliving this call is
    // sufficient — and it does, being a caller-owned borrow.
    unsafe {
        let scaled: IWICBitmapSource = if stored_target == stored {
            source
                .frame
                .cast()
                .map_err(|error| wic_error(format, "frame as IWICBitmapSource", &error))?
        } else {
            let scaler = factory
                .CreateBitmapScaler()
                .map_err(|error| wic_error(format, "CreateBitmapScaler", &error))?;
            // Fant is `WIC`'s area-averaging downsample — the only one of its
            // modes that does not alias when shrinking by more than 2x, which is
            // the common case for a thumbnail-sized target.
            //
            // Recorded difference from the software backend: this scales the
            // frame's own straight-alpha pixels, where `backend::resample`
            // weights by alpha first. Fully transparent colour values can
            // therefore bleed a faint halo into a downsampled edge here. Fixing
            // it would mean converting to a premultiplied format, scaling, and
            // converting back — two extra full-size passes to undo the memory
            // saving that decode-time scaling exists for.
            scaler
                .Initialize(
                    &source.frame,
                    stored_target.0,
                    stored_target.1,
                    WICBitmapInterpolationModeFant,
                )
                .map_err(|error| wic_error(format, "IWICBitmapScaler::Initialize", &error))?;
            scaler
                .cast()
                .map_err(|error| wic_error(format, "scaler as IWICBitmapSource", &error))?
        };

        let converter = factory
            .CreateFormatConverter()
            .map_err(|error| wic_error(format, "CreateFormatConverter", &error))?;
        converter
            .Initialize(
                &scaled,
                &GUID_WICPixelFormat32bppRGBA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(|error| wic_error(format, "IWICFormatConverter::Initialize", &error))?;

        copy_pixels(&converter, stored_target, format)
    }
}

/// Drains the pull chain into a tightly packed RGBA8 buffer.
fn copy_pixels(
    source: &IWICBitmapSource,
    (width, height): (u32, u32),
    format: ImageFormat,
) -> Result<Vec<u8>, ImageError> {
    let stride = width
        .checked_mul(4)
        .ok_or_else(|| ImageError::too_large(width, height, "stride overflows u32"))?;
    let length = crate::pixels::expected_byte_len(width, height).ok_or_else(|| {
        ImageError::too_large(width, height, "width * height * 4 overflows usize")
    })?;
    let mut buffer = vec![0u8; length];
    // SAFETY: COM method call. A null rectangle asks for the whole image, which
    // is what `stride` and `buffer` were sized for; the binding passes the
    // buffer's own length to the vtable, so `WIC` cannot be told to write past
    // it.
    unsafe { source.CopyPixels(ptr::null(), stride, &mut buffer) }
        .map_err(|error| wic_error(format, "CopyPixels", &error))?;
    Ok(buffer)
}

// -------------------------------------------------------------- orientation

/// The EXIF transform to apply, read from the file rather than from `WIC`.
///
/// `WIC` does expose the tag through `IWICMetadataQueryReader`, but reading it
/// there would need `PROPVARIANT`, i.e. two further `windows` crate features,
/// and would answer a *different* question: this crate's own
/// [`orientation::jpeg_orientation`] is what the software backend consults, so
/// consulting it here is the only way to guarantee the two backends report the
/// same `natural_size` and produce the same pixels for the same file. `WIC`
/// itself never auto-orients, so there is no double application to avoid.
fn orientation_of(format: ImageFormat, bytes: &[u8]) -> Orientation {
    if format == ImageFormat::Jpeg {
        orientation::jpeg_orientation(bytes)
    } else {
        Orientation::Identity
    }
}

// ------------------------------------------------------------------- alpha

/// `WIC` pixel formats that carry an alpha channel.
const WITH_ALPHA: [GUID; 15] = [
    GUID_WICPixelFormat32bppBGRA,
    GUID_WICPixelFormat32bppRGBA,
    GUID_WICPixelFormat32bppPBGRA,
    GUID_WICPixelFormat32bppPRGBA,
    GUID_WICPixelFormat32bppRGBA1010102,
    GUID_WICPixelFormat64bppBGRA,
    GUID_WICPixelFormat64bppRGBA,
    GUID_WICPixelFormat64bppPBGRA,
    GUID_WICPixelFormat64bppPRGBA,
    GUID_WICPixelFormat64bppRGBAHalf,
    GUID_WICPixelFormat128bppRGBAFloat,
    GUID_WICPixelFormat16bppBGRA5551,
    GUID_WICPixelFormat8bppAlpha,
    GUID_WICPixelFormat40bppCMYKAlpha,
    GUID_WICPixelFormat80bppCMYKAlpha,
];

/// `WIC` pixel formats that are unambiguously opaque.
const WITHOUT_ALPHA: [GUID; 14] = [
    GUID_WICPixelFormat24bppBGR,
    GUID_WICPixelFormat24bppRGB,
    GUID_WICPixelFormat32bppBGR,
    GUID_WICPixelFormat32bppRGB,
    GUID_WICPixelFormat48bppBGR,
    GUID_WICPixelFormat48bppRGB,
    GUID_WICPixelFormat64bppRGB,
    GUID_WICPixelFormat96bppRGBFloat,
    GUID_WICPixelFormat16bppBGR555,
    GUID_WICPixelFormat16bppBGR565,
    GUID_WICPixelFormat16bppGray,
    GUID_WICPixelFormatBlackWhite,
    GUID_WICPixelFormat32bppCMYK,
    GUID_WICPixelFormat64bppCMYK,
];

/// Whether a frame in `pixel_format` can carry transparency.
///
/// Indexed formats are deliberately in neither table: a PNG with a `tRNS` chunk
/// reaches `WIC` as `8bppIndexed` with alpha hiding in the palette, so the
/// pixel format alone cannot answer the question and the container's own default
/// is the better guess.
fn has_alpha(pixel_format: GUID, format: ImageFormat) -> bool {
    if WITH_ALPHA.contains(&pixel_format) {
        return true;
    }
    if WITHOUT_ALPHA.contains(&pixel_format) {
        return false;
    }
    default_alpha(format)
}

/// The per-container assumption for a pixel format this backend does not
/// recognise. PNG and `WebP` both routinely carry alpha; baseline and
/// progressive JPEG cannot. `has_alpha` is a hint layout never depends on, so
/// over-reporting it costs nothing while under-reporting it would lose a
/// transparent edge.
const fn default_alpha(format: ImageFormat) -> bool {
    !matches!(format, ImageFormat::Jpeg)
}

// ------------------------------------------------------------------- errors

fn wic_error(format: ImageFormat, context: &str, error: &ComError) -> ImageError {
    ImageError::decode(format, format!("WIC {context}: {error}"))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use windows::Win32::Graphics::Imaging::{
        GUID_ContainerFormatJpeg, GUID_ContainerFormatPng, GUID_ContainerFormatWebp,
        GUID_WICPixelFormat8bppIndexed, GUID_WICPixelFormat24bppBGR, GUID_WICPixelFormat32bppBGRA,
        GUID_WICPixelFormat32bppPRGBA, GUID_WICPixelFormatDontCare,
    };

    use super::{container_guid, default_alpha, has_alpha};
    use crate::format::ImageFormat;
    use crate::orientation::Orientation;

    #[test]
    fn every_supported_format_maps_to_its_container_guid() {
        assert_eq!(container_guid(ImageFormat::Png), &GUID_ContainerFormatPng);
        assert_eq!(container_guid(ImageFormat::Jpeg), &GUID_ContainerFormatJpeg);
        assert_eq!(container_guid(ImageFormat::WebP), &GUID_ContainerFormatWebp);
        // Distinct GUIDs, so the probe cannot silently claim one codec for
        // another.
        assert_ne!(GUID_ContainerFormatPng, GUID_ContainerFormatWebp);
    }

    #[test]
    fn known_pixel_formats_decide_alpha_regardless_of_container() {
        // A JPEG frame reported as BGRA is still opaque in practice, but the
        // pixel format is the more specific answer and wins.
        assert!(has_alpha(GUID_WICPixelFormat32bppBGRA, ImageFormat::Jpeg));
        assert!(has_alpha(GUID_WICPixelFormat32bppPRGBA, ImageFormat::WebP));
        assert!(!has_alpha(GUID_WICPixelFormat24bppBGR, ImageFormat::Png));
    }

    #[test]
    fn unrecognised_pixel_formats_fall_back_to_the_container_default() {
        // Indexed is the case this exists for: PNG's `tRNS` alpha lives in the
        // palette, out of the pixel format's sight.
        assert!(has_alpha(GUID_WICPixelFormat8bppIndexed, ImageFormat::Png));
        assert!(has_alpha(GUID_WICPixelFormat8bppIndexed, ImageFormat::WebP));
        assert!(!has_alpha(
            GUID_WICPixelFormat8bppIndexed,
            ImageFormat::Jpeg
        ));
        assert!(has_alpha(GUID_WICPixelFormatDontCare, ImageFormat::Png));

        assert!(default_alpha(ImageFormat::Png));
        assert!(default_alpha(ImageFormat::WebP));
        assert!(!default_alpha(ImageFormat::Jpeg));
    }

    #[test]
    fn an_oriented_target_maps_back_into_the_space_wic_scales_in() {
        // The decode path states its target in oriented space and asks `WIC` for
        // a stored-space size; `apply_to_size` is its own inverse, which is what
        // makes one call enough in each direction.
        let stored = (400u32, 200u32);
        for orientation in [
            Orientation::Identity,
            Orientation::Rotate180,
            Orientation::Rotate90,
            Orientation::Transverse,
        ] {
            let natural = orientation.apply_to_size(stored.0, stored.1);
            let target = (natural.0 / 2, natural.1 / 2);
            let stored_target = orientation.apply_to_size(target.0, target.1);
            assert_eq!(
                orientation.apply_to_size(stored_target.0, stored_target.1),
                target,
                "{orientation:?} must round-trip the target size"
            );
            assert_eq!(stored_target, (stored.0 / 2, stored.1 / 2));
        }
    }
}
