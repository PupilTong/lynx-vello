//! HTML's "Pixel manipulation": the answer of a `getImageData` readback
//! ([`ImageDataRead`]), and the pixel arithmetic `getImageData` and
//! `putImageData` share.

use super::CanvasError;
use crate::render::canvas::{PendingRead, SurfaceError};

/// A `getImageData` whose readback is issued
/// ([`Context2d::read_image_data`](super::Context2d::read_image_data)): the
/// answer's pixels once the GPU has sent them. Owns everything it needs,
/// so the canvas is free while it is waited on.
#[derive(Debug)]
#[must_use = "the pixels are only had by waiting"]
pub struct ImageDataRead {
    /// The answer, transparent black where the bitmap is not.
    pub(super) out: Vec<u8>,
    /// The part inside the bitmap, `None` when there is none.
    pub(super) read: Option<RegionRead>,
}

#[derive(Debug)]
pub(super) struct RegionRead {
    pub(super) pending: PendingRead,
    pub(super) place: Placement,
}

/// Where the rows read go in the answer.
#[derive(Clone, Copy, Debug)]
pub(super) struct Placement {
    /// The first pixel read, in the answer.
    pub(super) x0: i64,
    pub(super) y0: i64,
    /// Pixels per row read, and per row of the answer.
    pub(super) span: i64,
    pub(super) stride: i64,
}

impl ImageDataRead {
    /// Blocks the calling thread until the pixels are read.
    ///
    /// # Errors
    ///
    /// `InvalidState` when the GPU did not answer the readback: no device,
    /// a device lost, or, on wasm32, the Render Worker dropped it.
    pub fn wait(self) -> Result<Vec<u8>, CanvasError> {
        match self.read {
            None => Ok(self.out),
            Some(read) => {
                let pixels = read.pending.wait();
                read.place.write(self.out, pixels)
            }
        }
    }

    /// [`Self::wait`] for a caller that waits asynchronously.
    ///
    /// # Errors
    ///
    /// As [`Self::wait`].
    pub async fn wait_async(self) -> Result<Vec<u8>, CanvasError> {
        match self.read {
            None => Ok(self.out),
            Some(read) => {
                let pixels = read.pending.wait_async().await;
                read.place.write(self.out, pixels)
            }
        }
    }
}

impl Placement {
    /// Unpremultiplies the rows `read` answered into `out`.
    fn write(
        self,
        mut out: Vec<u8>,
        read: Result<Vec<u8>, SurfaceError>,
    ) -> Result<Vec<u8>, CanvasError> {
        let read = read.map_err(|_| CanvasError::InvalidState)?;
        let bytes = index(0, self.span, 0);
        for (row, from) in (self.y0..).zip(read.chunks_exact(bytes)) {
            let dst = index(row, self.x0, self.stride);
            let (texels, _) = out[dst..dst + bytes].as_chunks_mut::<4>();
            for (to, &from) in texels.iter_mut().zip(from.as_chunks::<4>().0) {
                *to = unpremultiply(from);
            }
        }
        Ok(out)
    }
}

/// A rectangle's start and extent with a negative extent turned around.
pub(super) fn normalize(start: i32, extent: i32) -> (i64, i64) {
    let (start, extent) = (i64::from(start), i64::from(extent));
    if extent < 0 {
        (start + extent, -extent)
    } else {
        (start, extent)
    }
}

/// A coordinate already clipped to the bitmap.
pub(super) fn to_u32(value: i64) -> u32 {
    u32::try_from(value).expect("a coordinate inside the bitmap")
}

/// The byte offset of pixel `(x, y)` in rows `stride` pixels wide.
pub(super) fn index(y: i64, x: i64, stride: i64) -> usize {
    usize::try_from((y * stride + x) * 4).expect("pixel inside the buffer")
}

fn unpremultiply(pixel: [u8; 4]) -> [u8; 4] {
    let a = u32::from(pixel[3]);
    match a {
        0 => [0; 4],
        255 => [pixel[0], pixel[1], pixel[2], 255],
        _ => {
            let channel =
                |c: u8| u8::try_from(((u32::from(c) * 255 + a / 2) / a).min(255)).unwrap_or(255);
            [
                channel(pixel[0]),
                channel(pixel[1]),
                channel(pixel[2]),
                pixel[3],
            ]
        }
    }
}

pub(super) fn premultiply(pixel: [u8; 4]) -> [u8; 4] {
    let a = u32::from(pixel[3]);
    let channel = |c: u8| u8::try_from((u32::from(c) * a + 127) / 255).unwrap_or(255);
    [
        channel(pixel[0]),
        channel(pixel[1]),
        channel(pixel[2]),
        pixel[3],
    ]
}
