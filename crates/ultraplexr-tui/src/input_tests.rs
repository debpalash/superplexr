use super::*;

#[test]
fn prefix_detaches_without_swallowing_normal_shell_controls() {
    let mut router = Router::default();
    assert!(matches!(
        router.route(KeyEvent::new(KeyCode::Char('c'), Mods::CONTROL)),
        Action::Key(_)
    ));
    assert_eq!(
        router.route(KeyEvent::new(KeyCode::Char(']'), Mods::CONTROL)),
        Action::Help
    );
    assert_eq!(
        router.route(KeyEvent::new(KeyCode::Char('d'), Mods::NONE)),
        Action::Detach
    );
    router.route(KeyEvent::new(KeyCode::Char(']'), Mods::CONTROL));
    assert!(matches!(
        router.route(KeyEvent::new(KeyCode::Char(']'), Mods::CONTROL)),
        Action::Key(_)
    ));
    assert_eq!(
        router.route(KeyEvent::new(KeyCode::Char('5'), Mods::CONTROL)),
        Action::Help
    );
    assert_eq!(
        router.route(KeyEvent::new(KeyCode::Char('r'), Mods::NONE)),
        Action::Release
    );
}

#[test]
fn keys_preserve_modifiers_unicode_arrows_and_backtab() {
    let backtab = encode(KeyEvent::new(KeyCode::BackTab, Mods::NONE)).expect("key");
    assert_eq!(backtab.physical_key, "tab");
    assert!(backtab.modifiers.shift);
    let unicode = encode(KeyEvent::new(KeyCode::Char('é'), Mods::NONE)).expect("key");
    assert_eq!(unicode.text.as_deref(), Some("é"));
    let arrow = encode(KeyEvent::new(KeyCode::Up, Mods::ALT)).expect("key");
    assert_eq!(arrow.physical_key, "up");
    assert!(arrow.modifiers.alt);
    assert!(risky_paste("echo yes\n"));
    assert!(risky_paste("\x1b]52;c;secret\x07"));
    assert!(!risky_paste("plain text"));
}

#[test]
fn search_is_local_only_after_prefix() {
    let mut router = Router::default();
    let slash = KeyEvent::new(KeyCode::Char('/'), Mods::NONE);
    assert!(matches!(router.route(slash), Action::Key(_)));
    router.route(KeyEvent::new(KeyCode::Char('5'), Mods::CONTROL));
    assert_eq!(router.route(slash), Action::Search);
    assert!(!router.waiting());
}
