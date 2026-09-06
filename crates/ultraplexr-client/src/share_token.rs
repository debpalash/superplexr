//! Bounded Unix credential-file loading shared by all native frontends.
use std::{
    fs::OpenOptions,
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

const MAX_FILE_BYTES: u64 = 16_384;
const MAX_TOKEN_BYTES: usize = 512;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Open once, validate the descriptor, and read a bounded token. The final path
/// component cannot be a symlink; FIFO opens cannot wait for a writer. Ancestor
/// symlinks are not prohibited. This neither connects nor grants authority.
/// Errors never contain token bytes. The returned string is not zeroized.
pub fn read_share_token_file(path: &Path) -> io::Result<String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let before = file.metadata()?;
    // SAFETY: geteuid only reads the effective process credentials.
    let uid = unsafe { libc::geteuid() };
    if !before.is_file()
        || before.uid() != uid
        || before.mode() & 0o077 != 0
        || before.len() > MAX_FILE_BYTES
    {
        return Err(invalid(
            "Share token must be an effective-user-owned private regular file of at most 16 KiB",
        ));
    }
    let mut text = String::new();
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| invalid("Share token file could not be read as bounded UTF-8"))?;
    let after = file.metadata()?;
    if text.len() > MAX_FILE_BYTES as usize
        || before.len() != after.len()
        || before.uid() != after.uid()
        || before.mode() != after.mode()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(invalid(
            "Share token file changed during the read; retry explicitly",
        ));
    }
    let token = text.trim();
    if token.is_empty()
        || token.len() > MAX_TOKEN_BYTES
        || token
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(invalid(
            "Share token must contain one nonempty token of at most 512 bytes",
        ));
    }
    Ok(token.to_owned())
}
