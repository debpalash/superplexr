//! Opt-in Cargo binary verification. This is a recipe for the existing bounded
//! runner, not a second runner or a claim of hermetic/reproducible toolchains.
use super::*;
use std::path::Component;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    version: u16,
    cargo: PathBuf,
    package: PathBuf,
    binary: String,
    scratch_root: PathBuf,
    smoke_args: Vec<String>,
    expected_stdout: String,
}

impl Recipe {
    fn validate(&self) -> Result<()> {
        if self.version != 1
            || !self.cargo.is_absolute()
            || !self.scratch_root.is_absolute()
            || self.package.as_os_str().is_empty()
            || self
                .package
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
            || self.binary.is_empty()
            || self.binary.len() > 128
            || !self
                .binary
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
            || self.smoke_args.len() > 32
            || self
                .smoke_args
                .iter()
                .any(|arg| arg.len() > 1024 || arg.contains('\0'))
            || self.expected_stdout.len() > 1024
        {
            return invalid(
                "Rust recipe v1 requires absolute Cargo/scratch paths, a relative package without '..', a simple binary name, <=32 bounded smoke arguments, and <=1024 expected stdout bytes",
            );
        }
        private_directory(&self.scratch_root)?;
        Ok(())
    }

    fn cargo(&self, cwd: &Path, scratch: &Path) -> Process {
        let mut command = Process::new(&self.cargo);
        command
            .current_dir(cwd)
            .env("CARGO_TARGET_DIR", scratch.join("target"))
            .env("CARGO_INCREMENTAL", "0")
            .env("CARGO_BUILD_JOBS", "1")
            .env("CARGO_NET_OFFLINE", "true")
            .env("CARGO_TERM_COLOR", "never");
        command
    }
}

/// Write a new private six-check plan, embedding the reviewed recipe settings.
/// No project/tool command runs and existing output is never overwritten.
/// The result can be reviewed/launched by either desktop or the normal CLI flow.
pub fn write_rust_project_plan(recipe_path: &Path, output: &Path, runner: &Path) -> Result<()> {
    let recipe: Recipe = serde_json::from_slice(&read_private(recipe_path, MAX_DOCUMENT)?)?;
    recipe.validate()?;
    let recipe_json = serde_json::to_string(&recipe)?;
    let plan = Plan {
        version: 1,
        checks: REQUIRED
            .iter()
            .map(|id| Check {
                id: (*id).into(),
                program: runner.to_path_buf(),
                args: vec![
                    "verification-rust-check".into(),
                    "--check".into(),
                    (*id).into(),
                    "--recipe-json".into(),
                    recipe_json.clone(),
                ],
                timeout_seconds: 600,
                output_limit_bytes: MAX_OUTPUT,
            })
            .collect(),
    };
    plan.validate()?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    private_directory(parent)?;
    write_new(output, &serde_json::to_vec_pretty(&plan)?)
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(root: &Path, cwd: &Path) -> Result<Self> {
        let root = private_directory(root)?;
        if root.starts_with(cwd) {
            return invalid("Rust build scratch must be outside the Candidate checkout");
        }
        let path = root.join(format!("rust-check-{}", Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        // Only the UUID directory created by this invocation is retired. The
        // owner's scratch root, checkout, caches and install locations are not.
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("scratch cleanup failed for {}: {error}", self.0.display());
        }
    }
}

// Children inherit the outer check's process group. Giving nested Cargo/tool
// processes a new group would let them escape the bounded runner's cancellation.
// Stdout collection is bounded; stderr stays on the outer bounded pipe. A direct
// invocation has no outer timeout and is therefore not the recommended entry.
fn output(command: &mut Process) -> Result<Vec<u8>> {
    println!(
        "command: {:?} {:?}",
        command.get_program(),
        command.get_args().collect::<Vec<_>>()
    );
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let result = (|| -> Result<Vec<u8>> {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| VerificationError::Invalid("missing recipe stdout".into()))?;
        let mut bytes = Vec::new();
        stdout.take(MAX_OUTPUT as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_OUTPUT {
            return invalid("Rust check command exceeded 1 MiB stdout");
        }
        let status = child.wait()?;
        // Record exact tool output even when the command failed.
        std::io::stdout().write_all(&bytes)?;
        if !status.success() {
            return invalid(&format!("Rust check command failed: {status}"));
        }
        Ok(bytes)
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return invalid("expected a regular build input/output file");
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0; 16_384];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn install_and_smoke(recipe: &Recipe, package: &Path, scratch: &Path) -> Result<String> {
    let install = scratch.join("install");
    output(
        recipe
            .cargo(package, scratch)
            .args(["install", "--locked", "--offline", "--path"])
            .arg(package)
            .args(["--bin", &recipe.binary, "--root"])
            .arg(&install)
            .arg("--target-dir")
            .arg(scratch.join("target")),
    )?;
    let binary = install.join("bin").join(&recipe.binary);
    // Run away from the source tree, so a binary that needs undeclared files
    // from its checkout does not accidentally pass a delivery check.
    let bytes = output(
        Process::new(&binary)
            .args(&recipe.smoke_args)
            .current_dir(&install),
    )?;
    if bytes != recipe.expected_stdout.as_bytes() {
        return invalid(&format!(
            "installed binary stdout differs: expected {:?}, observed {:?}",
            recipe.expected_stdout,
            String::from_utf8_lossy(&bytes)
        ));
    }
    let sha256 = hash_file(&binary)?;
    println!("installed binary SHA-256 {sha256}");
    Ok(sha256)
}

fn provenance(recipe: &Recipe, cwd: &Path, package: &Path, scratch: &Path) -> Result<()> {
    output(recipe.cargo(package, scratch).arg("--version"))?;
    let bytes = output(recipe.cargo(package, scratch).args([
        "metadata",
        "--offline",
        "--locked",
        "--format-version",
        "1",
    ]))?;
    let metadata: serde_json::Value = serde_json::from_slice(&bytes)?;
    let workspace = metadata["workspace_root"].as_str().ok_or_else(|| {
        VerificationError::Invalid("Cargo metadata omitted workspace root".into())
    })?;
    let workspace = Path::new(workspace).canonicalize()?;
    if !workspace.starts_with(cwd) {
        return invalid("Cargo workspace escapes the Candidate checkout");
    }
    let lock = tracked_input(cwd, &workspace.join("Cargo.lock"))?;
    println!("Cargo.lock SHA-256 {}", hash_file(&lock)?);
    let packages = metadata["packages"]
        .as_array()
        .ok_or_else(|| VerificationError::Invalid("Cargo metadata omitted packages".into()))?;
    for dependency in packages {
        if !dependency["source"].is_null() {
            continue;
        }
        let path = dependency["manifest_path"]
            .as_str()
            .ok_or_else(|| VerificationError::Invalid("Cargo metadata omitted manifest".into()))?;
        let path = tracked_input(cwd, Path::new(path))?;
        println!(
            "local manifest {} SHA-256 {}",
            path.display(),
            hash_file(&path)?
        );
        let targets = dependency["targets"]
            .as_array()
            .ok_or_else(|| VerificationError::Invalid("Cargo metadata omitted targets".into()))?;
        for target in targets {
            let source = target["src_path"].as_str().ok_or_else(|| {
                VerificationError::Invalid("Cargo metadata omitted target source".into())
            })?;
            let source = tracked_input(cwd, Path::new(source))?;
            println!(
                "local target {} SHA-256 {}",
                source.display(),
                hash_file(&source)?
            );
        }
    }
    Ok(())
}

fn tracked_input(cwd: &Path, path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize()?;
    let relative = path.strip_prefix(cwd).map_err(|_| {
        VerificationError::Invalid("local build input escapes the frozen Candidate checkout".into())
    })?;
    output(
        Process::new("/usr/bin/git")
            .current_dir(cwd)
            .args([
                "-c",
                "core.fsmonitor=false",
                "ls-files",
                "--error-unmatch",
                "--",
            ])
            .arg(relative),
    )?;
    Ok(path)
}

/// Execute one recipe check as a child of the bounded runner. Only trusted local
/// project/tool code is supported. The outer runner enforces timeout, output,
/// process-group cleanup, Candidate identity and evidence/receipt semantics.
pub fn execute_rust_project_check(recipe_json: &str, check: &str) -> Result<()> {
    if recipe_json.len() > 4096 || !REQUIRED.contains(&check) {
        return invalid("unknown Rust check or oversized recipe");
    }
    let recipe: Recipe = serde_json::from_str(recipe_json)?;
    recipe.validate()?;
    let cwd = std::env::current_dir()?.canonicalize()?;
    let package = cwd.join(&recipe.package).canonicalize()?;
    if !package.starts_with(&cwd) || !package.join("Cargo.toml").is_file() {
        return invalid("Rust package must be inside the Candidate checkout with a Cargo.toml");
    }
    let scratch = Scratch::new(&recipe.scratch_root, &cwd)?;
    println!("Rust recipe v1: {check}; locked/offline, one build job; trusted local execution");
    match check {
        "format" => {
            output(
                recipe
                    .cargo(&package, &scratch.0)
                    .args(["fmt", "--all", "--", "--check"]),
            )?;
        }
        "lint" => {
            output(recipe.cargo(&package, &scratch.0).args([
                "clippy",
                "--workspace",
                "--all-targets",
                "--locked",
                "--offline",
                "--",
                "-D",
                "warnings",
            ]))?;
        }
        "tests" => {
            output(recipe.cargo(&package, &scratch.0).args([
                "test",
                "--workspace",
                "--locked",
                "--offline",
            ]))?;
        }
        "delivery" => {
            install_and_smoke(&recipe, &package, &scratch.0)?;
        }
        "provenance" => provenance(&recipe, &cwd, &package, &scratch.0)?,
        "repeatability" => {
            let other = Scratch::new(&recipe.scratch_root, &cwd)?;
            let first = install_and_smoke(&recipe, &package, &scratch.0)?;
            let second = install_and_smoke(&recipe, &package, &other.0)?;
            if first != second {
                return invalid("fresh installed binaries differ; build repeatability failed");
            }
            println!(
                "two fresh builds produced identical binary SHA-256 {first} and expected stdout"
            );
        }
        _ => unreachable!("check name validated above"),
    }
    Ok(())
}
