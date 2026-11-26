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

use euclid::default::{Point2D, Rect, Size2D};
use seldon::text::FontBlob;

use super::*;
use crate::style::engine::StylesheetOrigin;

const HOST_RULE: &str = "[contenteditable]:not([contenteditable=\"false\" i]) \
                         { white-space: pre-wrap; }";

const FLOW_RULE: &str = ".flow { display: block; }";

const MIXED: &str = "ab😀中\ncde\u{301}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Text,
    View,
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
        "../../../seldon/tests/fixtures/Ahem.ttf"
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
    let widths: Vec<f32> = hosts
        .iter()
        .map(|&host| paragraph_size(&doc, host).0)
        .collect();
    assert_eq!(widths, [40.0, 40.0, 30.0, 30.0, 30.0]);
}

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
    assert_eq!(doc.take_focus_events(), []);

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
            .contains(stylo_dom::ElementState::FOCUS)
    );

    doc.append_child(page, host);
    assert!(doc.focus(host));
    doc.take_focus_events();
    doc.set_attribute(host, "contenteditable", "False");
    assert_eq!(doc.focused(), None, "no longer a host");
    assert_eq!(doc.take_focus_events(), []);
    doc.set_attribute(host, "contenteditable", "true");
    assert!(doc.focus(host));
    doc.remove_attribute(host, "contenteditable");
    assert_eq!(doc.focused(), None);
}

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
        select(&mut doc, 2..5);
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
        select(&mut doc, 4..4);
        assert!(doc.apply_editing(Command::DeleteBackward).applied);
        assert_eq!(value(&doc), "ab中\ncde");
        assert_eq!(selection(&doc), 2..2);
        select(&mut doc, 4..4);
        assert!(doc.apply_editing(Command::DeleteBackward).applied);
        assert_eq!(value(&doc), "ab中cde");
        select(&mut doc, 0..0);
        let outcome = doc.apply_editing(Command::DeleteBackward);
        assert!(!outcome.applied);
        assert_eq!(outcome.events, []);
    }
}

#[test]
fn delete_forward_takes_a_whole_cluster() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &[MIXED], 200.0);
        select(&mut doc, 2..2);
        let outcome = doc.apply_editing(Command::DeleteForward);
        assert_eq!(
            kinds(&outcome)[0],
            ("beforeinput", "deleteContentForward", None, false)
        );
        assert_eq!(value(&doc), "ab中\ncde\u{301}");
        assert_eq!(selection(&doc), 2..2);
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
        select(&mut doc, 11..11);
        let outcome = doc.apply_editing(Command::DeleteWordBackward);
        assert_eq!(kinds(&outcome)[0].1, "deleteWordBackward");
        assert_eq!(value(&doc), "hello  world");
        assert_eq!(selection(&doc), 6..6);
        let outcome = doc.apply_editing(Command::DeleteWordForward);
        assert_eq!(kinds(&outcome)[0].1, "deleteWordForward");
        assert_eq!(value(&doc), "hello world");
        assert!(doc.apply_editing(Command::DeleteWordForward).applied);
        assert_eq!(value(&doc), "hello ");
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
        assert_eq!(step(&mut doc, Character, Forward, false), 4..4);
        assert_eq!(step(&mut doc, Character, Backward, true), 2..4);
        assert_eq!(step(&mut doc, Character, Backward, false), 2..2);
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
        select(&mut doc, 3..3);
        assert_eq!(selection(&doc), 2..2);
        select(&mut doc, 4..99);
        assert_eq!(selection(&doc), 4..10);
        select(&mut doc, Range { start: 6, end: 1 });
        assert_eq!(selection(&doc), 1..6);
    }
}

#[test]
fn pointer_commands_place_and_extend_by_cluster_word_and_line() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
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
            assert_eq!(outcome.events, []);
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
        let origin = bounds.origin;
        assert_eq!(
            input.caret,
            rect(origin.x + 5.0 + 10.0, origin.y + 3.0 + 10.0, 1.0, 10.0)
        );
        assert_eq!(input.composition_bounds, []);

        doc.set_attribute(host, "inputmode", "bogus");
        assert_eq!(state(&doc).input_mode, InputMode::Default);

        for (value, expected) in [
            ("password", InputType::Password),
            ("EMAIL", InputType::Email),
            ("number", InputType::Number),
            ("tel", InputType::Tel),
            ("search", InputType::Search),
            ("url", InputType::Url),
            ("digit", InputType::Text),
        ] {
            doc.set_attribute(host, "type", value);
            assert_eq!(state(&doc).input_type, expected, "{value:?}");
        }
    }
}

#[test]
fn a_value_written_by_the_element_layer_ends_the_composition_and_places_the_selection() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["ab", "c"], 200.0);
        assert!(
            doc.apply_editing(Command::SetComposition {
                text: "x".to_owned(),
                selection: 1..1,
            })
            .applied
        );
        assert_eq!(state(&doc).composition, Some(0..1));
        doc.set_host_value(host, "h😀llo", 3..3);
        let input = state(&doc);
        assert_eq!(input.value, "h😀llo");
        assert_eq!(input.selection, 3..3);
        assert_eq!(input.composition, None);
        let texts = doc.get(host).unwrap().child_ids().len();
        assert_eq!(texts, 1);
        doc.set_host_value(host, "h😀llo", 2..99);
        assert_eq!(state(&doc).selection, 1..6);
        assert!(doc.apply_editing(Command::Insert("!".to_owned())).applied);
        assert_eq!(value(&doc), "h!");
        doc.layout();
        assert_eq!(selection(&doc), 2..2);

        let other = doc.create_element("view", ());
        doc.append_child(doc.document_element().id(), other);
        doc.set_host_value(other, "plain", 0..0);
        assert_eq!(doc.host_value(other), "plain");
        assert_eq!(doc.focused(), Some(host));
        assert!(!doc.has_focus_events());
        assert!(doc.blur());
        assert!(doc.has_focus_events());
        doc.take_focus_events();
        assert!(!doc.has_focus_events());
    }
}

#[test]
fn a_host_keeps_its_selection_while_it_is_not_focused() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["hello"], 200.0);
        select(&mut doc, 1..3);
        doc.layout();
        assert_eq!(doc.host_selection(host), Some(1..3), "the session's");
        assert!(doc.blur());
        doc.take_focus_events();
        assert_eq!(doc.focused(), None);
        assert_eq!(doc.host_selection(host), Some(1..3));

        assert!(doc.focus(host));
        doc.take_focus_events();
        assert_eq!(selection(&doc), 1..3, "restored, as value offsets");
        doc.layout();
        assert_eq!(selection(&doc), 1..3, "placed in the paragraph");

        let other = kind.create(&mut doc);
        doc.set_attribute(other, "contenteditable", "");
        doc.append_child(doc.document_element().id(), other);
        let text = doc.create_text_node("abc", ());
        doc.append_child(other, text);
        doc.layout();
        assert_eq!(doc.host_selection(other), None, "never focused");
        assert!(doc.focus(other));
        doc.take_focus_events();
        assert_eq!(selection(&doc), 0..0);
        assert_eq!(doc.host_selection(host), Some(1..3));

        doc.set_host_value(host, "h😀llo", 2..99);
        assert_eq!(doc.host_selection(host), Some(1..6));
        assert!(doc.focus(host));
        doc.take_focus_events();
        assert_eq!(selection(&doc), 1..6);
        doc.set_host_value(host, "h😀llo", 3..6);
        assert!(doc.blur());
        doc.take_focus_events();

        let text = doc.get(host).unwrap().first_child().unwrap().id();
        doc.set_text_node_data(text, "ab😀");
        assert_eq!(doc.host_selection(host), Some(2..4));
        assert!(doc.focus(host));
        doc.take_focus_events();
        assert_eq!(selection(&doc), 2..4);
        doc.layout();
        assert_eq!(selection(&doc), 2..4);
    }
}

#[test]
fn a_retained_selection_outlives_editing_but_not_the_node() {
    let (mut doc, host) = host_in(Kind::View, &["hello"], 200.0);
    select(&mut doc, 2..4);
    doc.set_attribute(host, "contenteditable", "false");
    assert_eq!(doc.focused(), None);
    assert_eq!(doc.take_focus_events(), []);
    assert_eq!(doc.host_selection(host), Some(2..4), "read-only keeps it");
    doc.set_attribute(host, "contenteditable", "");
    assert!(doc.focus(host));
    assert_eq!(selection(&doc), 2..4);
    doc.remove_element(host);
    assert_eq!(doc.focused(), None);
    assert_eq!(doc.host_selection(host), Some(2..4), "removed, not freed");
    assert_eq!(doc.editing.retained.len(), 1);
    doc.drop_element(host);
    assert!(
        doc.editing.retained.is_empty(),
        "a retired id keeps no entry"
    );
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
fn an_empty_host_s_caret_is_its_paragraph_s_one_empty_line() {
    for kind in KINDS {
        let mut doc = document();
        let host = kind.create(&mut doc);
        doc.set_attribute(host, "contenteditable", "plaintext-only");
        doc.set_inline_style(host, "width: 200px; padding: 3px 5px; line-height: normal;");
        doc.append_child(doc.document_element().id(), host);
        doc.layout();
        assert!(doc.focus(host), "{kind:?}");
        doc.layout();
        let origin = doc.bounding_client_rect(host).unwrap().origin;
        let caret = state(&doc).caret;
        if kind == Kind::View {
            assert!(doc.editable_paragraph(host).is_none());
            assert_eq!(caret, rect(origin.x + 5.0, origin.y + 3.0, 1.0, 12.0));
        } else {
            let paragraph = doc
                .editable_paragraph(host)
                .expect("the host's own paragraph");
            assert_eq!(paragraph.node, host, "{kind:?}");
            assert_eq!(doc.paragraph_line_count(host), Some(1), "{kind:?}");
            assert_eq!(
                caret,
                rect(origin.x + 5.0, origin.y + 3.0, 1.0, 10.0),
                "{kind:?}"
            );
            assert_eq!(
                doc.bounding_client_rect(host).unwrap().size.height,
                16.0,
                "{kind:?}"
            );
        }
    }
}

#[test]
fn an_element_with_a_line_if_empty_is_one_line_tall_without_being_a_host() {
    for kind in [Kind::Text, Kind::Block] {
        let mut doc = document();
        let element = kind.create(&mut doc);
        doc.set_attribute(element, "contenteditable", "false");
        doc.set_inline_style(element, "width: 200px; padding: 3px 5px;");
        doc.append_child(doc.document_element().id(), element);
        doc.layout();
        let height = |doc: &Document<()>| doc.bounding_client_rect(element).unwrap().size.height;
        assert_eq!(height(&doc), 6.0, "{kind:?}: padding only");

        doc.set_has_line_if_empty(element, true);
        doc.layout();
        assert!(!doc.is_editing_host(element));
        assert_eq!(doc.paragraph_line_count(element), Some(1), "{kind:?}");
        assert_eq!(height(&doc), 16.0, "{kind:?}: the line and the padding");

        doc.set_attribute(element, "contenteditable", "");
        doc.layout();
        assert_eq!(height(&doc), 16.0, "{kind:?}: a host");
        doc.set_has_line_if_empty(element, false);
        doc.layout();
        assert_eq!(height(&doc), 16.0, "{kind:?}: a host without the bit");
        doc.remove_attribute(element, "contenteditable");
        doc.layout();
        assert_eq!(height(&doc), 6.0, "{kind:?}: neither");
    }
}

#[test]
fn commands_over_an_empty_host_s_one_line_stay_at_its_start() {
    use Direction::{Backward, Forward};
    use Granularity::{Character, Document as Whole, Line, LineBoundary, Word};
    for kind in [Kind::Text, Kind::Block] {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &[], 200.0);
        assert!(doc.editable_paragraph(host).is_some());
        for x in [1.0, 6.0, 13.0, 40.0] {
            for command in [
                Command::SelectFromPoint {
                    point: Point2D::new(x, 8.0),
                    extend: false,
                },
                Command::SelectWordAtPoint {
                    point: Point2D::new(x, 8.0),
                },
                Command::SelectLineAtPoint {
                    point: Point2D::new(x, 8.0),
                },
            ] {
                doc.apply_editing(command);
                assert_eq!(selection(&doc), 0..0, "{kind:?} at {x}");
            }
        }
        for granularity in [Character, Word, LineBoundary, Line, Whole] {
            for direction in [Forward, Backward] {
                doc.apply_editing(Command::Move {
                    granularity,
                    direction,
                    extend: false,
                });
                assert_eq!(selection(&doc), 0..0, "{kind:?} {granularity:?}");
            }
        }
        assert!(doc.apply_editing(Command::Insert("ab".to_owned())).applied);
        doc.layout();
        assert_eq!((value(&doc), selection(&doc)), ("ab".to_owned(), 2..2));
        assert!(doc.apply_editing(Command::DeleteBackward).applied);
        assert!(doc.apply_editing(Command::DeleteBackward).applied);
        doc.layout();
        assert_eq!((value(&doc), selection(&doc)), (String::new(), 0..0));
        assert_eq!(doc.paragraph_line_count(host), Some(1), "{kind:?}");
        let origin = doc.bounding_client_rect(host).unwrap().origin;
        assert_eq!(
            state(&doc).caret,
            rect(origin.x + 5.0, origin.y + 3.0, 1.0, 10.0)
        );
    }
}

#[test]
fn an_empty_host_s_caret_follows_text_align_exactly() {
    for kind in [Kind::Text, Kind::Block] {
        for (align, offset) in [
            ("start", 0.0),
            ("left", 0.0),
            ("center", 95.0),
            ("right", 190.0),
            ("end", 190.0),
            ("justify", 0.0),
        ] {
            let mut doc = document();
            let host = kind.create(&mut doc);
            doc.set_attribute(host, "contenteditable", "");
            doc.set_inline_style(
                host,
                &format!(
                    "width: 200px; box-sizing: border-box; padding: 3px 5px; \
                     text-align: {align};"
                ),
            );
            doc.append_child(doc.document_element().id(), host);
            doc.layout();
            assert!(doc.focus(host), "{kind:?}");
            doc.layout();
            let origin = doc.bounding_client_rect(host).unwrap().origin;
            assert_eq!(
                state(&doc).caret,
                rect(origin.x + 5.0 + offset, origin.y + 3.0, 1.0, 10.0),
                "{kind:?} text-align: {align}"
            );
        }
    }
}

#[test]
fn a_paragraph_that_does_not_show_the_value_refuses_edits_but_takes_selections() {
    for kind in [Kind::Text, Kind::Block] {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["ab"], 200.0);
        doc.add_stylesheet(".generated { content: 'XYZ'; }", StylesheetOrigin::Author);
        doc.add_class(host, "generated");
        doc.layout();
        assert!(!doc.editable_paragraph(host).unwrap().shows_value);
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
fn a_masked_host_edits_its_value_over_a_layout_of_masks() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["a\u{1F600} b"], 200.0);
        doc.set_inline_style(
            host,
            "width: 200px; padding: 3px 5px; -webkit-text-security: disc;",
        );
        doc.layout();
        assert!(doc.editable_paragraph(host).unwrap().shows_value);
        assert_eq!(paragraph_size(&doc, host), (40.0, 10.0));
        let origin = doc.bounding_client_rect(host).unwrap().origin;

        select(&mut doc, 1..1);
        assert!(
            doc.apply_editing(Command::Move {
                granularity: Granularity::Character,
                direction: Direction::Forward,
                extend: false,
            })
            .applied
        );
        doc.layout();
        doc.layout();
        let input = state(&doc);
        assert_eq!(input.value, "a\u{1F600} b");
        assert_eq!(input.selection, 3..3);
        assert_eq!(
            input.caret,
            rect(origin.x + 5.0 + 20.0, origin.y + 3.0, 1.0, 10.0)
        );

        assert!(doc.apply_editing(Command::Insert("x".to_owned())).applied);
        doc.layout();
        let input = state(&doc);
        assert_eq!(input.value, "a\u{1F600}x b");
        assert_eq!(input.selection, 4..4);
        assert_eq!(
            input.caret,
            rect(origin.x + 5.0 + 30.0, origin.y + 3.0, 1.0, 10.0)
        );

        assert!(doc.apply_editing(Command::DeleteBackward).applied);
        assert!(doc.apply_editing(Command::DeleteBackward).applied);
        doc.layout();
        let input = state(&doc);
        assert_eq!(input.value, "a b");
        assert_eq!(input.selection, 1..1);
        assert_eq!(
            input.caret,
            rect(origin.x + 5.0 + 10.0, origin.y + 3.0, 1.0, 10.0)
        );
        assert!(
            doc.apply_editing(Command::Move {
                granularity: Granularity::Character,
                direction: Direction::Forward,
                extend: false,
            })
            .applied
        );
        assert_eq!(selection(&doc), 2..2);
    }
}

#[test]
fn a_carriage_return_is_shaped_as_a_space_and_edits_stay_aligned() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["a\r\nb"], 200.0);
        assert!(doc.editable_paragraph(host).unwrap().shows_value);
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
        assert_eq!(
            kinds(&outcome),
            [
                ("beforeinput", "deleteContentBackward", None, false),
                ("input", "deleteContentBackward", None, false),
            ]
        );
        assert_eq!(value(&doc), "ad");
        assert_eq!(selection(&doc), 1..1);
        assert_eq!(state(&doc).composition, None);
        assert!(
            !doc.apply_editing(Command::SetComposition {
                text: String::new(),
                selection: 0..0,
            })
            .applied
        );
    }
}

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
        assert_eq!(state(&doc).caret, caret_at(30.0));
        select(&mut doc, 2..2);
        assert_eq!(state(&doc).caret, caret_at(10.0));
        doc.layout();
        assert_eq!(state(&doc).caret, caret_at(10.0));
        assert_eq!(selection(&doc), 1..1, "read back as the span's start");
        select(&mut doc, 3..3);
        doc.layout();
        assert_eq!(state(&doc).caret, caret_at(20.0));
        assert_eq!(selection(&doc), 3..3);
        assert!(doc.apply_editing(Command::DeleteBackward).applied);
        assert_eq!(value(&doc), "a b");
        assert_eq!(selection(&doc), 2..2);
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
    assert!(doc.apply_editing(Command::Insert("ab".to_owned())).applied);
    doc.layout();
    assert!(doc.paragraph_node_of(host).is_some());
    assert_eq!(
        state(&doc).caret,
        rect(origin.x + 5.0 + 30.0, origin.y + 3.0, 1.0, 10.0)
    );
}

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

#[test]
fn a_host_that_is_not_rendered_cannot_hold_focus() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["ab"], 200.0);
        doc.set_inline_style_property(host, "display", "none");
        assert_eq!(doc.focused(), Some(host), "until the pass");
        doc.layout();
        assert_eq!(doc.focused(), None);
        assert_eq!(doc.take_focus_events(), []);
        assert!(
            !doc.get(host)
                .unwrap()
                .element_state()
                .contains(stylo_dom::ElementState::FOCUS)
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

#[test]
fn each_host_s_paragraph_reports_its_kind_and_its_lines() {
    for kind in KINDS {
        let (mut doc, host) = host_in(kind, &["aa aa\nb"], 40.0);
        let paragraph = doc.paragraph_node_of(host).unwrap();
        let expected = if kind == Kind::Text {
            crate::layout::ParagraphKind::Lynx
        } else {
            crate::layout::ParagraphKind::InlineFormattingContext
        };
        assert_eq!(doc.paragraph_kind(paragraph), Some(expected), "{kind:?}");
        assert_eq!(doc.paragraph_line_count(paragraph), Some(3), "{kind:?}");
        let end = value(&doc).encode_utf16().count();
        select(&mut doc, end..end);
        assert!(doc.apply_editing(Command::InsertLineBreak).applied);
        doc.layout();
        let paragraph = doc.paragraph_node_of(host).unwrap();
        let trailing = usize::from(kind == Kind::Text);
        assert_eq!(
            doc.paragraph_line_count(paragraph),
            Some(3 + trailing),
            "{kind:?}: a trailing break"
        );
    }
    let (doc, host) = host_in(Kind::View, &["a"], 40.0);
    assert_eq!(
        doc.paragraph_kind(host),
        None,
        "the anonymous box's, not the view's"
    );
    assert_eq!(doc.paragraph_line_count(host), None);
}

fn took_selectionchange(doc: &mut Document<()>) -> bool {
    let taken = doc.take_selectionchange();
    assert!(
        !doc.has_pending_selectionchange(),
        "the take clears the flag"
    );
    taken
}

fn anchor_focus(doc: &Document<()>) -> (u32, u32) {
    let selection = doc.selection().expect("a focused host has a selection");
    (selection.anchor, selection.focus)
}

#[test]
fn focus_sets_the_selectionchange_flag_once() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let mut doc = document();
        let host = kind.create(&mut doc);
        doc.set_attribute(host, "contenteditable", "");
        doc.append_child(doc.document_element().id(), host);
        doc.layout();
        assert!(!doc.has_pending_selectionchange());
        assert_eq!(doc.selection(), None, "nothing focused, no selection");

        assert!(doc.focus(host));
        assert!(doc.has_pending_selectionchange());
        assert!(took_selectionchange(&mut doc));
        assert!(!took_selectionchange(&mut doc), "taken once");
        assert_eq!(
            doc.selection(),
            Some(DocumentSelection {
                host,
                anchor: 0,
                focus: 0
            })
        );

        assert!(doc.focus(host), "the focused host");
        assert!(!doc.has_pending_selectionchange(), "no new session");
    }
}

#[test]
fn a_caret_move_sets_selectionchange_and_the_selection_keeps_its_direction() {
    let extend = |doc: &mut Document<()>, direction| {
        assert!(
            doc.apply_editing(Command::Move {
                granularity: Granularity::Character,
                direction,
                extend: true,
            })
            .applied
        );
    };
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &[MIXED], 200.0);
        took_selectionchange(&mut doc);

        extend(&mut doc, Direction::Forward);
        assert!(took_selectionchange(&mut doc));
        assert_eq!(anchor_focus(&doc), (0, 1));
        extend(&mut doc, Direction::Forward);
        extend(&mut doc, Direction::Forward);
        assert!(took_selectionchange(&mut doc), "two moves, one flag");
        assert_eq!(anchor_focus(&doc), (0, 4), "over the pair: two units");
        assert_eq!(doc.selection().map(|selection| selection.host), Some(host));

        select(&mut doc, 4..4);
        assert!(took_selectionchange(&mut doc));
        extend(&mut doc, Direction::Backward);
        assert!(took_selectionchange(&mut doc));
        assert_eq!(anchor_focus(&doc), (4, 2), "backward: focus before anchor");
        assert_eq!(selection(&doc), 2..4, "the input state orders them");
    }
}

#[test]
fn a_set_selection_to_the_held_range_does_not_set_selectionchange() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, _) = host_in(kind, &["hello"], 200.0);
        took_selectionchange(&mut doc);

        select(&mut doc, 1..3);
        assert!(took_selectionchange(&mut doc));
        select(&mut doc, 1..3);
        assert!(!took_selectionchange(&mut doc), "held as value offsets");
        doc.layout();
        assert!(
            !took_selectionchange(&mut doc),
            "placing the offsets in the paragraph changes none of them"
        );
        select(&mut doc, 1..3);
        assert!(!took_selectionchange(&mut doc), "held in the paragraph");
        select(&mut doc, Range { start: 3, end: 1 });
        assert!(took_selectionchange(&mut doc), "the other direction");
        assert_eq!(anchor_focus(&doc), (3, 1));
        select(&mut doc, 2..3);
        assert!(took_selectionchange(&mut doc));
        assert!(doc.apply_editing(Command::Insert("x".to_owned())).applied);
        assert!(took_selectionchange(&mut doc), "a value change");
    }
}

#[test]
fn a_value_write_sets_selectionchange_only_when_it_changes_something() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["hello"], 200.0);
        took_selectionchange(&mut doc);

        doc.set_host_value(host, "hello", 1..2);
        assert!(took_selectionchange(&mut doc));
        doc.set_host_value(host, "hello", 1..2);
        assert!(
            !took_selectionchange(&mut doc),
            "the same value and offsets"
        );
        doc.set_host_value(host, "hello", 0..2);
        assert!(took_selectionchange(&mut doc), "other offsets");
        doc.set_host_value(host, "help", 0..2);
        assert!(took_selectionchange(&mut doc), "another value");
        assert_eq!(anchor_focus(&doc), (0, 2));

        let other = doc.create_element("view", ());
        doc.append_child(doc.document_element().id(), other);
        doc.set_host_value(other, "plain", 0..0);
        assert!(!took_selectionchange(&mut doc), "not the focused host");
    }
}

#[test]
fn a_blur_or_the_fixup_rule_ends_the_document_selection() {
    for kind in KINDS {
        eprintln!("kind {kind:?}");
        let (mut doc, host) = host_in(kind, &["hello"], 200.0);
        select(&mut doc, 1..3);
        took_selectionchange(&mut doc);

        assert!(doc.blur());
        assert!(took_selectionchange(&mut doc));
        assert_eq!(doc.selection(), None);
        assert_eq!(
            doc.host_selection(host),
            Some(1..3),
            "the host retains its own"
        );
        assert!(!doc.blur());
        assert!(!took_selectionchange(&mut doc), "nothing was focused");

        assert!(doc.focus(host));
        doc.take_focus_events();
        took_selectionchange(&mut doc);
        doc.remove_attribute(host, "contenteditable");
        assert_eq!(doc.focused(), None);
        assert!(doc.take_focus_events().is_empty(), "no focus event");
        assert!(took_selectionchange(&mut doc), "the fixup rule's ending");
        assert_eq!(doc.selection(), None);
    }
}

const RAW_TEXT_RULE: &str = "raw-text { display: contents; content: attr(text); }";

const ATOM: char = '\u{FFFC}';

fn selectable_document() -> Document<()> {
    let mut doc = document();
    crate::test_common::define_lynx_text(&mut doc, "inline-text");
    doc.add_stylesheet(RAW_TEXT_RULE, StylesheetOrigin::UserAgent);
    doc
}

fn selectable_root() -> (Document<()>, NodeId, NodeId) {
    let mut doc = selectable_document();
    let root = doc.create_element("text", ());
    doc.set_inline_style(root, "width: 200px; user-select: text;");
    doc.append_child(doc.document_element().id(), root);
    let ab = doc.create_text_node("ab", ());
    doc.append_child(root, ab);
    let raw = doc.create_element("raw-text", ());
    doc.set_attribute(raw, "text", "cd");
    doc.append_child(root, raw);
    let nested = doc.create_element("inline-text", ());
    doc.append_child(root, nested);
    let ef = doc.create_text_node("ef", ());
    doc.append_child(nested, ef);
    let atom = doc.create_element("view", ());
    doc.set_inline_style(atom, "display: flex; width: 10px; height: 10px;");
    doc.append_child(root, atom);
    let gh = doc.create_text_node("gh", ());
    doc.append_child(root, gh);
    doc.layout();
    (doc, root, nested)
}

fn on_line(x: f32) -> Point2D<f32> {
    Point2D::new(x, 5.0)
}

fn select_from(doc: &mut Document<()>, x: f32, extend: bool) {
    assert!(
        doc.apply_editing(Command::SelectFromPoint {
            point: on_line(x),
            extend,
        })
        .applied
    );
}

#[test]
fn begin_selection_needs_a_rendered_selectable_paragraph() {
    let (mut doc, root, nested) = selectable_root();
    assert!(doc.is_user_selectable(root));
    let ab = doc.get(root).unwrap().child_ids()[0];

    assert_eq!(doc.selectable_root_of(ab), Some(root), "from a text node");
    assert_eq!(doc.selectable_root_of(root), Some(root));
    let ef = doc.get(nested).unwrap().child_ids()[0];
    assert_eq!(
        doc.selectable_root_of(ef),
        Some(root),
        "a nested scope has no paragraph of its own"
    );
    assert!(!doc.begin_selection(nested));
    assert_eq!(doc.selection_host(), None);

    let page = doc.document_element().id();
    assert_eq!(doc.selectable_root_of(page), None, "no paragraph");
    let plain = doc.create_element("text", ());
    doc.set_inline_style(plain, "user-select: none;");
    doc.append_child(page, plain);
    let text = doc.create_text_node("plain", ());
    doc.append_child(plain, text);
    doc.layout();
    assert_eq!(doc.selectable_root_of(text), None, "user-select: none");
    assert!(!doc.begin_selection(plain));

    let detached = doc.create_element("text", ());
    doc.set_inline_style(detached, "user-select: text;");
    assert!(!doc.begin_selection(detached), "not in the document");

    doc.set_inline_style_property(root, "display", "none");
    doc.layout();
    assert_eq!(doc.selectable_root_of(ab), None, "not rendered");
    assert!(!doc.begin_selection(root));
    assert!(!doc.has_pending_selectionchange(), "nothing was selected");
    doc.set_inline_style_property(root, "display", "");
    doc.layout();
    assert!(doc.begin_selection(root));
    assert_eq!(doc.selection_host(), Some(root));
    assert_eq!(doc.focused(), None, "selected, not focused");
    assert_eq!(
        doc.selection(),
        Some(DocumentSelection {
            host: root,
            anchor: 0,
            focus: 0
        })
    );
}

#[test]
fn begin_selection_blurs_a_focused_host() {
    let (mut doc, root, _) = selectable_root();
    let host = doc.create_element("view", ());
    doc.set_attribute(host, "contenteditable", "");
    doc.append_child(doc.document_element().id(), host);
    let text = doc.create_text_node("field", ());
    doc.append_child(host, text);
    doc.layout();
    assert!(doc.focus(host));
    select(&mut doc, 1..3);
    doc.take_focus_events();
    took_selectionchange(&mut doc);

    assert!(doc.begin_selection(root));
    assert_eq!(doc.focused(), None);
    assert_eq!(doc.selection_host(), Some(root));
    let events: Vec<_> = doc
        .take_focus_events()
        .into_iter()
        .map(|event| (event.kind, event.target, event.related_target))
        .collect();
    assert_eq!(
        events,
        [
            (FocusEventKind::Blur, host, None),
            (FocusEventKind::FocusOut, host, None)
        ]
    );
    assert!(took_selectionchange(&mut doc));
    assert_eq!(doc.host_selection(host), Some(1..3), "retained");
    assert_eq!(
        doc.text_input_state(),
        TextInputState::default(),
        "nothing an input method edits"
    );
    assert!(
        !doc.get(host)
            .unwrap()
            .element_state()
            .contains(crate::link::ElementState::FOCUS)
    );
}

#[test]
fn pointer_selection_reads_source_units_and_the_selected_text() {
    let (mut doc, root, _) = selectable_root();
    assert!(doc.begin_selection(root));
    doc.layout();
    took_selectionchange(&mut doc);

    select_from(&mut doc, 12.0, false);
    assert!(took_selectionchange(&mut doc));
    assert_eq!(anchor_focus(&doc), (1, 1));
    assert_eq!(doc.selected_text(root).as_deref(), Some(""), "collapsed");
    select_from(&mut doc, 72.0, true);
    assert!(took_selectionchange(&mut doc));
    assert_eq!(anchor_focus(&doc), (1, 7));
    assert_eq!(
        doc.selected_text(root),
        Some(format!("bcdef{ATOM}")),
        "the generated `cd`, the nested `ef` and the atom"
    );
    select_from(&mut doc, 8.0, false);
    select_from(&mut doc, 32.0, true);
    select_from(&mut doc, 88.0, false);
    select_from(&mut doc, 28.0, true);
    assert_eq!(anchor_focus(&doc), (9, 3));
    assert_eq!(doc.selected_text(root), Some(format!("def{ATOM}gh")));

    assert_eq!(doc.text_content_offset(root, 3), Some(3));
    assert_eq!(doc.text_content_offset(root, 6), Some(6), "before the atom");
    assert_eq!(doc.text_content_offset(root, 7), Some(6), "after it");
    assert_eq!(doc.text_content_offset(root, 9), Some(8), "the end");
    assert_eq!(doc.text_content_offset(root, 10), None, "past the end");
    let page = doc.document_element().id();
    assert_eq!(doc.text_content_offset(page, 0), None, "no paragraph");
    assert_eq!(doc.selected_text(page), None, "not the selection host");
}

#[test]
fn a_word_selects_in_a_selectable_root() {
    let mut doc = selectable_document();
    let root = doc.create_element("text", ());
    doc.set_inline_style(root, "width: 200px; user-select: text;");
    doc.append_child(doc.document_element().id(), root);
    let text = doc.create_text_node("hello world", ());
    doc.append_child(root, text);
    doc.layout();
    assert!(doc.begin_selection(root));

    assert!(
        doc.apply_editing(Command::SelectWordAtPoint {
            point: on_line(75.0)
        })
        .applied
    );
    assert_eq!(anchor_focus(&doc), (6, 11));
    assert_eq!(doc.selected_text(root).as_deref(), Some("world"));
    assert!(doc.apply_editing(Command::SelectAll).applied);
    assert_eq!(doc.selected_text(root).as_deref(), Some("hello world"));
}

#[test]
fn a_selectable_root_refuses_edits_and_compositions() {
    let (mut doc, root, _) = selectable_root();
    assert!(doc.begin_selection(root));
    select_from(&mut doc, 12.0, false);
    select_from(&mut doc, 32.0, true);
    took_selectionchange(&mut doc);
    for command in [
        Command::Insert("x".to_owned()),
        Command::InsertLineBreak,
        Command::DeleteBackward,
        Command::DeleteForward,
        Command::DeleteWordBackward,
        Command::DeleteWordForward,
        Command::DeleteSurrounding {
            before: 1,
            after: 1,
        },
        Command::ExtendSelectionAndDelete {
            before: 1,
            after: 0,
        },
        Command::SetComposition {
            text: "x".to_owned(),
            selection: 1..1,
        },
        Command::CommitText {
            text: "x".to_owned(),
            relative_cursor: 0,
        },
        Command::SetCompositionFromExistingText { range: 0..2 },
        Command::FinishComposition {
            keep_selection: true,
        },
    ] {
        assert!(
            !doc.apply_editing(command.clone()).applied,
            "{command:?} is refused"
        );
    }
    assert!(!took_selectionchange(&mut doc));
    assert_eq!(anchor_focus(&doc), (1, 3));
    assert_eq!(doc.selected_text(root).as_deref(), Some("bc"));
    let ab = doc.get(root).unwrap().child_ids()[0];
    assert_eq!(
        doc.get(ab).and_then(crate::tree::node::Node::text),
        Some("ab")
    );

    assert!(
        doc.apply_editing(Command::Move {
            granularity: Granularity::Character,
            direction: Direction::Forward,
            extend: true,
        })
        .applied
    );
    assert_eq!(anchor_focus(&doc), (1, 4));
    select(&mut doc, 7..9);
    assert_eq!(doc.selected_text(root).as_deref(), Some("gh"));
}

#[test]
fn a_layout_pass_keeps_a_root_s_selection() {
    let (mut doc, root, _) = selectable_root();
    assert!(doc.begin_selection(root));
    select(&mut doc, 2..5);
    took_selectionchange(&mut doc);
    doc.layout();
    assert!(!took_selectionchange(&mut doc), "placing changes nothing");
    assert_eq!(anchor_focus(&doc), (2, 5));
    assert!(
        doc.editing.session.as_ref().unwrap().pending.is_none(),
        "placed in the paragraph"
    );

    doc.set_inline_style_property(root, "width", "40px");
    doc.layout();
    assert_eq!(anchor_focus(&doc), (2, 5));
    assert_eq!(doc.selected_text(root).as_deref(), Some("cde"));
    assert!(!took_selectionchange(&mut doc));
    assert_eq!(doc.selection_host(), Some(root));
}

#[test]
fn clear_and_repeated_begin_selection_set_the_flag_only_on_change() {
    let (mut doc, root, _) = selectable_root();
    assert!(!doc.clear_selection(), "nothing selected");
    assert!(!took_selectionchange(&mut doc));

    assert!(doc.begin_selection(root));
    assert!(took_selectionchange(&mut doc));
    select(&mut doc, 1..2);
    assert!(took_selectionchange(&mut doc));
    assert!(doc.begin_selection(root), "already in the root");
    assert!(!took_selectionchange(&mut doc));
    assert_eq!(anchor_focus(&doc), (1, 2), "kept");

    assert!(doc.clear_selection());
    assert!(took_selectionchange(&mut doc));
    assert_eq!(doc.selection(), None);
    assert_eq!(doc.selected_text(root), None);
    assert!(doc.take_focus_events().is_empty(), "no focus event");
    assert!(!doc.clear_selection());

    let host = doc.create_element("view", ());
    doc.set_attribute(host, "contenteditable", "");
    doc.append_child(doc.document_element().id(), host);
    doc.layout();
    assert!(doc.focus(host));
    took_selectionchange(&mut doc);
    assert!(!doc.clear_selection(), "a focused host's session stays");
    assert_eq!(doc.focused(), Some(host));
    assert!(!took_selectionchange(&mut doc));
}

#[test]
fn a_root_s_session_ends_with_its_root() {
    let (mut doc, root, _) = selectable_root();
    assert!(doc.begin_selection(root));
    took_selectionchange(&mut doc);
    doc.remove_element(root);
    assert_eq!(doc.selection_host(), None, "left the document");
    assert!(took_selectionchange(&mut doc));
    assert_eq!(doc.take_focus_events(), []);

    doc.append_child(doc.document_element().id(), root);
    doc.layout();
    assert!(doc.begin_selection(root));
    took_selectionchange(&mut doc);
    doc.set_inline_style_property(root, "user-select", "none");
    assert_eq!(
        doc.selection_host(),
        Some(root),
        "a style change shows after the next pass"
    );
    doc.layout();
    assert_eq!(doc.selection_host(), None, "user-select: none");
    assert!(took_selectionchange(&mut doc));

    doc.set_inline_style_property(root, "user-select", "text");
    doc.layout();
    assert!(doc.begin_selection(root));
    took_selectionchange(&mut doc);
    doc.set_inline_style_property(root, "display", "none");
    doc.layout();
    assert_eq!(doc.selection_host(), None, "not rendered");
    assert!(took_selectionchange(&mut doc));

    doc.set_inline_style_property(root, "display", "");
    doc.layout();
    assert!(doc.begin_selection(root));
    took_selectionchange(&mut doc);
    let host = doc.create_element("view", ());
    doc.set_attribute(host, "contenteditable", "");
    doc.append_child(doc.document_element().id(), host);
    doc.layout();
    assert!(doc.focus(host));
    assert_eq!(doc.selection_host(), Some(host), "focus replaces it");
    assert_eq!(doc.focused(), Some(host));
    assert!(took_selectionchange(&mut doc));
    let events: Vec<_> = doc
        .take_focus_events()
        .into_iter()
        .map(|event| (event.kind, event.target, event.related_target))
        .collect();
    assert_eq!(
        events,
        [
            (FocusEventKind::Focus, host, None),
            (FocusEventKind::FocusIn, host, None)
        ],
        "no blur at the root, which was never focused"
    );
}

#[test]
fn the_used_user_select_decides_and_a_freed_root_ends_its_session() {
    let mut doc = selectable_document();
    let page = doc.document_element().id();
    let outer = doc.create_element("view", ());
    doc.set_inline_style(outer, "user-select: none;");
    doc.append_child(page, outer);
    let text_under = |doc: &mut Document<()>, parent: NodeId, style: &str| {
        let text = doc.create_element("text", ());
        doc.set_inline_style(text, style);
        doc.append_child(parent, text);
        let run = doc.create_text_node("abc", ());
        doc.append_child(text, run);
        (text, run)
    };
    let (inherited, inherited_run) = text_under(&mut doc, outer, "");
    let (opted_in, opted_in_run) = text_under(&mut doc, outer, "user-select: text;");
    let (contained, contained_run) = text_under(&mut doc, outer, "user-select: contain;");
    let (whole, whole_run) = text_under(&mut doc, outer, "user-select: all;");
    let (default, default_run) = text_under(&mut doc, page, "");
    doc.layout();

    assert!(!doc.is_user_selectable(inherited_run), "auto under none");
    assert_eq!(doc.selectable_root_of(inherited_run), None);
    assert!(!doc.begin_selection(inherited));
    assert_eq!(doc.selectable_root_of(opted_in_run), Some(opted_in));
    assert_eq!(doc.selectable_root_of(contained_run), Some(contained));
    assert_eq!(doc.selectable_root_of(whole_run), Some(whole));
    assert!(
        doc.is_user_selectable(default_run),
        "the root's auto is text"
    );
    assert_eq!(doc.selectable_root_of(default_run), Some(default));

    let unstyled = doc.create_element("text", ());
    assert!(doc.is_user_selectable(unstyled), "no flush yet: auto");
    doc.append_child(outer, unstyled);
    assert!(!doc.is_user_selectable(unstyled), "the parent's none");

    assert!(doc.begin_selection(opted_in));
    took_selectionchange(&mut doc);
    doc.remove_element(opted_in);
    doc.drop_element(opted_in);
    assert_eq!(doc.selection_host(), None, "freed");
    assert!(took_selectionchange(&mut doc));
}
