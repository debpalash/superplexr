use super::*;

#[test]
fn verification_file_choices_round_trip_without_execution_or_changing_dismissals() {
    let root =
        std::env::temp_dir().join(format!("up-verification-settings-{}", uuid::Uuid::new_v4()));
    let store = WorkspaceStore::new(root.join("workspaces.json"));
    let mut document = WorkspaceDocument::new(
        1,
        2,
        ThemeSelection::default(),
        ProviderUsageSettings::default(),
        None,
        vec![],
    );
    let mission = MissionId::new();
    let run = superplexr_core::RunId::new();
    let setup = superplexr_verification::Setup {
        plan_path: "/private/project-checks.json".into(),
        evidence_root: "/private/evidence".into(),
        runner_path: "/usr/local/bin/superplexr".into(),
    };
    document.verification_setups.insert(mission, setup.clone());
    document.verification_runs.insert(run, setup);
    document.dismissals.missions.insert(mission);
    store
        .save(&document)
        .expect("save file choices, without touching referenced paths");
    assert_eq!(store.load().expect("load"), Some(document));
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn old_workspace_documents_do_not_enable_verification() {
    let document: WorkspaceDocument = serde_json::from_str(
        r#"{"version":3,"active_workspace":1,"next_sequence":2,"workspaces":[]}"#,
    )
    .expect("old document");
    assert!(document.verification_setups.is_empty());
    assert!(document.verification_runs.is_empty());
}
