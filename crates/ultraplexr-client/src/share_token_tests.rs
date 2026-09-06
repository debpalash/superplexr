use std::{
    ffi::CString,
    fs::{self, DirBuilder, OpenOptions, Permissions},
    io::{self, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

use crate::read_share_token_file;

const SECRET: &str = "private-token-must-never-appear-in-errors";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        // Keep socket fixtures below macOS's short sockaddr_un path limit.
        let path = std::env::temp_dir().join(format!("ut-{}", uuid::Uuid::new_v4().simple()));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn file(&self, name: &str, contents: &[u8], mode: u32) -> PathBuf {
        let path = self.path(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(contents).unwrap();
        // Set the exact fixture permissions independently of the test host's umask.
        file.set_permissions(Permissions::from_mode(mode)).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_redacted(error: &io::Error) {
    assert!(!error.to_string().contains(SECRET));
    assert!(!format!("{error:?}").contains(SECRET));
}

fn assert_invalid(path: &Path) {
    let error = read_share_token_file(path).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_redacted(&error);
}

#[test]
fn share_token_reads_private_files_and_trims_surrounding_whitespace() {
    let fixture = Fixture::new();
    for mode in [0o400, 0o600] {
        let path = fixture.file(
            &format!("token-{mode:o}"),
            format!(" \t\r\n\u{2003}{SECRET}\u{2003}\n").as_bytes(),
            mode,
        );
        assert_eq!(read_share_token_file(&path).unwrap(), SECRET);
    }
}

#[test]
fn share_token_limit_counts_utf8_bytes_after_trimming() {
    let fixture = Fixture::new();
    for (name, token) in [("ascii", "x".repeat(512)), ("utf8", "é".repeat(256))] {
        let path = fixture.file(name, format!("\n{token}\n").as_bytes(), 0o600);
        assert_eq!(read_share_token_file(&path).unwrap(), token);
    }
    for (name, token) in [
        (
            "ascii-over",
            format!("{SECRET}{}", "x".repeat(513 - SECRET.len())),
        ),
        ("utf8-over", "é".repeat(257)),
    ] {
        assert_invalid(&fixture.file(name, token.as_bytes(), 0o600));
    }
}

#[test]
fn share_token_file_limit_includes_padding_before_trimming() {
    let fixture = Fixture::new();
    let mut contents = SECRET.as_bytes().to_vec();
    contents.resize(16_384, b' ');
    let exact = fixture.file("exact-file-limit", &contents, 0o600);
    assert_eq!(read_share_token_file(&exact).unwrap(), SECRET);
    contents.push(b' ');
    assert_invalid(&fixture.file("over-file-limit", &contents, 0o600));
}

#[test]
fn share_token_rejects_empty_invalid_utf8_and_internal_separators() {
    let fixture = Fixture::new();
    for (index, contents) in [
        Vec::new(),
        b" \r\n\t".to_vec(),
        format!("{SECRET} second").into_bytes(),
        format!("{SECRET}\tsecond").into_bytes(),
        format!("{SECRET}\nsecond").into_bytes(),
        format!("{SECRET}\u{2003}second").into_bytes(),
        format!("{SECRET}\0second").into_bytes(),
        format!("{SECRET}\u{7f}second").into_bytes(),
        format!("{SECRET}\u{1b}").into_bytes(),
        [SECRET.as_bytes(), &[0xff]].concat(),
    ]
    .into_iter()
    .enumerate()
    {
        assert_invalid(&fixture.file(&format!("invalid-{index}"), &contents, 0o600));
    }
}

#[test]
fn share_token_rejects_each_group_or_other_permission() {
    let fixture = Fixture::new();
    for mode in [0o640, 0o620, 0o610, 0o604, 0o602, 0o601, 0o666] {
        assert_invalid(&fixture.file(&format!("mode-{mode:o}"), SECRET.as_bytes(), mode));
    }
}

#[test]
fn share_token_rejects_final_symlink_but_allows_symlinked_ancestor() {
    let fixture = Fixture::new();
    let actual = fixture.file("actual", SECRET.as_bytes(), 0o600);
    let link = fixture.path("final-link");
    symlink(&actual, &link).unwrap();
    assert_redacted(&read_share_token_file(&link).unwrap_err());

    let directory = fixture.path("directory-link");
    symlink(&fixture.0, &directory).unwrap();
    assert_eq!(
        read_share_token_file(&directory.join("actual")).unwrap(),
        SECRET
    );
}

#[test]
fn share_token_rejects_directory_socket_and_missing_file_without_path_disclosure() {
    let fixture = Fixture::new();
    assert_invalid(&fixture.0);
    let socket_path = fixture.path("socket");
    let _listener = UnixListener::bind(&socket_path).unwrap();
    assert_redacted(&read_share_token_file(&socket_path).unwrap_err());
    let missing = fixture.path(SECRET);
    let error = read_share_token_file(&missing).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert_redacted(&error);
}

#[test]
fn share_token_rejects_fifo_without_waiting_for_writer() {
    let fixture = Fixture::new();
    let path = fixture.path("fifo");
    let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: c_path is a live NUL-terminated path in this private fixture.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

    let (sender, receiver) = mpsc::channel();
    let read_path = path.clone();
    let worker = thread::spawn(move || {
        let _ = sender.send(read_share_token_file(&read_path));
    });
    let result = receiver.recv_timeout(Duration::from_secs(5));
    if result.is_err() {
        // If O_NONBLOCK regresses, release a reader blocked in open before
        // failing, rather than leaving the test process stuck on that FIFO.
        let _writer = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path);
        let _ = receiver.recv_timeout(Duration::from_secs(5));
        panic!("credential FIFO read waited for a writer");
    }
    worker.join().unwrap();
    let error = result.unwrap().unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_redacted(&error);
}
