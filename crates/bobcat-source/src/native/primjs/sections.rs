//! The bytecode payloads a `.lynx.bundle` carries.

/// Where in a bundle a bytecode payload was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionKind {
    /// The `ROOT_LEPUS` section: the main-thread script.
    RootLepus,
    /// One entry of the `LEPUS_CHUNK` section.
    LepusChunk,
    /// One file of the `JS_BYTECODE` section. The payload is packed (see
    /// [`super::unwrap_packed`]).
    JsBytecode,
    /// A custom section whose encoding is `JsBytecode`. tasm 0.0.53 writes
    /// it with the main-thread context's writer, without a packed header.
    CustomSection,
}

/// One bytecode payload of a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeSection<'a> {
    /// Kind of section.
    pub kind: SectionKind,
    /// `ROOT_LEPUS` for the root section; the chunk name, file path or
    /// custom-section name otherwise.
    pub name: String,
    /// Absolute offset of `bytes` in the bundle.
    pub offset: usize,
    /// The payload as stored: `PrimJS` bytecode, or a packed buffer for
    /// [`SectionKind::JsBytecode`].
    pub bytes: &'a [u8],
}
