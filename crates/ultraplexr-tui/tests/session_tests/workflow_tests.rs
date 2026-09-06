use super::*;
use ultraplexr_core::{Actor, MissionId};
use ultraplexr_tui::{
    navigator::{Navigator, Target},
    navigator_worker::Worker,
    workflow::CatalogPage,
};

#[test]
fn verifier_discovery_is_scoped_bounded_and_does_not_mutate_the_mission() {
    let fixture = Fixture::new();
    let mission = MissionId::new();
    fixture
        .owner
        .create_mission(
            mission,
            "Verifier discovery",
            Actor::human("test").expect("actor"),
        )
        .expect("Mission");
    let version = fixture.owner.get_mission(mission).expect("before").version;
    let (_, token) = fixture
        .owner
        .create_share(
            "Mission observer",
            ShareRole::Observer,
            vec![mission],
            vec![],
            60,
        )
        .expect("share");
    let observer =
        ControlClient::connect_with_share(fixture.root.join("s"), token).expect("observer");
    let mut nav = Navigator::default();
    nav.open_catalog(CatalogPage {
        mission,
        after: None,
    });
    nav.refresh(&observer)
        .expect("authorized compact discovery");
    assert_eq!(nav.entries.len(), 1);
    assert!(nav.entries[0].label.contains("No verifiers"));
    assert_eq!(nav.entries[0].target, Target::Unavailable);
    assert_eq!(nav.next_verifier_page(), None);
    assert!(nav.entries[0].detail.contains(&mission.to_string()));
    assert_eq!(
        fixture.owner.get_mission(mission).expect("after").version,
        version
    );

    let (_, token) = fixture
        .owner
        .create_share(
            "Session observer",
            ShareRole::Observer,
            vec![],
            vec![fixture.terminal.id()],
            60,
        )
        .expect("session share");
    let session_only =
        ControlClient::connect_with_share(fixture.root.join("s"), token).expect("session observer");
    assert!(
        nav.refresh(&session_only).is_err(),
        "Session scope must not imply Mission discovery"
    );
}

#[test]
fn verifier_discovery_worker_replaces_pending_scope_and_discards_cancelled_results() {
    let fixture = Fixture::new();
    let mission = MissionId::new();
    let other = MissionId::new();
    for id in [mission, other] {
        fixture
            .owner
            .create_mission(id, "Discovery worker", Actor::human("test").expect("actor"))
            .expect("Mission");
    }
    let worker = Worker::spawn(fixture.owner.clone()).expect("worker");
    let first = CatalogPage {
        mission,
        after: None,
    };
    let last = CatalogPage {
        mission: other,
        after: None,
    };
    worker.catalog(first);
    worker.cancel();
    assert!(worker.take().is_none());
    for _ in 0..20 {
        worker.catalog(first);
        worker.catalog(last);
    }
    let mut result = None;
    until(|| {
        result = worker.take();
        result.is_some()
    });
    let rows = result.expect("completion").expect("compact catalog");
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].detail.contains(&other.to_string()),
        "only latest scope is published"
    );
    assert!(!rows[0].detail.contains(&mission.to_string()));
    worker.catalog(first);
    worker.cancel();
    until(|| !worker.busy());
    assert!(
        worker.take().is_none(),
        "cancelled request cannot republish rows"
    );
}
