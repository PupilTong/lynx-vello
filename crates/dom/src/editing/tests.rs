//! Editing hosts, focus, commands, `TextInputState` and the selection's life
//! across layouts, on Ahem (one em square per glyph) at 10px so geometry is
//! exact wherever the text is ASCII. Emoji and CJK fall back to whatever face
//! the host has, so for them only offsets are asserted, never positions.
//!
//! Editing is decoupled from the text container, so the command and state
//! tests run over every kind of host ([`Kind`]): a Lynx `<text>`, whose own
//! Lynx paragraph holds its text; a flex `view`, whose loose text an
//! anonymous block box lays out in a standard inline formatting context;
//! and a block container (an engine sheet's `display: block`), whose own
//! inline formatting context does.

#![allow(clippy::float_cmp)] // Ahem's em squares have exact metrics.

use super::*;
use crate::{FontBlob, Point2D, Rect, Size2D, StylesheetOrigin};

/// The embedder's UA rule for an editing host, as `bobcat-core` writes it.
const HOST_RULE: &str = "[contenteditable]:not([contenteditable=\"false\" i]) \
                         { white-space: pre-wrap; }";

/// The block container an engine sheet writes, which an author sheet cannot;
/// a UA sheet stands in for the engine's own here.
const FLOW_RULE: &str = ".flow { display: block; }";

/// A value with a surrogate pair, a CJK character, a hard break and a
/// combining mark: `a b 😀 中 \n c d e ◌́`, bytes 0..15, UTF-16 units 0..10.
const MIXED: &str = "ab😀中\ncde\u{301}";

/// Which paragraph a host's text is laid out in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A Lynx `<text>`: its own Lynx paragraph.
    Text,
    /// A flex `view`: the anonymous block box around its loose text.
    View,
    /// An engine `display: block` box: its own inline formatting context.
    Block,
}

const KINDS: [Kind; 3] = [Kind::Text, Kind::View, Kind::Block];

impl Kind {
    fn create(self, doc: &mut Document<()>) -> NodeId {
        match self {
            Self::Text => doc.create_element("text", ()),
            Self::View => doc.create_element("view", ()),
            Self::Block => {
                let host = doc.create_element("div", ());
                doc.set_attribute(host, "class", "flow");
                host
            }
        }
    }
}

fn document() -> Document<()> {
    let mut doc = Document::new(crate::tree::document::tests::device(), "page", ());
    crate::test_common::define_lynx_text(&mut doc, "text");
    doc.register_fonts(FontBlob::from_static(include_bytes!(
        "../../../hughie/tests/fixtures/Ahem.ttf"
    )));
    doc.add_stylesheet(HOST_RULE, StylesheetOrigin::UserAgent);
    doc.add_stylesheet(FLOW_RULE, StylesheetOrigin::UserAgent);
    doc.add_stylesheet(
        "page { display: flex; align-items: flex-start; font-family: Ahem; font-size: 10px; \
         line-height: 10px; } view { display: flex; }",
        StylesheetOrigin::Author,
    );
    doc
}

/// A focused host of `kind`, `width` wide, holding one text node per entry
/// of `texts`, laid out.
fn host_in(kind: Kind, texts: &[&str], width: f32) -> (Document<()>, NodeId) {
    let mut doc = document();
    let host = kind.create(&mut doc);
    doc.set_attribute(host, "contenteditable", "");
    doc.set_inline_style(host, &format!("width: {width}px; padding: 3px 5px;"));
    doc.append_child(doc.document_element().id(), host);
    for text in texts {
        let node = doc.create_text_node(*text, ());
        doc.append_child(host, node);
    }
    doc.layout();
    assert!(doc.focus(host), "{kind:?}");
    doc.take_focus_events();
    doc.layout();
    (doc, host)
}

/// The measured size of the paragraph `host`'s text is laid out in.
fn paragraph_size(doc: &Document<()>, host: NodeId) -> (f32, f32) {
    let node = doc
        .paragraph_node_of(host)
        .expect("a paragraph holds the text");
    let size = doc.text_block_size(node).expect("a committed paragraph");
    (size.width, size.height)
}

fn state(doc: &Document<()>) -> TextInputState {
    doc.text_input_state()
}

fn value(doc: &Document<()>) -> String {
    state(doc).value
}

fn selection(doc: &Document<()>) -> Range<u32> {
    state(doc).selection
}

fn select(doc: &mut Document<()>, range: Range<usize>) {
    assert!(doc.apply_editing(Command::SetSelection { range }).applied);
}

fn kinds(outcome: &EditOutcome) -> Vec<(&'static str, &'static str, Option<&str>, bool)> {
    outcome
        .events
        .iter()
        .map(|event| {
            (
                event.kind.name(),
                event.input_type,
                event.data.as_deref(),
                event.is_composing,
            )
        })
        .collect()
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect<f32> {
    Rect::new(Point2D::new(x, y), Size2D::new(width, height))
}

// --- Editing hosts -------------------------------------------------------

#[test]
fn contenteditable_values_decide_the_editing_host() {
    let mut doc = document();
    let element = doc.create_element("view", ());
    doc.append_child(doc.document_element().id(), element);
    assert!(!doc.is_editing_host(element), "absent");
    for (value, host) in [
        ("", true),
        ("true", true),
        ("plaintext-only", true),
        ("TRUE", true),
        ("anything", true),
        ("false", false),
        ("FALSE", false),
        ("False", false),
    ] {
        doc.set_attribute(element, "contenteditable", value);
        assert_eq!(doc.is_editing_host(element), host, "{value:?}");
    }
    let text = doc.create_text_node("x", ());
    doc.append_child(element, text);
    assert!(!doc.is_editing_host(text), "a text node is never a host");
}

#[test]
fn the_ua_rule_keeps_the_hosts_display_and_defaults_its_white_space() {
    let mut doc = document();
    let page = doc.document_element().id();
    let mut hosts = Vec::new();
    for (value, style) in [
        ("", ""),
        ("true", ""),
        ("FALSE", ""),
        ("false", ""),
        ("", "white-space: normal;"),
    ] {
        let host = doc.create_element("view", ());
        doc.set_attribute(host, "contenteditable", value);
        doc.set_inline_style(host, style);
        doc.append_child(page, host);
        let text = doc.create_text_node("a  b", ());
        doc.append_child(host, text);
        hosts.push(host);
    }
    doc.layout();
    for &host in &hosts {
        let node = doc.get(host).unwrap();
        let style = node.computed_style().unwrap();
        assert_eq!(
            crate::layout::display_mode(node, *style.get_box().get_display()),
            crate::layout::DisplayMode::Flex,
            "a host keeps its own display"
        );
        assert_ne!(doc.paragraph_node_of(host), Some(host), "an anonymous box");
    }
    // `pre-wrap` keeps both spaces, four Ahem squares, for every value but
    // `false` (the `i` flag: `FALSE` is `false`); an author's `normal`
    // collapses them.
    let widths: Vec<f32> = hosts
        .iter()
        .map(|&host| paragraph_size(&doc, host).0)
        .collect();
    assert_eq!(widths, [40.0, 40.0, 30.0, 30.0, 30.0]);
}

// --- Focus -----------------------------------------------------------------

#[test]
fn focus_moves_between_hosts_with_the_four_events_in_order() {
    let mut doc = document();
    let page = doc.document_element().id();
    let first = doc.create_element("view", ());
    let second = doc.create_element("view", ());
    let plain = doc.create_element("view", ());
    for element in [first, second, plain] {
        doc.append_child(page, element);
    }
    doc.set_attribute(first, "contenteditable", "true");
    doc.set_attribute(second, "contenteditable", "");

    assert!(!doc.focus(plain), "only editing hosts are focusable");
    assert_eq!(doc.focused(), None);
    assert!(doc.take_focus_events().is_empty());

    assert!(doc.focus(first));
    assert!(doc.focus(first), "focusing the focused element is a no-op");
    assert_eq!(
        doc.take_focus_events(),
        [
            FocusEvent {
                kind: FocusEventKind::Focus,
                target: first,
                related_target: None
            },
            FocusEvent {
                kind: FocusEventKind::FocusIn,
                target: first,
                related_target: None
            },
        ]
    );
    assert_eq!(doc.query_selector(page, ":focus").unwrap(), Some(first));

    assert!(doc.focus(second));
    let events = doc.take_focus_events();
    let names: Vec<_> = events
        .iter()
        .map(|event| {
            (
                event.kind.name(),
                event.kind.bubbles(),
                event.target,
                event.related_target,
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            ("blur", false, first, Some(second)),
            ("focusout", true, first, Some(second)),
            ("focus", false, second, Some(first)),
            ("focusin", true, second, Some(first)),
        ]
    );
    assert_eq!(doc.query_selector(page, ":focus").unwrap(), Some(second));

    assert!(doc.blur());
    assert!(!doc.blur());
    assert_eq!(doc.take_focus_events().len(), 2);
    assert_eq!(doc.focused(), None);
    assert_eq!(doc.query_selector(page, ":focus").unwrap(), None);
    assert_eq!(doc.text_input_state(), TextInputState::default());
}

#[test]
fn a_detached_host_cannot_be_focused_and_a_removed_one_loses_focus_silently() {
    let mut doc = document();
    let page = doc.document_element().id();
    let host = doc.create_element("view", ());
    doc.set_attribute(host, "contenteditable", "");
    assert!(!doc.focus(host), "detached");
    doc.append_child(page, host);
    assert!(doc.focus(host));
    doc.take_focus_events();
    doc.remove_element(host);
    assert_eq!(doc.focused(), None);
    assert!(
        doc.take_focus_events().is_empty(),
        "the fixup rule fires nothing"
    );
    assert!(
        !doc.get(host)
            .unwrap()
            .element_state()
            .contains(crate::ElementState::FOCUS)
    );

    doc.append_child(page, host);
    assert!(doc.focus(host));
    doc.take_focus_events();
    doc.set_attribute(host, "contenteditable", "False");
    assert_eq!(doc.focused(), None, "no longer a host");
    assert!(doc.take_focus_events().is_empty());
    doc.set_attribute(host, "contenteditable", "true");
    assert!(doc.focus(host));
    doc.remove_attribute(host, "contenteditable");
    assert_eq!(doc.focused(), None);
}

// --- Commands --------------------------------------------------------------

#[test]
fn focus_places_the_caret_at_the_start_and_insert_types_there() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &[MIXED], 200.0);
        assert_eq!(selection(&doc), 0..0);
        let outcome = doc.apply_editing(Command::Insert("X".to_owned()));
        assert!(outcome.applied);
        assert_eq!(
            kinds(&outcome),
            [
                ("beforeinput", "insertText", Some("X"), false),
                ("input", "insertText", Some("X"), false),
            ]
        );
        assert!(outcome.events[0].cancelable());
        assert!(!outcome.events[1].cancelable());
        assert_eq!(value(&doc), format!("X{MIXED}"));
        assert_eq!(selection(&doc), 1..1);
        assert!(!doc.apply_editing(Command::Insert(String::new())).applied);
    }
}

#[test]
fn insert_replaces_a_selection_and_a_line_break_has_no_data() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &[MIXED], 200.0);
        select(&mut doc, 2..5); // the emoji and 中
        let outcome = doc.apply_editing(Command::InsertLineBreak);
        assert_eq!(
            kinds(&outcome),
            [
                ("beforeinput", "insertLineBreak", None, false),
                ("input", "insertLineBreak", None, false),
            ]
        );
        assert_eq!(value(&doc), "ab\n\ncde\u{301}");
        assert_eq!(selection(&doc), 3..3);
    }
}

#[test]
fn delete_backward_takes_a_code_point_an_emoji_or_a_line_break() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &[MIXED], 200.0);
        // After the combining mark: only the mark goes, as parley's backdelete
        // (and Chromium's backspace) does.
        select(&mut doc, 10..10);
        let outcome = doc.apply_editing(Command::DeleteBackward);
        assert_eq!(
            kinds(&outcome),
            [
                ("beforeinput", "deleteContentBackward", None, false),
                ("input", "deleteContentBackward", None, false),
            ]
        );
        assert_eq!(value(&doc), "ab😀中\ncde");
        assert_eq!(selection(&doc), 9..9);
        // After the emoji: both of its UTF-16 units.
        select(&mut doc, 4..4);
        assert!(doc.apply_editing(Command::DeleteBackward).applied);
        assert_eq!(value(&doc), "ab中\ncde");
        assert_eq!(selection(&doc), 2..2);
        // After the line break: the break, joining the lines.
        select(&mut doc, 4..4);
        assert!(doc.apply_editing(Command::DeleteBackward).applied);
        assert_eq!(value(&doc), "ab中cde");
        // At the start: nothing to delete, no event.
        select(&mut doc, 0..0);
        let outcome = doc.apply_editing(Command::DeleteBackward);
        assert!(!outcome.applied);
        assert!(outcome.events.is_empty());
    }
}

#[test]
fn delete_forward_takes_a_whole_cluster() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &[MIXED], 200.0);
        select(&mut doc, 2..2); // before the emoji
        let outcome = doc.apply_editing(Command::DeleteForward);
        assert_eq!(
            kinds(&outcome)[0],
            ("beforeinput", "deleteContentForward", None, false)
        );
        assert_eq!(value(&doc), "ab中\ncde\u{301}");
        assert_eq!(selection(&doc), 2..2);
        // Over the hard break.
        select(&mut doc, 3..3);
        assert!(doc.apply_editing(Command::DeleteForward).applied);
        assert_eq!(value(&doc), "ab中cde\u{301}");
        select(&mut doc, 8..8);
        assert!(
            !doc.apply_editing(Command::DeleteForward).applied,
            "at the end"
        );
    }
}

#[test]
fn word_deletion_stops_at_word_boundaries() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &["hello brave world"], 300.0);
        select(&mut doc, 11..11); // after "brave"
        let outcome = doc.apply_editing(Command::DeleteWordBackward);
        assert_eq!(kinds(&outcome)[0].1, "deleteWordBackward");
        assert_eq!(value(&doc), "hello  world");
        assert_eq!(selection(&doc), 6..6);
        // Forward from between the two spaces, the space is a word segment of
        // its own (parley's `next_logical_word`), then the word.
        let outcome = doc.apply_editing(Command::DeleteWordForward);
        assert_eq!(kinds(&outcome)[0].1, "deleteWordForward");
        assert_eq!(value(&doc), "hello world");
        assert!(doc.apply_editing(Command::DeleteWordForward).applied);
        assert_eq!(value(&doc), "hello ");
        // A range selection is deleted whole, whatever the granularity.
        select(&mut doc, 1..3);
        assert!(doc.apply_editing(Command::DeleteWordBackward).applied);
        assert_eq!(value(&doc), "hlo ");
    }
}

#[test]
fn moves_follow_clusters_words_lines_and_the_ends() {
    use Direction::{Backward, Forward};
    use Granularity::{Character, Document as Whole, Line, LineBoundary, Word};
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &[MIXED], 200.0);
        let step = |doc: &mut Document<()>, granularity, direction, extend| {
            assert!(
                doc.apply_editing(Command::Move {
                    granularity,
                    direction,
                    extend
                })
                .applied
            );
            selection(doc)
        };
        assert_eq!(step(&mut doc, Character, Forward, false), 1..1);
        assert_eq!(step(&mut doc, Character, Forward, false), 2..2);
        // Over the surrogate pair in one step.
        assert_eq!(step(&mut doc, Character, Forward, false), 4..4);
        assert_eq!(step(&mut doc, Character, Backward, true), 2..4);
        // Without `extend`, a range collapses to its start.
        assert_eq!(step(&mut doc, Character, Backward, false), 2..2);
        // Down a line keeps the horizontal position (two Ahem squares in).
        select(&mut doc, 1..1);
        assert_eq!(step(&mut doc, Line, Forward, false), 7..7);
        assert_eq!(step(&mut doc, Line, Backward, false), 1..1);
        assert_eq!(step(&mut doc, LineBoundary, Forward, false), 5..5);
        assert_eq!(step(&mut doc, LineBoundary, Backward, false), 0..0);
        assert_eq!(step(&mut doc, Whole, Forward, true), 0..10);
        assert_eq!(step(&mut doc, Whole, Backward, false), 0..0);

        let (mut doc, _) = host_in(kind, &["one two three"], 300.0);
        assert_eq!(step(&mut doc, Word, Forward, false), 3..3);
        assert_eq!(step(&mut doc, Word, Forward, false), 7..7);
        assert_eq!(step(&mut doc, Word, Backward, false), 4..4);
    }
}

#[test]
fn select_all_and_set_selection_clamp_to_the_value() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &[MIXED], 200.0);
        assert!(doc.apply_editing(Command::SelectAll).applied);
        assert_eq!(selection(&doc), 0..10);
        // Inside the surrogate pair: rounded down to its start.
        select(&mut doc, 3..3);
        assert_eq!(selection(&doc), 2..2);
        select(&mut doc, 4..99);
        assert_eq!(selection(&doc), 4..10);
        // A backward selection keeps its anchor; the state reports start ≤ end.
        select(&mut doc, Range { start: 6, end: 1 });
        assert_eq!(selection(&doc), 1..6);
    }
}

#[test]
fn pointer_commands_place_and_extend_by_cluster_word_and_line() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        // Two lines of Ahem squares, the host's content box 5px right and 3px
        // down of its border box.
        let (mut doc, _) = host_in(kind, &["one two\nthree"], 200.0);
        let at = |x: f32, y: f32| Point2D::new(x + 5.0, y + 3.0);
        assert!(
            doc.apply_editing(Command::SelectFromPoint {
                point: at(12.0, 5.0),
                extend: false
            })
            .applied
        );
        assert_eq!(selection(&doc), 1..1);
        doc.apply_editing(Command::SelectFromPoint {
            point: at(31.0, 15.0),
            extend: true,
        });
        assert_eq!(selection(&doc), 1..11);
        doc.apply_editing(Command::SelectWordAtPoint {
            point: at(45.0, 5.0),
        });
        assert_eq!(selection(&doc), 4..7);
        doc.apply_editing(Command::SelectLineAtPoint {
            point: at(5.0, 15.0),
        });
        assert_eq!(selection(&doc), 8..13);
    }
}

#[test]
fn a_composition_starts_updates_and_commits() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &["ab"], 200.0);
        select(&mut doc, 1..1);
        let start = doc.apply_editing(Command::SetComposition {
            text: "に".to_owned(),
            selection: 1..1,
        });
        assert_eq!(
            kinds(&start),
            [
                ("compositionstart", "", Some(""), false),
                ("beforeinput", "insertCompositionText", Some("に"), true),
                ("compositionupdate", "", Some("に"), false),
                ("input", "insertCompositionText", Some("に"), true),
            ]
        );
        assert!(!start.events[1].cancelable(), "Input Events Level 2");
        assert_eq!(value(&doc), "aにb");
        assert_eq!(state(&doc).composition, Some(1..2));
        assert_eq!(selection(&doc), 2..2);

        let update = doc.apply_editing(Command::SetComposition {
            text: "にほ".to_owned(),
            selection: 0..2,
        });
        assert_eq!(kinds(&update).len(), 3, "no second compositionstart");
        assert_eq!(value(&doc), "aにほb");
        assert_eq!(state(&doc).composition, Some(1..3));
        assert_eq!(selection(&doc), 1..3);

        let commit = doc.apply_editing(Command::CommitText {
            text: "日本".to_owned(),
            relative_cursor: 0,
        });
        assert_eq!(
            kinds(&commit),
            [
                ("beforeinput", "insertCompositionText", Some("日本"), true),
                ("compositionupdate", "", Some("日本"), false),
                ("input", "insertCompositionText", Some("日本"), true),
                ("compositionend", "", Some("日本"), false),
            ]
        );
        assert_eq!(value(&doc), "a日本b");
        assert_eq!(state(&doc).composition, None);
        assert_eq!(selection(&doc), 3..3);
    }
}

#[test]
fn commit_text_places_the_caret_relative_to_the_inserted_end() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &["ab"], 200.0);
        select(&mut doc, 1..1);
        let outcome = doc.apply_editing(Command::CommitText {
            text: "xyz".to_owned(),
            relative_cursor: -1,
        });
        assert_eq!(
            kinds(&outcome)[0],
            ("beforeinput", "insertText", Some("xyz"), false)
        );
        assert_eq!(value(&doc), "axyzb");
        assert_eq!(selection(&doc), 3..3);
        doc.apply_editing(Command::CommitText {
            text: "😀".to_owned(),
            relative_cursor: 100,
        });
        assert_eq!(selection(&doc), 7..7, "clamped to the end");
    }
}

#[test]
fn a_composition_can_be_cancelled_finished_or_taken_from_existing_text() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &["abc"], 200.0);
        select(&mut doc, 3..3);
        doc.apply_editing(Command::SetComposition {
            text: "xy".to_owned(),
            selection: 0..0,
        });
        let cancel = doc.apply_editing(Command::SetComposition {
            text: String::new(),
            selection: 0..0,
        });
        assert_eq!(
            kinds(&cancel).last().copied(),
            Some(("compositionend", "", Some(""), false))
        );
        assert_eq!(value(&doc), "abc");
        assert_eq!(selection(&doc), 3..3);

        let existing = doc.apply_editing(Command::SetCompositionFromExistingText { range: 0..2 });
        assert_eq!(
            kinds(&existing),
            [("compositionstart", "", Some("ab"), false)]
        );
        assert_eq!(state(&doc).composition, Some(0..2));
        assert_eq!(selection(&doc), 3..3);
        let finish = doc.apply_editing(Command::FinishComposition {
            keep_selection: false,
        });
        assert_eq!(kinds(&finish), [("compositionend", "", Some("ab"), false)]);
        assert_eq!(state(&doc).composition, None);
        assert_eq!(selection(&doc), 2..2, "moved to the composition's end");
        assert!(
            !doc.apply_editing(Command::FinishComposition {
                keep_selection: true
            })
            .applied,
            "nothing to finish"
        );
    }
}

#[test]
fn surrounding_deletion_keeps_the_selection_and_refuses_split_pairs() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &[MIXED], 200.0);
        select(&mut doc, 1..2);
        let outcome = doc.apply_editing(Command::DeleteSurrounding {
            before: 1,
            after: 2,
        });
        assert_eq!(
            kinds(&outcome),
            [
                ("beforeinput", "deleteContentBackward", None, false),
                ("input", "deleteContentBackward", None, false),
            ]
        );
        assert_eq!(value(&doc), "b中\ncde\u{301}");
        assert_eq!(selection(&doc), 0..1, "the selected text stays selected");

        let (mut doc, _) = host_in(kind, &[MIXED], 200.0);
        select(&mut doc, 2..2);
        assert!(
            !doc.apply_editing(Command::DeleteSurrounding {
                before: 0,
                after: 1
            })
            .applied,
            "half an emoji"
        );
        select(&mut doc, 4..4);
        assert!(
            !doc.apply_editing(Command::ExtendSelectionAndDelete {
                before: 1,
                after: 0
            })
            .applied
        );
        let outcome = doc.apply_editing(Command::ExtendSelectionAndDelete {
            before: 2,
            after: 1,
        });
        assert_eq!(kinds(&outcome)[0].1, "deleteContentBackward");
        assert_eq!(value(&doc), "ab\ncde\u{301}");
        assert_eq!(selection(&doc), 2..2);
    }
}

#[test]
fn edits_inside_a_composition_report_is_composing() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &["ab"], 200.0);
        select(&mut doc, 2..2);
        doc.apply_editing(Command::SetComposition {
            text: "xyz".to_owned(),
            selection: 3..3,
        });
        let outcome = doc.apply_editing(Command::DeleteSurrounding {
            before: 1,
            after: 0,
        });
        assert!(outcome.events.iter().all(|event| event.is_composing));
        assert_eq!(value(&doc), "abxy");
        assert_eq!(state(&doc).composition, Some(2..4));
        // Typing during a composition ends it first.
        let typed = doc.apply_editing(Command::Insert("!".to_owned()));
        assert_eq!(
            kinds(&typed),
            [
                ("compositionend", "", Some("xy"), false),
                ("beforeinput", "insertText", Some("!"), false),
                ("input", "insertText", Some("!"), false),
            ]
        );
        assert_eq!(value(&doc), "abxy!");
        assert_eq!(state(&doc).composition, None);
    }
}

// --- Content model -------------------------------------------------------

#[test]
fn the_first_edit_normalizes_the_host_to_one_text_node() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["ab", "cd", "ef"], 200.0);
        let texts = doc.get(host).unwrap().child_ids().to_vec();
        assert_eq!(value(&doc), "abcdef");
        select(&mut doc, 6..6);
        assert!(doc.apply_editing(Command::Insert("g".to_owned())).applied);
        assert_eq!(doc.get(host).unwrap().child_ids(), [texts[0]]);
        assert_eq!(doc.get(texts[0]).unwrap().text(), Some("abcdefg"));
        for removed in &texts[1..] {
            assert_eq!(
                doc.get(*removed).unwrap().parent_id(),
                None,
                "unlinked, not freed"
            );
        }

        let (mut doc, host) = host_in(kind, &[], 200.0);
        assert_eq!(value(&doc), "");
        assert!(doc.apply_editing(Command::Insert("hi".to_owned())).applied);
        let children = doc.get(host).unwrap().child_ids().to_vec();
        assert_eq!(children.len(), 1);
        assert_eq!(doc.get(children[0]).unwrap().text(), Some("hi"));
    }
}

#[test]
fn an_element_child_makes_the_host_read_only() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["ab"], 200.0);
        let child = doc.create_element("view", ());
        doc.set_inline_style(child, "width: 10px; height: 10px;");
        doc.append_child(host, child);
        doc.layout();
        for command in [
            Command::Insert("x".to_owned()),
            Command::InsertLineBreak,
            Command::DeleteBackward,
            Command::DeleteForward,
            Command::SetComposition {
                text: "x".to_owned(),
                selection: 0..0,
            },
            Command::CommitText {
                text: "x".to_owned(),
                relative_cursor: 0,
            },
            Command::DeleteSurrounding {
                before: 1,
                after: 1,
            },
        ] {
            let outcome = doc.apply_editing(command.clone());
            assert!(!outcome.applied, "{command:?}");
            assert!(outcome.events.is_empty());
        }
        assert_eq!(value(&doc), "ab");
        assert!(
            doc.apply_editing(Command::SetSelection { range: 1..2 })
                .applied
        );
        assert_eq!(selection(&doc), 1..2);
    }
}

#[test]
fn a_prepared_edit_whose_value_moved_commits_nothing() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["ab"], 200.0);
        let edit = doc
            .prepare_editing(Command::Insert("x".to_owned()))
            .expect("an insert at the caret");
        assert_eq!(edit.host(), host);
        assert!(edit.changes_value());
        assert_eq!(edit.commit_index(), 1, "beforeinput, then the change");
        let text = doc.get(host).unwrap().child_ids()[0];
        doc.set_text_node_data(text, "changed by a listener");
        let outcome = doc.commit_editing(edit);
        assert!(!outcome.applied);
        assert_eq!(value(&doc), "changed by a listener");
        assert!(doc.prepare_editing(Command::SelectAll).is_some());
        doc.blur();
        assert!(
            doc.prepare_editing(Command::SelectAll).is_none(),
            "nothing focused"
        );
    }
}

// --- TextInputState and the layout -----------------------------------------

#[test]
fn the_input_state_reports_value_offsets_and_geometry() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["abc\ndef"], 200.0);
        doc.set_attribute(host, "inputmode", "NUMERIC");
        doc.set_attribute(host, "enterkeyhint", "send");
        select(&mut doc, 5..5);
        doc.layout();
        let input = state(&doc);
        assert_eq!(input.host, Some(host));
        assert_eq!(input.input_type, InputType::Text);
        assert_eq!(input.input_mode, InputMode::Numeric);
        assert_eq!(input.enter_key_hint, EnterKeyHint::Send);
        assert_eq!(input.value, "abc\ndef");
        let bounds = doc.bounding_client_rect(host).unwrap();
        assert_eq!(input.editable_bounds, bounds);
        // After "d" on the second line: one square in, one line down, inside
        // the 5px/3px padding.
        let origin = bounds.origin;
        assert_eq!(
            input.caret,
            rect(origin.x + 5.0 + 10.0, origin.y + 3.0 + 10.0, 1.0, 10.0)
        );
        assert!(input.composition_bounds.is_empty());

        doc.set_attribute(host, "inputmode", "bogus");
        assert_eq!(state(&doc).input_mode, InputMode::Default);
    }
}

#[test]
fn composition_bounds_have_one_box_per_utf16_unit() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["ab"], 200.0);
        select(&mut doc, 2..2);
        doc.apply_editing(Command::SetComposition {
            text: "x😀y".to_owned(),
            selection: 4..4,
        });
        doc.layout();
        let input = state(&doc);
        assert_eq!(input.composition, Some(2..6));
        let bounds = &input.composition_bounds;
        assert_eq!(bounds.len(), 4);
        let origin = doc.bounding_client_rect(host).unwrap().origin;
        // The emoji's fallback face sets the line's height, so only the inline
        // axis is Ahem's to assert.
        assert_eq!(bounds[0].min_x(), origin.x + 5.0 + 20.0);
        assert_eq!(bounds[0].width(), 10.0);
        assert!(bounds.iter().all(|bound| bound.height() > 0.0));
        assert_eq!(bounds[1], bounds[2], "a surrogate pair shares its box");
        assert_eq!(bounds[1].min_x(), bounds[0].max_x());
        assert_eq!(bounds[3].min_x(), bounds[2].max_x());
    }
}

#[test]
fn the_selection_follows_a_relayout_that_rebreaks_its_lines() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["aaaa bbbb"], 50.0);
        select(&mut doc, 7..7);
        doc.layout();
        let origin = doc.bounding_client_rect(host).unwrap().origin;
        assert_eq!(
            state(&doc).caret,
            rect(origin.x + 5.0 + 20.0, origin.y + 3.0 + 10.0, 1.0, 10.0),
            "after `bb` on the second line"
        );
        doc.set_inline_style_property(host, "width", "200px");
        doc.layout();
        assert_eq!(
            state(&doc).caret,
            rect(origin.x + 5.0 + 70.0, origin.y + 3.0, 1.0, 10.0),
            "the same offset on the one line left"
        );
        assert_eq!(selection(&doc), 7..7);
    }
}

#[test]
fn an_empty_host_shows_a_caret_one_line_tall() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (doc, host) = host_in(kind, &[], 200.0);
        let origin = doc.bounding_client_rect(host).unwrap().origin;
        assert_eq!(
            state(&doc).caret,
            rect(origin.x + 5.0, origin.y + 3.0, 1.0, 10.0)
        );
    }
}

#[test]
fn a_paragraph_that_does_not_show_the_value_refuses_edits_but_takes_selections() {
    // Generated `content` replaces the children of a paragraph's own
    // element; a flex `view` lays out no paragraph of its own.
    for kind in [Kind::Text, Kind::Block] {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["ab"], 200.0);
        doc.add_stylesheet(".generated { content: 'XYZ'; }", StylesheetOrigin::Author);
        doc.add_class(host, "generated");
        doc.layout();
        assert!(!doc.editable_paragraph(host).unwrap().shows_value);
        // The selection was placed in the paragraph; it has no value offsets.
        assert_eq!(selection(&doc), 2..2, "collapsed at the value's end");
        for command in [
            Command::Insert("x".to_owned()),
            Command::DeleteBackward,
            Command::CommitText {
                text: "x".to_owned(),
                relative_cursor: 0,
            },
            Command::SetComposition {
                text: "x".to_owned(),
                selection: 0..0,
            },
            Command::DeleteSurrounding {
                before: 1,
                after: 0,
            },
            Command::Move {
                granularity: Granularity::Character,
                direction: Direction::Forward,
                extend: false,
            },
            Command::SelectFromPoint {
                point: Point2D::new(8.0, 5.0),
                extend: false,
            },
        ] {
            assert!(!doc.apply_editing(command.clone()).applied, "{command:?}");
        }
        assert_eq!(value(&doc), "ab");
        assert!(doc.apply_editing(Command::SelectAll).applied);
        assert_eq!(selection(&doc), 0..2);
        assert!(
            doc.apply_editing(Command::SetSelection { range: 1..1 })
                .applied
        );
        assert_eq!(selection(&doc), 1..1);
    }
}

#[test]
fn a_tab_is_shown_as_a_space_and_edits_stay_aligned() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["a\tb"], 200.0);
        assert!(doc.editable_paragraph(host).unwrap().shows_value);
        select(&mut doc, 3..3);
        assert!(doc.apply_editing(Command::Insert("c".to_owned())).applied);
        assert_eq!(value(&doc), "a\tbc");
        assert!(doc.apply_editing(Command::DeleteBackward).applied);
        assert_eq!(value(&doc), "a\tb");
        assert_eq!(selection(&doc), 3..3);
    }
}

#[test]
fn a_carriage_return_is_shaped_as_a_space_and_edits_stay_aligned() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["a\r\nb"], 200.0);
        assert!(doc.editable_paragraph(host).unwrap().shows_value);
        // Two lines: "a" plus the CR's space, then "b". A `<text>` hangs
        // every line-end space, as native Lynx does; `pre-wrap` in the
        // inline formatting context hangs it only conditionally before the
        // forced break, so it is measured there.
        let width = if kind == Kind::Text { 10.0 } else { 20.0 };
        assert_eq!(paragraph_size(&doc, host), (width, 20.0));
        select(&mut doc, 4..4);
        assert!(doc.apply_editing(Command::Insert("c".to_owned())).applied);
        assert_eq!(value(&doc), "a\r\nbc");
        assert!(doc.apply_editing(Command::DeleteBackward).applied);
        assert_eq!(value(&doc), "a\r\nb");
        assert_eq!(selection(&doc), 4..4);
    }
}

#[test]
fn an_empty_composition_outside_a_composition_deletes_the_selection() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &["abcd"], 200.0);
        select(&mut doc, 1..3);
        let outcome = doc.apply_editing(Command::SetComposition {
            text: String::new(),
            selection: 0..0,
        });
        // Blink's third case: no composition event, `DeleteSelection`'s pair.
        assert_eq!(
            kinds(&outcome),
            [
                ("beforeinput", "deleteContentBackward", None, false),
                ("input", "deleteContentBackward", None, false),
            ]
        );
        assert!(outcome.events[0].cancelable());
        assert_eq!(value(&doc), "ad");
        assert_eq!(selection(&doc), 1..1);
        assert_eq!(state(&doc).composition, None);
        // A collapsed selection: nothing to delete.
        assert!(
            !doc.apply_editing(Command::SetComposition {
                text: String::new(),
                selection: 0..0,
            })
            .applied
        );
    }
}

// --- The paragraph of a host ---------------------------------------------

/// Under an author `white-space: normal` the paragraph collapses what the
/// value keeps: `"a  b"` shows `"a b"`. The value keeps both spaces, a caret
/// in the value lands on the one space the run emitted, and a caret read
/// back from the layout is the span's start.
#[test]
fn a_collapsed_run_keeps_the_value_and_maps_the_caret_onto_its_emitted_byte() {
    for kind in [Kind::View, Kind::Block] {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &[], 200.0);
        doc.set_inline_style_property(host, "white-space", "normal");
        doc.layout();
        for typed in ["a", " ", " ", "b"] {
            assert!(doc.apply_editing(Command::Insert(typed.to_owned())).applied);
            doc.layout();
        }
        assert_eq!(value(&doc), "a  b");
        assert_eq!(selection(&doc), 4..4);
        let shown = doc.editable_paragraph(host).unwrap();
        assert_eq!(shown.map.display_text(), "a b");
        assert!(shown.shows_value);
        let origin = doc.bounding_client_rect(host).unwrap().origin;
        let caret_at = |x: f32| rect(origin.x + 5.0 + x, origin.y + 3.0, 1.0, 10.0);
        // After `b`: three squares in.
        assert_eq!(state(&doc).caret, caret_at(30.0));
        // Between the two spaces, and after both: on the emitted space, and
        // after it.
        select(&mut doc, 2..2);
        assert_eq!(state(&doc).caret, caret_at(10.0));
        doc.layout();
        assert_eq!(state(&doc).caret, caret_at(10.0));
        assert_eq!(selection(&doc), 1..1, "read back as the span's start");
        select(&mut doc, 3..3);
        doc.layout();
        assert_eq!(state(&doc).caret, caret_at(20.0));
        assert_eq!(selection(&doc), 3..3);
        // A deletion before `b` takes one of the value's spaces.
        assert!(doc.apply_editing(Command::DeleteBackward).applied);
        assert_eq!(value(&doc), "a b");
        assert_eq!(selection(&doc), 2..2);
        // Moving over the collapsed span crosses both units in one step.
        doc.set_text_node_data(doc.get(host).unwrap().child_ids()[0], "a  b");
        select(&mut doc, 1..1);
        doc.layout();
        assert!(
            doc.apply_editing(Command::Move {
                granularity: Granularity::Character,
                direction: Direction::Forward,
                extend: false,
            })
            .applied
        );
        assert_eq!(selection(&doc), 3..3);
    }
}

/// A host whose text no paragraph holds — empty, or white space a flex
/// container does not render (css-flexbox-1 §4) — is edited on value
/// offsets alone, its caret at the content box's origin, one line tall.
#[test]
fn a_host_with_no_paragraph_is_edited_on_value_offsets() {
    let (mut doc, host) = host_in(Kind::View, &[], 200.0);
    assert_eq!(doc.paragraph_node_of(host), None);
    assert!(doc.apply_editing(Command::Insert("  ".to_owned())).applied);
    doc.layout();
    assert_eq!(doc.paragraph_node_of(host), None, "white space only");
    assert_eq!(value(&doc), "  ");
    assert_eq!(selection(&doc), 2..2);
    let origin = doc.bounding_client_rect(host).unwrap().origin;
    assert_eq!(
        state(&doc).caret,
        rect(origin.x + 5.0, origin.y + 3.0, 1.0, 10.0)
    );
    let step = |doc: &mut Document<()>, granularity, direction| {
        assert!(
            doc.apply_editing(Command::Move {
                granularity,
                direction,
                extend: false,
            })
            .applied
        );
        selection(doc)
    };
    assert_eq!(
        step(&mut doc, Granularity::Character, Direction::Backward),
        1..1
    );
    assert_eq!(
        step(&mut doc, Granularity::LineBoundary, Direction::Backward),
        0..0
    );
    assert_eq!(
        step(&mut doc, Granularity::Document, Direction::Forward),
        2..2
    );
    assert!(doc.apply_editing(Command::DeleteBackward).applied);
    assert_eq!(value(&doc), " ");
    assert!(
        doc.apply_editing(Command::SelectFromPoint {
            point: Point2D::new(50.0, 5.0),
            extend: false,
        })
        .applied
    );
    assert_eq!(selection(&doc), 1..1, "every point is the value's end");
    // Text that renders brings the paragraph back.
    assert!(doc.apply_editing(Command::Insert("ab".to_owned())).applied);
    doc.layout();
    assert!(doc.paragraph_node_of(host).is_some());
    assert_eq!(
        state(&doc).caret,
        rect(origin.x + 5.0 + 30.0, origin.y + 3.0, 1.0, 10.0)
    );
}

/// The anonymous box is laid out inside its host like any flex item, so
/// where it sits — here at the end of the main axis — moves the caret and
/// the points a pointer command reads.
#[test]
fn the_anonymous_box_s_place_in_its_host_moves_the_caret() {
    let (mut doc, host) = host_in(Kind::View, &["ab"], 200.0);
    doc.set_inline_style_property(host, "justify-content", "flex-end");
    doc.layout();
    let paragraph = doc.paragraph_node_of(host).unwrap();
    assert_ne!(paragraph, host);
    let bounds = doc.bounding_client_rect(host).unwrap();
    let content_right = bounds.max_x() - 5.0;
    assert_eq!(
        doc.bounding_client_rect(paragraph).unwrap().min_x(),
        content_right - 20.0
    );
    select(&mut doc, 1..1);
    assert_eq!(
        state(&doc).caret,
        rect(content_right - 10.0, bounds.min_y() + 3.0, 1.0, 10.0)
    );
    let left = content_right - bounds.min_x() - 20.0;
    assert!(
        doc.apply_editing(Command::SelectFromPoint {
            point: Point2D::new(left + 18.0, 5.0),
            extend: false,
        })
        .applied
    );
    assert_eq!(selection(&doc), 2..2);
}

/// HTML's focusable areas are rendered: a `display: none` host, or one
/// inside a `display: none` element, cannot be focused, and a focused host
/// that stops being rendered loses focus at the next layout pass with no
/// event (the fixup rule).
#[test]
fn a_host_that_is_not_rendered_cannot_hold_focus() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["ab"], 200.0);
        doc.set_inline_style_property(host, "display", "none");
        assert_eq!(doc.focused(), Some(host), "until the pass");
        doc.layout();
        assert_eq!(doc.focused(), None);
        assert!(doc.take_focus_events().is_empty());
        assert!(
            !doc.get(host)
                .unwrap()
                .element_state()
                .contains(crate::ElementState::FOCUS)
        );
        assert!(!doc.focus(host));
        doc.set_inline_style_property(host, "display", "");
        doc.layout();
        assert!(doc.focus(host));

        let page = doc.document_element().id();
        let hidden = doc.create_element("view", ());
        doc.set_inline_style(hidden, "display: none;");
        doc.append_child(page, hidden);
        doc.append_child(hidden, host);
        doc.layout();
        assert_eq!(doc.focused(), None, "inside a display: none element");
        assert!(!doc.focus(host));
    }
}

/// A Lynx `<text>` keeps its truncation: past a `text-maxlength` cut the
/// value has no place in the layout, so a command that must measure there
/// is refused while value-offset commands still apply.
#[test]
fn a_truncated_text_refuses_to_measure_past_its_cut() {
    let (mut doc, host) = host_in(Kind::Text, &["abcdef"], 200.0);
    doc.add_stylesheet(
        r#"@property --lynx-text-maxlength { syntax: "<integer>"; inherits: false; initial-value: -1; }"#,
        StylesheetOrigin::UserAgent,
    );
    doc.set_inline_style_property(host, "--lynx-text-maxlength", "3");
    doc.layout();
    let paragraph = doc.editable_paragraph(host).unwrap();
    assert!(paragraph.map.truncated());
    assert_eq!(paragraph.map.cut_byte(), 3);
    let forward = Command::Move {
        granularity: Granularity::Character,
        direction: Direction::Forward,
        extend: false,
    };
    select(&mut doc, 1..1);
    doc.layout();
    assert!(doc.apply_editing(forward.clone()).applied);
    assert_eq!(selection(&doc), 2..2);
    assert!(!doc.apply_editing(forward.clone()).applied, "onto the cut");
    select(&mut doc, 5..5);
    doc.layout();
    assert_eq!(selection(&doc), 5..5, "kept as value offsets");
    assert!(!doc.apply_editing(forward).applied);
    assert!(!doc.apply_editing(Command::DeleteBackward).applied);
    assert!(doc.apply_editing(Command::Insert("x".to_owned())).applied);
    assert_eq!(value(&doc), "abcdexf");
    assert_eq!(selection(&doc), 6..6);
}
