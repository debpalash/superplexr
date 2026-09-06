use super::*;
use ultraplexr_protocol::{
    TerminalWaitCondition,
    search_stream::{MAX_PAGE_BYTES, SearchPage},
};

#[test]
fn cli_prints_incremental_pages_from_the_real_shared_runtime() {
    let fixture = Fixture::new();
    let client = fixture.client();
    let session = client.start_terminal(TerminalSessionSpec {
        session_id: SessionId::new(), mission_id: None, run_id: None,
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "awk 'BEGIN { for(i=0;i<200;i++) print \"CLI-ROW\" }'; printf 'READY\\n'; read -r done".into()],
        cwd: fixture.root.clone(), environment_delta: BTreeMap::new(),
        grid: GridSize::new(80, 24).expect("grid"),
    }).expect("real PTY");
    struct Kill(ultraplexr_client::DaemonSession);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
        }
    }
    let session = Kill(session);
    session
        .0
        .wait(
            TerminalWaitCondition::Text {
                query: "READY".into(),
                case_sensitive: true,
            },
            Duration::from_secs(3),
        )
        .expect("ready");
    let before = session.0.capture().expect("identity").terminal;
    let output = Process::new(env!("CARGO_BIN_EXE_ultraplexr"))
        .arg("--socket")
        .arg(fixture.root.join("s"))
        .arg("terminal-search-pages")
        .arg(session.0.id().to_string())
        .args(["CLI-ROW", "--case-sensitive", "--limit", "150"])
        .output()
        .expect("CLI process");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = std::str::from_utf8(&output.stdout)
        .expect("JSON lines")
        .lines()
        .collect::<Vec<_>>();
    assert!(lines.len() >= 3);
    let mut count = 0;
    for (index, line) in lines.iter().enumerate() {
        assert!(line.len() <= MAX_PAGE_BYTES);
        let page: SearchPage =
            serde_json::from_str(line).expect("one independently parseable page per line");
        assert_eq!(page.sequence, index as u64 + 1);
        assert_eq!(page.session_id, session.0.id());
        assert_eq!(page.complete, index + 1 == lines.len());
        count += page.matches.len();
    }
    assert_eq!(count, 150);
    assert_eq!(
        before.process_id,
        session.0.capture().expect("same PTY").terminal.process_id
    );
}
