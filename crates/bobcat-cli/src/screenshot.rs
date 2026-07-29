use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScreenshotError {
    #[error("a {width}\u{d7}{height} RGBA frame needs {expected} bytes, got {actual}")]
    BufferSize {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("PNG encoding failed: {0}")]
    Codec(#[from] png::EncodingError),
}

pub(crate) fn write_png(
    path: &Path,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<(), ScreenshotError> {
    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .unwrap_or(usize::MAX);
    if pixels.len() != expected {
        return Err(ScreenshotError::BufferSize {
            width,
            height,
            expected,
            actual: pixels.len(),
        });
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let output = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(output, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::write_png;

    #[test]
    fn writes_a_png_and_creates_its_parent() {
        let root = std::env::temp_dir().join(format!(
            "bobcat-cli-png-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = root.join("nested/frame.png");
        write_png(&path, 2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
