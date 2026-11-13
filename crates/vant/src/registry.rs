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
//! Module bytecode is registered as the host hands it over
//! ([`Registered::ModuleBytecode`]): this crate never reads it.
//! [`Register`] is what a registration is written through, and
//! [`RegisterInBackground`] a host that can also run one off the calling
//! thread; a source parser writes through these and names no fetcher.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::Bytes;
use rustc_hash::FxHashMap;
use url::Url;

use crate::mime::MediaType;

/// One registered resource.
#[derive(Debug)]
pub enum Registered<S> {
    /// Bytes with the media type they were registered under, if any; an
    /// absent type is sniffed like a response without a `Content-Type`.
    Bytes {
        bytes: Bytes,
        media_type: Option<MediaType>,
    },
    /// A stylesheet the host already parsed; it answers only a stylesheet
    /// source request, since it has no bytes to give any other one.
    StyleSheet(Arc<S>),
    /// Module bytecode of the host's script engine. It answers only a module
    /// source request; every other request for its URL fails, as for
    /// [`Self::StyleSheet`]. Only a registration makes one: no response is
    /// ever read as bytecode.
    ModuleBytecode(Bytes),
}

impl<S> Clone for Registered<S> {
    fn clone(&self) -> Self {
        match self {
            Self::Bytes { bytes, media_type } => Self::Bytes {
                bytes: bytes.clone(),
                media_type: media_type.clone(),
            },
            Self::StyleSheet(sheet) => Self::StyleSheet(Arc::clone(sheet)),
            Self::ModuleBytecode(bytes) => Self::ModuleBytecode(bytes.clone()),
        }
    }
}

/// The registered contents, keyed by normalized URL. Read from the fetcher's
/// blocking-pool threads, which serve image and source loads from it, so it
/// locks rather than borrows.
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
    /// Registers `registered` under `url`, replacing any earlier
    /// registration.
    pub fn insert(&self, url: &Url, registered: Registered<S>) {
        self.lock().insert(url.to_string(), registered);
    }

    /// What is registered under `url`. A clone: the bytes and the sheet are
    /// both reference-counted.
    #[must_use]
    pub fn get(&self, url: &Url) -> Option<Registered<S>> {
        self.lock().get(url.as_str()).cloned()
    }

    /// Whether anything is registered under `url`.
    #[must_use]
    pub fn contains(&self, url: &Url) -> bool {
        self.lock().contains_key(url.as_str())
    }

    /// Forgets the registration of `url` and returns it.
    pub fn remove(&self, url: &Url) -> Option<Registered<S>> {
        self.lock().remove(url.as_str())
    }

    /// Forgets every registration.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// How many URLs are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no URL is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> MutexGuard<'_, FxHashMap<String, Registered<S>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A URL could not be registered.
#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    #[error("`{url}` is not a valid URL: {message}")]
    InvalidUrl { url: String, message: String },
}

/// The normalization every registration goes through: the URL as
/// [`Url::parse`] writes it, which is the key it is registered and looked up
/// under.
///
/// # Errors
///
/// [`RegisterError::InvalidUrl`] for a URL that does not parse.
pub fn parse_registration_url(url: &str) -> Result<Url, RegisterError> {
    Url::parse(url).map_err(|error| RegisterError::InvalidUrl {
        url: url.to_owned(),
        message: error.to_string(),
    })
}

/// Something contents can be registered into: the reference fetcher's
/// resource system, or a [`Registrar`] onto its registry.
pub trait Register<S> {
    /// Registers `bytes` under `url`, replacing any earlier registration.
    /// `media_type` labels them the way a `Content-Type` would; without it
    /// they are sniffed. Returns the normalized URL they answer to.
    ///
    /// # Errors
    ///
    /// [`RegisterError::InvalidUrl`] for a URL that does not parse.
    fn register(
        &self,
        url: &str,
        bytes: Bytes,
        media_type: Option<&str>,
    ) -> Result<Url, RegisterError>;

    /// Registers a stylesheet the host already parsed. It answers a
    /// stylesheet source request pre-parsed, and no other request at all.
    ///
    /// # Errors
    ///
    /// [`RegisterError::InvalidUrl`] for a URL that does not parse.
    fn register_style_sheet(&self, url: &str, sheet: Arc<S>) -> Result<Url, RegisterError>;

    /// Registers module bytecode of the host's script engine under `url`,
    /// replacing any earlier registration. It answers a module source
    /// request, and no other request at all.
    ///
    /// # Errors
    ///
    /// [`RegisterError::InvalidUrl`] for a URL that does not parse.
    fn register_module_bytecode(&self, url: &str, bytes: Bytes) -> Result<Url, RegisterError>;
}

/// What a [`ContainerInstaller`] registers through: the registry alone,
/// reachable from the thread the load finished on.
///
/// It is the resource system's own registration minus the per-scope
/// stylesheet-cache invalidation, which is thread-bound and has nothing to
/// invalidate here: a container's section URLs were never asked for before
/// the container existed, so no cached response for one can be standing.
pub struct Registrar<S> {
    registry: Arc<Registry<S>>,
}

impl<S> fmt::Debug for Registrar<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Registrar").finish_non_exhaustive()
    }
}

impl<S> Registrar<S> {
    /// A registrar onto `registry`.
    #[must_use]
    pub fn new(registry: Arc<Registry<S>>) -> Self {
        Self { registry }
    }

    /// Registers `bytes` under `url`, replacing any earlier registration.
    /// Returns the normalized URL they answer to.
    ///
    /// # Errors
    ///
    /// [`RegisterError::InvalidUrl`] for a URL that does not parse.
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

    /// Registers a stylesheet the installer already parsed. It answers a
    /// stylesheet source request pre-parsed, and no other request at all.
    ///
    /// # Errors
    ///
    /// [`RegisterError::InvalidUrl`] for a URL that does not parse.
    pub fn register_style_sheet(&self, url: &str, sheet: Arc<S>) -> Result<Url, RegisterError> {
        let url = parse_registration_url(url)?;
        self.registry.insert(&url, Registered::StyleSheet(sheet));
        Ok(url)
    }

    /// Registers module bytecode of the host's script engine under `url`,
    /// replacing any earlier registration. It answers a module source
    /// request, and no other request at all.
    ///
    /// # Errors
    ///
    /// [`RegisterError::InvalidUrl`] for a URL that does not parse.
    pub fn register_module_bytecode(
        &self,
        url: &str,
        bytes: impl Into<Bytes>,
    ) -> Result<Url, RegisterError> {
        let url = parse_registration_url(url)?;
        self.registry
            .insert(&url, Registered::ModuleBytecode(bytes.into()));
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

    fn register_module_bytecode(&self, url: &str, bytes: Bytes) -> Result<Url, RegisterError> {
        Self::register_module_bytecode(self, url, bytes)
    }
}

/// The hook that makes a fetched Lynx container's sections loadable.
///
/// A plain fetch is one where the engine asks for a URL and learns only that
/// the fetch is over. What the bytes *were* is the fetcher's and its host's
/// business, and this is where that decision is made: if they are a Lynx
/// container, its bodies and stylesheets are registered so that the source
/// requests a realm makes for them by URL answer. Deciding *what* the bytes
/// are is the source parser's, so what is here is the fetch, the URL an
/// install is based on, and the registry it writes.
///
/// A host without one still fetches; nothing about the completion changes.
///
/// It runs on whichever thread the load finished on — a blocking-pool thread
/// natively — which is why it is `Send + Sync`, and why what it writes
/// through is a [`Registrar`] rather than the resource system itself.
pub trait ContainerInstaller<S>: fmt::Debug + Send + Sync + 'static {
    /// Offers the bytes fetched from `url`. `url` is the **resolved request
    /// URL**, which is what a realm derives its section URLs from; a redirect
    /// is the transport's business and no part of it.
    ///
    /// `Ok(false)` is "these are not a container I recognize": the fetch
    /// completes and nothing was registered, which is what an image or a
    /// script fetched this way leaves. `Ok(true)` is installed. An error is a
    /// container that would not decode, which **fails the fetch** with that
    /// message.
    ///
    /// # Errors
    ///
    /// A container that would not decode or register.
    fn install(&self, url: &Url, bytes: &[u8], registrar: &Registrar<S>) -> Result<bool, String>;
}

/// A host that can run an install off the calling thread, and hold every
/// source request for a URL under the container until it has run.
pub trait RegisterInBackground<S>: Register<S> {
    /// Runs `install` off this thread with a [`Registrar`] onto this host's
    /// registry, and holds every source request for a URL under `container`
    /// until it has run: such a request is then answered from what it
    /// registered, or fails with the message it failed with. A URL is under
    /// `container` when, its query and fragment aside, it is the container
    /// URL's path or below it.
    fn register_in_background(
        &self,
        container: &Url,
        install: impl FnOnce(&Registrar<S>) -> Result<(), String> + Send + 'static,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the host's stylesheet type.
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
    fn module_bytecode_is_registered_as_the_host_handed_it_over() {
        let registry = Arc::new(Registry::<Sheet>::default());
        let registered = Register::register_module_bytecode(
            &Registrar::new(Arc::clone(&registry)),
            "APP://host/a.js",
            Bytes::from_static(b"module bytecode"),
        )
        .expect("a valid URL");
        assert_eq!(registered.as_str(), "app://host/a.js");
        let Some(Registered::ModuleBytecode(found)) = registry.get(&url("app://host/a.js")) else {
            panic!("the module bytecode is registered");
        };
        assert_eq!(&found[..], b"module bytecode");
    }

    #[test]
    fn module_bytecode_replaces_and_is_replaced_by_other_registrations() {
        let registrar = Registrar::new(Arc::new(Registry::<Sheet>::default()));
        registrar
            .register("app://host/a.js", b"text".as_slice(), None)
            .expect("a valid URL");
        registrar
            .register_module_bytecode("app://host/a.js", b"bytecode".as_slice())
            .expect("a valid URL");
        assert!(matches!(
            registrar.registry.get(&url("app://host/a.js")),
            Some(Registered::ModuleBytecode(_))
        ));
        registrar
            .register_style_sheet("app://host/a.js", Arc::new(Sheet))
            .expect("a valid URL");
        assert!(matches!(
            registrar.registry.get(&url("app://host/a.js")),
            Some(Registered::StyleSheet(_))
        ));
        assert_eq!(registrar.registry.len(), 1);
        assert!(matches!(
            registrar.register_module_bytecode("not a url", Bytes::new()),
            Err(RegisterError::InvalidUrl { .. })
        ));
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
