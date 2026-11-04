use anyhow::Result;

pub fn capture_lock(path: &std::path::Path) -> Result<nix::fcntl::Flock<std::fs::File>> {
    use anyhow::Context;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .context("cannot open capture lock")?;
    nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
        .map_err(|(_, error)| error)
        .context("another Prysm capturer is already running")
}

#[cfg(test)]
mod tests {
    #[test]
    fn capture_is_exclusive_until_the_previous_process_releases_its_lock() {
        let path = std::env::temp_dir().join(format!("prysm-capture-{}.lock", std::process::id()));
        let lock = super::capture_lock(&path).unwrap();
        assert!(super::capture_lock(&path).is_err());
        drop(lock);
        let lock = super::capture_lock(&path).unwrap();
        drop(lock);
        std::fs::remove_file(path).unwrap();
    }
}
