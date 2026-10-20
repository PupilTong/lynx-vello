//! RGBA8 images and their PNG container.

use std::fmt;
use std::path::Path;

/// A tightly packed, row-major RGBA8 image.
#[derive(PartialEq, Eq)]
pub struct Image {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl fmt::Debug for Image {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Image")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum ImageError {
    Size { width: u32, height: u32, len: usize },
    Dimensions { width: u32, height: u32 },
    Format(String),
    Codec(String),
    Io(std::io::Error),
}

impl fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Size { width, height, len } => write!(
                formatter,
                "a {width}\u{d7}{height} RGBA image needs {} bytes, got {len}",
                usize::try_from(*width)
                    .unwrap_or(usize::MAX)
                    .saturating_mul(
                        usize::try_from(*height)
                            .unwrap_or(usize::MAX)
                            .saturating_mul(4)
                    )
            ),
            Self::Dimensions { width, height } => write!(
                formatter,
                "a {width}\u{d7}{height} RGBA image does not fit in memory on this target"
            ),
            Self::Format(detail) => write!(formatter, "unsupported PNG: {detail}"),
            Self::Codec(detail) => write!(formatter, "PNG codec failed: {detail}"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ImageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ImageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl Image {
    /// Wraps a tightly packed RGBA8 buffer.
    pub fn from_rgba8(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, ImageError> {
        let expected =
            Self::byte_len(width, height).ok_or(ImageError::Dimensions { width, height })?;
        if pixels.len() == expected {
            Ok(Self {
                width,
                height,
                pixels,
            })
        } else {
            Err(ImageError::Size {
                width,
                height,
                len: pixels.len(),
            })
        }
    }

    /// A fully transparent image, for diff output.
    pub fn transparent(width: u32, height: u32) -> Result<Self, ImageError> {
        let len = Self::byte_len(width, height).ok_or(ImageError::Dimensions { width, height })?;
        Ok(Self {
            width,
            height,
            pixels: vec![0; len],
        })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// The number of pixels, for diff-budget arithmetic.
    #[must_use]
    pub const fn pixel_count(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// Whether both images cover the same area.
    #[must_use]
    pub const fn has_same_size(&self, other: &Self) -> bool {
        self.width == other.width && self.height == other.height
    }

    /// Decodes an 8-bit RGBA PNG.
    pub fn decode_png(bytes: &[u8]) -> Result<Self, ImageError> {
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder
            .read_info()
            .map_err(|error| ImageError::Codec(error.to_string()))?;
        let info = reader.info();
        if info.color_type != png::ColorType::Rgba {
            return Err(ImageError::Format(format!(
                "expected RGBA, got {:?}",
                info.color_type
            )));
        }
        if info.bit_depth != png::BitDepth::Eight {
            return Err(ImageError::Format(format!(
                "expected 8 bits per channel, got {:?}",
                info.bit_depth
            )));
        }
        let Some(buffer_size) = reader.output_buffer_size() else {
            return Err(ImageError::Format(
                "the PNG does not declare a usable buffer size".to_owned(),
            ));
        };
        let mut pixels = vec![0; buffer_size];
        let frame = reader
            .next_frame(&mut pixels)
            .map_err(|error| ImageError::Codec(error.to_string()))?;
        pixels.truncate(frame.buffer_size());
        Self::from_rgba8(frame.width, frame.height, pixels)
    }

    /// Encodes an 8-bit RGBA PNG.
    pub fn encode_png(&self) -> Result<Vec<u8>, ImageError> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .and_then(|mut writer| writer.write_image_data(&self.pixels))
            .map_err(|error| ImageError::Codec(error.to_string()))?;
        Ok(bytes)
    }

    /// Reads an image from a PNG file.
    pub fn read_png(path: impl AsRef<Path>) -> Result<Self, ImageError> {
        Self::decode_png(&std::fs::read(path)?)
    }

    /// Writes the image as a PNG file, creating parent directories.
    pub fn write_png(&self, path: impl AsRef<Path>) -> Result<(), ImageError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.encode_png()?)?;
        Ok(())
    }

    const fn byte_len(width: u32, height: u32) -> Option<usize> {
        match (width as usize).checked_mul(height as usize) {
            Some(pixels) => pixels.checked_mul(4),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Image, ImageError};

    #[test]
    fn rejects_a_buffer_of_the_wrong_length() {
        let error = Image::from_rgba8(2, 2, vec![0; 15]).unwrap_err();
        assert!(matches!(error, ImageError::Size { len: 15, .. }));
    }

    #[test]
    fn png_round_trips_exactly() {
        let pixels: Vec<u8> = (0..(3u8 * 2 * 4)).collect();
        let image = Image::from_rgba8(3, 2, pixels.clone()).unwrap();
        let decoded = Image::decode_png(&image.encode_png().unwrap()).unwrap();
        assert_eq!(decoded.width(), 3);
        assert_eq!(decoded.height(), 2);
        assert_eq!(decoded.pixels(), pixels.as_slice());
    }

    #[test]
    fn oversized_dimensions_are_an_error_not_a_panic() {
        let error = Image::from_rgba8(u32::MAX, u32::MAX, Vec::new()).unwrap_err();
        assert!(matches!(error, ImageError::Dimensions { .. }));
        assert!(Image::transparent(u32::MAX, u32::MAX).is_err());
    }

    #[test]
    fn transparent_images_are_zeroed_and_sized() {
        let image = Image::transparent(4, 5).unwrap();
        assert_eq!(image.pixel_count(), 20);
        assert_eq!(image.pixels().len(), 80);
        assert!(image.pixels().iter().all(|&byte| byte == 0));
    }
}
