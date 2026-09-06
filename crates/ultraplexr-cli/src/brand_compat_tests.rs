use super::*;

#[test]
fn legacy_shell_block_is_upgraded_in_place_and_remains_idempotent() {
    for shell in [Shell::Zsh, Shell::Bash, Shell::Fish] {
        let old = format!(
            "# user before\n{LEGACY_BEGIN_MARKER}\nold integration\n{LEGACY_END_MARKER}\n# user after\n"
        );
        let (updated, outcome) = install_into(&old, shell);
        assert_eq!(outcome, InstallOutcome::Updated);
        assert!(updated.starts_with("# user before\n"));
        assert!(updated.ends_with("# user after\n"));
        assert!(!updated.contains(LEGACY_BEGIN_MARKER));
        assert_eq!(updated.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(
            install_into(&updated, shell),
            (updated, InstallOutcome::Unchanged)
        );
        assert!(snippet(shell).contains("TERMI9NE_SESSION"));
    }
}
