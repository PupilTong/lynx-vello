//! Registered contents: resources the embedder already holds, served under
//! a URL of its choosing ahead of every other transport.
//!
//! This is what lets one fetcher replace the in-memory ones the embedders
//! carried: a CLI registers the scripts and stylesheet it decoded out of a
//! bundle under `bobcat-memory://` URLs, a browser host registers the bytes
//! its own `fetch` produced under their response URLs, and a test registers
//! a PNG. A registered URL resolves and fetches like any other, with any
//! scheme at all and bypasses the transport cache. A decoded stylesheet response
//! may be cached by the source loader; registration changes invalidate it.
//!
//! `S` throughout is the host's parsed-stylesheet type: this crate names no
//! style type, so a registered stylesheet is whatever the host parsed.
//! [`Register`] is what a registration is written through, and
//! [`RegisterInBackground`] a host that can also run one off the calling
//! thread; a source parser writes through these and names no fetcher.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::Bytes;
use rustc_hash::FxHashMap;
use url::Url;

use crate::mime::MediaType;

#[derive(Debug)]
pub enum Registered<S> {
    Bytes {
        bytes: Bytes,
        media_type: Option<MediaType>,
    },
    StyleSheet(Arc<S>),
}

impl<S> Clone for Registered<S> {
    fn clone(&self) -> Self {
        match self {
            Self::Bytes { bytes, media_type } => Self::Bytes {
                bytes: bytes.clone(),
                media_type: media_type.clone(),
            },
            Self::StyleSheet(sheet) => Self::StyleSheet(Arc::clone(sheet)),
        }
    }
}

#[derive(Debug)]
pub struct Registry<S> {
    entries: Mutex<FxHashMap<String, Registered<S>>>,
}

impl<S> Default for Registry<S> {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
        }
    }
}

impl<S> Registry<S> {
    pub fn insert(&self, url: &Url, registered: Registered<S>) {
        self.lock().insert(url.to_string(), registered);
    }

    #[must_use]
    pub fn get(&self, url: &Url) -> Option<Registered<S>> {
        self.lock().get(url.as_str()).cloned()
    }

    #[must_use]
    pub fn contains(&self, url: &Url) -> bool {
        self.lock().contains_key(url.as_str())
    }

    pub fn remove(&self, url: &Url) -> Option<Registered<S>> {
        self.lock().remove(url.as_str())
    }

    pub fn clear(&self) {
        self.lock().clear();
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> MutexGuard<'_, FxHashMap<String, Registered<S>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    #[error("`{url}` is not a valid URL: {message}")]
    InvalidUrl { url: String, message: String },
}

pub fn parse_registration_url(url: &str) -> Result<Url, RegisterError> {
    Url::parse(url).map_err(|error| RegisterError::InvalidUrl {
        url: url.to_owned(),
        message: error.to_string(),
    })
}

pub trait Register<S> {
    fn register(
        &self,
        url: &str,
        bytes: Bytes,
        media_type: Option<&str>,
    ) -> Result<Url, RegisterError>;

    fn register_style_sheet(&self, url: &str, sheet: Arc<S>) -> Result<Url, RegisterError>;
}

pub struct Registrar<S> {
    registry: Arc<Registry<S>>,
}

impl<S> fmt::Debug for Registrar<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Registrar").finish_non_exhaustive()
    }
}

impl<S> Registrar<S> {
    #[must_use]
    pub fn new(registry: Arc<Registry<S>>) -> Self {
        Self { registry }
    }

    pub fn register(
        &self,
        url: &str,
        bytes: impl Into<Bytes>,
        media_type: Option<&str>,
    ) -> Result<Url, RegisterError> {
        let url = parse_registration_url(url)?;
        self.registry.insert(
            &url,
            Registered::Bytes {
                bytes: bytes.into(),
                media_type: media_type.and_then(MediaType::parse),
            },
        );
        Ok(url)
    }

    pub fn register_style_sheet(&self, url: &str, sheet: Arc<S>) -> Result<Url, RegisterError> {
        let url = parse_registration_url(url)?;
        self.registry.insert(&url, Registered::StyleSheet(sheet));
        Ok(url)
    }
}

impl<S> Register<S> for Registrar<S> {
    fn register(
        &self,
        url: &str,
        bytes: Bytes,
        media_type: Option<&str>,
    ) -> Result<Url, RegisterError> {
        Self::register(self, url, bytes, media_type)
    }

    fn register_style_sheet(&self, url: &str, sheet: Arc<S>) -> Result<Url, RegisterError> {
        Self::register_style_sheet(self, url, sheet)
    }
}

pub trait ContainerInstaller<S>: fmt::Debug + Send + Sync + 'static {
    fn install(&self, url: &Url, bytes: &[u8], registrar: &Registrar<S>) -> Result<bool, String>;
}

pub trait RegisterInBackground<S>: Register<S> {
    fn register_in_background(
        &self,
        container: &Url,
        install: impl FnOnce(&Registrar<S>) -> Result<(), String> + Send + 'static,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Sheet;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("a URL")
    }

    #[test]
    fn a_registration_is_found_under_its_normalized_url_and_replaced_by_a_later_one() {
        let registrar = Registrar::new(Arc::new(Registry::<Sheet>::default()));
        let registered = registrar
            .register("APP://host/a.js", b"1".as_slice(), Some("text/javascript"))
            .expect("a valid URL");
        assert_eq!(registered.as_str(), "app://host/a.js");
        registrar
            .register("app://host/a.js", b"2".as_slice(), None)
            .expect("a valid URL");

        let Some(Registered::Bytes { bytes, media_type }) =
            registrar.registry.get(&url("app://host/a.js"))
        else {
            panic!("the bytes are registered");
        };
        assert_eq!(&bytes[..], b"2");
        assert!(media_type.is_none(), "the later registration replaced it");
        assert_eq!(registrar.registry.len(), 1);
    }

    #[test]
    fn a_style_sheet_is_registered_as_the_host_parsed_it() {
        let registry = Arc::new(Registry::<Sheet>::default());
        let sheet = Arc::new(Sheet);
        Register::register_style_sheet(
            &Registrar::new(Arc::clone(&registry)),
            "app://host/a.css",
            Arc::clone(&sheet),
        )
        .expect("a valid URL");
        let Some(Registered::StyleSheet(found)) = registry.get(&url("app://host/a.css")) else {
            panic!("the sheet is registered");
        };
        assert!(Arc::ptr_eq(&found, &sheet));
    }

    #[test]
    fn removing_and_clearing_forget_registrations() {
        let registry = Registry::<Sheet>::default();
        for name in ["a", "b"] {
            registry.insert(
                &url(&format!("app://host/{name}")),
                Registered::Bytes {
                    bytes: Bytes::new(),
                    media_type: None,
                },
            );
        }
        assert!(registry.remove(&url("app://host/a")).is_some());
        assert!(!registry.contains(&url("app://host/a")));
        assert!(registry.contains(&url("app://host/b")));
        registry.clear();
        assert!(registry.is_empty());
    }

    #[test]
    fn a_url_that_does_not_parse_is_not_registered() {
        let registrar = Registrar::new(Arc::new(Registry::<Sheet>::default()));
        assert!(matches!(
            registrar.register("not a url", Bytes::new(), None),
            Err(RegisterError::InvalidUrl { .. })
        ));
        assert!(registrar.registry.is_empty());
    }
}
