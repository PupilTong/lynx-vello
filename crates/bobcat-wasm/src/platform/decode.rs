//! [`BrowserDecoder`]: the main thread's `Image` element, reached over the
//! mailbox protocol in [`mailbox`](super::mailbox).

use std::rc::Rc;

use bytes::Bytes;
use vant::image::animation::ImageFrames;
use vant::image::{Bitmap, DecodeError};
use vant::image_header::ImageHeader;
use vant::mime::ImageFormat;

use super::mailbox;

/// The main thread's `Image` element as a [`vant::ImageDecoder`], reached
/// over the port the host handed the Render Worker. Without a port every
/// decode answers [`DecodeError::Unavailable`].
#[derive(Clone, Debug, Default)]
pub struct BrowserDecoder {
    decoder: Option<Rc<mailbox::ImageDecoder>>,
}

impl BrowserDecoder {
    /// The decoder over `port`, this Worker's end of the channel whose other
    /// end the host's main-thread image decoder listens on
    /// (`js/image-decoder.ts`).
    ///
    /// # Errors
    ///
    /// When the port refuses the decoder's first message.
    pub fn new(port: web_sys::MessagePort) -> Result<Self, String> {
        mailbox::ImageDecoder::new(port).map(|decoder| Self {
            decoder: Some(decoder),
        })
    }

    /// The decoder with no port: every decode is unavailable.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    fn decoder(&self) -> Result<&mailbox::ImageDecoder, DecodeError> {
        self.decoder
            .as_deref()
            .ok_or_else(|| DecodeError::Unavailable("no image decoder was configured".to_owned()))
    }
}

impl vant::ImageDecoder for BrowserDecoder {
    /// The browser decodes on the main thread's event loop, which this
    /// Worker cannot block on, so a blocking decode is unavailable: every
    /// decode, a restore included, goes through [`Self::decode`].
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

    /// The main thread's decoder takes the media type rather than a
    /// header.
    async fn decode(
        &self,
        bytes: Bytes,
        format: ImageFormat,
        _header: Option<ImageHeader>,
        max: (u32, u32),
    ) -> Result<Bitmap, DecodeError> {
        self.decoder()?
            .decode(&bytes, format.media_type(), max)
            .await
    }

    fn available(&self) -> Result<(), DecodeError> {
        self.decoder().map(|_| ())
    }

    /// One frame, whatever the file: the main thread's `Image` element
    /// hands back the first frame of an animated GIF, APNG or WebP and
    /// nothing of the rest, so the browser draws such a file as its first
    /// frame until the Render Worker decodes frames itself
    /// (`docs/picture-source-design.md`, step 5). Answered here rather than
    /// inherited, so the browser's answer is a decision on record.
    fn frames_blocking(
        &self,
        _bytes: &[u8],
        _format: ImageFormat,
        _header: Option<ImageHeader>,
    ) -> Result<ImageFrames, DecodeError> {
        Ok(ImageFrames::still())
    }
}
