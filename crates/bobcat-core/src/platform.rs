//! Host-injected native text-interaction contracts.
//!
//! Bobcat owns the semantic text and selection state. An embedder implements
//! these protocols to project that state onto its OS clipboard, text-input
//! service, and selection chrome. The contracts deliberately contain no
//! `winit`, `UIKit`, `AppKit`, Android, Windows, or desktop-toolkit vocabulary.
//!
//! Providers are not required to be `Send` or `Sync`: selection UI and text
//! input normally live on the presenting thread beside the native event loop.
//! Platform callbacks travel in the other direction as [`TextInputEvent`] and
//! [`SelectionUiEvent`] values; wiring those events into [`crate::engine::Engine`]
//! is intentionally outside this protocol-only module.

use std::sync::Arc;

use thiserror::Error;

/// Clipboard access supplied by an embedder.
///
/// Reads should happen only in response to an explicit paste action. A
/// provider may retain or clone written content when its OS uses delayed
/// clipboard rendering.
pub trait ClipboardProvider {
    /// Reads the best available representation requested by `request`.
    fn read(
        &mut self,
        request: &ClipboardReadRequest,
    ) -> Result<Option<ClipboardContent>, PlatformTextError>;

    /// Replaces the current system clipboard contents.
    fn write(&mut self, content: &ClipboardContent) -> Result<(), PlatformTextError>;
}

/// Native editable-text service supplied by an embedder.
///
/// A provider maps sessions to facilities such as `UITextInput`,
/// `NSTextInputClient`, Android `InputConnection`, or Windows TSF. All text
/// offsets crossing this boundary are UTF-16 code-unit offsets.
pub trait TextInputProvider {
    /// Opens one focused text-input session with its complete initial state.
    fn open(&mut self, session: &TextInputSession) -> Result<(), PlatformTextError>;

    /// Publishes a new text, selection, or composition snapshot.
    fn update_state(
        &mut self,
        session: TextSessionId,
        state: &TextInputState,
    ) -> Result<(), PlatformTextError>;

    /// Publishes geometry after layout, scrolling, or a window transform.
    fn update_geometry(
        &mut self,
        session: TextSessionId,
        geometry: &TextInputGeometry,
    ) -> Result<(), PlatformTextError>;

    /// Closes the session if it is still current.
    fn close(&mut self, session: TextSessionId) -> Result<(), PlatformTextError>;
}

/// Native selection chrome supplied by an embedder.
///
/// On iOS this can be backed by system text interaction. Other embedders may
/// use native edit menus while drawing selection handles themselves. The
/// semantic selection remains owned by Bobcat in either case.
pub trait SelectionUiProvider {
    /// Presents or updates the chrome for one selection snapshot.
    fn update(&mut self, state: &SelectionUiState) -> Result<(), PlatformTextError>;

    /// Dismisses chrome belonging to `session`, leaving stale dismissals inert.
    fn dismiss(&mut self, session: TextSessionId) -> Result<(), PlatformTextError>;
}

/// One logical text-interaction session.
///
/// Identifiers are allocated by Bobcat and are never interpreted by the
/// provider. Reusing an identifier while an older session can still emit
/// callbacks is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextSessionId(pub u64);

/// A point in the view's logical coordinate space, with a top-left origin.
///
/// The embedder converts this to its native view or screen coordinate space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewPoint {
    pub x: f32,
    pub y: f32,
}

/// A rectangle in the view's logical coordinate space, with a top-left origin.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// One ordered half-open range measured in UTF-16 code units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextRange {
    pub start: u32,
    pub end: u32,
}

impl TextRange {
    /// Builds a range with endpoints normalized into document order.
    #[must_use]
    pub const fn ordered(first: u32, second: u32) -> Self {
        if first <= second {
            Self {
                start: first,
                end: second,
            }
        } else {
            Self {
                start: second,
                end: first,
            }
        }
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// Direction-preserving selection endpoints measured in UTF-16 code units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextSelection {
    pub anchor: u32,
    pub focus: u32,
}

impl TextSelection {
    #[must_use]
    pub const fn range(self) -> TextRange {
        TextRange::ordered(self.anchor, self.focus)
    }

    #[must_use]
    pub const fn is_collapsed(self) -> bool {
        self.anchor == self.focus
    }
}

/// Clipboard representations Bobcat can currently exchange with a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClipboardFormat {
    PlainText,
    Html,
}

/// Ordered clipboard format preferences for one explicit read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardReadRequest {
    /// Formats in descending preference order.
    pub formats: Vec<ClipboardFormat>,
}

impl ClipboardReadRequest {
    #[must_use]
    pub fn new(formats: impl IntoIterator<Item = ClipboardFormat>) -> Self {
        Self {
            formats: formats.into_iter().collect(),
        }
    }

    /// Requests only the baseline format used by Lynx text and form controls.
    #[must_use]
    pub fn plain_text() -> Self {
        Self::new([ClipboardFormat::PlainText])
    }
}

/// Normalized clipboard data independent of platform encodings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardContent {
    pub plain_text: Option<Arc<str>>,
    pub html: Option<Arc<str>>,
}

impl ClipboardContent {
    #[must_use]
    pub fn plain_text(text: impl Into<Arc<str>>) -> Self {
        Self {
            plain_text: Some(text.into()),
            html: None,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plain_text.is_none() && self.html.is_none()
    }
}

/// Platform keyboard and editing behavior requested for a focused control.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "these are independent native text-service capability switches"
)]
pub struct TextInputConfiguration {
    pub input_type: TextInputType,
    pub action: TextInputAction,
    pub capitalization: TextCapitalization,
    pub multiline: bool,
    pub secure: bool,
    pub autocorrect: bool,
    pub spellcheck: bool,
}

impl Default for TextInputConfiguration {
    fn default() -> Self {
        Self {
            input_type: TextInputType::Text,
            action: TextInputAction::Default,
            capitalization: TextCapitalization::None,
            multiline: false,
            secure: false,
            autocorrect: true,
            spellcheck: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextInputType {
    #[default]
    Text,
    Password,
    Email,
    Url,
    Number,
    Phone,
    Search,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextInputAction {
    #[default]
    Default,
    None,
    Done,
    Go,
    Next,
    Previous,
    Search,
    Send,
    Newline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextCapitalization {
    #[default]
    None,
    Characters,
    Words,
    Sentences,
}

/// Versioned editable text mirrored to the native text service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextInputState {
    /// Changes whenever Bobcat publishes a new authoritative state.
    pub revision: u64,
    pub text: Arc<str>,
    pub selection: TextSelection,
    /// Ordered range in `text`, or `None` when no composition is active.
    pub composing: Option<TextRange>,
}

/// View geometry needed by an OS input method and candidate UI.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextInputGeometry {
    pub editor_rect: ViewRect,
    pub caret_rect: ViewRect,
    pub composing_rects: Vec<ViewRect>,
}

/// Complete state used to open a native text-input session atomically.
#[derive(Clone, Debug, PartialEq)]
pub struct TextInputSession {
    pub id: TextSessionId,
    pub configuration: TextInputConfiguration,
    pub state: TextInputState,
    pub geometry: TextInputGeometry,
}

/// A platform text-service callback for a currently open session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextInputEvent {
    pub session: TextSessionId,
    /// The authoritative revision on which the platform based this event.
    pub revision: u64,
    pub kind: TextInputEventKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TextInputEventKind {
    /// Replaces the current composing text. `selection` is relative to
    /// `text`, in UTF-16 code units; `None` asks the engine to hide the caret.
    Preedit {
        text: Arc<str>,
        selection: Option<TextSelection>,
    },
    Commit(Arc<str>),
    DeleteSurrounding {
        before: u32,
        after: u32,
    },
    SetSelection(TextSelection),
    PerformAction(TextInputAction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SelectionUiMode {
    ReadOnly,
    Editable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextDirection {
    LeftToRight,
    RightToLeft,
}

/// Geometry for one direction-preserving endpoint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionEndpointGeometry {
    pub rect: ViewRect,
    pub direction: TextDirection,
}

/// One shaped cluster exposed to native text interaction.
///
/// `range` indexes [`SelectionUiState::text`] in UTF-16 code units. Together
/// the text and cluster geometry let an adapter answer native synchronous
/// queries such as "closest text position to this point" without borrowing
/// the mutable DOM or waiting for a script batch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextClusterGeometry {
    pub range: TextRange,
    pub rect: ViewRect,
    pub direction: TextDirection,
}

/// Standard actions the native selection UI may expose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "these booleans are an explicit set of independently enabled OS actions"
)]
pub struct SelectionActions {
    pub copy: bool,
    pub cut: bool,
    pub paste: bool,
    pub select_all: bool,
}

/// Immutable selection geometry and capabilities published after a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionUiState {
    pub session: TextSessionId,
    /// Identifies the retained layout frame that produced this geometry.
    pub epoch: u64,
    pub mode: SelectionUiMode,
    /// Flattened selectable text represented by this retained frame.
    pub text: Arc<str>,
    /// Direction-preserving endpoints within `text`, in UTF-16 code units.
    pub selection: TextSelection,
    /// Cluster-to-view mapping used by native text-range queries.
    pub clusters: Vec<TextClusterGeometry>,
    pub selection_rects: Vec<ViewRect>,
    pub anchor: SelectionEndpointGeometry,
    pub focus: SelectionEndpointGeometry,
    pub menu_anchor_rect: ViewRect,
    pub actions: SelectionActions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SelectionEndpoint {
    Anchor,
    Focus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SelectionGranularity {
    Character,
    Word,
    Line,
    Paragraph,
    Document,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SelectionAction {
    Copy,
    Cut,
    Paste,
    SelectAll,
}

/// A native selection gesture or menu action returned by an embedder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionUiEvent {
    pub session: TextSessionId,
    /// The retained layout epoch on which the interaction began.
    pub epoch: u64,
    pub kind: SelectionUiEventKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum SelectionUiEventKind {
    SelectAt {
        point: ViewPoint,
        granularity: SelectionGranularity,
    },
    MoveEndpoint {
        endpoint: SelectionEndpoint,
        point: ViewPoint,
    },
    /// A native text interaction supplied new UTF-16 endpoints directly.
    SetSelection(TextSelection),
    Action(SelectionAction),
    Dismissed,
}

/// Sanitized failure returned by an embedder text service.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{kind:?}: {message}")]
pub struct PlatformTextError {
    pub kind: PlatformTextErrorKind,
    pub message: Arc<str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PlatformTextErrorKind {
    Unsupported,
    Unavailable,
    PermissionDenied,
    InvalidState,
    InvalidData,
    Other,
}

#[cfg(test)]
mod tests {
    use super::{TextRange, TextSelection};

    #[test]
    fn range_preserves_document_order_without_losing_selection_direction() {
        let selection = TextSelection {
            anchor: 9,
            focus: 3,
        };

        assert_eq!(selection.range(), TextRange { start: 3, end: 9 });
        assert!(!selection.is_collapsed());
        assert!(TextRange::ordered(4, 4).is_empty());
    }
}
