mod common;

use common::Doc;
use neutron_star::geometry::Size;
use neutron_star::tree::AvailableSpace;
use stylo_dom::layout::{DomLayoutSession, DomLayoutSource, DomLayoutSourceError};

const AHEM: &[u8] = include_bytes!("../../neutron-star/tests/fixtures/Ahem.ttf");

fn assert_text_commits_through(display: &str) {
    let css = format!(
        "page {{ display:{display}; width:40px; \
         font-family:Ahem; font-size:10px; line-height:10px; word-break:break-all; }}"
    );
    let mut doc = Doc::with_css(&css);
    let text = doc.arena.insert_text("abcdefgh");
    let index = doc.arena.children_len(doc.root);
    doc.arena.attach_at(doc.root, text, index);
    doc.flush();

    let source = DomLayoutSource::new(&doc.arena, doc.root).expect("styled root projects");
    let mut session = DomLayoutSession::<()>::without_system_fonts();
    assert_eq!(session.register_fonts(AHEM), 1);
    let _ = session.commit(
        &source,
        Size::new(AvailableSpace::Definite(40.0), AvailableSpace::MaxContent),
        1.0,
    );

    let text_box = session
        .final_layout(&source, text)
        .expect("DOM Text resolves to its anonymous item's output box");
    let paragraph = session
        .committed_text_layout(&source, text)
        .expect("algorithm dispatch commits a retained paragraph");
    assert!(text_box.size.width > 0.0);
    assert!(text_box.size.height > 0.0);
    assert!(paragraph.line_count() >= 2);
    assert!(paragraph.first_baseline().is_some());
}

#[test]
fn session_commits_and_queries_real_dom_text_nodes() {
    let mut doc = Doc::with_css("page { display:flex; width:80px; font:10px/10px Ahem; }");
    let text = doc
        .arena
        .insert_text("anonymous DOM text participates in flex layout");
    let index = doc.arena.children_len(doc.root);
    doc.arena.attach_at(doc.root, text, index);
    doc.flush();

    let source = DomLayoutSource::new(&doc.arena, doc.root).expect("styled root projects");
    let mut session = DomLayoutSession::<()>::without_system_fonts();
    assert_eq!(session.register_fonts(AHEM), 1);
    let root_layout = session.commit(
        &source,
        Size::new(AvailableSpace::Definite(80.0), AvailableSpace::MaxContent),
        1.0,
    );

    assert_eq!(session.final_layout(&source, doc.root), Some(root_layout));
    let text_box = session.final_layout(&source, text).unwrap();
    let anonymous = source.anonymous_text_child(doc.root).unwrap();
    assert_eq!(
        session.formatting_layout(&source, anonymous),
        Some(text_box)
    );
    let paragraph = session
        .committed_text_layout(&source, text)
        .expect("a committed Text node retains its Parley artifact");
    assert!(core::ptr::eq(
        session.formatting_text_layout(&source, anonymous).unwrap(),
        paragraph
    ));
    assert!(paragraph.line_count() >= 1);
    assert!(session.committed_text_layout(&source, doc.root).is_none());
}

#[test]
fn a_new_dom_revision_hides_stale_results_until_the_next_commit() {
    let mut doc =
        Doc::with_css("page { display:flex; width:80px; font-family:Ahem; font-size:10px; }");
    let text = doc.arena.insert_text("before");
    let index = doc.arena.children_len(doc.root);
    doc.arena.attach_at(doc.root, text, index);
    doc.flush();

    let mut session = DomLayoutSession::<()>::without_system_fonts();
    assert_eq!(session.register_fonts(AHEM), 1);
    {
        let source = DomLayoutSource::new(&doc.arena, doc.root).expect("styled root projects");
        let _ = session.commit(
            &source,
            Size::new(AvailableSpace::Definite(80.0), AvailableSpace::MaxContent),
            1.0,
        );
        assert!(session.final_layout(&source, text).is_some());
    }

    assert!(doc.arena.set_text(text, "after mutation"));
    let changed = DomLayoutSource::new(&doc.arena, doc.root).expect("changed root projects");
    assert!(session.final_layout(&changed, text).is_none());
    assert!(session.committed_text_layout(&changed, text).is_none());

    let _ = session.commit(
        &changed,
        Size::new(AvailableSpace::Definite(80.0), AvailableSpace::MaxContent),
        1.0,
    );
    assert!(session.final_layout(&changed, text).is_some());
    assert!(session.committed_text_layout(&changed, text).is_some());
}

#[test]
fn equal_revisions_from_distinct_arenas_do_not_alias_session_results() {
    let css = "page { display:flex; font:10px/10px Ahem; }";
    let mut first = Doc::with_css(css);
    let first_text = first.arena.insert_text("first");
    first.arena.attach_at(first.root, first_text, 0);
    first.flush();

    let mut second = Doc::with_css(css);
    let second_text = second.arena.insert_text("second");
    second.arena.attach_at(second.root, second_text, 0);
    second.flush();

    let first_source = DomLayoutSource::new(&first.arena, first.root).unwrap();
    let second_source = DomLayoutSource::new(&second.arena, second.root).unwrap();
    assert_eq!(first_source.revision(), second_source.revision());
    assert_eq!(first.root, second.root);
    assert_eq!(first_text, second_text);

    let mut session = DomLayoutSession::<()>::without_system_fonts();
    assert_eq!(session.register_fonts(AHEM), 1);
    let _ = session.commit(&first_source, Size::MAX_CONTENT, 1.0);
    assert!(session.final_layout(&first_source, first_text).is_some());
    assert!(session.final_layout(&second_source, second_text).is_none());
}

#[test]
fn visible_descendants_require_a_completed_style_flush() {
    let mut doc = Doc::with_css("page { display:flex; }");
    doc.flush();

    let child = doc.el(doc.root, "view");
    assert_eq!(
        DomLayoutSource::new(&doc.arena, doc.root).unwrap_err(),
        DomLayoutSourceError::MissingStyle(child)
    );
}

#[test]
fn display_none_root_does_not_expose_a_generated_box() {
    let mut doc = Doc::with_css("page { display:none; }");
    doc.flush();

    let source = DomLayoutSource::new(&doc.arena, doc.root).unwrap();
    let mut session = DomLayoutSession::<()>::without_system_fonts();
    let _ = session.commit(&source, Size::MAX_CONTENT, 1.0);
    assert!(session.final_layout(&source, doc.root).is_none());
    assert!(
        session
            .formatting_layout(&source, source.root_node())
            .is_none()
    );
}

#[test]
fn flex_and_grid_commit_w3c_anonymous_text_items() {
    assert_text_commits_through("flex");
    assert_text_commits_through("grid");
}

#[test]
fn linear_and_relative_commit_dom_text_as_project_extensions() {
    assert_text_commits_through("linear");
    assert_text_commits_through("relative");
}
