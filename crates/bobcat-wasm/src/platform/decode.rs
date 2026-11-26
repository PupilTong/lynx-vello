//! [`BrowserDecoder`]: the browser's own decoders, in the Render Worker
//! itself. Nothing here parses an image container, and nothing crosses to
//! the main thread.
//!
//! - **A still image, and an animation's first frame**, are `createImageBitmap` of the bytes as a
//!   `Blob`: the browser's codecs, its colour management and its EXIF orientation. The answer is an
//!   `ImageBitmap`, a [`BrowserFrame`], whose pixels never pass through Wasm memory: the painter
//!   copies it straight into a texture ([`bobcat_core::FrameWriter`]). An image larger than the
//!   size asked for is resized by a second `createImageBitmap` of the first, with `resizeWidth`,
//!   `resizeHeight` and `resizeQuality: "high"`; the first is closed.
//! - **An animation's frames** are `WebCodecs`' `ImageDecoder`: its selected track's `frameCount`
//!   and `repetitionCount` and each frame's `duration` say what the frames are
//!   ([`crate::image_frames`]), and `decode({ frameIndex })` answers a frame as a `VideoFrame`,
//!   which `createImageBitmap` turns into an `ImageBitmap` at frame 0's decoded size before the
//!   `VideoFrame` is closed. One `ImageDecoder` per animation stays open between frames, so the
//!   browser composites a frame from the one before it rather than from the first, until
//!   [`vant::ImageDecoder::forget_frames`]. Where `ImageDecoder` is missing, or does not decode the
//!   type, an image is one frame: its `createImageBitmap` decode.
//!
//! Every bitmap is decoded with `imageOrientation: "from-image"`,
//! `premultiplyAlpha: "premultiply"` and `colorSpaceConversion: "default"`:
//! the orientation and colour management the `Image` element applied, and
//! the premultiplied alpha the engine draws. The bytes are copied into a
//! buffer of their own first: the Wasm memory is a `SharedArrayBuffer`, which
//! neither a `Blob` nor an `ImageDecoder` may be built over, and a view of it
//! is detached by a memory growth.

use std::cell::RefCell;

use bytes::Bytes;
use js_sys::{Array, Object, Promise, Reflect, Uint8Array};
use vant::image::animation::ImageFrames;
use vant::image::{Bitmap, DecodeError, DecodedImage, PlatformBitmap, target_size};
use vant::image_header::ImageHeader;
use vant::mime::ImageFormat;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use super::frame::BrowserFrame;
use crate::image_frames::{self, Track};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = createImageBitmap, catch)]
    fn create_image_bitmap(
        source: &JsValue,
        options: &web_sys::ImageBitmapOptions,
    ) -> Result<Promise, JsValue>;

    #[derive(Clone, Debug)]
    #[wasm_bindgen(js_name = ImageDecoder)]
    type WebImageDecoder;

    #[wasm_bindgen(constructor, catch, js_class = "ImageDecoder")]
    fn new(init: &Object) -> Result<WebImageDecoder, JsValue>;

    #[wasm_bindgen(static_method_of = WebImageDecoder, js_class = "ImageDecoder", js_name = isTypeSupported, catch)]
    fn is_type_supported(media_type: &str) -> Result<Promise, JsValue>;

    #[wasm_bindgen(method, getter)]
    fn completed(this: &WebImageDecoder) -> Promise;

    #[wasm_bindgen(method, getter)]
    fn tracks(this: &WebImageDecoder) -> ImageTrackList;

    #[wasm_bindgen(method, catch)]
    fn decode(this: &WebImageDecoder, options: &Object) -> Result<Promise, JsValue>;

    #[wasm_bindgen(method)]
    fn close(this: &WebImageDecoder);

    type ImageTrackList;

    #[wasm_bindgen(method, getter)]
    fn ready(this: &ImageTrackList) -> Promise;

    #[wasm_bindgen(method, getter, js_name = selectedTrack)]
    fn selected_track(this: &ImageTrackList) -> Option<ImageTrack>;

    type ImageTrack;

    #[wasm_bindgen(method, getter, js_name = frameCount)]
    fn frame_count(this: &ImageTrack) -> u32;

    #[wasm_bindgen(method, getter, js_name = repetitionCount)]
    fn repetition_count(this: &ImageTrack) -> f64;

    #[wasm_bindgen(js_name = VideoFrame)]
    type DecodedFrame;

    #[wasm_bindgen(method, getter, js_name = displayWidth)]
    fn display_width(this: &DecodedFrame) -> u32;

    #[wasm_bindgen(method, getter, js_name = displayHeight)]
    fn display_height(this: &DecodedFrame) -> u32;

    #[wasm_bindgen(method, getter)]
    fn duration(this: &DecodedFrame) -> Option<f64>;

    #[wasm_bindgen(method, js_name = close)]
    fn close_frame(this: &DecodedFrame);
}

#[derive(Debug)]
struct Open {
    bytes: Bytes,
    decoder: WebImageDecoder,
}

#[derive(Debug, Default)]
pub struct BrowserDecoder {
    open: RefCell<Vec<Open>>,
}

impl BrowserDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    async fn decoder_for(
        &self,
        bytes: &Bytes,
        format: ImageFormat,
    ) -> Result<Option<WebImageDecoder>, DecodeError> {
        if let Some(open) = self.find(bytes) {
            return Ok(Some(open));
        }
        let Some(decoder) = open_decoder(bytes, format).await? else {
            return Ok(None);
        };
        if let Some(open) = self.find(bytes) {
            decoder.close();
            return Ok(Some(open));
        }
        self.open.borrow_mut().push(Open {
            bytes: bytes.clone(),
            decoder: decoder.clone(),
        });
        Ok(Some(decoder))
    }

    fn find(&self, bytes: &Bytes) -> Option<WebImageDecoder> {
        self.open
            .borrow()
            .iter()
            .find(|open| same_allocation(&open.bytes, bytes))
            .map(|open| open.decoder.clone())
    }
}

impl vant::ImageDecoder for BrowserDecoder {
    type Frame = BrowserFrame;

    fn decode_blocking(
        &self,
        _bytes: &[u8],
        _format: ImageFormat,
        _header: Option<ImageHeader>,
        _max: (u32, u32),
    ) -> Result<Bitmap, DecodeError> {
        Err(DecodeError::Unavailable(
            "the browser's image decoder answers asynchronously".to_owned(),
        ))
    }

    async fn decode(
        &self,
        bytes: Bytes,
        format: ImageFormat,
        _header: Option<ImageHeader>,
        max: (u32, u32),
    ) -> Result<DecodedImage<BrowserFrame>, DecodeError> {
        let blob = blob(&bytes, format.media_type())?;
        let full = bitmap_of(&blob, &bitmap_options(None)).await?;
        let source = full.size();
        if source.0 == 0 || source.1 == 0 {
            return Err(DecodeError::Malformed(
                "the image has no intrinsic size".to_owned(),
            ));
        }
        let size = target_size(source.0, source.1, max);
        let frame = if size == source {
            full
        } else {
            let bitmap = full.bitmap().expect("`createImageBitmap` answers a bitmap");
            bitmap_of(bitmap, &bitmap_options(Some(size))).await?
        };
        Ok(platform(frame, source))
    }

    fn available(&self) -> Result<(), DecodeError> {
        if has_global("createImageBitmap") {
            Ok(())
        } else {
            Err(DecodeError::Unavailable(
                "this Worker has no createImageBitmap".to_owned(),
            ))
        }
    }

    async fn frames(
        &self,
        bytes: Bytes,
        format: ImageFormat,
        _header: Option<ImageHeader>,
    ) -> Result<ImageFrames, DecodeError> {
        let Some(decoder) = open_decoder(&bytes, format).await? else {
            return Ok(image_frames::frames(None));
        };
        let track = read_track(&decoder).await;
        decoder.close();
        Ok(image_frames::frames(track?.as_ref()))
    }

    async fn decode_frame(
        &self,
        bytes: Bytes,
        format: ImageFormat,
        header: Option<ImageHeader>,
        index: usize,
        max: (u32, u32),
    ) -> Result<DecodedImage<BrowserFrame>, DecodeError> {
        if index == 0 {
            return self.decode(bytes, format, header, max).await;
        }
        let Some(decoder) = self.decoder_for(&bytes, format).await? else {
            return Err(DecodeError::Unsupported(format!(
                "this browser decodes one frame, not frame {index}"
            )));
        };
        let frame = decode_index(&decoder, index).await?;
        let source = (frame.display_width(), frame.display_height());
        let size = target_size(source.0, source.1, max);
        let bitmap = bitmap_of(&frame, &bitmap_options((size != source).then_some(size))).await;
        frame.close_frame();
        Ok(platform(bitmap?, source))
    }

    fn forget_frames(&self, bytes: &Bytes) {
        let mut open = self.open.borrow_mut();
        if let Some(index) = open
            .iter()
            .position(|open| same_allocation(&open.bytes, bytes))
        {
            open.swap_remove(index).decoder.close();
        }
    }
}

fn platform(frame: BrowserFrame, source: (u32, u32)) -> DecodedImage<BrowserFrame> {
    let (width, height) = frame.size();
    DecodedImage::Platform(PlatformBitmap {
        frame,
        width,
        height,
        source_width: source.0,
        source_height: source.1,
    })
}

fn same_allocation(a: &Bytes, b: &Bytes) -> bool {
    a.as_ptr() == b.as_ptr() && a.len() == b.len()
}

fn has_global(name: &str) -> bool {
    Reflect::has(&js_sys::global(), &JsValue::from_str(name)).unwrap_or(false)
}

fn own_copy(bytes: &[u8]) -> Uint8Array {
    Uint8Array::from(bytes)
}

fn blob(bytes: &[u8], media_type: &str) -> Result<web_sys::Blob, DecodeError> {
    let parts = Array::of1(&own_copy(bytes));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(media_type);
    web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
        .map_err(|error| DecodeError::Unavailable(describe(&error)))
}

fn bitmap_options(size: Option<(u32, u32)>) -> web_sys::ImageBitmapOptions {
    let options = web_sys::ImageBitmapOptions::new();
    options.set_image_orientation(web_sys::ImageOrientation::FromImage);
    options.set_premultiply_alpha(web_sys::PremultiplyAlpha::Premultiply);
    options.set_color_space_conversion(web_sys::ColorSpaceConversion::Default);
    if let Some((width, height)) = size {
        options.set_resize_width(width);
        options.set_resize_height(height);
        options.set_resize_quality(web_sys::ResizeQuality::High);
    }
    options
}

async fn bitmap_of(
    source: &JsValue,
    options: &web_sys::ImageBitmapOptions,
) -> Result<BrowserFrame, DecodeError> {
    let promise = create_image_bitmap(source, options).map_err(malformed)?;
    let bitmap = JsFuture::from(promise).await.map_err(malformed)?;
    Ok(BrowserFrame::new(bitmap.unchecked_into()))
}

async fn open_decoder(
    bytes: &[u8],
    format: ImageFormat,
) -> Result<Option<WebImageDecoder>, DecodeError> {
    if !has_global("ImageDecoder") {
        return Ok(None);
    }
    let media_type = format.media_type();
    let supported = WebImageDecoder::is_type_supported(media_type).map_err(malformed)?;
    if JsFuture::from(supported).await.map_err(malformed)? != JsValue::TRUE {
        return Ok(None);
    }
    let init = Object::new();
    set(&init, "type", &JsValue::from_str(media_type));
    set(&init, "data", &own_copy(bytes));
    set(&init, "preferAnimation", &JsValue::TRUE);
    set(&init, "colorSpaceConversion", &JsValue::from_str("default"));
    WebImageDecoder::new(&init).map(Some).map_err(malformed)
}

async fn read_track(decoder: &WebImageDecoder) -> Result<Option<Track>, DecodeError> {
    let tracks = decoder.tracks();
    JsFuture::from(tracks.ready()).await.map_err(malformed)?;
    JsFuture::from(decoder.completed())
        .await
        .map_err(malformed)?;
    let Some(track) = tracks.selected_track() else {
        return Ok(None);
    };
    let count = track.frame_count() as usize;
    let mut durations = Vec::with_capacity(count);
    if count > 1 {
        for index in 0..count {
            let frame = decode_index(decoder, index).await?;
            durations.push(frame.duration());
            frame.close_frame();
        }
    }
    Ok(Some(Track {
        durations,
        repetition_count: track.repetition_count(),
    }))
}

async fn decode_index(
    decoder: &WebImageDecoder,
    index: usize,
) -> Result<DecodedFrame, DecodeError> {
    let options = Object::new();
    #[expect(
        clippy::cast_precision_loss,
        reason = "a frame index is far below 2^53"
    )]
    set(&options, "frameIndex", &JsValue::from_f64(index as f64));
    let promise = decoder.decode(&options).map_err(malformed)?;
    let result = JsFuture::from(promise).await.map_err(malformed)?;
    Reflect::get(&result, &JsValue::from_str("image"))
        .map(JsCast::unchecked_into)
        .map_err(malformed)
}

fn set(object: &Object, key: &str, value: &JsValue) {
    let _ = Reflect::set(object, &JsValue::from_str(key), value);
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the shape `map_err` hands a JavaScript error over in"
)]
fn malformed(error: JsValue) -> DecodeError {
    DecodeError::Malformed(describe(&error))
}

fn describe(error: &JsValue) -> String {
    error
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .or_else(|| error.as_string())
        .unwrap_or_else(|| format!("{error:?}"))
}
