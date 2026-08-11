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

use std::marker::PhantomData;
use std::ptr;
use std::sync::OnceLock;

use bobcat_core::image::{
    Acceleration, AlphaType, Capabilities, DecodeRequest, DecodeResponse, DecodedImage, Decoder,
    ImageError, ImageFormat, ImageHeader, PixelSize, expected_byte_len,
};
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

use crate::image_decoders::orientation::{self, Orientation};
use crate::image_decoders::resample;

/// Decoder backed by Windows Imaging Component codecs.
#[derive(Clone, Copy, Debug)]
pub struct WicDecoder {
    capabilities: Capabilities,
}

static PROBE: OnceLock<Capabilities> = OnceLock::new();

impl WicDecoder {
    /// Probes installed codecs and returns a usable decoder.
    #[must_use]
    pub fn detect() -> Option<Self> {
        let capabilities = *PROBE.get_or_init(probe_capabilities);
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
        let source = FrameSource::open(&factory, bytes, format)?;
        let header = read_header(&source, format, bytes)?;
        request.check(&header)?;

        let orientation = orientation_of(format, bytes);
        let target = request.effective_size(header.natural_size);
        let stored_target = orientation.apply_to_size(target.width, target.height);
        let stored =
            orientation.apply_to_size(header.natural_size.width, header.natural_size.height);

        let pixels = copy_converted(&factory, &source, stored, stored_target, format)?;
        let (pixels, width, height) = orientation.apply(pixels, stored_target.0, stored_target.1);

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
            image: DecodedImage::from_rgba8(width, height, AlphaType::Straight, pixels, format)?,
            header,
            acceleration: Acceleration::PlatformSoftware,
            backend: "wic",
        })
    }
}

const CONTAINERS: [(ImageFormat, &GUID); 3] = [
    (ImageFormat::Png, &GUID_ContainerFormatPng),
    (ImageFormat::Jpeg, &GUID_ContainerFormatJpeg),
    (ImageFormat::WebP, &GUID_ContainerFormatWebp),
];

fn probe_capabilities() -> Capabilities {
    let Ok(factory) = imaging_factory() else {
        return Capabilities::none();
    };
    let mut capabilities = Capabilities::none();
    for (format, container) in CONTAINERS {
        if codec_present(&factory, container) {
            capabilities = capabilities.with(format, Acceleration::PlatformSoftware);
        }
    }
    capabilities
}

fn codec_present(factory: &IWICImagingFactory, container: &GUID) -> bool {
    let created = unsafe { factory.CreateDecoder(container, ptr::null()) };
    created.is_ok()
}

thread_local! {
    static APARTMENT: ComApartment = ComApartment::enter();
}

#[derive(Debug)]
struct ComApartment {
    entered: HRESULT,
    owns_reference: bool,
}

impl ComApartment {
    fn enter() -> Self {
        let entered = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self {
            entered,
            owns_reference: entered.is_ok(),
        }
    }

    fn usable(&self) -> bool {
        self.entered.is_ok() || self.entered == RPC_E_CHANGED_MODE
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.owns_reference {
            unsafe { CoUninitialize() };
        }
    }
}

fn ensure_apartment() -> ComResult<()> {
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
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

#[derive(Debug)]
struct FrameSource<'bytes> {
    frame: IWICBitmapFrameDecode,
    decoder: IWICBitmapDecoder,
    _stream: IWICStream,
    bytes: PhantomData<&'bytes [u8]>,
}

impl<'bytes> FrameSource<'bytes> {
    fn open(
        factory: &IWICImagingFactory,
        bytes: &'bytes [u8],
        format: ImageFormat,
    ) -> Result<Self, ImageError> {
        unsafe {
            let stream = factory
                .CreateStream()
                .map_err(|error| wic_error(format, "CreateStream", &error))?;
            stream
                .InitializeFromMemory(bytes)
                .map_err(|error| wic_error(format, "InitializeFromMemory", &error))?;
            let decoder = factory
                .CreateDecoderFromStream(&stream, ptr::null(), WICDecodeMetadataCacheOnDemand)
                .map_err(|error| wic_error(format, "CreateDecoderFromStream", &error))?;
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

    fn stored_size(&self, format: ImageFormat) -> Result<(u32, u32), ImageError> {
        let mut width = 0u32;
        let mut height = 0u32;
        unsafe { self.frame.GetSize(&raw mut width, &raw mut height) }
            .map_err(|error| wic_error(format, "GetSize", &error))?;
        Ok((width, height))
    }
}

fn read_header(
    source: &FrameSource<'_>,
    format: ImageFormat,
    bytes: &[u8],
) -> Result<ImageHeader, ImageError> {
    let (width, height) = source.stored_size(format)?;
    let pixel_format = unsafe { source.frame.GetPixelFormat() };
    let frame_count = unsafe { source.decoder.GetFrameCount() };

    let (width, height) = orientation_of(format, bytes).apply_to_size(width, height);
    Ok(ImageHeader {
        format,
        natural_size: PixelSize { width, height },
        has_alpha: pixel_format
            .map_or_else(|_| default_alpha(format), |guid| has_alpha(guid, format)),
        animated: frame_count.is_ok_and(|count| count > 1),
    })
}

fn copy_converted(
    factory: &IWICImagingFactory,
    source: &FrameSource<'_>,
    stored: (u32, u32),
    stored_target: (u32, u32),
    format: ImageFormat,
) -> Result<Vec<u8>, ImageError> {
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

fn copy_pixels(
    source: &IWICBitmapSource,
    (width, height): (u32, u32),
    format: ImageFormat,
) -> Result<Vec<u8>, ImageError> {
    let stride = width
        .checked_mul(4)
        .ok_or_else(|| ImageError::too_large(width, height, "stride overflows u32"))?;
    let length = expected_byte_len(width, height).ok_or_else(|| {
        ImageError::too_large(width, height, "width * height * 4 overflows usize")
    })?;
    let mut buffer = vec![0u8; length];
    unsafe { source.CopyPixels(ptr::null(), stride, &mut buffer) }
        .map_err(|error| wic_error(format, "CopyPixels", &error))?;
    Ok(buffer)
}

fn orientation_of(format: ImageFormat, bytes: &[u8]) -> Orientation {
    if format == ImageFormat::Jpeg {
        orientation::jpeg_orientation(bytes)
    } else {
        Orientation::Identity
    }
}

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

fn has_alpha(pixel_format: GUID, format: ImageFormat) -> bool {
    if WITH_ALPHA.contains(&pixel_format) {
        return true;
    }
    if WITHOUT_ALPHA.contains(&pixel_format) {
        return false;
    }
    default_alpha(format)
}

const fn default_alpha(format: ImageFormat) -> bool {
    !matches!(format, ImageFormat::Jpeg)
}

fn wic_error(format: ImageFormat, context: &str, error: &ComError) -> ImageError {
    ImageError::decode(format, format!("WIC {context}: {error}"))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use bobcat_core::image::ImageFormat;
    use windows::Win32::Graphics::Imaging::{
        GUID_WICPixelFormat8bppIndexed, GUID_WICPixelFormat24bppBGR, GUID_WICPixelFormat32bppBGRA,
        GUID_WICPixelFormat32bppPRGBA, GUID_WICPixelFormatDontCare,
    };

    use super::{CONTAINERS, default_alpha, has_alpha};
    use crate::image_decoders::orientation::Orientation;

    #[test]
    fn every_claimed_format_maps_to_a_distinct_container_guid() {
        for (index, (_, container)) in CONTAINERS.iter().enumerate() {
            for (_, other) in &CONTAINERS[index + 1..] {
                assert_ne!(*container, *other);
            }
        }
    }

    #[test]
    fn known_pixel_formats_decide_alpha_regardless_of_container() {
        assert!(has_alpha(GUID_WICPixelFormat32bppBGRA, ImageFormat::Jpeg));
        assert!(has_alpha(GUID_WICPixelFormat32bppPRGBA, ImageFormat::WebP));
        assert!(!has_alpha(GUID_WICPixelFormat24bppBGR, ImageFormat::Png));
    }

    #[test]
    fn unrecognised_pixel_formats_fall_back_to_the_container_default() {
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
