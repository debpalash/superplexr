use super::*;

fn check(script: &str) -> Check {
    Check {
        id: "tests".into(),
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        timeout_seconds: 1,
        output_limit_bytes: 1024,
    }
}

#[test]
fn runner_records_real_exit_status_and_separate_bounded_outputs() {
    let (result, out, err) = run_check(
        &check("printf output; printf error >&2; exit 7"),
        Path::new("/tmp"),
        &AtomicBool::new(false),
    )
    .expect("check");
    assert_eq!(result.outcome, Outcome::Exit { code: Some(7) });
    assert_eq!(out, b"output");
    assert_eq!(err, b"error");
    assert_eq!(result.stdout_sha256, digest(&out));
    assert_eq!(result.stderr_sha256, digest(&err));
}

#[test]
fn runner_stops_excessive_output_and_timed_out_process_groups() {
    let (result, out, _) = run_check(
        &check("while :; do printf 0123456789; done"),
        Path::new("/tmp"),
        &AtomicBool::new(false),
    )
    .expect("bounded output");
    assert_eq!(result.outcome, Outcome::OutputLimit);
    assert_eq!(out.len(), 1024);
    let started = Instant::now();
    let (result, _, _) = run_check(
        &check("sleep 20 & wait"),
        Path::new("/tmp"),
        &AtomicBool::new(false),
    )
    .expect("timeout");
    assert_eq!(result.outcome, Outcome::Timeout);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn runner_cleans_up_background_pipe_holders_and_fails_spawn_or_cancellation() {
    let started = Instant::now();
    let (result, _, _) = run_check(
        &check("sleep 20 & exit 0"),
        Path::new("/tmp"),
        &AtomicBool::new(false),
    )
    .expect("background cleanup");
    assert!(result.outcome.passed());
    assert!(started.elapsed() < Duration::from_secs(1));
    let mut missing = check("");
    missing.program = "/does-not-exist/superplexr-test".into();
    assert!(matches!(
        run_check(&missing, Path::new("/tmp"), &AtomicBool::new(false))
            .expect("spawn result")
            .0
            .outcome,
        Outcome::SpawnFailed { .. }
    ));
    assert!(run_check(&check("exit 0"), Path::new("/tmp"), &AtomicBool::new(true)).is_err());
}

#[test]
fn plan_requires_all_checks_and_explicit_bounded_commands() {
    let mut plan = Plan {
        version: 1,
        checks: REQUIRED
            .iter()
            .map(|id| {
                let mut c = check("exit 0");
                c.id = (*id).into();
                c
            })
            .collect(),
    };
    plan.validate().expect("complete plan");
    plan.checks[0].timeout_seconds = 0;
    assert!(plan.validate().is_err());
    plan.checks[0].timeout_seconds = 1;
    plan.checks[0].output_limit_bytes = MAX_OUTPUT + 1;
    assert!(plan.validate().is_err());
    plan.checks[0].output_limit_bytes = 1;
    plan.checks[0].program = "sh".into();
    assert!(plan.validate().is_err());
    plan.checks[0].program = "/bin/sh".into();
    plan.checks[0].id = "tests".into();
    assert!(plan.validate().is_err());
    plan.checks.pop();
    assert!(plan.validate().is_err());
    assert!(
        serde_json::from_str::<Plan>(r#"{"version":1,"checks":[],"allow_skip":true}"#).is_err()
    );
}

#[test]
fn private_evidence_rejects_symlinks_and_oversize_without_following_them() {
    use std::os::unix::fs::symlink;
    let root = PathBuf::from("/tmp").join(format!("up-check-file-{}", Uuid::new_v4()));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("fixture");
    let file = root.join("file");
    write_new(&file, b"evidence").expect("private evidence");
    assert_eq!(read_private(&file, 8).expect("bounded read"), b"evidence");
    assert!(read_private(&file, 7).is_err());
    symlink(&file, root.join("link")).expect("fixture link");
    assert!(read_private(&root.join("link"), 8).is_err());
    assert!(write_new(&file, b"replacement").is_err());
    fs::remove_dir_all(root).expect("fixture cleanup");
}
