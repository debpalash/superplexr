use super::*;

#[test]
fn fresh_legacy_and_dual_installations_resolve_without_moving_state() {
    let root = std::env::temp_dir().join(format!("superplexr-paths-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).expect("fixture root");
    for version in [None, Some(crate::PROTOCOL_VERSION)] {
        let (current, legacy) = match version {
            None => (root.join(".superplexr"), root.join(".termi9ne")),
            Some(v) => (
                root.join(format!(".superplexr-dev/v{v}")),
                root.join(format!(".termi9ne-dev/v{v}")),
            ),
        };
        assert_eq!(select_state_dir(&root, version), current);
        std::fs::create_dir_all(&legacy).expect("legacy fixture");
        std::fs::write(legacy.join("workspaces.json"), b"saved state").expect("saved fixture");
        assert_eq!(select_state_dir(&root, version), legacy);
        assert!(!current.exists(), "lookup must not migrate or create state");
        let previous = match version {
            None => root.join(".ultraplexr"),
            Some(v) => root.join(format!(".ultraplexr-dev/v{v}")),
        };
        std::fs::create_dir_all(&previous).expect("previous brand fixture");
        std::fs::write(previous.join("workspaces.json"), b"previous state").expect("saved fixture");
        assert_eq!(select_state_dir(&root, version), previous);
        assert!(!current.exists(), "lookup must preserve existing state");
        std::fs::create_dir_all(&current).expect("branded fixture");
        assert_eq!(select_state_dir(&root, version), current);
        assert_eq!(
            std::fs::read(legacy.join("workspaces.json")).unwrap(),
            b"saved state"
        );
    }
    std::fs::remove_dir_all(root).expect("remove own fixture");
}
