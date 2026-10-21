//! The browser host's editing vocabulary at the Render Worker seam
//! (`docs/editable-text-design.md`, E4).
//!
//! The facade's hidden `<textarea>` turns the page's `beforeinput` and
//! composition events into `bobcat-edit` messages; the Render Worker hands
//! each to [`BobcatRenderer::edit`](crate::BobcatRenderer) as one flat call —
//! a command name, its text and two integers — which [`edit_command`] reads
//! back into the [`Command`] `LynxView::edit` takes. The other way, a
//! [`TextInputState`](bobcat_core::editing::TextInputState) crosses as a
//! plain object whose enumerations are the HTML attribute keywords the
//! names below give.
//!
//! Compiled for the browser and for this crate's native tests, which are the
//! only native build that links `bobcat-core` here.

use bobcat_core::editing::{Command, EnterKeyHint, InputMode, InputType};

/// The [`Command`] one `bobcat-edit` message names: `kind` is the
/// `EditCommand.kind` of `js/protocol.d.ts`, and `first` and `second` carry
/// its numbers —
///
/// | `kind` | `text` | `first` | `second` |
/// |---|---|---|---|
/// | `setComposition` | the composition | selection start | selection end |
/// | `commitText` | the committed text | relative cursor | — |
/// | `finishComposition` | — | `1` keeps the selection | — |
/// | `insert` | the text | — | — |
/// | `insertLineBreak`, `deleteBackward`, `deleteForward`, `deleteWordBackward`, `deleteWordForward` | — | — | — |
///
/// The selection is in UTF-16 units relative to the start of the
/// composition, as [`Command::SetComposition`] takes it; `terminus` clamps one
/// past the text's end.
///
/// # Errors
///
/// A name this table does not hold, a command that needs text and has none,
/// or a selection that is negative or reversed: each is a facade that sent
/// something its protocol does not describe.
pub(crate) fn edit_command(
    kind: &str,
    text: Option<String>,
    first: i32,
    second: i32,
) -> Result<Command, String> {
    let required =
        |text: Option<String>| text.ok_or_else(|| format!("bobcat-edit {kind} needs text"));
    Ok(match kind {
        "setComposition" => {
            let (Ok(start), Ok(end)) = (usize::try_from(first), usize::try_from(second)) else {
                return Err(format!(
                    "bobcat-edit setComposition selection {first}..{second} is negative"
                ));
            };
            if start > end {
                return Err(format!(
                    "bobcat-edit setComposition selection {first}..{second} is reversed"
                ));
            }
            Command::SetComposition {
                text: required(text)?,
                selection: start..end,
            }
        }
        "commitText" => Command::CommitText {
            text: required(text)?,
            relative_cursor: first,
        },
        "finishComposition" => Command::FinishComposition {
            keep_selection: first != 0,
        },
        "insert" => Command::Insert(required(text)?),
        "insertLineBreak" => Command::InsertLineBreak,
        "deleteBackward" => Command::DeleteBackward,
        "deleteForward" => Command::DeleteForward,
        "deleteWordBackward" => Command::DeleteWordBackward,
        "deleteWordForward" => Command::DeleteWordForward,
        _ => return Err(format!("unknown bobcat-edit command {kind:?}")),
    })
}

/// HTML's `inputmode` keyword for `mode`, or `""` for the default (the
/// attribute absent).
#[must_use]
pub(crate) const fn input_mode_name(mode: InputMode) -> &'static str {
    match mode {
        InputMode::Default => "",
        InputMode::None => "none",
        InputMode::Text => "text",
        InputMode::Decimal => "decimal",
        InputMode::Numeric => "numeric",
        InputMode::Tel => "tel",
        InputMode::Search => "search",
        InputMode::Email => "email",
        InputMode::Url => "url",
    }
}

/// HTML's `enterkeyhint` keyword for `hint`, or `""` for the default (the
/// attribute absent).
#[must_use]
pub(crate) const fn enter_key_hint_name(hint: EnterKeyHint) -> &'static str {
    match hint {
        EnterKeyHint::Default => "",
        EnterKeyHint::Enter => "enter",
        EnterKeyHint::Done => "done",
        EnterKeyHint::Go => "go",
        EnterKeyHint::Next => "next",
        EnterKeyHint::Previous => "previous",
        EnterKeyHint::Search => "search",
        EnterKeyHint::Send => "send",
    }
}

/// HTML's `<input type>` keyword for `kind`. `InputType` is non-exhaustive:
/// a kind a later engine adds is reported as `"text"` until it is named here.
#[must_use]
pub(crate) const fn input_type_name(kind: InputType) -> &'static str {
    match kind {
        InputType::Password => "password",
        InputType::Search => "search",
        InputType::Email => "email",
        InputType::Number => "number",
        InputType::Tel => "tel",
        InputType::Url => "url",
        _ => "text",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_edit_command_reads_back_as_its_command() {
        let read = |kind, text: Option<&str>, first, second| {
            edit_command(kind, text.map(str::to_owned), first, second)
        };
        assert_eq!(
            read("setComposition", Some("かな"), 1, 2),
            Ok(Command::SetComposition {
                text: "かな".to_owned(),
                selection: 1..2,
            })
        );
        assert_eq!(
            read("setComposition", Some(""), 0, 0),
            Ok(Command::SetComposition {
                text: String::new(),
                selection: 0..0,
            }),
            "an empty composition is the cancellation dom defines"
        );
        assert_eq!(
            read("commitText", Some("仮名"), -1, 0),
            Ok(Command::CommitText {
                text: "仮名".to_owned(),
                relative_cursor: -1,
            })
        );
        assert_eq!(
            read("finishComposition", None, 0, 0),
            Ok(Command::FinishComposition {
                keep_selection: false
            })
        );
        assert_eq!(
            read("finishComposition", None, 1, 0),
            Ok(Command::FinishComposition {
                keep_selection: true
            })
        );
        assert_eq!(
            read("insert", Some("x"), 0, 0),
            Ok(Command::Insert("x".to_owned()))
        );
        for (kind, command) in [
            ("insertLineBreak", Command::InsertLineBreak),
            ("deleteBackward", Command::DeleteBackward),
            ("deleteForward", Command::DeleteForward),
            ("deleteWordBackward", Command::DeleteWordBackward),
            ("deleteWordForward", Command::DeleteWordForward),
        ] {
            assert_eq!(read(kind, None, 0, 0), Ok(command), "{kind}");
        }
    }

    #[test]
    fn an_edit_command_the_protocol_does_not_describe_is_refused() {
        assert!(edit_command("insert", None, 0, 0).is_err());
        assert!(edit_command("commitText", None, 0, 0).is_err());
        assert!(edit_command("setComposition", None, 0, 0).is_err());
        assert!(edit_command("setComposition", Some("a".to_owned()), -1, 0).is_err());
        assert!(edit_command("setComposition", Some("a".to_owned()), 1, 0).is_err());
        assert!(edit_command("insertText", Some("a".to_owned()), 0, 0).is_err());
        assert!(edit_command("", None, 0, 0).is_err());
    }

    #[test]
    fn the_enumerations_cross_as_html_keywords() {
        assert_eq!(input_mode_name(InputMode::Default), "");
        assert_eq!(input_mode_name(InputMode::Numeric), "numeric");
        assert_eq!(input_mode_name(InputMode::None), "none");
        assert_eq!(enter_key_hint_name(EnterKeyHint::Default), "");
        assert_eq!(enter_key_hint_name(EnterKeyHint::Send), "send");
        assert_eq!(input_type_name(InputType::Text), "text");
        assert_eq!(input_type_name(InputType::Password), "password");
        assert_eq!(input_type_name(InputType::Number), "number");
    }
}
