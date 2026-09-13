use super::*;

#[test]
fn legacy_environment_aliases_preserve_values_removals_and_branded_precedence() {
    let mut env = BTreeMap::from([
        ("SUPERPLEXR_RUN_ID".to_owned(), Some("current".to_owned())),
        ("TERMI9NE_RUN_ID".to_owned(), Some("stale".to_owned())),
        ("ULTRAPLEXR_RUN_ID".to_owned(), Some("previous".to_owned())),
        (
            "ULTRAPLEXR_SESSION".to_owned(),
            Some("previous session".to_owned()),
        ),
        ("TERMI9NE_SESSION".to_owned(), Some("saved".to_owned())),
        ("SUPERPLEXR_AGENT_TOKEN_FD".to_owned(), None),
        (
            "TERMI9NE_AGENT_TOKEN_FD".to_owned(),
            Some("stale".to_owned()),
        ),
        ("PATH".to_owned(), Some("/bin".to_owned())),
    ]);
    add_legacy_environment_aliases(&mut env);
    assert_eq!(env["TERMI9NE_RUN_ID"], Some("current".to_owned()));
    assert_eq!(
        env["SUPERPLEXR_SESSION"],
        Some("previous session".to_owned())
    );
    assert_eq!(env["ULTRAPLEXR_RUN_ID"], Some("current".to_owned()));
    assert_eq!(env["ULTRAPLEXR_AGENT_TOKEN_FD"], None);
    assert_eq!(env["TERMI9NE_AGENT_TOKEN_FD"], None);
    assert_eq!(env["PATH"], Some("/bin".to_owned()));
    let once = env.clone();
    add_legacy_environment_aliases(&mut env);
    assert_eq!(env, once);
}
