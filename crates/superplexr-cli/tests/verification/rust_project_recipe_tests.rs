use super::*;
use std::os::unix::fs::MetadataExt;
use superplexr_verification::{LaunchTarget, Setup, launch, prepare};

fn seed_project(fixture: &Fixture, external_dependency: bool) {
    let repo = fixture.root.join("repo");
    fs::create_dir(repo.join("src")).expect("source directory");
    let mut manifest =
        "[package]\nname = \"answer-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"
            .to_owned();
    if external_dependency {
        let external = fixture.root.join("external");
        fs::create_dir(&external).expect("external crate");
        fs::write(external.join("Cargo.toml"), "[package]\nname = \"outside\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\npath = \"lib.rs\"\n").expect("external manifest");
        fs::write(external.join("lib.rs"), "pub fn answer() -> u32 { 42 }\n")
            .expect("external source");
        manifest.push_str(&format!(
            "[dependencies]\noutside = {{ path = {:?} }}\n",
            external.to_string_lossy()
        ));
    }
    fs::write(repo.join("Cargo.toml"), manifest).expect("manifest");
    fs::write(
        repo.join("rust-toolchain.toml"),
        include_str!("../../../../rust-toolchain.toml"),
    )
    .expect("installed test toolchain");
    fs::write(
        repo.join("src/main.rs"),
        concat!(
            "fn main() {\n    print!(\"{}\", include_str!(\"../product\"));\n}\n\n",
            "#[cfg(test)]\nmod tests {\n    #[test]\n    fn actual_answer() {\n",
            "        assert_eq!(include_str!(\"../product\"), \"42\\n\");\n    }\n}\n"
        ),
    )
    .expect("real Rust executable and test");
    let generated = Process::new(env!("CARGO"))
        .current_dir(&repo)
        .args(["generate-lockfile", "--offline"])
        .output()
        .expect("generate lockfile offline");
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    for args in [
        vec!["add", "."],
        vec![
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-qm",
            "real Cargo fixture",
        ],
    ] {
        let result = Process::new("/usr/bin/git")
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()
            .expect("commit fixture");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

fn recipe(fixture: &Fixture, expected: &str) -> (PathBuf, serde_json::Value) {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(fixture.root.join("scratch"))
        .expect("private build scratch");
    let value = serde_json::json!({
        "version":1,"cargo":env!("CARGO"),"package":".","binary":"answer-fixture",
        "scratch_root":fixture.root.join("scratch"),"smoke_args":[],"expected_stdout":expected
    });
    let path = fixture.root.join("rust-recipe.json");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .expect("private recipe");
    file.write_all(&serde_json::to_vec(&value).expect("recipe JSON"))
        .expect("recipe bytes");
    (path, value)
}

fn generate(fixture: &Fixture, recipe: &PathBuf) -> std::process::Output {
    Process::new(env!("CARGO_BIN_EXE_superplexr"))
        .arg("verification-plan-rust")
        .arg("--recipe")
        .arg(recipe)
        .arg("--output")
        .arg(fixture.root.join("rust-plan.json"))
        .output()
        .expect("public recipe generation CLI")
}

fn run_recipe(
    fixture: &Fixture,
    client: &ControlClient,
    mission: MissionId,
    producer: RunId,
    expected: &str,
) -> RunId {
    let (recipe, _) = recipe(fixture, expected);
    let before = client.get_mission(mission).expect("before generating plan");
    let generated = generate(fixture, &recipe);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    assert_eq!(
        client
            .get_mission(mission)
            .expect("generation executes nothing")
            .version,
        before.version
    );
    let plan = fixture.root.join("rust-plan.json");
    assert_eq!(
        fs::metadata(&plan).expect("plan permissions").mode() & 0o777,
        0o600
    );
    let bytes = fs::read(&plan).expect("plan");
    assert!(
        !generate(fixture, &recipe).status.success(),
        "generation never overwrites"
    );
    assert_eq!(fs::read(&plan).expect("unchanged plan"), bytes);
    let setup = Setup {
        plan_path: plan,
        evidence_root: fixture.root.join("evidence"),
        runner_path: env!("CARGO_BIN_EXE_superplexr").into(),
    };
    let reviewed = prepare(&setup).expect("review generated commands");
    let outcome = launch(client, mission, LaunchTarget::Subject(producer), &reviewed)
        .expect("launch real recipe");
    assert!(outcome.launch_error.is_none(), "{:?}", outcome.launch_error);
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let state = client.get_mission(mission).expect("verifier state");
        if state.runs[&outcome.run_id].status.is_finished() {
            assert_eq!(
                state.runs[&outcome.run_id].outcome,
                Some(FinishOutcome::Succeeded),
                "examination should complete even when checks fail"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "real Cargo verification deadline"
        );
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        fs::read_dir(fixture.root.join("scratch"))
            .expect("scratch")
            .count(),
        0,
        "normal completion retires only per-check scratch"
    );
    outcome.run_id
}

#[test]
fn real_cargo_recipe_builds_installs_and_compares_fresh_binaries_after_restart() {
    let mut fixture = Fixture::new();
    seed_project(&fixture, false);
    let client = fixture.client();
    let (mission, producer, candidate) = subject(&fixture, &client);
    let verifier = run_recipe(&fixture, &client, mission, producer, "42\n");
    drop(client);
    fixture.restart();
    let result = fixture.collect(mission, verifier);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let receipt: EvaluationReceipt = serde_json::from_slice(&result.stdout).expect("real receipt");
    let dir = fixture.root.join("evidence").join(verifier.to_string());
    for index in 0..6 {
        assert!(
            receipt.checks[index].passed,
            "check {} failed\nstdout: {}\nstderr: {}",
            index,
            fs::read_to_string(dir.join(format!("{index}.stdout"))).expect("stdout"),
            fs::read_to_string(dir.join(format!("{index}.stderr"))).expect("stderr")
        );
    }
    assert_eq!(receipt.verdict, EvaluationVerdict::Passed);
    assert_eq!(receipt.candidate_sha256, candidate.content_sha256);
    assert!(
        fs::read_to_string(dir.join("2.stdout"))
            .expect("real test log")
            .contains("tests::actual_answer ... ok")
    );
    assert!(
        fs::read_to_string(dir.join("3.stdout"))
            .expect("delivery log")
            .contains("installed binary SHA-256")
    );
    assert!(
        fs::read_to_string(dir.join("4.stdout"))
            .expect("provenance log")
            .contains("Cargo.lock SHA-256")
    );
    assert!(
        fs::read_to_string(dir.join("5.stdout"))
            .expect("repeatability log")
            .contains("two fresh builds produced identical binary")
    );
    let client = fixture.client();
    assert_eq!(
        client
            .get_mission(mission)
            .expect("owner still decides")
            .runs[&producer]
            .disposition,
        RunDisposition::AwaitingReview
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("repo/product")).expect("original repo"),
        "base\n"
    );
    assert!(
        fixture.collect(mission, verifier).status.success(),
        "repeat collection never rebuilds"
    );
}

#[test]
fn real_cargo_recipe_rejects_wrong_delivery_and_external_path_provenance() {
    for external_dependency in [false, true] {
        let fixture = Fixture::new();
        seed_project(&fixture, external_dependency);
        let client = fixture.client();
        let (mission, producer, _) = subject(&fixture, &client);
        let verifier = run_recipe(
            &fixture,
            &client,
            mission,
            producer,
            if external_dependency { "42\n" } else { "43\n" },
        );
        let result = fixture.collect(mission, verifier);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let receipt: EvaluationReceipt =
            serde_json::from_slice(&result.stdout).expect("failed receipt");
        assert_eq!(receipt.verdict, EvaluationVerdict::Failed);
        assert!(
            receipt
                .checks
                .iter()
                .find(|check| check.name == "tests")
                .expect("tests")
                .passed,
            "working unit tests must not mask delivery/provenance failure"
        );
        let failed = if external_dependency {
            "provenance"
        } else {
            "delivery"
        };
        assert!(
            !receipt
                .checks
                .iter()
                .find(|check| check.name == failed)
                .expect("failed check")
                .passed
        );
        assert!(
            client
                .dispatch(
                    mission,
                    Command::AcceptRunResult {
                        run_id: producer,
                        by: ActorId::new("owner").expect("owner"),
                        note: "must not accept failed checks".into()
                    }
                )
                .is_err()
        );
    }
}

#[test]
fn recipe_generation_rejects_escaping_package_without_writing_or_executing() {
    let fixture = Fixture::new();
    let (path, mut value) = recipe(&fixture, "42\n");
    value["package"] = "../outside".into();
    fs::write(&path, serde_json::to_vec(&value).expect("JSON")).expect("invalid recipe");
    assert!(!generate(&fixture, &path).status.success());
    assert!(!fixture.root.join("rust-plan.json").exists());
    assert!(
        fixture
            .client()
            .list_all_terminals()
            .expect("no processes")
            .is_empty()
    );
}

#[test]
fn recipe_cancellation_kills_nested_tool_processes_without_a_receipt() {
    let fixture = Fixture::new();
    seed_project(&fixture, false);
    let client = fixture.client();
    let (mission, producer, candidate) = subject(&fixture, &client);
    let (recipe_path, mut value) = recipe(&fixture, "42\n");
    let marker = fixture.root.join("nested-tool.pid");
    let cargo = fixture.root.join("hanging-cargo");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&cargo)
        .expect("hanging tool");
    writeln!(
        file,
        "#!/bin/sh\nsleep 30 &\nnested=$!\nprintf '%s\\n' \"$nested\" > '{}'\nwait \"$nested\"",
        marker.display()
    )
    .expect("nested tool script");
    drop(file);
    value["cargo"] = serde_json::json!(cargo);
    fs::write(
        &recipe_path,
        serde_json::to_vec(&value).expect("recipe JSON"),
    )
    .expect("tool override");
    assert!(generate(&fixture, &recipe_path).status.success());
    let (run, session) = verifier(
        &fixture,
        &client,
        mission,
        producer,
        &candidate,
        fixture.root.join("rust-plan.json"),
    );
    wait(|| fs::read_to_string(&marker).is_ok_and(|text| text.trim().parse::<i32>().is_ok()));
    let nested = fs::read_to_string(&marker)
        .expect("nested PID")
        .trim()
        .parse::<i32>()
        .expect("PID");
    let worker = client
        .list_terminals()
        .expect("fixture workers")
        .iter()
        .find(|terminal| terminal.session_id == session)
        .expect("worker")
        .process_id
        .expect("PID");
    // SAFETY: the PID belongs to this fixture's runtime-launched worker.
    assert_eq!(unsafe { libc::kill(worker as i32, libc::SIGTERM) }, 0);
    wait(|| {
        client.get_mission(mission).expect("Mission").runs[&run]
            .status
            .is_finished()
    });
    // SAFETY: signal zero only probes the nested fixture process for cleanup.
    wait(|| unsafe { libc::kill(nested, 0) } != 0);
    assert!(!fixture.collect(mission, run).status.success());
    assert!(
        !fixture
            .root
            .join("evidence")
            .join(run.to_string())
            .join("report.json")
            .exists()
    );
    assert!(
        client
            .get_mission(mission)
            .expect("no receipt")
            .verified_delivery
            .evaluation_receipts
            .is_empty()
    );
}
