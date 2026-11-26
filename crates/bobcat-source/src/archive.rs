//! Bounded, platform-independent ZIP resources. No filesystem or browser IO.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};

use percent_encoding::percent_decode_str;
use thiserror::Error;
use url::Url;
use vant::registry::Register;

use crate::{PageSource, SourceError};

pub const MAX_ZIP_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXPANDED_BYTES: usize = 128 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;

#[derive(Debug)]
pub struct ZipSource {
    files: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ZipSourceError {
    #[error("ZIP exceeds the 64 MiB compressed size limit")]
    CompressedLimit,
    #[error("ZIP exceeds the 128 MiB extracted size limit")]
    ExpandedLimit,
    #[error("ZIP exceeds the 4096 entry limit")]
    EntryLimit,
    #[error("invalid ZIP: {0}")]
    Decode(#[from] zip::result::ZipError),
    #[error("could not read ZIP entry: {0}")]
    Read(#[from] std::io::Error),
    #[error("ZIP paths must be UTF-8 relative paths without empty, dot or parent segments")]
    InvalidPath,
    #[error("ZIP contains duplicate paths")]
    DuplicatePath,
    #[error("ZIP symbolic links are not supported")]
    SymbolicLink,
    #[error("ZIP entry URL must be hierarchical and have a valid UTF-8 path")]
    InvalidEntryUrl,
    #[error("entry template is not in the ZIP")]
    MissingEntry,
    #[error(transparent)]
    Page(#[from] SourceError),
}

impl ZipSource {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ZipSourceError> {
        Self::decode(bytes, MAX_EXPANDED_BYTES, MAX_ENTRIES)
    }

    fn decode(bytes: &[u8], limit: usize, entry_limit: usize) -> Result<Self, ZipSourceError> {
        if bytes.len() > MAX_ZIP_BYTES {
            return Err(ZipSourceError::CompressedLimit);
        }
        validate_directory(bytes, entry_limit)?;
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
        if zip.len() > entry_limit {
            return Err(ZipSourceError::EntryLimit);
        }
        let mut files = BTreeMap::new();
        let mut total = 0;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index)?;
            let name =
                std::str::from_utf8(entry.name_raw()).map_err(|_| ZipSourceError::InvalidPath)?;
            validate_path(name.strip_suffix('/').unwrap_or(name))?;
            if entry.is_symlink() {
                return Err(ZipSourceError::SymbolicLink);
            }
            if entry.is_dir() {
                continue;
            }
            let name = name.to_owned();
            if entry.size() > (limit - total) as u64 {
                return Err(ZipSourceError::ExpandedLimit);
            }
            let mut data = Vec::new();
            let mut chunk = [0; 8192];
            loop {
                let count = entry.read(&mut chunk)?;
                total += count;
                if total > limit {
                    return Err(ZipSourceError::ExpandedLimit);
                }
                if count == 0 {
                    break;
                }
                data.extend_from_slice(&chunk[..count]);
            }
            files.insert(name, data);
        }
        Ok(Self { files })
    }

    pub fn page(&self, entry: &Url) -> Result<PageSource, ZipSourceError> {
        let path = entry_path(entry)?;
        let bytes = self.files.get(&path).ok_or(ZipSourceError::MissingEntry)?;
        Ok(PageSource::from_bytes(entry, bytes)?)
    }

    pub fn register_with(
        self,
        resources: &impl Register<bobcat_core::PreparsedStyleSheet>,
        entry: &Url,
    ) -> Result<(), ZipSourceError> {
        entry_path(entry)?;
        let mut root = entry.clone();
        root.set_query(None);
        root.set_fragment(None);
        root.set_path("/");
        for (path, bytes) in self.files {
            let mut url = root.clone();
            url.path_segments_mut()
                .map_err(|()| ZipSourceError::InvalidEntryUrl)?
                .clear()
                .extend(path.split('/'));
            resources
                .register(url.as_str(), bytes.into(), None)
                .expect("archive registration URLs were built from a parsed URL");
        }
        Ok(())
    }
}

fn validate_path(path: &str) -> Result<(), ZipSourceError> {
    if path.contains(['\\', '\0'])
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ZipSourceError::InvalidPath);
    }
    Ok(())
}

fn entry_path(entry: &Url) -> Result<String, ZipSourceError> {
    if entry.cannot_be_a_base() || !entry.path().starts_with('/') {
        return Err(ZipSourceError::InvalidEntryUrl);
    }
    let path = percent_decode_str(&entry.path()[1..])
        .decode_utf8()
        .map_err(|_| ZipSourceError::InvalidEntryUrl)?;
    validate_path(&path)?;
    Ok(path.into_owned())
}

fn validate_directory(bytes: &[u8], limit: usize) -> Result<(), ZipSourceError> {
    let malformed = || {
        ZipSourceError::Decode(zip::result::ZipError::InvalidArchive(
            "invalid ZIP central directory".into(),
        ))
    };
    let read16 = |offset| u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
    let read32 = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let end = (bytes.len().saturating_sub(65_557)..bytes.len().saturating_sub(21))
        .rev()
        .find(|&i| {
            bytes[i..].starts_with(b"PK\x05\x06")
                && i + 22 + usize::from(read16(i + 20)) == bytes.len()
        })
        .ok_or_else(malformed)?;
    let count = usize::from(read16(end + 10));
    if count > limit {
        return Err(ZipSourceError::EntryLimit);
    }
    if read16(end + 4) != 0 || read16(end + 6) != 0 || usize::from(read16(end + 8)) != count {
        return Err(malformed());
    }
    let size = usize::try_from(read32(end + 12)).map_err(|_| malformed())?;
    let mut offset = usize::try_from(read32(end + 16)).map_err(|_| malformed())?;
    if offset.checked_add(size) != Some(end) {
        return Err(malformed());
    }
    let mut names = BTreeSet::new();
    for _ in 0..count {
        if offset.checked_add(46).is_none_or(|next| next > end)
            || !bytes[offset..].starts_with(b"PK\x01\x02")
        {
            return Err(malformed());
        }
        let name_end = offset + 46 + usize::from(read16(offset + 28));
        let next = name_end + usize::from(read16(offset + 30)) + usize::from(read16(offset + 32));
        if next > end {
            return Err(malformed());
        }
        if !names.insert(&bytes[offset + 46..name_end]) {
            return Err(ZipSourceError::DuplicatePath);
        }
        offset = next;
    }
    if offset != end {
        return Err(malformed());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn limits_count_actual_output_and_central_directory_entries() {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("large", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&[42; 100]).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(matches!(
            ZipSource::decode(&bytes, 99, 4),
            Err(ZipSourceError::ExpandedLimit)
        ));
        assert!(matches!(
            ZipSource::decode(&bytes, 100, 0),
            Err(ZipSourceError::EntryLimit)
        ));
        assert!(ZipSource::decode(&bytes, 100, 1).is_ok());
    }
}
