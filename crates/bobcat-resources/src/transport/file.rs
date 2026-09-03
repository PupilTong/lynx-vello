//! `file:` URLs on native targets: the local filesystem, labelled by
//! extension.

use std::io;

use bobcat_core::resource::{ResourceErrorKind, ResourceErrorPhase};
use bytes::Bytes;
use url::Url;

use crate::error::Failure;
use crate::mime;

/// Reads the file `url` names.
pub(crate) fn read(url: &Url) -> Result<(Bytes, Option<mime::MediaType>), Failure> {
    let path = url.to_file_path().map_err(|()| {
        Failure::new(
            ResourceErrorKind::InvalidUrl,
            ResourceErrorPhase::Open,
            "the file URL has no local path",
        )
    })?;
    let bytes = std::fs::read(&path).map_err(|error| {
        let kind = match error.kind() {
            io::ErrorKind::NotFound => ResourceErrorKind::NotFound,
            io::ErrorKind::PermissionDenied => ResourceErrorKind::PermissionDenied,
            _ => ResourceErrorKind::Io,
        };
        Failure::new(
            kind,
            ResourceErrorPhase::Open,
            format!("could not read `{}`: {error}", path.display()),
        )
    })?;
    let media_type = mime::from_extension(url.path());
    Ok((Bytes::from(bytes), media_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_reads_with_its_extension_type_and_a_missing_one_is_not_found() {
        let dir =
            std::env::temp_dir().join(format!("bobcat-resources-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("style.css");
        std::fs::write(&path, b"a{}").expect("write");
        let url = Url::from_file_path(&path).expect("a file URL");
        let (bytes, media_type) = read(&url).expect("read");
        assert_eq!(&bytes[..], b"a{}");
        assert_eq!(
            media_type
                .map(|media_type| media_type.essence().to_owned())
                .as_deref(),
            Some("text/css")
        );

        let missing = Url::from_file_path(dir.join("missing.png")).expect("a file URL");
        let failure = read(&missing).expect_err("missing");
        assert_eq!(failure.kind, ResourceErrorKind::NotFound);
        assert_eq!(failure.phase, ResourceErrorPhase::Open);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
