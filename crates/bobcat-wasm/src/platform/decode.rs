//! [`BrowserDecoder`]: the main thread's `Image` element, reached over the
//! mailbox protocol in [`mailbox`](super::mailbox).

use std::rc::Rc;

use bytes::Bytes;
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
    /// Blocks this Worker on the job's mailbox, for a restore. The main
    /// thread's decoder takes the media type rather than a header.
    fn decode_blocking(
        &self,
        bytes: &[u8],
        format: ImageFormat,
        _header: Option<ImageHeader>,
        max: (u32, u32),
    ) -> Result<Bitmap, DecodeError> {
        self.decoder()?
            .decode_blocking(bytes, format.media_type(), max)
    }

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
}
