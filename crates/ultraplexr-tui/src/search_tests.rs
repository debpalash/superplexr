use super::*;
use ultraplexr_protocol::search_stream::SearchPage;

#[test]
fn query_and_result_retention_are_bounded_without_splitting_unicode() {
    let mut panel = Panel::new(SessionId::new());
    panel.append(&format!("\x1b\n{}", "界".repeat(1000)));
    assert_eq!(panel.query.len(), 1023);
    assert!(!panel.query.chars().any(char::is_control));
    let page = |preview| SearchPage {
        search_id: uuid::Uuid::new_v4(),
        session_id: panel.session_id,
        sequence: 1,
        matches: vec![SearchMatch {
            line: 0,
            column: 0,
            preview,
        }],
        complete: false,
        error: None,
    };
    let first = page("a".repeat(MAX_PREVIEW_BYTES));
    let second = page("b".into());
    assert!(panel.page(first));
    assert!(!panel.page(second));
    assert_eq!(panel.matches.len(), 1);
    assert!(panel.note.contains("display limit"));
    panel.reset();
    assert!(panel.matches.is_empty());
}
