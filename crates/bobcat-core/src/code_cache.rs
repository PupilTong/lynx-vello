//! The code cache, as a realm sees it: which compiles look bytecode up in
//! [`vant::artifact::Artifacts`], under which key, and where the bytecode of
//! a compile that missed goes (`docs/resource-cache-design.md`, "PR2: `QuickJS`
//! code cache").
//!
//! Every rule about keys is here, so the realm that reads an artifact and
//! the resource system that preloads one agree on them:
//!
//! - The source is the [`SourceDigest`] of exactly the text the realm is handed.
//! - A module's variant is its name — the name it is completed or required under, which its stack
//!   frames and its module record bake in — and whether the runtime strips source text. Its
//!   response URL, which is its `import.meta.url` and its imports' base, is set after the read and
//!   is no part of the key, so one artifact serves every realm that answered the same text under
//!   the same name from anywhere.
//! - A `CommonJS` wrapper's variant is the response URL it is compiled under, the wrapper's
//!   parameter list, and the strip flag.
//! - The engine is [`script_engine_id`].
//!
//! A text under [`MIN_CACHED_SOURCE_BYTES`] is compiled as it always was and
//! never hashed, and so is every text while the memory tier's budget is 0.
//!
//! The lookup is in memory only and synchronous, inside the call that would
//! compile; the resource system preloads what its Store holds for a text
//! before it delivers that text. A miss compiles the text and asks the
//! bridge to keep the bytecode; the realm hands what it kept to
//! [`RealmCodeCache::insert`] at the end of the operation that compiled it,
//! which queues it for the resource system to persist.
//!
//! The bridge names a compile by what it was compiled under, not by its
//! text, and a module is compiled when it is first linked, which can be
//! operations after its text was handed over. So the realm remembers, per
//! name, the digest of the text it handed over, and files a compile under
//! that digest only while every text handed over under the name was the
//! same one ([`Expected`]): bytecode is never filed under a text it might
//! not be the compile of.

use std::cell::RefCell;
use std::sync::Arc;

use bytes::Bytes;
use quickjs_rust_bridge as quickjs;
use rustc_hash::FxHashMap;
use vant::artifact::{ArtifactKey, Artifacts, EngineId, SourceDigest, Variant};

/// The [`EngineId`] the script engine's bytecode is filed under: the bridge's
/// [`bytecode_engine_id`](quickjs::bytecode_engine_id), which names this
/// `QuickJS` build, its bytecode format, pointer width and endianness.
#[must_use]
pub fn script_engine_id() -> EngineId {
    EngineId(quickjs::bytecode_engine_id())
}

/// The shortest script text the code cache keeps bytecode for. Shorter texts
/// compile in less time than hashing and a lookup save.
pub const MIN_CACHED_SOURCE_BYTES: usize = 1024;

/// The key of the bytecode of `source` compiled as an ES module named `name`.
fn module_key(source: SourceDigest, strip: bool, name: &str) -> ArtifactKey {
    ArtifactKey {
        source,
        variant: Variant::of(&[b"module", &[u8::from(strip)], name.as_bytes()]),
    }
}

/// The key of the bytecode of `source` compiled as a `CommonJS` wrapper
/// with `parameters`, under the response URL `url`.
fn commonjs_key(source: SourceDigest, strip: bool, url: &str, parameters: &str) -> ArtifactKey {
    ArtifactKey {
        source,
        variant: Variant::of(&[
            b"commonjs",
            &[u8::from(strip)],
            url.as_bytes(),
            parameters.as_bytes(),
        ]),
    }
}

/// What a realm completes one module's text with: the bridge's
/// [`quickjs::ModuleCode`], owning the bytecode it lends.
pub(crate) enum ModuleCode {
    /// Not cached: compile as without a cache.
    Compile,
    /// A miss: compile, and keep the bytecode.
    Capture,
    /// A hit: read this bytecode.
    Cached(Bytes),
}

impl ModuleCode {
    /// The bridge's form, lending the bytecode.
    pub(crate) fn as_bridge(&self) -> quickjs::ModuleCode<'_> {
        match self {
            Self::Compile => quickjs::ModuleCode::Compile,
            Self::Capture => quickjs::ModuleCode::Capture,
            Self::Cached(bytecode) => quickjs::ModuleCode::Cached(bytecode),
        }
    }
}

/// What the texts a realm handed the bridge under one name agree on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    /// Every text handed over under the name had this digest, so a compile
    /// under the name is a compile of that text.
    Text(SourceDigest),
    /// Two different texts were handed over under the name. Which of them a
    /// compile under it compiled is not known here, so none is filed: the
    /// realm compiles as it would without a cache, and another realm files
    /// the bytecode.
    Ambiguous,
}

impl Expected {
    /// Records one more text handed over under `name`.
    fn hand_over<K: std::hash::Hash + Eq>(
        names: &mut FxHashMap<K, Self>,
        name: K,
        source: SourceDigest,
    ) {
        names
            .entry(name)
            .and_modify(|expected| {
                if *expected != Self::Text(source) {
                    *expected = Self::Ambiguous;
                }
            })
            .or_insert(Self::Text(source));
    }
}

/// The names a realm handed texts to the bridge under, with their bytecode
/// wanted. An entry stays for the realm's life: there is one per module name
/// and per required file, as in the realm's own module registry.
#[derive(Default)]
struct HandedOver {
    /// By the name a module compile is kept under.
    modules: FxHashMap<String, Expected>,
    /// By the response URL and parameter list a wrapper compile is kept
    /// under.
    wrappers: FxHashMap<(String, String), Expected>,
}

/// One realm's side of the code cache: the shared memory tier, and what the
/// texts it handed the bridge with a capture asked for are to be filed under.
///
/// Shared by the realm's [`ScriptEngine`](crate::main::quickjs::ScriptEngine)
/// and its synchronous loader, and never leaves the realm's thread.
pub(crate) struct RealmCodeCache {
    artifacts: Arc<Artifacts>,
    engine: EngineId,
    /// Whether the realm's runtime strips source text, which changes what a
    /// function's `toString` answers and so is part of every key.
    strip: bool,
    handed_over: RefCell<HandedOver>,
}

impl RealmCodeCache {
    pub(crate) fn new(artifacts: Arc<Artifacts>, strip: bool) -> Self {
        Self {
            artifacts,
            engine: script_engine_id(),
            strip,
            handed_over: RefCell::default(),
        }
    }

    /// A cache for a realm on the same runtime: the same tier, the same
    /// strip flag, and names of its own.
    pub(crate) fn sibling(&self) -> Self {
        Self::new(Arc::clone(&self.artifacts), self.strip)
    }

    /// The digest of `text` when it is to be cached: long enough, and the
    /// tier enabled.
    fn digest(&self, text: &str) -> Option<SourceDigest> {
        (text.len() >= MIN_CACHED_SOURCE_BYTES && self.artifacts.budget() > 0)
            .then(|| SourceDigest::of(text.as_bytes()))
    }

    /// What to complete the module `name` with, when its text is `text`.
    pub(crate) fn module_code(&self, name: &str, text: &str) -> ModuleCode {
        let Some(source) = self.digest(text) else {
            return ModuleCode::Compile;
        };
        let cached = self
            .artifacts
            .get(self.engine, module_key(source, self.strip, name));
        // Either way: bytecode that turns out unreadable falls back to the
        // text, whose compile is kept and lands here.
        Expected::hand_over(
            &mut self.handed_over.borrow_mut().modules,
            name.to_owned(),
            source,
        );
        cached.map_or(ModuleCode::Capture, ModuleCode::Cached)
    }

    /// What a synchronous load of `asked` with `parameters` answers with,
    /// given the `source` it fetched: bytecode a previous compile of the
    /// same text under the same names left, and a capture of whatever is
    /// compiled.
    ///
    /// An import being linked inline is asked with no parameter list (`""`)
    /// and is always a module, so only the module key is looked up for it.
    /// JSON compiles nothing and is never hashed.
    pub(crate) fn required(
        &self,
        asked: &str,
        parameters: &str,
        source: quickjs::RequiredSource,
    ) -> quickjs::RequiredCode {
        use quickjs::RequiredKind;

        let digest = match source.kind {
            RequiredKind::Json => None,
            _ => self.digest(&source.text),
        };
        let Some(digest) = digest else {
            return source.into();
        };
        let inline = parameters.is_empty();
        let commonjs = (!inline
            && matches!(source.kind, RequiredKind::CommonJs | RequiredKind::Detect))
        .then(|| (source.url.clone(), parameters.to_owned()));
        let module = (inline || matches!(source.kind, RequiredKind::Module | RequiredKind::Detect))
            .then(|| asked.to_owned());
        let cached = commonjs
            .as_ref()
            .and_then(|(url, parameters)| {
                self.artifacts
                    .get(
                        self.engine,
                        commonjs_key(digest, self.strip, url, parameters),
                    )
                    .map(|bytes| (RequiredKind::CommonJs, bytes))
            })
            .or_else(|| {
                module.as_ref().and_then(|name| {
                    self.artifacts
                        .get(self.engine, module_key(digest, self.strip, name))
                        .map(|bytes| (RequiredKind::Module, bytes))
                })
            })
            .map(|(kind, bytes)| (kind, bytes.to_vec()));
        // A load the host could not tell the kind of names both, and the
        // compile says which it was.
        let mut handed_over = self.handed_over.borrow_mut();
        if let Some(name) = module {
            Expected::hand_over(&mut handed_over.modules, name, digest);
        }
        if let Some(wrapper) = commonjs {
            Expected::hand_over(&mut handed_over.wrappers, wrapper, digest);
        }
        drop(handed_over);
        quickjs::RequiredCode {
            source,
            cached,
            capture: true,
        }
    }

    /// Files the bytecode of one compile the realm kept under the key of the
    /// text it was compiled from, which makes it resident and queues it for
    /// persistence. A compile under a name no text was handed over for, or
    /// two different texts were ([`Expected::Ambiguous`]), is dropped.
    pub(crate) fn insert(&self, compiled: quickjs::Compiled) {
        let quickjs::Compiled {
            kind,
            name,
            bytecode,
        } = compiled;
        let key = {
            let handed_over = self.handed_over.borrow();
            match kind {
                quickjs::CompiledKind::Module => match handed_over.modules.get(&name) {
                    Some(&Expected::Text(source)) => module_key(source, self.strip, &name),
                    _ => return,
                },
                quickjs::CompiledKind::CommonJs { parameters } => {
                    let wrapper = (name, parameters);
                    match handed_over.wrappers.get(&wrapper) {
                        Some(&Expected::Text(source)) => {
                            commonjs_key(source, self.strip, &wrapper.0, &wrapper.1)
                        }
                        _ => return,
                    }
                }
            }
        };
        self.artifacts
            .insert(self.engine, key, Bytes::from(bytecode));
    }

    /// How many names texts were handed over under with their bytecode
    /// wanted.
    #[cfg(test)]
    pub(crate) fn handed_over_len(&self) -> usize {
        let handed_over = self.handed_over.borrow();
        handed_over.modules.len() + handed_over.wrappers.len()
    }

    /// The key a module compile of `text` named `name` is filed under, for
    /// tests that plant or look for one.
    #[cfg(test)]
    pub(crate) fn module_key_of(&self, name: &str, text: &str) -> ArtifactKey {
        module_key(SourceDigest::of(text.as_bytes()), self.strip, name)
    }

    /// The key a wrapper compile of `text` is filed under.
    #[cfg(test)]
    pub(crate) fn commonjs_key_of(&self, url: &str, parameters: &str, text: &str) -> ArtifactKey {
        commonjs_key(
            SourceDigest::of(text.as_bytes()),
            self.strip,
            url,
            parameters,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(tag: &str) -> String {
        format!("// {tag}\n{}", "x".repeat(MIN_CACHED_SOURCE_BYTES))
    }

    fn required(url: &str, text: &str) -> quickjs::RequiredSource {
        quickjs::RequiredSource {
            url: url.to_owned(),
            text: text.to_owned(),
            kind: quickjs::RequiredKind::CommonJs,
        }
    }

    fn module_compile(name: &str) -> quickjs::Compiled {
        quickjs::Compiled {
            kind: quickjs::CompiledKind::Module,
            name: name.to_owned(),
            bytecode: b"module bytecode".to_vec(),
        }
    }

    fn wrapper_compile(url: &str, parameters: &str) -> quickjs::Compiled {
        quickjs::Compiled {
            kind: quickjs::CompiledKind::CommonJs {
                parameters: parameters.to_owned(),
            },
            name: url.to_owned(),
            bytecode: b"wrapper bytecode".to_vec(),
        }
    }

    #[test]
    fn a_compile_is_filed_under_the_text_handed_over_under_its_name() {
        let artifacts = Arc::new(Artifacts::new(1 << 20));
        let cache = RealmCodeCache::new(Arc::clone(&artifacts), false);
        let (module, wrapper) = (text("module"), text("wrapper"));
        assert!(matches!(
            cache.module_code("app:///a.js", &module),
            ModuleCode::Capture
        ));
        let code = cache.required(
            "a.cjs",
            "exports",
            required("https://a.test/a.cjs", &wrapper),
        );
        assert!(code.capture && code.cached.is_none());

        cache.insert(module_compile("app:///a.js"));
        cache.insert(wrapper_compile("https://a.test/a.cjs", "exports"));
        // Under a name nothing was handed over for, and under another
        // parameter list.
        cache.insert(module_compile("app:///other.js"));
        cache.insert(wrapper_compile("https://a.test/a.cjs", "exports, module"));

        assert_eq!(artifacts.len(), 2);
        assert!(artifacts.contains(
            script_engine_id(),
            cache.module_key_of("app:///a.js", &module)
        ));
        assert!(artifacts.contains(
            script_engine_id(),
            cache.commonjs_key_of("https://a.test/a.cjs", "exports", &wrapper)
        ));
    }

    #[test]
    fn the_same_text_handed_over_twice_under_a_name_is_still_filed() {
        let artifacts = Arc::new(Artifacts::new(1 << 20));
        let cache = RealmCodeCache::new(Arc::clone(&artifacts), false);
        let module = text("module");
        let _ = cache.module_code("app:///a.js", &module);
        let _ = cache.module_code("app:///a.js", &module);
        assert_eq!(cache.handed_over_len(), 1);
        cache.insert(module_compile("app:///a.js"));
        assert!(artifacts.contains(
            script_engine_id(),
            cache.module_key_of("app:///a.js", &module)
        ));
    }

    #[test]
    fn a_name_handed_two_different_texts_files_no_compile() {
        let artifacts = Arc::new(Artifacts::new(1 << 20));
        let cache = RealmCodeCache::new(Arc::clone(&artifacts), false);
        let (first, second) = (text("first"), text("second"));

        let _ = cache.module_code("app:///a.js", &first);
        let _ = cache.module_code("app:///a.js", &second);
        cache.insert(module_compile("app:///a.js"));

        let _ = cache.required("a.cjs", "exports", required("https://a.test/a.cjs", &first));
        let _ = cache.required(
            "a.cjs",
            "exports",
            required("https://a.test/a.cjs", &second),
        );
        cache.insert(wrapper_compile("https://a.test/a.cjs", "exports"));

        assert!(artifacts.is_empty());
    }

    #[test]
    fn a_text_read_from_bytecode_never_lends_its_key_to_a_later_text() {
        let artifacts = Arc::new(Artifacts::new(1 << 20));
        let cache = RealmCodeCache::new(Arc::clone(&artifacts), false);
        let (first, second) = (text("first"), text("second"));
        let key = cache.commonjs_key_of("https://a.test/a.cjs", "exports", &first);
        artifacts.insert(script_engine_id(), key, Bytes::from_static(b"of the first"));

        // The first load reads its bytecode, so no compile arrives for it.
        let hit = cache.required("a.cjs", "exports", required("https://a.test/a.cjs", &first));
        assert!(hit.cached.is_some());
        // The file changed; the realm loads it again and compiles it.
        let miss = cache.required(
            "a.cjs",
            "exports",
            required("https://a.test/a.cjs", &second),
        );
        assert!(miss.cached.is_none());
        cache.insert(wrapper_compile("https://a.test/a.cjs", "exports"));

        assert_eq!(
            artifacts.get(script_engine_id(), key).as_deref(),
            Some(&b"of the first"[..])
        );
        assert_eq!(artifacts.len(), 1);
    }
}
