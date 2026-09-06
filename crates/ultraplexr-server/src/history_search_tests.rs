use super::*;
use crate::{TerminalSessionSpec, persist_terminal_spec, replay_terminal_model};
use std::{collections::BTreeMap, fs};
use ultraplexr_terminal::GridSize;

struct Archive {
    root: PathBuf,
    id: SessionId,
}
impl Archive {
    fn new() -> Self {
        let id = SessionId::new();
        let root = std::env::temp_dir().join(format!("up-search-replay-{id}"));
        let spec = TerminalSessionSpec {
            session_id: id,
            mission_id: None,
            run_id: None,
            program: "/bin/sh".into(),
            args: vec![],
            cwd: std::env::current_dir().expect("cwd"),
            environment_delta: BTreeMap::new(),
            grid: GridSize::new(80, 24).expect("grid"),
        };
        persist_terminal_spec(&spec, &root).expect("persist archive metadata");
        let journal = (0..4000)
            .map(|n| format!("ROW-{n:06}\r\n"))
            .collect::<String>();
        fs::write(
            root.join("sessions")
                .join(id.to_string())
                .join("output.raw"),
            journal,
        )
        .expect("journal");
        Self { root, id }
    }
    fn search(&self) -> ReplaySearch {
        ReplaySearch::start(self.root.clone(), self.id, "ROW".into(), true, 1000)
    }
}
impl Drop for Archive {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn archived_search_pages_complete_and_cancelled_worker_retires() {
    let archive = Archive::new();
    let mut search = archive.search();
    let mut count = 0;
    loop {
        let page = tokio::time::timeout(Duration::from_secs(2), search.next_page())
            .await
            .expect("page deadline")
            .expect("page");
        assert!(page.matches.len() <= 64);
        count += page.matches.len();
        if page.complete {
            break;
        }
    }
    assert_eq!(count, 1000);
    search._worker.await.expect("completed worker");

    let mut search = archive.search();
    assert!(!search.next_page().await.expect("admitted replay").complete);
    // One queued page plus the remaining >900 matches forces backpressure.
    // Do not abort a blocking task: prove dropping the consumer ends it.
    let worker = search._worker.abort_handle();
    let token = search._cancellation.0.clone();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !worker.is_finished(),
        "consumer should be holding native work"
    );
    drop(search);
    assert!(token.is_cancelled());
    tokio::time::timeout(Duration::from_secs(1), async {
        while !worker.is_finished() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelled replay must release its worker and native model");
}

#[test]
fn cancelled_replay_is_refused_before_accessing_files() {
    let token = SearchCancellation::default();
    token.cancel();
    assert!(matches!(
        replay_terminal_model_for_search(
            std::path::Path::new("/nonexistent-ultraplexr-fixture"),
            SessionId::new(),
            ReplayBudget::HISTORY,
            &token
        ),
        Err(RequestError::Runtime(RuntimeError::SearchIncomplete))
    ));
}

#[test]
fn search_replay_deadline_refuses_partial_success_without_changing_recovery() {
    let archive = Archive::new();
    let budget = ReplayBudget {
        bytes: ReplayBudget::HISTORY.bytes,
        deadline: Duration::ZERO,
    };
    assert!(
        matches!(replay_terminal_model_for_search(&archive.root, archive.id, budget, &SearchCancellation::default()), Err(RequestError::HistoryWorker(message)) if message.contains("deadline"))
    );
    assert!(
        replay_terminal_model(&archive.root, archive.id, budget).is_ok(),
        "startup recovery may still return a bounded partial frame"
    );
}
