//! The backend owns one configuration file, shared by TOML and the web API.
use anyhow::{Context, Result};
use atomic_write_file::AtomicWriteFile;
use prysm_contract::Config;
use std::{io::Write, path::PathBuf};

pub(super) async fn save(path: PathBuf, config: Config) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let mut file =
            AtomicWriteFile::open(&path).context("cannot prepare configuration write")?;
        file.write_all(toml::to_string_pretty(&config)?.as_bytes())?;
        file.commit().context("cannot replace configuration")?;
        #[cfg(unix)]
        std::fs::File::open(
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new(".")),
        )?
        .sync_all()?;
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}
