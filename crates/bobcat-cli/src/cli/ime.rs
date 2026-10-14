//! winit's input-method events as the editing commands a view takes
//! (`docs/editable-text-design.md`, E3).
//!
//! What winit 0.30.13's `AppKit` view sends, and so what this bridge relies
//! on (`src/platform_impl/macos/view.rs` in the winit crate):
//!
//! - **A key the input method did not consume** reaches the host as one `KeyboardInput` whose
//!   `KeyEvent.text` is the text it typed. `insertText:` with no marked text queues nothing
//!   (`insert_text`, lines 411-415, commits only while there is marked text), so the key's text
//!   arrives with the key and nowhere else; the host passes it on as the key event's `text`.
//! - **A key the input method consumed** queues no `KeyboardInput` at all (`key_down`, lines
//!   479-497: a preedit or a commit in that `keyDown:` suppresses it, unless `doCommandBySelector:`
//!   asked for the key), and its text arrives as `Ime::Preedit` and `Ime::Commit`. So no key's text
//!   arrives both ways.
//! - **A commit is an empty `Preedit` and then the `Commit`**, queued back to back by one
//!   `insertText:` (lines 412-413). An empty `Preedit` alone is the composition emptied
//!   (`setMarkedText:` with an empty string, lines 322-334, or `unmarkText`, line 345).
//!
//! The engine's empty `SetComposition` cancels the composition and removes
//! its text, which is right for an empty `Preedit` alone and wrong for the
//! one before a `Commit`: forwarded as it comes, it would fire a
//! `compositionend` and delete the text before the commit replaced it. So
//! the bridge holds an empty `Preedit` back until the next event says which
//! it was: a `Commit` drops it (the commit replaces the composition itself),
//! and anything else — another input-method event, any other window event,
//! the end of the event-loop turn — sends it first. One `insertText:`
//! queues both events inside one `keyDown:`, so the turn always ends after
//! the `Commit`.

use std::ops::Range;

use bobcat_core::editing::Command;
use winit::event::Ime;

/// The input-method state the host keeps between winit's events.
#[derive(Debug, Default)]
pub(crate) struct ImeBridge {
    /// A non-empty composition was sent to the view and nothing has ended
    /// it since.
    composing: bool,
    /// winit's empty `Preedit` arrived while composing, and has not been
    /// sent: the next event decides whether it was a cancellation.
    held_clear: bool,
}

impl ImeBridge {
    /// The commands one input-method event asks for, in order.
    pub(crate) fn ime(&mut self, event: Ime) -> Vec<Command> {
        let mut commands = Vec::new();
        match event {
            Ime::Preedit(text, _) if text.is_empty() => {
                // Nothing composed here, nothing to cancel: an empty
                // `SetComposition` with no composition would delete a
                // selected range instead.
                if self.composing {
                    self.held_clear = true;
                }
            }
            Ime::Preedit(text, cursor) => {
                commands.extend(self.flush());
                let selection = composition_selection(&text, cursor);
                self.composing = true;
                commands.push(Command::SetComposition { text, selection });
            }
            Ime::Commit(text) => {
                // The empty `Preedit` before a commit is part of it.
                self.held_clear = false;
                self.composing = false;
                commands.push(Command::CommitText {
                    text,
                    relative_cursor: 0,
                });
            }
            Ime::Disabled => {
                commands.extend(self.flush());
                if self.composing {
                    self.composing = false;
                    commands.push(Command::FinishComposition {
                        keep_selection: false,
                    });
                }
            }
            Ime::Enabled => commands.extend(self.flush()),
        }
        commands
    }

    /// The cancellation an empty `Preedit` held back, now that an event
    /// other than a `Commit` followed it (or the turn ended).
    pub(crate) fn flush(&mut self) -> Option<Command> {
        if !std::mem::take(&mut self.held_clear) {
            return None;
        }
        self.composing = false;
        Some(Command::SetComposition {
            text: String::new(),
            selection: 0..0,
        })
    }

    /// Forgets the composition: the focus moved to another editing host or
    /// to none, and whatever was composed belongs to the one it left.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// winit's preedit cursor — a byte range into `text`, or `None` for a
/// hidden cursor — as the UTF-16 range `SetComposition` takes, relative to
/// the start of `text`. A hidden cursor is a caret at the end.
fn composition_selection(text: &str, cursor: Option<(usize, usize)>) -> Range<usize> {
    let units = |byte: usize| {
        let mut byte = byte.min(text.len());
        while !text.is_char_boundary(byte) {
            byte -= 1;
        }
        text[..byte].encode_utf16().count()
    };
    let (start, end) = cursor.map_or_else(
        || {
            let end = units(text.len());
            (end, end)
        },
        |(start, end)| (units(start), units(end)),
    );
    start.min(end)..start.max(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(bridge: &mut ImeBridge, events: Vec<Ime>) -> Vec<Command> {
        events
            .into_iter()
            .flat_map(|event| bridge.ime(event))
            .collect()
    }

    fn composition(text: &str, selection: Range<usize>) -> Command {
        Command::SetComposition {
            text: text.to_owned(),
            selection,
        }
    }

    fn commit(text: &str) -> Command {
        Command::CommitText {
            text: text.to_owned(),
            relative_cursor: 0,
        }
    }

    /// The dead-key sequence winit documents: Option-e composes `´`, then
    /// `e` commits `é` through an empty `Preedit` and the `Commit`. The
    /// empty `Preedit` before the commit sends nothing, so the composition
    /// is replaced once and never cancelled first.
    #[test]
    fn the_empty_preedit_before_a_commit_is_not_a_cancellation() {
        let mut bridge = ImeBridge::default();
        let commands = feed(
            &mut bridge,
            vec![
                Ime::Enabled,
                Ime::Preedit("´".to_owned(), Some((0, 2))),
                Ime::Preedit(String::new(), None),
                Ime::Commit("é".to_owned()),
            ],
        );
        assert_eq!(commands, [composition("´", 0..1), commit("é")]);
        assert_eq!(bridge.flush(), None, "the turn ends with nothing held");
    }

    /// Pinyin, as winit documents it: preedits with a byte cursor, then a
    /// commit of different text.
    #[test]
    fn preedit_cursors_become_utf16_ranges() {
        let mut bridge = ImeBridge::default();
        let commands = feed(
            &mut bridge,
            vec![
                Ime::Preedit("a".to_owned(), Some((1, 1))),
                Ime::Preedit("a b".to_owned(), Some((3, 3))),
                Ime::Preedit("啊b".to_owned(), Some((3, 3))),
                Ime::Preedit("😀x".to_owned(), Some((4, 5))),
                Ime::Preedit("x".to_owned(), None),
                Ime::Preedit(String::new(), None),
                Ime::Commit("啊不".to_owned()),
            ],
        );
        assert_eq!(
            commands,
            [
                composition("a", 1..1),
                composition("a b", 3..3),
                composition("啊b", 1..1),
                composition("😀x", 2..3),
                composition("x", 1..1),
                commit("啊不"),
            ]
        );
    }

    /// A composition the user emptied (Backspace over its last character)
    /// is an empty `Preedit` with no `Commit` after it: it is sent once the
    /// turn ends, or before whatever event comes next.
    #[test]
    fn an_empty_preedit_alone_cancels_the_composition() {
        let mut bridge = ImeBridge::default();
        let commands = feed(
            &mut bridge,
            vec![
                Ime::Preedit("k".to_owned(), Some((1, 1))),
                Ime::Preedit(String::new(), None),
            ],
        );
        assert_eq!(commands, [composition("k", 1..1)]);
        assert_eq!(bridge.flush(), Some(composition("", 0..0)));
        assert_eq!(bridge.flush(), None);

        // Held, then a new composition: the cancellation goes first.
        let commands = feed(
            &mut bridge,
            vec![
                Ime::Preedit("k".to_owned(), Some((1, 1))),
                Ime::Preedit(String::new(), None),
                Ime::Preedit("n".to_owned(), Some((1, 1))),
            ],
        );
        assert_eq!(
            commands,
            [
                composition("k", 1..1),
                composition("", 0..0),
                composition("n", 1..1)
            ]
        );
    }

    /// With nothing composed, an empty `Preedit` is never sent: the
    /// engine's empty `SetComposition` would delete a selected range.
    #[test]
    fn an_empty_preedit_with_nothing_composed_sends_nothing() {
        let mut bridge = ImeBridge::default();
        assert!(bridge.ime(Ime::Preedit(String::new(), None)).is_empty());
        assert_eq!(bridge.flush(), None);
        assert_eq!(
            feed(&mut bridge, vec![Ime::Commit("y".to_owned())]),
            [commit("y")],
            "a commit with nothing composed inserts at the selection"
        );
    }

    /// The input method going away mid-composition keeps what was composed.
    #[test]
    fn disabling_the_input_method_finishes_the_composition() {
        let mut bridge = ImeBridge::default();
        let commands = feed(
            &mut bridge,
            vec![
                Ime::Preedit("に".to_owned(), Some((3, 3))),
                Ime::Disabled,
                Ime::Disabled,
            ],
        );
        assert_eq!(
            commands,
            [
                composition("に", 1..1),
                Command::FinishComposition {
                    keep_selection: false
                }
            ]
        );

        bridge.ime(Ime::Preedit("に".to_owned(), Some((3, 3))));
        bridge.reset();
        assert!(
            bridge.ime(Ime::Disabled).is_empty(),
            "a composition the focus left is not finished in the next host"
        );
    }
}
